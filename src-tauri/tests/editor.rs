// The editor client is the app's binary: these tests launch the real one, as git would,
// against a real server and disposable repositories.
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ramify_lib::branch_info::BranchSync;
use ramify_lib::branches::Repo;
use ramify_lib::commands::RepoLocks;
use ramify_lib::console::Console;
use ramify_lib::editor::{
    Ask, EditKind, EditorHandler, Form, Prompt, Request, Server, Trivial, REBASE_CONFIG,
};
use ramify_lib::git::GitError;
use ramify_lib::pull;
use ramify_lib::rebase::{self, Outcome as Rebase};
use ramify_lib::task::{GitOutput, Runner, TaskContext, TaskId, TaskKind};
use ramify_lib::update::Outcome;
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(30);
const CLIENT: &str = env!("CARGO_BIN_EXE_ramify");

fn git_in(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(dir)
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

struct Scratch {
    dir: TempDir,
}

impl Scratch {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        git_in(dir.path(), &["init", "-q", "-b", "main"]);
        Self::configure(dir.path());
        Self { dir }
    }

    fn configure(path: &Path) {
        git_in(path, &["config", "user.name", "Test"]);
        git_in(path, &["config", "user.email", "test@example.com"]);
        git_in(path, &["config", "commit.gpgsign", "false"]);
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn git(&self, args: &[&str]) -> String {
        git_in(self.path(), args)
    }

    fn commit(&self, message: &str) -> String {
        fs::write(self.path().join(format!("{message}.txt")), message).unwrap();
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", message]);
        self.git(&["rev-parse", "HEAD"]).trim().to_string()
    }

    fn subjects(&self) -> Vec<String> {
        self.git(&["log", "--format=%s"])
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn git_dir(&self) -> PathBuf {
        PathBuf::from(self.git(&["rev-parse", "--absolute-git-dir"]).trim())
    }

    fn in_git_dir(&self, name: &str) -> bool {
        self.git_dir().join(name).exists()
    }

    // Whether the command succeeded. A rebase that stops at a conflict does not.
    fn git_status(&self, args: &[&str]) -> bool {
        Command::new("git")
            .current_dir(self.path())
            .args(args)
            .env("GIT_EDITOR", "true")
            .output()
            .unwrap()
            .status
            .success()
    }
}

// A handler that records what it sees and answers whatever the test says.
type Reply = Box<dyn Fn(EditKind, &str) -> Option<String> + Send + Sync>;

struct Scripted {
    seen: Mutex<Vec<(EditKind, String)>>,
    reply: Reply,
}

impl Scripted {
    fn new(reply: impl Fn(EditKind, &str) -> Option<String> + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self {
            seen: Mutex::default(),
            reply: Box::new(reply),
        })
    }

    fn seen(&self) -> Vec<(EditKind, String)> {
        self.seen.lock().unwrap().clone()
    }
}

impl EditorHandler for Scripted {
    fn edit(&self, kind: EditKind, text: &str) -> Option<String> {
        self.seen.lock().unwrap().push((kind, text.to_string()));
        (self.reply)(kind, text)
    }
}

// Changes the action of the line whose subject is `subject`.
fn change_action(todo: &str, subject: &str, action: &str) -> String {
    todo.lines()
        .map(|line| match line.strip_prefix("pick ") {
            Some(rest) if line.ends_with(&format!(" {subject}")) => format!("{action} {rest}\n"),
            _ => format!("{line}\n"),
        })
        .collect()
}

type Asked = (u64, EditKind, String);

struct Fixture {
    server: Server,
    runner: Runner,
    requests: Receiver<Asked>,
    // How each request asked the window to show it, in the same order.
    forms: Receiver<Form>,
    dismissed: Receiver<u64>,
}

impl Fixture {
    // `shown` is what the app answers to "is there anyone to show it to?".
    fn new(shown: bool) -> Self {
        let (send, requests) = mpsc::channel();
        let send = Mutex::new(send);
        let (form, forms) = mpsc::channel();
        let form = Mutex::new(form);
        let (gone, dismissed) = mpsc::channel();
        let gone = Mutex::new(gone);
        let server = Server::start_with_program(
            PathBuf::from(CLIENT),
            move |id, prompt: Prompt| {
                if !shown {
                    return false;
                }
                form.lock().unwrap().send(prompt.form).unwrap();
                let asked = (id, prompt.kind, prompt.text);
                send.lock().unwrap().send(asked).is_ok()
            },
            move |id| gone.lock().unwrap().send(id).unwrap(),
        )
        .unwrap();
        let runner = Runner::new(Arc::new(RepoLocks::default()), Arc::new(Console::default()));
        runner.use_editor(server.handle());
        Self {
            server,
            runner,
            requests,
            forms,
            dismissed,
        }
    }

    fn in_task<T: Send + 'static>(
        &self,
        repo: &Path,
        work: impl FnOnce(&TaskContext) -> T + Send + 'static,
    ) -> Receiver<T> {
        self.start_task(repo, work).1
    }

    fn start_task<T: Send + 'static>(
        &self,
        repo: &Path,
        work: impl FnOnce(&TaskContext) -> T + Send + 'static,
    ) -> (TaskId, Receiver<T>) {
        self.start_task_as(repo, TaskKind::Write, work)
    }

    // `git_pull` starts a read task: the pull takes the write lock itself, around `git pull`.
    fn start_pull<T: Send + 'static>(
        &self,
        repo: &Path,
        work: impl FnOnce(&TaskContext) -> T + Send + 'static,
    ) -> (TaskId, Receiver<T>) {
        self.start_task_as(repo, TaskKind::Read, work)
    }

    fn start_task_as<T: Send + 'static>(
        &self,
        repo: &Path,
        kind: TaskKind,
        work: impl FnOnce(&TaskContext) -> T + Send + 'static,
    ) -> (TaskId, Receiver<T>) {
        let (send, receive) = mpsc::channel();
        let id = self
            .runner
            .start(repo, kind, "test".to_string(), move |ctx| {
                send.send(work(ctx)).unwrap();
                Ok(())
            });
        (id, receive)
    }

    fn run<T: Send + 'static>(
        &self,
        repo: &Path,
        work: impl FnOnce(&TaskContext) -> T + Send + 'static,
    ) -> T {
        self.in_task(repo, work)
            .recv_timeout(WAIT)
            .expect("the task did not finish")
    }

    fn rebase(
        &self,
        repo: &Path,
        base: &str,
        handler: Arc<dyn EditorHandler>,
    ) -> Result<GitOutput, GitError> {
        let base = base.to_string();
        self.run(repo, move |ctx| {
            let args: Vec<&str> = REBASE_CONFIG
                .iter()
                .copied()
                .chain(["rebase", "-i", &base])
                .collect();
            ctx.git_editing(&args, handler)
        })
    }

    // Launches the real client with a registration's environment, as git would.
    fn client(&self, handler: Arc<dyn EditorHandler>, git_dir: PathBuf, file: &Path) -> Command {
        let registration = self.server.register(handler, git_dir);
        let mut command = Command::new(CLIENT);
        registration.apply(&mut command);
        command.arg("--ramify-editor").arg(file);
        // The registration lives as long as the test's command lives.
        std::mem::forget(registration);
        command
    }
}

fn code(output: Output) -> Option<i32> {
    output.status.code()
}

#[test]
fn interactive_rebase_uses_the_handlers_todo() {
    let fixture = Fixture::new(true);
    let repo = Scratch::new();
    let base = repo.commit("base");
    for subject in ["A", "B", "C"] {
        repo.commit(subject);
    }
    let handler = Scripted::new(|kind, text| {
        assert_eq!(kind, EditKind::Todo);
        Some(change_action(text, "B", "drop"))
    });

    fixture
        .rebase(repo.path(), &base, handler.clone())
        .expect("the rebase must finish");

    assert_eq!(repo.subjects(), ["C", "A", "base"]);
    let seen = handler.seen();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert!(seen[0].1.contains("pick "), "{seen:?}");
    // With IntelliJ's comment character, the rebase help arrives with it in front.
    assert!(
        seen[0].1.lines().any(|line| line.starts_with('\u{1}')),
        "{seen:?}"
    );
}

#[test]
fn cancelling_handler_leaves_the_repository_unchanged() {
    let fixture = Fixture::new(true);
    let repo = Scratch::new();
    let base = repo.commit("base");
    repo.commit("A");
    let head = repo.commit("B");
    let tree = repo.git(&["rev-parse", "HEAD^{tree}"]);

    let result = fixture.rebase(repo.path(), &base, Scripted::new(|_, _| None));

    assert!(result.is_err());
    assert!(!repo.in_git_dir("rebase-merge"));
    assert!(!repo.in_git_dir("rebase-apply"));
    assert_eq!(repo.git(&["rev-parse", "HEAD"]).trim(), head);
    assert_eq!(repo.git(&["rev-parse", "HEAD^{tree}"]), tree);
    assert_eq!(repo.git(&["status", "--porcelain"]), "");
}

#[test]
fn users_sequence_editor_does_not_win() {
    let fixture = Fixture::new(true);
    let repo = Scratch::new();
    let base = repo.commit("base");
    repo.commit("A");
    repo.git(&["config", "sequence.editor", "false"]);
    repo.git(&["config", "core.editor", "false"]);
    let handler = Scripted::new(|_, text| Some(text.to_string()));

    fixture
        .rebase(repo.path(), &base, handler.clone())
        .expect("the rebase must finish");

    assert_eq!(handler.seen().len(), 1);
    assert_eq!(handler.seen()[0].0, EditKind::Todo);
}

#[test]
fn reword_message_arrives_as_message() {
    let fixture = Fixture::new(true);
    let repo = Scratch::new();
    let base = repo.commit("base");
    repo.commit("A");
    repo.commit("B");
    let handler = Scripted::new(|kind, text| match kind {
        EditKind::Todo => Some(change_action(text, "B", "reword")),
        EditKind::Message => Some("B rewritten\n".to_string()),
    });

    fixture
        .rebase(repo.path(), &base, handler.clone())
        .expect("the rebase must finish");

    assert_eq!(repo.subjects(), ["B rewritten", "A", "base"]);
    let seen = handler.seen();
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert_eq!(seen[0].0, EditKind::Todo);
    assert_eq!(seen[1].0, EditKind::Message);
    assert!(seen[1].1.starts_with("B\n"), "{seen:?}");
}

#[test]
fn client_writes_inside_the_git_directory() {
    let fixture = Fixture::new(true);
    let repo = Scratch::new();
    let file = repo.git_dir().join("COMMIT_EDITMSG");
    fs::write(&file, "old\n").unwrap();
    let handler = Scripted::new(|kind, text| {
        assert_eq!((kind, text), (EditKind::Message, "old\n"));
        Some("new\n".to_string())
    });

    let output = fixture
        .client(handler, repo.git_dir(), &file)
        .output()
        .unwrap();

    assert_eq!(code(output), Some(0));
    assert_eq!(fs::read_to_string(file).unwrap(), "new\n");
}

#[test]
fn nothing_is_written_outside_the_git_directory() {
    let fixture = Fixture::new(true);
    let repo = Scratch::new();
    let elsewhere = TempDir::new().unwrap();
    let file = elsewhere.path().join("x");
    fs::write(&file, "intact\n").unwrap();
    let handler = Scripted::new(|_, _| Some("changed\n".to_string()));

    let output = fixture
        .client(handler.clone(), repo.git_dir(), &file)
        .output()
        .unwrap();

    assert_eq!(code(output), Some(1));
    assert_eq!(fs::read_to_string(file).unwrap(), "intact\n");
    assert!(
        handler.seen().is_empty(),
        "the handler must not even see it"
    );
}

#[test]
fn a_link_inside_the_git_directory_does_not_redirect_the_write() {
    let fixture = Fixture::new(true);
    let repo = Scratch::new();
    let elsewhere = TempDir::new().unwrap();
    let target = elsewhere.path().join("x");
    fs::write(&target, "intact\n").unwrap();
    let link = repo.git_dir().join("link");
    symlink(&target, &link).unwrap();
    let handler = Scripted::new(|_, _| Some("changed\n".to_string()));

    let output = fixture
        .client(handler.clone(), repo.git_dir(), &link)
        .output()
        .unwrap();

    assert_eq!(code(output), Some(1));
    assert_eq!(fs::read_to_string(target).unwrap(), "intact\n");
    assert!(handler.seen().is_empty());
}

#[test]
fn foreign_token_is_rejected() {
    let fixture = Fixture::new(true);
    let repo = Scratch::new();
    let file = repo.git_dir().join("COMMIT_EDITMSG");
    fs::write(&file, "old\n").unwrap();
    let handler = Scripted::new(|_, _| Some("changed\n".to_string()));
    let mut command = fixture.client(handler.clone(), repo.git_dir(), &file);

    let output = command
        .env("RAMIFY_EDITOR_TOKEN", "0".repeat(64))
        .output()
        .unwrap();

    assert_eq!(code(output), Some(1));
    assert_eq!(fs::read_to_string(file).unwrap(), "old\n");
    assert!(handler.seen().is_empty());
}

#[test]
fn without_a_window_to_answer_it_fails_immediately() {
    let fixture = Fixture::new(false);
    let repo = Scratch::new();
    let base = repo.commit("base");
    let head = repo.commit("A");
    let started = Instant::now();

    let result = {
        let base = base.clone();
        fixture.run(repo.path(), move |ctx| {
            let args: Vec<&str> = REBASE_CONFIG
                .iter()
                .copied()
                .chain(["rebase", "-i", &base])
                .collect();
            ctx.git_editing(&args, ctx.asking_editor())
        })
    };

    assert!(result.is_err());
    assert!(started.elapsed() < Duration::from_secs(1), "git waited");
    assert_eq!(repo.git(&["rev-parse", "HEAD"]).trim(), head);
    assert!(!repo.in_git_dir("rebase-merge"));
}

// `Ask` takes the app the text without the comment lines and writes whatever it answers.
#[test]
fn ask_asks_the_window_and_writes_the_answer() {
    let fixture = Fixture::new(true);
    let repo = Scratch::new();
    let base = repo.commit("base");
    repo.commit("A");
    repo.commit("B");
    let base_arg = base.clone();
    let done = fixture.in_task(repo.path(), move |ctx| {
        let args: Vec<&str> = REBASE_CONFIG
            .iter()
            .copied()
            .chain(["rebase", "-i", &base_arg])
            .collect();
        ctx.git_editing(&args, ctx.asking_editor())
    });

    let (id, kind, text) = fixture.requests.recv_timeout(WAIT).unwrap();
    assert_eq!(kind, EditKind::Todo);
    assert!(text.starts_with("pick "), "{text:?}");
    assert!(
        text.lines().all(|line| !line.starts_with('\u{1}')),
        "{text:?}"
    );
    fixture
        .server
        .answer(id, Some(change_action(&text, "A", "drop")));

    done.recv_timeout(WAIT)
        .unwrap()
        .expect("the rebase finishes");
    assert_eq!(repo.subjects(), ["B", "base"]);
}

struct Remote {
    _origin: TempDir,
    colleague: Scratch,
    repo: Scratch,
}

impl Remote {
    fn new() -> Self {
        let origin = TempDir::new().unwrap();
        git_in(origin.path(), &["init", "-q", "--bare", "-b", "main"]);
        let url = origin.path().to_str().unwrap();
        let colleague = Scratch::new();
        colleague.commit("base");
        colleague.git(&["remote", "add", "origin", url]);
        colleague.git(&["push", "-q", "origin", "main"]);
        let clone = TempDir::new().unwrap();
        git_in(clone.path(), &["clone", "-q", url, "."]);
        Scratch::configure(clone.path());
        Self {
            _origin: origin,
            colleague,
            repo: Scratch { dir: clone },
        }
    }
}

// Fixes the finding "pull.rebase=interactive without an editor": git opened the user's editor without a
// terminal and the task never finished.
#[test]
fn pull_rebase_interactive_does_not_wait_for_a_terminal() {
    let fixture = Fixture::new(true);
    let remote = Remote::new();
    remote.repo.commit("own");
    remote.colleague.commit("from outside");
    remote.colleague.git(&["push", "-q", "origin", "main"]);
    remote.repo.git(&["config", "pull.rebase", "interactive"]);
    let (path, data) = (remote.repo.path().to_path_buf(), TempDir::new().unwrap());
    let data_path = data.path().to_path_buf();
    let (_, done) = fixture.start_pull(remote.repo.path(), move |ctx| {
        let repo = Repo {
            ctx,
            path: &path,
            data: &data_path,
        };
        pull::pull(&repo, &BranchSync::default(), "origin", "main", &[], || {
            Ok(())
        })
    });

    let (id, kind, text) = fixture.requests.recv_timeout(WAIT).expect("no request");
    assert_eq!(kind, EditKind::Todo);
    assert!(text.contains("pick ") && text.contains("own"), "{text:?}");
    fixture.server.answer(id, Some(text));

    let result = done.recv_timeout(WAIT).expect("the pull did not finish");
    assert_eq!(result.outcome, Outcome::Success);
    assert_eq!(remote.repo.subjects(), ["own", "from outside", "base"]);
}

fn interactive_rebase(ctx: &TaskContext, base: &str) -> Result<GitOutput, GitError> {
    let args: Vec<&str> = REBASE_CONFIG
        .iter()
        .copied()
        .chain(["rebase", "-i", base])
        .collect();
    ctx.git_editing(&args, Scripted::new(|_, text| Some(text.to_string())))
}

// Without a server there is no one to answer, and the user's editor has no terminal: git must fail.
#[test]
fn without_a_server_the_editor_refuses_and_git_fails() {
    let runner = Runner::new(Arc::new(RepoLocks::default()), Arc::new(Console::default()));
    let repo = Scratch::new();
    let base = repo.commit("base");
    repo.commit("A");
    repo.git(&["config", "core.editor", "sleep 60"]);
    let started = Instant::now();

    let (send, done) = mpsc::channel();
    runner.start(
        repo.path(),
        TaskKind::Write,
        "test".to_string(),
        move |ctx| {
            send.send(interactive_rebase(ctx, &base)).unwrap();
            Ok(())
        },
    );

    assert!(done.recv_timeout(WAIT).unwrap().is_err());
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(!repo.in_git_dir("rebase-merge"));
}

#[test]
fn quiet_task_does_not_open_the_editor() {
    let fixture = Fixture::new(true);
    let repo = Scratch::new();
    let base = repo.commit("base");
    repo.commit("A");

    let result = fixture
        .runner
        .quiet(repo.path(), WAIT, |ctx| interactive_rebase(ctx, &base));

    assert!(result.is_err());
    assert!(
        fixture.requests.try_recv().is_err(),
        "no one is waiting for it"
    );
    assert!(!repo.in_git_dir("rebase-merge"));
}

// The list is redone by itself and the message is asked to the window, which is what a reword does.
struct RewordThenAsk(Ask);

impl EditorHandler for RewordThenAsk {
    fn edit(&self, kind: EditKind, text: &str) -> Option<String> {
        match kind {
            EditKind::Todo => Some(change_action(text, "B", "reword")),
            EditKind::Message => self.0.edit(kind, text),
        }
    }

    fn edit_request(&self, request: &Request, kind: EditKind, text: &str) -> Option<String> {
        match kind {
            EditKind::Todo => self.edit(kind, text),
            EditKind::Message => self.0.edit_request(request, kind, text),
        }
    }
}

#[test]
fn cancelling_the_task_closes_the_pending_request() {
    let fixture = Fixture::new(true);
    let repo = Scratch::new();
    let base = repo.commit("base");
    repo.commit("A");
    repo.commit("B");
    let handle = fixture.server.handle();
    let (task, done) = fixture.start_task(repo.path(), move |ctx| {
        let args: Vec<&str> = REBASE_CONFIG
            .iter()
            .copied()
            .chain(["rebase", "-i", &base])
            .collect();
        let handler = RewordThenAsk(Ask::new(Some(handle)));
        ctx.git_editing(&args, Arc::new(handler))
    });
    let (id, kind, _) = fixture.requests.recv_timeout(WAIT).expect("no request");
    assert_eq!(kind, EditKind::Message);

    fixture.runner.cancel(task);

    assert_eq!(fixture.dismissed.recv_timeout(WAIT), Ok(id));
    assert_eq!(
        done.recv_timeout(WAIT).unwrap().unwrap_err(),
        GitError::Cancelled
    );
    // The window answers late, with the dialog still on screen: it writes nothing and does not hang.
    fixture.server.answer(id, Some("late\n".to_string()));
    let message = repo.git_dir().join("COMMIT_EDITMSG");
    assert!(
        !message.exists() || !fs::read_to_string(message).unwrap().contains("late"),
        "a late answer was written"
    );
}

// A SIGTERM to git stopped in the editor leaves `rebase-merge/` halfway: on cancel the
// request is withdrawn so that the editor exits with 1 and git undoes what it started.
#[test]
fn cancelling_an_interactive_pull_with_the_todo_pending_leaves_no_rebase() {
    let fixture = Fixture::new(true);
    let remote = Remote::new();
    remote.repo.commit("own");
    remote.colleague.commit("from outside");
    remote.colleague.git(&["push", "-q", "origin", "main"]);
    remote.repo.git(&["config", "pull.rebase", "interactive"]);
    let head = remote.repo.git(&["rev-parse", "HEAD"]);
    let (path, data) = (remote.repo.path().to_path_buf(), TempDir::new().unwrap());
    let data_path = data.path().to_path_buf();
    let (task, done) = fixture.start_pull(remote.repo.path(), move |ctx| {
        let repo = Repo {
            ctx,
            path: &path,
            data: &data_path,
        };
        pull::pull(&repo, &BranchSync::default(), "origin", "main", &[], || {
            Ok(())
        })
    });
    let (id, kind, _) = fixture.requests.recv_timeout(WAIT).expect("no request");
    assert_eq!(kind, EditKind::Todo);
    let started = Instant::now();

    fixture.runner.cancel(task);

    assert_eq!(fixture.dismissed.recv_timeout(WAIT), Ok(id));
    let result = done.recv_timeout(WAIT).expect("the pull did not finish");
    assert_eq!(result.outcome, Outcome::Cancelled);
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "git did not exit by itself"
    );
    assert!(!remote.repo.in_git_dir("rebase-merge"));
    assert!(!remote.repo.in_git_dir("rebase-apply"));
    assert_eq!(remote.repo.git(&["rev-parse", "HEAD"]), head);
    assert_eq!(remote.repo.git(&["status", "--porcelain"]), "");
}

#[test]
fn cancelling_an_interactive_rebase_with_the_todo_pending_leaves_no_rebase() {
    let fixture = Fixture::new(true);
    let repo = Scratch::new();
    let base = repo.commit("base");
    repo.commit("A");
    let head = repo.commit("B");
    let tree = repo.git(&["rev-parse", "HEAD^{tree}"]);
    let (task, done) = fixture.start_task(repo.path(), move |ctx| {
        interactive_rebase_asking(ctx, &base)
    });
    let (id, kind, _) = fixture.requests.recv_timeout(WAIT).expect("no request");
    assert_eq!(kind, EditKind::Todo);

    fixture.runner.cancel(task);

    assert_eq!(fixture.dismissed.recv_timeout(WAIT), Ok(id));
    assert_eq!(
        done.recv_timeout(WAIT).unwrap().unwrap_err(),
        GitError::Cancelled
    );
    assert!(!repo.in_git_dir("rebase-merge"));
    assert_eq!(repo.git(&["rev-parse", "HEAD"]).trim(), head);
    assert_eq!(repo.git(&["rev-parse", "HEAD^{tree}"]), tree);
    assert_eq!(repo.git(&["status", "--porcelain"]), "");
}

fn interactive_rebase_asking(ctx: &TaskContext, base: &str) -> Result<GitOutput, GitError> {
    let args: Vec<&str> = REBASE_CONFIG
        .iter()
        .copied()
        .chain(["rebase", "-i", base])
        .collect();
    ctx.git_editing(&args, ctx.asking_editor())
}

// If git is not stopped in the editor, there is nothing to wait for: it is cancelled like any command.
#[test]
fn cancelling_without_a_pending_request_kills_git_immediately() {
    let fixture = Fixture::new(true);
    let repo = Scratch::new();
    repo.commit("base");
    let (task, done) = fixture.start_task(repo.path(), |ctx| {
        ctx.git_editing(&["-c", "alias.slow=!sleep 30", "slow"], Arc::new(Trivial))
    });
    std::thread::sleep(Duration::from_millis(500));
    let started = Instant::now();

    fixture.runner.cancel(task);

    assert_eq!(
        done.recv_timeout(WAIT).unwrap().unwrap_err(),
        GitError::Cancelled
    );
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "took too long to die"
    );
}

// A rebase of `side` onto `main` stopped at the conflict of its first commit; it stays on `side`.
fn stopped_rebase() -> Scratch {
    let repo = Scratch::new();
    fs::write(repo.path().join("a.txt"), "base\n").unwrap();
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-q", "-m", "base"]);
    repo.git(&["checkout", "-q", "-b", "side"]);
    fs::write(repo.path().join("a.txt"), "side\n").unwrap();
    repo.git(&["commit", "-qam", "clashes"]);
    repo.commit("follows");
    repo.git(&["checkout", "-q", "main"]);
    fs::write(repo.path().join("a.txt"), "main\n").unwrap();
    repo.git(&["commit", "-qam", "main"]);
    repo.git(&["checkout", "-q", "side"]);
    let stopped = repo.git_status(&["rebase", "main"]);
    assert!(!stopped, "the rebase should have stopped at the conflict");
    repo
}

// With a task's `Repo`: what `rebase` needs.
fn in_repo<T: Send + 'static>(
    fixture: &Fixture,
    repo: &Scratch,
    work: impl FnOnce(&Repo) -> T + Send + 'static,
) -> Receiver<T> {
    let path = repo.path().to_path_buf();
    fixture.in_task(repo.path(), move |ctx| {
        let data = TempDir::new().unwrap();
        work(&Repo {
            ctx,
            path: &path,
            data: data.path(),
        })
    })
}

fn resolve(repo: &Scratch) {
    fs::write(repo.path().join("a.txt"), "resolved\n").unwrap();
    repo.git(&["add", "a.txt"]);
}

#[test]
fn continue_after_resolving_finishes() {
    let fixture = Fixture::new(true);
    let repo = stopped_rebase();
    resolve(&repo);

    let done = in_repo(&fixture, &repo, |repo| {
        rebase::run(repo, &["rebase", "--continue"], Arc::new(Trivial))
    });

    assert_eq!(done.recv_timeout(WAIT).unwrap(), Ok(Rebase::Done));
    assert!(!repo.in_git_dir("rebase-merge"));
    assert_eq!(repo.subjects(), ["follows", "clashes", "main", "base"]);
}

#[test]
fn continue_that_asks_for_a_message_asks_it() {
    let fixture = Fixture::new(true);
    let repo = stopped_rebase();
    resolve(&repo);

    let done = in_repo(&fixture, &repo, rebase::continue_rebase);

    let (id, kind, text) = fixture.requests.recv_timeout(WAIT).unwrap();
    assert_eq!(kind, EditKind::Message);
    assert!(text.starts_with("clashes"), "{text:?}");
    assert!(
        text.lines().all(|line| !line.starts_with('\u{1}')),
        "{text:?}"
    );
    fixture
        .server
        .answer(id, Some("clashes, already resolved\n".to_string()));

    assert_eq!(done.recv_timeout(WAIT).unwrap(), Ok(Rebase::Done));
    assert_eq!(
        repo.subjects(),
        ["follows", "clashes, already resolved", "main", "base"]
    );
}

// Cancelling the message leaves the rebase stopped there, like IntelliJ (`wasUnstructuredEditorCancelled`).
#[test]
fn cancelling_the_continue_message_leaves_the_rebase_stopped() {
    let fixture = Fixture::new(true);
    let repo = stopped_rebase();
    resolve(&repo);

    let done = in_repo(&fixture, &repo, rebase::continue_rebase);
    let (id, ..) = fixture.requests.recv_timeout(WAIT).unwrap();
    fixture.server.answer(id, None);

    assert_eq!(done.recv_timeout(WAIT).unwrap(), Ok(Rebase::Editing));
    assert!(repo.in_git_dir("rebase-merge"));
}

// Cancelling the list starts nothing, and it is not a failure to notify.
#[test]
fn cancelling_the_list_does_not_start_the_rebase() {
    let fixture = Fixture::new(true);
    let repo = Scratch::new();
    let base = repo.commit("base");
    repo.commit("A");
    let head = repo.git(&["rev-parse", "HEAD"]);

    let done = in_repo(&fixture, &repo, move |repo| {
        rebase::run(repo, &["rebase", "-i", &base], repo.ctx.asking_editor())
    });
    let (id, kind, _) = fixture.requests.recv_timeout(WAIT).unwrap();
    assert_eq!(kind, EditKind::Todo);
    fixture.server.answer(id, None);

    assert_eq!(done.recv_timeout(WAIT).unwrap(), Err(GitError::Cancelled));
    assert!(!repo.in_git_dir("rebase-merge"));
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), head);
}

#[test]
fn an_edit_leaves_the_rebase_stopped_for_editing() {
    let fixture = Fixture::new(true);
    let repo = Scratch::new();
    let base = repo.commit("base");
    repo.commit("A");
    repo.commit("B");
    let editing = Scripted::new(|_, text| Some(change_action(text, "A", "edit")));

    let done = in_repo(&fixture, &repo, move |repo| {
        rebase::run(repo, &["rebase", "-i", &base], editing)
    });

    assert_eq!(done.recv_timeout(WAIT).unwrap(), Ok(Rebase::Editing));
    assert!(repo.in_git_dir("rebase-merge"));
}

// Edit Commit Message, Drop and Squash from the log: the todo and the messages are answered by `Rewriter`,
// without asking the window.
mod rewrite {
    use super::*;
    use ramify_lib::preserve::RestoreOutcome;
    use ramify_lib::rewrite::{self, Rewritten};
    use ramify_lib::settings::{self, CleanTree};
    use ramify_lib::shelf;

    // `base`, `A`, `B`, `C` and `D`, each with its own file. Returns their hashes.
    fn five() -> (Scratch, Vec<String>) {
        let repo = Scratch::new();
        let hashes = ["base", "A", "B", "C", "D"]
            .iter()
            .map(|subject| repo.commit(subject))
            .collect();
        (repo, hashes)
    }

    fn head(repo: &Scratch) -> String {
        repo.git(&["rev-parse", "HEAD"]).trim().to_string()
    }

    fn files_of(repo: &Scratch, revision: &str) -> Vec<String> {
        repo.git(&["show", "--format=", "--name-only", revision])
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn stashes(repo: &Scratch) -> usize {
        repo.git(&["stash", "list"]).lines().count()
    }

    fn rewrite_in(
        fixture: &Fixture,
        repo: &Scratch,
        work: impl FnOnce(&Repo, &str) -> Result<Rewritten, GitError> + Send + 'static,
    ) -> Result<Rewritten, GitError> {
        let expected = head(repo);
        in_repo(fixture, repo, move |repo| work(repo, &expected))
            .recv_timeout(WAIT)
            .expect("the task did not finish")
    }

    // As `rewrite_in`, with a data folder the test keeps: the clean-tree setting and the shelf live
    // there.
    fn rewrite_with_data(
        fixture: &Fixture,
        repo: &Scratch,
        data: &Path,
        work: impl FnOnce(&Repo, &str) -> Result<Rewritten, GitError> + Send + 'static,
    ) -> Result<Rewritten, GitError> {
        let expected = head(repo);
        let (path, data) = (repo.path().to_path_buf(), data.to_path_buf());
        fixture
            .in_task(repo.path(), move |ctx| {
                work(
                    &Repo {
                        ctx,
                        path: &path,
                        data: &data,
                    },
                    &expected,
                )
            })
            .recv_timeout(WAIT)
            .expect("the task did not finish")
    }

    fn nothing_asked(fixture: &Fixture) {
        assert!(fixture.requests.try_recv().is_err(), "the window was asked");
    }

    #[test]
    fn reword_of_an_earlier_one_uses_rebase_and_keeps_the_others() {
        let fixture = Fixture::new(true);
        let (repo, hashes) = five();
        let old = head(&repo);
        let target = hashes[2].clone();

        let rewritten = rewrite_in(&fixture, &repo, move |repo, head| {
            rewrite::reword(repo, &target, "B new\n\nwith body", head)
        })
        .unwrap();

        assert_eq!(rewritten.outcome, Rebase::Done);
        assert_eq!(rewritten.old_head, old);
        assert_eq!(rewritten.new_head, head(&repo));
        assert_eq!(repo.subjects(), ["D", "C", "B new", "A", "base"]);
        assert_eq!(
            repo.git(&["log", "-1", "--format=%B", "HEAD~2"]).trim_end(),
            "B new\n\nwith body"
        );
        // What came before is not rewritten, and each commit keeps its own.
        assert_eq!(repo.git(&["rev-parse", "HEAD~3"]).trim(), hashes[1]);
        assert_eq!(files_of(&repo, "HEAD~2"), ["B.txt"]);
        assert_eq!(files_of(&repo, "HEAD"), ["D.txt"]);
        assert!(!repo.in_git_dir("rebase-merge"));
        nothing_asked(&fixture);
    }

    #[test]
    fn reword_that_does_not_start_with_the_original_fails() {
        let fixture = Fixture::new(true);
        let (repo, hashes) = five();
        let hook = repo.git_dir().join("hooks/prepare-commit-msg");
        fs::write(
            &hook,
            "#!/bin/sh\nprintf 'other\\n' | cat - \"$1\" > \"$1.x\" && mv \"$1.x\" \"$1\"\n",
        )
        .unwrap();
        Command::new("chmod").arg("+x").arg(&hook).status().unwrap();
        let target = hashes[2].clone();

        let rewritten = rewrite_in(&fixture, &repo, move |repo, head| {
            rewrite::reword(repo, &target, "B new", head)
        })
        .unwrap();

        // The editor refuses: git stops at that commit, without the new message, and it can be aborted.
        assert_eq!(rewritten.outcome, Rebase::Editing);
        assert!(repo.in_git_dir("rebase-merge"));
        assert!(!repo.git(&["log", "--format=%s"]).contains("B new"));
        nothing_asked(&fixture);
        let aborted = in_repo(&fixture, &repo, rebase::abort)
            .recv_timeout(WAIT)
            .unwrap();
        assert_eq!(aborted, Ok(()));
        assert_eq!(head(&repo), hashes[4]);
    }

    #[test]
    fn drop_of_two_commits() {
        let fixture = Fixture::new(true);
        let (repo, hashes) = five();
        // In the log's order, from the most recent to the oldest.
        let dropped = vec![hashes[3].clone(), hashes[1].clone()];

        let rewritten = rewrite_in(&fixture, &repo, move |repo, head| {
            rewrite::drop(repo, &dropped, head)
        })
        .unwrap();

        assert_eq!(rewritten.outcome, Rebase::Done);
        assert_eq!(repo.subjects(), ["D", "B", "base"]);
        assert!(!repo.path().join("A.txt").exists());
        assert!(!repo.path().join("C.txt").exists());
        assert!(repo.path().join("D.txt").exists());
        nothing_asked(&fixture);
    }

    #[test]
    fn non_contiguous_squash_reorders_like_intellij() {
        let fixture = Fixture::new(true);
        let (repo, hashes) = five();
        let squashed = vec![hashes[3].clone(), hashes[1].clone()];

        let rewritten = rewrite_in(&fixture, &repo, move |repo, head| {
            rewrite::squash(repo, &squashed, "A and C\n\ntogether", head)
        })
        .unwrap();

        // `unite`: the oldest is the root, and the other goes right behind it as `fixup`.
        assert_eq!(rewritten.outcome, Rebase::Done);
        assert_eq!(repo.subjects(), ["D", "B", "A and C", "base"]);
        assert_eq!(
            repo.git(&["log", "-1", "--format=%B", "HEAD~2"]).trim_end(),
            "A and C\n\ntogether"
        );
        assert_eq!(files_of(&repo, "HEAD~2"), ["A.txt", "C.txt"]);
        assert_eq!(files_of(&repo, "HEAD~1"), ["B.txt"]);
        nothing_asked(&fixture);
    }

    // With `rebase.abbreviateCommands`, git would write `p` and `f`, which the model does not know.
    #[test]
    fn reword_drop_and_squash_with_the_users_abbreviated_commands() {
        let fixture = Fixture::new(false);
        let (repo, hashes) = five();
        repo.git(&["config", "rebase.abbreviateCommands", "true"]);

        let target = hashes[1].clone();
        let rewritten = rewrite_in(&fixture, &repo, move |repo, head| {
            rewrite::reword(repo, &target, "A new", head)
        })
        .unwrap();
        assert_eq!(rewritten.outcome, Rebase::Done);
        assert_eq!(repo.subjects(), ["D", "C", "B", "A new", "base"]);

        let dropped = vec![repo.git(&["rev-parse", "HEAD~1"]).trim().to_string()];
        let rewritten = rewrite_in(&fixture, &repo, move |repo, head| {
            rewrite::drop(repo, &dropped, head)
        })
        .unwrap();
        assert_eq!(rewritten.outcome, Rebase::Done);
        assert_eq!(repo.subjects(), ["D", "B", "A new", "base"]);

        let squashed = vec![
            head(&repo),
            repo.git(&["rev-parse", "HEAD~1"]).trim().to_string(),
        ];
        let rewritten = rewrite_in(&fixture, &repo, move |repo, head| {
            rewrite::squash(repo, &squashed, "B and D", head)
        })
        .unwrap();
        assert_eq!(rewritten.outcome, Rebase::Done);
        assert_eq!(repo.subjects(), ["B and D", "A new", "base"]);
        assert!(!repo.in_git_dir("rebase-merge"));
    }

    // A Drop with local changes, saved the way `Clean working tree using:` says: they come back
    // after the rebase. Returns the repository after a second Drop, of the commit that created
    // `B.txt`, with `B.txt` changed again: the saved change no longer has its file.
    fn drop_twice_with_local_changes(
        fixture: &Fixture,
        data: &Path,
        clean_tree: CleanTree,
    ) -> (Scratch, Rewritten) {
        settings::save_clean_tree(data, clean_tree).unwrap();
        let (repo, hashes) = five();
        fs::write(repo.path().join("B.txt"), "written by Devin\n").unwrap();
        repo.git(&["add", "B.txt"]);
        fs::write(repo.path().join("D.txt"), "halfway\n").unwrap();
        let dropped = vec![hashes[3].clone()];

        let rewritten = rewrite_with_data(fixture, &repo, data, move |repo, head| {
            rewrite::drop(repo, &dropped, head)
        })
        .unwrap();

        assert_eq!(rewritten.outcome, Rebase::Done);
        assert_eq!(repo.subjects(), ["D", "B", "A", "base"]);
        assert_eq!(rewritten.saved.len(), 1);
        assert_eq!(rewritten.saved[0].saved.kind, clean_tree);
        assert_eq!(
            rewritten.saved[0].saved.message,
            "Uncommitted changes before rebase"
        );
        assert_eq!(rewritten.saved[0].restore, Some(RestoreOutcome::Restored));
        assert_eq!(
            fs::read_to_string(repo.path().join("B.txt")).unwrap(),
            "written by Devin\n"
        );
        assert_eq!(
            fs::read_to_string(repo.path().join("D.txt")).unwrap(),
            "halfway\n"
        );
        assert_eq!(stashes(&repo), 0);

        fs::write(repo.path().join("B.txt"), "again\n").unwrap();
        let b = repo.git(&["rev-parse", "HEAD~1"]).trim().to_string();
        let dropped = vec![b];
        let rewritten = rewrite_with_data(fixture, &repo, data, move |repo, head| {
            rewrite::drop(repo, &dropped, head)
        })
        .unwrap();

        assert_eq!(rewritten.outcome, Rebase::Done);
        assert_eq!(repo.subjects(), ["D", "A", "base"]);
        assert_eq!(rewritten.saved.len(), 1);
        nothing_asked(fixture);
        (repo, rewritten)
    }

    // With the stash, restoring them clashes with what was rewritten: the entry stays in the stash.
    #[test]
    fn local_changes_are_saved_and_restored_with_the_stash() {
        let fixture = Fixture::new(true);
        let data = TempDir::new().unwrap();

        let (repo, rewritten) =
            drop_twice_with_local_changes(&fixture, data.path(), CleanTree::Stash);

        assert_eq!(rewritten.saved[0].restore, Some(RestoreOutcome::Conflicts));
        assert_eq!(stashes(&repo), 1);
        assert!(repo
            .git(&["stash", "list"])
            .contains("Uncommitted changes before rebase"));
        assert!(shelf::list(data.path()).is_empty());
    }

    // With the shelf, 241's default: `VcsShelveChangesSaver.load` unshelves what applies (`D.txt`)
    // and the change of a file the rebase removed fails with `Cannot find file to patch: B.txt`
    // (`PathsVerifier.fileNotFoundMessage`). The failed patch stays in its list
    // (`ShelveChangesManager.unshelveChangeList` keeps `getFailedPatches`): nothing is lost.
    #[test]
    fn local_changes_are_saved_and_restored_with_the_shelf() {
        let fixture = Fixture::new(true);
        let data = TempDir::new().unwrap();

        let (repo, rewritten) =
            drop_twice_with_local_changes(&fixture, data.path(), CleanTree::Shelve);

        assert_eq!(
            rewritten.saved[0].restore,
            Some(RestoreOutcome::Failed(
                "Cannot find file to patch: B.txt".to_string()
            ))
        );
        assert_eq!(stashes(&repo), 0);
        assert!(!repo.path().join("B.txt").exists());
        assert_eq!(
            fs::read_to_string(repo.path().join("D.txt")).unwrap(),
            "halfway\n"
        );
        let saved = &rewritten.saved[0].saved.lists[0];
        let kept: Vec<_> = shelf::list(data.path())
            .into_iter()
            .filter(|list| list.name == saved.name && !list.recycled && !list.deleted)
            .collect();
        assert_eq!(
            kept.len(),
            1,
            "the list with the failed change is not in the shelf"
        );
        // `VcsShelveChangesSaver.save`: the message and the changelist's name.
        assert_eq!(
            kept[0].description,
            "Uncommitted changes before rebase [Changes]"
        );
        assert_eq!(kept[0].description, saved.description);
        let patch = fs::read_to_string(&kept[0].patch).unwrap();
        assert!(
            patch.contains("B.txt") && patch.contains("+again"),
            "{patch}"
        );
        assert!(!patch.contains("D.txt"), "{patch}");
    }

    #[test]
    fn conflict_when_rewriting_follows_task_2() {
        let fixture = Fixture::new(true);
        let repo = Scratch::new();
        repo.commit("base");
        fs::write(repo.path().join("a.txt"), "1\n").unwrap();
        repo.git(&["add", "-A"]);
        repo.git(&["commit", "-qm", "creates"]);
        let created = head(&repo);
        fs::write(repo.path().join("a.txt"), "2\n").unwrap();
        repo.git(&["commit", "-qam", "changes"]);
        let old = head(&repo);
        let dropped = vec![created];

        let rewritten = rewrite_in(&fixture, &repo, move |repo, head| {
            rewrite::drop(repo, &dropped, head)
        })
        .unwrap();

        // Like a Task 2 rebase: it stays in progress, with Continue and Abort.
        assert_eq!(rewritten.outcome, Rebase::Conflicts);
        assert_eq!(rewritten.old_head, old);
        assert!(repo.in_git_dir("rebase-merge"));
        let aborted = in_repo(&fixture, &repo, rebase::abort)
            .recv_timeout(WAIT)
            .unwrap();
        assert_eq!(aborted, Ok(()));
        assert_eq!(head(&repo), old);
        nothing_asked(&fixture);
    }

    // Interactively Rebase from Here… from the log.
    use ramify_lib::rebase::Rebased;
    use ramify_lib::rewrite::{NewMessage, Plan};
    use ramify_lib::todo::{Action, Entry};

    fn interactive_in(
        fixture: &Fixture,
        repo: &Scratch,
        commit: &str,
        plan: Option<Plan>,
    ) -> Receiver<Result<Rebased, GitError>> {
        let (commit, expected) = (commit.to_string(), head(repo));
        in_repo(fixture, repo, move |repo| {
            rewrite::interactive(repo, &commit, plan, &expected)
        })
    }

    fn entries_of(fixture: &Fixture, repo: &Scratch, commit: &str) -> Option<Vec<Entry>> {
        let commit = commit.to_string();
        in_repo(fixture, repo, move |repo| {
            rewrite::interactive_entries(repo, &commit)
        })
        .recv_timeout(WAIT)
        .unwrap()
        .unwrap()
    }

    // What comes out of the dialog: the generated entries with the chosen actions.
    fn planned(entries: Vec<Entry>, actions: &[(&str, Action)], messages: &[(&str, &str)]) -> Plan {
        let entries = entries
            .into_iter()
            .map(|mut entry| {
                if let Some((_, action)) = actions.iter().find(|(s, _)| *s == entry.subject) {
                    entry.action = action.clone();
                }
                entry
            })
            .collect::<Vec<_>>();
        let messages = messages
            .iter()
            .map(|(subject, message)| NewMessage {
                hash: entries
                    .iter()
                    .find(|entry| entry.subject == *subject)
                    .unwrap()
                    .hash
                    .clone(),
                message: message.to_string(),
            })
            .collect();
        Plan { entries, messages }
    }

    #[test]
    fn merge_in_the_range_gives_none_and_uses_native_mode() {
        let fixture = Fixture::new(true);
        let repo = Scratch::new();
        repo.commit("base");
        let a = repo.commit("A");
        repo.git(&["checkout", "-q", "-b", "side"]);
        repo.commit("L");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit("B");
        repo.git(&["merge", "-q", "--no-ff", "-m", "M", "side"]);
        repo.commit("C");

        assert_eq!(entries_of(&fixture, &repo, &a), None);

        // Without a plan, git generates the list and the window shows it in the dialog, already with the details.
        let done = interactive_in(&fixture, &repo, &a, None);
        let (id, kind, text) = fixture.requests.recv_timeout(WAIT).unwrap();
        assert_eq!(kind, EditKind::Todo);
        assert_eq!(fixture.forms.recv_timeout(WAIT).unwrap(), Form::Commits);
        let commits: serde_json::Value = serde_json::from_str(&text).unwrap();
        let commits = commits.as_array().unwrap();
        // The merge is flattened: without `--rebase-merges`, like `startInteractiveRebase`.
        let messages: Vec<&str> = commits
            .iter()
            .map(|commit| commit["message"].as_str().unwrap())
            .collect();
        assert_eq!(messages, ["A", "B", "L", "C"]);
        assert_eq!(commits[0]["full"], a.as_str());
        assert_eq!(commits[0]["message"], "A");
        assert_eq!(commits[0]["action"], "pick");
        // Answers with the lines as they are, without the last commit.
        let entries: Vec<serde_json::Value> = commits
            .iter()
            .map(|commit| {
                let drop = commit["message"] == "C";
                serde_json::json!({
                    "action": if drop { "drop" } else { "pick" },
                    "hash": commit["hash"],
                    "subject": commit["subject"],
                })
            })
            .collect();
        let answer = serde_json::json!({ "entries": entries, "messages": [] });
        fixture.server.answer(id, Some(answer.to_string()));

        let rebased = done
            .recv_timeout(WAIT)
            .unwrap()
            .expect("the rebase finishes");
        assert_eq!(rebased.outcome, Rebase::Done);
        assert_eq!(repo.subjects(), ["L", "B", "A", "base"]);
    }

    #[test]
    fn mismatching_plan_fails_without_rebasing() {
        let fixture = Fixture::new(true);
        let (repo, hashes) = five();
        let old = head(&repo);
        let plan = planned(
            entries_of(&fixture, &repo, &hashes[2]).unwrap(),
            &[("C", Action::Drop)],
            &[],
        );
        // Git writes something other than the log: the generated todo is no longer valid.
        repo.git(&["config", "rebase.instructionFormat", "%s [%an]"]);

        let result = interactive_in(&fixture, &repo, &hashes[2], Some(plan))
            .recv_timeout(WAIT)
            .unwrap()
            .map_err(ramify_lib::commands::ApiError::from)
            .map_err(|error| error.message);

        assert_eq!(
            result.map(|rebased| rebased.outcome),
            Err(
                "Cannot perform the rebase because an invalid 'git-rebase-todo' file was \
                 generated. Do you want to retry the rebase using Git native file generation?"
                    .to_string()
            )
        );
        assert!(!repo.in_git_dir("rebase-merge"));
        assert_eq!(head(&repo), old);
        assert_eq!(repo.subjects(), ["D", "C", "B", "A", "base"]);
        nothing_asked(&fixture);
    }

    // A dialog subject with a newline would be an `exec` line, which git passes to a shell.
    const INJECTED: &str = "B\nexec touch injected";

    #[test]
    fn a_subject_with_an_injected_exec_in_the_log_plan_does_not_reach_git() {
        let fixture = Fixture::new(true);
        let (repo, hashes) = five();
        let old = head(&repo);
        let mut plan = planned(entries_of(&fixture, &repo, &hashes[1]).unwrap(), &[], &[]);
        plan.entries[1].subject = INJECTED.to_string();

        let result = interactive_in(&fixture, &repo, &hashes[1], Some(plan))
            .recv_timeout(WAIT)
            .unwrap();

        assert!(!repo.path().join("injected").exists(), "git ran the exec");
        assert!(result.is_err(), "{result:?}");
        assert!(!repo.in_git_dir("rebase-merge"));
        assert_eq!(head(&repo), old);
        nothing_asked(&fixture);
    }

    #[test]
    fn a_subject_with_an_injected_exec_in_the_native_dialog_does_not_reach_git() {
        let fixture = Fixture::new(true);
        let (repo, hashes) = five();
        let old = head(&repo);

        let done = interactive_in(&fixture, &repo, &hashes[1], None);
        let (id, kind, text) = fixture.requests.recv_timeout(WAIT).unwrap();
        assert_eq!(kind, EditKind::Todo);
        assert_eq!(fixture.forms.recv_timeout(WAIT).unwrap(), Form::Commits);
        let commits: serde_json::Value = serde_json::from_str(&text).unwrap();
        let entries: Vec<serde_json::Value> = commits
            .as_array()
            .unwrap()
            .iter()
            .map(|commit| {
                let subject = if commit["message"] == "B" {
                    serde_json::json!(INJECTED)
                } else {
                    commit["subject"].clone()
                };
                serde_json::json!({ "action": "pick", "hash": commit["hash"], "subject": subject })
            })
            .collect();
        let answer = serde_json::json!({ "entries": entries, "messages": [] });
        fixture.server.answer(id, Some(answer.to_string()));

        let result = done.recv_timeout(WAIT).unwrap();
        assert!(!repo.path().join("injected").exists(), "git ran the exec");
        assert!(result.is_err(), "{result:?}");
        assert!(!repo.in_git_dir("rebase-merge"));
        assert_eq!(head(&repo), old);
    }

    #[test]
    fn log_entries_and_plan_with_drop_reword_and_fixup() {
        let fixture = Fixture::new(true);
        let (repo, hashes) = five();
        let entries = entries_of(&fixture, &repo, &hashes[1]).unwrap();
        let mut plan = planned(
            entries,
            &[
                ("B", Action::Reword),
                ("C", Action::Fixup),
                ("D", Action::Drop),
            ],
            &[("B", "B and C\n\ntogether")],
        );
        // The `drop`s go at the end, like `convertToEntries`.
        let dropped = plan.entries.remove(3);
        plan.entries.push(dropped);

        let rebased = interactive_in(&fixture, &repo, &hashes[1], Some(plan))
            .recv_timeout(WAIT)
            .unwrap()
            .expect("the rebase finishes");

        assert_eq!(rebased.outcome, Rebase::Done);
        assert_eq!(repo.subjects(), ["B and C", "A", "base"]);
        assert_eq!(files_of(&repo, "HEAD"), ["B.txt", "C.txt"]);
        assert_eq!(
            repo.git(&["log", "-1", "--format=%B", "HEAD"]).trim_end(),
            "B and C\n\ntogether"
        );
        nothing_asked(&fixture);
    }

    #[test]
    fn edit_stops_and_notifies_rebase_stopped_for_editing() {
        let fixture = Fixture::new(true);
        let (repo, hashes) = five();
        let plan = planned(
            entries_of(&fixture, &repo, &hashes[1]).unwrap(),
            &[("B", Action::Edit), ("D", Action::Reword)],
            &[("D", "D new")],
        );

        let rebased = interactive_in(&fixture, &repo, &hashes[1], Some(plan))
            .recv_timeout(WAIT)
            .unwrap()
            .expect("the rebase stops");

        // `Rebase stopped for editing`, with Continue and Abort: the rebase is still in progress.
        assert_eq!(rebased.outcome, Rebase::Editing);
        assert!(repo.in_git_dir("rebase-merge"));
        assert_eq!(head(&repo), hashes[2]);
        // On continuing, the `reword`'s message is already saved: it is not asked.
        let done = in_repo(&fixture, &repo, rebase::continue_rebase)
            .recv_timeout(WAIT)
            .unwrap();
        assert_eq!(done, Ok(Rebase::Done));
        assert_eq!(repo.subjects(), ["D new", "C", "B", "A", "base"]);
        nothing_asked(&fixture);
    }

    #[test]
    fn reword_without_a_saved_message_asks_git_commit_message() {
        let fixture = Fixture::new(true);
        let (repo, hashes) = five();
        let hook = repo.git_dir().join("hooks/prepare-commit-msg");
        fs::write(
            &hook,
            "#!/bin/sh\ncase \"$(head -1 \"$1\")\" in C) printf 'other\\n' | cat - \"$1\" > \"$1.x\" \
             && mv \"$1.x\" \"$1\";; esac\n",
        )
        .unwrap();
        Command::new("chmod").arg("+x").arg(&hook).status().unwrap();
        let plan = planned(
            entries_of(&fixture, &repo, &hashes[3]).unwrap(),
            &[("C", Action::Reword)],
            &[("C", "C new")],
        );

        let done = interactive_in(&fixture, &repo, &hashes[3], Some(plan));

        // The text no longer starts with the original: the free-text editor decides.
        let (id, kind, text) = fixture.requests.recv_timeout(WAIT).unwrap();
        assert_eq!(kind, EditKind::Message);
        assert_eq!(fixture.forms.recv_timeout(WAIT).unwrap(), Form::Text);
        assert!(text.starts_with("other\nC"), "{text:?}");
        fixture.server.answer(id, Some("C by hand\n".to_string()));
        let rebased = done
            .recv_timeout(WAIT)
            .unwrap()
            .expect("the rebase finishes");
        assert_eq!(rebased.outcome, Rebase::Done);
        assert_eq!(repo.subjects(), ["D", "C by hand", "B", "A", "base"]);
    }

    #[test]
    fn cancelling_the_dialog_does_not_launch_git() {
        let fixture = Fixture::new(true);
        let (repo, hashes) = five();
        let old = head(&repo);
        fs::write(repo.path().join("B.txt"), "local\n").unwrap();

        let done = interactive_in(&fixture, &repo, &hashes[1], None);
        let (id, kind, _) = fixture.requests.recv_timeout(WAIT).unwrap();
        assert_eq!(kind, EditKind::Todo);
        fixture.server.answer(id, None);

        assert_eq!(done.recv_timeout(WAIT).unwrap(), Err(GitError::Cancelled));
        assert!(!repo.in_git_dir("rebase-merge"));
        assert_eq!(head(&repo), old);
        assert_eq!(
            fs::read_to_string(repo.path().join("B.txt")).unwrap(),
            "local\n"
        );
        assert_eq!(stashes(&repo), 0);

        // Without a window to answer, git ends the same, with no rebase halfway.
        let alone = Fixture::new(false);
        let result = interactive_in(&alone, &repo, &hashes[1], None)
            .recv_timeout(WAIT)
            .unwrap();
        assert_eq!(
            result.map(|rebased| rebased.outcome),
            Err(GitError::Cancelled)
        );
        assert!(!repo.in_git_dir("rebase-merge"));
        assert_eq!(head(&repo), old);
    }

    #[test]
    fn noop_asks_whether_to_continue() {
        let fixture = Fixture::new(true);
        let repo = Scratch::new();
        let base = repo.commit("base");
        repo.git(&["checkout", "-q", "-b", "ahead"]);
        let ahead = repo.commit("A");
        repo.git(&["checkout", "-q", "main"]);
        let rebase_onto = |fixture: &Fixture| {
            let expected = head(&repo);
            in_repo(fixture, &repo, move |repo| {
                let params = rebase::RebaseParams {
                    upstream: Some("ahead".to_string()),
                    interactive: true,
                    ..Default::default()
                };
                rebase::rebase(repo, &params, &expected).map(|rebased| rebased.outcome)
            })
        };

        // `Cancel` changes nothing.
        let done = rebase_onto(&fixture);
        let (id, kind, _) = fixture.requests.recv_timeout(WAIT).unwrap();
        assert_eq!(kind, EditKind::Todo);
        assert_eq!(fixture.forms.recv_timeout(WAIT).unwrap(), Form::Noop);
        fixture.server.answer(id, None);
        assert_eq!(done.recv_timeout(WAIT).unwrap(), Err(GitError::Cancelled));
        assert_eq!(head(&repo), base);

        // `OK` continues: the branch ends up where the base is.
        let done = rebase_onto(&fixture);
        let (id, ..) = fixture.requests.recv_timeout(WAIT).unwrap();
        assert_eq!(fixture.forms.recv_timeout(WAIT).unwrap(), Form::Noop);
        fixture.server.answer(id, Some(String::new()));
        assert_eq!(done.recv_timeout(WAIT).unwrap(), Ok(Rebase::Done));
        assert_eq!(head(&repo), ahead);
    }
}

// Commit and Rebase… for Fixup… and Squash Into…: `rebase --interactive` without autosquash flags.
mod autosquash {
    use super::*;
    use ramify_lib::rewrite;

    // `base`, `A`, `B` and a `fixup! A` on top. Returns the hash of `A`.
    fn with_fixup() -> (Scratch, String) {
        let repo = Scratch::new();
        repo.commit("base");
        let a = repo.commit("A");
        repo.commit("B");
        fs::write(repo.path().join("A.txt"), "A fixed\n").unwrap();
        repo.git(&["commit", "-qam", "fixup! A"]);
        (repo, a)
    }

    #[test]
    fn commit_and_rebase_respects_the_users_rebase_autosquash_and_restores_local_changes() {
        let fixture = Fixture::new(true);
        let (repo, a) = with_fixup();
        repo.git(&["config", "rebase.autoSquash", "true"]);
        fs::write(repo.path().join("B.txt"), "local\n").unwrap();
        // What the handler sees is the list git already reordered: the fixup goes behind `A`.
        let handler = Scripted::new(|kind, text| {
            assert_eq!(kind, EditKind::Todo);
            Some(text.to_string())
        });
        let asked = handler.clone();

        let done = in_repo(&fixture, &repo, move |repo| {
            rewrite::autosquash_with(repo, &format!("{a}~"), asked)
        });
        let rebased = done
            .recv_timeout(WAIT)
            .unwrap()
            .expect("the rebase must finish");

        assert_eq!(rebased.outcome, Rebase::Done);
        assert_eq!(repo.subjects(), ["B", "A", "base"]);
        assert_eq!(repo.git(&["show", "HEAD~1:A.txt"]), "A fixed\n");
        let todo = &handler.seen()[0].1;
        let picks: Vec<&str> = todo
            .lines()
            .filter(|line| !line.starts_with('\u{1}'))
            .collect();
        assert!(picks[1].starts_with("fixup "), "{picks:?}");
        // What was local was saved beforehand and comes back afterwards.
        assert_eq!(rebased.saved.len(), 1);
        assert_eq!(
            fs::read_to_string(repo.path().join("B.txt")).unwrap(),
            "local\n"
        );
    }

    #[test]
    fn commit_and_rebase_with_the_option_off_does_not_reorder() {
        let fixture = Fixture::new(true);
        let (repo, a) = with_fixup();
        repo.git(&["config", "rebase.autoSquash", "false"]);
        let handler = Scripted::new(|_, text| Some(text.to_string()));

        let done = in_repo(&fixture, &repo, move |repo| {
            rewrite::autosquash_with(repo, &format!("{a}~"), handler)
        });
        let rebased = done
            .recv_timeout(WAIT)
            .unwrap()
            .expect("the rebase must finish");

        assert_eq!(rebased.outcome, Rebase::Done);
        assert_eq!(repo.subjects(), ["fixup! A", "B", "A", "base"]);
    }
}
