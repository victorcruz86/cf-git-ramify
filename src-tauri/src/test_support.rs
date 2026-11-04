use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::{mpsc, Arc};
use std::time::Duration;

use tempfile::TempDir;

use crate::branches::Repo;
use crate::commands::RepoLocks;
use crate::console::Console;
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
    let (send, receive) = mpsc::channel();
    runner.start(repo, TaskKind::Write, "test".to_string(), move |ctx| {
        send.send(work(ctx)).unwrap();
        Ok(())
    });
    receive.recv_timeout(Duration::from_secs(60)).unwrap()
}

pub struct TempRepo {
    dir: TempDir,
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
        let repo = Self { dir };
        repo.git(&["init", "-q", "-b", "main"]);
        repo.git(&["config", "user.name", "Test"]);
        repo.git(&["config", "user.email", "test@example.com"]);
        repo.git(&["config", "commit.gpgsign", "false"]);
        repo
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
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
        let status = Command::new("git")
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
        let merge = Command::new("git")
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
        let output = Command::new("git")
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
        Self {
            repo: TempRepo::new(),
            data: TempDir::new().unwrap(),
            console,
            runner,
        }
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
        let output = Command::new("git")
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
        self.path().join(".git").join(name).exists()
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
