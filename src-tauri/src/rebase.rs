// A rebase and what follows: reading its output like `GitRebaseProblemDetector`, continuing it, skipping a
// commit and aborting it. Ported from `GitRebaseProcess`, `GitRebaseProblemDetector`, `GitImpl.rebase*` and
// `GitAbortRebaseProcess`.
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::branches::{self, Repo};
use crate::commit;
use crate::console;
use crate::editor::{EditKind, EditorHandler, Request, REBASE_CONFIG};
use crate::git::{self, GitError};
use crate::operation;
use crate::preserve::{self, RestoreOutcome, Saved};
use crate::push;
use crate::rewrite;
use crate::settings::{self, CleanTree};
use crate::snapshots;
use crate::task::GitOutput;
use crate::update;

const REBASE: &str = "Rebase";
const REBASE_ABORT: &str = "Rebase --abort";
const NO_REBASE: &str = "No rebase in progress";
const HEAD: &str = "HEAD";
const HEADS: &str = "refs/heads/";
const REBASE_DIRS: [&str; 2] = ["rebase-merge", "rebase-apply"];
const ORIG_HEAD_FILE: &str = "orig-head";
// `GitRebaseProblemDetector`.
const CONFLICT_INDICATORS: [&str; 7] = [
    "Merge conflict in",
    "after resolving the conflicts, mark the corrected paths",
    "After resolving the conflicts, mark them with",
    "Resolve all conflicts manually, mark them as resolved with",
    "You must edit all merge conflicts",
    "Failed to merge in the changes",
    "could not apply",
];
const NO_CHANGES: &str = "No changes - did you forget to use 'git add'?";
const DIRTY_TREE: [&str; 2] = [
    "you have unstaged changes",
    "your index contains uncommitted changes",
];
const STOPPED_FOR_EDITING: &str = "You can amend the commit now";
// `GitUntrackedFilesOverwrittenByOperationDetector`: the old git format.
const OLD_UNTRACKED: &str = "Untracked working tree file '";
const OLD_UNTRACKED_END: &str = "' would be overwritten by";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "reason", rename_all = "camelCase")]
pub enum Outcome {
    Done,
    // Stopped at conflicts, as git leaves it.
    Conflicts,
    // Stopped at an `edit`, or with a `reword`'s message not written.
    Editing,
    // Untracked files that the rebase would overwrite.
    Untracked { files: Vec<String> },
    Failed(String),
}

#[derive(Default)]
struct Detected {
    conflict: bool,
    no_changes: bool,
    dirty_tree: bool,
    stopped_for_editing: bool,
}

impl Detected {
    // Each line counts for one thing only, in the order of IntelliJ's detector.
    fn of(text: &str) -> Self {
        let mut found = Self::default();
        for line in text.lines() {
            let lower = line.to_lowercase();
            let has = |needle: &str| lower.contains(&needle.to_lowercase());
            if CONFLICT_INDICATORS.iter().any(|needle| has(needle)) {
                found.conflict = true;
            } else if has(NO_CHANGES) {
                found.no_changes = true;
            } else if DIRTY_TREE.iter().any(|needle| has(needle)) {
                found.dirty_tree = true;
            } else if has(STOPPED_FOR_EDITING) {
                found.stopped_for_editing = true;
            }
        }
        found
    }
}

fn old_untracked_files(text: &str) -> impl Iterator<Item = String> + '_ {
    text.lines().filter_map(|line| {
        let rest = line.split_once(OLD_UNTRACKED)?.1;
        Some(rest.split_once(OLD_UNTRACKED_END)?.0.to_string())
    })
}

fn untracked_files(text: &str) -> Vec<String> {
    let mut files = preserve::listed_files(text, preserve::UNTRACKED_FILES);
    files.extend(old_untracked_files(text));
    files
}

// What the user did in the editor: cancelling the list leaves the rebase unstarted, and cancelling a
// message leaves it stopped there (`wasCommitListEditorCancelled` and `wasUnstructuredEditorCancelled`).
// It also delegates `edit_request`: that way an `Ask` stays alert to the editor leaving.
struct Watched {
    inner: Arc<dyn EditorHandler>,
    list_cancelled: AtomicBool,
    message_cancelled: AtomicBool,
}

impl Watched {
    fn note(&self, kind: EditKind, answer: Option<String>) -> Option<String> {
        if answer.is_none() {
            match kind {
                EditKind::Todo => self.list_cancelled.store(true, Ordering::SeqCst),
                EditKind::Message => self.message_cancelled.store(true, Ordering::SeqCst),
            }
        }
        answer
    }
}

impl EditorHandler for Watched {
    fn edit(&self, kind: EditKind, text: &str) -> Option<String> {
        let answer = self.inner.edit(kind, text);
        self.note(kind, answer)
    }

    fn edit_request(&self, request: &Request, kind: EditKind, text: &str) -> Option<String> {
        let answer = self.inner.edit_request(request, kind, text);
        self.note(kind, answer)
    }
}

fn exec(
    repo: &Repo,
    args: &[&str],
    handler: Arc<dyn EditorHandler>,
) -> Result<GitOutput, GitError> {
    let full: Vec<&str> = REBASE_CONFIG.iter().chain(args).copied().collect();
    repo.ctx.git_editing(&full, handler)
}

// What git saw: a dirty tree in the starting command is told apart, because `Process` saves
// again and repeats.
enum Ran {
    Outcome(Outcome),
    DirtyTree(String),
}

// `GitRebaseProcess.rebaseSingleRoot`, without the multiple roots or the saving of changes. `args`
// starts with `rebase`. If git says `No changes`, it repeats with `--skip` without asking, like IntelliJ.
pub fn run(
    repo: &Repo,
    args: &[&str],
    handler: Arc<dyn EditorHandler>,
) -> Result<Outcome, GitError> {
    Ok(match ran(repo, args, handler)? {
        Ran::Outcome(outcome) => outcome,
        Ran::DirtyTree(text) => Outcome::Failed(text),
    })
}

fn ran(repo: &Repo, args: &[&str], handler: Arc<dyn EditorHandler>) -> Result<Ran, GitError> {
    let watched = Arc::new(Watched {
        inner: handler,
        list_cancelled: AtomicBool::new(false),
        message_cancelled: AtomicBool::new(false),
    });
    let mut current = args.to_vec();
    loop {
        let (stdout, stderr) = match exec(repo, &current, watched.clone()) {
            Ok(output) => {
                let text = format!("{}\n{}", output.stdout, output.stderr);
                return Ok(Ran::Outcome(if Detected::of(&text).stopped_for_editing {
                    Outcome::Editing
                } else {
                    Outcome::Done
                }));
            }
            Err(GitError::Failed { stdout, stderr, .. }) => (stdout, stderr),
            Err(error) => return Err(error),
        };
        // Without a list nothing was started: whoever cancelled already knows.
        if watched.list_cancelled.load(Ordering::SeqCst) {
            return Err(GitError::Cancelled);
        }
        if watched.message_cancelled.load(Ordering::SeqCst) {
            return Ok(Ran::Outcome(Outcome::Editing));
        }
        let text = format!("{stdout}\n{stderr}");
        let found = Detected::of(&text);
        // Only in the starting command: with `--continue` or `--skip` everything should already be committed.
        if found.dirty_tree && current == args {
            return Ok(Ran::DirtyTree(failure_text(&stdout, &stderr)));
        }
        let files = untracked_files(&text);
        if !files.is_empty() {
            return Ok(Ran::Outcome(Outcome::Untracked { files }));
        }
        if found.no_changes && commit::rebase_in_progress(repo.path) {
            current = vec!["rebase", "--skip"];
        } else if found.dirty_tree || !found.conflict {
            return Ok(Ran::Outcome(Outcome::Failed(failure_text(
                &stdout, &stderr,
            ))));
        } else {
            return Ok(Ran::Outcome(Outcome::Conflicts));
        }
    }
}

// What git said, without credentials: falls back to the normal output if it wrote no error.
pub(crate) fn failure_text(stdout: &str, stderr: &str) -> String {
    let text = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    console::hide_credentials(text.trim())
}

fn resume(repo: &Repo, flag: &str) -> Result<Outcome, GitError> {
    if !commit::rebase_in_progress(repo.path) {
        return Err(GitError::Io(NO_REBASE.to_string()));
    }
    let outcome = run(repo, &["rebase", flag], rewrite::resuming_editor(repo));
    if !commit::rebase_in_progress(repo.path) {
        rewrite::forget(repo);
    }
    outcome
}

pub fn continue_rebase(repo: &Repo) -> Result<Outcome, GitError> {
    resume(repo, "--continue")
}

pub fn skip(repo: &Repo) -> Result<Outcome, GitError> {
    resume(repo, "--skip")
}

// `rebase-*/orig-head` is the tip from before starting, and it is the state `--abort` goes back to.
// `ORIG_HEAD` is moved by any other command while the rebase is stopped. Not cancellable, like
// `--abort` itself.
fn original_head(repo: &Repo) -> Result<String, GitError> {
    for dir in REBASE_DIRS {
        let file = git::git_path(repo.path, &format!("{dir}/{ORIG_HEAD_FILE}"))?;
        if let Ok(text) = std::fs::read_to_string(file) {
            let spec = format!("{}^{{commit}}", text.trim());
            let verified = repo
                .ctx
                .git_to_the_end(&["rev-parse", "-q", "--verify", &spec]);
            return match verified {
                Ok(output) => Ok(output.stdout.trim().to_string()),
                Err(GitError::Failed { .. }) => Err(branches::invalid(format!(
                    "Could not find \"{}\"",
                    text.trim()
                ))),
                Err(error) => Err(error),
            };
        }
    }
    Err(branches::invalid(format!(
        "Could not find \"{ORIG_HEAD_FILE}\" of the rebase"
    )))
}

// `--abort` leaves the tree as before with a hard reset: whatever was resolved by hand would go with it. Not
// cancellable: it also runs when an Update was already cancelled, and then it is what undoes it.
pub fn abort(repo: &Repo) -> Result<(), GitError> {
    if !commit::rebase_in_progress(repo.path) {
        return Err(GitError::Io(NO_REBASE.to_string()));
    }
    let original = original_head(repo)?;
    branches::copy_before(repo, REBASE_ABORT, |path| {
        snapshots::at_risk(path, &original)
    })?;
    let aborted = repo.ctx.git_to_the_end(&["rebase", "--abort"]).map(drop);
    rewrite::forget(repo);
    aborted
}

// `Rebase onto Remote`, through `Process`. With conflicts it stays in progress, as git leaves it.
pub fn rebase_onto(repo: &Repo, start: &str, branch: &str) -> Result<Rebased, GitError> {
    rebase_branch(repo, start, Some(branch))
}

// `git rebase <onto> [<branch>]`. Git goes through the branch and through `onto`, and in each one it overwrites what is ignored:
// it is copied beforehand, once it is seen that the rebase can be done.
fn rebase_branch(repo: &Repo, onto: &str, branch: Option<&str>) -> Result<Rebased, GitError> {
    branches::check_argument(onto, "The target")?;
    let base = branches::commit_of(repo, onto)?;
    let tip = match branch {
        Some(name) => {
            branches::check_branch(repo.ctx, name)?;
            branches::commit_of(repo, &format!("{HEADS}{name}"))?
        }
        None => branches::commit_of(repo, HEAD)?,
    };
    let mut args = vec!["rebase", onto];
    args.extend(branch);
    Process::new(repo, repo.ctx.asking_editor())
        .run(&args, || copy_in_the_way(repo, &tip, Some(&base)))
}

pub(crate) fn copy_in_the_way(repo: &Repo, tip: &str, base: Option<&str>) -> Result<(), GitError> {
    branches::copy_before(repo, REBASE, |path| {
        let mut paths = snapshots::in_the_way(path, tip)?;
        if let Some(base) = base {
            paths.extend(snapshots::in_the_way(path, base)?);
        }
        paths.sort();
        paths.dedup();
        Ok(paths)
    })
}

// `GitRebaseProcess`: check, save the changes, launch the rebase and restore them.
pub const NOT_ALLOWED: &str = "Rebase not allowed";
pub const NOT_STARTED: &str = "Rebase not started";

// What was saved before the rebase. `restore` is `None` if restoring was not attempted: the rebase was left
// halfway, and what was saved stays in the stash until it finishes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Kept {
    pub saved: Saved,
    pub restore: Option<RestoreOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rebased {
    pub outcome: Outcome,
    // With a title, the rebase did not get to start and nothing was touched: it is the notification's.
    pub title: Option<String>,
    pub saved: Vec<Kept>,
}

pub struct Process<'a, 'b> {
    repo: &'a Repo<'b>,
    handler: Arc<dyn EditorHandler>,
}

const SAVE_TITLE: &str = "rebase";
// `stash.changes.message` with `rebase.operation.name`.
const SAVE_MESSAGE: &str = "Uncommitted changes before rebase";
// `rebase.notification.failed.stash.text` and `.shelf.text` (G:298, G:297).
const STASH_FAILED: &str = "Could not stash local uncommitted changes:";
const SHELVE_FAILED: &str = "Could not shelve local uncommitted changes:";
const RESTORE_LATER_FIRST: &str =
    "The changes saved after these were not restored: restore them first.";
const REMOTES: &str = "refs/remotes/";

impl<'a, 'b> Process<'a, 'b> {
    pub fn new(repo: &'a Repo<'b>, handler: Arc<dyn EditorHandler>) -> Self {
        Self { repo, handler }
    }

    fn dirty(&self) -> Result<bool, GitError> {
        let args = ["status", "--porcelain", "--untracked-files=no"];
        Ok(!self.repo.ctx.git(&args)?.stdout.trim().is_empty())
    }

    fn save(&self) -> Result<Option<Saved>, GitError> {
        let repo = self.repo;
        preserve::save_local_as(repo.ctx, repo.path, repo.data, SAVE_TITLE, SAVE_MESSAGE)
    }

    fn save_failed(&self, error: GitError) -> String {
        let failed = match preserve::saver(self.repo.data) {
            CleanTree::Stash => STASH_FAILED,
            CleanTree::Shelve => SHELVE_FAILED,
        };
        format!("{failed}\n{}", branches::text_of(error))
    }

    // `GitRebaseUtils.rebase` and `GitRebaseProcess.doRebase`. `before` runs once it is seen that the rebase can
    // be done and before saving anything. Once saved no `?` comes out: what was saved always
    // comes back or is accounted for.
    pub fn run(
        &self,
        args: &[&str],
        before: impl FnOnce() -> Result<(), GitError>,
    ) -> Result<Rebased, GitError> {
        if let Some(text) = not_allowed(self.repo)? {
            return Ok(Rebased::refused(NOT_ALLOWED, text));
        }
        before()?;
        let mut saved = Vec::new();
        // `saveDirtyRootsInitially`: only if there is something to save.
        let initially = if self.dirty()? { self.save() } else { Ok(None) };
        match initially {
            Ok(first) => saved.extend(first),
            Err(GitError::Cancelled) => return Err(GitError::Cancelled),
            Err(error) => return Ok(Rebased::refused(NOT_STARTED, self.save_failed(error))),
        }
        let result = self.rebase(args, &mut saved);
        // With the rebase halfway, `pop` would get mixed up with it: what was saved waits for it to finish.
        let kept = if update::stopped_midway(self.repo.path) {
            saved
                .into_iter()
                .map(|saved| Kept {
                    saved,
                    restore: None,
                })
                .collect()
        } else {
            restore_all(self.repo, saved)
        };
        match result {
            Ok(outcome) => Ok(Rebased {
                outcome,
                title: None,
                saved: kept,
            }),
            Err(error) if kept.iter().all(Kept::is_back) => Err(error),
            Err(error) => Ok(Rebased {
                outcome: Outcome::Failed(branches::text_of(error)),
                title: None,
                saved: kept,
            }),
        }
    }

    // If the first save did not see everything (Devin wrote in between), it is saved again when git
    // sees the tree dirty, but only once: if it happens again, the save is of no use.
    fn rebase(&self, args: &[&str], saved: &mut Vec<Saved>) -> Result<Outcome, GitError> {
        let mut retried = false;
        loop {
            match ran(self.repo, args, self.handler.clone())? {
                Ran::Outcome(outcome) => return Ok(outcome),
                Ran::DirtyTree(text) if retried => return Ok(Outcome::Failed(text)),
                Ran::DirtyTree(text) => {
                    retried = true;
                    match self.save() {
                        Ok(Some(again)) => saved.push(again),
                        Ok(None) => return Ok(Outcome::Failed(text)),
                        Err(GitError::Cancelled) => return Err(GitError::Cancelled),
                        Err(error) => return Ok(Outcome::Failed(self.save_failed(error))),
                    }
                }
            }
        }
    }
}

impl Rebased {
    fn refused(title: &str, text: String) -> Self {
        Self {
            outcome: Outcome::Failed(text),
            title: Some(title.to_string()),
            saved: Vec::new(),
        }
    }
}

impl Kept {
    fn is_back(&self) -> bool {
        matches!(
            self.restore,
            Some(RestoreOutcome::Restored | RestoreOutcome::Gone)
        )
    }
}

// From the last saved to the first, each on top of what the previous one left. After one that does not come back
// whole it does not go on: `pop` would get mixed up with what was left halfway, and the rest stays in the stash.
fn restore_all(repo: &Repo, saved: Vec<Saved>) -> Vec<Kept> {
    let mut stop = false;
    let mut kept: Vec<Kept> = saved
        .into_iter()
        .rev()
        .map(|saved| {
            let restore = if stop {
                RestoreOutcome::Failed(RESTORE_LATER_FIRST.to_string())
            } else {
                preserve::restore_tracked(repo.ctx, repo.data, &saved)
            };
            stop = stop || !matches!(restore, RestoreOutcome::Restored | RestoreOutcome::Gone);
            Kept {
                saved,
                restore: Some(restore),
            }
        })
        .collect();
    kept.reverse();
    kept
}

// `GitRebaseUtils.isRebaseAllowed`, with the repository's short name.
fn not_allowed(repo: &Repo) -> Result<Option<String>, GitError> {
    let state = operation::state(repo.path)?;
    let name = repo.path.file_name().map_or_else(
        || repo.path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    let lines = |first: String, second: &str| Some(format!("{first}\n{second}"));
    Ok(match state.operation {
        Some(operation::Operation::Merging) => lines(
            format!("There is an unfinished merge process in {name}."),
            "You should complete the merge before starting a rebase.",
        ),
        Some(operation::Operation::Rebasing) => lines(
            format!("There is an unfinished rebase process in {name}."),
            "You should complete it before starting another rebase.",
        ),
        Some(operation::Operation::CherryPicking) => lines(
            format!("There is an unfinished cherry-pick process in {name}."),
            "You should finish it before starting a rebase.",
        ),
        Some(operation::Operation::Reverting) => lines(
            format!("There is an unfinished revert process in {name}."),
            "You should finish it before starting a rebase.",
        ),
        None if state.head.is_empty() => Some(format!("Repository {name} is empty.")),
        None if state.branch.is_none() => lines(
            format!("You are in the detached HEAD state in {name}."),
            "Rebase is not possible.",
        ),
        None => None,
    })
}

fn lines_of(repo: &Repo, args: &[&str]) -> Result<Vec<String>, GitError> {
    let output = repo.ctx.git(args)?;
    Ok(output.stdout.lines().map(str::to_string).collect())
}

// `findProtectedRemoteBranch`: the remote branches whose name, without the remote, matches a
// protected pattern, as (full reference, short name `origin/main`). Without `origin/HEAD`.
pub(crate) fn protected_remotes(
    path: &Path,
    patterns: &[String],
) -> Result<Vec<(String, String)>, GitError> {
    let args = ["for-each-ref", "--format=%(refname)%09%(symref)", REMOTES];
    let output = git::run(path, &args)?;
    let mut found = Vec::new();
    for line in String::from_utf8_lossy(&output).lines() {
        let (full, symref) = line.split_once('\t').unwrap_or((line, ""));
        let Some(short) = full.strip_prefix(REMOTES).filter(|_| symref.is_empty()) else {
            continue;
        };
        let Some((_, name)) = branches::split_remote(path, short)? else {
            continue;
        };
        if push::is_protected(patterns, &name) {
            found.push((full.to_string(), short.to_string()));
        }
    }
    Ok(found)
}

// `GitRebaseProcess.isRebasingPublishedCommit`: the protected remote branch (with this
// repository's patterns) that already has some commit of `upstream..branch`, or of the whole branch without `upstream`.
// A commit of both is in their common bases and in what they carry behind them.
pub fn published(
    repo: &Repo,
    upstream: Option<&str>,
    branch: &str,
) -> Result<Option<String>, GitError> {
    if let Some(upstream) = upstream {
        branches::check_argument(upstream, "The base")?;
        branches::commit_of(repo, upstream)?;
    }
    branches::check_argument(branch, "The branch")?;
    branches::commit_of(repo, branch)?;
    let patterns = settings::load_repo(repo.data).protected_branches;
    for (full, short) in protected_remotes(repo.path, &patterns)? {
        let bases = match lines_of(repo, &["merge-base", "--all", &full, branch]) {
            Ok(bases) => bases,
            Err(GitError::Failed { .. }) => continue,
            Err(error) => return Err(error),
        };
        if bases.is_empty() {
            continue;
        }
        let mut args = vec!["rev-list".to_string(), "-1".to_string()];
        args.extend(bases);
        args.extend(upstream.map(|upstream| format!("^{upstream}")));
        args.push("--".to_string());
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        if !repo.ctx.git(&args)?.stdout.trim().is_empty() {
            return Ok(Some(short));
        }
    }
    Ok(None)
}

// `GitRebaseBranchAction` (`Rebase 'y' onto 'x'`, without `branch`) and `GitCheckoutWithRebaseAction` with
// a local one (`git rebase HEAD x`). The asker is the popup: if HEAD moved, nothing is touched.
pub fn branch_rebase(
    repo: &Repo,
    onto: &str,
    branch: Option<&str>,
    expected_head: &str,
) -> Result<Rebased, GitError> {
    operation::check_head(repo.path, expected_head)?;
    rebase_branch(repo, onto, branch)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "reason", rename_all = "camelCase")]
pub enum RemoteRebase {
    Rebased(Rebased),
    NeedsName { suggested: String },
    LocalHasCommits { local: String },
}

// `GitCheckoutWithRebaseAction` with a remote one and `GitCheckoutAndRebaseRemoteBranchWorkflow`: the local one
// named like it is created from the remote one (`git branch [-f] x origin/x`, which leaves it tracking it) and
// it is rebased onto the current one. With a chosen name it does not look at what it tracks, like `checkout_remote`.
pub fn rebase_remote(
    repo: &Repo,
    remote_branch: &str,
    name: Option<&str>,
    drop_commits: bool,
    expected_head: &str,
) -> Result<RemoteRebase, GitError> {
    operation::check_head(repo.path, expected_head)?;
    branches::check_argument(remote_branch, "The branch")?;
    branches::commit_of(repo, &format!("{REMOTES}{remote_branch}"))?;
    if let Some(text) = not_allowed(repo)? {
        return Ok(RemoteRebase::Rebased(Rebased::refused(NOT_ALLOWED, text)));
    }
    let local = match name {
        Some(name) => {
            branches::check_branch(repo.ctx, name)?;
            name.to_string()
        }
        None => {
            let suggested = branches::split_remote(repo.path, remote_branch)?
                .map(|(_, branch)| branch)
                .filter(|branch| branch != HEAD);
            let Some(suggested) = suggested else {
                return Ok(RemoteRebase::NeedsName {
                    suggested: remote_branch.to_string(),
                });
            };
            let exists = branches::ref_exists(repo, &format!("{HEADS}{suggested}"))?;
            let tracked = if exists {
                branches::upstream_of(repo, &suggested)?
            } else {
                None
            };
            let invalid = branches::check_branch(repo.ctx, &suggested).is_err();
            if invalid || tracked.is_some_and(|tracked| tracked != remote_branch) {
                return Ok(RemoteRebase::NeedsName { suggested });
            }
            suggested
        }
    };
    if branches::current_name(repo.path)?.as_deref() == Some(local.as_str()) {
        return Err(branches::invalid(format!(
            "Cannot overwrite current branch {local}"
        )));
    }
    let exists = branches::ref_exists(repo, &format!("{HEADS}{local}"))?;
    if exists && !drop_commits && branches::has_own_commits(repo, remote_branch, &local) {
        return Ok(RemoteRebase::LocalHasCommits { local });
    }
    let mut args = vec!["branch"];
    if exists {
        args.push("-f");
    }
    args.extend([local.as_str(), remote_branch]);
    repo.ctx.git_loud(&args)?;
    Ok(RemoteRebase::Rebased(rebase_branch(
        repo,
        HEAD,
        Some(&local),
    )?))
}

// `GitRebaseParams`: what the Rebase… dialog chooses. The flags travel as booleans and come out in a fixed
// order, that of `GitRebaseOption`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RebaseParams {
    pub upstream: Option<String>,
    // `--onto`: the new base. With it, `upstream` is the old base.
    pub onto: Option<String>,
    // The branch being rebased, if it is not the current one.
    pub branch: Option<String>,
    pub interactive: bool,
    pub rebase_merges: bool,
    pub keep_empty: bool,
    pub root: bool,
    // `--update-refs`: git 2.38, with no version check, as in 241.
    pub update_refs: bool,
}

const REBASE_MERGES: &str = "--rebase-merges";
const PRESERVE_MERGES: &str = "--preserve-merges";
const VERSION_PREFIX: &str = "git version ";

// `GitVersionSpecialty.REBASE_MERGES_REPLACES_PRESERVE_MERGES`: since git 2.22. If the version cannot be read,
// the current one.
fn merges_flag_of(version: &str) -> &'static str {
    let mut numbers = version
        .split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .map(str::parse::<u32>);
    match (numbers.next(), numbers.next()) {
        (Some(Ok(major)), Some(Ok(minor))) if (major, minor) < (2, 22) => PRESERVE_MERGES,
        _ => REBASE_MERGES,
    }
}

fn merges_flag(repo: &Repo) -> &'static str {
    match repo.ctx.git(&["--version"]) {
        Ok(output) => merges_flag_of(output.stdout.trim().trim_start_matches(VERSION_PREFIX)),
        Err(_) => REBASE_MERGES,
    }
}

// A blank text is an absence, like `takeIf { it.isNotBlank() }` of `GitRebaseParams`.
fn present(text: &Option<String>) -> Option<&str> {
    text.as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

// `GitRebaseParams.asCommandLineArguments`: the flags in the order of `GitRebaseOption`, `--onto`, the
// base, `--root` only if there is no base (with one it is omitted, like IntelliJ) and the branch. With
// `--interactive`, `--preserve-merges` does not fit and is omitted.
fn arguments(params: &RebaseParams, merges_flag: &str) -> Vec<String> {
    let mut args = vec!["rebase".to_string()];
    if params.rebase_merges && !(params.interactive && merges_flag == PRESERVE_MERGES) {
        args.push(merges_flag.to_string());
    }
    if params.keep_empty {
        args.push("--keep-empty".to_string());
    }
    if params.interactive {
        args.push("--interactive".to_string());
    }
    if params.update_refs {
        args.push("--update-refs".to_string());
    }
    if let Some(onto) = present(&params.onto) {
        args.extend(["--onto".to_string(), onto.to_string()]);
    }
    let upstream = present(&params.upstream);
    args.extend(upstream.map(str::to_string));
    if params.root && upstream.is_none() {
        args.push("--root".to_string());
    }
    args.extend(present(&params.branch).map(str::to_string));
    args
}

// `GitRebaseDialog.isValidRevision`: for each revision, whether git resolves it to a commit. What starts
// with `-` never reaches git.
pub fn valid_revisions(path: &Path, revisions: &[String]) -> Result<Vec<bool>, GitError> {
    revisions
        .iter()
        .map(|revision| {
            if branches::check_argument(revision, "The revision").is_err() {
                return Ok(false);
            }
            let spec = format!("{revision}^{{commit}}");
            match git::run(path, &["rev-parse", "-q", "--verify", &spec]) {
                Ok(_) => Ok(true),
                Err(GitError::Failed { .. }) => Ok(false),
                Err(error) => Err(error),
            }
        })
        .collect()
}

// The menu's Rebase… through `Process` like `GitRebase.java`. The rebaser works on what the
// dialog saw: if HEAD moved, nothing is touched. As in `rebase_onto`, the ignored files that the rebase
// would overwrite are copied beforehand.
pub fn rebase(
    repo: &Repo,
    params: &RebaseParams,
    expected_head: &str,
) -> Result<Rebased, GitError> {
    operation::check_head(repo.path, expected_head)?;
    operation::check_idle(repo.path)?;
    let (upstream, onto, branch) = (
        present(&params.upstream),
        present(&params.onto),
        present(&params.branch),
    );
    for (what, value) in [
        ("The base", upstream),
        ("The new base", onto),
        ("The branch", branch),
    ] {
        if let Some(value) = value {
            branches::check_argument(value, what)?;
        }
    }
    let upstream_commit = upstream
        .map(|name| branches::commit_of(repo, name))
        .transpose()?;
    let onto_commit = onto
        .map(|name| branches::commit_of(repo, name))
        .transpose()?;
    let tip = match branch {
        Some(name) => {
            branches::check_branch(repo.ctx, name)?;
            branches::commit_of(repo, &format!("{HEADS}{name}"))?
        }
        None => branches::commit_of(repo, HEAD)?,
    };
    // The new base is the one checked out at the start; the old one only bounds the commits.
    let base = onto_commit.or(upstream_commit);
    let flag = if params.rebase_merges {
        merges_flag(repo)
    } else {
        REBASE_MERGES
    };
    let args = arguments(params, flag);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    // `Edit commits before rebasing`: the list git generates goes to the `Rebasing Commits` dialog.
    let editor = if params.interactive {
        rewrite::interactive_editor(repo)
    } else {
        repo.ctx.asking_editor()
    };
    Process::new(repo, editor).run(&args, || copy_in_the_way(repo, &tip, base.as_deref()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_failed_save_says_could_not_stash_or_shelve() {
        let harness = RepoHarness::new();
        let failed = |harness: &RepoHarness| {
            harness.run(|repo| {
                Process::new(repo, repo.ctx.asking_editor())
                    .save_failed(GitError::Io("boom".into()))
            })
        };
        let text = failed(&harness);
        assert!(
            text.starts_with("Could not stash local uncommitted changes:\n"),
            "{text}"
        );
        harness.clean_tree(CleanTree::Shelve);
        let text = failed(&harness);
        assert!(
            text.starts_with("Could not shelve local uncommitted changes:\n"),
            "{text}"
        );
    }

    use super::*;
    use crate::test_support::RepoHarness;

    // `side` has two commits and the first one clashes with `main` in `a.txt`; it stays on `side`.
    fn stopped_by_conflict() -> RepoHarness {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"base\n", "base");
        harness.git(&["checkout", "-q", "-b", "side"]);
        harness.repo.commit("a.txt", b"side\n", "clashes");
        harness.repo.commit("b.txt", b"b\n", "follows");
        harness.git(&["checkout", "-q", "main"]);
        harness.repo.commit("a.txt", b"main\n", "main");
        harness.git(&["checkout", "-q", "side"]);
        harness
    }

    fn started() -> RepoHarness {
        let harness = stopped_by_conflict();
        let outcome =
            harness.run(|repo| rebase_onto(repo, "main", "side").map(|rebased| rebased.outcome));
        assert_eq!(outcome, Ok(Outcome::Conflicts));
        harness
    }

    fn subjects(harness: &RepoHarness) -> Vec<String> {
        let log = harness.git(&["log", "--format=%s"]);
        log.lines().map(str::to_string).collect()
    }

    #[test]
    fn rebase_with_conflict_leaves_it_in_progress_and_abort_goes_back() {
        let harness = stopped_by_conflict();
        let tip = harness.git(&["rev-parse", "side"]);

        let outcome =
            harness.run(|repo| rebase_onto(repo, "main", "side").map(|rebased| rebased.outcome));

        assert_eq!(outcome, Ok(Outcome::Conflicts));
        assert!(harness.in_git_dir("rebase-merge"));
        // Whatever had been resolved by hand would go with the abort's `reset --hard`.
        harness.write("a.txt", "resolved by hand\n");

        assert_eq!(harness.run(abort), Ok(()));

        assert!(!harness.in_git_dir("rebase-merge"));
        assert_eq!(harness.git(&["branch", "--show-current"]), "side\n");
        assert_eq!(harness.head(), tip.trim());
        assert_eq!(harness.saved_paths(), ["a.txt"]);
    }

    #[test]
    fn clean_rebase_finishes() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"base\n", "base");
        harness.git(&["checkout", "-q", "-b", "side"]);
        harness.repo.commit("b.txt", b"b\n", "side");
        harness.git(&["checkout", "-q", "main"]);
        harness.repo.commit("c.txt", b"c\n", "main");

        let outcome =
            harness.run(|repo| rebase_onto(repo, "main", "side").map(|rebased| rebased.outcome));

        assert_eq!(outcome, Ok(Outcome::Done));
        assert_eq!(subjects(&harness), ["side", "main", "base"]);
    }

    #[test]
    fn rebase_already_in_progress_is_not_allowed() {
        let harness = started();

        let outcome = harness.run(|repo| rebase_onto(repo, "main", "side"));

        assert!(
            matches!(&outcome, Ok(Rebased { outcome: Outcome::Failed(text), title: Some(_), .. })
                if text.contains("There is an unfinished rebase process in")),
            "{outcome:?}"
        );
    }

    // Local changes are saved beforehand, even if they clash with what the rebase brings: with the rebase stopped, it stays
    // in the stash.
    #[test]
    fn rebase_that_clashes_with_local_changes_leaves_them_in_the_stash() {
        let harness = stopped_by_conflict();
        harness.write("a.txt", "local\n");

        let rebased = harness
            .run(|repo| rebase_onto(repo, "main", "side"))
            .unwrap();

        assert_eq!(rebased.outcome, Outcome::Conflicts);
        assert_eq!(rebased.saved.len(), 1);
        assert_eq!(rebased.saved[0].restore, None);
        assert_eq!(harness.git(&["show", "stash@{0}:a.txt"]), "local\n");
        assert!(harness.in_git_dir("rebase-merge"));
    }

    #[test]
    fn rebase_with_shelve_says_local_changes_were_shelved_before_rebase_with_view_shelf() {
        let harness = stopped_by_conflict();
        harness.clean_tree(CleanTree::Shelve);
        harness.write("a.txt", "local\n");

        let rebased = harness
            .run(|repo| rebase_onto(repo, "main", "side"))
            .unwrap();

        assert_eq!(rebased.outcome, Outcome::Conflicts);
        assert_eq!(rebased.saved.len(), 1);
        assert_eq!(rebased.saved[0].restore, None);
        let saved = &rebased.saved[0].saved;
        assert_eq!(saved.kind, CleanTree::Shelve);
        assert_eq!(saved.message, "Uncommitted changes before rebase");
        assert_eq!(
            saved.lists[0].description,
            "Uncommitted changes before rebase [Changes]"
        );
        assert_eq!(stash_count(&harness), 0);
        let patch = crate::shelf::load_patch(harness.data.path(), &saved.lists[0].name).unwrap();
        assert_eq!(patch.files.len(), 1);
        assert!(harness.in_git_dir("rebase-merge"));
    }

    // `side` overwrites `ignored.txt` when checked out, which on `main` is ignored and does exist on the branch.
    #[test]
    fn rebase_copies_the_ignored_files_it_overwrites() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"1\n", "one");
        harness.git(&["checkout", "-q", "-b", "t"]);
        harness.repo.commit("ignored.txt", b"from t\n", "two");
        harness.git(&["checkout", "-q", "main"]);
        harness.write(".gitignore", "ignored.txt\n");
        harness.git(&["add", ".gitignore"]);
        harness.git(&["commit", "-q", "-m", "ignore"]);
        harness.write("ignored.txt", "generated\n");

        let outcome =
            harness.run(|repo| rebase_onto(repo, "main", "t").map(|rebased| rebased.outcome));

        assert_eq!(outcome, Ok(Outcome::Done));
        assert_eq!(harness.git(&["branch", "--show-current"]), "t\n");
        assert_eq!(harness.read("ignored.txt"), "from t\n");
        assert_eq!(harness.saved_paths(), ["ignored.txt"]);
    }

    #[test]
    fn rebase_with_untracked_files_that_would_be_overwritten_gives_untracked() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"1\n", "one");
        harness.git(&["checkout", "-q", "-b", "side"]);
        harness.repo.commit("a.txt", b"side\n", "side");
        harness.git(&["checkout", "-q", "main"]);
        harness.repo.commit("new.txt", b"from main\n", "main");
        harness.git(&["checkout", "-q", "side"]);
        harness.write("new.txt", "mine\n");

        let outcome =
            harness.run(|repo| rebase_onto(repo, "main", "side").map(|rebased| rebased.outcome));

        assert_eq!(
            outcome,
            Ok(Outcome::Untracked {
                files: vec!["new.txt".to_string()]
            })
        );
        assert_eq!(harness.read("new.txt"), "mine\n");
    }

    #[test]
    fn continue_with_conflicts_gives_conflicts() {
        let harness = started();

        let outcome = harness.run(continue_rebase);

        assert_eq!(outcome, Ok(Outcome::Conflicts));
        assert!(harness.in_git_dir("rebase-merge"));
    }

    #[test]
    fn skip_skips_the_commit() {
        let harness = started();

        let outcome = harness.run(skip);

        assert_eq!(outcome, Ok(Outcome::Done));
        assert!(!harness.in_git_dir("rebase-merge"));
        assert_eq!(subjects(&harness), ["follows", "main", "base"]);
        assert_eq!(harness.read("a.txt"), "main\n");
    }

    // With `--apply`, resolving a conflict leaving what is already there leaves the commit empty and git says
    // `No changes`: it skips by itself, without asking.
    #[test]
    fn no_changes_skips_by_itself() {
        let harness = stopped_by_conflict();
        let stopped = harness.run(|repo| {
            run(
                repo,
                &["rebase", "--apply", "main", "side"],
                repo.ctx.asking_editor(),
            )
        });
        assert_eq!(stopped, Ok(Outcome::Conflicts));
        assert!(harness.in_git_dir("rebase-apply"));
        harness.write("a.txt", "main\n");
        harness.git(&["add", "a.txt"]);

        let outcome = harness.run(continue_rebase);

        assert_eq!(outcome, Ok(Outcome::Done));
        assert!(harness.ran("rebase --skip"));
        assert!(!harness.in_git_dir("rebase-apply"));
        assert_eq!(subjects(&harness), ["follows", "main", "base"]);
    }

    // An ignored file that `side` does version: `--abort` overwrites it, and the copy knows it only if it looks at
    // the tip of `side` and not at where `ORIG_HEAD` points when another command moves it.
    #[test]
    fn abort_rebase_copies_against_the_rebases_orig_head() {
        let harness = RepoHarness::new();
        harness.write(".gitignore", "new.txt\n");
        harness.repo.commit("a.txt", b"base\n", "base");
        harness.git(&["checkout", "-q", "-b", "side"]);
        harness.repo.commit("a.txt", b"side\n", "clashes");
        harness.write("new.txt", "from side\n");
        harness.git(&["add", "-f", "new.txt"]);
        harness.git(&["commit", "-q", "-m", "adds"]);
        harness.git(&["checkout", "-q", "main"]);
        harness.repo.commit("a.txt", b"main\n", "main");
        harness.git(&["checkout", "-q", "side"]);
        let outcome =
            harness.run(|repo| rebase_onto(repo, "main", "side").map(|rebased| rebased.outcome));
        assert_eq!(outcome, Ok(Outcome::Conflicts));
        harness.write("new.txt", "mine\n");
        // Another command leaves `ORIG_HEAD` on a commit that does not have that file.
        harness.git(&["update-ref", "ORIG_HEAD", "main"]);

        assert_eq!(harness.run(abort), Ok(()));

        assert!(!harness.in_git_dir("rebase-merge"));
        assert_eq!(harness.read("new.txt"), "from side\n");
        assert_eq!(harness.saved_paths(), ["a.txt", "new.txt"]);
    }

    #[test]
    fn abort_without_rebase_fails_and_touches_nothing() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"1\n", "one");
        harness.write("a.txt", "local\n");

        let aborted = harness.run(abort);

        assert_eq!(aborted, Err(GitError::Io("No rebase in progress".into())));
        assert_eq!(harness.read("a.txt"), "local\n");
        assert_eq!(harness.snapshots(), 0);
    }

    #[test]
    fn continue_and_skip_without_rebase_fail() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"1\n", "one");

        assert_eq!(
            harness.run(continue_rebase),
            Err(GitError::Io("No rebase in progress".into()))
        );
        assert_eq!(
            harness.run(skip),
            Err(GitError::Io("No rebase in progress".into()))
        );
    }

    // `side` hangs off `old`, which hangs off `base`; `main` stays at `base`. `--onto main old side`
    // takes only the commit of `side` onto `main`.
    fn nested() -> RepoHarness {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"base\n", "base");
        harness.git(&["checkout", "-q", "-b", "old"]);
        harness.repo.commit("v.txt", b"v\n", "old");
        harness.git(&["checkout", "-q", "-b", "side"]);
        harness.repo.commit("l.txt", b"l\n", "side");
        harness.git(&["checkout", "-q", "main"]);
        harness.repo.commit("m.txt", b"m\n", "main");
        harness.git(&["checkout", "-q", "side"]);
        harness
    }

    fn params(upstream: Option<&str>) -> RebaseParams {
        RebaseParams {
            upstream: upstream.map(str::to_string),
            ..Default::default()
        }
    }

    fn rebase_with(harness: &RepoHarness, params: RebaseParams) -> Result<Outcome, GitError> {
        let head = harness.head();
        harness.run(move |repo| rebase(repo, &params, &head).map(|rebased| rebased.outcome))
    }

    #[test]
    fn rebase_onto_and_root_build_the_command_in_order() {
        let all = RebaseParams {
            upstream: Some("old".into()),
            onto: Some("main".into()),
            branch: Some("side".into()),
            interactive: true,
            rebase_merges: true,
            keep_empty: true,
            root: true,
            update_refs: true,
        };
        let line = |params: &RebaseParams| arguments(params, "--rebase-merges").join(" ");

        // With an old base, `--root` does not fit and is omitted, as in `GitRebaseParams`.
        assert_eq!(
            line(&all),
            "rebase --rebase-merges --keep-empty --interactive --update-refs --onto main old side"
        );
        assert_eq!(
            line(&RebaseParams {
                upstream: None,
                ..all.clone()
            }),
            "rebase --rebase-merges --keep-empty --interactive --update-refs --onto main --root side"
        );
        assert_eq!(line(&params(Some("main"))), "rebase main");
        assert_eq!(
            line(&RebaseParams {
                onto: Some("main".into()),
                ..params(Some("old"))
            }),
            "rebase --onto main old"
        );
        // Blank texts are absences.
        assert_eq!(
            line(&RebaseParams {
                onto: Some("  ".into()),
                branch: Some(String::new()),
                ..params(Some("main"))
            }),
            "rebase main"
        );
        // Without `--rebase-merges` (git < 2.22) `--preserve-merges` is used, except with `--interactive`.
        let merges = RebaseParams {
            rebase_merges: true,
            ..params(Some("main"))
        };
        assert_eq!(
            arguments(&merges, "--preserve-merges").join(" "),
            "rebase --preserve-merges main"
        );
        let interactive = RebaseParams {
            interactive: true,
            ..merges
        };
        assert_eq!(
            arguments(&interactive, "--preserve-merges").join(" "),
            "rebase --interactive main"
        );
        assert_eq!(merges_flag_of("2.21.0"), "--preserve-merges");
        assert_eq!(merges_flag_of("2.22.0"), "--rebase-merges");
        assert_eq!(merges_flag_of("2.39.5 (Apple Git-154)"), "--rebase-merges");
        assert_eq!(merges_flag_of("weird"), "--rebase-merges");
    }

    #[test]
    fn rebase_with_update_refs_passes_the_flag() {
        let harness = nested();
        let params = RebaseParams {
            update_refs: true,
            ..params(Some("main"))
        };
        assert_eq!(
            arguments(&params, "--rebase-merges").join(" "),
            "rebase --update-refs main"
        );

        let outcome = rebase_with(&harness, params);

        assert_eq!(outcome, Ok(Outcome::Done));
        assert!(harness.ran("rebase --update-refs main"));
    }

    #[test]
    fn rebase_with_onto_takes_only_the_branch_commits() {
        let harness = nested();
        let params = RebaseParams {
            onto: Some("main".into()),
            ..params(Some("old"))
        };

        let outcome = rebase_with(&harness, params);

        assert_eq!(outcome, Ok(Outcome::Done));
        assert!(harness.ran("rebase --onto main old"));
        assert_eq!(subjects(&harness), ["side", "main", "base"]);
    }

    #[test]
    fn rebase_of_another_branch_leaves_it_current() {
        let harness = nested();
        harness.git(&["checkout", "-q", "main"]);
        let params = RebaseParams {
            branch: Some("side".into()),
            ..params(Some("main"))
        };

        let outcome = rebase_with(&harness, params);

        assert_eq!(outcome, Ok(Outcome::Done));
        assert!(harness.ran("rebase main side"));
        assert_eq!(harness.git(&["branch", "--show-current"]), "side\n");
        assert_eq!(subjects(&harness), ["side", "old", "main", "base"]);
    }

    // A branch unrelated to `main`: `--root` takes all of its commits.
    #[test]
    fn rebase_root_takes_all_the_branch_commits() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"base\n", "base");
        harness.git(&["checkout", "-q", "--orphan", "other"]);
        harness.git(&["rm", "-rf", "-q", "."]);
        harness.repo.commit("o.txt", b"o\n", "o1");
        harness.repo.commit("p.txt", b"p\n", "o2");
        let params = RebaseParams {
            onto: Some("main".into()),
            root: true,
            ..Default::default()
        };

        let outcome = rebase_with(&harness, params);

        assert_eq!(outcome, Ok(Outcome::Done));
        assert!(harness.ran("rebase --onto main --root"));
        assert_eq!(subjects(&harness), ["o2", "o1", "base"]);
    }

    // `GitRebase.java` goes through `GitRebaseUtils.rebase`: the menu's Rebase… saves what is local and
    // restores it like the popup ones.
    #[test]
    fn rebase_from_the_dialog_with_local_changes_saves_and_restores_them() {
        let harness = nested();
        harness.write("l.txt", "mine\n");
        let head = harness.head();

        let rebased = harness
            .run(move |repo| rebase(repo, &params(Some("main")), &head))
            .unwrap();

        assert_eq!(rebased.outcome, Outcome::Done);
        assert_eq!(rebased.saved.len(), 1);
        assert_eq!(rebased.saved[0].restore, Some(RestoreOutcome::Restored));
        assert_eq!(harness.read("l.txt"), "mine\n");
        assert_eq!(subjects(&harness), ["side", "old", "main", "base"]);
        assert_eq!(stash_count(&harness), 0);
    }

    #[test]
    fn rebase_with_conflict_gives_conflicts() {
        let harness = stopped_by_conflict();

        let outcome = rebase_with(&harness, params(Some("main")));

        assert_eq!(outcome, Ok(Outcome::Conflicts));
        assert!(harness.in_git_dir("rebase-merge"));
    }

    #[test]
    fn rebase_copies_the_ignored_files_it_overwrites_like_rebase_onto() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"1\n", "one");
        harness.git(&["checkout", "-q", "-b", "t"]);
        harness.repo.commit("ignored.txt", b"from t\n", "two");
        harness.git(&["checkout", "-q", "main"]);
        harness.write(".gitignore", "ignored.txt\n");
        harness.git(&["add", ".gitignore"]);
        harness.git(&["commit", "-q", "-m", "ignore"]);
        harness.write("ignored.txt", "generated\n");
        let params = RebaseParams {
            branch: Some("t".into()),
            ..params(Some("main"))
        };

        let outcome = rebase_with(&harness, params);

        assert_eq!(outcome, Ok(Outcome::Done));
        assert_eq!(harness.read("ignored.txt"), "from t\n");
        assert_eq!(harness.saved_paths(), ["ignored.txt"]);
    }

    #[test]
    fn rebase_rejects_what_starts_with_a_dash_or_does_not_exist() {
        let harness = nested();
        let head = harness.head();
        let bad = |params: RebaseParams| {
            let head = head.clone();
            harness.run(move |repo| rebase(repo, &params, &head).is_err())
        };

        assert!(bad(params(Some("-x"))));
        assert!(bad(params(Some("--root"))));
        assert!(bad(RebaseParams {
            onto: Some("-x".into()),
            ..params(Some("main"))
        }));
        assert!(bad(RebaseParams {
            branch: Some("-y".into()),
            ..params(Some("main"))
        }));
        assert!(bad(params(Some("does-not-exist"))));
        // An old base that does not exist is rejected even if there is a valid `--onto`.
        assert!(bad(RebaseParams {
            onto: Some("main".into()),
            ..params(Some("does-not-exist"))
        }));
        assert!(bad(RebaseParams {
            onto: Some("does-not-exist".into()),
            ..params(Some("main"))
        }));
        assert!(bad(RebaseParams {
            branch: Some("does-not-exist".into()),
            ..params(Some("main"))
        }));
        assert!(!harness.ran("rebase -x"));
        assert!(!harness.ran("rebase --root"));
        assert!(!harness.ran("does-not-exist"));
    }

    #[test]
    fn valid_revisions_says_which_ones_git_resolves() {
        let harness = nested();
        harness.git(&["tag", "v1"]);
        let asked = ["main", "v1", "HEAD~1", "does-not-exist", "-x", "--root", ""]
            .map(String::from)
            .to_vec();

        let valid = valid_revisions(harness.path(), &asked);

        assert_eq!(
            valid,
            Ok(vec![true, true, true, false, false, false, false])
        );
    }

    #[test]
    fn rebase_checks_head() {
        let harness = nested();
        let before = harness.head();

        let outcome = harness.run(|repo| {
            rebase(repo, &params(Some("main")), &"0".repeat(40)).map(|rebased| rebased.outcome)
        });

        assert_eq!(
            outcome,
            Err(GitError::Io("Repository has already been changed".into()))
        );
        assert_eq!(harness.head(), before);
        assert!(!harness.ran("rebase main"));
    }

    // The interface reads these shapes: `kind` and, if it carries data, `reason`.
    #[test]
    fn the_result_travels_as_the_interface_expects() {
        let json = |outcome: Outcome| serde_json::to_value(outcome).unwrap();

        assert_eq!(json(Outcome::Done), serde_json::json!({ "kind": "done" }));
        assert_eq!(
            json(Outcome::Conflicts),
            serde_json::json!({ "kind": "conflicts" })
        );
        assert_eq!(
            json(Outcome::Editing),
            serde_json::json!({ "kind": "editing" })
        );
        assert_eq!(
            json(Outcome::Untracked {
                files: vec!["a".into()]
            }),
            serde_json::json!({ "kind": "untracked", "reason": { "files": ["a"] } })
        );
        assert_eq!(
            json(Outcome::Failed("bad".into())),
            serde_json::json!({ "kind": "failed", "reason": "bad" })
        );
    }

    // `Process`. `side` hangs off `base` with `l.txt`; `main` adds `m.txt`. It stays on `side`, which
    // also versions `a.txt` and `b.txt`.
    fn apart() -> RepoHarness {
        apart_with(RepoHarness::new())
    }

    fn apart_with(harness: RepoHarness) -> RepoHarness {
        harness.repo.commit("a.txt", b"1\n2\n3\n", "base");
        harness.repo.commit("b.txt", b"b\n", "b");
        harness.git(&["checkout", "-q", "-b", "side"]);
        harness.repo.commit("l.txt", b"l\n", "side");
        harness.git(&["checkout", "-q", "main"]);
        harness.repo.commit("m.txt", b"m\n", "main");
        harness.git(&["checkout", "-q", "side"]);
        harness
    }

    // A `git` that goes through a `case " $* " in` before the real one. `{dir}` is a folder of the
    // test for marks and counts.
    fn faked(body: &str) -> (RepoHarness, tempfile::TempDir) {
        use std::os::unix::fs::PermissionsExt;
        let scripts = tempfile::TempDir::new().unwrap();
        let script = scripts.path().join("git");
        let body = body.replace("{dir}", scripts.path().to_str().unwrap());
        let text = format!("#!/bin/sh\ncase \" $* \" in\n{body}\n*) exec git \"$@\" ;;\nesac\n");
        std::fs::write(&script, text).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        (RepoHarness::with_binary(Some(script)), scripts)
    }

    fn rebase_side_onto_main(harness: &RepoHarness) -> Result<Rebased, GitError> {
        let head = harness.head();
        harness.run(move |repo| branch_rebase(repo, "main", None, &head))
    }

    fn stash_count(harness: &RepoHarness) -> usize {
        harness
            .git(&["stash", "list", "--format=%H"])
            .lines()
            .count()
    }

    fn restored(rebased: &Rebased) -> Vec<Option<RestoreOutcome>> {
        rebased
            .saved
            .iter()
            .map(|kept| kept.restore.clone())
            .collect()
    }

    #[test]
    fn rebase_saves_any_change_and_restores() {
        let harness = apart();
        harness.write("a.txt", "1\n2\nmine\n");
        harness.write("new.txt", "staged\n");
        harness.git(&["add", "new.txt"]);

        let rebased = rebase_side_onto_main(&harness).unwrap();

        assert_eq!(rebased.outcome, Outcome::Done);
        assert_eq!(rebased.title, None);
        assert_eq!(subjects(&harness), ["side", "main", "b", "base"]);
        assert_eq!(rebased.saved.len(), 1);
        assert_eq!(
            rebased.saved[0].saved.message,
            "Uncommitted changes before rebase"
        );
        assert_eq!(restored(&rebased), [Some(RestoreOutcome::Restored)]);
        assert_eq!(harness.read("a.txt"), "1\n2\nmine\n");
        assert_eq!(harness.read("new.txt"), "staged\n");
        assert_eq!(stash_count(&harness), 0);
        assert_eq!(
            preserve::pending(harness.path(), harness.data.path()),
            Ok(Vec::new())
        );
    }

    #[test]
    fn rebase_without_changes_saves_nothing() {
        let harness = apart();

        let rebased = rebase_side_onto_main(&harness).unwrap();

        assert_eq!(rebased.outcome, Outcome::Done);
        assert_eq!(rebased.saved, []);
        assert!(!harness.ran("stash save Uncommitted changes before rebase"));
    }

    // The first time the rebase is launched, Devin writes to `b.txt` right before: git sees the tree
    // dirty, it is saved again and repeated. If it happens again, it does not insist.
    #[test]
    fn rebase_retries_once_if_git_sees_a_dirty_tree() {
        let (harness, dir) = faked(
            "*\" rebase main \"*)\n  echo x >> '{dir}/count'\n  if [ ! -e '{dir}/mark' ]; then : > '{dir}/mark'; printf 'devin\\n' > b.txt; fi\n  exec git \"$@\" ;;",
        );
        let harness = apart_with(harness);
        harness.write("a.txt", "1\n2\nmine\n");

        let rebased = rebase_side_onto_main(&harness).unwrap();

        assert_eq!(rebased.outcome, Outcome::Done);
        let count = std::fs::read_to_string(dir.path().join("count")).unwrap();
        assert_eq!(count.lines().count(), 2);
        assert_eq!(subjects(&harness), ["side", "main", "b", "base"]);
        assert_eq!(rebased.saved.len(), 2);
        assert_eq!(
            restored(&rebased),
            [
                Some(RestoreOutcome::Restored),
                Some(RestoreOutcome::Restored)
            ]
        );
        assert_eq!(harness.read("a.txt"), "1\n2\nmine\n");
        assert_eq!(harness.read("b.txt"), "devin\n");
        assert_eq!(stash_count(&harness), 0);

        // Always dirty: the second time, git's error.
        let (stubborn, dir) = faked(
            "*\" rebase main \"*)\n  echo x >> '{dir}/count'\n  date +%N >> b.txt\n  exec git \"$@\" ;;",
        );
        let stubborn = apart_with(stubborn);

        let rebased = rebase_side_onto_main(&stubborn).unwrap();

        assert!(
            matches!(&rebased.outcome, Outcome::Failed(text) if text.contains("unstaged changes")),
            "{rebased:?}"
        );
        let count = std::fs::read_to_string(dir.path().join("count")).unwrap();
        assert_eq!(count.lines().count(), 2);
        assert!(!stubborn.in_git_dir("rebase-merge"));
    }

    // Devin writes to `a.txt` while the rebase lasts, and the user had changes there: git does not
    // overwrite Devin's when restoring. The entry stays in the stash and `Restore Changes` brings it afterwards.
    #[test]
    fn devin_writes_during_the_rebase() {
        let (harness, _dir) = faked(
            "*\" rebase main \"*)\n  git \"$@\"\n  status=$?\n  printf 'devin\\n' > a.txt\n  exit $status\n  ;;",
        );
        let harness = apart_with(harness);
        harness.write("a.txt", "1\n2\nmine\n");

        let rebased = rebase_side_onto_main(&harness).unwrap();

        assert_eq!(rebased.outcome, Outcome::Done);
        assert_eq!(
            restored(&rebased),
            [Some(RestoreOutcome::RestoreBlocked {
                files: vec!["a.txt".to_string()]
            })]
        );
        let saved = rebased.saved[0].saved.clone();
        assert_eq!(harness.git(&["rev-parse", "stash@{0}"]).trim(), saved.sha);
        assert_eq!(harness.git(&["show", "stash@{0}:a.txt"]), "1\n2\nmine\n");
        assert_eq!(harness.read("a.txt"), "devin\n");
        assert_eq!(
            preserve::pending(harness.path(), harness.data.path())
                .unwrap()
                .len(),
            0
        );

        harness.git(&["checkout", "-q", "--", "a.txt"]);
        let outcome = harness.run(move |repo| update::restore_requested(repo, &saved));
        assert_eq!(outcome, RestoreOutcome::Restored);
        assert_eq!(harness.read("a.txt"), "1\n2\nmine\n");
    }

    // With conflicts, what was saved stays in the stash until the rebase finishes.
    #[test]
    fn rebase_with_conflicts_leaves_the_saved_changes_in_the_stash() {
        let harness = stopped_by_conflict();
        harness.write("b.txt", "change\n");

        let head = harness.head();
        let rebased = harness
            .run(move |repo| branch_rebase(repo, "main", None, &head))
            .unwrap();

        assert_eq!(rebased.outcome, Outcome::Conflicts);
        assert_eq!(restored(&rebased), [None]);
        assert_eq!(stash_count(&harness), 1);
    }

    #[test]
    fn rebase_not_allowed_with_a_merge_in_progress_gives_intellijs_text() {
        let harness = RepoHarness::new();
        harness.diverged();
        harness.git_fails(&["merge", "side"]);
        harness.write("another.txt", "loose\n");
        let name = harness
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();

        let rebased = harness.run({
            let head = harness.head();
            move |repo| branch_rebase(repo, "side", None, &head)
        });

        assert_eq!(
            rebased,
            Ok(Rebased {
                outcome: Outcome::Failed(format!(
                    "There is an unfinished merge process in {name}.\nYou should complete the merge before starting a rebase."
                )),
                title: Some("Rebase not allowed".to_string()),
                saved: Vec::new(),
            })
        );
        assert!(!harness.ran("rebase side"));
        assert_eq!(stash_count(&harness), 0);
        assert_eq!(harness.snapshots(), 0);
    }

    #[test]
    fn rebase_not_allowed_with_detached_head() {
        let harness = apart();
        harness.git(&["checkout", "-q", "--detach"]);
        let name = harness
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();

        let rebased = rebase_side_onto_main(&harness).unwrap();

        assert_eq!(rebased.title.as_deref(), Some("Rebase not allowed"));
        assert_eq!(
            rebased.outcome,
            Outcome::Failed(format!(
                "You are in the detached HEAD state in {name}.\nRebase is not possible."
            ))
        );
    }

    #[test]
    fn rebase_y_onto_x_is_rebase_x() {
        let harness = apart();

        let rebased = rebase_side_onto_main(&harness).unwrap();

        assert_eq!(rebased.outcome, Outcome::Done);
        assert!(harness.ran("rebase main"));
        assert_eq!(harness.git(&["branch", "--show-current"]), "side\n");
    }

    #[test]
    fn checkout_and_rebase_local_is_rebase_head_x() {
        let harness = apart();
        harness.git(&["checkout", "-q", "main"]);
        let head = harness.head();

        let rebased = harness
            .run(move |repo| branch_rebase(repo, "HEAD", Some("side"), &head))
            .unwrap();

        assert_eq!(rebased.outcome, Outcome::Done);
        assert!(harness.ran("rebase HEAD side"));
        assert_eq!(harness.git(&["branch", "--show-current"]), "side\n");
        assert_eq!(subjects(&harness), ["side", "main", "b", "base"]);
    }

    #[test]
    fn branch_rebase_checks_head_and_what_starts_with_a_dash() {
        let harness = apart();
        let head = harness.head();

        let results = harness.run(move |repo| {
            [
                branch_rebase(repo, "main", None, &"0".repeat(40)).is_err(),
                branch_rebase(repo, "-x", None, &head).is_err(),
                branch_rebase(repo, "main", Some("-y"), &head).is_err(),
                branch_rebase(repo, "does-not-exist", None, &head).is_err(),
            ]
        });

        assert_eq!(results, [true; 4]);
        assert!(!harness.ran("rebase main"));
    }

    // `origin` is a bare remote with `main` and `feature`, which branches off `base` with `f.txt`.
    fn with_origin(harness: &RepoHarness) -> tempfile::TempDir {
        let origin = tempfile::TempDir::new().unwrap();
        let init = crate::test_support::git_command()
            .args(["init", "-q", "--bare"])
            .arg(origin.path())
            .status()
            .unwrap();
        assert!(init.success());
        harness.git(&["remote", "add", "origin", origin.path().to_str().unwrap()]);
        origin
    }

    fn remote_feature() -> (RepoHarness, tempfile::TempDir) {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"1\n", "base");
        let origin = with_origin(&harness);
        harness.git(&["checkout", "-q", "-b", "feature"]);
        harness.repo.commit("f.txt", b"f\n", "feature");
        harness.git(&["push", "-q", "origin", "main", "feature"]);
        harness.git(&["checkout", "-q", "main"]);
        harness.git(&["branch", "-q", "-D", "feature"]);
        harness.git(&["fetch", "-q", "origin"]);
        harness.repo.commit("m.txt", b"m\n", "main");
        (harness, origin)
    }

    fn rebase_feature(
        harness: &RepoHarness,
        name: Option<&str>,
        drop_commits: bool,
    ) -> Result<RemoteRebase, GitError> {
        let head = harness.head();
        let name = name.map(str::to_string);
        harness.run(move |repo| {
            rebase_remote(repo, "origin/feature", name.as_deref(), drop_commits, &head)
        })
    }

    #[test]
    fn checkout_and_rebase_remote_creates_the_local_one_first() {
        let (harness, _origin) = remote_feature();

        let outcome = rebase_feature(&harness, None, false);

        assert!(
            matches!(
                &outcome,
                Ok(RemoteRebase::Rebased(Rebased {
                    outcome: Outcome::Done,
                    ..
                }))
            ),
            "{outcome:?}"
        );
        assert!(harness.ran("branch feature origin/feature"));
        assert!(harness.ran("rebase HEAD feature"));
        assert_eq!(harness.git(&["branch", "--show-current"]), "feature\n");
        assert_eq!(
            harness.git(&["rev-parse", "--abbrev-ref", "feature@{upstream}"]),
            "origin/feature\n"
        );
        assert_eq!(subjects(&harness), ["feature", "main", "base"]);
    }

    #[test]
    fn checkout_and_rebase_remote_asks_like_intellij() {
        // A local one that tracks something else: the name is asked.
        let (harness, _origin) = remote_feature();
        harness.git(&["branch", "-q", "--track", "feature", "origin/main"]);
        assert_eq!(
            rebase_feature(&harness, None, false),
            Ok(RemoteRebase::NeedsName {
                suggested: "feature".to_string()
            })
        );

        // A local one with its own commits: it is asked whether to throw them away.
        let (harness, _origin) = remote_feature();
        harness.git(&["branch", "-q", "--track", "feature", "origin/feature"]);
        harness.git(&["checkout", "-q", "feature"]);
        harness.repo.commit("own.txt", b"p\n", "own");
        harness.git(&["checkout", "-q", "main"]);
        assert_eq!(
            rebase_feature(&harness, None, false),
            Ok(RemoteRebase::LocalHasCommits {
                local: "feature".to_string()
            })
        );
        let outcome = rebase_feature(&harness, None, true);
        assert!(
            matches!(outcome, Ok(RemoteRebase::Rebased(_))),
            "{outcome:?}"
        );
        assert_eq!(subjects(&harness), ["feature", "main", "base"]);

        // The current one is not overwritten.
        let (harness, _origin) = remote_feature();
        harness.git(&["checkout", "-q", "-b", "other"]);
        assert_eq!(
            rebase_feature(&harness, Some("other"), false),
            Err(GitError::Io("Cannot overwrite current branch other".into()))
        );
    }

    // `origin/main` already has the commit `published`, which is also in `side`.
    #[test]
    fn published_sees_a_commit_in_origin_main() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"1\n", "base");
        let _origin = with_origin(&harness);
        harness.git(&["checkout", "-q", "-b", "side"]);
        harness.repo.commit("p.txt", b"p\n", "published");
        harness.git(&["push", "-q", "origin", "side:main", "side:feature"]);
        harness.repo.commit("l.txt", b"l\n", "local");
        harness.git(&["fetch", "-q", "origin"]);
        let published_of = |upstream: Option<&'static str>| {
            harness.run(move |repo| published(repo, upstream, "side"))
        };

        assert_eq!(
            published_of(Some("main")),
            Ok(Some("origin/main".to_string()))
        );
        assert_eq!(published_of(None), Ok(Some("origin/main".to_string())));
        // What is already in the base is not rewritten.
        assert_eq!(published_of(Some("origin/main")), Ok(None));

        // With other patterns, `main` is no longer protected and `feature` is.
        settings::save_protected_branches(harness.data.path(), vec!["release.*".into()]).unwrap();
        assert_eq!(published_of(Some("main")), Ok(None));
        settings::save_protected_branches(harness.data.path(), vec!["feat.*".into()]).unwrap();
        assert_eq!(
            published_of(Some("main")),
            Ok(Some("origin/feature".to_string()))
        );

        assert!(harness
            .run(|repo| published(repo, Some("-x"), "side"))
            .is_err());
        assert!(harness.run(|repo| published(repo, None, "--all")).is_err());
    }

    #[test]
    fn what_starts_with_a_dash_does_not_reach_git_as_an_option() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"1\n", "one");

        let results = harness.run(|repo| {
            vec![
                rebase_onto(repo, "-x", "y").is_err(),
                rebase_onto(repo, "x", "-y").is_err(),
            ]
        });

        assert_eq!(results, [true, true]);
        assert!(!harness.ran("rebase -x y"));
    }
}
