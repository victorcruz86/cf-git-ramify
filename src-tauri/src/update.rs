// `Update Project`, with the steps of `GitUpdateProcess`. It saves the changes while Devin can keep
// writing: whatever happens, what was saved is still in the stash or has gone back to disk.
use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::branch_info::BranchSync;
use crate::branches::{self, Repo};
use crate::commit;
use crate::editor::REBASE_CONFIG;
use crate::fetch;
use crate::git::{self, GitError};
use crate::operation;
use crate::preserve::{self, RestoreOutcome, Saved};
use crate::rebase;
use crate::settings::{CleanTree, UpdateMethod};
use crate::snapshots;
use crate::task::TaskContext;

const HEAD: &str = "HEAD";
const HEADS: &str = "refs/heads/";
const LOCAL_REMOTE: &str = ".";
const GONE: &str = "[gone]";
const UPDATE: &str = "Update";
const MERGE_ABORT: &str = "Merge --abort";
const SQUASH_MSG: &str = "SQUASH_MSG";
const FINISH_FIRST: &str = "Finish or abort first";
const BRANCH_CHANGED: &str = "The current branch changed during the update.";
const CANNOT_UPDATE: &str = "Cannot update";
pub(crate) const UPDATE_FAILED: &str = "Update failed";
const NO_CURRENT_BRANCH: &str = "Cannot Update: No Current Branch";
const UPSTREAM_FAILED: &str = "Cannot set an upstream branch";
const SAVE_FAILED: &str = "Could not save uncommitted changes";
const MERGE_ABORT_FAILED: &str = "Failed to abort the merge operation";
const REBASE_ABORT_FAILED: &str = "Abort rebase failed";
const UNFINISHED_REBASE: &str =
    "You have unfinished rebase process. These conflicts must be resolved before update.";
const UNFINISHED_MERGE: &str =
    "You have unfinished merge. These conflicts must be resolved before update.";
const UNMERGED_FILES: &str =
    "Unmerged files detected. These conflicts must be resolved before update.";
const DETACHED_HEAD: &str = "You are in the 'detached HEAD' state, which means that you're not on any branch\nCheckout a branch to be able to update the project.";
pub(crate) const LOCAL_CHANGES_OVERWRITTEN: &str =
    "Your local changes will be overwritten by merge.\nCommit, stash, or revert them to proceed.";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "reason", rename_all = "camelCase")]
pub enum Outcome {
    Success,
    NothingToUpdate,
    Conflicts,
    NotReady(String),
    NoTrackedBranch { branch: String },
    Error(String),
    Cancelled,
    // It stops before saving anything: the answer to `Rebasing Merge Commits` is needed, for `branch`.
    RebasingMergeCommits { branch: String },
    // Decision 1: it also stops before saving, while IntelliJ asks with the changes already in the
    // stash. `protected` is the protected remote branch that has them, empty if it could not be read.
    RebasingPublishedCommits { branch: String, protected: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateResult {
    pub outcome: Outcome,
    // The notification's for `NotReady` and `Error`, which IntelliJ picks according to what failed.
    pub title: Option<String>,
    pub files: u32,
    pub commits: u32,
    pub range: Option<(String, String)>,
    // With a path filter, how many of the received commits touch them. `None`: no filter.
    pub matching: Option<u32>,
    pub saved: Option<Saved>,
    // `None`: it was not attempted, because nothing was saved or because git was left half-done.
    pub restore: Option<RestoreOutcome>,
    // Git was left with an unfinished merge or rebase, and there is something to abort.
    pub in_progress: bool,
}

impl UpdateResult {
    // `stillSaved` of `src/update/flow.ts`: what is only recovered with `Restore Changes`.
    pub fn still_saved(&self) -> bool {
        let back = matches!(
            self.restore,
            Some(
                RestoreOutcome::Restored
                    | RestoreOutcome::RestoredWithoutIndex
                    | RestoreOutcome::Gone
            )
        );
        self.saved.is_some() && !back
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UpdateOptions {
    // The branch chosen in `FixTrackedBranchDialog`, instead of the tracked one.
    pub target: Option<String>,
    pub set_upstream: bool,
    // To repeat after `Rebasing Merge Commits`: the fetch is already done.
    pub fetched: bool,
    // The branch for which `Rebase` was answered to `Rebasing Merge Commits`, as `published_confirmed`.
    pub rebase_merges: Option<String>,
    // The branch for which `Rebase Anyway` was answered to `Rebasing Published Commit`, and the
    // one `Pull…` pulls into. Another current branch is checked, and asked about, again.
    pub published_confirmed: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Aborted {
    Merge,
    Rebase,
    CherryPick,
    Revert,
    // A `pull --squash` with conflicts: there is no `MERGE_HEAD`, only unresolved files.
    Squash,
    Nothing,
}

struct Upstream {
    short: String,
    remote: String,
    remote_ref: String,
    gone: bool,
}

pub(crate) struct Tracked {
    pub(crate) branch: String,
    // `None`: the branch is not on a remote and there is nothing to fetch.
    pub(crate) remote: Option<String>,
    pub(crate) target: String,
}

// Where everything stood before updating.
pub(crate) struct Before {
    pub(crate) base: Option<String>,
    pub(crate) head: String,
}

// How git ended. What comes next does not depend on why.
enum Step {
    Updated,
    Stopped,
    Cancelled,
    Failed { title: String, text: String },
}

// `Ok(Err(..))`: it cannot be updated, and this is the result.
type Checked = Result<Result<Tracked, UpdateResult>, GitError>;

impl From<Outcome> for UpdateResult {
    fn from(outcome: Outcome) -> Self {
        Self {
            outcome,
            title: None,
            files: 0,
            commits: 0,
            range: None,
            matching: None,
            saved: None,
            restore: None,
            in_progress: false,
        }
    }
}

fn not_ready(title: &str, text: impl Into<String>) -> UpdateResult {
    UpdateResult {
        title: Some(title.to_string()),
        ..Outcome::NotReady(text.into()).into()
    }
}

pub(crate) fn failed(title: &str, text: impl Into<String>) -> UpdateResult {
    UpdateResult {
        title: Some(title.to_string()),
        ..Outcome::Error(text.into()).into()
    }
}

// `save.notification.failed.stash.text` and `.shelf.text` (G:374, G:373).
fn save_failed(saver: CleanTree, reason: &str) -> UpdateResult {
    let place = match saver {
        CleanTree::Stash => "stash",
        CleanTree::Shelve => "shelf",
    };
    let text = format!(
        "Tried to save uncommitted changes in {place} before Update, but failed with an error.\n{reason}"
    );
    failed(SAVE_FAILED, text)
}

fn upstream_of(path: &Path, branch: &str) -> Result<Option<Upstream>, GitError> {
    let full = format!("{HEADS}{branch}");
    let format = "--format=%(upstream:short)%09%(upstream:remotename)%09%(upstream:remoteref)%09%(upstream:track)";
    let output = git::run(path, &["for-each-ref", format, &full])?;
    let text = String::from_utf8_lossy(&output);
    let mut fields = text.trim_end_matches('\n').split('\t');
    let mut field = || fields.next().unwrap_or_default().to_string();
    let (short, remote, remote_ref, track) = (field(), field(), field(), field());
    Ok((!short.is_empty()).then(|| Upstream {
        short,
        remote,
        remote_ref,
        gone: track == GONE,
    }))
}

// The remote branch of `local`: the remote and its name there.
pub fn tracked_branch(path: &Path, local: &str) -> Result<Option<(String, String)>, GitError> {
    Ok(upstream_of(path, local)?.and_then(|upstream| {
        let name = upstream.remote_ref.strip_prefix(HEADS)?.to_string();
        Some((upstream.remote, name))
    }))
}

// `checkRebaseInProgress` and company, with `checkTrackedBranchesConfiguration`. Without the conflict
// resolver, which arrives in phase 3, what is half-done can only be reported.
fn checked(repo: &Repo, options: &UpdateOptions) -> Checked {
    if commit::rebase_in_progress(repo.path) {
        return Ok(Err(not_ready(CANNOT_UPDATE, UNFINISHED_REBASE)));
    }
    if commit::merge_in_progress(repo.path) {
        return Ok(Err(not_ready(CANNOT_UPDATE, UNFINISHED_MERGE)));
    }
    if preserve::has_unmerged(repo.ctx)? {
        return Ok(Err(not_ready(CANNOT_UPDATE, UNMERGED_FILES)));
    }
    let Some(branch) = branches::current_name(repo.path)? else {
        return Ok(Err(not_ready(NO_CURRENT_BRANCH, DETACHED_HEAD)));
    };
    if let Some(target) = &options.target {
        branches::check_argument(target, "The branch")?;
        let remote = branches::split_remote(repo.path, target)?.map(|(remote, _)| remote);
        let target = target.clone();
        return Ok(Ok(Tracked {
            branch,
            remote,
            target,
        }));
    }
    match upstream_of(repo.path, &branch)?.filter(|upstream| !upstream.gone) {
        Some(upstream) => {
            branches::check_argument(&upstream.short, "The branch")?;
            Ok(Ok(Tracked {
                branch,
                remote: Some(upstream.remote).filter(|remote| remote != LOCAL_REMOTE),
                target: upstream.short,
            }))
        }
        None => Ok(Err(UpdateResult {
            title: Some(CANNOT_UPDATE.to_string()),
            ..Outcome::NoTrackedBranch { branch }.into()
        })),
    }
}

fn set_upstream(repo: &Repo, options: &UpdateOptions, branch: &str) -> Option<UpdateResult> {
    let (Some(target), true) = (&options.target, options.set_upstream) else {
        return None;
    };
    let args = ["branch", "--set-upstream-to", target, branch];
    match repo.ctx.git_loud(&args) {
        Ok(_) => None,
        Err(GitError::Cancelled) => Some(Outcome::Cancelled.into()),
        Err(error) => Some(failed(UPSTREAM_FAILED, fetch::error_text(error))),
    }
}

pub(crate) fn merge_base(ctx: &TaskContext, tracked: &Tracked) -> Option<String> {
    let args = ["merge-base", &tracked.branch, &tracked.target];
    let output = ctx.git_to_the_end(&args).ok()?;
    Some(output.stdout.trim().to_string()).filter(|hash| !hash.is_empty())
}

fn lines_of(ctx: &TaskContext, args: &[&str]) -> Result<Vec<String>, GitError> {
    let output = ctx.git(args)?;
    Ok(output.stdout.lines().map(str::to_string).collect())
}

fn locally_changed(ctx: &TaskContext) -> Result<HashSet<String>, GitError> {
    let changed = lines_of(ctx, &["diff", "--name-only", "--no-renames", HEAD])?;
    Ok(changed.into_iter().collect())
}

// `GitMergeUpdater.isSaveNeeded`: a merge coexists with changes in other files, and what is not
// saved runs no risk. If it cannot be known, it is saved.
fn merge_touches_changes(
    ctx: &TaskContext,
    tracked: &Tracked,
    changed: &HashSet<String>,
) -> Result<bool, GitError> {
    let check = || {
        if !lines_of(ctx, &["diff", "--name-only", "--no-renames", "--cached"])?.is_empty() {
            return Ok(true);
        }
        let range = format!("{}..{}", tracked.branch, tracked.target);
        let incoming = lines_of(ctx, &["diff", "--name-only", "--pretty=format:", &range])?;
        Ok(incoming.iter().any(|path| changed.contains(path)))
    };
    match check() {
        Err(GitError::Cancelled) => Err(GitError::Cancelled),
        result => Ok(result.unwrap_or(true)),
    }
}

// `GitRebaseOverMergeProblem.hasProblem`: `-c` only lists what differs from all the parents, which
// is what had to be resolved by hand and would have to be resolved again. If it fails, it does not ask.
fn rebases_resolved_merges(ctx: &TaskContext, tracked: &Tracked) -> Result<bool, GitError> {
    has_resolved_merges(ctx, &tracked.target, &tracked.branch)
}

// `GitRebaseUpdater.checkForRebasingPublishedCommits`: the protected remote branch that already has
// some commit to rebase. If it cannot be read, it is asked, as in IntelliJ, with no branch.
fn rebases_published(repo: &Repo, tracked: &Tracked) -> Result<Option<String>, GitError> {
    match rebase::published(repo, Some(&tracked.target), &tracked.branch) {
        Err(GitError::Cancelled) => Err(GitError::Cancelled),
        Err(_) => Ok(Some(String::new())),
        Ok(found) => Ok(found),
    }
}

// `GitRebaseOverMergeProblem.hasProblem`.
pub(crate) fn has_resolved_merges(
    ctx: &TaskContext,
    base: &str,
    head: &str,
) -> Result<bool, GitError> {
    let range = format!("{base}..{head}");
    let args = ["log", "--merges", "-c", "--name-only", "--format=", &range];
    match lines_of(ctx, &args) {
        Err(GitError::Cancelled) => Err(GitError::Cancelled),
        result => Ok(result.is_ok_and(|paths| !paths.is_empty())),
    }
}

pub(crate) fn with_files(text: &str, files: &[String]) -> String {
    let mut lines = vec![text.to_string()];
    lines.extend_from_slice(files);
    lines.join("\n")
}

// `GitMergeUpdater.handleMergeFailure` and `GitRebaser.handleRebaseFailure`, without the dialogs: the
// files go in the text.
fn explain(operation: &str, error: GitError) -> Step {
    let stderr = branches::stderr_of(&error);
    let (title, text) = if stderr.contains(preserve::UNTRACKED_FILES) {
        let hint = format!("Move or commit them before {}", operation.to_lowercase());
        (
            format!("Untracked Files Prevent {operation}"),
            with_files(
                &hint,
                &preserve::listed_files(stderr, preserve::UNTRACKED_FILES),
            ),
        )
    } else if stderr.contains(branches::LOCAL_CHANGES) {
        (
            format!("Local Changes Prevent from {operation}"),
            with_files(
                LOCAL_CHANGES_OVERWRITTEN,
                &branches::overwritten_files(stderr),
            ),
        )
    } else {
        (format!("{operation} Error"), fetch::error_text(error))
    };
    Step::Failed { title, text }
}

// If it cannot be known it counts as half-done: nothing is lost in the stash.
pub fn stopped_midway(path: &Path) -> bool {
    commit::rebase_in_progress(path)
        || commit::merge_in_progress(path)
        || git::run(path, &["ls-files", "--unmerged"]).map_or(true, |paths| !paths.is_empty())
}

// Decision 6: `reset --merge` and `rebase --abort` take a copy, because they go over whatever is in
// those files. They are not cancelled: a half-done abort is worse than the half-done merge.
pub fn abort(repo: &Repo) -> Result<Aborted, GitError> {
    // A rebase with a merge inside (`--rebase-merges`) also has `MERGE_HEAD`, and what is meant to be
    // undone is the rebase.
    if commit::rebase_in_progress(repo.path) {
        rebase::abort(repo)?;
        return Ok(Aborted::Rebase);
    }
    if let Some(kind) = operation::current(repo.path)? {
        return operation::undo(repo, kind);
    }
    // A `pull --squash` with conflicts has no `MERGE_HEAD`, but it leaves `SQUASH_MSG` and unresolved
    // paths. Unresolved paths without that (a `stash pop` with conflicts) do not belong to an
    // operation to abort, and `reset --merge` would leave them as in HEAD.
    if !squash_pending(repo)? {
        return Ok(Aborted::Nothing);
    }
    branches::copy_before(repo, MERGE_ABORT, |path| snapshots::at_risk(path, HEAD))?;
    repo.ctx.git_to_the_end(&["reset", "--merge"])?;
    Ok(Aborted::Squash)
}

fn squash_pending(repo: &Repo) -> Result<bool, GitError> {
    let message = git::git_path(repo.path, SQUASH_MSG)?;
    Ok(message.exists() && preserve::has_unmerged(repo.ctx)?)
}

// A `git merge` killed before writing `MERGE_HEAD` (in its hook, or while writing the tree) leaves
// what was merged without any sign, and `abort` would see nothing to undo. Update always starts with
// the index equal to HEAD, so `reset --merge` only takes away what the merge did.
pub(crate) fn undo_merge(repo: &Repo) -> Result<(), GitError> {
    branches::copy_before(repo, MERGE_ABORT, |path| snapshots::at_risk(path, HEAD))?;
    repo.ctx.git_to_the_end(&["reset", "--merge"])?;
    Ok(())
}

fn after_cancel(repo: &Repo, method: UpdateMethod) -> Step {
    let undone = match method {
        UpdateMethod::Merge => undo_merge(repo),
        UpdateMethod::Rebase => abort(repo).map(|_| ()),
    };
    match undone {
        Ok(()) => Step::Cancelled,
        Err(error) => Step::Failed {
            title: match method {
                UpdateMethod::Merge => MERGE_ABORT_FAILED,
                UpdateMethod::Rebase => REBASE_ABORT_FAILED,
            }
            .to_string(),
            text: fetch::error_text(error),
        },
    }
}

fn run_updater(repo: &Repo, method: UpdateMethod, target: &str) -> Step {
    let (operation, result) = match method {
        UpdateMethod::Merge => (
            "Merge",
            repo.ctx.git_loud(&["merge", target, "--no-stat", "-v"]),
        ),
        UpdateMethod::Rebase => {
            let args: Vec<&str> = REBASE_CONFIG
                .iter()
                .copied()
                .chain(["rebase", target])
                .collect();
            (
                "Rebase",
                repo.ctx.git_editing(&args, repo.ctx.asking_editor()),
            )
        }
    };
    match result {
        Ok(_) => Step::Updated,
        Err(GitError::Cancelled) => after_cancel(repo, method),
        Err(_) if stopped_midway(repo.path) => Step::Stopped,
        Err(error) => explain(operation, error),
    }
}

// `Restore Changes`. The same as in `finish`: with an operation half-done `pop` can succeed and
// drop the entry, and a `rebase --abort` afterwards would take away the only thing left of those changes.
pub fn restore_requested(repo: &Repo, saved: &Saved) -> RestoreOutcome {
    if stopped_midway(repo.path) {
        return RestoreOutcome::Failed(FINISH_FIRST.to_string());
    }
    preserve::restore_tracked(repo.ctx, repo.data, saved)
}

// `GitUpdater.markStart`: from the HEAD before updating, so as not to count our own. It is a figure
// for the user: if it fails, zero.
pub(crate) fn updated_files(ctx: &TaskContext, head_before: &str) -> u32 {
    let revisions = format!("{head_before}..{HEAD}");
    let diff = [
        "diff",
        "--name-only",
        "--diff-filter=ADMRUX",
        "--no-renames",
        &revisions,
    ];
    let unmerged = ["ls-files", "--unmerged"];
    let mut files = HashSet::new();
    if let Ok(output) = ctx.git_to_the_end(&diff) {
        files.extend(output.stdout.lines().map(str::to_string));
    }
    if let Ok(output) = ctx.git_to_the_end(&unmerged) {
        let paths = output
            .stdout
            .lines()
            .filter_map(|line| line.split_once('\t'));
        files.extend(paths.map(|(_, path)| path.to_string()));
    }
    files.len() as u32
}

// `GitUpdatedRanges`: the received commits are those between the two bases, not the target of a
// rebase, which includes our own rewritten ones.
pub(crate) fn with_counts(
    ctx: &TaskContext,
    result: UpdateResult,
    tracked: &Tracked,
    before: Before,
) -> UpdateResult {
    let Before { base, head } = before;
    let received = base.zip(merge_base(ctx, tracked)).and_then(|(start, end)| {
        let range = format!("{start}..{end}");
        let output = ctx.git_to_the_end(&["rev-list", "--count", &range]).ok()?;
        let commits = output
            .stdout
            .trim()
            .parse()
            .ok()
            .filter(|count| *count > 0)?;
        Some((commits, start, end))
    });
    let Some((commits, start, end)) = received else {
        return result;
    };
    UpdateResult {
        files: updated_files(ctx, &head),
        commits,
        range: Some((start, end)),
        ..result
    }
}

// Decision 10: only after a successful update, as `GitUpdateProcess` decides for
// `GitPreservingProcess`. After an error or a cancel the changes stay saved, and the notification
// offers `View saved changes…`.
fn finish(repo: &Repo, step: Step, saved: Option<Saved>) -> UpdateResult {
    let in_progress = stopped_midway(repo.path);
    let restore = saved
        .as_ref()
        .filter(|_| matches!(step, Step::Updated) && !in_progress)
        .map(|saved| preserve::restore_tracked(repo.ctx, repo.data, saved));
    let result: UpdateResult = match step {
        Step::Updated => Outcome::Success.into(),
        Step::Stopped => Outcome::Conflicts.into(),
        Step::Cancelled => Outcome::Cancelled.into(),
        Step::Failed { title, text } => failed(&title, text),
    };
    UpdateResult {
        saved,
        restore,
        in_progress,
        ..result
    }
}

// Everything from seeing what has to be done to having done it: what was checked before the fetch
// no longer holds. Once saved, no `?` leaves: what was saved always reaches `finish`.
fn with_lock(
    repo: &Repo,
    method: UpdateMethod,
    options: &UpdateOptions,
    branch: &str,
) -> Result<UpdateResult, GitError> {
    let ctx = repo.ctx;
    let tracked = match checked(repo, options)? {
        Ok(tracked) => tracked,
        Err(result) => return Ok(result),
    };
    // What was chosen or fetched was for the branch from before the fetch.
    if tracked.branch != branch {
        return Ok(not_ready(CANNOT_UPDATE, BRANCH_CHANGED));
    }
    let incoming = format!("{HEAD}..{}", tracked.target);
    if ctx
        .git(&["rev-list", "-1", &incoming])?
        .stdout
        .trim()
        .is_empty()
    {
        return Ok(Outcome::NothingToUpdate.into());
    }
    let before = Before {
        base: merge_base(ctx, &tracked),
        head: ctx.git(&["rev-parse", HEAD])?.stdout.trim().to_string(),
    };
    let changed = locally_changed(ctx)?;
    let rebase = method == UpdateMethod::Rebase;
    // Without changes nothing would be saved, and the shortcut saves nothing.
    if rebase && !changed.is_empty() && ctx.git(&["merge", &tracked.target, "--ff-only"]).is_ok() {
        return Ok(with_counts(ctx, Outcome::Success.into(), &tracked, before));
    }
    ctx.check_cancelled()?;
    let merges_answered = options.rebase_merges.as_deref() == Some(tracked.branch.as_str());
    if rebase && !merges_answered && rebases_resolved_merges(ctx, &tracked)? {
        let branch = tracked.branch;
        return Ok(Outcome::RebasingMergeCommits { branch }.into());
    }
    if rebase && options.published_confirmed.as_deref() != Some(tracked.branch.as_str()) {
        if let Some(protected) = rebases_published(repo, &tracked)? {
            let branch = tracked.branch;
            return Ok(Outcome::RebasingPublishedCommits { branch, protected }.into());
        }
    }
    let save = if rebase {
        !changed.is_empty()
    } else {
        merge_touches_changes(ctx, &tracked, &changed)?
    };
    let saved = if save {
        let message = preserve::dated(UPDATE);
        match preserve::save_local_keeping(ctx, repo.path, repo.data, UPDATE, &message) {
            Ok(saved) => saved,
            Err(failed) if failed.error == GitError::Cancelled => return Err(GitError::Cancelled),
            // What it saved before failing stays saved, and is named (`View saved changes…`).
            Err(failed) => {
                let saver = preserve::saver(repo.data);
                let result = save_failed(saver, &branches::text_of(failed.error));
                return Ok(UpdateResult {
                    saved: failed.saved.map(|saved| *saved),
                    ..result
                });
            }
        }
    } else {
        None
    };
    let step = run_updater(repo, method, &tracked.target);
    Ok(with_counts(
        ctx,
        finish(repo, step, saved),
        &tracked,
        before,
    ))
}

fn prepared(
    repo: &Repo,
    sync: &BranchSync,
    method: UpdateMethod,
    options: &UpdateOptions,
    before_changes: impl FnOnce() -> Result<(), GitError>,
) -> Result<UpdateResult, GitError> {
    let tracked = match checked(repo, options)? {
        Ok(tracked) => tracked,
        Err(result) => return Ok(result),
    };
    if let Some(result) = set_upstream(repo, options, &tracked.branch) {
        return Ok(result);
    }
    if let Some(remote) = tracked.remote.as_ref().filter(|_| !options.fetched) {
        match fetch::fetch_remote(repo.ctx, sync, remote) {
            Ok(_) => {}
            Err(GitError::Cancelled) => return Err(GitError::Cancelled),
            Err(error) => return Ok(not_ready(UPDATE_FAILED, fetch::error_text(error))),
        }
    }
    before_changes()?;
    repo.ctx
        .exclusive(|| with_lock(repo, method, options, &tracked.branch))
        .unwrap_or(Err(GitError::Cancelled))
}

// `before_changes` runs after the fetch and without the lock: what was written in the diff window
// in the meantime must reach disk before saving, and saving it also needs the lock.
pub fn update_project(
    repo: &Repo,
    sync: &BranchSync,
    method: UpdateMethod,
    options: UpdateOptions,
    before_changes: impl FnOnce() -> Result<(), GitError>,
) -> UpdateResult {
    match prepared(repo, sync, method, &options, before_changes) {
        Ok(result) => result,
        Err(GitError::Cancelled) => Outcome::Cancelled.into(),
        Err(error) => failed(UPDATE_FAILED, fetch::error_text(error)),
    }
}

// `updateBranches`: a branch that is not the current one advances without touching the directory.
// With the full name: a leading `+` would force the branch and `x` could also be a tag.
pub fn update_branch(
    ctx: &TaskContext,
    sync: &BranchSync,
    local: &str,
    remote: &str,
    remote_branch: &str,
) -> Result<(), GitError> {
    branches::check_argument(remote, "The remote")?;
    branches::check_branch(ctx, local)?;
    branches::check_branch(ctx, remote_branch)?;
    let refspec = format!("{HEADS}{remote_branch}:{HEADS}{local}");
    let args = [
        "fetch",
        remote,
        &refspec,
        "--recurse-submodules=no",
        "--progress",
        "--prune",
    ];
    ctx.git_loud(&args)?;
    sync.mark_authenticated(ctx.repo(), remote);
    Ok(())
}

// `GitUpdateInfoAsLog`: with a path filter, how many of the received commits touch them. They are
// literal paths: a wildcard or a leading `-` mean nothing to git.
pub fn with_matching(ctx: &TaskContext, result: UpdateResult, paths: &[String]) -> UpdateResult {
    let Some((start, end)) = result.range.as_ref().filter(|_| !paths.is_empty()) else {
        return result;
    };
    let range = format!("{start}..{end}");
    let mut args = vec!["--literal-pathspecs", "rev-list", "--count", &range, "--"];
    args.extend(paths.iter().map(String::as_str));
    let matching = ctx
        .git_to_the_end(&args)
        .ok()
        .and_then(|output| output.stdout.trim().parse().ok());
    UpdateResult { matching, ..result }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_failed_save_is_titled_could_not_save_uncommitted_changes() {
        let result = save_failed(CleanTree::Stash, "boom");
        assert_eq!(
            result.title.as_deref(),
            Some("Could not save uncommitted changes")
        );
        assert!(format!("{result:?}").contains("Tried to save uncommitted changes in stash"));
        let shelf = save_failed(CleanTree::Shelve, "boom");
        assert!(format!("{shelf:?}")
            .contains("Tried to save uncommitted changes in shelf before Update"));
    }

    use super::*;
    use crate::commands::RepoLocks;
    use crate::console::Console;
    use crate::snapshots;
    use crate::task::{Runner, TaskId, TaskKind};
    use crate::test_support::TempRepo;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::mpsc::{self, Receiver};
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};
    use tempfile::TempDir;

    const WAIT: Duration = Duration::from_secs(60);
    const POLL: Duration = Duration::from_millis(20);
    const MERGE: &str = "merge origin/main --no-stat -v";

    struct Harness {
        repo: TempRepo,
        colleague: TempRepo,
        _origin: TempDir,
        data: TempDir,
        scripts: TempDir,
        locks: Arc<RepoLocks>,
        console: Arc<Console>,
        runner: Runner,
        sync: Arc<BranchSync>,
    }

    fn bare() -> TempDir {
        let dir = TempDir::new().unwrap();
        let status = Command::new("git")
            .args(["init", "-q", "--bare", "-b", "main"])
            .arg(dir.path())
            .status()
            .unwrap();
        assert!(status.success());
        dir
    }

    impl Harness {
        fn new() -> Self {
            Self::build(None)
        }

        // `body` is a `case " $* " in` that sees each command before the real git. `{mark}` is a
        // file with which the test knows how far along it is.
        fn wrapping(body: &str) -> Self {
            Self::build(Some(body))
        }

        fn build(body: Option<&str>) -> Self {
            let scripts = TempDir::new().unwrap();
            let binary = body.map(|body| {
                let script = scripts.path().join("git");
                let body = body.replace("{mark}", scripts.path().join("mark").to_str().unwrap());
                let text =
                    format!("#!/bin/sh\ncase \" $* \" in\n{body}\n*) exec git \"$@\" ;;\nesac\n");
                fs::write(&script, text).unwrap();
                fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
                script
            });
            let origin = bare();
            let url = origin.path().to_str().unwrap();
            let repo = TempRepo::new();
            repo.commit("a.txt", b"1\n2\n3\n", "one");
            repo.commit("b.txt", b"b\n", "two");
            repo.git(&["remote", "add", "origin", url]);
            repo.git(&["push", "-q", "-u", "origin", "main"]);
            let colleague = TempRepo::new();
            colleague.git(&["remote", "add", "origin", url]);
            colleague.git(&["fetch", "-q", "origin"]);
            colleague.git(&["checkout", "-q", "-B", "main", "origin/main"]);
            let locks = Arc::<RepoLocks>::default();
            let console = Arc::<Console>::default();
            let runner = Runner::with(
                locks.clone(),
                console.clone(),
                binary,
                Duration::from_secs(5),
            );
            let data = TempDir::new().unwrap();
            // The tests written before the shelf saver speak of the stash.
            crate::settings::save_clean_tree(data.path(), CleanTree::Stash).unwrap();
            Self {
                repo,
                colleague,
                _origin: origin,
                data,
                scripts,
                locks,
                console,
                runner,
                sync: Arc::default(),
            }
        }

        fn mark(&self) -> PathBuf {
            self.scripts.path().join("mark")
        }

        fn wait_for_mark(&self) {
            let deadline = Instant::now() + WAIT;
            while !self.mark().exists() {
                assert!(Instant::now() < deadline, "the command did not arrive");
                thread::sleep(POLL);
            }
        }

        fn start(
            &self,
            method: UpdateMethod,
            options: UpdateOptions,
        ) -> (TaskId, Receiver<UpdateResult>) {
            let (send, receive) = mpsc::channel();
            let path = self.repo.path().to_path_buf();
            let data = self.data.path().to_path_buf();
            let sync = self.sync.clone();
            let id = self.runner.start(
                self.repo.path(),
                TaskKind::Read,
                "Update".to_string(),
                move |ctx| {
                    let repo = Repo {
                        ctx,
                        path: &path,
                        data: &data,
                    };
                    let result = update_project(&repo, &sync, method, options, || Ok(()));
                    send.send(result).unwrap();
                    Ok(())
                },
            );
            (id, receive)
        }

        fn update_with(&self, method: UpdateMethod, options: UpdateOptions) -> UpdateResult {
            self.start(method, options).1.recv_timeout(WAIT).unwrap()
        }

        fn update(&self, method: UpdateMethod) -> UpdateResult {
            self.update_with(method, UpdateOptions::default())
        }

        fn cancelled_at_mark(&self, method: UpdateMethod) -> UpdateResult {
            let (id, result) = self.start(method, UpdateOptions::default());
            self.wait_for_mark();
            self.runner.cancel(id);
            result.recv_timeout(WAIT).unwrap()
        }

        fn in_repo<T: Send + 'static>(&self, work: impl FnOnce(&Repo) -> T + Send + 'static) -> T {
            let path = self.repo.path().to_path_buf();
            let data = self.data.path().to_path_buf();
            crate::test_support::in_task(&self.runner, self.repo.path(), move |ctx| {
                work(&Repo {
                    ctx,
                    path: &path,
                    data: &data,
                })
            })
        }

        // Like the app: the task does not take the lock and `update_project` asks for it when it needs it.
        fn in_repo_unlocked<T: Send + 'static>(
            &self,
            work: impl FnOnce(&Repo) -> T + Send + 'static,
        ) -> T {
            let (send, receive) = mpsc::channel();
            let path = self.repo.path().to_path_buf();
            let data = self.data.path().to_path_buf();
            self.runner.start(
                self.repo.path(),
                TaskKind::Read,
                "test".to_string(),
                move |ctx| {
                    send.send(work(&Repo {
                        ctx,
                        path: &path,
                        data: &data,
                    }))
                    .unwrap();
                    Ok(())
                },
            );
            receive.recv_timeout(WAIT).unwrap()
        }

        // What someone else publishes.
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

        fn stash_count(&self) -> usize {
            self.git(&["stash", "list", "--format=%H"]).lines().count()
        }

        fn ran(&self, command: &str) -> bool {
            self.console
                .lines()
                .iter()
                .any(|line| line.command && line.text.ends_with(command))
        }

        fn saved_anything(&self) -> bool {
            let stashed = self
                .console
                .lines()
                .iter()
                .any(|line| line.command && line.text.contains("stash save"));
            stashed || self.stash_count() > 0 || !snapshots::list(self.data.path()).is_empty()
        }

        // `feature` with a commit of its own that is already in `origin/main`, protected by default,
        // a commit from outside in `origin/feature` and an uncommitted change.
        fn rebasing_a_published_commit(&self) {
            self.git(&["checkout", "-q", "-b", "feature"]);
            self.git(&["push", "-q", "-u", "origin", "feature"]);
            self.repo.commit("p.txt", b"p\n", "published");
            self.git(&["push", "-q", "origin", "feature:main"]);
            self.colleague.git(&["fetch", "-q", "origin"]);
            self.colleague
                .git(&["checkout", "-q", "-B", "feature", "origin/feature"]);
            self.colleague.commit("c.txt", b"c\n", "from outside");
            self.colleague.git(&["push", "-q", "origin", "feature"]);
            self.write("b.txt", "local\n");
        }

        // After `rebasing_a_published_commit`: `other`, tracking `origin/other`, with the same
        // published commit of its own and a commit from outside, and the same uncommitted change.
        fn the_same_on_other(&self) {
            self.git(&["checkout", "-q", "-b", "other"]);
            self.git(&["push", "-q", "origin", "HEAD~1:refs/heads/other"]);
            self.git(&["branch", "-q", "--set-upstream-to=origin/other"]);
            self.colleague.git(&["fetch", "-q", "origin"]);
            self.colleague
                .git(&["checkout", "-q", "-B", "other", "origin/other"]);
            self.colleague.commit("o.txt", b"o\n", "from outside");
            self.colleague.git(&["push", "-q", "origin", "other"]);
        }

        // `main` with a merge whose conflict was resolved by hand, which `git log -c` lists.
        fn resolved_merge_on_main(&self) {
            self.git(&["checkout", "-q", "-b", "side"]);
            self.repo.commit("a.txt", b"side\n2\n3\n", "side");
            self.git(&["checkout", "-q", "main"]);
            self.repo.commit("a.txt", b"mine\n2\n3\n", "own");
            git_may_fail(self.repo.path(), &["merge", "side"]);
            self.repo.commit("a.txt", b"resolved\n2\n3\n", "merge");
        }

        fn in_git_dir(&self, name: &str) -> bool {
            self.repo.path().join(".git").join(name).exists()
        }

        // `main` with a commit of our own that collides with the one from outside, and an uncommitted
        // change in the same file.
        fn diverged_on_a_txt(&self) {
            self.repo.commit("a.txt", b"mine\n2\n3\n", "own");
            self.incoming("a.txt", "theirs\n2\n3\n");
            self.write("a.txt", "mine\n2\nlocal\n");
        }
    }

    fn git_may_fail(path: &Path, args: &[&str]) {
        Command::new("git")
            .current_dir(path)
            .args(args)
            .output()
            .unwrap();
    }

    fn blocked(file: &str) -> RestoreOutcome {
        RestoreOutcome::RestoreBlocked {
            files: vec![file.to_string()],
        }
    }

    fn not_ready(result: &UpdateResult) -> &str {
        match &result.outcome {
            Outcome::NotReady(text) => text,
            outcome => panic!("{outcome:?}"),
        }
    }

    #[test]
    fn without_incoming_it_does_nothing() {
        let harness = Harness::new();
        harness.write("a.txt", "local\n");

        let result = harness.update(UpdateMethod::Merge);

        assert_eq!(result.outcome, Outcome::NothingToUpdate);
        assert_eq!(result.saved, None);
        assert_eq!(result.restore, None);
        assert_eq!(harness.stash_count(), 0);
        assert!(!harness.ran("stash save"));
        assert_eq!(harness.read("a.txt"), "local\n");
    }

    #[test]
    fn fast_merge() {
        let harness = Harness::new();
        let tip = harness.incoming("c.txt", "c\n");

        let result = harness.update(UpdateMethod::Merge);

        assert_eq!(result.outcome, Outcome::Success);
        assert_eq!(harness.head(), tip);
        assert!(harness.ran(MERGE));
        assert!(harness.sync.is_authenticated(harness.repo.path(), "origin"));
    }

    #[test]
    fn merge_with_own_commit() {
        let harness = Harness::new();
        harness.repo.commit("mine.txt", b"m\n", "own");
        harness.incoming("c.txt", "c\n");

        let result = harness.update(UpdateMethod::Merge);

        assert_eq!(result.outcome, Outcome::Success);
        assert_eq!(
            harness
                .git(&["rev-list", "--merges", "HEAD"])
                .lines()
                .count(),
            1
        );
        assert_eq!(harness.read("c.txt"), "c\n");
    }

    #[test]
    fn linear_rebase() {
        let harness = Harness::new();
        harness.repo.commit("mine.txt", b"m\n", "own");
        let tip = harness.incoming("c.txt", "c\n");

        let result = harness.update(UpdateMethod::Rebase);

        assert_eq!(result.outcome, Outcome::Success);
        assert!(harness.ran("rebase origin/main"));
        assert_eq!(harness.git(&["rev-list", "--merges", "HEAD"]), "");
        assert_eq!(harness.git(&["rev-parse", "HEAD~1"]).trim(), tip);
    }

    #[test]
    fn merge_only_saves_if_the_changes_overlap() {
        let aside = Harness::new();
        aside.write("b.txt", "local\n");
        aside.incoming("a.txt", "1\n2\nremote\n");

        let result = aside.update(UpdateMethod::Merge);

        assert_eq!(result.outcome, Outcome::Success);
        assert_eq!(result.saved, None);
        assert!(!aside.ran("stash"));
        assert_eq!(aside.read("b.txt"), "local\n");

        let overlapping = Harness::new();
        overlapping.write("a.txt", "local\n2\n3\n");
        overlapping.incoming("a.txt", "1\n2\nremote\n");

        let result = overlapping.update(UpdateMethod::Merge);

        assert_eq!(result.outcome, Outcome::Success);
        assert!(result.saved.is_some());
        assert_eq!(result.restore, Some(RestoreOutcome::Restored));
        assert_eq!(overlapping.read("a.txt"), "local\n2\nremote\n");
        assert_eq!(overlapping.stash_count(), 0);
    }

    #[test]
    fn merge_saves_what_is_in_the_stage() {
        let harness = Harness::new();
        harness.write("b.txt", "local\n");
        harness.git(&["add", "b.txt"]);
        harness.incoming("a.txt", "1\n2\nremote\n");

        let result = harness.update(UpdateMethod::Merge);

        assert!(result.saved.is_some());
        assert_eq!(harness.read("b.txt"), "local\n");
    }

    #[test]
    fn rebase_saves_any_change() {
        let harness = Harness::new();
        harness.repo.commit("mine.txt", b"m\n", "own");
        harness.write("b.txt", "local\n");
        harness.incoming("a.txt", "1\n2\nremote\n");

        let result = harness.update(UpdateMethod::Rebase);

        assert_eq!(result.outcome, Outcome::Success);
        assert!(result.saved.is_some());
        assert_eq!(result.restore, Some(RestoreOutcome::Restored));
        assert_eq!(harness.read("b.txt"), "local\n");
        assert_eq!(harness.stash_count(), 0);
    }

    #[test]
    fn rebase_with_changes_tries_ff_only_first() {
        let harness = Harness::new();
        harness.write("b.txt", "local\n");
        let tip = harness.incoming("a.txt", "1\n2\nremote\n");

        let result = harness.update(UpdateMethod::Rebase);

        assert_eq!(result.outcome, Outcome::Success);
        assert_eq!(result.saved, None);
        assert!(harness.ran("merge origin/main --ff-only"));
        assert!(!harness.ran("rebase origin/main"));
        assert_eq!(harness.head(), tip);
        assert_eq!(harness.read("b.txt"), "local\n");
    }

    #[test]
    fn rebase_without_changes_does_not_try_ff_only() {
        let harness = Harness::new();
        harness.incoming("a.txt", "1\n2\nremote\n");

        harness.update(UpdateMethod::Rebase);

        assert!(!harness.ran("merge origin/main --ff-only"));
        assert!(harness.ran("rebase origin/main"));
    }

    #[test]
    fn rebase_over_a_merge_with_conflicts_asks_before_saving() {
        let harness = Harness::new();
        harness.resolved_merge_on_main();
        harness.incoming("c.txt", "c\n");
        harness.write("b.txt", "local\n");
        let head = harness.head();

        let result = harness.update(UpdateMethod::Rebase);

        assert_eq!(result.outcome, rebasing_merge_commits("main"));
        assert_eq!(result.saved, None);
        assert_eq!(harness.head(), head);
        assert_eq!(harness.read("b.txt"), "local\n");

        let options = UpdateOptions {
            fetched: true,
            rebase_merges: Some("main".to_string()),
            ..Default::default()
        };
        assert_ne!(
            harness.update_with(UpdateMethod::Rebase, options).outcome,
            rebasing_merge_commits("main")
        );
    }

    fn rebasing_merge_commits(branch: &str) -> Outcome {
        Outcome::RebasingMergeCommits {
            branch: branch.to_string(),
        }
    }

    // The answer to `Rebasing Merge Commits` was for the branch that was asked about: if another one
    // is current now, it is checked, and asked, again, with nothing saved.
    #[test]
    fn a_rebase_merges_answer_for_another_branch_asks_again() {
        let harness = Harness::new();
        harness.resolved_merge_on_main();
        harness.git(&["checkout", "-q", "-b", "other"]);
        harness.git(&["push", "-q", "origin", "origin/main:refs/heads/other"]);
        harness.git(&["branch", "-q", "--set-upstream-to=origin/other"]);
        harness.colleague.git(&["fetch", "-q", "origin"]);
        harness
            .colleague
            .git(&["checkout", "-q", "-B", "other", "origin/other"]);
        harness.colleague.commit("o.txt", b"o\n", "from outside");
        harness.colleague.git(&["push", "-q", "origin", "other"]);
        harness.write("b.txt", "local\n");
        let head = harness.head();
        let answered = |branch: &str| UpdateOptions {
            rebase_merges: Some(branch.to_string()),
            ..Default::default()
        };

        let result = harness.update_with(UpdateMethod::Rebase, answered("main"));

        assert_eq!(result.outcome, rebasing_merge_commits("other"));
        assert_eq!(result.saved, None);
        assert_eq!(result.restore, None);
        assert!(!harness.saved_anything());
        assert!(!harness.ran("rebase origin/other"));
        assert_eq!(harness.head(), head);
        assert_eq!(harness.read("b.txt"), "local\n");

        let result = harness.update_with(UpdateMethod::Rebase, answered("other"));

        assert_ne!(result.outcome, rebasing_merge_commits("other"));
        assert!(harness.ran("rebase origin/other"));
    }

    #[test]
    fn the_rebase_merges_outcome_carries_the_branch() {
        assert_eq!(
            serde_json::to_value(rebasing_merge_commits("feature")).unwrap(),
            serde_json::json!({
                "kind": "rebasingMergeCommits",
                "reason": { "branch": "feature" },
            })
        );
    }

    #[test]
    fn rebase_over_a_clean_merge_does_not_ask() {
        let harness = Harness::new();
        harness.git(&["checkout", "-q", "-b", "side"]);
        harness.repo.commit("side.txt", b"l\n", "side");
        harness.git(&["checkout", "-q", "main"]);
        harness.repo.commit("mine.txt", b"m\n", "own");
        harness.git(&["merge", "-q", "--no-edit", "side"]);
        harness.incoming("c.txt", "c\n");

        let result = harness.update(UpdateMethod::Rebase);

        assert_eq!(result.outcome, Outcome::Success);
    }

    // Decision 1: before saving, unlike IntelliJ, so that `Cancel` leaves the tree as it was.
    #[test]
    fn update_with_rebase_over_a_published_commit_asks_before_saving() {
        let harness = Harness::new();
        harness.rebasing_a_published_commit();
        let head = harness.head();

        let result = harness.update(UpdateMethod::Rebase);

        assert_eq!(
            result.outcome,
            Outcome::RebasingPublishedCommits {
                branch: "feature".to_string(),
                protected: "origin/main".to_string(),
            }
        );
        assert_eq!(result.saved, None);
        assert_eq!(result.restore, None);
        assert!(!harness.saved_anything());
        assert!(!harness.ran("rebase origin/feature"));
        assert_eq!(harness.head(), head);
        assert_eq!(harness.read("b.txt"), "local\n");
    }

    // `GitRebaseProcess.isRebasingPublishedCommit`: commits that cannot be read count as published.
    #[test]
    fn a_published_check_that_fails_asks_without_a_branch() {
        let harness = Harness::new();

        let found = harness.in_repo(|repo| {
            let tracked = Tracked {
                branch: "main".to_string(),
                remote: Some("origin".to_string()),
                target: "origin/missing".to_string(),
            };
            rebases_published(repo, &tracked)
        });

        assert_eq!(found, Ok(Some(String::new())));
    }

    #[test]
    fn the_published_outcome_serialises_as_the_other_outcomes() {
        let outcome = Outcome::RebasingPublishedCommits {
            branch: "feature".to_string(),
            protected: "origin/main".to_string(),
        };

        assert_eq!(
            serde_json::to_value(outcome).unwrap(),
            serde_json::json!({
                "kind": "rebasingPublishedCommits",
                "reason": { "branch": "feature", "protected": "origin/main" },
            })
        );
    }

    #[test]
    fn confirmed_update_rebases_as_before() {
        let harness = Harness::new();
        harness.rebasing_a_published_commit();
        harness.update(UpdateMethod::Rebase);
        let options = UpdateOptions {
            fetched: true,
            published_confirmed: Some("feature".to_string()),
            ..Default::default()
        };

        let result = harness.update_with(UpdateMethod::Rebase, options);

        assert_eq!(result.outcome, Outcome::Success);
        assert!(harness.ran("rebase origin/feature"));
        assert!(result.saved.is_some());
        assert_eq!(result.restore, Some(RestoreOutcome::Restored));
        assert_eq!(harness.stash_count(), 0);
        assert_eq!(harness.read("b.txt"), "local\n");
        assert_eq!(
            harness.git(&["log", "--format=%s", "-3"]),
            "published\nfrom outside\ntwo\n"
        );
    }

    // The answer was for the branch that was asked about: if another one is current now, it is
    // checked, and asked, again.
    #[test]
    fn a_confirmation_for_another_branch_asks_again() {
        let harness = Harness::new();
        harness.rebasing_a_published_commit();
        harness.the_same_on_other();
        let head = harness.head();
        let confirmed = |branch: &str| UpdateOptions {
            published_confirmed: Some(branch.to_string()),
            ..Default::default()
        };

        let result = harness.update_with(UpdateMethod::Rebase, confirmed("feature"));

        assert_eq!(
            result.outcome,
            Outcome::RebasingPublishedCommits {
                branch: "other".to_string(),
                protected: "origin/main".to_string(),
            }
        );
        assert_eq!(result.saved, None);
        assert!(!harness.saved_anything());
        assert_eq!(harness.head(), head);
        assert_eq!(harness.read("b.txt"), "local\n");

        let result = harness.update_with(UpdateMethod::Rebase, confirmed("other"));

        assert_eq!(result.outcome, Outcome::Success);
        assert!(harness.ran("rebase origin/other"));
        assert_eq!(harness.read("b.txt"), "local\n");
    }

    // Merge does not rewrite anything, and `Pull…` is a `git pull` in IntelliJ.
    #[test]
    fn merge_updates_and_pull_do_not_ask() {
        let merging = Harness::new();
        merging.rebasing_a_published_commit();

        let result = merging.update(UpdateMethod::Merge);

        assert_eq!(result.outcome, Outcome::Success);

        let pulling = Harness::new();
        pulling.rebasing_a_published_commit();
        let sync = pulling.sync.clone();

        let result = pulling.in_repo_unlocked(move |repo| {
            crate::pull::pull(
                repo,
                &sync,
                "origin",
                "feature",
                &[crate::pull::PullOption::Rebase],
                || Ok(()),
            )
        });

        assert_eq!(result.outcome, Outcome::Success);
        assert!(pulling.ran("rebase origin/feature"));
        assert_eq!(pulling.read("b.txt"), "local\n");
    }

    // `Pull…` with rebase is a `git pull --rebase` in IntelliJ, which does not ask about merges either.
    #[test]
    fn pull_with_rebase_over_resolved_merges_does_not_ask() {
        let harness = Harness::new();
        harness.resolved_merge_on_main();
        harness.incoming("c.txt", "c\n");
        let sync = harness.sync.clone();

        let result = harness.in_repo_unlocked(move |repo| {
            crate::pull::pull(
                repo,
                &sync,
                "origin",
                "main",
                &[crate::pull::PullOption::Rebase],
                || Ok(()),
            )
        });

        assert_ne!(result.outcome, rebasing_merge_commits("main"));
        assert!(harness.ran("rebase origin/main"));
    }

    #[test]
    fn detached_head_gives_not_ready_with_the_intellij_text() {
        let harness = Harness::new();
        harness.git(&["checkout", "-q", "--detach"]);

        let result = harness.update(UpdateMethod::Merge);

        assert_eq!(
            not_ready(&result),
            "You are in the 'detached HEAD' state, which means that you're not on any branch\n\
             Checkout a branch to be able to update the project."
        );
        assert_eq!(
            result.title.as_deref(),
            Some("Cannot Update: No Current Branch")
        );
    }

    #[test]
    fn without_a_tracked_branch_it_offers_to_choose_one() {
        let harness = Harness::new();
        harness.git(&["checkout", "-q", "-b", "loose"]);

        let result = harness.update(UpdateMethod::Merge);

        let branch = "loose".to_string();
        assert_eq!(result.outcome, Outcome::NoTrackedBranch { branch });
        assert_eq!(result.title.as_deref(), Some("Cannot update"));
    }

    #[test]
    fn set_as_tracked_branch_sets_upstream() {
        let harness = Harness::new();
        harness.git(&["checkout", "-q", "-b", "loose"]);
        let tip = harness.incoming("c.txt", "c\n");
        let options = UpdateOptions {
            target: Some("origin/main".to_string()),
            set_upstream: true,
            ..Default::default()
        };

        let result = harness.update_with(UpdateMethod::Merge, options);

        assert_eq!(result.outcome, Outcome::Success);
        assert_eq!(harness.head(), tip);
        assert_eq!(
            harness
                .git(&["rev-parse", "--abbrev-ref", "loose@{upstream}"])
                .trim(),
            "origin/main"
        );
    }

    #[test]
    fn choosing_a_branch_without_ticking_the_box_does_not_set_upstream() {
        let harness = Harness::new();
        harness.git(&["checkout", "-q", "-b", "loose"]);
        harness.incoming("c.txt", "c\n");
        let options = UpdateOptions {
            target: Some("origin/main".to_string()),
            ..Default::default()
        };

        harness.update_with(UpdateMethod::Merge, options);

        assert_eq!(harness.read("c.txt"), "c\n");
        let upstream = harness.git(&["for-each-ref", "--format=%(upstream)", "refs/heads/loose"]);
        assert_eq!(upstream.trim(), "");
    }

    #[test]
    fn a_chosen_branch_with_a_dash_does_not_reach_git() {
        let harness = Harness::new();
        let options = UpdateOptions {
            target: Some("--upload-pack=x".to_string()),
            ..Default::default()
        };

        let result = harness.update_with(UpdateMethod::Merge, options);

        assert!(matches!(result.outcome, Outcome::Error(_)), "{result:?}");
        assert!(!harness.ran("--upload-pack=x --no-stat -v"));
    }

    #[test]
    fn merge_in_progress_gives_not_ready() {
        let harness = Harness::new();
        harness.diverged_on_a_txt();
        harness.git(&["checkout", "-q", "--", "a.txt"]);
        harness.git(&["fetch", "-q", "origin"]);
        git_may_fail(harness.repo.path(), &["merge", "origin/main"]);

        let result = harness.update(UpdateMethod::Merge);

        assert_eq!(
            not_ready(&result),
            "You have unfinished merge. These conflicts must be resolved before update."
        );
        assert_eq!(result.title.as_deref(), Some("Cannot update"));
    }

    #[test]
    fn rebase_in_progress_gives_not_ready() {
        let harness = Harness::new();
        harness.diverged_on_a_txt();
        harness.git(&["checkout", "-q", "--", "a.txt"]);
        harness.git(&["fetch", "-q", "origin"]);
        git_may_fail(harness.repo.path(), &["rebase", "origin/main"]);

        let result = harness.update(UpdateMethod::Merge);

        assert_eq!(
            not_ready(&result),
            "You have unfinished rebase process. These conflicts must be resolved before update."
        );
    }

    #[test]
    fn failed_fetch_gives_not_ready_with_update_failed_and_does_not_save() {
        let harness = Harness::new();
        harness.write("a.txt", "local\n");
        harness.git(&["remote", "set-url", "origin", "/does/not/exist"]);

        let result = harness.update(UpdateMethod::Merge);

        assert!(
            not_ready(&result).contains("does not appear to be a git repository"),
            "{result:?}"
        );
        assert_eq!(result.title.as_deref(), Some("Update failed"));
        assert_eq!(harness.stash_count(), 0);
        assert_eq!(harness.read("a.txt"), "local\n");
    }

    #[test]
    fn the_tracked_branch_deleted_on_the_remote_stops_counting() {
        let harness = Harness::new();
        harness.git(&["push", "-q", "-u", "origin", "main:other"]);
        harness.git(&["checkout", "-q", "-B", "other", "origin/other"]);
        harness.colleague.git(&["push", "-q", "origin", ":other"]);

        let result = harness.update(UpdateMethod::Merge);

        let branch = "other".to_string();
        assert_eq!(result.outcome, Outcome::NoTrackedBranch { branch });
    }

    // What was checked before the fetch no longer holds afterwards: the network can take minutes.
    #[test]
    fn it_checks_the_state_again_after_the_fetch() {
        let harness = Harness::wrapping(
            "*\" fetch origin \"*)\n  git \"$@\"\n  git checkout -q -b meanwhile\n  ;;",
        );
        harness.incoming("c.txt", "c\n");

        let result = harness.update(UpdateMethod::Merge);

        let branch = "meanwhile".to_string();
        assert_eq!(result.outcome, Outcome::NoTrackedBranch { branch });
        assert!(!harness.ran(MERGE));
    }

    #[test]
    fn a_slow_fetch_does_not_stop_the_commits() {
        let harness = Harness::wrapping(
            "*\" fetch origin \"*)\n  : > \"{mark}\"\n  while [ ! -e \"{mark}.go\" ]; do sleep 0.02; done\n  exec git \"$@\"\n  ;;",
        );
        harness.incoming("c.txt", "c\n");
        harness.write("a.txt", "local\n");

        let (_, result) = harness.start(UpdateMethod::Merge, UpdateOptions::default());
        harness.wait_for_mark();

        let free = harness.locks.try_run(harness.repo.path(), POLL, || ());
        fs::write(harness.scripts.path().join("mark.go"), "").unwrap();
        assert!(free.is_some());
        assert_eq!(result.recv_timeout(WAIT).unwrap().outcome, Outcome::Success);
    }

    #[test]
    fn the_lock_is_held_from_saving_to_restoring() {
        let harness = Harness::wrapping(
            "*\" merge origin/main \"*)\n  git \"$@\"\n  status=$?\n  : > \"{mark}\"\n  while [ ! -e \"{mark}.go\" ]; do sleep 0.02; done\n  exit $status\n  ;;",
        );
        harness.write("a.txt", "local\n2\n3\n");
        harness.incoming("a.txt", "1\n2\nremote\n");

        let (_, result) = harness.start(UpdateMethod::Merge, UpdateOptions::default());
        harness.wait_for_mark();

        let free = harness.locks.try_run(harness.repo.path(), POLL, || ());
        fs::write(harness.scripts.path().join("mark.go"), "").unwrap();
        assert!(free.is_none());
        assert_eq!(
            result.recv_timeout(WAIT).unwrap().restore,
            Some(RestoreOutcome::Restored)
        );
    }

    #[test]
    fn what_is_in_the_diff_is_saved_again_before_taking_the_lock() {
        let harness = Harness::new();
        harness.incoming("c.txt", "c\n");
        let (send, seen) = mpsc::channel();
        let locks = harness.locks.clone();
        let root = harness.repo.path().to_path_buf();
        let sync = harness.sync.clone();

        let result = harness.in_repo_unlocked(move |repo| {
            update_project(
                repo,
                &sync,
                UpdateMethod::Merge,
                UpdateOptions::default(),
                || {
                    send.send(locks.try_run(&root, POLL, || ()).is_some())
                        .unwrap();
                    Ok(())
                },
            )
        });

        assert_eq!(result.outcome, Outcome::Success);
        assert!(seen.recv_timeout(WAIT).unwrap());
    }

    #[test]
    fn if_the_diff_cannot_be_saved_nothing_is_touched() {
        let harness = Harness::new();
        harness.incoming("c.txt", "c\n");
        harness.write("a.txt", "local\n");
        let head = harness.head();
        let sync = harness.sync.clone();

        let result = harness.in_repo_unlocked(move |repo| {
            update_project(
                repo,
                &sync,
                UpdateMethod::Merge,
                UpdateOptions::default(),
                || Err(GitError::Io("could not save".to_string())),
            )
        });

        assert!(matches!(result.outcome, Outcome::Error(_)), "{result:?}");
        assert_eq!(harness.head(), head);
        assert_eq!(harness.read("a.txt"), "local\n");
    }

    #[test]
    fn conflict_leaves_merge_head_and_the_stash() {
        let harness = Harness::new();
        harness.diverged_on_a_txt();

        let result = harness.update(UpdateMethod::Merge);

        assert_eq!(result.outcome, Outcome::Conflicts);
        assert!(result.in_progress);
        assert_eq!(result.restore, None);
        let saved = result.saved.unwrap();
        assert_eq!(harness.git(&["rev-parse", "stash@{0}"]).trim(), saved.sha);
        assert!(harness.in_git_dir("MERGE_HEAD"));
        assert!(harness.read("a.txt").contains("<<<<<<<"));
    }

    #[test]
    fn conflict_in_rebase_leaves_the_rebase_state() {
        let harness = Harness::new();
        harness.diverged_on_a_txt();

        let result = harness.update(UpdateMethod::Rebase);

        assert_eq!(result.outcome, Outcome::Conflicts);
        assert!(result.in_progress);
        assert_eq!(result.restore, None);
        assert_eq!(harness.stash_count(), 1);
        assert!(harness.in_git_dir("rebase-merge") || harness.in_git_dir("rebase-apply"));
    }

    #[test]
    fn conflict_with_nothing_saved_can_also_be_aborted() {
        let harness = Harness::new();
        harness.diverged_on_a_txt();
        harness.git(&["checkout", "-q", "--", "a.txt"]);

        let result = harness.update(UpdateMethod::Merge);

        assert_eq!(result.outcome, Outcome::Conflicts);
        assert_eq!(result.saved, None);
        assert!(result.in_progress);
    }

    #[test]
    // Decision 10: after an error the changes stay saved, as in `GitUpdateProcess`.
    fn unversioned_that_would_be_overwritten_gives_error_and_keeps_the_changes_saved() {
        let harness = Harness::new();
        harness.colleague.commit("new.txt", b"theirs\n", "new");
        harness.incoming("a.txt", "1\n2\nremote\n");
        harness.write("new.txt", "mine\n");
        harness.write("a.txt", "local\n2\n3\n");

        let result = harness.update(UpdateMethod::Merge);

        assert_eq!(
            result.outcome,
            Outcome::Error("Move or commit them before merge\nnew.txt".to_string())
        );
        assert_eq!(
            result.title.as_deref(),
            Some("Untracked Files Prevent Merge")
        );
        assert!(!result.in_progress);
        assert_eq!(result.restore, None);
        assert!(result.still_saved());
        assert_eq!(harness.read("new.txt"), "mine\n");
        assert_eq!(harness.read("a.txt"), "1\n2\n3\n");
        assert_eq!(harness.git(&["show", "stash@{0}:a.txt"]), "local\n2\n3\n");
    }

    #[test]
    fn devin_overwrites_the_files_of_the_merge_before_it_starts_and_git_refuses() {
        let harness = Harness::wrapping(
            "*\" merge origin/main \"*)\n  printf 'devin\\n' > a.txt\n  exec git \"$@\"\n  ;;",
        );
        harness.write("a.txt", "local\n2\n3\n");
        harness.incoming("a.txt", "1\n2\nremote\n");

        let result = harness.update(UpdateMethod::Merge);

        assert_eq!(
            result.outcome,
            Outcome::Error(
                "Your local changes will be overwritten by merge.\n\
                 Commit, stash, or revert them to proceed.\na.txt"
                    .to_string()
            )
        );
        assert_eq!(
            result.title.as_deref(),
            Some("Local Changes Prevent from Merge")
        );
        // Decision 10: after the error the changes stay saved; Devin's version stays on disk.
        assert_eq!(result.restore, None);
        assert_eq!(harness.read("a.txt"), "devin\n");
        assert_eq!(harness.git(&["show", "stash@{0}:a.txt"]), "local\n2\n3\n");
    }

    #[test]
    fn a_new_file_from_devin_in_the_place_of_a_saved_one_also_blocks() {
        let harness = Harness::wrapping(
            "*\" merge origin/main \"*)\n  git \"$@\"\n  status=$?\n  printf 'devin\\n' > new.txt\n  exit $status\n  ;;",
        );
        harness.write("new.txt", "mine\n");
        harness.git(&["add", "new.txt"]);
        harness.incoming("a.txt", "1\n2\nremote\n");

        let result = harness.update(UpdateMethod::Merge);

        assert_eq!(result.restore, Some(blocked("new.txt")));
        assert_eq!(harness.read("new.txt"), "devin\n");
        assert_eq!(harness.git(&["show", "stash@{0}:new.txt"]), "mine\n");
    }

    fn shelving() -> Harness {
        let harness = Harness::new();
        crate::settings::save_clean_tree(harness.data.path(), CleanTree::Shelve).unwrap();
        harness
    }

    fn alive_shelves(harness: &Harness) -> Vec<crate::shelf::ShelvedList> {
        crate::shelf::list(harness.data.path())
            .into_iter()
            .filter(|list| !list.recycled && !list.deleted)
            .collect()
    }

    #[test]
    fn update_with_shelve_saves_one_shelf_per_list_and_restores_into_it() {
        let harness = shelving();
        harness.write("a.txt", "local\n2\n3\n");
        harness.write("b.txt", "local b\n");
        let mut lists = crate::changelists::Changelists::load(harness.data.path());
        let other = lists.create("Other").unwrap();
        lists.move_paths(&["b.txt".to_string()], &other).unwrap();
        lists.save(harness.data.path()).unwrap();
        harness.incoming("a.txt", "1\n2\nremote\n");

        let result = harness.update(UpdateMethod::Merge);

        assert_eq!(result.outcome, Outcome::Success);
        let saved = result.saved.unwrap();
        assert_eq!(saved.kind, CleanTree::Shelve);
        let names: Vec<String> = saved
            .lists
            .iter()
            .map(|list| list.description.clone())
            .collect();
        assert_eq!(
            names,
            [
                format!("{} [Changes]", saved.message),
                format!("{} [Other]", saved.message)
            ]
        );
        assert_eq!(result.restore, Some(RestoreOutcome::Restored));
        assert_eq!(harness.read("a.txt"), "local\n2\nremote\n");
        assert_eq!(harness.read("b.txt"), "local b\n");
        assert_eq!(harness.stash_count(), 0);
        assert_eq!(alive_shelves(&harness), []);
        let lists = crate::changelists::Changelists::load(harness.data.path());
        assert_eq!(lists.list_for("b.txt"), other);
        assert!(!harness.ran("stash"));
    }

    // A save that fails while it rolls back still names what it saved: the lists hold the changes,
    // and the card offers `View saved changes…`.
    #[test]
    fn a_save_that_fails_after_a_partial_rollback_still_says_what_was_saved() {
        let harness = Harness::wrapping("*\" restore \"*)\n  echo boom >&2\n  exit 1\n  ;;");
        crate::settings::save_clean_tree(harness.data.path(), CleanTree::Shelve).unwrap();
        harness.write("a.txt", "local\n2\n3\n");
        harness.incoming("a.txt", "1\n2\nremote\n");

        let result = harness.update(UpdateMethod::Merge);

        assert_eq!(
            result.title.as_deref(),
            Some("Could not save uncommitted changes")
        );
        assert!(format!("{result:?}").contains("boom"));
        let saved = result.saved.clone().unwrap();
        assert_eq!(saved.kind, CleanTree::Shelve);
        assert_eq!(result.restore, None);
        assert!(result.still_saved());
        assert_eq!(alive_shelves(&harness).len(), 1);
        assert_eq!(harness.read("a.txt"), "local\n2\n3\n");
    }

    // Review Focus 4: the changes stay in the shelf, with nothing to restore by hand.
    #[test]
    fn a_stopped_update_leaves_the_shelf_and_offers_view_saved_changes() {
        let harness = shelving();
        harness.diverged_on_a_txt();

        let result = harness.update(UpdateMethod::Merge);

        assert_eq!(result.outcome, Outcome::Conflicts);
        assert!(result.in_progress);
        assert_eq!(result.restore, None);
        assert!(result.still_saved());
        let saved = result.saved.unwrap();
        assert_eq!(saved.kind, CleanTree::Shelve);
        let kept = alive_shelves(&harness);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].name, saved.lists[0].name);
        assert!(kept[0].description.ends_with(" [Changes]"));
        assert_eq!(harness.stash_count(), 0);
        // Nothing is recorded for `Restore Changes`: that button is for the stashes of earlier builds.
        assert_eq!(
            preserve::pending(harness.repo.path(), harness.data.path()),
            Ok(Vec::new())
        );

        harness.in_repo(abort).unwrap();
        let outcome = harness.in_repo(move |repo| restore_requested(repo, &saved));
        assert_eq!(outcome, RestoreOutcome::Restored);
        assert_eq!(harness.read("a.txt"), "mine\n2\nlocal\n");
    }

    #[test]
    fn a_real_conflict_is_still_a_conflict() {
        let harness = Harness::new();
        harness.write("a.txt", "local\n2\n3\n");
        harness.incoming("a.txt", "remote\n2\n3\n");

        let result = harness.update(UpdateMethod::Merge);

        assert_eq!(result.restore, Some(RestoreOutcome::Conflicts));
        assert!(harness.read("a.txt").contains("<<<<<<<"));
        assert_eq!(harness.stash_count(), 1);
    }

    // The review's reproduction: after resolving and `git add`, `pop` succeeds and drops the entry;
    // `rebase --abort` then takes away the only thing left of those changes.
    #[test]
    fn restoring_with_a_stopped_rebase_does_not_touch_the_entry() {
        let harness = Harness::new();
        harness.diverged_on_a_txt();
        let saved = harness.update(UpdateMethod::Rebase).saved.unwrap();
        harness.write("a.txt", "resolved\n2\n3\n");
        harness.git(&["add", "a.txt"]);
        let sha = saved.sha.clone();

        let outcome = harness.in_repo(move |repo| restore_requested(repo, &saved));

        assert_eq!(outcome, RestoreOutcome::Failed(FINISH_FIRST.to_string()));
        assert_eq!(harness.git(&["rev-parse", "stash@{0}"]).trim(), sha);
        assert_eq!(harness.read("a.txt"), "resolved\n2\n3\n");
    }

    #[test]
    fn restoring_with_a_stopped_merge_does_not_touch_the_entry() {
        let harness = Harness::new();
        harness.diverged_on_a_txt();
        let saved = harness.update(UpdateMethod::Merge).saved.unwrap();
        harness.write("a.txt", "resolved\n2\n3\n");
        harness.git(&["add", "a.txt"]);

        let outcome = harness.in_repo(move |repo| restore_requested(repo, &saved));

        assert_eq!(outcome, RestoreOutcome::Failed(FINISH_FIRST.to_string()));
        assert_eq!(harness.stash_count(), 1);
    }

    #[test]
    fn after_restarting_what_was_saved_and_what_is_half_done_remain() {
        let harness = Harness::new();
        harness.diverged_on_a_txt();
        let saved = harness.update(UpdateMethod::Merge).saved.unwrap();
        // This build records nothing; an earlier one did, for `Restore Changes`.
        let none = preserve::pending(harness.repo.path(), harness.data.path());
        assert_eq!(none, Ok(Vec::new()));
        let data = harness.data.path();
        preserve::record_earlier_build(data, &saved.sha, "Update", &saved.message);

        let pending = preserve::pending(harness.repo.path(), harness.data.path()).unwrap();

        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].sha, saved.sha);
        assert_eq!(pending[0].title, "Update");
        assert_eq!(pending[0].message, saved.message);
        assert!(stopped_midway(harness.repo.path()));

        harness.in_repo(abort).unwrap();
        assert!(!stopped_midway(harness.repo.path()));
        harness.in_repo(move |repo| restore_requested(repo, &saved));
        assert_eq!(
            preserve::pending(harness.repo.path(), harness.data.path()),
            Ok(Vec::new())
        );
    }

    #[test]
    fn a_rebase_someone_else_started_also_counts_as_half_done() {
        let harness = Harness::new();
        harness.diverged_on_a_txt();
        harness.git(&["checkout", "-q", "--", "a.txt"]);
        harness.git(&["fetch", "-q", "origin"]);
        git_may_fail(harness.repo.path(), &["rebase", "origin/main"]);

        assert!(stopped_midway(harness.repo.path()));
    }

    #[test]
    fn a_chosen_branch_is_not_merged_into_another_if_the_branch_changes() {
        let harness = Harness::wrapping(
            "*\" fetch origin \"*)\n  git \"$@\"\n  git checkout -q -b meanwhile\n  ;;",
        );
        let head = harness.head();
        harness.incoming("c.txt", "c\n");
        let options = UpdateOptions {
            target: Some("origin/main".to_string()),
            ..Default::default()
        };

        let result = harness.update_with(UpdateMethod::Merge, options);

        assert_eq!(not_ready(&result), BRANCH_CHANGED);
        assert_eq!(result.title.as_deref(), Some("Cannot update"));
        assert_eq!(harness.head(), head);
        assert!(!harness.ran(MERGE));
    }

    #[test]
    fn the_tracked_branch_is_not_swapped_for_another_during_the_fetch_either() {
        let harness =
            Harness::wrapping("*\" fetch origin \"*)\n  git \"$@\"\n  git checkout -q other\n  ;;");
        tracked_other(&harness);
        harness.incoming("c.txt", "c\n");

        let result = harness.update(UpdateMethod::Merge);

        assert_eq!(not_ready(&result), BRANCH_CHANGED);
    }

    #[test]
    fn cancelling_merge_does_reset_merge_and_keeps_the_changes_saved() {
        let harness = Harness::wrapping(
            "*\" merge origin/main \"*)\n  git \"$@\"\n  : > \"{mark}\"\n  sleep 30\n  ;;",
        );
        harness.diverged_on_a_txt();
        let head = harness.head();

        let result = harness.cancelled_at_mark(UpdateMethod::Merge);

        assert_eq!(result.outcome, Outcome::Cancelled);
        assert!(!result.in_progress);
        assert!(harness.ran("reset --merge"));
        assert!(!harness.in_git_dir("MERGE_HEAD"));
        assert_eq!(harness.head(), head);
        assert_eq!(result.restore, None);
        assert_eq!(harness.read("a.txt"), "mine\n2\n3\n");
        assert_eq!(harness.stash_count(), 1);
        assert_eq!(snapshots::list(harness.data.path()).len(), 1);
    }

    // git runs `pre-merge-commit` before writing `MERGE_HEAD`: the index and the tree are already
    // merged and there is no sign left that there was a merge.
    #[test]
    fn cancelling_a_clean_merge_before_merge_head_undoes_what_was_merged() {
        let harness = Harness::wrapping("*\" never \"*) ;;");
        harness.repo.commit("c.txt", b"c\n", "own");
        harness.incoming("a.txt", "theirs\n2\n3\n");
        harness.write("a.txt", "1\n2\nlocal\n");
        harness.write("another.txt", "untouched\n");
        let hook = harness.repo.path().join(".git/hooks/pre-merge-commit");
        let text = format!("#!/bin/sh\n: > '{}'\nsleep 30\n", harness.mark().display());
        fs::write(&hook, text).unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        let head = harness.head();

        let result = harness.cancelled_at_mark(UpdateMethod::Merge);

        assert_eq!(result.outcome, Outcome::Cancelled);
        assert!(!result.in_progress);
        assert_eq!(harness.head(), head);
        // Decision 10: the changes stay saved after a cancel.
        assert_eq!(result.restore, None);
        assert_eq!(harness.read("a.txt"), "1\n2\n3\n");
        assert_eq!(harness.git(&["show", "stash@{0}:a.txt"]), "1\n2\nlocal\n");
        assert_eq!(harness.read("another.txt"), "untouched\n");
        let status = harness
            .repo
            .git(&["status", "--porcelain", "--untracked-files=no"]);
        assert_eq!(status.trim(), "");
    }

    #[test]
    fn cancelling_rebase_does_abort_and_keeps_the_changes_saved() {
        let harness = Harness::wrapping(
            "*\" rebase origin/main \"*)\n  git \"$@\"\n  : > \"{mark}\"\n  sleep 30\n  ;;",
        );
        harness.diverged_on_a_txt();
        let head = harness.head();

        let result = harness.cancelled_at_mark(UpdateMethod::Rebase);

        assert_eq!(result.outcome, Outcome::Cancelled);
        assert!(!result.in_progress);
        assert!(harness.ran("rebase --abort"));
        assert!(!harness.in_git_dir("rebase-merge"));
        assert_eq!(harness.head(), head);
        assert_eq!(result.restore, None);
        assert_eq!(harness.read("a.txt"), "mine\n2\n3\n");
        assert_eq!(harness.stash_count(), 1);
    }

    #[test]
    fn cancelling_the_fetch_touches_nothing() {
        let harness =
            Harness::wrapping("*\" fetch origin \"*)\n  : > \"{mark}\"\n  sleep 30\n  ;;");
        harness.write("a.txt", "local\n");

        let result = harness.cancelled_at_mark(UpdateMethod::Merge);

        assert_eq!(result.outcome, Outcome::Cancelled);
        assert_eq!(result.saved, None);
        assert_eq!(harness.read("a.txt"), "local\n");
    }

    #[test]
    fn if_it_cannot_be_aborted_everything_stays_saved() {
        let harness = Harness::wrapping(
            "*\" merge origin/main \"*)\n  git \"$@\"\n  : > \"{mark}\"\n  sleep 30\n  ;;\n*\" reset --merge \"*)\n  echo 'fatal: cannot' >&2\n  exit 128\n  ;;",
        );
        harness.diverged_on_a_txt();

        let result = harness.cancelled_at_mark(UpdateMethod::Merge);

        assert!(matches!(result.outcome, Outcome::Error(_)), "{result:?}");
        assert_eq!(
            result.title.as_deref(),
            Some("Failed to abort the merge operation")
        );
        assert!(result.in_progress);
        assert_eq!(result.restore, None);
        assert_eq!(harness.stash_count(), 1);
    }

    // Git refuses to touch a dirty file: there are no conflict markers, and both versions remain
    // whole, one on disk and the other in the stash.
    #[test]
    fn devin_writes_while_it_is_saved() {
        let harness = Harness::wrapping(
            "*\" merge origin/main \"*)\n  git \"$@\"\n  status=$?\n  printf 'devin\\n' > a.txt\n  exit $status\n  ;;",
        );
        harness.write("a.txt", "local\n2\n3\n");
        harness.incoming("a.txt", "1\n2\nremote\n");

        let result = harness.update(UpdateMethod::Merge);

        assert_eq!(result.outcome, Outcome::Success);
        assert_eq!(result.restore, Some(blocked("a.txt")));
        let saved = result.saved.unwrap();
        assert_eq!(harness.git(&["rev-parse", "stash@{0}"]).trim(), saved.sha);
        assert_eq!(harness.read("a.txt"), "devin\n");
        assert_eq!(harness.git(&["show", "stash@{0}:a.txt"]), "local\n2\n3\n");

        harness.git(&["checkout", "-q", "--", "a.txt"]);
        let outcome = harness.in_repo(move |repo| restore_requested(repo, &saved));
        assert_eq!(outcome, RestoreOutcome::Restored);
        assert_eq!(harness.read("a.txt"), "local\n2\nremote\n");
    }

    #[test]
    fn restoring_does_not_take_another_stash() {
        let harness = Harness::wrapping(
            "*\" merge origin/main \"*)\n  git \"$@\"\n  status=$?\n  printf 'another\\n' > b.txt\n  git stash save -q another\n  exit $status\n  ;;",
        );
        harness.write("a.txt", "local\n2\n3\n");
        harness.incoming("a.txt", "1\n2\nremote\n");

        let result = harness.update(UpdateMethod::Merge);

        assert_eq!(result.restore, Some(RestoreOutcome::Restored));
        assert_eq!(harness.read("a.txt"), "local\n2\nremote\n");
        assert_eq!(harness.read("b.txt"), "b\n");
        assert_eq!(harness.stash_count(), 1);
        assert!(harness
            .git(&["stash", "list", "--format=%s"])
            .contains("another"));
    }

    #[test]
    fn restoring_when_someone_else_already_took_it_out_touches_nothing() {
        let harness = Harness::new();
        harness.diverged_on_a_txt();
        let saved = harness.update(UpdateMethod::Merge).saved.unwrap();
        harness.in_repo(abort).unwrap();
        harness.git(&["stash", "drop", "-q"]);

        let outcome = harness.in_repo(move |repo| restore_requested(repo, &saved));

        assert_eq!(outcome, RestoreOutcome::Gone);
        assert_eq!(
            preserve::pending(harness.repo.path(), harness.data.path()),
            Ok(Vec::new())
        );
    }

    #[test]
    fn abort_of_a_merge_copies_and_does_not_restore_by_itself() {
        let harness = Harness::new();
        harness.diverged_on_a_txt();
        let saved = harness.update(UpdateMethod::Merge).saved.unwrap();

        let aborted = harness.in_repo(abort);

        assert_eq!(aborted, Ok(Aborted::Merge));
        assert!(harness.ran("merge --abort"));
        assert!(!harness.in_git_dir("MERGE_HEAD"));
        assert_eq!(harness.read("a.txt"), "mine\n2\n3\n");
        assert_eq!(harness.stash_count(), 1);
        assert_eq!(snapshots::list(harness.data.path()).len(), 1);

        let outcome = harness.in_repo(move |repo| restore_requested(repo, &saved));
        assert_eq!(outcome, RestoreOutcome::Restored);
        assert_eq!(harness.read("a.txt"), "mine\n2\nlocal\n");
        assert_eq!(harness.stash_count(), 0);
    }

    #[test]
    fn abort_of_a_rebase_copies_and_does_not_restore_by_itself() {
        let harness = Harness::new();
        harness.diverged_on_a_txt();
        harness.update(UpdateMethod::Rebase);

        let aborted = harness.in_repo(abort);

        assert_eq!(aborted, Ok(Aborted::Rebase));
        assert!(harness.ran("rebase --abort"));
        assert_eq!(harness.read("a.txt"), "mine\n2\n3\n");
        assert_eq!(harness.stash_count(), 1);
        assert_eq!(snapshots::list(harness.data.path()).len(), 1);
    }

    // Unresolved paths from a `stash pop` with conflicts belong to no operation: aborting does not
    // touch them, and there is no success to announce.
    #[test]
    fn abort_with_unresolved_paths_and_no_operation_does_nothing() {
        let harness = Harness::new();
        harness.write("a.txt", "local\n");
        harness.git(&["stash", "save", "-q", "saved"]);
        harness.repo.commit("a.txt", b"another\n", "another");
        git_may_fail(harness.repo.path(), &["stash", "pop"]);
        assert_ne!(harness.git(&["ls-files", "--unmerged"]), "");
        let marked = harness.read("a.txt");

        assert_eq!(harness.in_repo(abort), Ok(Aborted::Nothing));

        assert!(!harness.ran("reset --merge"));
        assert_eq!(harness.read("a.txt"), marked);
        assert_ne!(harness.git(&["ls-files", "--unmerged"]), "");
        assert_eq!(snapshots::list(harness.data.path()).len(), 0);
    }

    #[test]
    fn abort_with_nothing_in_progress_does_nothing() {
        let harness = Harness::new();
        harness.write("a.txt", "local\n");

        assert_eq!(harness.in_repo(abort), Ok(Aborted::Nothing));
        assert!(!harness.ran("reset --merge"));
        assert_eq!(harness.read("a.txt"), "local\n");
    }

    #[test]
    fn counts_files_and_commits_of_the_range() {
        let harness = Harness::new();
        let base = harness.head();
        harness.repo.commit("mine.txt", b"m\n", "own");
        harness.colleague.commit("c.txt", b"c\n", "one from outside");
        let tip = harness.incoming("d.txt", "d\n");

        let result = harness.update(UpdateMethod::Merge);

        assert_eq!(result.commits, 2);
        assert_eq!(result.files, 2, "the own file has not been updated");
        assert_eq!(result.range, Some((base, tip)));
    }

    fn matching(harness: &Harness, result: &UpdateResult, paths: &[&str]) -> Option<u32> {
        let result = result.clone();
        let paths: Vec<String> = paths.iter().map(|path| path.to_string()).collect();
        harness.in_repo(move |repo| with_matching(repo.ctx, result, &paths).matching)
    }

    #[test]
    fn the_path_filter_counts_the_received_commits_that_touch_them() {
        let harness = Harness::new();
        harness.colleague.commit("src/a.rs", b"1\n", "in src");
        harness.colleague.commit("docs/d.md", b"1\n", "in docs");
        harness.incoming("src/b.rs", "2\n");
        let result = harness.update(UpdateMethod::Merge);
        assert_eq!(result.commits, 3);

        assert_eq!(matching(&harness, &result, &["src"]), Some(2));
        assert_eq!(matching(&harness, &result, &["docs", "src/b.rs"]), Some(2));
        assert_eq!(matching(&harness, &result, &["docs"]), Some(1));
        assert_eq!(matching(&harness, &result, &["nothing"]), Some(0));
        assert_eq!(
            matching(&harness, &result, &[]),
            None,
            "without a filter there is no count"
        );
    }

    #[test]
    fn the_path_filter_does_not_interpret_wildcards_or_options() {
        let harness = Harness::new();
        harness.incoming("src/a.rs", "1\n");
        let result = harness.update(UpdateMethod::Merge);

        assert_eq!(matching(&harness, &result, &["*.rs"]), Some(0));
        assert_eq!(matching(&harness, &result, &["--all"]), Some(0));
        assert_eq!(matching(&harness, &result, &["src/a.rs"]), Some(1));
    }

    #[test]
    fn without_a_range_the_path_filter_counts_nothing() {
        let harness = Harness::new();
        let result = harness.update(UpdateMethod::Merge);
        assert_eq!(result.range, None);

        assert_eq!(matching(&harness, &result, &["src"]), None);
    }

    #[test]
    fn without_received_commits_there_is_no_range() {
        let harness = Harness::new();
        harness.diverged_on_a_txt();

        let result = harness.update(UpdateMethod::Merge);

        assert_eq!((result.commits, result.files), (0, 0));
        assert_eq!(result.range, None);
    }

    fn tracked_other(harness: &Harness) {
        harness.git(&["push", "-q", "-u", "origin", "main:other"]);
        harness.git(&["branch", "-q", "other", "origin/other"]);
        harness.colleague.git(&["fetch", "-q", "origin"]);
        harness
            .colleague
            .git(&["checkout", "-q", "-B", "other", "origin/other"]);
    }

    fn update_other(harness: &Harness) -> Result<(), GitError> {
        let sync = harness.sync.clone();
        harness.in_repo(move |repo| update_branch(repo.ctx, &sync, "other", "origin", "other"))
    }

    #[test]
    fn update_of_a_non_current_branch_only_advances() {
        let harness = Harness::new();
        tracked_other(&harness);
        harness.write("a.txt", "local\n");
        let tip = harness.colleague.commit("c.txt", b"c\n", "from outside");
        harness.colleague.git(&["push", "-q", "origin", "other"]);

        assert_eq!(update_other(&harness), Ok(()));

        assert_eq!(harness.git(&["rev-parse", "other"]).trim(), tip);
        assert_eq!(harness.read("a.txt"), "local\n");
        assert!(!harness.repo.path().join("c.txt").exists());
        assert!(harness.sync.is_authenticated(harness.repo.path(), "origin"));
        assert!(harness.ran(
            "fetch origin refs/heads/other:refs/heads/other --recurse-submodules=no --progress --prune"
        ));
    }

    #[test]
    fn update_of_a_diverged_branch_fails_without_touching_anything() {
        let harness = Harness::new();
        tracked_other(&harness);
        harness.git(&["checkout", "-q", "other"]);
        let own = harness.repo.commit("mine.txt", b"m\n", "own");
        harness.git(&["checkout", "-q", "main"]);
        harness.colleague.commit("c.txt", b"c\n", "from outside");
        harness.colleague.git(&["push", "-q", "origin", "other"]);

        assert!(update_other(&harness).is_err());

        assert_eq!(harness.git(&["rev-parse", "other"]).trim(), own);
    }

    // `--prune` with a branch that is gone must not take the local one, nor its commits.
    #[test]
    fn update_of_a_branch_deleted_on_the_remote_does_not_delete_the_local() {
        let harness = Harness::new();
        tracked_other(&harness);
        harness.git(&["checkout", "-q", "other"]);
        let own = harness.repo.commit("mine.txt", b"m\n", "own");
        harness.git(&["checkout", "-q", "main"]);
        harness.colleague.git(&["push", "-q", "origin", ":other"]);

        assert!(update_other(&harness).is_err());

        assert_eq!(harness.git(&["rev-parse", "other"]).trim(), own);
    }

    // With a leading `+` the command would force the branch, and lose the own commits.
    #[test]
    fn branch_update_never_forces() {
        let harness = Harness::new();
        tracked_other(&harness);
        harness.git(&["checkout", "-q", "other"]);
        let own = harness.repo.commit("mine.txt", b"m\n", "own");
        harness.git(&["checkout", "-q", "main"]);
        let sync = harness.sync.clone();

        let result =
            harness.in_repo(move |repo| update_branch(repo.ctx, &sync, "other", "origin", "+main"));

        assert!(result.is_err());
        assert_eq!(harness.git(&["rev-parse", "other"]).trim(), own);
    }

    #[test]
    fn the_tracked_branch_of_another_branch() {
        let harness = Harness::new();
        tracked_other(&harness);

        let found = tracked_branch(harness.repo.path(), "other");
        assert_eq!(found, Ok(Some(("origin".to_string(), "other".to_string()))));
        harness.git(&["branch", "-q", "loose"]);
        assert_eq!(tracked_branch(harness.repo.path(), "loose"), Ok(None));
    }

    #[test]
    fn staged_changes_return_to_the_stage() {
        let harness = Harness::new();
        harness.runner.set_staging_area(true);
        harness.write("b.txt", "local\n");
        harness.git(&["add", "b.txt"]);
        harness.incoming("a.txt", "1\n2\nremote\n");

        let result = harness.update(UpdateMethod::Merge);

        assert_eq!(result.restore, Some(RestoreOutcome::Restored));
        assert_eq!(harness.git(&["status", "--porcelain"]), "M  b.txt\n");
    }

    // With staging off `pop` leaves what is on disk: what was only in the stage goes to the copy first.
    #[test]
    fn without_staging_area_what_is_only_in_stage_gets_a_copy() {
        let harness = Harness::new();
        harness.write("b.txt", "in stage\n");
        harness.git(&["add", "b.txt"]);
        harness.write("b.txt", "on disk\n");
        harness.incoming("a.txt", "1\n2\nremote\n");

        harness.update(UpdateMethod::Merge);

        assert_eq!(harness.read("b.txt"), "on disk\n");
        assert_eq!(snapshots::list(harness.data.path()).len(), 1);
    }
}
