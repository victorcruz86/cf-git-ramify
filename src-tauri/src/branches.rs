// Branches, tags and reset, with the steps of `GitBranchWorker`, `GitCheckoutOperation` and
// `GitResetOperation`. Anything that can lose work saves a copy or a stash first.
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::commands::ApiError;
use crate::git::{self, GitError};
use crate::preserve::{self, RestoreOutcome, Saved};
use crate::push;
use crate::settings;
use crate::snapshots;
use crate::task::{GitOutput, TaskContext};
use crate::update;

const HEAD: &str = "HEAD";
const HEADS: &str = "refs/heads/";
const TAGS: &str = "refs/tags/";
pub(crate) const COMMIT_FORMAT: &str = "--format=%H%x00%an%x00%at%x00%s";
const NEW_BRANCH: &str = "-b";
const RESET_BRANCH: &str = "-B";
pub(crate) const LOCAL_CHANGES: &str =
    "Your local changes to the following files would be overwritten by";
const ENTRY: &str = "Entry '";
const ENTRY_ENDS: [&str; 2] = [
    "' not uptodate. Cannot merge",
    "' would be overwritten by merge",
];
// Git 2.38 writes `The branch`, and other versions `the branch`.
const NOT_FULLY_MERGED: &str = "not fully merged";
const MERGED_TO_HEAD: &str = ", even though it is merged to";
// The titles of what is saved, and why each copy was taken.
const CHECKOUT: &str = "Checkout";
const FORCE_CHECKOUT: &str = "Force Checkout";
const RESET: &str = "Reset";
const RESET_HARD: &str = "Reset --hard";
const RESET_KEEP: &str = "Reset --keep";
const RESET_MIXED: &str = "Reset --mixed";
const FINISH_FIRST: &str =
    "A merge or rebase is in progress and saving local changes would break it. Finish or abort it first.";

pub struct Repo<'a> {
    pub ctx: &'a TaskContext,
    pub path: &'a Path,
    // The folder with the repository's own data, where the copies live.
    pub data: &'a Path,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CheckoutMode {
    Plain,
    Smart,
    Force,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CheckoutOutcome {
    Done,
    Overwritten {
        files: Vec<String>,
    },
    // `failure`: the operation also failed, but what was saved is already in the tree.
    Conflicts {
        stash: Saved,
        failure: Option<String>,
    },
    RestoredWithoutIndex {
        stash: Saved,
        failure: Option<String>,
    },
    NotRestored {
        stash: Saved,
        reason: String,
    },
    // Someone else took the entry out of the stash in the meantime: it cannot be said to still be there.
    Gone {
        stash: Saved,
        failure: Option<String>,
    },
}

// What else the thing that creates or moves a branch can answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum RemoteCheckout {
    Done,
    NeedsName {
        suggested: String,
    },
    LocalHasCommits {
        local: String,
    },
    Overwritten {
        files: Vec<String>,
    },
    Conflicts {
        stash: Saved,
        failure: Option<String>,
    },
    RestoredWithoutIndex {
        stash: Saved,
        failure: Option<String>,
    },
    NotRestored {
        stash: Saved,
        reason: String,
    },
    // Someone else took the entry out of the stash in the meantime: it cannot be said to still be there.
    Gone {
        stash: Saved,
        failure: Option<String>,
    },
}

impl From<CheckoutOutcome> for RemoteCheckout {
    fn from(outcome: CheckoutOutcome) -> Self {
        match outcome {
            CheckoutOutcome::Done => Self::Done,
            CheckoutOutcome::Overwritten { files } => Self::Overwritten { files },
            CheckoutOutcome::Conflicts { stash, failure } => Self::Conflicts { stash, failure },
            CheckoutOutcome::RestoredWithoutIndex { stash, failure } => {
                Self::RestoredWithoutIndex { stash, failure }
            }
            CheckoutOutcome::NotRestored { stash, reason } => Self::NotRestored { stash, reason },
            CheckoutOutcome::Gone { stash, failure } => Self::Gone { stash, failure },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Overwrite {
    No,
    // Moves the branch, unless it has commits the target does not have: then it asks.
    Try,
    Drop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResetMode {
    Soft,
    Mixed,
    Hard,
    Keep,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Deleted {
    pub name: String,
    pub tip: String,
    pub upstream: Option<String>,
    pub forced: bool,
    pub base: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletedRemote {
    pub local: Vec<Deleted>,
    // The local branches that could not be deleted: the remote one is gone.
    pub failures: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletedTag {
    pub name: String,
    pub tip: String,
}

#[derive(Debug, Clone, Copy)]
pub struct RemoteOptions<'a> {
    // The one the user gave when asked: if it exists it is overwritten, like `-B`.
    pub name: Option<&'a str>,
    // The name to offer instead of the remote branch's own, with the same questions
    // (`checkoutRemoteBranch(…, suggestedName)`): a pull request's `fork/<branch>`.
    pub suggested: Option<&'a str>,
    pub drop_commits: bool,
    pub mode: CheckoutMode,
}

impl Default for RemoteOptions<'_> {
    fn default() -> Self {
        Self {
            name: None,
            suggested: None,
            drop_commits: false,
            mode: CheckoutMode::Plain,
        }
    }
}

pub(crate) fn invalid(message: String) -> GitError {
    GitError::Io(message)
}

pub(crate) fn text_of(error: GitError) -> String {
    ApiError::from(error).message
}

pub(crate) fn stderr_of(error: &GitError) -> &str {
    match error {
        GitError::Failed { stderr, .. } => stderr,
        _ => "",
    }
}

fn trimmed(output: GitOutput) -> String {
    output.stdout.trim().to_string()
}

// Anything starting with `-` would be an option. `--end-of-options` is no shield: git 2.38's
// `checkout` and `reset` do not accept it, and `check-ref-format` accepts `refs/heads/-x`.
pub(crate) fn check_argument(text: &str, what: &str) -> Result<(), GitError> {
    if text.is_empty() || text.starts_with('-') {
        return Err(invalid(format!("{what} '{text}' is not valid")));
    }
    Ok(())
}

fn check_revision(revision: &str) -> Result<(), GitError> {
    check_argument(revision, "The target")
}

pub(crate) fn ref_is_valid(ctx: &TaskContext, full: &str) -> Result<bool, GitError> {
    match ctx.git(&["check-ref-format", full]) {
        Ok(_) => Ok(true),
        Err(GitError::Failed { .. }) => Ok(false),
        Err(error) => Err(error),
    }
}

// With `refs/heads/` in front and not with `--branch`, which would expand `@{-1}`.
pub(crate) fn check_branch(ctx: &TaskContext, name: &str) -> Result<(), GitError> {
    let full = format!("{HEADS}{name}");
    if name.is_empty() || name.starts_with('-') || name == HEAD || !ref_is_valid(ctx, &full)? {
        return Err(invalid(format!("Branch name {name} is not valid")));
    }
    Ok(())
}

fn check_tag(ctx: &TaskContext, name: &str) -> Result<(), GitError> {
    check_argument(name, "The tag")?;
    if !ref_is_valid(ctx, &format!("{TAGS}{name}"))? {
        return Err(invalid(format!("The tag '{name}' is not valid")));
    }
    Ok(())
}

pub(crate) fn commit_of(repo: &Repo, revision: &str) -> Result<String, GitError> {
    let spec = format!("{revision}^{{commit}}");
    match repo.ctx.git(&["rev-parse", "-q", "--verify", &spec]) {
        Ok(output) => Ok(trimmed(output)),
        Err(GitError::Failed { .. }) => Err(invalid(format!("Could not find \"{revision}\""))),
        Err(error) => Err(error),
    }
}

// The exact reference: `rev-parse` would also accept `refs/heads/x^0`.
pub(crate) fn ref_exists(repo: &Repo, full: &str) -> Result<bool, GitError> {
    match repo.ctx.git(&["show-ref", "--verify", "-q", full]) {
        Ok(_) => Ok(true),
        Err(GitError::Failed { .. }) => Ok(false),
        Err(error) => Err(error),
    }
}

// `None` with a detached HEAD.
pub fn current_name(path: &Path) -> Result<Option<String>, GitError> {
    match git::run(path, &["symbolic-ref", "-q", "--short", HEAD]) {
        Ok(output) => Ok(Some(String::from_utf8_lossy(&output).trim().to_string())),
        Err(GitError::Failed { code: Some(1), .. }) => Ok(None),
        Err(error) => Err(error),
    }
}

pub(crate) fn upstream_of(repo: &Repo, branch: &str) -> Result<Option<String>, GitError> {
    let full = format!("{HEADS}{branch}");
    let output = repo
        .ctx
        .git(&["for-each-ref", "--format=%(upstream:short)", &full])?;
    Ok(Some(trimmed(output)).filter(|upstream| !upstream.is_empty()))
}

// `git` fails if it cannot compare, and then it asks, like `hasCommits`.
pub(crate) fn has_own_commits(repo: &Repo, start: &str, branch: &str) -> bool {
    let range = format!("{start}..{HEADS}{branch}");
    repo.ctx
        .git(&["rev-list", "--count", &range])
        .map_or(true, |output| output.stdout.trim() != "0")
}

// The blocks of the command: `Your local changes…:`, the paths with a tab, and another line.
// `git reset` says `Entry 'a' not uptodate. Cannot merge.`.
pub(crate) fn overwritten_files(stderr: &str) -> Vec<String> {
    let mut files = Vec::new();
    let mut listing = false;
    for line in stderr.lines() {
        if line.contains(LOCAL_CHANGES) {
            listing = true;
        } else if listing {
            match line.strip_prefix('\t') {
                Some(file) => files.push(file.to_string()),
                None => listing = false,
            }
        } else if let Some((_, rest)) = line.split_once(ENTRY) {
            let name = ENTRY_ENDS
                .iter()
                .find_map(|end| rest.rsplit_once(end).map(|(name, _)| name));
            files.extend(name.map(str::to_string));
        }
    }
    files
}

fn or_overwritten(result: Result<GitOutput, GitError>) -> Result<CheckoutOutcome, GitError> {
    let Err(error) = result else {
        return Ok(CheckoutOutcome::Done);
    };
    let files = overwritten_files(stderr_of(&error));
    if files.is_empty() {
        return Err(error);
    }
    Ok(CheckoutOutcome::Overwritten { files })
}

// Like `rollback`: what was written after the copy is not in it. What someone writes after this
// check, in the instant before `git`, is still not covered.
fn copy_pausing(
    repo: &Repo,
    reason: &str,
    at_risk: impl Fn(&Path) -> Result<Vec<String>, GitError>,
    pause: impl FnOnce(),
) -> Result<(), GitError> {
    let paths = at_risk(repo.path)?;
    if paths.is_empty() {
        return Ok(());
    }
    let id = snapshots::create(repo.data, repo.path, &paths, reason).map_err(copy_failed)?;
    pause();
    let now = at_risk(repo.path)?;
    let changed = snapshots::changed_since(repo.data, repo.path, &id, &now).map_err(copy_failed)?;
    if changed.is_empty() {
        return Ok(());
    }
    let names: Vec<String> = changed.iter().map(|path| format!("'{path}'")).collect();
    Err(invalid(format!(
        "{} changed after the copy was saved: nothing was touched",
        names.join(", ")
    )))
}

fn copy_failed(error: std::io::Error) -> GitError {
    invalid(format!(
        "Could not save the backup copy and nothing was touched: {error}"
    ))
}

pub(crate) fn copy_before(
    repo: &Repo,
    reason: &str,
    at_risk: impl Fn(&Path) -> Result<Vec<String>, GitError>,
) -> Result<(), GitError> {
    copy_pausing(repo, reason, at_risk, || {})
}

// Gives back what was saved even if the operation fails or is cancelled, like the `finally` of
// `GitPreservingProcess`. What does not come back whole is reported: there is no other way to know
// it is still in the stash.
fn smart(
    repo: &Repo,
    title: &str,
    work: impl FnOnce() -> Result<CheckoutOutcome, GitError>,
) -> Result<CheckoutOutcome, GitError> {
    // `stash save` deletes `MERGE_HEAD` without saying anything, and `pop` leaves out of the stage
    // what was resolved in a rebase, so `--continue` fails.
    if update::stopped_midway(repo.path) {
        return Err(invalid(FINISH_FIRST.to_string()));
    }
    let saved = preserve::save_local(repo.ctx, repo.path, repo.data, title)?;
    let Some(stash) = saved else {
        return work();
    };
    let result = work();
    let restored = preserve::restore_tracked(repo.ctx, repo.data, &stash);
    // Whoever cancelled already knows the operation did not finish.
    let failure = match &result {
        Err(GitError::Cancelled) | Ok(_) => None,
        Err(error) => Some(text_of(error.clone())),
    };
    match restored {
        RestoreOutcome::Restored => result,
        RestoreOutcome::RestoredWithoutIndex => {
            Ok(CheckoutOutcome::RestoredWithoutIndex { stash, failure })
        }
        RestoreOutcome::Conflicts => Ok(CheckoutOutcome::Conflicts { stash, failure }),
        RestoreOutcome::Failed(reason) => Ok(CheckoutOutcome::NotRestored { stash, reason }),
        RestoreOutcome::RestoreBlocked { files } => {
            let reason = format!(
                "{} changed while saved: commit or move the version on disk first",
                files.join(", ")
            );
            Ok(CheckoutOutcome::NotRestored { stash, reason })
        }
        RestoreOutcome::Gone => Ok(CheckoutOutcome::Gone { stash, failure }),
    }
}

struct Switch<'a> {
    // `-b` or `-B`, and the name.
    branch: Option<(&'static str, &'a str)>,
    start: &'a str,
}

// `git checkout x` prefers the branch and `rev-parse x` the tag: the copy must look at git's tree.
fn checkout_commit(repo: &Repo, switch: &Switch) -> Result<String, GitError> {
    let branch = format!("{HEADS}{}", switch.start);
    if switch.branch.is_none() && ref_exists(repo, &branch)? {
        return commit_of(repo, &branch);
    }
    commit_of(repo, switch.start)
}

fn run_checkout(repo: &Repo, switch: &Switch, force: bool) -> Result<CheckoutOutcome, GitError> {
    let commit = checkout_commit(repo, switch)?;
    if force {
        copy_before(repo, FORCE_CHECKOUT, |path| {
            snapshots::at_risk(path, &commit)
        })?;
    } else {
        copy_before(repo, CHECKOUT, |path| snapshots::in_the_way(path, &commit))?;
    }
    let mut args = vec!["checkout"];
    if force {
        args.push("--force");
    }
    if let Some((flag, name)) = switch.branch {
        args.extend([flag, name]);
    }
    args.extend([switch.start, "--"]);
    or_overwritten(repo.ctx.git_loud(&args))
}

fn switch_to(
    repo: &Repo,
    switch: &Switch,
    mode: CheckoutMode,
) -> Result<CheckoutOutcome, GitError> {
    match mode {
        CheckoutMode::Plain => run_checkout(repo, switch, false),
        CheckoutMode::Smart => smart(repo, CHECKOUT, || run_checkout(repo, switch, false)),
        CheckoutMode::Force => run_checkout(repo, switch, true),
    }
}

pub fn checkout(
    repo: &Repo,
    target: &str,
    mode: CheckoutMode,
) -> Result<CheckoutOutcome, GitError> {
    check_revision(target)?;
    let switch = Switch {
        branch: None,
        start: target,
    };
    switch_to(repo, &switch, mode)
}

// The remote is looked up among the existing ones: its name may contain slashes.
pub fn split_remote(
    path: &Path,
    remote_branch: &str,
) -> Result<Option<(String, String)>, GitError> {
    let output = git::run(path, &["remote"])?;
    let remotes = String::from_utf8_lossy(&output);
    let found = remotes
        .lines()
        .filter_map(|remote| {
            let branch = remote_branch.strip_prefix(remote)?.strip_prefix('/')?;
            Some((remote.to_string(), branch.to_string()))
        })
        .max_by_key(|(remote, _)| remote.len());
    Ok(found.filter(|(_, branch)| !branch.is_empty()))
}

// `CheckoutRemoteBranchAction.checkoutRemoteBranch` with `GitBranchCheckoutOperation.perform`.
pub fn checkout_remote(
    repo: &Repo,
    remote_branch: &str,
    options: RemoteOptions,
) -> Result<RemoteCheckout, GitError> {
    check_revision(remote_branch)?;
    let branch = |flag, name| Switch {
        branch: Some((flag, name)),
        start: remote_branch,
    };
    if let Some(name) = options.name {
        check_branch(repo.ctx, name)?;
        let flag = if ref_exists(repo, &format!("{HEADS}{name}"))? {
            RESET_BRANCH
        } else {
            NEW_BRANCH
        };
        return Ok(switch_to(repo, &branch(flag, name), options.mode)?.into());
    }
    let suggested = match options.suggested {
        // `GitBranchPopupActions.checkoutRemoteBranch`: a name that is the remote branch's,
        // ignoring case, is asked for.
        Some(suggested) if suggested.eq_ignore_ascii_case(remote_branch) => {
            return Ok(RemoteCheckout::NeedsName {
                suggested: suggested.to_string(),
            });
        }
        Some(suggested) => suggested.to_string(),
        None => match split_remote(repo.path, remote_branch)? {
            Some((_, suggested)) => suggested,
            None => {
                return Ok(RemoteCheckout::NeedsName {
                    suggested: remote_branch.to_string(),
                })
            }
        },
    };
    if suggested == HEAD {
        return Ok(RemoteCheckout::NeedsName { suggested });
    }
    check_branch(repo.ctx, &suggested)?;
    if !ref_exists(repo, &format!("{HEADS}{suggested}"))? {
        return Ok(switch_to(repo, &branch(NEW_BRANCH, &suggested), options.mode)?.into());
    }
    if upstream_of(repo, &suggested)?.is_some_and(|tracked| tracked != remote_branch) {
        return Ok(RemoteCheckout::NeedsName { suggested });
    }
    if !options.drop_commits && has_own_commits(repo, remote_branch, &suggested) {
        return Ok(RemoteCheckout::LocalHasCommits { local: suggested });
    }
    Ok(switch_to(repo, &branch(RESET_BRANCH, &suggested), options.mode)?.into())
}

// Without `start`, it starts from `HEAD`. With it goes `X^0`, like `GitBranchPopupActions`: a new
// branch does not track the remote branch it starts from.
pub fn create(
    repo: &Repo,
    name: &str,
    start: Option<&str>,
    checkout: bool,
    overwrite: Overwrite,
    mode: CheckoutMode,
) -> Result<RemoteCheckout, GitError> {
    check_branch(repo.ctx, name)?;
    if let Some(start) = start {
        check_revision(start)?;
    }
    let from = start.unwrap_or(HEAD);
    let moves = overwrite != Overwrite::No;
    if overwrite == Overwrite::Try
        && ref_exists(repo, &format!("{HEADS}{name}"))?
        && has_own_commits(repo, from, name)
    {
        return Ok(RemoteCheckout::LocalHasCommits {
            local: name.to_string(),
        });
    }
    let point = start.map_or(HEAD.to_string(), |start| format!("{start}^0"));
    // `git branch -f` refuses with the current branch.
    if checkout || (moves && current_name(repo.path)?.as_deref() == Some(name)) {
        let flag = if moves { RESET_BRANCH } else { NEW_BRANCH };
        let switch = Switch {
            branch: Some((flag, name)),
            start: &point,
        };
        return Ok(switch_to(repo, &switch, mode)?.into());
    }
    let mut args = vec!["branch"];
    if moves {
        args.push("-f");
    }
    args.extend([name, point.as_str()]);
    repo.ctx.git_loud(&args)?;
    Ok(RemoteCheckout::Done)
}

pub fn rename(repo: &Repo, old: &str, new: &str) -> Result<(), GitError> {
    check_branch(repo.ctx, old)?;
    check_branch(repo.ctx, new)?;
    repo.ctx.git_loud(&["branch", "-m", old, new]).map(drop)
}

// `refs/remotes/origin/x` → `origin/x`, like `GitBranchUtil.stripRefsPrefix`.
fn short_ref(full: &str) -> &str {
    [HEADS, "refs/remotes/", TAGS]
        .iter()
        .find_map(|prefix| full.strip_prefix(prefix))
        .unwrap_or(full)
}

// warning: not deleting branch 'x' that is not yet merged to
//          'refs/remotes/origin/x', even though it is merged to HEAD.
fn unmerged_base(stderr: &str) -> Option<&str> {
    stderr.lines().find_map(|line| {
        let (quoted, _) = line.split_once(MERGED_TO_HEAD)?;
        let (_, base) = quoted.strip_suffix('\'')?.rsplit_once('\'')?;
        Some(short_ref(base))
    })
}

fn head_name(repo: &Repo) -> Result<String, GitError> {
    match current_name(repo.path)? {
        Some(branch) => Ok(branch),
        None => Ok(trimmed(repo.ctx.git(&["rev-parse", HEAD])?)),
    }
}

// Without asking, like `GitDeleteBranchOperation`: what gets lost comes back with `Restore`.
pub fn delete(repo: &Repo, name: &str) -> Result<Deleted, GitError> {
    check_branch(repo.ctx, name)?;
    let tip = commit_of(repo, &format!("{HEADS}{name}"))?;
    let upstream = upstream_of(repo, name)?;
    let (forced, base) = match repo.ctx.git_loud(&["branch", "-d", name]) {
        Ok(_) => (false, None),
        Err(error) if stderr_of(&error).contains(NOT_FULLY_MERGED) => {
            let base = match unmerged_base(stderr_of(&error)) {
                Some(base) => base.to_string(),
                None => head_name(repo)?,
            };
            repo.ctx.git_loud(&["branch", "-D", name])?;
            (true, Some(base))
        }
        Err(error) => return Err(error),
    };
    Ok(Deleted {
        name: name.to_string(),
        tip,
        upstream,
        forced,
        base,
    })
}

// The tracking may not come back, if its remote branch is gone: that does not remove the branch.
pub fn restore(repo: &Repo, deleted: &Deleted) -> Result<(), GitError> {
    check_branch(repo.ctx, &deleted.name)?;
    check_revision(&deleted.tip)?;
    repo.ctx
        .git_loud(&["branch", &deleted.name, &deleted.tip])?;
    if let Some(upstream) = &deleted.upstream {
        let flag = format!("--set-upstream-to={upstream}");
        match repo.ctx.git_loud(&["branch", &flag, &deleted.name]) {
            Ok(_) | Err(GitError::Failed { .. }) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

// The local branches that track that remote branch, except the current one: it cannot be deleted.
pub fn trackers(path: &Path, remote_branch: &str) -> Result<Vec<String>, GitError> {
    let format = "--format=%(HEAD)%00%(refname:strip=2)%00%(upstream)";
    let output = git::run(path, &["for-each-ref", format, "refs/heads"])?;
    let upstream = format!("refs/remotes/{remote_branch}");
    let text = String::from_utf8_lossy(&output);
    Ok(text
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\0');
            let (head, name, tracked) = (fields.next()?, fields.next()?, fields.next()?);
            (head != "*" && tracked == upstream).then(|| name.to_string())
        })
        .collect())
}

pub fn delete_remote(
    repo: &Repo,
    remote: &str,
    branch: &str,
    also_local: bool,
) -> Result<DeletedRemote, GitError> {
    check_argument(remote, "The remote")?;
    check_branch(repo.ctx, branch)?;
    let protected = settings::try_effective_protected(repo.data)
        .map_err(|error| GitError::Io(error.to_string()))?;
    if push::is_protected(&protected, branch) {
        return Err(GitError::Io(format!("Branch {branch} is protected")));
    }
    // With the full name: `:x` would delete a tag `x` and count it as a branch. If the branch is
    // already gone, git finishes fine and still removes the remote reference that was left.
    let refspec = format!(":{HEADS}{branch}");
    let args = ["push", "--progress", "--porcelain", remote, &refspec];
    repo.ctx.git_loud(&args)?;
    let mut outcome = DeletedRemote {
        local: Vec::new(),
        failures: Vec::new(),
    };
    if also_local {
        // The task runs as `TaskKind::Remote`, which does not hold the repository's write lock:
        // the local deletions take it, as `branch_delete` (`TaskKind::Write`) does.
        let tracking = format!("{remote}/{branch}");
        let deleted = repo
            .ctx
            .exclusive(|| -> Result<Vec<Result<Deleted, GitError>>, GitError> {
                let names = trackers(repo.path, &tracking)?;
                Ok(names.iter().map(|name| delete(repo, name)).collect())
            })
            .unwrap_or(Err(GitError::Cancelled))?;
        for result in deleted {
            match result {
                Ok(deleted) => outcome.local.push(deleted),
                Err(error) => outcome.failures.push(text_of(error)),
            }
        }
    }
    Ok(outcome)
}

fn run_reset(repo: &Repo, commit: &str, mode: ResetMode) -> Result<CheckoutOutcome, GitError> {
    let flag = match mode {
        ResetMode::Soft => "--soft",
        ResetMode::Mixed => "--mixed",
        ResetMode::Hard => "--hard",
        ResetMode::Keep => "--keep",
    };
    match mode {
        ResetMode::Soft => {}
        ResetMode::Hard => copy_before(repo, RESET_HARD, |path| snapshots::at_risk(path, commit))?,
        ResetMode::Keep => copy_before_keep(repo, commit)?,
        // The disk is not touched: what cannot be copied from it is no reason to stop.
        ResetMode::Mixed => {
            let paths = snapshots::index_only(repo.path)?;
            if !paths.is_empty() {
                snapshots::create_for_index(repo.data, repo.path, &paths, RESET_MIXED)
                    .map_err(copy_failed)?;
            }
        }
    }
    or_overwritten(repo.ctx.git_loud(&["reset", flag, commit, "--"]))
}

// What `reset --keep` to `commit` would overwrite: what is only in the stage and the unversioned
// that occupies the place of something in the target.
fn copy_before_keep(repo: &Repo, commit: &str) -> Result<(), GitError> {
    copy_before(repo, RESET_KEEP, |path| {
        let mut paths = snapshots::index_only(path)?;
        paths.extend(snapshots::in_the_way(path, commit)?);
        Ok(paths)
    })
}

// `reset --keep` with its copy, for whoever does not go through `Git Reset Problem` (the log's Undo).
pub(crate) fn reset_keep(repo: &Repo, commit: &str) -> Result<(), GitError> {
    copy_before_keep(repo, commit)?;
    repo.ctx
        .git_loud(&["reset", "--keep", commit, "--"])
        .map(drop)
}

// `how` is the answer to `Git Reset Problem`: `Smart` saves and repeats, `Force` is `Hard Reset`.
pub fn reset(
    repo: &Repo,
    target: &str,
    mode: ResetMode,
    how: CheckoutMode,
) -> Result<CheckoutOutcome, GitError> {
    check_revision(target)?;
    let commit = commit_of(repo, target)?;
    match how {
        CheckoutMode::Plain => run_reset(repo, &commit, mode),
        CheckoutMode::Smart => smart(repo, RESET, || run_reset(repo, &commit, mode)),
        CheckoutMode::Force => run_reset(repo, &commit, ResetMode::Hard),
    }
}

// With a message it is annotated. It goes with `-m` and not through a file: the message is the
// option's value, however it starts.
pub fn tag(
    repo: &Repo,
    name: &str,
    target: &str,
    message: &str,
    force: bool,
) -> Result<(), GitError> {
    check_tag(repo.ctx, name)?;
    check_revision(target)?;
    let mut args = vec!["tag"];
    if !message.is_empty() {
        args.extend(["-a", "-m", message]);
    }
    if force {
        args.push("-f");
    }
    args.extend([name, target]);
    repo.ctx.git_loud(&args).map(drop)
}

// The hash is the tag object's, not peeled: the commit's would lose the author and message of an annotated one.
pub fn delete_tag(repo: &Repo, name: &str) -> Result<DeletedTag, GitError> {
    check_tag(repo.ctx, name)?;
    let full = format!("{TAGS}{name}");
    let tip = trimmed(repo.ctx.git(&["rev-parse", "--verify", &full])?);
    repo.ctx.git_loud(&["tag", "-d", name])?;
    Ok(DeletedTag {
        name: name.to_string(),
        tip,
    })
}

pub fn restore_tag(repo: &Repo, deleted: &DeletedTag) -> Result<(), GitError> {
    check_tag(repo.ctx, &deleted.name)?;
    check_revision(&deleted.tip)?;
    repo.ctx
        .git_loud(&["tag", &deleted.name, &deleted.tip])
        .map(drop)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LostCommit {
    pub hash: String,
    pub subject: String,
    pub author: String,
    pub time: i64,
}

// The commits that `Branch Was Not Fully Merged` shows.
pub fn unmerged_commits(path: &Path, base: &str, tip: &str) -> Result<Vec<LostCommit>, GitError> {
    check_revision(base)?;
    check_revision(tip)?;
    let range = format!("{base}..{tip}");
    let output = git::run(
        path,
        &[
            "-c",
            "log.showSignature=false",
            "log",
            COMMIT_FORMAT,
            &range,
            "--",
        ],
    )?;
    Ok(commits_in(&String::from_utf8_lossy(&output)))
}

// What `git log` writes with `COMMIT_FORMAT`.
pub(crate) fn commits_in(text: &str) -> Vec<LostCommit> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.splitn(4, '\0');
            Some(LostCommit {
                hash: fields.next()?.to_string(),
                author: fields.next()?.to_string(),
                time: fields.next()?.parse().ok()?,
                subject: fields.next()?.to_string(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{ApiError, RepoLocks};
    use crate::console::Console;
    use crate::settings::CleanTree;
    use crate::snapshots;
    use crate::task::Runner;
    use crate::test_support::{in_task, TempRepo};
    use std::fs;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;
    use std::time::Instant;
    use tempfile::TempDir;

    struct Harness {
        repo: TempRepo,
        data: TempDir,
        console: Arc<Console>,
        runner: Runner,
        locks: Arc<RepoLocks>,
    }

    impl Harness {
        fn new() -> Self {
            Self::around(TempRepo::new())
        }

        fn around(repo: TempRepo) -> Self {
            let console = Arc::<Console>::default();
            let locks = Arc::<RepoLocks>::default();
            let runner = Runner::with(locks.clone(), console.clone(), None, Duration::from_secs(5));
            let data = TempDir::new().unwrap();
            // The tests written before the shelf saver speak of the stash.
            crate::settings::save_clean_tree(data.path(), CleanTree::Stash).unwrap();
            Self {
                repo,
                data,
                console,
                runner,
                locks,
            }
        }

        // As `branch_delete_remote` runs it: a task that talks to `origin`, outside the write lock.
        fn run_remote<T: Send + 'static>(
            &self,
            work: impl FnOnce(&Repo) -> T + Send + 'static,
        ) -> std::sync::mpsc::Receiver<T> {
            let path = self.repo.path().to_path_buf();
            let data = self.data.path().to_path_buf();
            let (send, receive) = std::sync::mpsc::channel();
            let kind = crate::task::TaskKind::Remote("origin".to_string());
            self.runner
                .start(self.repo.path(), kind, "test".to_string(), move |ctx| {
                    let result = work(&Repo {
                        ctx,
                        path: &path,
                        data: &data,
                    });
                    send.send(result).unwrap();
                    Ok(())
                });
            receive
        }

        fn run<T: Send + 'static>(&self, work: impl FnOnce(&Repo) -> T + Send + 'static) -> T {
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

        fn write(&self, file: &str, contents: &str) {
            let target = self.repo.path().join(file);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(target, contents).unwrap();
        }

        fn read(&self, file: &str) -> String {
            fs::read_to_string(self.repo.path().join(file)).unwrap()
        }

        fn git(&self, args: &[&str]) -> String {
            self.repo.git(args)
        }

        fn current(&self) -> String {
            self.git(&["symbolic-ref", "--short", "HEAD"])
                .trim()
                .to_string()
        }

        fn head(&self) -> String {
            self.git(&["rev-parse", "HEAD"]).trim().to_string()
        }

        fn commands(&self) -> Vec<String> {
            self.console
                .lines()
                .into_iter()
                .filter(|line| line.command)
                .map(|line| line.text)
                .collect()
        }

        fn ran(&self, command: &str) -> bool {
            self.commands().iter().any(|line| line.ends_with(command))
        }

        fn snapshot(&self) -> snapshots::Snapshot {
            snapshots::list(self.data.path()).remove(0)
        }

        fn saved_paths(&self) -> Vec<String> {
            let entries = self.snapshot().entries;
            entries.into_iter().map(|entry| entry.path).collect()
        }

        fn snapshots(&self) -> usize {
            snapshots::list(self.data.path()).len()
        }

        fn restore_snapshot(&self) {
            let id = self.snapshot().id;
            snapshots::restore(self.data.path(), self.repo.path(), &id).unwrap();
        }
    }

    // `main` with two files and `other` with a different `a.txt` and a `new.txt`.
    fn two_branches() -> Harness {
        let harness = Harness::new();
        harness.repo.commit("a.txt", b"base\n", "one");
        harness.write("b.txt", "b\n");
        harness.git(&["add", "-A"]);
        harness.git(&["commit", "-q", "-m", "two"]);
        harness.git(&["checkout", "-q", "-b", "other"]);
        harness.write("new.txt", "new\n");
        harness.repo.commit("a.txt", b"other\n", "three");
        harness.git(&["checkout", "-q", "main"]);
        harness
    }

    // A clone of a bare remote with `main` and `origin/x` one commit ahead.
    fn cloned() -> (Harness, TempDir) {
        let bare = TempDir::new().unwrap();
        let status = crate::test_support::git_command()
            .args(["init", "-q", "--bare", "-b", "main"])
            .arg(bare.path())
            .status()
            .unwrap();
        assert!(status.success());
        let harness = Harness::new();
        harness.repo.commit("a.txt", b"1\n", "one");
        let url = bare.path().to_str().unwrap();
        harness.git(&["remote", "add", "origin", url]);
        harness.git(&["push", "-q", "-u", "origin", "main"]);
        harness.git(&["checkout", "-q", "-b", "x"]);
        harness.repo.commit("x.txt", b"x\n", "of x");
        harness.git(&["push", "-q", "origin", "x"]);
        harness.git(&["checkout", "-q", "main"]);
        harness.git(&["branch", "-q", "-D", "x"]);
        (harness, bare)
    }

    fn upstream(harness: &Harness, branch: &str) -> String {
        let spec = format!("refs/heads/{branch}");
        harness
            .git(&["for-each-ref", "--format=%(upstream:short)", &spec])
            .trim()
            .to_string()
    }

    fn message(error: GitError) -> String {
        ApiError::from(error).message
    }

    #[test]
    fn checkout_local() {
        let harness = two_branches();

        let outcome = harness.run(|repo| checkout(repo, "other", CheckoutMode::Plain));

        assert_eq!(outcome, Ok(CheckoutOutcome::Done));
        assert_eq!(harness.current(), "other");
        assert!(harness.ran("checkout other --"), "{:?}", harness.commands());
    }

    // Two branches that collide on `a.txt`, with the merge stopped, resolved and staged.
    fn stopped_merge() -> Harness {
        let harness = two_branches();
        harness.repo.commit("a.txt", b"main\n", "four");
        let merged = crate::test_support::git_command()
            .current_dir(harness.repo.path())
            .args(["merge", "other"])
            .output()
            .unwrap();
        assert!(!merged.status.success());
        harness.write("a.txt", "resolved\n");
        harness.git(&["add", "a.txt"]);
        harness
    }

    fn stopped_rebase() -> Harness {
        let harness = two_branches();
        harness.repo.commit("a.txt", b"main\n", "four");
        let rebased = crate::test_support::git_command()
            .current_dir(harness.repo.path())
            .args(["rebase", "other"])
            .output()
            .unwrap();
        assert!(!rebased.status.success());
        harness.write("a.txt", "resolved\n");
        harness.git(&["add", "a.txt"]);
        harness
    }

    fn assert_refused_before_stashing(
        harness: &Harness,
        outcome: Result<CheckoutOutcome, GitError>,
    ) {
        let text = message(outcome.unwrap_err());
        assert!(text.contains("Finish or abort"), "{text}");
        assert_eq!(harness.git(&["stash", "list"]), "");
        assert!(!harness.ran("stash save"));
        assert_eq!(harness.read("a.txt"), "resolved\n");
        assert!(harness.git(&["status", "--porcelain"]).contains("a.txt"));
    }

    // `stash save` deletes `MERGE_HEAD` without saying anything: the resolved merge would become a normal commit.
    #[test]
    fn smart_checkout_with_a_stopped_merge_refuses_before_saving() {
        let harness = stopped_merge();

        let outcome = harness.run(|repo| checkout(repo, "main", CheckoutMode::Smart));

        assert_refused_before_stashing(&harness, outcome);
        assert!(harness.repo.path().join(".git/MERGE_HEAD").exists());
    }

    #[test]
    fn smart_reset_with_a_stopped_merge_refuses_before_saving() {
        let harness = stopped_merge();

        let outcome =
            harness.run(|repo| reset(repo, "HEAD~1", ResetMode::Keep, CheckoutMode::Smart));

        assert_refused_before_stashing(&harness, outcome);
        assert!(harness.repo.path().join(".git/MERGE_HEAD").exists());
    }

    // `pop` would leave what was resolved out of the stage, and `--continue` would fail.
    #[test]
    fn smart_checkout_with_a_stopped_rebase_refuses_before_saving() {
        let harness = stopped_rebase();

        let outcome = harness.run(|repo| checkout(repo, "other", CheckoutMode::Smart));

        assert_refused_before_stashing(&harness, outcome);
        assert!(harness.repo.path().join(".git/rebase-merge").exists());
    }

    #[test]
    fn smart_reset_with_a_stopped_rebase_refuses_before_saving() {
        let harness = stopped_rebase();

        let outcome =
            harness.run(|repo| reset(repo, "HEAD~1", ResetMode::Keep, CheckoutMode::Smart));

        assert_refused_before_stashing(&harness, outcome);
        assert!(harness.repo.path().join(".git/rebase-merge").exists());
    }

    #[test]
    fn the_entry_someone_else_took_is_not_counted_as_saved() {
        let harness = two_branches();
        harness.write("a.txt", "local\n");

        let outcome = harness.run(|repo| {
            smart(repo, "Checkout", || {
                repo.ctx.git(&["stash", "drop", "-q"])?;
                Ok(CheckoutOutcome::Done)
            })
        });

        assert!(
            matches!(outcome, Ok(CheckoutOutcome::Gone { failure: None, .. })),
            "{outcome:?}"
        );
    }

    #[test]
    // Decision 10: this build's stash leaves no record; one that an earlier build left is still
    // found while its entry is in the stash.
    fn smart_checkout_records_nothing_and_an_earlier_record_is_still_found() {
        let harness = two_branches();
        harness.write("a.txt", "local\n");

        let outcome = harness.run(|repo| checkout(repo, "other", CheckoutMode::Smart));

        let Ok(CheckoutOutcome::Conflicts { stash, .. }) = outcome else {
            panic!("{outcome:?}");
        };
        let (repo, data) = (harness.repo.path(), harness.data.path());
        assert_eq!(preserve::pending(repo, data), Ok(Vec::new()));
        preserve::record_earlier_build(data, &stash.sha, "Checkout", &stash.message);
        let pending = preserve::pending(repo, data).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].title, "Checkout");
        assert_eq!(pending[0].sha, stash.sha);
    }

    #[test]
    fn smart_checkout_with_shelve() {
        let harness = two_branches();
        crate::settings::save_clean_tree(harness.data.path(), CleanTree::Shelve).unwrap();
        harness.write("b.txt", "local\n");
        harness.write("a.txt", "local\n");

        let outcome = harness.run(|repo| checkout(repo, "other", CheckoutMode::Smart));

        // `a.txt` clashes with `other`: it stays in the shelf, untouched on disk.
        let Ok(CheckoutOutcome::Conflicts {
            stash,
            failure: None,
        }) = outcome
        else {
            panic!("{outcome:?}");
        };
        assert_eq!(stash.kind, CleanTree::Shelve);
        assert!(stash
            .message
            .starts_with("Uncommitted changes before Checkout at "));
        assert_eq!(harness.git(&["branch", "--show-current"]).trim(), "other");
        assert_eq!(harness.read("a.txt"), "other\n");
        assert_eq!(harness.read("b.txt"), "local\n");
        assert_eq!(harness.git(&["stash", "list"]), "");
        let kept: Vec<_> = crate::shelf::list(harness.data.path())
            .into_iter()
            .filter(|list| !list.deleted)
            .collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].name, stash.lists[0].name);
        assert_eq!(
            preserve::pending(harness.repo.path(), harness.data.path()),
            Ok(Vec::new())
        );

        let clean = two_branches();
        crate::settings::save_clean_tree(clean.data.path(), CleanTree::Shelve).unwrap();
        clean.write("b.txt", "local\n");
        let outcome = clean.run(|repo| checkout(repo, "other", CheckoutMode::Smart));
        assert_eq!(outcome, Ok(CheckoutOutcome::Done));
        assert_eq!(clean.read("b.txt"), "local\n");
    }

    #[test]
    fn smart_checkout_that_comes_back_records_nothing() {
        let harness = two_branches();
        harness.write("b.txt", "local\n");

        let outcome = harness.run(|repo| checkout(repo, "other", CheckoutMode::Smart));

        assert_eq!(outcome, Ok(CheckoutOutcome::Done));
        let pending = preserve::pending(harness.repo.path(), harness.data.path()).unwrap();
        assert_eq!(pending, []);
    }

    #[test]
    fn checkout_with_changes_it_overwrites_returns_files() {
        let harness = two_branches();
        harness.write("a.txt", "local\n");

        let outcome = harness.run(|repo| checkout(repo, "other", CheckoutMode::Plain));

        assert_eq!(
            outcome,
            Ok(CheckoutOutcome::Overwritten {
                files: vec!["a.txt".to_string()]
            })
        );
        assert_eq!(harness.current(), "main");
        assert_eq!(harness.read("a.txt"), "local\n");
        assert_eq!(harness.git(&["stash", "list"]), "");
        assert_eq!(harness.snapshots(), 0);
    }

    #[test]
    fn plain_checkout_copies_the_ignored_files_it_overwrites() {
        let harness = two_branches();
        harness.git(&["checkout", "-q", "other"]);
        harness.git(&["rm", "-q", "new.txt"]);
        harness.write(".gitignore", "new.txt\n");
        harness.git(&["add", ".gitignore"]);
        harness.git(&["commit", "-q", "-m", "ignore"]);
        harness.git(&["branch", "-q", "with-new", "other~1"]);
        harness.write("new.txt", "generated\n");

        let outcome = harness.run(|repo| checkout(repo, "with-new", CheckoutMode::Plain));

        assert_eq!(outcome, Ok(CheckoutOutcome::Done));
        assert_eq!(harness.read("new.txt"), "new\n");
        assert_eq!(harness.saved_paths(), ["new.txt"]);
    }

    #[test]
    fn smart_checkout_carries_the_changes() {
        let harness = two_branches();
        // The local change and the target's are on different lines: git merges them.
        harness.git(&["checkout", "-q", "-b", "long", "main"]);
        harness
            .repo
            .commit("a.txt", b"1\n2\n3\n4\n5\n6\n7\n8\n", "long");
        harness.git(&["checkout", "-q", "-b", "target"]);
        harness
            .repo
            .commit("a.txt", b"1\n2\n3\n4\n5\n6\n7\ntarget\n", "target");
        harness.git(&["checkout", "-q", "long"]);
        harness.write("a.txt", "local\n2\n3\n4\n5\n6\n7\n8\n");

        let outcome = harness.run(|repo| checkout(repo, "target", CheckoutMode::Smart));

        assert_eq!(outcome, Ok(CheckoutOutcome::Done));
        assert_eq!(harness.current(), "target");
        assert_eq!(harness.read("a.txt"), "local\n2\n3\n4\n5\n6\n7\ntarget\n");
        assert_eq!(harness.git(&["stash", "list"]), "");
    }

    #[test]
    fn smart_checkout_with_conflict_leaves_the_stash() {
        let harness = two_branches();
        harness.write("a.txt", "local\n");

        let outcome = harness.run(|repo| checkout(repo, "other", CheckoutMode::Smart));

        let Ok(CheckoutOutcome::Conflicts { stash, .. }) = outcome else {
            panic!("{outcome:?}");
        };
        assert_eq!(harness.current(), "other");
        assert_eq!(
            harness.git(&["stash", "list", "--format=%H"]).trim(),
            stash.sha
        );
        assert!(harness.read("a.txt").contains("local"));
    }

    // The stash does not carry what is unversioned: `git checkout` refuses and the changes must come back.
    #[test]
    fn smart_checkout_that_fails_returns_the_changes() {
        let harness = two_branches();
        harness.write("a.txt", "local\n");
        harness.write("new.txt", "unversioned\n");

        let outcome = harness.run(|repo| checkout(repo, "other", CheckoutMode::Smart));

        assert!(outcome.is_err(), "{outcome:?}");
        assert_eq!(harness.current(), "main");
        assert_eq!(harness.read("a.txt"), "local\n");
        assert_eq!(harness.read("new.txt"), "unversioned\n");
        assert_eq!(harness.git(&["stash", "list"]), "");
    }

    #[test]
    fn force_checkout_creates_a_copy_first() {
        let harness = two_branches();
        harness.write("a.txt", "in stage\n");
        harness.git(&["add", "a.txt"]);
        harness.write("a.txt", "on disk\n");
        harness.write("b.txt", "local\n");
        harness.write("new.txt", "unversioned\n");

        let outcome = harness.run(|repo| checkout(repo, "other", CheckoutMode::Force));

        assert_eq!(outcome, Ok(CheckoutOutcome::Done));
        assert_eq!(harness.current(), "other");
        assert!(
            harness.ran("checkout --force other --"),
            "{:?}",
            harness.commands()
        );
        assert_eq!(harness.read("new.txt"), "new\n");
        assert_eq!(harness.saved_paths(), ["a.txt", "b.txt", "new.txt"]);

        harness.git(&["checkout", "-q", "--force", "main"]);
        harness.restore_snapshot();
        assert_eq!(harness.read("a.txt"), "on disk\n");
        assert_eq!(harness.read("b.txt"), "local\n");
        assert_eq!(harness.read("new.txt"), "unversioned\n");
        assert_eq!(harness.git(&["show", ":a.txt"]), "in stage\n");
    }

    // Git picks the branch to switch to it and `rev-parse` picks the tag: the copy would look at another tree.
    #[test]
    fn force_checkout_with_a_tag_and_branch_of_the_same_name_copies_the_branch_files() {
        let harness = two_branches();
        harness.git(&["tag", "other", "main"]);
        harness.write("new.txt", "unversioned\n");

        let outcome = harness.run(|repo| checkout(repo, "other", CheckoutMode::Force));

        assert_eq!(outcome, Ok(CheckoutOutcome::Done));
        assert_eq!(harness.read("new.txt"), "new\n");
        assert_eq!(harness.saved_paths(), ["new.txt"]);
    }

    // `X^0` is not a branch: there the tag does win, and the tree is its own.
    #[test]
    fn checkout_of_a_revision_is_not_taken_for_a_branch() {
        let harness = two_branches();
        harness.git(&["tag", "other", "main"]);
        harness.write("new.txt", "unversioned\n");

        let outcome = harness.run(|repo| checkout(repo, "other^0", CheckoutMode::Force));

        assert_eq!(outcome, Ok(CheckoutOutcome::Done));
        assert_eq!(harness.read("new.txt"), "unversioned\n");
        assert_eq!(harness.snapshots(), 0);
    }

    fn with_staged_only() -> Harness {
        let harness = Harness::new();
        harness.repo.commit("a.txt", b"1\n2\n3\n4\n5\n", "one");
        harness.git(&["branch", "-q", "old"]);
        harness.git(&["checkout", "-q", "-b", "target"]);
        harness.repo.commit("c.txt", b"c\n", "two");
        harness
            .repo
            .commit("a.txt", b"1\n2\n3\n4\ntarget\n", "three");
        harness.git(&["checkout", "-q", "main"]);
        harness.write("a.txt", "stage\n2\n3\n4\n5\n");
        harness.git(&["add", "a.txt"]);
        harness.write("a.txt", "disk\n2\n3\n4\n5\n");
        harness
    }

    #[test]
    fn smart_without_staging_area_copies_what_is_only_in_the_stage() {
        let harness = with_staged_only();

        let outcome = harness.run(|repo| checkout(repo, "target", CheckoutMode::Smart));

        assert_eq!(outcome, Ok(CheckoutOutcome::Done));
        assert_eq!(harness.read("a.txt"), "disk\n2\n3\n4\ntarget\n");
        assert_eq!(harness.saved_paths(), ["a.txt"]);
        let id = harness.snapshot().id;
        let blob = snapshots::stored(harness.data.path(), &id, "a.txt", true).unwrap();
        assert_eq!(fs::read_to_string(blob).unwrap(), "stage\n2\n3\n4\n5\n");
    }

    #[test]
    fn smart_reset_without_staging_area_copies_what_is_only_in_the_stage() {
        let harness = with_staged_only();
        harness.git(&["reset", "-q", "--hard", "target"]);
        harness.write("c.txt", "stage\n");
        harness.git(&["add", "c.txt"]);
        harness.write("c.txt", "disk\n");

        let outcome =
            harness.run(|repo| reset(repo, "HEAD~1", ResetMode::Keep, CheckoutMode::Smart));

        assert_eq!(outcome, Ok(CheckoutOutcome::Done));
        assert_eq!(harness.saved_paths(), ["c.txt"]);
    }

    #[test]
    fn smart_with_staging_area_needs_no_copy() {
        let harness = with_staged_only();
        harness.runner.set_staging_area(true);

        let outcome = harness.run(|repo| checkout(repo, "target", CheckoutMode::Smart));

        assert_eq!(outcome, Ok(CheckoutOutcome::Done));
        assert_eq!(harness.snapshots(), 0);
        assert_eq!(
            harness.git(&["show", ":a.txt"]),
            "stage\n2\n3\n4\ntarget\n"
        );
    }

    // The changes came back with conflicts: saying they have not come back would be false, and the failure counts.
    #[test]
    fn failed_operation_with_conflicts_says_both_things() {
        let harness = two_branches();
        harness.write("a.txt", "local\n");
        let path = harness.repo.path().to_path_buf();

        let outcome = harness.run(move |repo| {
            smart(repo, "Checkout", || {
                fs::write(path.join("a.txt"), "another\n").unwrap();
                let commit = ["commit", "-q", "-a", "-m", "another"];
                assert!(crate::test_support::git_command()
                    .args(commit)
                    .current_dir(&path)
                    .status()
                    .unwrap()
                    .success());
                Err(GitError::Io("it stopped".to_string()))
            })
        });

        let Ok(CheckoutOutcome::Conflicts { failure, .. }) = outcome else {
            panic!("{outcome:?}");
        };
        assert_eq!(failure.as_deref(), Some("it stopped"));
    }

    #[test]
    fn cancelling_with_conflicts_does_not_count_as_a_failure() {
        let harness = two_branches();
        harness.write("a.txt", "local\n");
        let path = harness.repo.path().to_path_buf();

        let outcome = harness.run(move |repo| {
            smart(repo, "Checkout", || {
                fs::write(path.join("a.txt"), "another\n").unwrap();
                let commit = ["commit", "-q", "-a", "-m", "another"];
                assert!(crate::test_support::git_command()
                    .args(commit)
                    .current_dir(&path)
                    .status()
                    .unwrap()
                    .success());
                Err(GitError::Cancelled)
            })
        });

        assert!(
            matches!(
                outcome,
                Ok(CheckoutOutcome::Conflicts { failure: None, .. })
            ),
            "{outcome:?}"
        );
    }

    #[test]
    fn checkout_remote_creates_a_local_with_tracking() {
        let (harness, _bare) = cloned();
        harness.git(&["fetch", "-q", "origin"]);

        let outcome =
            harness.run(|repo| checkout_remote(repo, "origin/x", RemoteOptions::default()));

        assert_eq!(outcome, Ok(RemoteCheckout::Done));
        assert_eq!(harness.current(), "x");
        assert_eq!(upstream(&harness, "x"), "origin/x");
        assert!(
            harness.ran("checkout -b x origin/x --"),
            "{:?}",
            harness.commands()
        );
    }

    #[test]
    fn checkout_remote_with_a_suggested_name_offers_it_with_the_same_questions() {
        let (harness, _bare) = cloned();
        harness.git(&["fetch", "-q", "origin"]);
        let suggested = |name: &'static str| {
            harness.run(move |repo| {
                let options = RemoteOptions {
                    suggested: Some(name),
                    ..RemoteOptions::default()
                };
                checkout_remote(repo, "origin/x", options)
            })
        };

        // The remote branch's own name, ignoring case, is asked for.
        assert_eq!(
            suggested("Origin/X"),
            Ok(RemoteCheckout::NeedsName {
                suggested: "Origin/X".to_string()
            })
        );
        assert_eq!(suggested("fork/x"), Ok(RemoteCheckout::Done));
        assert_eq!(harness.current(), "fork/x");
        assert!(harness.ran("checkout -b fork/x origin/x --"));

        harness.git(&["checkout", "-q", "main"]);
        harness.git(&["branch", "-q", "--track", "taken", "origin/main"]);
        assert_eq!(
            suggested("taken"),
            Ok(RemoteCheckout::NeedsName {
                suggested: "taken".to_string()
            })
        );
        assert_eq!(harness.current(), "main");
    }

    #[test]
    fn checkout_remote_with_local_without_own_commits_moves_the_branch() {
        let (harness, _bare) = cloned();
        harness.git(&["fetch", "-q", "origin"]);
        harness.git(&["branch", "-q", "--no-track", "x", "origin/x~1"]);

        let outcome =
            harness.run(|repo| checkout_remote(repo, "origin/x", RemoteOptions::default()));

        assert_eq!(outcome, Ok(RemoteCheckout::Done));
        assert_eq!(harness.current(), "x");
        assert_eq!(
            harness.head(),
            harness.git(&["rev-parse", "origin/x"]).trim()
        );
        assert!(
            harness.ran("checkout -B x origin/x --"),
            "{:?}",
            harness.commands()
        );
    }

    #[test]
    fn checkout_remote_with_own_commits_asks() {
        let (harness, _bare) = cloned();
        harness.git(&["fetch", "-q", "origin"]);
        harness.git(&["checkout", "-q", "--no-track", "-b", "x", "origin/x~1"]);
        let own = harness.repo.commit("own.txt", b"p\n", "own");
        harness.git(&["checkout", "-q", "main"]);

        let outcome =
            harness.run(|repo| checkout_remote(repo, "origin/x", RemoteOptions::default()));

        assert_eq!(
            outcome,
            Ok(RemoteCheckout::LocalHasCommits {
                local: "x".to_string()
            })
        );
        assert_eq!(harness.current(), "main");
        assert_eq!(harness.git(&["rev-parse", "x"]).trim(), own);

        let dropped = harness.run(|repo| {
            let options = RemoteOptions {
                drop_commits: true,
                ..RemoteOptions::default()
            };
            checkout_remote(repo, "origin/x", options)
        });
        assert_eq!(dropped, Ok(RemoteCheckout::Done));
        assert_eq!(
            harness.head(),
            harness.git(&["rev-parse", "origin/x"]).trim()
        );
    }

    #[test]
    fn checkout_remote_with_local_tracking_another_asks_for_a_name() {
        let (harness, _bare) = cloned();
        harness.git(&["fetch", "-q", "origin"]);
        harness.git(&["branch", "-q", "--track", "x", "origin/main"]);

        let outcome =
            harness.run(|repo| checkout_remote(repo, "origin/x", RemoteOptions::default()));

        assert_eq!(
            outcome,
            Ok(RemoteCheckout::NeedsName {
                suggested: "x".to_string()
            })
        );
        assert_eq!(harness.current(), "main");

        let named = harness.run(|repo| {
            let options = RemoteOptions {
                name: Some("mine"),
                ..RemoteOptions::default()
            };
            checkout_remote(repo, "origin/x", options)
        });
        assert_eq!(named, Ok(RemoteCheckout::Done));
        assert_eq!(harness.current(), "mine");
    }

    #[test]
    fn checkout_remote_with_changes_it_overwrites_says_so() {
        let (harness, _bare) = cloned();
        harness.git(&["fetch", "-q", "origin"]);
        harness.write("x.txt", "unversioned\n");
        harness.git(&["add", "x.txt"]);

        let outcome =
            harness.run(|repo| checkout_remote(repo, "origin/x", RemoteOptions::default()));

        assert_eq!(
            outcome,
            Ok(RemoteCheckout::Overwritten {
                files: vec!["x.txt".to_string()]
            })
        );
        assert_eq!(harness.current(), "main");
    }

    #[test]
    fn new_branch_from_another_does_not_set_upstream() {
        let (harness, _bare) = cloned();
        harness.git(&["fetch", "-q", "origin"]);

        let outcome = harness.run(|repo| {
            create(
                repo,
                "n",
                Some("origin/x"),
                true,
                Overwrite::No,
                CheckoutMode::Plain,
            )
        });

        assert_eq!(outcome, Ok(RemoteCheckout::Done));
        assert_eq!(harness.current(), "n");
        assert_eq!(upstream(&harness, "n"), "");
        assert!(
            harness.ran("checkout -b n origin/x^0 --"),
            "{:?}",
            harness.commands()
        );

        let quiet = harness.run(|repo| {
            create(
                repo,
                "m",
                Some("origin/x"),
                false,
                Overwrite::No,
                CheckoutMode::Plain,
            )
        });
        assert_eq!(quiet, Ok(RemoteCheckout::Done));
        assert_eq!(harness.current(), "n");
        assert_eq!(upstream(&harness, "m"), "");
    }

    #[test]
    fn new_branch_without_start_comes_from_head() {
        let harness = two_branches();

        let outcome =
            harness.run(|repo| create(repo, "n", None, true, Overwrite::No, CheckoutMode::Plain));

        assert_eq!(outcome, Ok(RemoteCheckout::Done));
        assert!(
            harness.ran("checkout -b n HEAD --"),
            "{:?}",
            harness.commands()
        );
    }

    #[test]
    fn new_branch_that_already_exists_is_not_overwritten_unasked() {
        let harness = two_branches();
        let before = harness.git(&["rev-parse", "other"]);

        let outcome = harness.run(|repo| {
            create(
                repo,
                "other",
                None,
                false,
                Overwrite::No,
                CheckoutMode::Plain,
            )
        });

        assert!(outcome.is_err());
        assert_eq!(harness.git(&["rev-parse", "other"]), before);
    }

    #[test]
    fn overwrite_with_own_commits_asks_and_drop_discards_them() {
        let harness = two_branches();
        let main = harness.git(&["rev-parse", "main"]);

        let asked = harness.run(|repo| {
            create(
                repo,
                "other",
                None,
                false,
                Overwrite::Try,
                CheckoutMode::Plain,
            )
        });
        assert_eq!(
            asked,
            Ok(RemoteCheckout::LocalHasCommits {
                local: "other".to_string()
            })
        );
        assert_ne!(harness.git(&["rev-parse", "other"]), main);

        let dropped = harness.run(|repo| {
            create(
                repo,
                "other",
                None,
                false,
                Overwrite::Drop,
                CheckoutMode::Plain,
            )
        });
        assert_eq!(dropped, Ok(RemoteCheckout::Done));
        assert_eq!(harness.git(&["rev-parse", "other"]), main);
        assert!(
            harness.ran("branch -f other HEAD"),
            "{:?}",
            harness.commands()
        );
    }

    // `git branch -f` refuses with the current branch: `GitBranchCheckoutOperation.create` uses `-B`.
    #[test]
    fn overwrite_of_the_current_branch_without_checkout_uses_checkout() {
        let harness = two_branches();

        let outcome = harness.run(|repo| {
            create(
                repo,
                "main",
                Some("other"),
                false,
                Overwrite::Try,
                CheckoutMode::Plain,
            )
        });

        assert_eq!(outcome, Ok(RemoteCheckout::Done));
        assert_eq!(harness.current(), "main");
        assert_eq!(harness.head(), harness.git(&["rev-parse", "other"]).trim());
    }

    #[test]
    fn rename_changes_the_name_and_does_not_overwrite_another() {
        let harness = two_branches();
        harness.git(&["branch", "-q", "third"]);

        assert_eq!(harness.run(|repo| rename(repo, "other", "renamed")), Ok(()));
        assert!(
            harness.ran("branch -m other renamed"),
            "{:?}",
            harness.commands()
        );
        assert!(harness
            .git(&["branch", "--list", "renamed"])
            .contains("renamed"));

        let clash = harness.run(|repo| rename(repo, "renamed", "third"));
        assert!(clash.is_err());
        assert!(harness
            .git(&["branch", "--list", "renamed"])
            .contains("renamed"));
    }

    #[test]
    fn delete_unmerged_branch_forces_itself_and_says_so() {
        let harness = two_branches();
        let tip = harness.git(&["rev-parse", "other"]).trim().to_string();

        let deleted = harness.run(|repo| delete(repo, "other")).unwrap();

        assert_eq!(
            deleted,
            Deleted {
                name: "other".to_string(),
                tip,
                upstream: None,
                forced: true,
                base: Some("main".to_string()),
            }
        );
        assert!(harness.ran("branch -d other"), "{:?}", harness.commands());
        assert!(harness.ran("branch -D other"), "{:?}", harness.commands());
        assert_eq!(harness.git(&["branch", "--list", "other"]), "");
    }

    #[test]
    fn delete_unmerged_branch_on_detached_head_takes_the_commit() {
        let harness = two_branches();
        let head = harness.head();
        harness.git(&["checkout", "-q", "--detach"]);

        let deleted = harness.run(|repo| delete(repo, "other")).unwrap();

        assert_eq!(deleted.base, Some(head));
    }

    #[test]
    fn delete_merged_branch_does_not_force() {
        let harness = two_branches();
        harness.git(&["branch", "-q", "merged", "main"]);

        let deleted = harness.run(|repo| delete(repo, "merged")).unwrap();

        assert!(!deleted.forced);
        assert_eq!(deleted.base, None);
        assert!(!harness.ran("branch -D merged"), "{:?}", harness.commands());
    }

    // `git branch -d` compares with the upstream: what is missing is in it, even if HEAD has it.
    #[test]
    fn delete_unmerged_into_its_upstream_takes_that_base() {
        let (harness, _bare) = cloned();
        harness.git(&["fetch", "-q", "origin"]);
        harness.git(&["branch", "-q", "--track", "followed", "origin/main"]);
        harness.git(&["checkout", "-q", "followed"]);
        harness.repo.commit("s.txt", b"s\n", "s");
        harness.git(&["checkout", "-q", "main"]);
        harness.git(&["merge", "-q", "--ff-only", "followed"]);

        let deleted = harness.run(|repo| delete(repo, "followed")).unwrap();

        assert!(deleted.forced);
        assert_eq!(deleted.base, Some("origin/main".to_string()));
    }

    #[test]
    fn delete_of_the_current_branch_fails_and_does_not_force() {
        let harness = two_branches();

        let outcome = harness.run(|repo| delete(repo, "main"));

        assert!(outcome.is_err());
        assert_eq!(harness.current(), "main");
        assert!(!harness.ran("branch -D main"), "{:?}", harness.commands());
    }

    #[test]
    fn restore_returns_branch_and_tracking() {
        let (harness, _bare) = cloned();
        harness.git(&["fetch", "-q", "origin"]);
        harness.git(&["branch", "-q", "--track", "x", "origin/x"]);
        let tip = harness.git(&["rev-parse", "x"]);

        let deleted = harness.run(|repo| delete(repo, "x")).unwrap();
        assert_eq!(deleted.upstream.as_deref(), Some("origin/x"));
        assert_eq!(harness.git(&["branch", "--list", "x"]), "");

        assert_eq!(harness.run(move |repo| restore(repo, &deleted)), Ok(()));

        assert_eq!(harness.git(&["rev-parse", "x"]), tip);
        assert_eq!(upstream(&harness, "x"), "origin/x");
    }

    // The branch matters more than the tracking: the upstream may have disappeared.
    #[test]
    fn restore_without_a_live_upstream_returns_the_branch() {
        let (harness, _bare) = cloned();
        harness.git(&["fetch", "-q", "origin"]);
        harness.git(&["branch", "-q", "--track", "x", "origin/x"]);
        let deleted = harness.run(|repo| delete(repo, "x")).unwrap();
        harness.git(&["update-ref", "-d", "refs/remotes/origin/x"]);

        assert_eq!(harness.run(move |repo| restore(repo, &deleted)), Ok(()));

        assert!(harness.git(&["branch", "--list", "x"]).contains('x'));
    }

    #[test]
    fn restore_does_not_overwrite_a_new_branch_with_that_name() {
        let harness = two_branches();
        let deleted = harness.run(|repo| delete(repo, "other")).unwrap();
        harness.git(&["branch", "-q", "other", "main"]);
        let now = harness.git(&["rev-parse", "other"]);

        assert!(harness.run(move |repo| restore(repo, &deleted)).is_err());

        assert_eq!(harness.git(&["rev-parse", "other"]), now);
    }

    #[test]
    fn delete_remote_deletes_the_branch_and_its_locals_if_asked() {
        let (harness, bare) = cloned();
        harness.git(&["fetch", "-q", "origin"]);
        harness.git(&["branch", "-q", "--track", "x", "origin/x"]);
        harness.git(&["branch", "-q", "--track", "y", "origin/x"]);
        harness.git(&["checkout", "-q", "y"]);

        let outcome = harness
            .run_remote(|repo| delete_remote(repo, "origin", "x", true))
            .recv_timeout(Duration::from_secs(60))
            .unwrap()
            .unwrap();

        let names: Vec<&str> = outcome.local.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["x"]);
        assert_eq!(outcome.failures, Vec::<String>::new());
        assert!(
            harness.ran("push --progress --porcelain origin :refs/heads/x"),
            "{:?}",
            harness.commands()
        );
        assert_eq!(harness.git(&["branch", "--list", "x"]), "");
        assert!(harness.git(&["branch", "--list", "y"]).contains('y'));
        let refs = crate::test_support::git_command()
            .args(["for-each-ref", "refs/heads/x"])
            .current_dir(bare.path())
            .output()
            .unwrap();
        assert!(refs.stdout.is_empty());
    }

    // `branch_delete_remote` runs as `TaskKind::Remote`: the push does not wait for the
    // repository's writes, but deleting the local trackers does, as `branch_delete` waits.
    #[test]
    fn delete_remote_deletes_the_locals_inside_the_write_lock() {
        let (harness, _bare) = cloned();
        harness.git(&["fetch", "-q", "origin"]);
        harness.git(&["branch", "-q", "--track", "x", "origin/x"]);
        let (release, held) = std::sync::mpsc::channel::<()>();
        let (locked, wait_locked) = std::sync::mpsc::channel::<()>();
        let locks = harness.locks.clone();
        let repo = harness.repo.path().to_path_buf();
        let writer = thread::spawn(move || {
            locks.run(&repo, || {
                locked.send(()).unwrap();
                held.recv().unwrap();
            })
        });
        wait_locked.recv().unwrap();

        let done = harness.run_remote(|repo| delete_remote(repo, "origin", "x", true));
        let pushed = Instant::now();
        while !harness.ran("push --progress --porcelain origin :refs/heads/x") {
            assert!(
                pushed.elapsed() < Duration::from_secs(30),
                "the push did not run"
            );
            thread::sleep(Duration::from_millis(20));
        }
        thread::sleep(Duration::from_millis(300));
        assert!(
            done.try_recv().is_err(),
            "it must wait for the write in progress"
        );
        assert!(harness.git(&["branch", "--list", "x"]).contains('x'));

        release.send(()).unwrap();
        writer.join().unwrap();
        let outcome = done.recv_timeout(Duration::from_secs(60)).unwrap().unwrap();
        assert_eq!(outcome.local.len(), 1);
        assert_eq!(harness.git(&["branch", "--list", "x"]), "");
    }

    fn in_bare(bare: &TempDir, args: &[&str]) -> String {
        let output = crate::test_support::git_command()
            .args(args)
            .current_dir(bare.path())
            .output()
            .unwrap();
        assert!(output.status.success(), "{args:?}");
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    #[test]
    fn delete_remote_with_ref_already_deleted_removes_the_reference_and_respects_the_tag() {
        let (harness, bare) = cloned();
        harness.git(&["fetch", "-q", "origin"]);
        in_bare(&bare, &["tag", "x", "main"]);
        in_bare(&bare, &["update-ref", "-d", "refs/heads/x"]);

        let outcome = harness.run(|repo| delete_remote(repo, "origin", "x", false));

        assert!(outcome.is_ok(), "{outcome:?}");
        assert_eq!(harness.git(&["branch", "-r", "--list", "origin/x"]), "");
        assert_eq!(in_bare(&bare, &["tag", "--list", "x"]).trim(), "x");
    }

    #[test]
    fn delete_remote_protected_is_rejected() {
        for branch in ["main", "master"] {
            let (harness, bare) = cloned();
            in_bare(&bare, &["branch", "-f", "master", "main"]);

            let outcome = harness.run(|repo| delete_remote(repo, "origin", branch, false));

            assert!(outcome.is_err(), "{branch}");
            let heads = in_bare(&bare, &["branch", "--list", branch]);
            assert!(heads.contains(branch), "{branch} is still on the remote");
        }
    }

    #[test]
    fn delete_remote_uses_the_repository_patterns() {
        let (harness, bare) = cloned();
        in_bare(&bare, &["branch", "release-1", "main"]);
        in_bare(&bare, &["branch", "free", "main"]);
        let patterns = vec!["release.*".to_string()];
        settings::save_protected_branches(harness.data.path(), patterns).unwrap();

        let refused = harness.run(|repo| delete_remote(repo, "origin", "release-1", false));
        let freed = harness.run(|repo| delete_remote(repo, "origin", "free", false));

        assert!(refused.is_err());
        assert!(freed.is_ok());
        assert!(in_bare(&bare, &["branch", "--list", "release-1"]).contains("release-1"));
        assert!(in_bare(&bare, &["branch", "--list", "free"]).is_empty());
    }

    // `:x` fails with both, and would delete the tag if only it existed.
    #[test]
    fn delete_remote_with_tag_of_the_same_name_deletes_only_the_branch() {
        let (harness, bare) = cloned();
        harness.git(&["fetch", "-q", "origin"]);
        in_bare(&bare, &["tag", "x", "main"]);

        let outcome = harness.run(|repo| delete_remote(repo, "origin", "x", false));

        assert!(outcome.is_ok(), "{outcome:?}");
        assert_eq!(in_bare(&bare, &["branch", "--list", "x"]), "");
        assert_eq!(in_bare(&bare, &["tag", "--list", "x"]).trim(), "x");
    }

    #[test]
    fn trackers_are_the_locals_that_track_that_branch_except_the_current_one() {
        let (harness, _bare) = cloned();
        harness.git(&["fetch", "-q", "origin"]);
        harness.git(&["branch", "-q", "--track", "x", "origin/x"]);
        harness.git(&["branch", "-q", "--track", "y", "origin/x"]);
        harness.git(&["checkout", "-q", "y"]);

        let found = trackers(harness.repo.path(), "origin/x").unwrap();

        assert_eq!(found, ["x"]);
    }

    #[test]
    fn remote_with_slash_is_split_by_the_existing_remotes() {
        let (harness, _bare) = cloned();
        harness.git(&["remote", "add", "team/one", "/nothing"]);
        let path = harness.repo.path();

        assert_eq!(
            split_remote(path, "team/one/a/b").unwrap(),
            Some(("team/one".to_string(), "a/b".to_string()))
        );
        assert_eq!(
            split_remote(path, "origin/x").unwrap(),
            Some(("origin".to_string(), "x".to_string()))
        );
        assert_eq!(split_remote(path, "loose").unwrap(), None);
    }

    #[test]
    fn unmerged_commits_are_the_tip_ones_the_base_does_not_have() {
        let harness = two_branches();
        let tip = harness.git(&["rev-parse", "other"]).trim().to_string();

        let commits = unmerged_commits(harness.repo.path(), "main", &tip).unwrap();

        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].hash, tip);
        assert_eq!(commits[0].subject, "three");
        assert_eq!(commits[0].author, "Test");
    }

    // Three commits: `a.txt` changes in the last one and `b.txt` comes in the second.
    fn with_history() -> (Harness, String) {
        let harness = Harness::new();
        let first = harness.repo.commit("a.txt", b"1\n", "one");
        harness.repo.commit("b.txt", b"1\n", "two");
        harness.repo.commit("a.txt", b"2\n", "three");
        (harness, first)
    }

    #[test]
    fn reset_hard_creates_a_copy_of_what_was_modified() {
        let (harness, first) = with_history();
        harness.write("a.txt", "in stage\n");
        harness.git(&["add", "a.txt"]);
        harness.write("a.txt", "on disk\n");
        harness.write("new.txt", "new in stage\n");
        harness.git(&["add", "new.txt"]);
        let target = first.clone();

        let outcome =
            harness.run(move |repo| reset(repo, &target, ResetMode::Hard, CheckoutMode::Plain));

        assert_eq!(outcome, Ok(CheckoutOutcome::Done));
        assert_eq!(harness.head(), first);
        assert_eq!(harness.read("a.txt"), "1\n");
        assert!(!harness.repo.path().join("new.txt").exists());
        assert_eq!(harness.saved_paths(), ["a.txt", "new.txt"]);

        harness.restore_snapshot();
        assert_eq!(harness.read("a.txt"), "on disk\n");
        assert_eq!(harness.read("new.txt"), "new in stage\n");
        assert_eq!(harness.git(&["show", ":a.txt"]), "in stage\n");
    }

    #[test]
    fn reset_hard_includes_overwritten_unversioned() {
        let harness = Harness::new();
        harness.repo.commit("a.txt", b"1\n", "one");
        let second = harness.repo.commit("seen.txt", b"versioned\n", "two");
        harness.git(&["rm", "-q", "seen.txt"]);
        harness.write(".gitignore", "ignored.txt\n");
        harness.git(&["add", ".gitignore"]);
        harness.git(&["commit", "-q", "-m", "three"]);
        harness.write("seen.txt", "unversioned\n");
        harness.write("separate.txt", "untouched\n");
        let target = second.clone();

        let outcome =
            harness.run(move |repo| reset(repo, &target, ResetMode::Hard, CheckoutMode::Plain));

        assert_eq!(outcome, Ok(CheckoutOutcome::Done));
        assert_eq!(harness.read("seen.txt"), "versioned\n");
        assert_eq!(harness.read("separate.txt"), "untouched\n");
        assert_eq!(harness.saved_paths(), ["seen.txt"]);

        harness.restore_snapshot();
        assert_eq!(harness.read("seen.txt"), "unversioned\n");
    }

    #[test]
    fn reset_hard_includes_overwritten_ignored() {
        let harness = Harness::new();
        harness.repo.commit("seen.txt", b"versioned\n", "one");
        let first = harness.head();
        harness.git(&["rm", "-q", "seen.txt"]);
        harness.write(".gitignore", "seen.txt\n");
        harness.git(&["add", ".gitignore"]);
        harness.git(&["commit", "-q", "-m", "two"]);
        harness.write("seen.txt", "ignored\n");

        harness
            .run(move |repo| reset(repo, &first, ResetMode::Hard, CheckoutMode::Plain))
            .unwrap();

        assert_eq!(harness.saved_paths(), ["seen.txt"]);
    }

    // Git deletes the whole folder without warning to put the target's file.
    #[test]
    fn reset_hard_includes_the_unversioned_of_a_folder_in_the_place_of_a_file() {
        let harness = Harness::new();
        harness.repo.commit("f", b"file\n", "one");
        let first = harness.head();
        harness.git(&["rm", "-q", "f"]);
        harness.repo.commit("f/versioned", b"v\n", "two");
        harness.write("f/untracked", "loose\n");
        harness.write("f/sub/deep", "deep\n");
        symlink("untracked", harness.repo.path().join("f/link")).unwrap();

        harness
            .run(move |repo| reset(repo, &first, ResetMode::Hard, CheckoutMode::Plain))
            .unwrap();

        assert_eq!(harness.read("f"), "file\n");
        assert_eq!(
            harness.saved_paths(),
            ["f/link", "f/sub/deep", "f/untracked"]
        );
        fs::remove_file(harness.repo.path().join("f")).unwrap();
        harness.restore_snapshot();
        assert_eq!(harness.read("f/sub/deep"), "deep\n");
    }

    #[test]
    fn reset_hard_includes_a_file_in_the_way_of_a_folder() {
        let harness = Harness::new();
        harness.repo.commit("d/f", b"inside\n", "one");
        let first = harness.head();
        harness.git(&["rm", "-q", "-r", "d"]);
        harness.repo.commit("another.txt", b"o\n", "two");
        harness.write("d", "loose file\n");

        harness
            .run(move |repo| reset(repo, &first, ResetMode::Hard, CheckoutMode::Plain))
            .unwrap();

        assert_eq!(harness.read("d/f"), "inside\n");
        assert_eq!(harness.saved_paths(), ["d"]);
    }

    #[test]
    fn reset_hard_includes_changes_git_does_not_look_at() {
        let (harness, first) = with_history();
        harness.git(&["update-index", "--assume-unchanged", "a.txt"]);
        harness.write("a.txt", "hidden\n");

        harness
            .run(move |repo| reset(repo, &first, ResetMode::Hard, CheckoutMode::Plain))
            .unwrap();

        assert_eq!(harness.saved_paths(), ["a.txt"]);
    }

    #[test]
    fn reset_hard_with_nothing_to_lose_leaves_no_copy() {
        let (harness, first) = with_history();

        harness
            .run(move |repo| reset(repo, &first, ResetMode::Hard, CheckoutMode::Plain))
            .unwrap();

        assert_eq!(harness.snapshots(), 0);
    }

    #[test]
    fn reset_hard_unable_to_copy_touches_nothing() {
        let (harness, first) = with_history();
        harness.write("a.txt", "local\n");
        let head = harness.head();
        let data = harness.data.path();
        fs::set_permissions(data, fs::Permissions::from_mode(0o500)).unwrap();

        let outcome =
            harness.run(move |repo| reset(repo, &first, ResetMode::Hard, CheckoutMode::Plain));

        fs::set_permissions(data, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(outcome.is_err());
        assert_eq!(harness.head(), head);
        assert_eq!(harness.read("a.txt"), "local\n");
    }

    #[test]
    fn what_is_written_after_the_copy_stops_the_operation() {
        let (harness, first) = with_history();
        harness.write("a.txt", "local\n");
        let file: PathBuf = harness.repo.path().join("a.txt");

        let outcome = harness.run(move |repo| {
            copy_pausing(
                repo,
                "test",
                |path| snapshots::at_risk(path, &first),
                || fs::write(&file, "written after the copy\n").unwrap(),
            )
        });

        assert!(message(outcome.unwrap_err()).contains("a.txt"));
        assert_eq!(harness.read("a.txt"), "written after the copy\n");
    }

    #[test]
    fn reset_soft_does_not_touch_the_files() {
        let (harness, first) = with_history();
        let target = first.clone();

        let soft =
            harness.run(move |repo| reset(repo, &target, ResetMode::Soft, CheckoutMode::Plain));

        assert_eq!(soft, Ok(CheckoutOutcome::Done));
        assert_eq!(harness.head(), first);
        assert_eq!(harness.read("a.txt"), "2\n");
        assert!(harness.git(&["status", "--porcelain"]).contains("M  a.txt"));
        assert_eq!(harness.snapshots(), 0);
    }

    #[test]
    fn reset_mixed_copies_what_is_only_in_the_stage() {
        let (harness, first) = with_history();
        harness.write("b.txt", "in stage\n");
        harness.git(&["add", "b.txt"]);
        harness.write("b.txt", "on disk\n");

        let outcome =
            harness.run(move |repo| reset(repo, &first, ResetMode::Mixed, CheckoutMode::Plain));

        assert_eq!(outcome, Ok(CheckoutOutcome::Done));
        assert_eq!(harness.read("b.txt"), "on disk\n");
        assert_eq!(harness.saved_paths(), ["b.txt"]);
        assert!(harness.snapshot().entries[0].staged_blob);
    }

    #[test]
    fn reset_keep_copies_what_is_only_in_the_stage() {
        let (harness, _) = with_history();
        let previous = harness.git(&["rev-parse", "HEAD~1"]).trim().to_string();
        harness.write("b.txt", "in stage\n");
        harness.git(&["add", "b.txt"]);
        harness.write("b.txt", "on disk\n");

        let outcome =
            harness.run(move |repo| reset(repo, &previous, ResetMode::Keep, CheckoutMode::Plain));

        assert_eq!(outcome, Ok(CheckoutOutcome::Done));
        assert_eq!(harness.read("b.txt"), "on disk\n");
        assert_eq!(harness.read("a.txt"), "1\n");
        assert_eq!(harness.git(&["show", ":b.txt"]), "1\n");
        assert!(harness.snapshot().entries[0].staged_blob);
    }

    #[test]
    fn reset_keep_with_conflict_returns_files() {
        let (harness, first) = with_history();
        harness.write("a.txt", "local\n");
        let head = harness.head();
        let target = first.clone();

        let outcome =
            harness.run(move |repo| reset(repo, &target, ResetMode::Keep, CheckoutMode::Plain));

        assert_eq!(
            outcome,
            Ok(CheckoutOutcome::Overwritten {
                files: vec!["a.txt".to_string()]
            })
        );
        assert_eq!(harness.head(), head);
        assert_eq!(harness.read("a.txt"), "local\n");
    }

    #[test]
    fn smart_reset_saves_does_keep_and_restores() {
        let (harness, first) = with_history();
        harness.write("b.txt", "local\n");
        harness.write("a.txt", "2\nlocal\n");
        let target = first.clone();

        let outcome =
            harness.run(move |repo| reset(repo, &target, ResetMode::Keep, CheckoutMode::Smart));

        assert_eq!(harness.head(), first);
        assert!(
            matches!(outcome, Ok(CheckoutOutcome::Conflicts { .. })),
            "{outcome:?}"
        );
        assert_eq!(harness.read("b.txt"), "local\n");
    }

    #[test]
    fn hard_reset_from_the_problem_overwrites_with_a_copy() {
        let (harness, first) = with_history();
        harness.write("a.txt", "local\n");
        let target = first.clone();

        let outcome =
            harness.run(move |repo| reset(repo, &target, ResetMode::Keep, CheckoutMode::Force));

        assert_eq!(outcome, Ok(CheckoutOutcome::Done));
        assert_eq!(harness.read("a.txt"), "1\n");
        assert_eq!(harness.saved_paths(), ["a.txt"]);
        assert!(harness
            .commands()
            .iter()
            .any(|line| line.contains("reset --hard")));
    }

    #[test]
    fn lightweight_and_annotated_tag() {
        let harness = two_branches();
        let hash = harness.head();
        let target = hash.clone();

        harness
            .run(move |repo| tag(repo, "v1", &target, "", false))
            .unwrap();
        harness
            .run(|repo| tag(repo, "v2", "HEAD~1", "-starts with a dash", false))
            .unwrap();

        assert_eq!(
            harness.git(&["cat-file", "-t", "refs/tags/v1"]).trim(),
            "commit"
        );
        assert_eq!(harness.git(&["rev-parse", "v1"]).trim(), hash);
        assert_eq!(
            harness.git(&["cat-file", "-t", "refs/tags/v2"]).trim(),
            "tag"
        );
        assert_eq!(
            harness
                .git(&["tag", "-l", "--format=%(contents:subject)", "v2"])
                .trim(),
            "-starts with a dash"
        );
    }

    #[test]
    fn tag_force() {
        let harness = two_branches();
        let old = harness.git(&["rev-parse", "HEAD~1"]);
        harness.git(&["tag", "v1", "HEAD~1"]);

        assert!(harness
            .run(|repo| tag(repo, "v1", "HEAD", "", false))
            .is_err());
        assert_eq!(harness.git(&["rev-parse", "v1"]), old);

        harness
            .run(|repo| tag(repo, "v1", "HEAD", "", true))
            .unwrap();
        assert_eq!(harness.git(&["rev-parse", "v1"]).trim(), harness.head());
    }

    // An annotated tag keeps author and message in its own object: restoring it onto the commit
    // would lose them without anyone seeing it.
    #[test]
    fn delete_tag_and_restore() {
        let harness = two_branches();
        harness.git(&["tag", "-a", "-m", "tag message", "v1"]);
        harness.git(&["tag", "light"]);
        let object = harness.git(&["rev-parse", "refs/tags/v1"]);

        let deleted = harness.run(|repo| delete_tag(repo, "v1")).unwrap();
        assert_eq!(deleted.tip, object.trim());
        assert_eq!(harness.git(&["tag", "--list", "v1"]), "");
        harness
            .run(move |repo| restore_tag(repo, &deleted))
            .unwrap();

        assert_eq!(harness.git(&["rev-parse", "refs/tags/v1"]), object);
        assert_eq!(
            harness.git(&["cat-file", "-t", "refs/tags/v1"]).trim(),
            "tag"
        );
        assert_eq!(
            harness
                .git(&["tag", "-l", "--format=%(contents:subject)", "v1"])
                .trim(),
            "tag message"
        );

        let light = harness.run(|repo| delete_tag(repo, "light")).unwrap();
        harness.run(move |repo| restore_tag(repo, &light)).unwrap();
        assert_eq!(harness.git(&["rev-parse", "light"]).trim(), harness.head());
    }

    // A fake `git` that records its arguments: that nothing reaches it is what is checked.
    #[test]
    fn name_with_a_leading_dash_is_rejected() {
        let bin = TempDir::new().unwrap();
        let script = bin.path().join("git");
        let log = bin.path().join("command.log");
        fs::write(
            &script,
            format!("#!/bin/sh\necho \"$@\" >> '{}'\n", log.display()),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let repo = TempRepo::new();
        let data = TempDir::new().unwrap();
        let runner = Runner::with(
            Arc::<RepoLocks>::default(),
            Arc::<Console>::default(),
            Some(script),
            Duration::from_secs(5),
        );
        let (path, dir) = (repo.path().to_path_buf(), data.path().to_path_buf());

        let results: Vec<bool> = in_task(&runner, repo.path(), move |ctx| {
            let repo = Repo {
                ctx,
                path: &path,
                data: &dir,
            };
            let plain = CheckoutMode::Plain;
            let no = Overwrite::No;
            let keep = ResetMode::Keep;
            let lost = Deleted {
                name: "-x".to_string(),
                tip: "abc".to_string(),
                upstream: None,
                forced: false,
                base: None,
            };
            let tag_lost = DeletedTag {
                name: "-x".to_string(),
                tip: "abc".to_string(),
            };
            vec![
                checkout(&repo, "-x", plain).is_err(),
                checkout(&repo, "--force", CheckoutMode::Force).is_err(),
                checkout_remote(&repo, "-x", RemoteOptions::default()).is_err(),
                checkout_remote(
                    &repo,
                    "origin/x",
                    RemoteOptions {
                        name: Some("-x"),
                        ..RemoteOptions::default()
                    },
                )
                .is_err(),
                create(&repo, "-x", None, true, no, plain).is_err(),
                create(&repo, "x", Some("-y"), true, no, plain).is_err(),
                rename(&repo, "-x", "y").is_err(),
                rename(&repo, "x", "-y").is_err(),
                delete(&repo, "-x").is_err(),
                restore(&repo, &lost).is_err(),
                delete_remote(&repo, "-o", "x", false).is_err(),
                delete_remote(&repo, "origin", "-x", false).is_err(),
                reset(&repo, "-x", keep, plain).is_err(),
                tag(&repo, "-x", "HEAD", "", false).is_err(),
                tag(&repo, "x", "-y", "", false).is_err(),
                delete_tag(&repo, "-x").is_err(),
                restore_tag(&repo, &tag_lost).is_err(),
            ]
        });

        assert_eq!(results, vec![true; 17]);
        let orders = fs::read_to_string(&log).unwrap_or_default();
        let reached: Vec<&str> = orders
            .lines()
            .filter(|line| !line.contains("check-ref-format") && !line.contains("remote"))
            .collect();
        assert_eq!(reached, Vec::<&str>::new());
        assert!(!orders.contains(" -x"), "{orders}");
        assert!(!orders.contains(" -y"), "{orders}");
    }

    #[test]
    fn names_git_does_not_accept_are_rejected() {
        let harness = two_branches();
        for name in ["HEAD", "a..b", "a b", "@{-1}", "fin.lock", ""] {
            let owned = name.to_string();
            let outcome = harness.run(move |repo| {
                create(
                    repo,
                    &owned,
                    None,
                    false,
                    Overwrite::No,
                    CheckoutMode::Plain,
                )
            });
            assert!(outcome.is_err(), "{name}");
            let owned = name.to_string();
            if name != "HEAD" {
                let outcome = harness.run(move |repo| tag(repo, &owned, "HEAD", "", false));
                assert!(outcome.is_err(), "{name}");
            }
        }
        let error = harness
            .run(|repo| create(repo, "a b", None, false, Overwrite::No, CheckoutMode::Plain))
            .unwrap_err();
        assert_eq!(message(error), "Branch name a b is not valid");
    }
}
