// `Git.Pull`, with the steps of `GitPull` and `GitMergeAction`. A pull that merges is done by
// `git pull` itself, without saving anything: whatever collides with local changes is a git error.
// With `--rebase`, `git pull` is not launched, but the usual Update.
use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::branch_info::{self, BranchSync};
use crate::branches::{self, Repo};
use crate::commit;
use crate::fetch;
use crate::git::{self, GitError};
use crate::preserve;
use crate::settings::UpdateMethod;
use crate::storage;
use crate::task::{TaskContext, TaskKind};
use crate::update::{
    self, Before, Outcome, Tracked, UpdateOptions, UpdateResult, LOCAL_CHANGES_OVERWRITTEN,
};

const HEAD: &str = "HEAD";
const OPTIONS_FILE: &str = "pull-options.json";
const PULL_FAILED: &str = "Git Pull failed";
const PULL_CANCELLED: &str = "Pull canceled";
const LOCAL_CHANGES_FAILED: &str = "Git Pull Failed";
const UNTRACKED_FILES_PREVENT: &str = "Untracked Files Prevent Pull";
const MOVE_OR_COMMIT: &str = "Move or commit them before pull";
const PULL_PARTLY_APPLIED: &str =
    "Your staged changes were kept, so the pull may be partly applied. Review the changes before you continue.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PullOption {
    Rebase,
    FfOnly,
    NoFf,
    Squash,
    NoCommit,
    NoVerify,
}

impl PullOption {
    fn flag(self) -> &'static str {
        match self {
            Self::Rebase => "--rebase",
            Self::FfOnly => "--ff-only",
            Self::NoFf => "--no-ff",
            Self::Squash => "--squash",
            Self::NoCommit => "--no-commit",
            Self::NoVerify => "--no-verify",
        }
    }
}

// What the dialog shows: full names of the remote branches, with their remote in front.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullInfo {
    pub branch: Option<String>,
    pub remotes: Vec<String>,
    pub remote_branches: Vec<String>,
    pub tracked: Option<(String, String)>,
}

// With `--rebase` Update takes over, which takes the lock only when it needs it; otherwise `git pull`
// changes the tree from start to finish.
// `GitPullSettings`, per repository. Like `GitPullDialog.loadSettings`, they are loaded as they are:
// what is incompatible stays and shows greyed out. Only what is no longer an option is discarded.
pub fn read_options(dir: &Path) -> Vec<PullOption> {
    let values: Vec<serde_json::Value> = fs::read(dir.join(OPTIONS_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let mut options: Vec<PullOption> = Vec::new();
    for value in values {
        match serde_json::from_value(value) {
            Ok(option) if !options.contains(&option) => options.push(option),
            _ => {}
        }
    }
    options
}

pub fn write_options(dir: &Path, options: &[PullOption]) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let json = serde_json::to_vec(options).map_err(io::Error::other)?;
    storage::write_atomic(&dir.join(OPTIONS_FILE), &json)
}

pub fn task_kind(options: &[PullOption]) -> TaskKind {
    if options.contains(&PullOption::Rebase) {
        TaskKind::Read
    } else {
        TaskKind::Write
    }
}

pub fn info(path: &Path) -> Result<PullInfo, GitError> {
    let list = branch_info::list(path)?;
    let output = git::run(path, &["remote"])?;
    let remotes = String::from_utf8_lossy(&output)
        .lines()
        .map(str::to_string)
        .collect();
    let tracked = match &list.current {
        Some(branch) => update::tracked_branch(path, branch)?,
        None => None,
    };
    Ok(PullInfo {
        branch: list.current,
        remotes,
        remote_branches: list.remote,
        tracked,
    })
}

// The remote from the interface must be one of the configured ones: a URL, or an `ext::`, would
// reach git as the command's target.
pub fn check_remote(ctx: &TaskContext, remote: &str) -> Result<(), GitError> {
    branches::check_argument(remote, "The remote")?;
    if ctx
        .git(&["remote"])?
        .stdout
        .lines()
        .any(|name| name == remote)
    {
        Ok(())
    } else {
        Err(GitError::Io(format!("Remote {remote} is not configured")))
    }
}

// A leading `+` would turn the branch into a forced refspec.
pub(crate) fn check_branch(ctx: &TaskContext, branch: &str) -> Result<(), GitError> {
    if branch.starts_with('+') {
        return Err(GitError::Io(format!("Branch name {branch} is not valid")));
    }
    branches::check_branch(ctx, branch)
}

// A merge that is not committed (`--no-commit`, `--squash`) neither moves HEAD nor receives commits,
// but it brings the remote branch's changes: those from HEAD to it since their common base.
pub(crate) fn incoming_files(ctx: &TaskContext, target: &str) -> u32 {
    let revisions = format!("{HEAD}...{target}");
    let args = [
        "diff",
        "--name-only",
        "--diff-filter=ADMRUX",
        "--no-renames",
        &revisions,
    ];
    ctx.git_to_the_end(&args)
        .map_or(0, |output| output.stdout.lines().count() as u32)
}

// `GitMergeAction.handleResult`, without the conflict resolver: what is half-done can only be
// reported, and `Abort` undoes it. A merge that was already unfinished before starting is not a new conflict.
fn failure(repo: &Repo, error: GitError, unfinished_before: bool) -> UpdateResult {
    if !unfinished_before && update::stopped_midway(repo.path) {
        return UpdateResult {
            in_progress: true,
            ..Outcome::Conflicts.into()
        };
    }
    let stderr = branches::stderr_of(&error);
    if stderr.contains(preserve::UNTRACKED_FILES) {
        let files = preserve::listed_files(stderr, preserve::UNTRACKED_FILES);
        return update::failed(
            UNTRACKED_FILES_PREVENT,
            update::with_files(MOVE_OR_COMMIT, &files),
        );
    }
    if stderr.contains(branches::LOCAL_CHANGES) {
        let files = branches::overwritten_files(stderr);
        let text = update::with_files(LOCAL_CHANGES_OVERWRITTEN, &files);
        return update::failed(LOCAL_CHANGES_FAILED, text);
    }
    update::failed(PULL_FAILED, fetch::error_text(error))
}

fn index_tree(ctx: &TaskContext) -> Option<String> {
    let output = ctx.git_to_the_end(&["write-tree"]).ok()?;
    Some(output.stdout.trim().to_string())
}

// A dead `git merge` can leave what was merged without `MERGE_HEAD` or unresolved files. Pull saves
// nothing, and `reset --merge` would take away what was staged: it is only undone if nothing was staged.
fn after_cancel(
    repo: &Repo,
    index_before: Option<&str>,
    head_tree: &str,
    unfinished_before: bool,
) -> Result<UpdateResult, GitError> {
    if unfinished_before {
        return Err(GitError::Cancelled);
    }
    // With `pull.rebase`, git pull is a rebase, and killing it halfway leaves `rebase-merge/`: nobody
    // else undoes it. As in Update, Cancel aborts it (with a copy). The one cancelled with the editor
    // open does not get here: git undoes it by itself.
    if commit::rebase_in_progress(repo.path) {
        update::abort(repo)?;
        return Err(GitError::Cancelled);
    }
    let text = if index_before == Some(head_tree) {
        match update::undo_merge(repo) {
            Ok(()) => return Err(GitError::Cancelled),
            Err(error) => fetch::error_text(error),
        }
    } else if index_before.is_some() && index_tree(repo.ctx).as_deref() == index_before {
        return Err(GitError::Cancelled);
    } else {
        PULL_PARTLY_APPLIED.to_string()
    };
    Ok(UpdateResult {
        in_progress: update::stopped_midway(repo.path),
        ..update::failed(PULL_CANCELLED, text)
    })
}

fn merge(
    repo: &Repo,
    sync: &BranchSync,
    remote: &str,
    branch: &str,
    options: &[PullOption],
) -> Result<UpdateResult, GitError> {
    let ctx = repo.ctx;
    let tracked = Tracked {
        branch: branches::current_name(repo.path)?.unwrap_or_else(|| HEAD.to_string()),
        remote: Some(remote.to_string()),
        target: format!("{remote}/{branch}"),
    };
    let before = Before {
        base: update::merge_base(ctx, &tracked),
        head: ctx.git(&["rev-parse", HEAD])?.stdout.trim().to_string(),
    };
    let unfinished_before = update::stopped_midway(repo.path);
    let index_before = index_tree(ctx);
    let head_tree = ctx
        .git(&["rev-parse", "HEAD^{tree}"])?
        .stdout
        .trim()
        .to_string();
    let mut flags: Vec<PullOption> = options.to_vec();
    flags.sort();
    flags.dedup();
    let mut args = vec!["pull", "--no-stat"];
    args.extend(flags.iter().map(|option| option.flag()));
    args.extend(["-v", "--progress", remote, branch]);
    // With `pull.rebase=interactive`, git asks for the rebase list: the editor is the app.
    match ctx.git_editing(&args, ctx.asking_editor()) {
        Ok(_) => {
            sync.mark_authenticated(repo.path, remote);
            let result = update::with_counts(ctx, Outcome::Success.into(), &tracked, before);
            let unfinished = [PullOption::NoCommit, PullOption::Squash];
            if result.commits == 0 && flags.iter().any(|option| unfinished.contains(option)) {
                return Ok(UpdateResult {
                    files: incoming_files(ctx, &tracked.target),
                    ..result
                });
            }
            Ok(result)
        }
        Err(GitError::Cancelled) => {
            after_cancel(repo, index_before.as_deref(), &head_tree, unfinished_before)
        }
        Err(error) => Ok(failure(repo, error, unfinished_before)),
    }
}

// `before_changes` is only used by Update, with `--rebase`: there the task does not hold the lock.
pub fn pull(
    repo: &Repo,
    sync: &BranchSync,
    remote: &str,
    branch: &str,
    options: &[PullOption],
    before_changes: impl FnOnce() -> Result<(), GitError>,
) -> UpdateResult {
    let checked = check_remote(repo.ctx, remote).and_then(|()| check_branch(repo.ctx, branch));
    if let Err(error) = checked {
        return update::failed(PULL_FAILED, fetch::error_text(error));
    }
    // `Pull…` runs `git pull` in IntelliJ, which never asks about merge commits or published
    // commits: both are answered for the branch it pulls into. Without one, Update stops at the
    // detached HEAD. If another branch is current by then, Update asks for it and `Pull…` cancels.
    if options.contains(&PullOption::Rebase) {
        let current = branches::current_name(repo.path).ok().flatten();
        let options = UpdateOptions {
            target: Some(format!("{remote}/{branch}")),
            rebase_merges: current.clone(),
            published_confirmed: current,
            ..Default::default()
        };
        return update::update_project(repo, sync, UpdateMethod::Rebase, options, before_changes);
    }
    match merge(repo, sync, remote, branch, options) {
        Ok(result) => result,
        Err(GitError::Cancelled) => UpdateResult {
            in_progress: update::stopped_midway(repo.path),
            ..Outcome::Cancelled.into()
        },
        Err(error) => update::failed(PULL_FAILED, fetch::error_text(error)),
    }
}

// `GitPullBranchAction`: `GitUpdateExecutionProcess` with the current branch and the chosen remote
// one, which is an Update towards it, with its fetch, its saving and its notices.
pub fn pull_into(
    repo: &Repo,
    sync: &BranchSync,
    remote_branch: &str,
    method: UpdateMethod,
    before_changes: impl FnOnce() -> Result<(), GitError>,
) -> UpdateResult {
    let remote = branches::check_argument(remote_branch, "The branch")
        .and_then(|()| branches::split_remote(repo.path, remote_branch));
    match remote {
        Ok(Some(_)) => {}
        Ok(None) => {
            let text = format!("Remote branch {remote_branch} not found");
            return update::failed(update::UPDATE_FAILED, text);
        }
        Err(error) => return update::failed(update::UPDATE_FAILED, fetch::error_text(error)),
    }
    let options = UpdateOptions {
        target: Some(remote_branch.to_string()),
        ..Default::default()
    };
    update::update_project(repo, sync, method, options, before_changes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::RepoLocks;
    use crate::console::Console;
    use crate::snapshots;
    use crate::task::Runner;
    use crate::test_support::TempRepo;
    use crate::update::{Aborted, Outcome};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};
    use tempfile::TempDir;

    const WAIT: Duration = Duration::from_secs(60);

    struct Harness {
        repo: TempRepo,
        colleague: TempRepo,
        origin: TempDir,
        data: TempDir,
        console: Arc<Console>,
        runner: Runner,
        sync: Arc<BranchSync>,
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

    impl Harness {
        fn new() -> Self {
            let origin = bare();
            let url = origin.path().to_str().unwrap();
            let repo = TempRepo::new();
            repo.commit("a.txt", b"1\n2\n3\n", "one");
            repo.commit("b.txt", b"b\n", "two");
            repo.git(&["remote", "add", "origin", url]);
            repo.git(&["push", "-q", "-u", "origin", "main"]);
            // Without this git refuses to merge divergent histories: the user has it configured.
            repo.git(&["config", "pull.rebase", "false"]);
            let colleague = TempRepo::new();
            colleague.git(&["remote", "add", "origin", url]);
            colleague.git(&["fetch", "-q", "origin"]);
            colleague.git(&["checkout", "-q", "-B", "main", "origin/main"]);
            let console = Arc::<Console>::default();
            let runner = Runner::with(
                Arc::<RepoLocks>::default(),
                console.clone(),
                None,
                Duration::from_secs(5),
            );
            Self {
                repo,
                colleague,
                origin,
                data: TempDir::new().unwrap(),
                console,
                runner,
                sync: Arc::default(),
            }
        }

        fn pull(&self, remote: &str, branch: &str, options: &[PullOption]) -> UpdateResult {
            let (send, receive) = mpsc::channel();
            let path = self.repo.path().to_path_buf();
            let data = self.data.path().to_path_buf();
            let sync = self.sync.clone();
            let (remote, branch) = (remote.to_string(), branch.to_string());
            let options = options.to_vec();
            self.runner.start(
                self.repo.path(),
                task_kind(&options),
                "Pull".to_string(),
                move |ctx| {
                    let repo = Repo {
                        ctx,
                        path: &path,
                        data: &data,
                    };
                    let result = pull(&repo, &sync, &remote, &branch, &options, || Ok(()));
                    send.send(result).unwrap();
                    Ok(())
                },
            );
            receive.recv_timeout(WAIT).unwrap()
        }

        fn pull_into(&self, remote_branch: &str, method: UpdateMethod) -> UpdateResult {
            let (send, receive) = mpsc::channel();
            let path = self.repo.path().to_path_buf();
            let data = self.data.path().to_path_buf();
            let sync = self.sync.clone();
            let remote_branch = remote_branch.to_string();
            self.runner.start(
                self.repo.path(),
                TaskKind::Read,
                "Update".to_string(),
                move |ctx| {
                    let repo = Repo {
                        ctx,
                        path: &path,
                        data: &data,
                    };
                    let result = pull_into(&repo, &sync, &remote_branch, method, || Ok(()));
                    send.send(result).unwrap();
                    Ok(())
                },
            );
            receive.recv_timeout(WAIT).unwrap()
        }

        // What someone else publishes on `other`.
        fn incoming_on_other(&self) -> String {
            self.colleague.git(&["checkout", "-q", "-b", "other"]);
            let head = self.colleague.commit("o.txt", b"from other\n", "in other");
            self.colleague.git(&["push", "-q", "origin", "other"]);
            head
        }

        // Cancels as soon as `mark` exists, which the test's hook or script writes.
        fn pull_cancelled_at(&self, mark: &Path, options: &[PullOption]) -> UpdateResult {
            let (send, receive) = mpsc::channel();
            let path = self.repo.path().to_path_buf();
            let data = self.data.path().to_path_buf();
            let sync = self.sync.clone();
            let options = options.to_vec();
            let id = self.runner.start(
                self.repo.path(),
                task_kind(&options),
                "Pull".to_string(),
                move |ctx| {
                    let repo = Repo {
                        ctx,
                        path: &path,
                        data: &data,
                    };
                    let result = pull(&repo, &sync, "origin", "main", &options, || Ok(()));
                    send.send(result).unwrap();
                    Ok(())
                },
            );
            let deadline = Instant::now() + WAIT;
            while !mark.exists() {
                assert!(Instant::now() < deadline, "the command did not arrive");
                thread::sleep(Duration::from_millis(20));
            }
            self.runner.cancel(id);
            receive.recv_timeout(WAIT).unwrap()
        }

        fn slow_script(&self, name: &str, mark: &Path) -> PathBuf {
            let path = self.repo.path().join(".git").join(name);
            let text = format!("#!/bin/sh\n: > '{}'\nsleep 30\n", mark.display());
            fs::write(&path, text).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        }

        fn status(&self) -> String {
            self.git(&["status", "--porcelain", "--untracked-files=no"])
                .trim_end()
                .to_string()
        }

        fn abort(&self) -> Result<Aborted, GitError> {
            let path = self.repo.path().to_path_buf();
            let data = self.data.path().to_path_buf();
            crate::test_support::in_task(&self.runner, self.repo.path(), move |ctx| {
                update::abort(&Repo {
                    ctx,
                    path: &path,
                    data: &data,
                })
            })
        }

        fn incoming(&self, file: &str, contents: &str) -> String {
            let head = self.colleague.commit(file, contents.as_bytes(), "from outside");
            self.colleague.git(&["push", "-q", "origin", "main"]);
            head
        }

        fn write(&self, file: &str, contents: &str) {
            fs::write(self.repo.path().join(file), contents).unwrap();
        }

        fn read(&self, file: &str) -> String {
            fs::read_to_string(self.repo.path().join(file)).unwrap()
        }

        fn git(&self, args: &[&str]) -> String {
            self.repo.git(args)
        }

        fn head(&self) -> String {
            self.git(&["rev-parse", "HEAD"]).trim().to_string()
        }

        fn ran(&self, command: &str) -> bool {
            self.console
                .lines()
                .iter()
                .any(|line| line.command && line.text.ends_with(command))
        }

        fn ran_anything_with(&self, word: &str) -> bool {
            self.console
                .lines()
                .iter()
                .any(|line| line.command && line.text.split(' ').any(|part| part == word))
        }

        fn in_git_dir(&self, name: &str) -> bool {
            self.repo.path().join(".git").join(name).exists()
        }

        // One commit of our own and another from outside, in different files.
        fn diverged(&self) {
            self.repo.commit("own.txt", b"mine\n", "own");
            self.incoming("c.txt", "theirs\n");
        }

        // Both touch `a.txt`.
        fn diverged_on_a_txt(&self) {
            self.repo.commit("a.txt", b"mine\n2\n3\n", "own");
            self.incoming("a.txt", "theirs\n2\n3\n");
        }
    }

    // With `pull.rebase` in the configuration, `git pull` is a rebase that Cancel can kill halfway:
    // `rebase-merge/` stays and nothing else undoes it.
    #[test]
    fn cancelling_a_pull_that_is_a_rebase_aborts_it() {
        let harness = Harness::new();
        harness.diverged();
        harness.git(&["config", "pull.rebase", "true"]);
        let head = harness.head();
        let scratch = TempDir::new().unwrap();
        let mark = scratch.path().join("mark");
        harness.slow_script("hooks/post-checkout", &mark);

        let result = harness.pull_cancelled_at(&mark, &[]);

        assert_eq!(result.outcome, Outcome::Cancelled);
        assert!(!result.in_progress);
        assert!(!harness.in_git_dir("rebase-merge"));
        assert_eq!(harness.head(), head);
    }

    // git runs `pre-merge-commit` with what was merged already in the index and without `MERGE_HEAD`.
    #[test]
    fn cancelling_a_clean_merge_with_an_empty_stage_undoes_what_was_merged() {
        let harness = Harness::new();
        harness.diverged();
        harness.write("b.txt", "local\n");
        let head = harness.head();
        let scratch = TempDir::new().unwrap();
        let mark = scratch.path().join("mark");
        harness.slow_script("hooks/pre-merge-commit", &mark);

        let result = harness.pull_cancelled_at(&mark, &[PullOption::NoFf]);

        assert_eq!(result.outcome, Outcome::Cancelled);
        assert!(!result.in_progress);
        assert_eq!(harness.head(), head);
        assert!(harness.ran("reset --merge"));
        assert_eq!(harness.status(), " M b.txt");
        assert_eq!(harness.read("b.txt"), "local\n");
        assert!(!harness.repo.path().join("c.txt").exists());
    }

    // A fast-forward accepts what is staged in other files, and `post-merge` runs with everything done.
    #[test]
    fn cancelling_with_something_in_the_stage_and_the_index_changed_touches_nothing_and_warns() {
        let harness = Harness::new();
        harness.incoming("c.txt", "theirs\n");
        harness.write("mine.txt", "staged\n");
        harness.git(&["add", "mine.txt"]);
        let scratch = TempDir::new().unwrap();
        let mark = scratch.path().join("mark");
        harness.slow_script("hooks/post-merge", &mark);

        let result = harness.pull_cancelled_at(&mark, &[]);

        assert!(matches!(result.outcome, Outcome::Error(_)), "{result:?}");
        assert_eq!(result.title.as_deref(), Some(PULL_CANCELLED));
        assert!(!harness.ran("reset --merge"));
        assert_eq!(harness.status(), "A  mine.txt");
    }

    #[test]
    fn cancelling_in_the_fetch_with_something_in_the_stage_touches_nothing_and_does_not_warn() {
        let harness = Harness::new();
        harness.incoming("c.txt", "theirs\n");
        harness.write("mine.txt", "staged\n");
        harness.git(&["add", "mine.txt"]);
        let scratch = TempDir::new().unwrap();
        let mark = scratch.path().join("mark");
        let slow = harness.slow_script("slow-upload-pack", &mark);
        harness.git(&["config", "remote.origin.uploadpack", slow.to_str().unwrap()]);

        let result = harness.pull_cancelled_at(&mark, &[]);

        assert_eq!(result.outcome, Outcome::Cancelled);
        assert!(!result.in_progress);
        assert!(!harness.ran("reset --merge"));
        assert_eq!(harness.status(), "A  mine.txt");
    }

    #[test]
    fn pull_merge_uses_the_intellij_arguments() {
        let harness = Harness::new();
        harness.incoming("c.txt", "x\n");

        let result = harness.pull("origin", "main", &[]);

        assert_eq!(result.outcome, Outcome::Success);
        assert!(harness.ran("pull --no-stat -v --progress origin main"));
        assert_eq!((result.files, result.commits), (1, 1));
        assert!(result.range.is_some());
        assert_eq!(harness.read("c.txt"), "x\n");
    }

    #[test]
    fn pull_with_options_go_before_v_and_in_the_intellij_order() {
        let harness = Harness::new();
        harness.diverged();

        let result = harness.pull("origin", "main", &[PullOption::NoVerify, PullOption::NoFf]);

        assert_eq!(result.outcome, Outcome::Success);
        assert!(harness.ran("pull --no-stat --no-ff --no-verify -v --progress origin main"));
        let parents = harness.git(&["rev-list", "--parents", "-1", "HEAD"]);
        assert_eq!(parents.split(' ').count(), 3, "a merge has two parents");
    }

    #[test]
    fn pull_without_remote_prefix() {
        let harness = Harness::new();
        harness.incoming("c.txt", "x\n");
        harness
            .colleague
            .git(&["checkout", "-q", "-b", "feature/x"]);
        harness.colleague.commit("f.txt", b"f\n", "from the branch");
        harness
            .colleague
            .git(&["push", "-q", "origin", "feature/x"]);

        harness.pull("origin", "main", &[]);
        let result = harness.pull("origin", "feature/x", &[]);

        assert_eq!(result.outcome, Outcome::Success);
        assert!(harness.ran("origin main"));
        assert!(harness.ran("origin feature/x"));
        assert!(!harness.ran("origin origin/main"));
        assert!(!harness.ran_anything_with("origin/main"));
        assert_eq!(harness.read("f.txt"), "f\n");
    }

    #[test]
    fn pull_rebase_goes_through_update() {
        let harness = Harness::new();
        harness.diverged();

        let result = harness.pull("origin", "main", &[PullOption::Rebase]);

        assert_eq!(result.outcome, Outcome::Success);
        assert!(!harness.ran_anything_with("pull"));
        assert!(harness.ran("rebase origin/main"));
        assert_eq!(result.commits, 1);
        let parents = harness.git(&["rev-list", "--parents", "-1", "HEAD"]);
        assert_eq!(parents.split(' ').count(), 2, "the history stays linear");
        assert_eq!(harness.read("c.txt"), "theirs\n");
        assert_eq!(harness.read("own.txt"), "mine\n");
    }

    #[test]
    fn pull_rebase_ignores_the_other_options() {
        let harness = Harness::new();
        harness.diverged();

        let result = harness.pull("origin", "main", &[PullOption::Squash, PullOption::Rebase]);

        assert_eq!(result.outcome, Outcome::Success);
        assert!(!harness.ran_anything_with("--squash"));
        assert!(!harness.ran_anything_with("pull"));
    }

    #[test]
    fn pull_rebase_with_local_changes_saves_them_like_update() {
        let harness = Harness::new();
        harness.diverged();
        harness.write("b.txt", "local\n");

        let result = harness.pull("origin", "main", &[PullOption::Rebase]);

        assert_eq!(result.outcome, Outcome::Success);
        assert_eq!(harness.read("b.txt"), "local\n");
    }

    #[test]
    fn the_remembered_options_belong_to_each_repository() {
        let (one, other) = (TempDir::new().unwrap(), TempDir::new().unwrap());
        assert_eq!(read_options(one.path()), []);

        write_options(one.path(), &[PullOption::Squash, PullOption::NoVerify]).unwrap();

        assert_eq!(
            read_options(one.path()),
            [PullOption::Squash, PullOption::NoVerify]
        );
        assert_eq!(
            read_options(other.path()),
            [],
            "the other repository's do not carry over"
        );
        write_options(one.path(), &[]).unwrap();
        assert_eq!(read_options(one.path()), []);
    }

    #[test]
    fn remembered_values_that_no_longer_apply_are_discarded_but_not_filtered_as_incompatible() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("pull-options.json"),
            r#"["rebase", "nonsense", "squash", 3, "rebase", "noVerify"]"#,
        )
        .unwrap();

        // Like `GitPullDialog.loadSettings`: what was saved is loaded as it is.
        assert_eq!(
            read_options(dir.path()),
            [PullOption::Rebase, PullOption::Squash, PullOption::NoVerify]
        );
    }

    #[test]
    fn damaged_options_are_none() {
        let dir = TempDir::new().unwrap();
        for text in ["{ broken", "{\"a\": 1}", "\"rebase\"", ""] {
            fs::write(dir.path().join("pull-options.json"), text).unwrap();
            assert_eq!(read_options(dir.path()), [], "{text}");
        }
    }

    #[test]
    fn the_task_kind_is_read_with_rebase_and_write_without_it() {
        assert_eq!(task_kind(&[]), TaskKind::Write);
        assert_eq!(task_kind(&[PullOption::Squash]), TaskKind::Write);
        assert_eq!(
            task_kind(&[PullOption::NoVerify, PullOption::Rebase]),
            TaskKind::Read
        );
    }

    #[test]
    fn pull_does_not_save_changes() {
        let harness = Harness::new();
        harness.incoming("c.txt", "x\n");
        harness.write("b.txt", "local\n");

        let result = harness.pull("origin", "main", &[]);

        assert_eq!(result.outcome, Outcome::Success);
        assert_eq!(result.saved, None);
        assert_eq!(result.restore, None);
        assert_eq!(harness.read("b.txt"), "local\n");
        assert_eq!(harness.git(&["stash", "list"]), "");
        assert!(!harness.ran_anything_with("stash"));
    }

    #[test]
    fn pull_that_would_overwrite_local_changes_is_the_git_error_and_touches_nothing() {
        let harness = Harness::new();
        harness.incoming("b.txt", "from outside\n");
        harness.write("b.txt", "local\n");
        let head = harness.head();

        let result = harness.pull("origin", "main", &[]);

        let Outcome::Error(text) = &result.outcome else {
            panic!("{:?}", result.outcome);
        };
        assert_eq!(result.title.as_deref(), Some("Git Pull Failed"));
        assert!(
            text.starts_with("Your local changes will be overwritten by merge."),
            "{text}"
        );
        assert!(text.ends_with("\nb.txt"), "{text}");
        assert_eq!(harness.read("b.txt"), "local\n");
        assert_eq!(harness.head(), head);
        assert_eq!(harness.git(&["stash", "list"]), "");
        assert_eq!(result.saved, None);
        assert!(!result.in_progress);
    }

    #[test]
    fn pull_with_an_unversioned_file_in_the_way() {
        let harness = Harness::new();
        harness.incoming("c.txt", "from outside\n");
        harness.write("c.txt", "mine\n");

        let result = harness.pull("origin", "main", &[]);

        let Outcome::Error(text) = &result.outcome else {
            panic!("{:?}", result.outcome);
        };
        assert_eq!(
            result.title.as_deref(),
            Some("Untracked Files Prevent Pull")
        );
        assert!(
            text.starts_with("Move or commit them before pull\n"),
            "{text}"
        );
        assert!(text.ends_with("\nc.txt"), "{text}");
        assert_eq!(harness.read("c.txt"), "mine\n");
    }

    #[test]
    fn pull_no_commit_leaves_a_prepared_merge() {
        let harness = Harness::new();
        harness.diverged();
        let head = harness.head();

        let result = harness.pull("origin", "main", &[PullOption::NoCommit]);

        assert_eq!(result.outcome, Outcome::Success);
        assert!(harness.in_git_dir("MERGE_HEAD"));
        assert_eq!(harness.head(), head);
        assert_eq!(harness.read("c.txt"), "theirs\n");
        assert!(!result.in_progress, "there are no conflicts to resolve");
        assert_eq!((result.files, result.commits), (1, 0));
    }

    // The state sees the merge that `--no-commit` leaves uncommitted, and the widget and the menu
    // offer to abort it.
    #[test]
    fn merge_no_commit_stays_as_a_merge_in_progress() {
        let harness = Harness::new();
        harness.diverged();
        let head = harness.head();
        harness.pull("origin", "main", &[PullOption::NoCommit]);

        let state = crate::operation::state(harness.repo.path()).unwrap();

        assert_eq!(state.operation, Some(crate::operation::Operation::Merging));
        assert_eq!(state.label, "Merging main");
        assert_eq!(harness.abort(), Ok(Aborted::Merge));
        assert!(!harness.in_git_dir("MERGE_HEAD"));
        assert_eq!(harness.head(), head);
        assert!(!harness.repo.path().join("c.txt").exists());
    }

    #[test]
    fn pull_with_conflict_leaves_the_merge_half_done_and_says_there_is_something_to_abort() {
        let harness = Harness::new();
        harness.diverged_on_a_txt();

        let result = harness.pull("origin", "main", &[]);

        assert_eq!(result.outcome, Outcome::Conflicts);
        assert!(result.in_progress);
        assert!(harness.in_git_dir("MERGE_HEAD"));
        assert!(harness.read("a.txt").contains("<<<<<<<"));
        assert_eq!(result.saved, None);
    }

    #[test]
    fn divergent_pull_without_configuring_how_to_reconcile_gives_the_git_error() {
        let harness = Harness::new();
        harness.git(&["config", "--unset", "pull.rebase"]);
        harness.diverged();

        let result = harness.pull("origin", "main", &[]);

        let Outcome::Error(text) = &result.outcome else {
            panic!("{:?}", result.outcome);
        };
        assert_eq!(result.title.as_deref(), Some("Git Pull failed"));
        assert!(text.contains("reconcile divergent branches"), "{text}");
        assert!(!result.in_progress);
    }

    #[test]
    fn pull_squash_counts_the_files_it_brings_even_if_it_does_not_move_head() {
        let harness = Harness::new();
        harness.diverged();
        let head = harness.head();

        let result = harness.pull("origin", "main", &[PullOption::Squash]);

        assert_eq!(result.outcome, Outcome::Success);
        assert!(harness.ran("pull --no-stat --squash -v --progress origin main"));
        assert_eq!(harness.head(), head);
        assert!(!harness.in_git_dir("MERGE_HEAD"));
        assert_eq!(harness.read("c.txt"), "theirs\n");
        assert_eq!((result.files, result.commits), (1, 0));
        assert!(!result.in_progress);
    }

    #[test]
    fn pull_squash_without_news_counts_nothing() {
        let harness = Harness::new();

        let result = harness.pull("origin", "main", &[PullOption::Squash]);

        assert_eq!(result.outcome, Outcome::Success);
        assert_eq!((result.files, result.commits), (0, 0));
    }

    #[test]
    fn pull_squash_with_conflict_can_be_aborted_with_a_copy() {
        let harness = Harness::new();
        harness.diverged_on_a_txt();
        let head = harness.head();

        let result = harness.pull("origin", "main", &[PullOption::Squash]);

        assert_eq!(result.outcome, Outcome::Conflicts);
        assert!(result.in_progress);
        assert!(!harness.in_git_dir("MERGE_HEAD"));
        assert!(harness.in_git_dir("SQUASH_MSG"));

        let aborted = harness.abort();

        assert_eq!(aborted, Ok(Aborted::Squash));
        assert!(harness.ran("reset --merge"));
        assert_eq!(harness.head(), head);
        assert_eq!(harness.read("a.txt"), "mine\n2\n3\n");
        assert_eq!(harness.git(&["ls-files", "--unmerged"]), "");
        assert!(!harness.in_git_dir("SQUASH_MSG"));
        assert_eq!(snapshots::list(harness.data.path()).len(), 1);
        assert_eq!(harness.abort(), Ok(Aborted::Nothing));
    }

    #[test]
    fn pull_over_an_unfinished_merge_does_not_take_it_for_a_new_conflict() {
        let harness = Harness::new();
        harness.diverged_on_a_txt();
        harness.pull("origin", "main", &[]);
        harness.incoming("d.txt", "another\n");

        let result = harness.pull("origin", "main", &[]);

        let Outcome::Error(text) = &result.outcome else {
            panic!("{:?}", result.outcome);
        };
        assert_eq!(result.title.as_deref(), Some("Git Pull failed"));
        assert!(text.contains("merge"), "{text}");
    }

    #[test]
    fn pull_ff_only_that_cannot_gives_the_git_error() {
        let harness = Harness::new();
        harness.diverged();
        let head = harness.head();

        let result = harness.pull("origin", "main", &[PullOption::FfOnly]);

        let Outcome::Error(text) = &result.outcome else {
            panic!("{:?}", result.outcome);
        };
        assert_eq!(result.title.as_deref(), Some("Git Pull failed"));
        assert!(text.contains("fast-forward"), "{text}");
        assert_eq!(harness.head(), head);
    }

    #[test]
    fn pull_without_news_counts_nothing() {
        let harness = Harness::new();

        let result = harness.pull("origin", "main", &[]);

        assert_eq!(result.outcome, Outcome::Success);
        assert_eq!((result.files, result.commits, result.range), (0, 0, None));
    }

    #[test]
    fn successful_pull_marks_the_remote_as_authenticated() {
        let harness = Harness::new();
        assert!(!harness.sync.is_authenticated(harness.repo.path(), "origin"));

        harness.pull("origin", "main", &[]);

        assert!(harness.sync.is_authenticated(harness.repo.path(), "origin"));
    }

    #[test]
    fn failed_pull_does_not_mark_the_remote() {
        let harness = Harness::new();

        let result = harness.pull("origin", "does-not-exist", &[]);

        assert!(matches!(result.outcome, Outcome::Error(_)));
        assert_eq!(result.title.as_deref(), Some("Git Pull failed"));
        assert!(!harness.sync.is_authenticated(harness.repo.path(), "origin"));
    }

    #[test]
    fn a_remote_or_branch_from_the_interface_does_not_reach_git_as_an_option_or_a_url() {
        let harness = Harness::new();
        let url = harness.origin.path().to_str().unwrap().to_string();
        let cases = [
            ("--upload-pack=touch /tmp/not-created", "main"),
            ("ext::sh -c touch% /tmp/not-created", "main"),
            (url.as_str(), "main"),
            ("", "main"),
            ("another", "main"),
            ("origin", "--rebase"),
            ("origin", "-x"),
            ("origin", "+main"),
            ("origin", ""),
            ("origin", "a:b"),
            ("origin", "a b"),
            ("origin", "HEAD"),
        ];
        for (remote, branch) in cases {
            for options in [vec![], vec![PullOption::Rebase]] {
                let result = harness.pull(remote, branch, &options);

                assert!(
                    matches!(result.outcome, Outcome::Error(_)),
                    "{remote:?} {branch:?}: {:?}",
                    result.outcome
                );
            }
        }
        assert!(!harness.ran_anything_with("pull"));
        assert!(!harness.ran_anything_with("fetch"));
        assert!(!harness.ran_anything_with("rebase"));
    }

    #[test]
    fn info_brings_remotes_branches_and_the_tracked_one() {
        let harness = Harness::new();
        harness.git(&["remote", "add", "unused", "/does/not/matter"]);
        harness.git(&["branch", "-q", "loose"]);

        let info = info(harness.repo.path()).unwrap();

        assert_eq!(info.branch.as_deref(), Some("main"));
        assert_eq!(info.remotes, ["origin", "unused"]);
        assert_eq!(info.remote_branches, ["origin/main"]);
        assert_eq!(
            info.tracked,
            Some(("origin".to_string(), "main".to_string()))
        );
    }

    #[test]
    fn info_without_tracked_branch_or_current_branch() {
        let harness = Harness::new();
        harness.git(&["checkout", "-q", "-b", "new"]);
        assert_eq!(info(harness.repo.path()).unwrap().tracked, None);

        harness.git(&["checkout", "-q", "--detach"]);
        let detached = info(harness.repo.path()).unwrap();
        assert_eq!(detached.branch, None);
        assert_eq!(detached.tracked, None);
    }

    #[test]
    fn pull_into_does_fetch_and_merge_of_the_remote() {
        let harness = Harness::new();
        let theirs = harness.incoming_on_other();
        harness.repo.commit("own.txt", b"mine\n", "own");

        let result = harness.pull_into("origin/other", UpdateMethod::Merge);

        assert_eq!(result.outcome, Outcome::Success, "{result:?}");
        assert!(harness.ran("fetch origin --recurse-submodules=no --progress --prune"));
        assert!(harness.ran("merge origin/other --no-stat -v"));
        assert_eq!(harness.git(&["branch", "--show-current"]), "main\n");
        let parents = harness.git(&["rev-list", "--parents", "-1", "HEAD"]);
        assert!(parents.contains(&theirs), "{parents}");
        assert_eq!(result.commits, 1);
    }

    #[test]
    fn pull_into_rebase_goes_through_update_with_rebase() {
        let harness = Harness::new();
        let theirs = harness.incoming_on_other();
        harness.repo.commit("own.txt", b"mine\n", "own");
        harness.write("a.txt", "1\n2\nlocal\n");

        let result = harness.pull_into("origin/other", UpdateMethod::Rebase);

        assert_eq!(result.outcome, Outcome::Success, "{result:?}");
        assert!(harness.ran("rebase origin/other"));
        assert_eq!(harness.git(&["rev-parse", "HEAD~1"]).trim(), theirs);
        assert_eq!(harness.git(&["log", "-1", "--format=%s"]), "own\n");
        assert_eq!(result.restore, Some(preserve::RestoreOutcome::Restored));
        assert_eq!(harness.read("a.txt"), "1\n2\nlocal\n");
    }

    #[test]
    fn pull_into_of_something_that_is_not_a_remote_branch_launches_nothing() {
        let harness = Harness::new();

        let bad = harness.pull_into("-x", UpdateMethod::Merge);
        let local = harness.pull_into("main", UpdateMethod::Merge);

        assert!(matches!(bad.outcome, Outcome::Error(_)), "{bad:?}");
        assert!(matches!(local.outcome, Outcome::Error(_)), "{local:?}");
        assert!(!harness.ran_anything_with("merge"));
        assert!(!harness.ran_anything_with("fetch"));
    }
}
