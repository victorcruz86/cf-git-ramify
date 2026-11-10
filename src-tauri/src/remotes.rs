// `Git.Configure.Remotes`, `Git.Clone`, `Git.Init` and `Git.OpenExcludeFile`. No remote, URL or
// path starting with `-` reaches git: it would be an option (`-oProxyCommand=…`). `protocol.allow`
// is not touched; the transports that run commands (`ext::`) are blocked by git itself.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use crate::branch_info::BranchSync;
use crate::branches;
use crate::commands::repo_root;
use crate::fetch;
use crate::git::{self, GitError};
use crate::task::TaskContext;

const ORIGIN: &str = "origin";
const EXCLUDE: &str = "info/exclude";
// Absolute path: what the app launches does not depend on the PATH it inherited.
const OPEN: &str = "/usr/bin/open";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Remote {
    pub name: String,
    pub url: String,
    pub push_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Cloned {
    pub path: String,
    // What failed after cloning completely (the submodules): the clone is kept.
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum InitOutcome {
    Created { path: String },
    UnderGit,
}

fn invalid(message: String) -> GitError {
    GitError::Io(message)
}

// The remotes with their first URL, `origin` first and the rest by name, like IntelliJ's dialog.
// `remote.<name>.url`: the name may contain dots.
pub fn list(repo: &Path) -> Result<Vec<Remote>, GitError> {
    let pattern = r"^remote\..*\.(url|pushurl)$";
    let output = match git::run(repo, &["config", "-z", "--get-regexp", pattern]) {
        Ok(output) => output,
        Err(GitError::Failed { code: Some(1), .. }) => Vec::new(),
        Err(error) => return Err(error),
    };
    let mut remotes: Vec<Remote> = Vec::new();
    for entry in String::from_utf8_lossy(&output).split('\0') {
        let Some((key, value)) = entry.split_once('\n') else {
            continue;
        };
        let Some((name, variable)) = key.strip_prefix("remote.").and_then(|k| k.rsplit_once('.'))
        else {
            continue;
        };
        let index = match remotes.iter().position(|remote| remote.name == name) {
            Some(index) => index,
            None => {
                let remote = Remote {
                    name: name.to_string(),
                    url: String::new(),
                    push_url: None,
                };
                remotes.push(remote);
                remotes.len() - 1
            }
        };
        let remote = &mut remotes[index];
        match variable {
            "url" if remote.url.is_empty() => remote.url = value.to_string(),
            "pushurl" if remote.push_url.is_none() => remote.push_url = Some(value.to_string()),
            _ => {}
        }
    }
    remotes.retain(|remote| !remote.url.is_empty());
    remotes.sort_by(|a, b| (a.name != ORIGIN, &a.name).cmp(&(b.name != ORIGIN, &b.name)));
    Ok(remotes)
}

// With `refs/remotes/` in front: it is where git keeps its own, and it rejects the same as
// IntelliJ's name validator (spaces, `..`, `~`…).
pub(crate) fn check_name(ctx: &TaskContext, name: &str) -> Result<(), GitError> {
    branches::check_argument(name, "The remote name")?;
    match ctx.git(&["check-ref-format", &format!("refs/remotes/{name}")]) {
        Ok(_) => Ok(()),
        Err(GitError::Failed { .. }) => Err(invalid(format!(
            "The remote name '{name}' contains invalid characters"
        ))),
        Err(error) => Err(error),
    }
}

fn check_url(url: &str) -> Result<(), GitError> {
    branches::check_argument(url, "The URL")
}

pub fn add(ctx: &TaskContext, name: &str, url: &str) -> Result<(), GitError> {
    check_name(ctx, name)?;
    check_url(url)?;
    ctx.git_loud(&["remote", "add", name, url])?;
    Ok(())
}

// `changeRemote`: first the name and then the URL, already with the new name.
pub fn edit(ctx: &TaskContext, name: &str, new_name: &str, new_url: &str) -> Result<(), GitError> {
    branches::check_argument(name, "The remote name")?;
    check_name(ctx, new_name)?;
    check_url(new_url)?;
    let remotes = list(ctx.repo())?;
    let Some(current) = remotes.iter().find(|remote| remote.name == name) else {
        return Err(invalid(format!("The remote '{name}' does not exist")));
    };
    if new_name != name {
        ctx.git_loud(&["remote", "rename", name, new_name])?;
    }
    if new_url != current.url {
        ctx.git_loud(&["remote", "set-url", new_name, new_url])?;
    }
    Ok(())
}

// Only the configuration: `git remote remove` does not touch the tree or the index.
pub fn remove(ctx: &TaskContext, name: &str) -> Result<(), GitError> {
    branches::check_argument(name, "The remote name")?;
    ctx.git_loud(&["remote", "remove", name])?;
    Ok(())
}

// Like `GitDefineRemoteDialog.checkUrlAccess`. With `remote`, known by its name, a successful
// `ls-remote` counts as authenticated.
pub fn test_url(
    ctx: &TaskContext,
    sync: &BranchSync,
    url: &str,
    remote: Option<&str>,
) -> Result<(), GitError> {
    check_url(url)?;
    ctx.git_loud(&["ls-remote", url])?;
    if let Some(remote) = remote {
        sync.mark_authenticated(ctx.repo(), remote);
    }
    Ok(())
}

// A leading `~` is the user's folder, as in a terminal. `~another` is left alone.
pub fn expand_home(path: &str, home: Option<&Path>) -> PathBuf {
    let rest = match path.strip_prefix('~') {
        Some("") => Some(""),
        Some(rest) => rest.strip_prefix('/'),
        None => None,
    };
    match (rest, home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(path),
    }
}

// `CloneDvcsValidationUtils.createDestination`: what is missing from the path is created. The
// destination is not: git takes care of that, and that way it is known whether it created it.
pub fn create_parent(dir: &Path) -> std::io::Result<()> {
    match dir.parent() {
        Some(parent) => fs::create_dir_all(parent),
        None => Ok(()),
    }
}

// What was in the destination before cloning: what can be deleted if the clone does not finish
// depends on it. Git does not clone into a folder with things in it, so nothing is ever touched in
// an occupied one.
enum Destination {
    Created,
    Empty,
    Occupied,
}

impl Destination {
    fn prepare(dir: &Path) -> Self {
        if dir.symlink_metadata().is_err() {
            return Self::Created;
        }
        let empty = fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_none());
        if empty {
            Self::Empty
        } else {
            Self::Occupied
        }
    }

    // The errors say nothing new: what is left, stays.
    fn discard(self, dir: &Path) {
        match self {
            Self::Created => {
                let _ = fs::remove_dir_all(dir);
            }
            Self::Empty => {
                let Ok(entries) = fs::read_dir(dir) else {
                    return;
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
                    let _ = if is_dir {
                        fs::remove_dir_all(&path)
                    } else {
                        fs::remove_file(&path)
                    };
                }
            }
            Self::Occupied => {}
        }
    }
}

// `GitImpl.clone`: in the parent folder, with the URL before `--` and the destination's name after.
// The URL cannot start with `-`; the destination, though it can, goes after `--`.
pub fn clone(
    ctx: &TaskContext,
    sync: &BranchSync,
    url: &str,
    dir: &Path,
) -> Result<Cloned, GitError> {
    check_url(url)?;
    let name = match (dir.parent(), dir.file_name().and_then(|name| name.to_str())) {
        (Some(parent), Some(name)) if parent == ctx.repo() => name,
        _ => {
            return Err(invalid(format!(
                "The target '{}' is not in the working folder",
                dir.display()
            )))
        }
    };
    let destination = Destination::prepare(dir);
    let args = [
        "clone",
        "--progress",
        "--recurse-submodules",
        url,
        "--",
        name,
    ];
    let mut warning = None;
    if let Err(error) = ctx.git_loud(&args) {
        // If only `--recurse-submodules` fails, git exits with 1 and leaves the whole clone, and
        // IntelliJ keeps it. What was left is removed only if it was cancelled or there is no repository.
        let complete = error != GitError::Cancelled && dir.join(".git/HEAD").is_file();
        if !complete {
            destination.discard(dir);
            return Err(error);
        }
        warning = Some(fetch::error_text(error));
    }
    let root = repo_root(dir)?;
    for remote in list(&root)? {
        sync.mark_authenticated(&root, &remote.name);
    }
    Ok(Cloned {
        path: root.display().to_string(),
        warning,
    })
}

// `GitInit`: if the folder is already under Git it has to be confirmed before creating another root.
pub fn init(ctx: &TaskContext, confirmed: bool) -> Result<InitOutcome, GitError> {
    let path = ctx.repo();
    if !path.is_absolute() || !path.is_dir() {
        return Err(invalid(format!(
            "The directory '{}' does not exist",
            path.display()
        )));
    }
    if !confirmed && under_git(path)? {
        return Ok(InitOutcome::UnderGit);
    }
    ctx.git_loud(&["init"])?;
    Ok(InitOutcome::Created {
        path: repo_root(path)?.display().to_string(),
    })
}

fn under_git(path: &Path) -> Result<bool, GitError> {
    match git::run(path, &["rev-parse", "--git-dir"]) {
        Ok(_) => Ok(true),
        Err(GitError::NotARepository { .. }) => Ok(false),
        Err(error) => Err(error),
    }
}

// `OpenGitExcludeAction`: if the file does not exist it does nothing. `true` if it opened it.
pub fn open_exclude_file(
    repo: &Path,
    open: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<bool, GitError> {
    let path = git::git_path(repo, EXCLUDE)?;
    if !path.is_file() {
        return Ok(false);
    }
    open(&path)
        .map_err(|error| GitError::Io(format!("Could not open '{}': {error}", path.display())))?;
    Ok(true)
}

// The app has no editor: the file is opened with the system's text editor.
pub fn open_in_text_editor(path: &Path) -> std::io::Result<()> {
    let status = Command::new(OPEN).arg("-t").arg(path).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("open exited with {status}")))
    }
}

// `RevealFileAction`: Finder opens the folder with the file selected. The path comes from a chooser
// or a dialog: only an absolute one, which is never taken for an option of `open`.
pub fn reveal_command(path: &Path) -> Result<Command, String> {
    if !path.is_absolute() {
        return Err(format!("The path '{}' is not absolute", path.display()));
    }
    let mut command = Command::new(OPEN);
    command.arg("-R").arg(path);
    Ok(command)
}

pub fn reveal_in_finder(path: &Path) -> Result<(), String> {
    let status = reveal_command(path)?
        .status()
        .map_err(|error| format!("Could not open Finder: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("open exited with {status}"))
    }
}

// One window per repository and the app does not open several yet: another window is another
// process. Inside an `.app`, `open -n` launches it with the path as `argv[1]`, which is what
// `initial_repo` reads. Outside one (development) there is nothing `open` knows how to launch, and
// the binary is run.
pub fn new_window_command(exe: &Path, repo: &Path) -> Result<Command, String> {
    if !repo.is_absolute() {
        return Err(format!("The path '{}' is not absolute", repo.display()));
    }
    let bundle = exe
        .ancestors()
        .find(|path| path.extension().is_some_and(|ext| ext == "app"));
    Ok(match bundle {
        Some(bundle) => {
            let mut command = Command::new(OPEN);
            command.arg("-n").arg(bundle).arg("--args").arg(repo);
            command
        }
        None => {
            let mut command = Command::new(exe);
            command.arg(repo);
            command
        }
    })
}

pub fn open_in_new_window(repo: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let mut child = new_window_command(&exe, repo)?
        .spawn()
        .map_err(|error| format!("Could not open another window: {error}"))?;
    // Without waiting for it to close: `open` finishes on launching, the binary directly does not.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::RepoLocks;
    use crate::console::Console;
    use crate::task::{Runner, TaskKind};
    use crate::test_support::TempRepo;
    use std::ffi::OsStr;
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::{mpsc, Arc};
    use std::time::Duration;
    use tempfile::TempDir;

    const WAIT: Duration = Duration::from_secs(60);

    struct Harness {
        console: Arc<Console>,
        runner: Runner,
        sync: Arc<BranchSync>,
    }

    impl Harness {
        fn new() -> Self {
            Self::with_binary(None)
        }

        fn with_binary(binary: Option<PathBuf>) -> Self {
            let console = Arc::<Console>::default();
            let runner = Runner::with(
                Arc::<RepoLocks>::default(),
                console.clone(),
                binary,
                Duration::from_secs(5),
            );
            Self {
                console,
                runner,
                sync: Arc::default(),
            }
        }

        // A task with `dir` as its working folder, the way the commands launch them.
        fn run<T: Send + 'static>(
            &self,
            dir: &Path,
            work: impl FnOnce(&TaskContext, &BranchSync) -> T + Send + 'static,
        ) -> T {
            let (send, receive) = mpsc::channel();
            let sync = self.sync.clone();
            self.runner
                .start(dir, TaskKind::Write, "test".to_string(), move |ctx| {
                    send.send(work(ctx, &sync)).unwrap();
                    Ok(())
                });
            receive.recv_timeout(WAIT).unwrap()
        }

        fn commands(&self) -> Vec<String> {
            let lines = self.console.lines();
            let commands = lines.into_iter().filter(|line| line.command);
            commands.map(|line| line.text).collect()
        }
    }

    fn bare() -> TempDir {
        let dir = TempDir::new().unwrap();
        let status = crate::test_support::git_command()
            .args(["init", "-q", "--bare", "-b", "main"])
            .arg(dir.path())
            .status()
            .unwrap();
        assert!(status.success());
        dir
    }

    fn text(path: &Path) -> String {
        path.to_str().unwrap().to_string()
    }

    #[test]
    fn add_rename_seturl_remove() {
        let harness = Harness::new();
        let repo = TempRepo::new();
        let first = text(bare().keep().as_path());
        let second = text(bare().keep().as_path());

        let added = harness.run(repo.path(), {
            let first = first.clone();
            move |ctx, _| add(ctx, "origin", &first)
        });
        assert_eq!(added, Ok(()));
        assert_eq!(repo.git(&["remote", "get-url", "origin"]).trim(), first);

        let edited = harness.run(repo.path(), {
            let second = second.clone();
            move |ctx, _| edit(ctx, "origin", "copy", &second)
        });
        assert_eq!(edited, Ok(()));
        assert_eq!(repo.git(&["remote"]).trim(), "copy");
        assert_eq!(repo.git(&["remote", "get-url", "copy"]).trim(), second);

        let removed = harness.run(repo.path(), |ctx, _| remove(ctx, "copy"));
        assert_eq!(removed, Ok(()));
        assert_eq!(repo.git(&["remote"]), "");
    }

    #[test]
    fn edit_only_what_changes() {
        let harness = Harness::new();
        let repo = TempRepo::new();
        let first = bare();
        let second = text(bare().path());
        repo.git(&["remote", "add", "origin", &text(first.path())]);

        harness.run(repo.path(), {
            let second = second.clone();
            move |ctx, _| edit(ctx, "origin", "origin", &second).unwrap()
        });
        harness.run(repo.path(), {
            let second = second.clone();
            move |ctx, _| edit(ctx, "origin", "another", &second).unwrap()
        });

        let commands = harness.commands();
        let has = |part: &str| commands.iter().any(|line| line.contains(part));
        assert!(has("remote set-url origin"), "{commands:?}");
        assert!(has("remote rename origin another"), "{commands:?}");
        assert!(!has("set-url another"), "{commands:?}");
        assert_eq!(repo.git(&["remote"]).trim(), "another");
    }

    #[test]
    fn editing_a_remote_that_does_not_exist_fails() {
        let harness = Harness::new();
        let repo = TempRepo::new();

        let result = harness.run(repo.path(), |ctx, _| edit(ctx, "origin", "another", "/x"));

        assert!(result.is_err());
    }

    #[test]
    fn order_with_origin_first() {
        let repo = TempRepo::new();
        let dir = bare();
        let url = text(dir.path());
        for name in ["zeta", "alpha", "origin", "beta"] {
            repo.git(&["remote", "add", name, &url]);
        }
        repo.git(&["config", "remote.beta.pushurl", "/other/path"]);

        let remotes = list(repo.path()).unwrap();

        let names: Vec<&str> = remotes.iter().map(|remote| remote.name.as_str()).collect();
        assert_eq!(names, ["origin", "alpha", "beta", "zeta"]);
        assert_eq!(remotes[0].url, url);
        assert_eq!(remotes[0].push_url, None);
        assert_eq!(remotes[2].push_url.as_deref(), Some("/other/path"));
    }

    #[test]
    fn a_remote_with_dots_in_the_name_is_listed_whole() {
        let repo = TempRepo::new();
        let dir = bare();
        repo.git(&["remote", "add", "my.remote", &text(dir.path())]);

        let remotes = list(repo.path()).unwrap();

        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].name, "my.remote");
    }

    #[test]
    fn without_remotes_the_list_is_empty() {
        assert_eq!(list(TempRepo::new().path()), Ok(vec![]));
    }

    #[test]
    fn name_or_url_with_a_leading_dash_do_not_reach_git() {
        let harness = Harness::new();
        let repo = TempRepo::new();
        let url = text(bare().path());

        let results = harness.run(repo.path(), move |ctx, _| {
            [
                add(ctx, "-oProxyCommand=touch", &url),
                add(ctx, "origin", "-oProxyCommand=touch"),
                add(ctx, "", &url),
                add(ctx, "origin", ""),
                add(ctx, "name with spaces", &url),
                remove(ctx, "-f"),
                edit(ctx, "origin", "-x", &url),
            ]
        });

        for result in &results {
            assert!(result.is_err(), "{results:?}");
        }
        assert_eq!(repo.git(&["remote"]), "");
        let commands = harness.commands();
        assert!(
            !commands.iter().any(|line| line.contains("ProxyCommand")),
            "{commands:?}"
        );
    }

    #[test]
    fn url_is_tested_with_ls_remote() {
        let harness = Harness::new();
        let repo = TempRepo::new();
        let origin = bare();
        let url = text(origin.path());

        let ok = harness.run(repo.path(), {
            let url = url.clone();
            move |ctx, sync| test_url(ctx, sync, &url, Some("origin"))
        });

        assert_eq!(ok, Ok(()));
        let commands = harness.commands();
        let expected = format!("ls-remote {url}");
        assert!(
            commands.iter().any(|line| line.ends_with(&expected)),
            "{commands:?}"
        );
        assert!(harness.sync.is_authenticated(repo.path(), "origin"));

        let missing = TempDir::new().unwrap();
        let gone = text(&missing.path().join("does-not-exist"));
        let failed = harness.run(repo.path(), move |ctx, sync| {
            test_url(ctx, sync, &gone, Some("broken"))
        });
        let Err(GitError::Failed { stderr, .. }) = failed else {
            panic!("{failed:?}");
        };
        assert!(
            stderr.contains("does not appear to be a git repository"),
            "{stderr}"
        );
        assert!(!harness.sync.is_authenticated(repo.path(), "broken"));
    }

    #[test]
    fn url_with_a_leading_dash_is_rejected() {
        let harness = Harness::new();
        let repo = TempRepo::new();
        let marker = repo.path().join("marker");
        let url = format!("-oProxyCommand=touch {}", marker.display());
        let dir = repo.path().join("target");

        let (tested, cloned) = harness.run(repo.path(), move |ctx, sync| {
            (
                test_url(ctx, sync, &url, None),
                clone(ctx, sync, &url, &dir),
            )
        });

        assert!(tested.is_err());
        assert!(cloned.is_err());
        assert!(!marker.exists());
        assert!(harness.commands().is_empty(), "{:?}", harness.commands());
    }

    #[test]
    fn ext_url_is_rejected() {
        let harness = Harness::new();
        let repo = TempRepo::new();
        let marker = repo.path().join("marker");
        let url = format!("ext::sh -c touch% {}", marker.display());
        let parent = TempDir::new().unwrap();
        let dir = parent.path().join("target");

        let tested = harness.run(repo.path(), {
            let url = url.clone();
            move |ctx, sync| test_url(ctx, sync, &url, None)
        });
        let cloned = harness.run(parent.path(), {
            let dir = dir.clone();
            move |ctx, sync| clone(ctx, sync, &url, &dir)
        });

        assert!(tested.is_err());
        assert!(cloned.is_err());
        assert!(!marker.exists(), "git ran the ext:: command");
        assert!(!dir.exists());
        // The protection is git's: the app does not change which transports are allowed.
        for line in harness.commands() {
            assert!(!line.contains("protocol"), "{line}");
        }
    }

    #[test]
    fn clone_with_recurse_submodules() {
        let harness = Harness::new();
        let origin = TempRepo::new();
        origin.commit("a.txt", b"1\n", "one");
        let parent = TempDir::new().unwrap();
        let dir = parent.path().join("copy");
        let url = text(origin.path());

        let cloned = harness
            .run(parent.path(), {
                let (url, dir) = (url.clone(), dir.clone());
                move |ctx, sync| clone(ctx, sync, &url, &dir)
            })
            .unwrap();

        let commands = harness.commands();
        let expected = format!("clone --progress --recurse-submodules {url} -- copy");
        assert!(
            commands.iter().any(|line| line.ends_with(&expected)),
            "{commands:?}"
        );
        assert_eq!(
            fs::canonicalize(&cloned.path).unwrap(),
            fs::canonicalize(&dir).unwrap()
        );
        assert!(dir.join("a.txt").exists());
        let root = PathBuf::from(&cloned.path);
        assert!(harness.sync.is_authenticated(&root, "origin"));
        assert_eq!(cloned.warning, None);
    }

    #[test]
    fn clone_into_a_directory_with_a_dash_goes_after_the_double_dash() {
        let harness = Harness::new();
        let origin = TempRepo::new();
        origin.commit("a.txt", b"1\n", "one");
        let parent = TempDir::new().unwrap();
        let dir = parent.path().join("-odd");

        let cloned = harness.run(parent.path(), {
            let url = text(origin.path());
            let dir = dir.clone();
            move |ctx, sync| clone(ctx, sync, &url, &dir)
        });

        assert!(cloned.is_ok(), "{cloned:?}");
        assert!(dir.join("a.txt").exists());
    }

    #[test]
    fn failing_clone_does_not_leave_the_directory_it_created() {
        let harness = Harness::new();
        let parent = TempDir::new().unwrap();
        let dir = parent.path().join("copy");
        let url = text(&parent.path().join("does-not-exist"));

        let cloned = harness.run(parent.path(), {
            let dir = dir.clone();
            move |ctx, sync| clone(ctx, sync, &url, &dir)
        });

        assert!(cloned.is_err());
        assert!(!dir.exists());
    }

    #[test]
    fn cancelled_clone_deletes_only_what_it_created() {
        let parent = TempDir::new().unwrap();

        // It did not exist: it goes entirely.
        let new = parent.path().join("new");
        let created = Destination::prepare(&new);
        fs::create_dir_all(new.join(".git")).unwrap();
        fs::write(new.join(".git/HEAD"), "x").unwrap();
        created.discard(&new);
        assert!(!new.exists());

        // It existed and was empty: the folder stays, without what the clone left.
        let empty = parent.path().join("empty");
        fs::create_dir(&empty).unwrap();
        let kept = Destination::prepare(&empty);
        fs::create_dir_all(empty.join(".git")).unwrap();
        fs::write(empty.join("a.txt"), "x").unwrap();
        kept.discard(&empty);
        assert!(empty.is_dir());
        assert_eq!(fs::read_dir(&empty).unwrap().count(), 0);

        // It existed with the user's things: git does not clone there, and nothing is touched.
        let full = parent.path().join("full");
        fs::create_dir(&full).unwrap();
        fs::write(full.join("mine.txt"), "mine").unwrap();
        let untouched = Destination::prepare(&full);
        untouched.discard(&full);
        assert_eq!(fs::read_to_string(full.join("mine.txt")).unwrap(), "mine");
    }

    #[test]
    fn clone_into_a_directory_with_files_fails_and_keeps_them() {
        let harness = Harness::new();
        let origin = TempRepo::new();
        origin.commit("a.txt", b"1\n", "one");
        let parent = TempDir::new().unwrap();
        let dir = parent.path().join("full");
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("mine.txt"), "mine").unwrap();

        let cloned = harness.run(parent.path(), {
            let url = text(origin.path());
            let dir = dir.clone();
            move |ctx, sync| clone(ctx, sync, &url, &dir)
        });

        assert!(cloned.is_err());
        assert_eq!(fs::read_to_string(dir.join("mine.txt")).unwrap(), "mine");
    }

    // `Read`: it does not wait for its turn, so the work runs even if it is already cancelled.
    #[test]
    fn cancelled_clone_leaves_no_folders_and_respects_the_one_that_already_existed() {
        let harness = Harness::new();
        let origin = TempRepo::new();
        origin.commit("a.txt", b"1\n", "one");
        let parent = TempDir::new().unwrap();
        let (new, existing) = (parent.path().join("new"), parent.path().join("empty"));
        fs::create_dir(&existing).unwrap();

        let (send, receive) = mpsc::channel();
        let (release, wait) = mpsc::channel::<()>();
        let url = text(origin.path());
        let targets = (new.clone(), existing.clone());
        let task = harness.runner.start(
            parent.path(),
            TaskKind::Read,
            "clone".to_string(),
            move |ctx| {
                wait.recv().unwrap();
                let sync = BranchSync::default();
                let results = (
                    clone(ctx, &sync, &url, &targets.0),
                    clone(ctx, &sync, &url, &targets.1),
                );
                send.send(results).unwrap();
                Ok(())
            },
        );
        harness.runner.cancel(task);
        release.send(()).unwrap();

        let (first, second) = receive.recv_timeout(WAIT).unwrap();
        assert_eq!(first, Err(GitError::Cancelled));
        assert_eq!(second, Err(GitError::Cancelled));
        assert!(!new.exists());
        assert!(existing.is_dir());
    }

    // A source with a submodule whose URL does not exist: the clone finishes whole and
    // `--recurse-submodules` makes git exit with 1.
    fn source_with_broken_submodule() -> TempRepo {
        let origin = TempRepo::new();
        let head = origin.commit("a.txt", b"1\n", "one");
        fs::write(
            origin.path().join(".gitmodules"),
            "[submodule \"sub\"]\n\tpath = sub\n\turl = /does/not/exist/sub\n",
        )
        .unwrap();
        origin.git(&[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{head},sub"),
        ]);
        origin.git(&["add", ".gitmodules"]);
        origin.git(&["commit", "-q", "-m", "with submodule"]);
        origin
    }

    #[test]
    fn a_failing_submodule_keeps_the_clone_and_says_so() {
        let harness = Harness::new();
        let origin = source_with_broken_submodule();
        let parent = TempDir::new().unwrap();
        let dir = parent.path().join("copy");

        let cloned = harness
            .run(parent.path(), {
                let url = text(origin.path());
                let dir = dir.clone();
                move |ctx, sync| clone(ctx, sync, &url, &dir)
            })
            .unwrap();

        assert!(dir.join("a.txt").exists());
        assert!(dir.join(".git/HEAD").is_file());
        let warning = cloned.warning.expect("the submodule failure is reported");
        assert!(warning.contains("sub"), "{warning}");
        let root = PathBuf::from(&cloned.path);
        assert!(harness.sync.is_authenticated(&root, "origin"));
    }

    // A test git that starts a clone and never finishes. It writes a `HEAD`: without the cancellation,
    // the folder would look like a whole clone.
    fn slow_git() -> (TempDir, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let script = dir.path().join("git");
        fs::write(
            &script,
            "#!/bin/sh\nfor last; do :; done\nmkdir -p \"$last/.git\"\necho 'ref: refs/heads/main' > \"$last/.git/HEAD\"\necho x > \"$last/.git/partial\"\nexec sleep 60\n",
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        (dir, script)
    }

    #[test]
    fn cancelling_with_git_running_deletes_what_it_created_and_respects_the_empty_folder() {
        let (_scripts, script) = slow_git();
        let harness = Harness::with_binary(Some(script));
        let parent = TempDir::new().unwrap();
        let (new, existing) = (parent.path().join("new"), parent.path().join("empty"));
        fs::create_dir(&existing).unwrap();

        for dir in [&new, &existing] {
            let (send, receive) = mpsc::channel();
            let target = dir.clone();
            let task = harness.runner.start(
                parent.path(),
                TaskKind::Write,
                "clone".to_string(),
                move |ctx| {
                    let result = clone(ctx, &BranchSync::default(), "/x", &target);
                    send.send(result).unwrap();
                    Ok(())
                },
            );
            let started = dir.join(".git/partial");
            let deadline = std::time::Instant::now() + WAIT;
            while !started.exists() {
                assert!(std::time::Instant::now() < deadline, "git did not start");
                std::thread::sleep(Duration::from_millis(20));
            }
            harness.runner.cancel(task);

            let result = receive.recv_timeout(WAIT).unwrap();
            assert_eq!(result, Err(GitError::Cancelled));
        }

        assert!(!new.exists());
        assert!(existing.is_dir());
        assert_eq!(fs::read_dir(&existing).unwrap().count(), 0);
    }

    #[test]
    fn the_leading_tilde_expands_to_the_users_directory() {
        let home = Path::new("/Users/ana");

        assert_eq!(expand_home("~", Some(home)), PathBuf::from("/Users/ana"));
        assert_eq!(
            expand_home("~/code/repo", Some(home)),
            PathBuf::from("/Users/ana/code/repo")
        );
        assert_eq!(
            expand_home("/abs/~/x", Some(home)),
            PathBuf::from("/abs/~/x")
        );
        assert_eq!(expand_home("~another/x", Some(home)), PathBuf::from("~another/x"));
        assert_eq!(expand_home("~/x", None), PathBuf::from("~/x"));
    }

    #[test]
    fn the_missing_parent_folder_is_created_and_the_target_is_not() {
        let base = TempDir::new().unwrap();
        let dir = base.path().join("a/b/copy");

        create_parent(&dir).unwrap();

        assert!(base.path().join("a/b").is_dir());
        assert!(!dir.exists());
    }

    #[test]
    fn if_the_parent_is_a_file_it_cannot_be_created() {
        let base = TempDir::new().unwrap();
        fs::write(base.path().join("file"), "x").unwrap();

        assert!(create_parent(&base.path().join("file/copy")).is_err());
    }

    #[test]
    fn init_creates_a_repository() {
        let harness = Harness::new();
        let dir = TempDir::new().unwrap();

        let outcome = harness.run(dir.path(), |ctx, _| init(ctx, false)).unwrap();

        let InitOutcome::Created { path } = outcome else {
            panic!("{outcome:?}");
        };
        assert_eq!(
            fs::canonicalize(&path).unwrap(),
            fs::canonicalize(dir.path()).unwrap()
        );
        assert!(dir.path().join(".git").is_dir());
        let commands = harness.commands();
        assert!(
            commands.iter().any(|line| line.ends_with(" init")),
            "{commands:?}"
        );
    }

    #[test]
    fn init_in_a_repository_warns() {
        let harness = Harness::new();
        let repo = TempRepo::new();
        let inner = repo.path().join("sub");
        fs::create_dir(&inner).unwrap();

        let warned = harness.run(&inner, |ctx, _| init(ctx, false)).unwrap();
        assert_eq!(warned, InitOutcome::UnderGit);
        assert!(!inner.join(".git").exists());

        let confirmed = harness.run(&inner, |ctx, _| init(ctx, true)).unwrap();
        assert!(matches!(confirmed, InitOutcome::Created { .. }));
        assert!(inner.join(".git").is_dir());
    }

    #[test]
    fn nonexistent_exclude_is_silent() {
        let repo = TempRepo::new();
        let exclude = repo.path().join(".git/info/exclude");
        let _ = fs::remove_file(&exclude);
        let mut opened = Vec::new();

        let result = open_exclude_file(repo.path(), |path| {
            opened.push(path.to_path_buf());
            Ok(())
        });

        assert_eq!(result, Ok(false));
        assert!(opened.is_empty());
    }

    #[test]
    fn existing_exclude_is_opened() {
        let repo = TempRepo::new();
        let exclude = repo.path().join(".git/info/exclude");
        fs::create_dir_all(exclude.parent().unwrap()).unwrap();
        fs::write(&exclude, "*.log\n").unwrap();
        let mut opened = Vec::new();

        let result = open_exclude_file(repo.path(), |path| {
            opened.push(path.to_path_buf());
            Ok(())
        });

        assert_eq!(result, Ok(true));
        assert_eq!(opened.len(), 1);
        assert_eq!(
            fs::canonicalize(&opened[0]).unwrap(),
            fs::canonicalize(&exclude).unwrap()
        );
    }

    fn args(command: &Command) -> Vec<&OsStr> {
        command.get_args().collect()
    }

    #[test]
    fn new_window_with_bundle_uses_open_n_and_the_path_goes_in_argv() {
        let exe = Path::new("/Applications/Ramify.app/Contents/MacOS/ramify");

        let command = new_window_command(exe, Path::new("/tmp/my repo")).unwrap();

        assert_eq!(command.get_program(), "/usr/bin/open");
        assert_eq!(
            args(&command),
            ["-n", "/Applications/Ramify.app", "--args", "/tmp/my repo"]
        );
    }

    #[test]
    fn new_window_without_bundle_launches_the_binary_with_the_path() {
        let exe = Path::new("/project/target/debug/ramify");

        let command = new_window_command(exe, Path::new("/tmp/repo")).unwrap();

        assert_eq!(command.get_program(), exe.as_os_str());
        assert_eq!(args(&command), ["/tmp/repo"]);
    }

    #[test]
    fn new_window_rejects_relative_or_dashed_paths() {
        let exe = Path::new("/Applications/Ramify.app/Contents/MacOS/ramify");

        assert!(new_window_command(exe, Path::new("-n")).is_err());
        assert!(new_window_command(exe, Path::new("relative/repo")).is_err());
        assert!(new_window_command(exe, Path::new("")).is_err());
    }

    #[test]
    fn reveal_in_finder_is_open_dash_r_with_the_path_as_an_argument() {
        let command = reveal_command(Path::new("/tmp/my patch.patch")).unwrap();

        assert_eq!(command.get_program(), "/usr/bin/open");
        assert_eq!(args(&command), ["-R", "/tmp/my patch.patch"]);
    }

    #[test]
    fn reveal_in_finder_refuses_a_relative_path() {
        assert!(reveal_command(Path::new("-a")).is_err());
        assert!(reveal_command(Path::new("relative/a.patch")).is_err());
        assert!(reveal_command(Path::new("")).is_err());
    }
}
