use std::collections::VecDeque;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tempfile::TempDir;

use crate::branches::Repo;
use crate::commands::RepoLocks;
use crate::console::Console;
use crate::settings::CleanTree;
use crate::snapshots;
use crate::task::{Runner, TaskContext, TaskKind};

// Typical damage from a cut: garbage, an empty file, a truncated hash or a symbolic reference
// to something that does not exist (git follows it and sees the missing name, not the branch).
pub const DAMAGED_REFS: [&[u8]; 6] = [
    b"garbage\n",
    b"",
    b"abc123\n",
    b"ref: garbage\n",
    b"ref: refs/hea",
    b"ref: refs/heads/gone\n",
];

// Every git the tests run sees none of this machine's configuration: not the system's
// (`GIT_CONFIG_NOSYSTEM`), as the user's an empty file of the tests' own (`GIT_CONFIG_GLOBAL`), and
// as `XDG_CONFIG_HOME` an empty folder of the tests' own, so that the default `core.excludesFile`
// and `core.attributesFile` (`~/.config/git/ignore`, `attributes`) are not read either. `HOME`
// stays as it is. `git::command` calls it under `cfg(test)`, so the code under test does too.
pub fn isolate(command: &mut Command) -> &mut Command {
    let (config, xdg) = isolated_places();
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", config)
        .env("XDG_CONFIG_HOME", xdg)
}

// `git`, isolated: what every test module spawns instead of `Command::new("git")`.
pub fn git_command() -> Command {
    let mut command = Command::new("git");
    isolate(&mut command);
    command
}

// One file and one folder per temporary folder, created if missing and never truncated, so test
// processes that run at once share them safely. Something in them means a test wrote a
// `--global` setting: every test after it would read that, so the run stops instead.
pub fn isolated_places() -> (&'static Path, &'static Path) {
    static PLACES: OnceLock<(PathBuf, PathBuf)> = OnceLock::new();
    let (file, xdg) = PLACES.get_or_init(|| {
        let dir = std::env::temp_dir().join("ramify-tests-git");
        let xdg = dir.join("empty-xdg-config");
        fs::create_dir_all(&xdg).unwrap();
        let file = dir.join("empty-global-config");
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file)
            .unwrap();
        let size = fs::metadata(&file).unwrap().len();
        assert_eq!(
            size,
            0,
            "{} must be empty: a test wrote a --global setting into it",
            file.display()
        );
        let entries = fs::read_dir(&xdg).unwrap().count();
        assert_eq!(
            entries,
            0,
            "{} must be empty: a test wrote a --global setting into it",
            xdg.display()
        );
        (file, xdg)
    });
    (file, xdg)
}

// Linear congruential generator: a test depends on no crate and does not change between runs.
pub struct Random(pub u64);

impl Random {
    pub fn below(&mut self, limit: u32) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) % limit as u64) as u32
    }
}

// What runs here has a real `TaskContext`, and the result goes back to the test.
pub fn in_task<T: Send + 'static>(
    runner: &Runner,
    repo: &Path,
    work: impl FnOnce(&TaskContext) -> T + Send + 'static,
) -> T {
    in_task_as(runner, repo, TaskKind::Write, work)
}

// The same with another kind: a task that takes the write lock only for pieces of its work.
pub fn in_task_as<T: Send + 'static>(
    runner: &Runner,
    repo: &Path,
    kind: TaskKind,
    work: impl FnOnce(&TaskContext) -> T + Send + 'static,
) -> T {
    let (send, receive) = mpsc::channel();
    runner.start(repo, kind, "test".to_string(), move |ctx| {
        send.send(work(ctx)).unwrap();
        Ok(())
    });
    receive.recv_timeout(Duration::from_secs(60)).unwrap()
}

// `_dir` is the temporary folder it owns and `root` its work tree. They are the same folder for a
// repository of its own; a linked worktree lives in a folder inside `_dir`, and a submodule lives
// inside its parent while `_dir` holds its bare remote.
pub struct TempRepo {
    _dir: TempDir,
    root: PathBuf,
}

impl TempRepo {
    pub fn new() -> Self {
        Self::init(TempDir::new().unwrap())
    }

    // To test somewhere else (for example, a case-sensitive volume).
    pub fn new_in(base: &Path) -> Self {
        Self::init(TempDir::new_in(base).unwrap())
    }

    fn init(dir: TempDir) -> Self {
        let root = dir.path().to_path_buf();
        let repo = Self { _dir: dir, root };
        repo.git(&["init", "-q", "-b", "main"]);
        repo.configure();
        repo
    }

    fn configure(&self) {
        self.git(&["config", "user.name", "Test"]);
        self.git(&["config", "user.email", "test@example.com"]);
        self.git(&["config", "commit.gpgsign", "false"]);
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    // A linked worktree of this repository, from `git worktree add`, at `<temporary folder>/<name>`
    // and on a new branch `<name>` that starts at HEAD. It shares this repository's config.
    pub fn worktree(&self, name: &str) -> TempRepo {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join(name);
        self.git(&["worktree", "add", "-q", "-b", name, &root.to_string_lossy()]);
        TempRepo { _dir: dir, root }
    }

    // A submodule at `path` (relative to this work tree), added and committed here. Its remote
    // is a local bare repository with one commit (`sub.txt`), which the returned repository
    // keeps alive. git refuses local submodule URLs unless `protocol.file.allow` says so: only the
    // tests do that.
    pub fn with_submodule(&self, path: &str) -> TempRepo {
        let seed = TempRepo::new();
        seed.commit("sub.txt", b"sub\n", "first in the submodule");
        let bare = TempDir::new().unwrap();
        let status = git_command()
            .args(["clone", "-q", "--bare"])
            .arg(seed.path())
            .arg(bare.path())
            .status()
            .unwrap();
        assert!(status.success());
        let url = bare.path().to_string_lossy();
        self.git(&[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            &url,
            path,
        ]);
        self.git(&["commit", "-q", "-m", &format!("add submodule {path}")]);
        let submodule = TempRepo {
            _dir: bare,
            root: self.path().join(path),
        };
        submodule.configure();
        submodule
    }

    // A repository of its own at `path`, added here as a gitlink with no entry in `.gitmodules`
    // (a cloned folder added with `git add`), and committed.
    pub fn orphan_gitlink(&self, path: &str) {
        let folder = self.path().join(path);
        fs::create_dir_all(&folder).unwrap();
        let inner = TempRepo {
            _dir: TempDir::new().unwrap(),
            root: folder,
        };
        inner.git(&["init", "-q", "-b", "main"]);
        inner.configure();
        inner.git(&["commit", "-q", "--allow-empty", "-m", "inside"]);
        self.git(&["add", path]);
        self.git(&["commit", "-q", "-m", &format!("add the gitlink {path}")]);
    }

    // Whether `name` exists in this work tree's git folder, wherever git keeps it.
    pub fn in_git_dir(&self, name: &str) -> bool {
        let git_dir = self.git(&["rev-parse", "--git-dir"]);
        self.path()
            .join(git_dir.strip_suffix('\n').unwrap_or(&git_dir))
            .join(name)
            .exists()
    }

    pub fn commit(&self, file: &str, contents: &[u8], message: &str) -> String {
        let target = self.path().join(file);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, contents).unwrap();
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", message]);
        self.git(&["rev-parse", "HEAD"]).trim().to_string()
    }

    // Author and committer with that date: `--date-order` and the ordering depend on it.
    pub fn commit_dated(&self, file: &str, message: &str, date: &str) {
        fs::write(self.path().join(file), message).unwrap();
        self.git(&["add", "--", file]);
        self.git_dated(&["commit", "-q", "-m", message], date);
    }

    pub fn merge_dated(&self, branch: &str, message: &str, date: &str) {
        self.git_dated(&["merge", "-q", "--no-ff", "-m", message, branch], date);
    }

    fn git_dated(&self, args: &[&str], date: &str) {
        let status = git_command()
            .args(args)
            .current_dir(self.path())
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date)
            .status()
            .unwrap();
        assert!(status.success());
    }

    // Leaves a merge of `other` into `main` stopped at a content conflict in `file`. Git's exit
    // code does not count (a merge with a conflict exits with 1): what is checked is that the
    // merge was left half-done, so that a merge that does not conflict does not go unnoticed.
    pub fn conflict(&self, file: &str) {
        self.commit(file, b"base\n", "base");
        self.git(&["checkout", "-q", "-b", "other"]);
        self.commit(file, b"other\n", "on other");
        self.git(&["checkout", "-q", "main"]);
        self.commit(file, b"main\n", "on main");
        let merge = git_command()
            .current_dir(self.path())
            .args(["merge", "-q", "other"])
            .output()
            .unwrap();
        assert!(
            self.path().join(".git/MERGE_HEAD").exists(),
            "the merge did not end in a conflict: {}",
            String::from_utf8_lossy(&merge.stderr)
        );
    }

    pub fn git(&self, args: &[&str]) -> String {
        let output = git_command()
            .current_dir(self.path())
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    // Commits signed with an SSH key of its own (`ssh-keygen` ships with macOS, gpg may not), and
    // `log.showSignature` on in this repository: `log` and `show` then print a signature check
    // (`No signature`, since no allowed signers are set) before each record.
    pub fn sign_commits(&self) {
        let key = self.path().join(".git/test-signing-key");
        if !key.exists() {
            let status = Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                .arg(&key)
                .status()
                .unwrap();
            assert!(status.success());
        }
        self.git(&["config", "gpg.format", "ssh"]);
        self.git(&["config", "user.signingkey", &key.to_string_lossy()]);
        self.git(&["config", "commit.gpgsign", "true"]);
        self.git(&["config", "log.showSignature", "true"]);
    }
}

// A throwaway repository with its own data folder and a real task for each command: what the
// tests of long operations (merge, rebase, abort) need.
pub struct RepoHarness {
    pub repo: TempRepo,
    pub data: TempDir,
    console: Arc<Console>,
    pub runner: Runner,
}

impl RepoHarness {
    pub fn new() -> Self {
        Self::with_binary(None)
    }

    // With a fake `git`, to provoke what the real one does not give.
    pub fn with_binary(binary: Option<std::path::PathBuf>) -> Self {
        let console = Arc::<Console>::default();
        let runner = Runner::with(
            Arc::<RepoLocks>::default(),
            console.clone(),
            binary,
            Duration::from_secs(5),
        );
        let harness = Self {
            repo: TempRepo::new(),
            data: TempDir::new().unwrap(),
            console,
            runner,
        };
        // The tests written before the shelf saver speak of the stash; the ones of the shelf ask for it.
        harness.clean_tree(CleanTree::Stash);
        harness
    }

    // `Clean working tree using:` of this repository.
    pub fn clean_tree(&self, clean_tree: CleanTree) {
        crate::settings::save_clean_tree(self.data.path(), clean_tree).unwrap();
    }

    pub fn run<T: Send + 'static>(&self, work: impl FnOnce(&Repo) -> T + Send + 'static) -> T {
        let path = self.repo.path().to_path_buf();
        let data = self.data.path().to_path_buf();
        in_task(&self.runner, self.repo.path(), move |ctx| {
            work(&Repo {
                ctx,
                path: &path,
                data: &data,
            })
        })
    }

    pub fn path(&self) -> &Path {
        self.repo.path()
    }

    pub fn write(&self, file: &str, contents: &str) {
        let target = self.path().join(file);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, contents).unwrap();
    }

    pub fn read(&self, file: &str) -> String {
        fs::read_to_string(self.path().join(file)).unwrap()
    }

    pub fn git(&self, args: &[&str]) -> String {
        self.repo.git(args)
    }

    // A command that must fail (a merge with a conflict): returns its output.
    pub fn git_fails(&self, args: &[&str]) -> String {
        let output = git_command()
            .current_dir(self.path())
            .args(args)
            .env("GIT_EDITOR", "true")
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "git {args:?} was supposed to fail"
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
            + &String::from_utf8_lossy(&output.stderr)
    }

    pub fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"]).trim().to_string()
    }

    pub fn in_git_dir(&self, name: &str) -> bool {
        self.repo.in_git_dir(name)
    }

    pub fn ran(&self, command: &str) -> bool {
        let lines = self.console.lines();
        lines
            .iter()
            .any(|line| line.command && line.text.ends_with(command))
    }

    pub fn saved_paths(&self) -> Vec<String> {
        let list = snapshots::list(self.data.path());
        list.first()
            .map(|snapshot| snapshot.entries.iter().map(|e| e.path.clone()).collect())
            .unwrap_or_default()
    }

    pub fn snapshots(&self) -> usize {
        snapshots::list(self.data.path()).len()
    }

    // `main` and `side` start from `a.txt = base` and both change it: joining them has a
    // conflict. It stays on `main`.
    pub fn diverged(&self) {
        self.repo.commit("a.txt", b"base\n", "base");
        self.git(&["checkout", "-q", "-b", "side"]);
        self.repo.commit("a.txt", b"side\n", "side");

        self.git(&["checkout", "-q", "main"]);
        self.repo.commit("a.txt", b"main\n", "main");
    }
}

// One answer of the fake GitHub: what request it expects (method and path with query) and what
// it sends back.
pub struct Reply {
    pub method: String,
    pub target: String,
    pub status: u16,
    pub reason: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Reply {
    pub fn new(
        method: &str,
        target: &str,
        status: u16,
        reason: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> Self {
        Self {
            method: method.to_string(),
            target: target.to_string(),
            status,
            reason: reason.to_string(),
            headers: headers
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
            body: body.to_string(),
        }
    }

    // A JSON body, with the content type GitHub sends.
    pub fn json(method: &str, target: &str, status: u16, reason: &str, body: &str) -> Self {
        Self::new(
            method,
            target,
            status,
            reason,
            &[("Content-Type", "application/json; charset=utf-8")],
            body,
        )
    }
}

// A request the fake GitHub received.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    pub target: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        crate::hosting::http::header(&self.headers, name)
    }
}

#[derive(Default)]
struct FakeState {
    script: VecDeque<Reply>,
    requests: Vec<Recorded>,
    unexpected: Vec<String>,
}

// GitHub on `127.0.0.1:0`, for the tests: a thread answers the queued replies in order, one per
// connection, and records every request. A request that is not the next one queued is answered
// with a 500. Both that and a queued reply nobody asked for fail the test when the fake is
// dropped, or earlier at `finish`.
pub struct FakeGitHub {
    address: SocketAddr,
    state: Arc<Mutex<FakeState>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FakeGitHub {
    pub fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let state = Arc::new(Mutex::new(FakeState::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let state = state.clone();
            let stop = stop.clone();
            thread::spawn(move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    if let Ok(stream) = stream {
                        fake_answer(stream, &state);
                    }
                }
            })
        };
        Self {
            address,
            state,
            stop,
            thread: Some(thread),
        }
    }

    // `http://127.0.0.1:<port>`, with no slash at the end.
    pub fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    pub fn reply(&self, reply: Reply) -> &Self {
        self.state.lock().unwrap().script.push_back(reply);
        self
    }

    pub fn script(&self, replies: Vec<Reply>) -> &Self {
        for reply in replies {
            self.reply(reply);
        }
        self
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.state.lock().unwrap().requests.clone()
    }

    // Fails unless every queued reply was asked for and nothing else was.
    pub fn finish(&self) {
        if let Some(problem) = self.problem() {
            panic!("{problem}");
        }
    }

    // Takes what went wrong, so that `Drop` does not report it a second time.
    fn problem(&self) -> Option<String> {
        let mut state = self.state.lock().unwrap();
        let unexpected = std::mem::take(&mut state.unexpected);
        let left: Vec<String> = state
            .script
            .drain(..)
            .map(|reply| format!("{} {}", reply.method, reply.target))
            .collect();
        match (unexpected.is_empty(), left.is_empty()) {
            (true, true) => None,
            _ => Some(format!(
                "the fake GitHub got unexpected requests {unexpected:?} and was never asked for {left:?}"
            )),
        }
    }
}

impl Drop for FakeGitHub {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wakes the thread up from `accept`.
        let _ = TcpStream::connect(self.address);
        if let Some(worker) = self.thread.take() {
            let _ = worker.join();
        }
        if let Some(problem) = self.problem() {
            if !thread::panicking() {
                panic!("{problem}");
            }
        }
    }
}

fn fake_answer(stream: TcpStream, state: &Mutex<FakeState>) {
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        // The wake-up connection of `Drop`, or a client that gave up.
        return;
    }
    let mut parts = line.trim_end().splitn(3, ' ');
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default().to_string();
    let mut headers = Vec::new();
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap_or(0) == 0 {
            break;
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }
    let length = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0; length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let recorded = Recorded {
        method,
        target,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    };

    let reply = {
        let mut state = state.lock().unwrap();
        let next = state.script.pop_front();
        let reply = match next {
            Some(reply) if reply.method == recorded.method && reply.target == recorded.target => {
                reply
            }
            other => {
                let expected = other
                    .map(|reply| format!("{} {}", reply.method, reply.target))
                    .unwrap_or_else(|| "nothing".to_string());
                state.unexpected.push(format!(
                    "{} {} (expected {expected})",
                    recorded.method, recorded.target
                ));
                Reply::new("", "", 500, "Unexpected", &[], "unexpected request")
            }
        };
        state.requests.push(recorded);
        reply
    };

    let mut answer = format!("HTTP/1.1 {} {}\r\n", reply.status, reply.reason);
    for (name, value) in &reply.headers {
        answer.push_str(&format!("{name}: {value}\r\n"));
    }
    answer.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        reply.body.len()
    ));
    answer.push_str(&reply.body);
    let mut stream = reader.into_inner();
    let _ = stream.write_all(answer.as_bytes());
    let _ = stream.flush();
}
