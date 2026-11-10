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

// What one repository's watcher looks at: the work tree, the work tree's own git folder and the
// common one (`git::git_dirs`). In a normal clone both git folders are `<root>/.git`; in a linked
// worktree and in a submodule they are outside the work tree (`git_dirs` has the layouts).
// `nested_worktrees`: the roots of the repository's other worktrees that sit inside this work tree
// (`.worktrees/<x>`, as agent tools make them), whose files are not this checkout's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchedPaths {
    pub root: PathBuf,
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
    pub nested_worktrees: Vec<PathBuf>,
}

impl WatchedPaths {
    pub fn of(root: &Path) -> Result<Self, GitError> {
        Self::of_with(&crate::git::git_binary(), root)
    }

    // `binary`: the git that lists the worktrees.
    fn of_with(binary: &Path, root: &Path) -> Result<Self, GitError> {
        let (git_dir, common_dir) = crate::git::git_dirs(root)?;
        let root = root.canonicalize().map_err(io_error)?;
        // Without the list, nothing is filtered: a nested worktree's activity only refreshes, and
        // the repository still opens and is watched.
        let listed = crate::git::worktree_paths(binary, &root).unwrap_or_else(|error| {
            eprintln!("git worktree list failed, nested worktrees are watched too: {error:?}");
            Vec::new()
        });
        // A worktree whose folder is gone (`prunable`) cannot be canonicalized and has no files.
        let nested_worktrees = listed
            .iter()
            .filter_map(|path| path.canonicalize().ok())
            .filter(|path| path.starts_with(&root) && *path != root)
            .collect();
        Ok(Self {
            root,
            git_dir,
            common_dir,
            nested_worktrees,
        })
    }

    // FSEvents reports canonical paths (/private/var, not /var). `git_dirs` and `of` already give
    // them; a `WatchedPaths` built by hand may not.
    fn canonical(&self) -> Result<Self, GitError> {
        let canonical = |path: &Path| path.canonicalize().map_err(io_error);
        Ok(Self {
            root: canonical(&self.root)?,
            git_dir: canonical(&self.git_dir)?,
            common_dir: canonical(&self.common_dir)?,
            nested_worktrees: self
                .nested_worktrees
                .iter()
                .filter_map(|path| path.canonicalize().ok())
                .collect(),
        })
    }
}

// The folders watched recursively, as `GitRepositoryUpdater` asks for the main and the worktree
// git folders: the work tree, and each git folder that is not already inside a watched one. A
// normal clone is a single watch.
pub fn watch_roots(paths: &WatchedPaths) -> Vec<PathBuf> {
    let mut roots = vec![paths.root.clone()];
    // The common folder first: a linked worktree's own folder is inside it.
    for folder in [&paths.common_dir, &paths.git_dir] {
        if !roots.iter().any(|root| folder.starts_with(root)) {
            roots.push(folder.clone());
        }
    }
    roots
}

// What concerns this repository, and nothing that only another checkout of it writes:
// - the work tree, except the other worktrees nested in it;
// - the own git folder, except `worktrees/` when it is also the common folder (those are the other
//   worktrees' private folders: their `HEAD`, `index` and locks);
// - in a linked worktree, from the common folder only what all checkouts share (`shared_entry`):
//   the main checkout's own `index`, `HEAD`, `logs/HEAD`, its submodules (`modules/`) and the
//   other worktrees' folders are not this checkout's.
// Never `objects/` nor a `*.lock`, at any depth (`modules/<sub>/objects/…`, `modules/<sub>/index.lock`).
pub fn relevant(path: &Path, paths: &WatchedPaths) -> bool {
    // The own folder first: in a linked worktree it is inside the common one.
    if let Ok(inside) = path.strip_prefix(&paths.git_dir) {
        let owns_common = paths.git_dir == paths.common_dir;
        return not_objects_nor_lock(inside) && !(owns_common && first_is(inside, "worktrees"));
    }
    if let Ok(inside) = path.strip_prefix(&paths.common_dir) {
        return not_objects_nor_lock(inside) && shared_entry(inside);
    }
    path.starts_with(&paths.root)
        && !paths
            .nested_worktrees
            .iter()
            .any(|nested| path.starts_with(nested))
}

fn names(inside: &Path) -> Vec<&str> {
    inside
        .components()
        .map(|part| part.as_os_str().to_str().unwrap_or(""))
        .collect()
}

fn first_is(inside: &Path, name: &str) -> bool {
    names(inside).first() == Some(&name)
}

// `objects` is a folder of git's, unless it comes after `refs` or `logs`: there it is part of a
// ref's name (`refs/heads/objects`). git refuses a ref name that ends in `.lock`.
fn not_objects_nor_lock(inside: &Path) -> bool {
    let mut in_refs = false;
    for name in names(inside) {
        if name.ends_with(".lock") || (name == "objects" && !in_refs) {
            return false;
        }
        in_refs |= name == "refs" || name == "logs";
    }
    true
}

// What git keeps for all the checkouts in the common folder (git's `common_list`, and P§3.2).
fn shared_entry(inside: &Path) -> bool {
    match names(inside).as_slice() {
        ["config" | "packed-refs" | "shallow"] => true,
        ["refs", "bisect" | "worktree" | "rewritten", ..] => false,
        ["refs", ..] => true,
        ["logs", "HEAD"] => false,
        ["logs", ..] => true,
        ["info", ..] => true,
        _ => false,
    }
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
// All three are in the common folder: every worktree of a repository shares one stash.
pub fn touches_stash(path: &Path, paths: &WatchedPaths) -> bool {
    let Ok(relative) = path.strip_prefix(&paths.common_dir) else {
        return false;
    };
    let parts: Vec<_> = relative.components().map(Component::as_os_str).collect();
    let names: Vec<&str> = parts.iter().filter_map(|part| part.to_str()).collect();
    matches!(
        names.as_slice(),
        ["refs", "stash"] | ["logs", "refs", "stash"] | ["packed-refs"]
    )
}

pub fn start(
    paths: WatchedPaths,
    on_change: impl Fn(RepoChange) + Send + 'static,
) -> Result<Watcher, GitError> {
    let paths = paths.canonical()?;
    let roots = watch_roots(&paths);
    let (sender, receiver) = mpsc::channel();
    let mut inner = notify::recommended_watcher(move |event: notify::Result<Event>| {
        let Ok(event) = event else { return };
        let touches_repo = event.paths.iter().any(|path| relevant(path, &paths));
        if touches_repo && !event.kind.is_access() {
            let stash = event.paths.iter().any(|path| touches_stash(path, &paths));
            let _ = sender.send(stash);
        }
    })
    .map_err(io_error)?;
    for root in &roots {
        inner
            .watch(root, RecursiveMode::Recursive)
            .map_err(io_error)?;
    }

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
    use std::os::unix::fs::PermissionsExt;
    use std::sync::mpsc;

    const REPO: &str = "/repo";

    fn clone_paths() -> WatchedPaths {
        WatchedPaths {
            root: PathBuf::from(REPO),
            git_dir: PathBuf::from(format!("{REPO}/.git")),
            common_dir: PathBuf::from(format!("{REPO}/.git")),
            nested_worktrees: Vec::new(),
        }
    }

    // A linked worktree at `/wt` of the repository at `/main`.
    fn worktree_paths() -> WatchedPaths {
        WatchedPaths {
            root: PathBuf::from("/wt"),
            git_dir: PathBuf::from("/main/.git/worktrees/wt"),
            common_dir: PathBuf::from("/main/.git"),
            nested_worktrees: Vec::new(),
        }
    }

    fn is_relevant(path: &str) -> bool {
        relevant(Path::new(&format!("{REPO}/{path}")), &clone_paths())
    }

    fn is_stash(path: &str) -> bool {
        touches_stash(Path::new(&format!("{REPO}/{path}")), &clone_paths())
    }

    // A real watcher on a repository, and what it reported.
    struct Watching {
        _watcher: Watcher,
        receiver: mpsc::Receiver<RepoChange>,
    }

    impl Watching {
        fn on(repo: &Path) -> Self {
            let (sender, receiver) = mpsc::channel();
            let watcher = start(WatchedPaths::of(repo).unwrap(), move |change| {
                let _ = sender.send(change);
            })
            .unwrap();
            let watching = Self {
                _watcher: watcher,
                receiver,
            };
            watching.settle();
            watching
        }

        // Discards whatever is still arriving (creating the repository, an earlier write).
        fn settle(&self) {
            while self
                .receiver
                .recv_timeout(Duration::from_millis(500))
                .is_ok()
            {}
        }

        // What `act` produces: the first change within 3 s, and the ones after it until a quiet
        // 700 ms.
        fn after(&self, act: impl FnOnce()) -> Vec<RepoChange> {
            self.settle();
            act();
            let mut seen = Vec::new();
            let mut wait = Duration::from_secs(3);
            while let Ok(change) = self.receiver.recv_timeout(wait) {
                seen.push(change);
                wait = Duration::from_millis(700);
            }
            seen
        }
    }

    // What arrives within `wait` once the stream has started.
    fn changes_after(repo: &TempRepo, act: impl FnOnce(), wait: Duration) -> Vec<RepoChange> {
        let watching = Watching::on(repo.path());
        act();
        let until = Instant::now() + wait;
        let mut seen = Vec::new();
        while let Some(left) = until.checked_duration_since(Instant::now()) {
            match watching.receiver.recv_timeout(left) {
                Ok(change) => seen.push(change),
                Err(_) => break,
            }
        }
        seen
    }

    // `git -C <repo>`, as from a terminal: not through rama.
    fn terminal(repo: &Path, args: &[&str]) {
        let output = crate::test_support::git_command()
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn a_normal_clone_still_registers_a_single_watch() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"one\n", "first");

        let paths = WatchedPaths::of(repo.path()).unwrap();

        assert_eq!(
            watch_roots(&paths),
            vec![repo.path().canonicalize().unwrap()]
        );
    }

    // The worktree's own folder is inside the common one: one watch covers both.
    #[test]
    fn a_linked_worktree_also_watches_the_common_folder() {
        let main = TempRepo::new();
        main.commit("a.txt", b"one\n", "first");
        let linked = main.worktree("wt");

        let paths = WatchedPaths::of(linked.path()).unwrap();

        let common = main.path().join(".git").canonicalize().unwrap();
        assert_eq!(
            watch_roots(&paths),
            vec![linked.path().canonicalize().unwrap(), common]
        );
    }

    #[test]
    fn objects_and_locks_in_the_common_dir_are_ignored() {
        let paths = worktree_paths();
        let is = |path: &str| relevant(Path::new(path), &paths);

        assert!(!is("/main/.git/objects/ab/cd"));
        assert!(!is("/main/.git/objects/pack/pack-1.pack"));
        assert!(!is("/main/.git/packed-refs.lock"));
        assert!(!is("/main/.git/config.lock"));
        assert!(!is("/main/.git/worktrees/wt/index.lock"));
        assert!(!is("/main/.git/worktrees/wt/HEAD.lock"));

        assert!(is("/main/.git/refs/heads/main"));
        assert!(is("/main/.git/packed-refs"));
        assert!(is("/main/.git/config"));
        assert!(is("/main/.git/worktrees/wt/index"));
        assert!(is("/main/.git/worktrees/wt/HEAD"));
        assert!(is("/wt/a.txt"));
        assert!(is("/wt/.git"));
        assert!(!is("/main/a.txt"));
        assert!(!is("/elsewhere/.git/index"));
    }

    // The main checkout's own files sit directly in the common folder, next to the shared ones; the
    // other worktrees' folders and the main checkout's submodules are there too. None of them is
    // this checkout's (the other worktrees' commits still show: they write the shared refs).
    #[test]
    fn a_linked_worktree_ignores_the_private_files_of_the_other_checkouts() {
        let paths = worktree_paths();
        let is = |path: &str| relevant(Path::new(path), &paths);

        for private in [
            "/main/.git",
            "/main/.git/index",
            "/main/.git/HEAD",
            "/main/.git/ORIG_HEAD",
            "/main/.git/MERGE_HEAD",
            "/main/.git/FETCH_HEAD",
            "/main/.git/COMMIT_EDITMSG",
            "/main/.git/logs/HEAD",
            "/main/.git/rebase-merge/done",
            "/main/.git/sequencer/todo",
            "/main/.git/refs/bisect/bad",
            "/main/.git/refs/worktree/x",
            "/main/.git/refs/rewritten/x",
            "/main/.git/worktrees/other/index",
            "/main/.git/worktrees/other/HEAD",
            "/main/.git/worktrees/other/logs/HEAD",
            "/main/.git/worktrees/other/index.lock",
            "/main/.git/modules/x/HEAD",
            "/main/.git/modules/x/refs/heads/main",
            "/main/.git/refs/heads/main.lock",
        ] {
            assert!(!is(private), "{private}");
        }
        for shared in [
            "/main/.git/config",
            "/main/.git/packed-refs",
            "/main/.git/shallow",
            "/main/.git/refs/heads/main",
            "/main/.git/refs/heads/feature/objects",
            "/main/.git/refs/remotes/origin/main",
            "/main/.git/refs/tags/v1",
            "/main/.git/refs/stash",
            "/main/.git/logs/refs/heads/main",
            "/main/.git/logs/refs/stash",
            "/main/.git/info/exclude",
            "/main/.git/worktrees/wt/logs/HEAD",
            "/main/.git/worktrees/wt/modules/sub/HEAD",
        ] {
            assert!(is(shared), "{shared}");
        }
    }

    // Its own submodules' folders stay (a commit inside a submodule changes the parent's status),
    // without their objects and locks; the linked worktrees' private folders do not.
    #[test]
    fn a_normal_clone_ignores_other_worktrees_but_keeps_its_submodules() {
        for ignored in [
            ".git/worktrees",
            ".git/worktrees/other/index",
            ".git/worktrees/other/HEAD",
            ".git/worktrees/other/index.lock",
            ".git/modules/sub/objects/ab/cd",
            ".git/modules/libs/sub/objects/pack/pack-1.pack",
            ".git/modules/sub/index.lock",
            ".git/refs/heads/main.lock",
        ] {
            assert!(!is_relevant(ignored), "{ignored}");
        }
        for kept in [
            ".git/modules/sub/HEAD",
            ".git/modules/sub/index",
            ".git/modules/sub/refs/heads/main",
            ".git/modules/libs/sub/HEAD",
            ".git/refs/heads/objects",
            ".git/logs/refs/heads/objects",
        ] {
            assert!(is_relevant(kept), "{kept}");
        }
    }

    #[test]
    fn files_of_a_worktree_nested_in_the_work_tree_are_not_this_checkouts() {
        let paths = WatchedPaths {
            nested_worktrees: vec![PathBuf::from("/repo/.worktrees/x")],
            ..clone_paths()
        };
        let is = |path: &str| relevant(Path::new(path), &paths);

        assert!(!is("/repo/.worktrees/x/a.txt"));
        assert!(!is("/repo/.worktrees/x/.git"));
        assert!(is("/repo/.worktrees/xy/a.txt"));
        assert!(is("/repo/.worktrees"));
        assert!(is("/repo/a.txt"));
    }

    // The stash is shared: it lives in the common folder, never in the worktree's own one.
    #[test]
    fn the_stash_of_a_linked_worktree_is_in_the_common_folder() {
        let paths = worktree_paths();
        let is = |path: &str| touches_stash(Path::new(path), &paths);

        assert!(is("/main/.git/refs/stash"));
        assert!(is("/main/.git/logs/refs/stash"));
        assert!(is("/main/.git/packed-refs"));
        assert!(!is("/main/.git/worktrees/wt/refs/stash"));
        assert!(!is("/main/.git/worktrees/wt/logs/HEAD"));
        assert!(!is("/wt/.git/refs/stash"));
        assert!(!is("/main/.git/refs/heads/main"));
    }

    #[test]
    fn a_linked_worktree_sees_index_branch_and_stash_changes_from_outside() {
        let main = TempRepo::new();
        main.commit("a.txt", b"one\n", "first");
        let linked = main.worktree("wt");
        let path = linked.path();
        let watching = Watching::on(path);

        fs::write(path.join("a.txt"), b"two\n").unwrap();
        let added = watching.after(|| terminal(path, &["add", "a.txt"]));
        assert!(!added.is_empty(), "git add");

        let committed = watching.after(|| terminal(path, &["commit", "-q", "-m", "second"]));
        assert!(!committed.is_empty(), "git commit");

        let switched = watching.after(|| terminal(path, &["checkout", "-q", "-b", "feature"]));
        assert!(!switched.is_empty(), "git checkout -b");

        fs::write(path.join("a.txt"), b"three\n").unwrap();
        let stashed = watching.after(|| terminal(path, &["stash", "push", "-q", "-m", "work"]));
        assert!(
            stashed.iter().any(|change| change.stash),
            "git stash: {stashed:?}"
        );
    }

    #[test]
    fn a_submodule_sees_its_own_index_and_branch_changes() {
        let parent = TempRepo::new();
        parent.commit("a.txt", b"one\n", "first");
        let submodule = parent.with_submodule("sub");
        let path = submodule.path();
        let watching = Watching::on(path);

        fs::write(path.join("sub.txt"), b"changed\n").unwrap();
        let added = watching.after(|| terminal(path, &["add", "sub.txt"]));
        assert!(!added.is_empty(), "git add");

        let switched = watching.after(|| terminal(path, &["checkout", "-q", "-b", "feature"]));
        assert!(!switched.is_empty(), "git checkout -b");
    }

    #[test]
    fn a_submodule_inside_a_worktree_is_watched_on_its_own_git_dir() {
        let main = TempRepo::new();
        main.commit("a.txt", b"one\n", "first");
        let linked = main.worktree("wt");
        let submodule = linked.with_submodule("sub");
        let path = submodule.path();
        let own = main
            .path()
            .join(".git/worktrees/wt/modules/sub")
            .canonicalize()
            .unwrap();
        let paths = WatchedPaths::of(path).unwrap();
        assert_eq!((&paths.git_dir, &paths.common_dir), (&own, &own));
        assert_eq!(watch_roots(&paths), vec![path.canonicalize().unwrap(), own]);
        let watching = Watching::on(path);

        fs::write(path.join("sub.txt"), b"changed\n").unwrap();
        let added = watching.after(|| terminal(path, &["add", "sub.txt"]));
        assert!(!added.is_empty(), "git add");

        let switched = watching.after(|| terminal(path, &["checkout", "-q", "-b", "feature"]));
        assert!(!switched.is_empty(), "git checkout -b");
    }

    // Devin, say, works in the main checkout and in another worktree while rama shows this one.
    #[test]
    fn activity_in_the_other_checkouts_does_not_refresh_a_linked_worktree() {
        let main = TempRepo::new();
        main.commit("a.txt", b"one\n", "first");
        let linked = main.worktree("wt");
        let other = main.worktree("other");
        let watching = Watching::on(linked.path());

        fs::write(main.path().join("a.txt"), b"main\n").unwrap();
        let in_main = watching.after(|| {
            terminal(main.path(), &["add", "a.txt"]);
            terminal(main.path(), &["status", "--porcelain"]);
            terminal(main.path(), &["checkout", "-q", "--detach"]);
        });
        assert_eq!(in_main, vec![], "the main checkout's index and HEAD");

        fs::write(other.path().join("a.txt"), b"other\n").unwrap();
        let in_other = watching.after(|| {
            terminal(other.path(), &["add", "a.txt"]);
            terminal(other.path(), &["status", "--porcelain"]);
        });
        assert_eq!(in_other, vec![], "the other worktree's index");

        let committed = watching.after(|| terminal(other.path(), &["commit", "-q", "-m", "other"]));
        assert!(!committed.is_empty(), "a commit writes the shared branch");
    }

    #[test]
    fn a_normal_clone_ignores_activity_in_its_linked_worktrees() {
        let main = TempRepo::new();
        main.commit("a.txt", b"one\n", "first");
        let linked = main.worktree("wt");
        let watching = Watching::on(main.path());

        fs::write(linked.path().join("a.txt"), b"two\n").unwrap();
        let added = watching.after(|| {
            terminal(linked.path(), &["add", "a.txt"]);
            terminal(linked.path(), &["checkout", "-q", "--detach"]);
        });
        assert_eq!(added, vec![], "the worktree's index and HEAD");

        terminal(linked.path(), &["checkout", "-q", "wt"]);
        let committed = watching.after(|| terminal(linked.path(), &["commit", "-q", "-m", "two"]));
        assert!(!committed.is_empty(), "a commit writes the shared branch");
    }

    #[test]
    fn a_worktree_inside_the_work_tree_does_not_refresh_the_main_checkout() {
        let main = TempRepo::new();
        main.commit("a.txt", b"one\n", "first");
        main.git(&["worktree", "add", "-q", "-b", "x", ".worktrees/x"]);
        let nested = main.path().join(".worktrees/x");
        let paths = WatchedPaths::of(main.path()).unwrap();
        assert_eq!(paths.nested_worktrees, vec![nested.canonicalize().unwrap()]);
        let watching = Watching::on(main.path());

        let edited = watching.after(|| fs::write(nested.join("a.txt"), b"edited\n").unwrap());
        assert_eq!(edited, vec![], "a file of the nested worktree");

        let own = watching.after(|| fs::write(main.path().join("a.txt"), b"edited\n").unwrap());
        assert!(!own.is_empty(), "a file of the main checkout");
    }

    // The final review: a `git worktree list` that fails costs only the filter of the nested
    // worktrees. The repository still opens, and a Reopen still watches it.
    #[test]
    fn a_failing_worktree_list_watches_without_nested_worktrees() {
        let main = TempRepo::new();
        main.commit("a.txt", b"one\n", "first");
        main.git(&["worktree", "add", "-q", "-b", "x", ".worktrees/x"]);
        let scripts = tempfile::TempDir::new().unwrap();
        let git = scripts.path().join("git");
        let text = "#!/bin/sh\ncase \" $* \" in\n*\" worktree list \"*) echo broken >&2; exit 128 ;;\n*) exec git \"$@\" ;;\nesac\n";
        fs::write(&git, text).unwrap();
        fs::set_permissions(&git, fs::Permissions::from_mode(0o755)).unwrap();

        let paths = WatchedPaths::of_with(&git, main.path()).unwrap();

        assert_eq!(paths.root, main.path().canonicalize().unwrap());
        assert_eq!(paths.nested_worktrees, Vec::<PathBuf>::new());
        assert_eq!(WatchedPaths::of(main.path()).unwrap().nested_worktrees.len(), 1);
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
        assert!(!relevant(Path::new("/other/a.txt"), &clone_paths()));
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
        let _watcher = start(WatchedPaths::of(repo.path()).unwrap(), move |_| {
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
        let watcher = start(WatchedPaths::of(repo.path()).unwrap(), move |_| {
            let _ = sender.send(());
        })
        .unwrap();
        while receiver.recv_timeout(Duration::from_millis(500)).is_ok() {}

        drop(watcher);
        fs::write(repo.path().join("a.txt"), b"hello").unwrap();

        assert!(receiver.recv_timeout(Duration::from_millis(800)).is_err());
    }
}
