use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher as _};
use serde::Serialize;

use crate::git::GitError;

const QUIET_PERIOD: Duration = Duration::from_millis(200);

// While it exists, the repository is watched; when it is dropped the stream stops and the thread ends.
pub struct Watcher {
    _inner: RecommendedWatcher,
}

pub struct Debouncer {
    delay: Duration,
    last: Option<Instant>,
}

impl Debouncer {
    pub fn new(delay: Duration) -> Self {
        Self { delay, last: None }
    }

    pub fn push(&mut self, now: Instant) {
        self.last = Some(now);
    }

    // True only once, when `delay` has passed with no new changes.
    pub fn ready(&mut self, now: Instant) -> bool {
        match self.last {
            Some(last) if now.saturating_duration_since(last) >= self.delay => {
                self.last = None;
                true
            }
            _ => false,
        }
    }
}

pub fn relevant(path: &Path, repo: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(repo) else {
        return false;
    };
    let mut parts = relative.components().map(Component::as_os_str);
    if parts.next().is_none_or(|first| first != ".git") {
        return true;
    }
    let Some(inside) = parts.next() else {
        return true;
    };
    inside != "objects" && !inside.to_string_lossy().ends_with(".lock")
}

// The `repo-changed` payload. `stash`: the stash ref or its reflog was touched in that quiet period
// (`GitStashTracker` reads the list again only then). `shelf`: the store of the open repository
// was written since the previous signal (`SHELF_TOPIC`); the store lives in the app's data, which
// the watcher does not see, so only `commands::emit_repo_changed` sets it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct RepoChange {
    pub stash: bool,
    pub shelf: bool,
}

// `refs/stash`, its reflog, and `packed-refs`, which `git stash drop` of the last entry may rewrite.
pub fn touches_stash(path: &Path, repo: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(repo) else {
        return false;
    };
    let parts: Vec<_> = relative.components().map(Component::as_os_str).collect();
    let names: Vec<&str> = parts.iter().filter_map(|part| part.to_str()).collect();
    matches!(
        names.as_slice(),
        [".git", "refs", "stash"] | [".git", "logs", "refs", "stash"] | [".git", "packed-refs"]
    )
}

pub fn start(
    repo: PathBuf,
    on_change: impl Fn(RepoChange) + Send + 'static,
) -> Result<Watcher, GitError> {
    // FSEvents reports canonical paths (/private/var, not /var).
    let root = repo.canonicalize().map_err(io_error)?;
    let (sender, receiver) = mpsc::channel();
    let filter_root = root.clone();
    let mut inner = notify::recommended_watcher(move |event: notify::Result<Event>| {
        let Ok(event) = event else { return };
        let touches_repo = event.paths.iter().any(|path| relevant(path, &filter_root));
        if touches_repo && !event.kind.is_access() {
            let stash = event
                .paths
                .iter()
                .any(|path| touches_stash(path, &filter_root));
            let _ = sender.send(stash);
        }
    })
    .map_err(io_error)?;
    inner
        .watch(&root, RecursiveMode::Recursive)
        .map_err(io_error)?;

    thread::spawn(move || {
        let mut debouncer = Debouncer::new(QUIET_PERIOD);
        let mut waiting = false;
        // Whether any change of the period touched the stash.
        let mut stash = false;
        loop {
            let received = if waiting {
                receiver.recv_timeout(QUIET_PERIOD)
            } else {
                receiver.recv().map_err(|_| RecvTimeoutError::Disconnected)
            };
            match received {
                Ok(touched) => {
                    debouncer.push(Instant::now());
                    waiting = true;
                    stash |= touched;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            if debouncer.ready(Instant::now()) {
                waiting = false;
                on_change(RepoChange {
                    stash,
                    shelf: false,
                });
                stash = false;
            }
        }
    });
    Ok(Watcher { _inner: inner })
}

fn io_error(error: impl ToString) -> GitError {
    GitError::Io(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempRepo;
    use std::fs;
    use std::sync::mpsc;

    const REPO: &str = "/repo";

    fn is_relevant(path: &str) -> bool {
        relevant(Path::new(&format!("{REPO}/{path}")), Path::new(REPO))
    }

    fn is_stash(path: &str) -> bool {
        touches_stash(Path::new(&format!("{REPO}/{path}")), Path::new(REPO))
    }

    // What arrives within `wait` once the stream has started.
    fn changes_after(repo: &TempRepo, act: impl FnOnce(), wait: Duration) -> Vec<RepoChange> {
        let (sender, receiver) = mpsc::channel();
        let _watcher = start(repo.path().to_path_buf(), move |change| {
            let _ = sender.send(change);
        })
        .unwrap();
        while receiver.recv_timeout(Duration::from_millis(500)).is_ok() {}
        act();
        let until = Instant::now() + wait;
        let mut seen = Vec::new();
        while let Some(left) = until.checked_duration_since(Instant::now()) {
            match receiver.recv_timeout(left) {
                Ok(change) => seen.push(change),
                Err(_) => break,
            }
        }
        seen
    }

    #[test]
    fn ignores_objects_and_locks() {
        assert!(!is_relevant(".git/objects/ab/cd"));
        assert!(!is_relevant(".git/index.lock"));
    }

    #[test]
    fn counts_git_changes() {
        assert!(is_relevant(".git/index"));
        assert!(is_relevant(".git/HEAD"));
        assert!(is_relevant(".git/refs/heads/main"));
    }

    // `GitStashTracker` reads the list again when the stash changes: `git stash` writes `refs/stash`
    // and its reflog, and dropping the last one may rewrite `packed-refs`.
    #[test]
    fn counts_stash_changes() {
        assert!(is_relevant(".git/refs/stash"));
        assert!(is_relevant(".git/logs/refs/stash"));
        assert!(is_stash(".git/refs/stash"));
        assert!(is_stash(".git/logs/refs/stash"));
        assert!(is_stash(".git/packed-refs"));
        assert!(!is_stash(".git/index"));
        assert!(!is_stash(".git/refs/heads/main"));
        assert!(!is_stash("refs/stash"));
        assert!(!is_stash("src/a.txt"));
    }

    #[test]
    fn a_worktree_save_is_not_a_stash_change() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"one\n", "first");

        let seen = changes_after(
            &repo,
            || fs::write(repo.path().join("a.txt"), b"two\n").unwrap(),
            Duration::from_millis(1500),
        );

        assert!(!seen.is_empty());
        assert!(seen.iter().all(|change| !change.stash));
    }

    #[test]
    fn a_stash_push_is_a_stash_change() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"one\n", "first");
        fs::write(repo.path().join("a.txt"), b"two\n").unwrap();

        let seen = changes_after(
            &repo,
            || {
                repo.git(&["stash", "push", "-m", "work"]);
            },
            Duration::from_secs(2),
        );

        assert!(seen.iter().any(|change| change.stash));
    }

    #[test]
    fn counts_working_files() {
        assert!(is_relevant("src/a.txt"));
    }

    #[test]
    fn ignores_paths_outside_the_repository() {
        assert!(!relevant(Path::new("/other/a.txt"), Path::new(REPO)));
    }

    #[test]
    fn groups_consecutive_changes() {
        let start = Instant::now();
        let at = |millis| start + Duration::from_millis(millis);
        let mut debouncer = Debouncer::new(Duration::from_millis(200));

        debouncer.push(at(0));
        debouncer.push(at(50));
        debouncer.push(at(100));

        assert!(!debouncer.ready(at(150)));
        assert!(!debouncer.ready(at(299)));
        assert!(debouncer.ready(at(300)));
        assert!(!debouncer.ready(at(600)));
    }

    #[test]
    fn detects_a_real_write() {
        let repo = TempRepo::new();
        let (sender, receiver) = mpsc::channel();
        let _watcher = start(repo.path().to_path_buf(), move |_| {
            let _ = sender.send(());
        })
        .unwrap();
        // Discards whatever is left from creating the repository and lets the stream start.
        while receiver.recv_timeout(Duration::from_millis(500)).is_ok() {}

        fs::write(repo.path().join("a.txt"), b"hello").unwrap();

        assert!(receiver.recv_timeout(Duration::from_secs(2)).is_ok());
    }

    #[test]
    fn dropping_the_watcher_stops_notifying() {
        let repo = TempRepo::new();
        let (sender, receiver) = mpsc::channel();
        let watcher = start(repo.path().to_path_buf(), move |_| {
            let _ = sender.send(());
        })
        .unwrap();
        while receiver.recv_timeout(Duration::from_millis(500)).is_ok() {}

        drop(watcher);
        fs::write(repo.path().join("a.txt"), b"hello").unwrap();

        assert!(receiver.recv_timeout(Duration::from_millis(800)).is_err());
    }
}
