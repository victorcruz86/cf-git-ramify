// Rewriting history from the log: Edit Commit Message, Drop Commits and Squash Commits, and their
// Undo. Ported from `GitCommitEditingActionBase`, `GitSingleCommitEditingAction`,
// `GitCommitEditingOperation`, `GitCommitEditingEditorHandler`, `GitCommitEditingOperationResult`,
// `GitRewordAction`, `GitRewordOperation`, `GitSquashOperation`, `GitDropOperation`,
// `GitAutomaticRebaseEditor`, `GitInteractiveRebaseEditorHandler.processModel` and
// `GitRewordedCommitMessageProvider`; and Undo Commit, Fixup… and Squash Into…: `GitUncommitAction`,
// `GitResetOperation`, `GitAutoSquashCommitAction` and `GitRebaseCheckinHandlerFactory`. The messages
// live in the `Rewriter`, not on disk.
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};

use crate::branches::{self, CheckoutMode, CheckoutOutcome, Repo, ResetMode};
use crate::commit;
use crate::editor::{Ask, EditKind, EditorHandler, Form, Prompt, Request};
use crate::git::{self, GitError};
use crate::history;
use crate::operation::{self, Operation};
use crate::rebase::{self, Kept, Outcome, Process, Rebased};
use crate::settings;
use crate::todo::{self, Action, Entry, Model, TodoError};

const HEAD: &str = "HEAD";
const REPOSITORY_CHANGED: &str = "Repository has already been changed";
const NOT_IN_HEAD: &str = "The commit is not in the current branch";
// `rebase.log.action.operation.reword.name`.
const REWORD: &str = "reword";
const ONLY_FROM_REWORDED: &str = "Full message should be taken from reworded commits only";
// `git.undo.action.description`.
const NOT_LAST: &str = "The selected commit is not the last in the current branch";
// `git.undo.action.failed.notification.title`, for a reset that gave no further reason.
const UNDO_FAILED: &str = "Undo commit failed";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Reword,
    Drop,
    Squash,
}

// `GitCommitEditingOperationResult.Complete`, with what `rebase::Process` reports of what was saved:
// with `title`, the rebase never got to start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rewritten {
    pub old_head: String,
    pub new_head: String,
    pub outcome: Outcome,
    pub title: Option<String>,
    pub saved: Vec<Kept>,
}

impl Rewritten {
    // What the Undo needs: the two HEADs.
    pub fn between(old_head: String, new_head: String) -> Self {
        Self {
            old_head,
            new_head,
            outcome: Outcome::Done,
            title: None,
            saved: Vec::new(),
        }
    }
}

// A message that git will ask for on a `reword`: it is answered if the text starts with the original
// (`GitRewordedCommitMessageProvider.getRewordedCommitMessage`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reworded {
    pub original: String,
    pub message: String,
}

type TodoEdit = dyn Fn(Vec<Entry>) -> Result<(Vec<Entry>, Vec<Reworded>), TodoError> + Send + Sync;

// The editor of a rewrite: the first list is changed by `todo`, and the messages of the `reword`s
// come from what was saved. What it does not expect goes to `fallback` (the window), unless it is `strict`:
// `GitRewordOperation.supplyNewMessage` fails on a text it did not expect.
pub struct Rewriter {
    todo: Box<TodoEdit>,
    strict: bool,
    fallback: Arc<dyn EditorHandler>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    written: bool,
    reworded: Vec<Reworded>,
    next: usize,
    failure: Option<String>,
}

enum Answer {
    Mine(Option<String>),
    Ask,
}

impl Rewriter {
    pub fn new(
        todo: impl Fn(Vec<Entry>) -> Result<(Vec<Entry>, Vec<Reworded>), TodoError>
            + Send
            + Sync
            + 'static,
        strict: bool,
        fallback: Arc<dyn EditorHandler>,
    ) -> Self {
        Self {
            todo: Box::new(todo),
            strict,
            fallback,
            state: Mutex::default(),
        }
    }

    // Why the list was cancelled, if it was the model's doing.
    pub fn failure(&self) -> Option<String> {
        self.lock().failure.clone()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn answer(&self, kind: EditKind, text: &str) -> Answer {
        let mut state = self.lock();
        match kind {
            EditKind::Todo if !state.written => {
                state.written = true;
                match todo::parse(text).and_then(|entries| (self.todo)(entries)) {
                    Ok((entries, reworded)) => {
                        state.reworded = reworded;
                        state.next = 0;
                        Answer::Mine(Some(todo::write(&entries)))
                    }
                    // `confirmNoopRebase` and `handleUnstructuredEditor`: whoever asks decides them.
                    Err(TodoError::Noop | TodoError::Unstructured) => Answer::Ask,
                    Err(TodoError::Invalid(reason)) => {
                        state.failure = Some(reason);
                        Answer::Mine(None)
                    }
                }
            }
            EditKind::Todo => Answer::Ask,
            EditKind::Message => match state.reworded.get(state.next) {
                Some(reworded) if text.starts_with(&reworded.original) => {
                    let message = reworded.message.clone();
                    state.next += 1;
                    Answer::Mine(Some(message))
                }
                _ if self.strict => Answer::Mine(None),
                _ => Answer::Ask,
            },
        }
    }
}

// It also delegates `edit_request`: so an `Ask` stays attentive to the editor going away.
impl EditorHandler for Rewriter {
    fn edit(&self, kind: EditKind, text: &str) -> Option<String> {
        match self.answer(kind, text) {
            Answer::Mine(answer) => answer,
            Answer::Ask => self.fallback.edit(kind, text),
        }
    }

    fn edit_request(&self, request: &Request, kind: EditKind, text: &str) -> Option<String> {
        match self.answer(kind, text) {
            Answer::Mine(answer) => answer,
            Answer::Ask => self.fallback.edit_request(request, kind, text),
        }
    }
}

// Why it cannot be done: IntelliJ's text, or a git failure while looking.
enum Refused {
    Reason(String),
    Git(GitError),
}

impl From<GitError> for Refused {
    fn from(error: GitError) -> Self {
        Self::Git(error)
    }
}

impl Refused {
    fn text(self) -> String {
        match self {
            Self::Reason(text) => text,
            Self::Git(error) => branches::text_of(error),
        }
    }

    fn into_error(self) -> GitError {
        match self {
            Self::Reason(text) => branches::invalid(text),
            Self::Git(error) => error,
        }
    }
}

fn refuse<T>(text: String) -> Result<T, Refused> {
    Err(Refused::Reason(text))
}

// `{0,choice,1#commit|2#commits}`.
fn commits(count: usize) -> &'static str {
    if count > 1 {
        "commits"
    } else {
        "commit"
    }
}

// `rebase.log.commit.editing.action.disabled.parents.description`.
fn parents_text(count: usize) -> String {
    format!("Selected commit has {count} parents")
}

// `rebase.log.commit.editing.action.commit.pushed.to.protected.branch.error.text`.
fn pushed_text(branch: &str) -> String {
    format!("The commit is already pushed to protected branch '{branch}'")
}

// `rebase.log.reword.dialog.failed.repository.changed.message`: what the message dialog sees.
fn dialog_changed(count: usize) -> String {
    format!(
        "Cannot edit selected {}: repository state was changed",
        commits(count)
    )
}

fn head_of(repo: &Repo) -> Result<Option<String>, GitError> {
    match repo.ctx.git(&["rev-parse", "-q", "--verify", HEAD]) {
        Ok(output) => Ok(Some(output.stdout.trim().to_string())),
        Err(GitError::Failed { code: Some(1), .. }) => Ok(None),
        Err(error) => Err(error),
    }
}

fn is_ancestor(repo: &Repo, commit: &str, of: &str) -> Result<bool, GitError> {
    match repo.ctx.git(&["merge-base", "--is-ancestor", commit, of]) {
        Ok(_) => Ok(true),
        Err(GitError::Failed { code: Some(1), .. }) => Ok(false),
        Err(error) => Err(error),
    }
}

// `findProtectedRemoteBranch` with the branches that contain the commit: the protected remote, with the
// short name (`origin/main`).
fn protected_branch(repo: &Repo, commit: &str) -> Result<Option<String>, GitError> {
    let patterns = settings::effective_protected(repo.data);
    for (full, short) in rebase::protected_remotes(repo.path, &patterns)? {
        if is_ancestor(repo, commit, &full)? {
            return Ok(Some(short));
        }
    }
    Ok(None)
}

struct Selected {
    hash: String,
    parents: usize,
}

// The chosen commits, complete and with their parents, in a single command and in the given order.
fn selected(repo: &Repo, hashes: &[String]) -> Result<Vec<Selected>, GitError> {
    let mut unique: Vec<&str> = Vec::new();
    for hash in hashes {
        branches::check_argument(hash, "The commit")?;
        if !unique.contains(&hash.as_str()) {
            unique.push(hash);
        }
    }
    let mut args = vec![
        "rev-list",
        "--no-walk=unsorted",
        "--parents",
        "--end-of-options",
    ];
    args.extend(&unique);
    args.push("--");
    let output = match repo.ctx.git(&args) {
        Ok(output) => output,
        Err(GitError::Failed { .. }) => {
            return Err(branches::invalid(format!(
                "Could not find \"{}\"",
                unique.join(" ")
            )))
        }
        Err(error) => return Err(error),
    };
    Ok(output
        .stdout
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let hash = parts.next()?.to_string();
            Some(Selected {
                hash,
                parents: parts.count(),
            })
        })
        .collect())
}

// `getProhibitedStateMessage`, with the action's operation. The one of `GitRewordAction` lets HEAD be edited
// during a rebase (`head_in_rebase`).
fn prohibited(repo: &Repo, operation: &str, head_in_rebase: bool) -> Result<(), Refused> {
    let during = match operation::current(repo.path)? {
        None => return Ok(()),
        Some(Operation::Rebasing) if head_in_rebase => return Ok(()),
        Some(Operation::Rebasing) => "rebase",
        Some(Operation::Merging) => "merge",
        Some(Operation::CherryPicking) => "cherry-pick",
        Some(Operation::Reverting) => "revert",
    };
    refuse(format!("Cannot {operation} during {during}"))
}

// Which action asks: those of several commits and Edit Commit Message according to their `Kind`, and those of a single
// commit as they are (`GitSingleCommitEditingAction`: Undo Commit, Fixup… and Squash Into…), which
// prohibit no state in progress and do not let a merge in HEAD be edited.
#[derive(Clone, Copy)]
enum Mode {
    Of(Kind),
    Single,
}

// `GitCommitEditingActionBase.update`: what the menu checks. Returns HEAD and the commits.
fn quick(repo: &Repo, hashes: &[String], kind: Kind) -> Result<(String, Vec<Selected>), Refused> {
    checked(repo, hashes, Mode::Of(kind))
}

fn checked(repo: &Repo, hashes: &[String], mode: Mode) -> Result<(String, Vec<Selected>), Refused> {
    let count = hashes.len();
    match mode {
        Mode::Of(Kind::Reword) if count != 1 => {
            return refuse("Select a single commit".to_string())
        }
        Mode::Of(Kind::Squash) if count < 2 => {
            return refuse("Select at least two commits".to_string())
        }
        Mode::Single if count != 1 => return refuse("Select a single commit".to_string()),
        _ if count == 0 => return refuse("Select at least one commit".to_string()),
        _ => {}
    }
    // `rebase.log.multiple.commit.editing.action.cant.find.head`.
    let Some(head) = head_of(repo)? else {
        return refuse(format!(
            "Could not find HEAD branch in repository which contains selected {}",
            commits(count)
        ));
    };
    let commits = selected(repo, hashes)?;
    let (Some(first), Some(last)) = (commits.first(), commits.last()) else {
        return refuse("Select at least one commit".to_string());
    };
    if commits.iter().any(|commit| commit.parents == 0) {
        return refuse(parents_text(0));
    }
    // `GitRewordAction.checkNotMergeCommit`: a merge in HEAD is edited with amend.
    let reword = matches!(mode, Mode::Of(Kind::Reword));
    let is_head = reword && first.hash == head;
    if let Some(merge) = commits.iter().find(|commit| commit.parents > 1) {
        if !is_head {
            return refuse(parents_text(merge.parents));
        }
    }
    // `checkCommitsCanBeEdited`: the first and the last.
    for commit in [first, last] {
        if !is_ancestor(repo, &commit.hash, &head)? {
            return refuse(NOT_IN_HEAD.to_string());
        }
        if let Some(branch) = protected_branch(repo, &commit.hash)? {
            return refuse(pushed_text(&branch));
        }
    }
    if reword {
        prohibited(repo, REWORD, is_head)?;
    }
    Ok((head, commits))
}

// The common base of the chosen ones: the walk does not need to go below it. Without a common base, nothing.
fn common_base(repo: &Repo, commits: &[Selected]) -> Result<Option<String>, GitError> {
    let mut args = vec!["merge-base", "--octopus"];
    args.extend(commits.iter().map(|commit| commit.hash.as_str()));
    match repo.ctx.git(&args) {
        Ok(output) => Ok(output.stdout.lines().next().map(str::to_string)),
        Err(GitError::Failed { code: Some(1), .. }) => Ok(None),
        Err(error) => Err(error),
    }
}

// `checkHeadLinearHistory`: from HEAD downwards, without merges or roots, until all are seen. Returns
// the chosen ones from the most recent to the oldest.
fn linear(repo: &Repo, head: &str, commits: &[Selected]) -> Result<Vec<String>, Refused> {
    let below = common_base(repo, commits)?.map(|base| format!("^{base}^@"));
    let mut args = vec!["rev-list", "--topo-order", "--parents", head];
    args.extend(below.as_deref());
    args.push("--");
    let output = repo.ctx.git(&args)?;
    let mut remaining: Vec<&str> = commits.iter().map(|commit| commit.hash.as_str()).collect();
    let mut found = Vec::new();
    for line in output.stdout.lines() {
        let mut parts = line.split_whitespace();
        let Some(hash) = parts.next() else {
            continue;
        };
        let count = parts.count();
        if count != 1 {
            for missing in &remaining {
                if !is_ancestor(repo, missing, head)? {
                    return refuse(format!("Commit {missing} is not in the current branch"));
                }
            }
            return refuse(format!("Commit {hash} has {count} parents"));
        }
        if let Some(position) = remaining.iter().position(|wanted| *wanted == hash) {
            remaining.remove(position);
            found.push(hash.to_string());
            if remaining.is_empty() {
                return Ok(found);
            }
        }
    }
    match remaining.first() {
        Some(missing) => refuse(format!("Commit {missing} is not in the current branch")),
        None => Ok(found),
    }
}

// What was checked: HEAD and the commits, from the most recent to the oldest.
struct Checked {
    head: String,
    ordered: Vec<String>,
}

// `update` and `lastCheckCommitsEditingAvailability`. With several, if any is on a protected
// branch the oldest is too: it is enough to look at that one.
fn planned(repo: &Repo, hashes: &[String], kind: Kind) -> Result<Checked, Refused> {
    // What is going to be rewritten comes from `rev-parse --verify`, like any hash that reaches git.
    for hash in hashes {
        branches::check_argument(hash, "The commit")?;
        branches::commit_of(repo, hash)?;
    }
    let (head, commits) = quick(repo, hashes, kind)?;
    if kind == Kind::Reword {
        let ordered = commits.into_iter().map(|commit| commit.hash).collect();
        return Ok(Checked { head, ordered });
    }
    let ordered = linear(repo, &head, &commits)?;
    if let Some(branch) = ordered
        .last()
        .map(|oldest| protected_branch(repo, oldest))
        .transpose()?
        .flatten()
    {
        return refuse(pushed_text(&branch));
    }
    Ok(Checked { head, ordered })
}

// What the log menu checks when it opens: without walking the history.
pub fn quick_check(repo: &Repo, hashes: &[String], kind: Kind) -> Result<(), String> {
    quick(repo, hashes, kind).map(|_| ()).map_err(Refused::text)
}

// All the checks of `GitCommitEditingActionBase`, with their texts.
pub fn check(repo: &Repo, hashes: &[String], kind: Kind) -> Result<(), String> {
    planned(repo, hashes, kind)
        .map(|_| ())
        .map_err(Refused::text)
}

// The single-commit actions of the log menu (`GitSingleCommitEditingAction`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Single {
    // Undo Commit…: only the last of the branch.
    Uncommit,
    // Fixup… and Squash Into…: any of the branch.
    Autosquash,
    // Interactively Rebase from Here…: any of the branch, without another rebase in progress
    // (`ProhibitRebaseDuringRebasePolicy.Prohibit`).
    Rebase,
}

// The checks of `GitCommitEditingActionBase.update` and, for Undo Commit, `isHeadCommit`.
// Returns the full commit.
fn single_plan(repo: &Repo, hash: &str, action: Single) -> Result<String, Refused> {
    branches::check_argument(hash, "The commit")?;
    let commit = branches::commit_of(repo, hash)?;
    let (head, _) = checked(repo, std::slice::from_ref(&commit), Mode::Single)?;
    if action == Single::Uncommit && commit != head {
        return refuse(NOT_LAST.to_string());
    }
    if action == Single::Rebase {
        prohibited(repo, REBASE, false)?;
    }
    Ok(commit)
}

// What the menu and the click check for Undo Commit…, Fixup… and Squash Into…: the reason, with IntelliJ's
// text.
pub fn check_single(repo: &Repo, hash: &str, action: Single) -> Result<(), String> {
    single_plan(repo, hash, action)
        .map(|_| ())
        .map_err(Refused::text)
}

fn expect_head(repo: &Repo, expected: &str, text: String) -> Result<(), GitError> {
    if head_of(repo)?.unwrap_or_default() != expected {
        return Err(branches::invalid(text));
    }
    Ok(())
}

// `VcsCommitMetadata.getFullMessage`.
fn full_message(repo: &Repo, hash: &str) -> Result<String, GitError> {
    let args = ["log", "-1", "--format=%B", "--end-of-options", hash, "--"];
    Ok(repo.ctx.git(&args)?.stdout.trim().to_string())
}

fn not_in_todo(hash: &str) -> TodoError {
    TodoError::Invalid(format!("The commit \"{hash}\" is not in the rebase list"))
}

// `GitRewordOperation.rewordViaAmend`: only the message, without hooks. The file goes in git's
// directory and is deleted when dropped.
fn amend(repo: &Repo, message: &str) -> Result<(), GitError> {
    let dir = repo.ctx.git(&["rev-parse", "--absolute-git-dir"])?;
    let mut file = tempfile::Builder::new()
        .prefix("rama-reword-")
        .tempfile_in(dir.stdout.trim_end_matches('\n'))
        .map_err(git::io_error)?;
    file.write_all(message.as_bytes()).map_err(git::io_error)?;
    file.flush().map_err(git::io_error)?;
    let path = file
        .path()
        .to_str()
        .ok_or_else(|| branches::invalid("Invalid temporary path".to_string()))?;
    let args = ["commit", "--amend", "-F", path, "--only", "--no-verify"];
    repo.ctx.git_loud(&args).map(|_| ())
}

// `GitCommitEditingOperation.rebase`: `rebase --interactive --no-autosquash` from the parent of the
// oldest, through `rebase::Process`, with the `Rewriter` as the editor.
fn rebase_from(
    repo: &Repo,
    oldest: &str,
    old_head: String,
    rewriter: Rewriter,
) -> Result<Rewritten, GitError> {
    let base = branches::commit_of(repo, &format!("{oldest}^"))?;
    let rewriter = Arc::new(rewriter);
    let args = ["rebase", "--interactive", "--no-autosquash", base.as_str()];
    let ran = Process::new(repo, rewriter.clone()).run(&args, || {
        rebase::copy_in_the_way(repo, &old_head, Some(&base))
    });
    let failure = rewriter.failure();
    let mut rebased = match (ran, failure) {
        (Err(GitError::Cancelled), Some(reason)) => return Err(branches::invalid(reason)),
        (ran, _) => ran?,
    };
    if let (Outcome::Failed(text), Some(reason)) = (&mut rebased.outcome, rewriter.failure()) {
        *text = reason;
    }
    Ok(Rewritten {
        new_head: head_of(repo)?.unwrap_or_default(),
        old_head,
        outcome: rebased.outcome,
        title: rebased.title,
        saved: rebased.saved,
    })
}

// `injectRewordAction`.
fn inject_reword(entries: Vec<Entry>, hash: &str) -> Result<Vec<Entry>, TodoError> {
    let mut found = false;
    let entries: Vec<Entry> = entries
        .into_iter()
        .map(|mut entry| {
            if entry.action == Action::Pick && entry.is(hash) {
                entry.action = Action::Reword;
                found = true;
            }
            entry
        })
        .collect();
    if found {
        Ok(entries)
    } else {
        Err(not_in_todo(hash))
    }
}

// `GitRewordOperation.execute`: with HEAD, amend; otherwise, or if it fails, a rebase with `reword`.
pub fn reword(
    repo: &Repo,
    hash: &str,
    message: &str,
    expected_head: &str,
) -> Result<Rewritten, GitError> {
    expect_head(repo, expected_head, dialog_changed(1))?;
    let plan = planned(repo, &[hash.to_string()], Kind::Reword).map_err(Refused::into_error)?;
    let Some(target) = plan.ordered.first().cloned() else {
        return Err(branches::invalid(NOT_IN_HEAD.to_string()));
    };
    let old_head = plan.head;
    if target == old_head {
        match amend(repo, message) {
            Ok(()) => {
                return Ok(Rewritten::between(
                    old_head,
                    head_of(repo)?.unwrap_or_default(),
                ))
            }
            Err(GitError::Cancelled) => return Err(GitError::Cancelled),
            Err(_) => {}
        }
    }
    let reworded = vec![Reworded {
        original: full_message(repo, &target)?,
        message: message.to_string(),
    }];
    let wanted = target.clone();
    let rewriter = Rewriter::new(
        move |entries| Ok((inject_reword(entries, &wanted)?, reworded.clone())),
        true,
        repo.ctx.asking_editor(),
    );
    rebase_from(repo, &target, old_head, rewriter)
}

// `GitDropOperation`. Without confirmation, like IntelliJ.
pub fn drop(repo: &Repo, hashes: &[String], expected_head: &str) -> Result<Rewritten, GitError> {
    expect_head(repo, expected_head, REPOSITORY_CHANGED.to_string())?;
    let plan = planned(repo, hashes, Kind::Drop).map_err(Refused::into_error)?;
    let Some(oldest) = plan.ordered.last().cloned() else {
        return Err(branches::invalid(NOT_IN_HEAD.to_string()));
    };
    let dropped = plan.ordered;
    let rewriter = Rewriter::new(
        move |entries| {
            let mut model = Model::from_entries(entries)?;
            let hashes: Vec<&str> = dropped.iter().map(String::as_str).collect();
            model.drop(&hashes)?;
            Ok((model.to_entries(), Vec::new()))
        },
        false,
        repo.ctx.asking_editor(),
    );
    rebase_from(repo, &oldest, plan.head, rewriter)
}

// `GitSquashOperation`: the oldest is the root, the others go behind as `fixup`, and the root
// is rewritten with the new message.
pub fn squash(
    repo: &Repo,
    hashes: &[String],
    message: &str,
    expected_head: &str,
) -> Result<Rewritten, GitError> {
    expect_head(repo, expected_head, dialog_changed(hashes.len()))?;
    let plan = planned(repo, hashes, Kind::Squash).map_err(Refused::into_error)?;
    let Some(oldest) = plan.ordered.last().cloned() else {
        return Err(branches::invalid(NOT_IN_HEAD.to_string()));
    };
    let originals = plan
        .ordered
        .iter()
        .map(|hash| Ok((hash.clone(), full_message(repo, hash)?)))
        .collect::<Result<Vec<_>, GitError>>()?;
    let message = message.to_string();
    let rewriter = Rewriter::new(
        move |entries| {
            let mut model = Model::from_entries(entries)?;
            // `indicesByPredicate`: in the list's order, from the oldest to the most recent.
            let mut united = originals
                .iter()
                .map(|(hash, _)| Ok((model.index_of(hash)?, hash.as_str())))
                .collect::<Result<Vec<_>, TodoError>>()?;
            united.sort_by_key(|(index, _)| *index);
            let hashes: Vec<&str> = united.iter().map(|(_, hash)| *hash).collect();
            let Some((root, children)) = hashes.split_first() else {
                return Err(not_in_todo(&oldest_of(&originals)));
            };
            model.unite(root, children)?;
            model.reword(root, &message)?;
            let reworded = model
                .reworded()
                .into_iter()
                .map(|(entry, new)| {
                    let original = originals
                        .iter()
                        .find(|(hash, _)| entry.is(hash))
                        .map(|(_, original)| original.clone())
                        .ok_or_else(|| TodoError::Invalid(ONLY_FROM_REWORDED.to_string()))?;
                    Ok(Reworded {
                        original,
                        message: new.to_string(),
                    })
                })
                .collect::<Result<Vec<_>, TodoError>>()?;
            Ok((model.to_entries(), reworded))
        },
        false,
        repo.ctx.asking_editor(),
    );
    rebase_from(repo, &oldest, plan.head, rewriter)
}

fn oldest_of(originals: &[(String, String)]) -> String {
    originals
        .last()
        .map(|(hash, _)| hash.clone())
        .unwrap_or_default()
}

// `GitNewCommitMessageActionDialog.validate`, on pressing OK: HEAD moved, or the oldest of the
// selection (the last, in the log's order) already on a protected branch. `None` if it is accepted.
pub fn dialog_problem(
    repo: &Repo,
    hashes: &[String],
    expected_head: &str,
) -> Result<Option<String>, GitError> {
    let count = hashes.len();
    if head_of(repo)?.unwrap_or_default() != expected_head {
        return Ok(Some(dialog_changed(count)));
    }
    let Some(last) = hashes.last() else {
        return Ok(None);
    };
    branches::check_argument(last, "The commit")?;
    let last = branches::commit_of(repo, last)?;
    // `rebase.log.reword.dialog.failed.pushed.to.protected.message`.
    Ok(protected_branch(repo, &last)?.map(|branch| {
        format!(
            "Cannot edit selected {}: commit {last} was pushed to protected branch '{branch}'",
            commits(count)
        )
    }))
}

// `findFirstChangedHash`: the oldest of what was rewritten, which is what a push would have pushed.
fn first_changed(repo: &Repo, rewritten: &Rewritten) -> Result<Option<String>, GitError> {
    let base = match repo
        .ctx
        .git(&["merge-base", &rewritten.old_head, &rewritten.new_head])
    {
        Ok(output) => output.stdout.trim().to_string(),
        Err(GitError::Failed { .. }) => return Ok(None),
        Err(error) => return Err(error),
    };
    let range = format!("{base}..{}", rewritten.new_head);
    let output = repo.ctx.git(&["rev-list", "--reverse", &range, "--"])?;
    Ok(output.stdout.lines().next().map(str::to_string))
}

// `checkUndoPossibility`: why it can no longer be undone, or `None`.
pub fn undo_impossible(repo: &Repo, rewritten: &Rewritten) -> Result<Option<String>, GitError> {
    for hash in [&rewritten.old_head, &rewritten.new_head] {
        branches::check_argument(hash, "The commit")?;
    }
    if head_of(repo)?.as_deref() != Some(rewritten.new_head.as_str()) {
        return Ok(Some(REPOSITORY_CHANGED.to_string()));
    }
    let Some(first) = first_changed(repo, rewritten)? else {
        return Ok(None);
    };
    // `rebase.log.undo.impossible.pushed.to.protected.branch.notification.text`.
    Ok(protected_branch(repo, &first)?.map(|branch| {
        format!("Commits from current branch have already been pushed to protected branch {branch}")
    }))
}

// `GitCommitEditingOperationResult.Complete.undo`: `reset --keep` to the previous HEAD.
pub fn undo(repo: &Repo, rewritten: &Rewritten) -> Result<(), GitError> {
    if let Some(text) = undo_impossible(repo, rewritten)? {
        return Err(branches::invalid(text));
    }
    let old = branches::commit_of(repo, &rewritten.old_head)?;
    branches::reset_keep(repo, &old)
}

// The paths a commit touches, before and after a rename: what `getChanges` gives to
// `findCorrespondentLocalChanges`.
fn commit_paths(repo: &Repo, commit: &str) -> Result<Vec<String>, GitError> {
    let args = [
        "diff-tree",
        "-r",
        "--no-renames",
        "--name-only",
        "-z",
        "--no-commit-id",
        "--end-of-options",
        commit,
    ];
    let output = repo.ctx.git(&args)?;
    Ok(output
        .stdout
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect())
}

// `GitUncommitAction.resetInBackground`: `reset --soft` to the parent, and the commit's paths so that
// the caller moves them to the chosen changelist. It changes no file.
pub fn uncommit(repo: &Repo, hash: &str, expected_head: &str) -> Result<Vec<String>, GitError> {
    expect_head(repo, expected_head, REPOSITORY_CHANGED.to_string())?;
    let commit = single_plan(repo, hash, Single::Uncommit).map_err(Refused::into_error)?;
    let paths = commit_paths(repo, &commit)?;
    let parent = branches::commit_of(repo, &format!("{commit}^"))?;
    match branches::reset(repo, &parent, ResetMode::Soft, CheckoutMode::Plain)? {
        CheckoutOutcome::Done => Ok(paths),
        _ => Err(branches::invalid(UNDO_FAILED.to_string())),
    }
}

// Who answers the editor of the `Commit and Rebase…` rebase: the `Rebasing Commits` dialog with the
// list git generates, like IntelliJ's normal interactive editor.
fn autosquash_editor(repo: &Repo) -> Arc<dyn EditorHandler> {
    interactive_editor(repo)
}

// `GitRebaseCheckinHandlerFactory.checkinSuccessful`: `rebase --interactive <base>` through
// `rebase::Process`. It sets neither `--autosquash` nor `--no-autosquash`: the user's `rebase.autoSquash`
// rules, like `AutoSquashOption.DEFAULT`.
pub fn autosquash(repo: &Repo, base: &str) -> Result<Rebased, GitError> {
    autosquash_with(repo, base, autosquash_editor(repo))
}

pub fn autosquash_with(
    repo: &Repo,
    base: &str,
    handler: Arc<dyn EditorHandler>,
) -> Result<Rebased, GitError> {
    branches::check_argument(base, "The base")?;
    let target = branches::commit_of(repo, base)?;
    let old_head = branches::commit_of(repo, HEAD)?;
    // A base that is not on the branch would rebase what it should not touch.
    if !is_ancestor(repo, &target, &old_head)? {
        return Err(branches::invalid(NOT_IN_HEAD.to_string()));
    }
    // The commit may have closed a merge or the branch may have changed since it was proposed: without
    // `--rebase-merges`, git would flatten the merge that remains between the base and HEAD.
    let range = format!("{target}..{old_head}");
    let merges = repo.ctx.git(&[
        "rev-list",
        "--parents",
        "--min-parents=2",
        "--end-of-options",
        &range,
        "--",
    ])?;
    if let Some(merge) = merges.stdout.lines().next() {
        let mut parts = merge.split_whitespace();
        let hash = parts.next().unwrap_or_default();
        return Err(branches::invalid(format!(
            "Commit {hash} has {} parents",
            parts.count()
        )));
    }
    let args = ["rebase", "--interactive", target.as_str()];
    Process::new(repo, handler).run(&args, || {
        rebase::copy_in_the_way(repo, &old_head, Some(&target))
    })
}

// What the log menu shows for each entry: `None` if it is enabled. Edit Commit Message only
// counts with one commit, and Squash Commits with two or more.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MenuChecks {
    // Interactively Rebase from Here…: only counts with one commit.
    pub rebase: Option<String>,
    pub reword: Option<String>,
    pub drop: Option<String>,
    pub squash: Option<String>,
    // Undo Commit…, and Fixup… with Squash Into…: only count with one commit.
    pub uncommit: Option<String>,
    pub autosquash: Option<String>,
}

pub fn menu_checks(repo: &Repo, hashes: &[String]) -> MenuChecks {
    let reason = |kind| quick_check(repo, hashes, kind).err();
    let drop = reason(Kind::Drop);
    let single = |action| match hashes {
        [hash] => check_single(repo, hash, action).err(),
        _ => None,
    };
    // Undo Commit… has the reasons of Fixup… and, after that, that of not being the last.
    let autosquash = single(Single::Autosquash);
    let uncommit = match (&autosquash, hashes) {
        (Some(reason), _) => Some(reason.clone()),
        (None, [_]) => single(Single::Uncommit),
        (None, _) => None,
    };
    MenuChecks {
        rebase: single(Single::Rebase),
        uncommit,
        autosquash,
        reword: if hashes.len() == 1 {
            reason(Kind::Reword)
        } else {
            None
        },
        squash: if hashes.len() > 1 { drop.clone() } else { None },
        drop,
    }
}

// How an Undo ended: whether it could not be attempted (`Cannot Undo …`) or failed (`Undo … Failed`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "reason", rename_all = "camelCase")]
pub enum Undone {
    Done,
    Impossible(String),
    Failed(String),
}

// `undoInBackground`.
pub fn undo_reporting(repo: &Repo, rewritten: &Rewritten) -> Result<Undone, GitError> {
    if let Some(text) = undo_impossible(repo, rewritten)? {
        return Ok(Undone::Impossible(text));
    }
    match undo(repo, rewritten) {
        Ok(()) => Ok(Undone::Done),
        Err(GitError::Cancelled) => Err(GitError::Cancelled),
        Err(error) => Ok(Undone::Failed(branches::text_of(error))),
    }
}

// Interactively Rebase from Here…: the `Rebasing Commits` dialog and the editor that takes it to git.
// Ported from `GitInteractiveRebaseAction`, `GitInteractiveRebaseUsingLog`,
// `GitInteractiveRebaseEditorHandler` and `GitRewordedCommitMessageProvider`.

// `rebase.using.log.couldnt.start.error`, without the IDE's name.
pub const INVALID_TODO: &str = "Cannot perform the rebase because an invalid 'git-rebase-todo' \
    file was generated. Do you want to retry the rebase using Git native file generation?";
// A row of the dialog does not reach the todo as text, but a line break in it would be another command.
const LINE_BREAK: &str = "The dialog list has a line break";
// `rebase.log.action.operation.rebase.name`.
const REBASE: &str = "rebase";
const FIXUP_PREFIX: &str = "fixup!";
const SQUASH_PREFIX: &str = "squash!";
const REBASE_DIRS: [&str; 2] = ["rebase-merge", "rebase-apply"];
const TODO_SUBJECT_MARK: &str = "# ";

// A row of the dialog: the todo line, the full commit and its whole message
// (`GitRebaseEntryWithDetails`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RebaseCommit {
    #[serde(flatten)]
    pub entry: Entry,
    pub full: String,
    pub message: String,
}

// What the log sees before opening the dialog: the rebase's base and, if the log can be used, the
// rows. `None` is the native mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InteractiveStart {
    pub base: String,
    pub commits: Option<Vec<RebaseCommit>>,
}

// The new message of a `reword`, by the hash of its line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewMessage {
    pub hash: String,
    pub message: String,
}

// What comes out of the dialog: `convertToEntries` and the messages of `processModel`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub entries: Vec<Entry>,
    pub messages: Vec<NewMessage>,
}

// `getEntriesUsingLog`: from HEAD downwards to the commit, from the oldest to the most recent. `None`
// if there is a merge or a root, if the hash does not resolve, or if any subject starts with `fixup!` or
// `squash!`: then the list is generated by git (decision 10).
pub fn interactive_entries(repo: &Repo, hash: &str) -> Result<Option<Vec<Entry>>, GitError> {
    branches::check_argument(hash, "The commit")?;
    let Ok(commit) = branches::commit_of(repo, hash) else {
        return Ok(None);
    };
    let below = format!("^{commit}^@");
    let args = [
        "log",
        "--topo-order",
        "--format=%H %P%x00%s",
        "--end-of-options",
        HEAD,
        below.as_str(),
        "--",
    ];
    let output = repo.ctx.git(&args)?;
    let mut entries = Vec::new();
    for line in output.stdout.lines() {
        let Some((ids, subject)) = line.split_once('\0') else {
            continue;
        };
        let mut ids = ids.split_whitespace();
        let Some(id) = ids.next() else {
            continue;
        };
        if ids.count() != 1 {
            return Ok(None);
        }
        entries.push(Entry {
            action: Action::Pick,
            hash: id.to_string(),
            subject: subject.trim_start().to_string(),
        });
        if id == commit {
            break;
        }
    }
    if entries.last().map(|entry| entry.hash.as_str()) != Some(commit.as_str()) {
        return Ok(None);
    }
    let marked = |entry: &Entry| {
        entry.subject.starts_with(FIXUP_PREFIX) || entry.subject.starts_with(SQUASH_PREFIX)
    };
    if entries.iter().any(marked) {
        return Ok(None);
    }
    entries.reverse();
    Ok(Some(entries))
}

// `GitLogUtil.collectMetadata`: the full hash and the whole message of each line, in one command.
fn commits_of(path: &Path, entries: Vec<Entry>) -> Result<Vec<RebaseCommit>, GitError> {
    if entries.is_empty() {
        return Ok(Vec::new());
    }
    let mut args = vec![
        history::NO_SIGNATURES[0],
        history::NO_SIGNATURES[1],
        "log",
        "--no-walk=unsorted",
        "--format=%H%x00%B%x00",
        "--end-of-options",
    ];
    for entry in &entries {
        branches::check_argument(&entry.hash, "The commit")?;
        args.push(&entry.hash);
    }
    args.push("--");
    let output = String::from_utf8_lossy(&git::run(path, &args)?).into_owned();
    let mut fields = output.split('\0');
    let mut found = Vec::new();
    while let (Some(full), Some(message)) = (fields.next(), fields.next()) {
        found.push((full.trim().to_string(), message.trim().to_string()));
    }
    entries
        .into_iter()
        .map(|entry| {
            let (full, message) = found
                .iter()
                .find(|(full, _)| entry.is(full))
                .cloned()
                .ok_or_else(|| branches::invalid(format!("Could not find \"{}\"", entry.hash)))?;
            Ok(RebaseCommit {
                entry,
                full,
                message,
            })
        })
        .collect()
}

// What the log needs to open the dialog, or to know that it cannot.
pub fn interactive_start(repo: &Repo, hash: &str) -> Result<InteractiveStart, GitError> {
    branches::check_argument(hash, "The commit")?;
    let base = branches::commit_of(repo, &format!("{hash}^"))?;
    let commits = interactive_entries(repo, hash)?
        .map(|entries| commits_of(repo.path, entries))
        .transpose()?;
    Ok(InteractiveStart { base, commits })
}

// `equalsWithReal`: the line git wrote is the one that was generated, with the abbreviated hash. Recent
// versions of git comment the subject (`pick 1a2b # subject`): that mark does not count.
fn same_line(generated: &Entry, real: &Entry) -> bool {
    let subject = real
        .subject
        .strip_prefix(TODO_SUBJECT_MARK)
        .unwrap_or(&real.subject);
    generated.action == real.action && real.is(&generated.hash) && generated.subject == subject
}

// What comes back from the dialog is only valid if they are the same lines, in another order and with other actions,
// and if each `reword` carries its message. The messages go to the message editor and may have line breaks.
fn check_plan(plan: &Plan, lines: &[Entry]) -> Result<(), String> {
    let broken = |text: &str| text.contains(['\n', '\r']);
    if plan
        .entries
        .iter()
        .any(|entry| broken(&entry.hash) || broken(&entry.subject))
    {
        return Err(LINE_BREAK.to_string());
    }
    let mut hashes: Vec<&str> = plan
        .entries
        .iter()
        .map(|entry| entry.hash.as_str())
        .collect();
    let mut expected: Vec<&str> = lines.iter().map(|entry| entry.hash.as_str()).collect();
    hashes.sort_unstable();
    expected.sort_unstable();
    if hashes != expected {
        return Err("The dialog list is not the rebase list".to_string());
    }
    if plan
        .entries
        .iter()
        .any(|entry| matches!(entry.action, Action::Other(_)))
    {
        return Err("The dialog list has an unknown action".to_string());
    }
    Model::from_entries(plan.entries.clone()).map_err(|_| INVALID_TODO.to_string())?;
    let reworded: Vec<&str> = plan
        .entries
        .iter()
        .filter(|entry| entry.action == Action::Reword)
        .map(|entry| entry.hash.as_str())
        .collect();
    let mut given: Vec<&str> = plan.messages.iter().map(|m| m.hash.as_str()).collect();
    let mut wanted = reworded.clone();
    given.sort_unstable();
    wanted.sort_unstable();
    if given != wanted {
        return Err("Every reword needs its message".to_string());
    }
    Ok(())
}

// `GitRewordedCommitMessageProvider`: the messages of the last dialog, by git directory, for
// the `reword`s of this rebase even if git asks for them after a Continue. They are valid while the rebase is
// the same (`onto`). They live in memory: IntelliJ keeps them in the workspace.
struct Remembered {
    onto: String,
    next: usize,
    mappings: Vec<Reworded>,
}

static REMEMBERED: Mutex<BTreeMap<PathBuf, Remembered>> = Mutex::new(BTreeMap::new());

fn remembered() -> MutexGuard<'static, BTreeMap<PathBuf, Remembered>> {
    REMEMBERED.lock().unwrap_or_else(PoisonError::into_inner)
}

// `GitRebaseUtils.getOntoHash`.
fn onto_of(git_dir: &Path) -> Option<String> {
    REBASE_DIRS.iter().find_map(|dir| {
        let text = fs::read_to_string(git_dir.join(dir).join("onto")).ok()?;
        Some(text.trim().to_string()).filter(|onto| !onto.is_empty())
    })
}

fn remember(git_dir: &Path, mappings: Vec<Reworded>) {
    let Some(onto) = onto_of(git_dir) else {
        return;
    };
    remembered().insert(
        git_dir.to_path_buf(),
        Remembered {
            onto,
            next: 0,
            mappings,
        },
    );
}

// Once the rebase is finished or aborted, what was saved belongs to no one.
pub(crate) fn forget(repo: &Repo) {
    if let Ok(git_dir) = git_dir_of(repo) {
        remembered().remove(&git_dir);
    }
}

// `getRewordedCommitMessage`.
fn recalled(git_dir: &Path, text: &str) -> Option<String> {
    let onto = onto_of(git_dir)?;
    let mut all = remembered();
    let saved = all.get_mut(git_dir).filter(|saved| saved.onto == onto)?;
    let mapping = saved.mappings.get(saved.next)?;
    if !text.starts_with(&mapping.original) {
        return None;
    }
    let message = mapping.message.clone();
    saved.next += 1;
    Some(message)
}

fn git_dir_of(repo: &Repo) -> Result<PathBuf, GitError> {
    let output = repo.ctx.git(&["rev-parse", "--absolute-git-dir"])?;
    Ok(PathBuf::from(output.stdout.trim_end_matches('\n')))
}

// The plan of a rebase with the log: what was generated, which git's todo is compared with, and what is
// written in its place.
struct FromLog {
    generated: Vec<Entry>,
    plan: Plan,
}

// `GitInteractiveRebaseEditorHandler` and, with `log`, `GitInteractiveRebaseUsingLogEditorHandler`. The
// first list goes to the dialog (or is replaced by the log's); what it does not understand, to the free
// text editor, and a `noop`, to the question of whether to go on. The messages of the `reword`s come from what was saved.
pub struct Interactive {
    path: PathBuf,
    git_dir: PathBuf,
    log: Option<FromLog>,
    ask: Ask,
    state: Mutex<Shown>,
}

#[derive(Default)]
struct Shown {
    // `myRebaseEditorShown`.
    list: bool,
    failure: Option<String>,
}

impl Interactive {
    fn new(repo: &Repo, log: Option<FromLog>) -> Result<Self, GitError> {
        Ok(Self {
            path: repo.path.to_path_buf(),
            git_dir: git_dir_of(repo)?,
            log,
            ask: repo.ctx.asking(),
            state: Mutex::default(),
        })
    }

    pub fn failure(&self) -> Option<String> {
        self.lock().failure.clone()
    }

    fn lock(&self) -> MutexGuard<'_, Shown> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn fail(&self, reason: String) -> Option<String> {
        self.lock().failure = Some(reason);
        None
    }

    fn free_text(&self, request: Option<&Request>, kind: EditKind, text: &str) -> Option<String> {
        self.ask.prompt(request, Prompt::text(kind, text))
    }

    // The original messages of the `reword`s, in the list's order: what is saved.
    fn mappings(&self, plan: &Plan) -> Result<Vec<Reworded>, String> {
        let reworded: Vec<Entry> = plan
            .entries
            .iter()
            .filter(|entry| entry.action == Action::Reword)
            .cloned()
            .collect();
        let commits = commits_of(&self.path, reworded).map_err(branches::text_of)?;
        commits
            .into_iter()
            .map(|commit| {
                let message = plan
                    .messages
                    .iter()
                    .find(|message| message.hash == commit.entry.hash)
                    .map(|message| message.message.clone())
                    .ok_or_else(|| "Every reword needs its message".to_string())?;
                Ok(Reworded {
                    original: commit.message,
                    message,
                })
            })
            .collect()
    }

    // Of the dialog only the order and the actions count: the hash and the subject are those of the line of
    // `lines` (the one git wrote, or the one the log generated).
    fn written(&self, plan: &Plan, lines: &[Entry]) -> Option<String> {
        let entries: Option<Vec<Entry>> = plan
            .entries
            .iter()
            .map(|entry| {
                let line = lines.iter().find(|line| line.hash == entry.hash)?;
                Some(Entry {
                    action: entry.action.clone(),
                    ..line.clone()
                })
            })
            .collect();
        let Some(entries) = entries else {
            return self.fail(INVALID_TODO.to_string());
        };
        match self.mappings(plan) {
            Ok(mappings) => {
                remember(&self.git_dir, mappings);
                Some(todo::write(&entries))
            }
            Err(reason) => self.fail(reason),
        }
    }

    // `handleInteractiveEditor`.
    fn list(&self, request: Option<&Request>, text: &str) -> Option<String> {
        let entries = match todo::parse(text) {
            Ok(entries) => entries,
            // `confirmNoopRebase`: with OK, the list stays as it is.
            Err(TodoError::Noop) => {
                let prompt = Prompt {
                    kind: EditKind::Todo,
                    form: Form::Noop,
                    text: String::new(),
                };
                return self.ask.prompt(request, prompt).map(|_| text.to_string());
            }
            Err(_) => return self.free_text(request, EditKind::Todo, text),
        };
        if entries
            .iter()
            .any(|entry| matches!(entry.action, Action::Other(_)))
        {
            return self.free_text(request, EditKind::Todo, text);
        }
        if let Some(log) = &self.log {
            // IntelliJ does not compare how many there are: an extra git line would not be written, and git
            // would take it for a `drop`.
            let same = log.generated.len() == entries.len()
                && log
                    .generated
                    .iter()
                    .zip(&entries)
                    .all(|(generated, real)| same_line(generated, real));
            if !same {
                return self.fail(INVALID_TODO.to_string());
            }
            return self.written(&log.plan, &log.generated);
        }
        let commits = match commits_of(&self.path, entries.clone()) {
            Ok(commits) => commits,
            Err(error) => return self.fail(branches::text_of(error)),
        };
        let shown = serde_json::to_string(&commits).ok()?;
        let prompt = Prompt {
            kind: EditKind::Todo,
            form: Form::Commits,
            text: shown,
        };
        let answer = self.ask.prompt(request, prompt)?;
        let plan: Plan = match serde_json::from_str(&answer) {
            Ok(plan) => plan,
            Err(_) => return self.fail(INVALID_TODO.to_string()),
        };
        if let Err(reason) = check_plan(&plan, &entries) {
            return self.fail(reason);
        }
        self.written(&plan, &entries)
    }

    fn handle(&self, request: Option<&Request>, kind: EditKind, text: &str) -> Option<String> {
        match kind {
            EditKind::Todo => {
                let first = !std::mem::replace(&mut self.lock().list, true);
                if first {
                    self.list(request, text)
                } else {
                    self.free_text(request, kind, text)
                }
            }
            EditKind::Message => {
                recalled(&self.git_dir, text).or_else(|| self.free_text(request, kind, text))
            }
        }
    }
}

// It also delegates `edit_request`: so the window stays attentive to the editor going away.
impl EditorHandler for Interactive {
    fn edit(&self, kind: EditKind, text: &str) -> Option<String> {
        self.handle(None, kind, text)
    }

    fn edit_request(&self, request: &Request, kind: EditKind, text: &str) -> Option<String> {
        self.handle(Some(request), kind, text)
    }
}

// The editor of an interactive rebase without the log (the Rebase… of the Git menu, Commit and Rebase…): the
// list is generated by git and the window shows it in the dialog.
pub fn interactive_editor(repo: &Repo) -> Arc<dyn EditorHandler> {
    match Interactive::new(repo, None) {
        Ok(handler) => Arc::new(handler),
        Err(_) => repo.ctx.asking_editor(),
    }
}

// The editor of Continue and Skip (`createEditor` without the list): the messages of the `reword`s that the
// dialog saved, and otherwise, the free text editor.
pub struct Resuming {
    git_dir: Option<PathBuf>,
    ask: Ask,
}

impl Resuming {
    fn handle(&self, request: Option<&Request>, kind: EditKind, text: &str) -> Option<String> {
        let saved = match (kind, &self.git_dir) {
            (EditKind::Message, Some(git_dir)) => recalled(git_dir, text),
            _ => None,
        };
        saved.or_else(|| self.ask.prompt(request, Prompt::text(kind, text)))
    }
}

impl EditorHandler for Resuming {
    fn edit(&self, kind: EditKind, text: &str) -> Option<String> {
        self.handle(None, kind, text)
    }

    fn edit_request(&self, request: &Request, kind: EditKind, text: &str) -> Option<String> {
        self.handle(Some(request), kind, text)
    }
}

pub fn resuming_editor(repo: &Repo) -> Arc<dyn EditorHandler> {
    Arc::new(Resuming {
        git_dir: git_dir_of(repo).ok(),
        ask: repo.ctx.asking(),
    })
}

// `startInteractiveRebase`: `rebase --interactive <full parent>` through `rebase::Process`, without
// `--no-autosquash` (`AutoSquashOption.DEFAULT`). With `plan`, git's todo must be the one the
// log generated; without it, the list goes to the window.
pub fn interactive(
    repo: &Repo,
    hash: &str,
    plan: Option<Plan>,
    expected_head: &str,
) -> Result<Rebased, GitError> {
    operation::check_head(repo.path, expected_head)?;
    let commit = single_plan(repo, hash, Single::Rebase).map_err(Refused::into_error)?;
    let base = branches::commit_of(repo, &format!("{commit}^"))?;
    let log = match plan {
        Some(plan) => {
            let generated = interactive_entries(repo, &commit)?
                .ok_or_else(|| branches::invalid(REPOSITORY_CHANGED.to_string()))?;
            check_plan(&plan, &generated).map_err(branches::invalid)?;
            Some(FromLog { generated, plan })
        }
        None => None,
    };
    let handler = Arc::new(Interactive::new(repo, log)?);
    let args = ["rebase", "--interactive", base.as_str()];
    let ran = Process::new(repo, handler.clone()).run(&args, || {
        rebase::copy_in_the_way(repo, expected_head, Some(&base))
    });
    if !commit::rebase_in_progress(repo.path) {
        forget(repo);
    }
    let mut rebased = match (ran, handler.failure()) {
        (Err(GitError::Cancelled), Some(reason)) => return Err(branches::invalid(reason)),
        (ran, _) => ran?,
    };
    if let (Outcome::Failed(text), Some(reason)) = (&mut rebased.outcome, handler.failure()) {
        *text = reason;
    }
    Ok(rebased)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Command;

    use super::*;
    use crate::editor::Trivial;
    use crate::test_support::RepoHarness;
    use crate::todo::Action;

    fn ok_or_text<T>(result: Result<T, GitError>) -> Result<T, String> {
        result.map_err(crate::branches::text_of)
    }

    fn message_of(harness: &RepoHarness, revision: &str) -> String {
        harness
            .git(&["log", "-1", "--format=%B", revision])
            .trim_end()
            .to_string()
    }

    fn subjects(harness: &RepoHarness) -> Vec<String> {
        harness
            .git(&["log", "--format=%s"])
            .lines()
            .map(str::to_string)
            .collect()
    }

    // `base`, `A`, `B` and `C`, each with its own file. Returns their hashes.
    fn four() -> (RepoHarness, Vec<String>) {
        let harness = RepoHarness::new();
        let hashes = ["base", "A", "B", "C"]
            .iter()
            .map(|name| {
                harness
                    .repo
                    .commit(&format!("{name}.txt"), name.as_bytes(), name)
            })
            .collect();
        (harness, hashes)
    }

    fn checked(harness: &RepoHarness, hashes: &[&str], kind: Kind) -> Result<(), String> {
        let hashes: Vec<String> = hashes.iter().map(|hash| hash.to_string()).collect();
        harness.run(move |repo| check(repo, &hashes, kind))
    }

    fn quick(harness: &RepoHarness, hashes: &[&str], kind: Kind) -> Result<(), String> {
        let hashes: Vec<String> = hashes.iter().map(|hash| hash.to_string()).collect();
        harness.run(move |repo| quick_check(repo, &hashes, kind))
    }

    fn reworded(harness: &RepoHarness, hash: &str, message: &str) -> Result<Rewritten, String> {
        let (hash, message, head) = (hash.to_string(), message.to_string(), harness.head());
        harness.run(move |repo| ok_or_text(reword(repo, &hash, &message, &head)))
    }

    fn undone(harness: &RepoHarness, rewritten: Rewritten) -> Result<(), String> {
        harness.run(move |repo| ok_or_text(undo(repo, &rewritten)))
    }

    fn with_origin(harness: &RepoHarness) -> tempfile::TempDir {
        let remote = tempfile::TempDir::new().unwrap();
        harness.git(&["init", "-q", "--bare", remote.path().to_str().unwrap()]);
        harness.git(&["remote", "add", "origin", remote.path().to_str().unwrap()]);
        harness.git(&["push", "-q", "origin", "main"]);
        remote
    }

    #[test]
    fn reword_of_head_uses_amend() {
        let (harness, hashes) = four();
        harness.write("A.txt", "local change\n");
        harness.git(&["add", "A.txt"]);
        harness.write("B.txt", "unstaged\n");

        let rewritten = reworded(&harness, &hashes[3], "C new\n\nwith body").unwrap();

        assert_eq!(message_of(&harness, "HEAD"), "C new\n\nwith body");
        assert_eq!(harness.git(&["rev-parse", "HEAD^"]).trim(), hashes[2]);
        assert!(harness.ran("--only --no-verify"), "amend was not used");
        assert_eq!(rewritten.old_head, hashes[3]);
        assert_eq!(rewritten.new_head, harness.head());
        assert_eq!(rewritten.outcome, Outcome::Done);
        // Only the message: what was in the stage and on disk is still there, without going into the commit.
        assert_eq!(
            harness
                .git(&["show", "--format=", "--name-only", "HEAD"])
                .trim(),
            "C.txt"
        );
        assert_eq!(
            harness.git(&["diff", "--cached", "--name-only"]).trim(),
            "A.txt"
        );
        assert_eq!(harness.read("B.txt"), "unstaged\n");
        // The message file does not stay in git's directory.
        let left: Vec<String> = fs::read_dir(harness.path().join(".git"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("rama-"))
            .collect();
        assert!(left.is_empty(), "{left:?}");
    }

    #[test]
    fn undo_goes_back_with_reset_keep() {
        let (harness, hashes) = four();
        let rewritten = reworded(&harness, &hashes[3], "C new").unwrap();
        harness.write("A.txt", "local change\n");

        undone(&harness, rewritten).unwrap();

        assert_eq!(harness.head(), hashes[3]);
        assert_eq!(message_of(&harness, "HEAD"), "C");
        assert!(harness.ran(&format!("reset --keep {} --", hashes[3])));
        assert_eq!(harness.read("A.txt"), "local change\n");
    }

    #[test]
    fn undo_after_another_change_gives_repository_has_already_been_changed() {
        let (harness, hashes) = four();
        let rewritten = reworded(&harness, &hashes[3], "C new").unwrap();
        let moved = harness.repo.commit("D.txt", b"D", "D");

        let (checked_rewritten, done) = (rewritten.clone(), rewritten);
        let impossible =
            harness.run(move |repo| ok_or_text(undo_impossible(repo, &checked_rewritten)));
        assert_eq!(
            impossible,
            Ok(Some("Repository has already been changed".to_string()))
        );
        assert_eq!(
            undone(&harness, done),
            Err("Repository has already been changed".to_string())
        );
        assert_eq!(harness.head(), moved);
    }

    #[test]
    fn commit_outside_the_branch_gives_the_commit_is_not_in_the_current_branch() {
        let (harness, hashes) = four();
        harness.git(&["checkout", "-q", "-b", "other", &hashes[1]]);
        let other = harness.repo.commit("other.txt", b"other", "other");
        harness.git(&["checkout", "-q", "main"]);
        let head = harness.head();

        let not_in_head = Err("The commit is not in the current branch".to_string());
        assert_eq!(checked(&harness, &[&other], Kind::Reword), not_in_head);
        assert_eq!(checked(&harness, &[&other], Kind::Drop), not_in_head);
        assert_eq!(
            quick(&harness, &[&other, &hashes[2]], Kind::Squash),
            not_in_head
        );
        // In the middle of the selection only the full check sees it, with the hash.
        let middle = [hashes[3].as_str(), other.as_str(), hashes[2].as_str()];
        assert_eq!(quick(&harness, &middle, Kind::Drop), Ok(()));
        assert_eq!(
            checked(&harness, &middle, Kind::Drop),
            Err(format!("Commit {other} is not in the current branch"))
        );
        assert_eq!(
            reworded(&harness, &other, "new").map(std::mem::drop),
            Err("The commit is not in the current branch".to_string())
        );
        assert_eq!(harness.head(), head);
        assert_eq!(
            checked(&harness, &[&hashes[2], &hashes[1]], Kind::Squash),
            Ok(())
        );
    }

    #[test]
    fn merge_in_the_range_gives_commit_has_n_parents() {
        let (harness, hashes) = four();
        harness.git(&["checkout", "-q", "-b", "side", &hashes[1]]);
        harness.repo.commit("side.txt", b"side", "side");
        harness.git(&["checkout", "-q", "main"]);
        harness.git(&["merge", "-q", "--no-ff", "-m", "merge", "side"]);
        let merge = harness.head();
        let top = harness.repo.commit("D.txt", b"D", "D");

        assert_eq!(quick(&harness, &[&top, &hashes[2]], Kind::Drop), Ok(()));
        assert_eq!(
            checked(&harness, &[&top, &hashes[2]], Kind::Drop),
            Err(format!("Commit {merge} has 2 parents"))
        );
        assert_eq!(checked(&harness, &[&top], Kind::Drop), Ok(()));
        let merge_parents = Err("Selected commit has 2 parents".to_string());
        assert_eq!(checked(&harness, &[&merge], Kind::Reword), merge_parents);
        assert_eq!(
            checked(&harness, &[&top, &merge], Kind::Squash),
            merge_parents
        );
        assert_eq!(
            checked(&harness, &[&hashes[0]], Kind::Reword),
            Err("Selected commit has 0 parents".to_string())
        );
        // A merge in HEAD can be edited: it is an amend.
        harness.git(&["reset", "-q", "--hard", &merge]);
        assert_eq!(checked(&harness, &[&merge], Kind::Reword), Ok(()));
        assert_eq!(checked(&harness, &[&merge], Kind::Drop), merge_parents);
    }

    #[test]
    fn commit_on_protected_branch_is_rejected_with_its_text() {
        let (harness, hashes) = four();
        let _remote = with_origin(&harness);
        let top = harness.repo.commit("D.txt", b"D", "D");

        let pushed =
            Err("The commit is already pushed to protected branch 'origin/main'".to_string());
        assert_eq!(checked(&harness, &[&hashes[3]], Kind::Reword), pushed);
        assert_eq!(quick(&harness, &[&top, &hashes[2]], Kind::Drop), pushed);
        assert_eq!(checked(&harness, &[&top], Kind::Drop), Ok(()));
        let head = harness.head();
        let refused = harness.run({
            let hashes = vec![hashes[2].clone()];
            move |repo| ok_or_text(drop(repo, &hashes, &head))
        });
        assert_eq!(refused.map(std::mem::drop), pushed);

        // Undo neither, if the new one was already pushed.
        let rewritten = reworded(&harness, &top, "D new").unwrap();
        harness.git(&["push", "-q", "origin", "main"]);
        assert_eq!(
            undone(&harness, rewritten),
            Err("Commits from current branch have already been pushed to protected branch origin/main".to_string())
        );
        assert_eq!(message_of(&harness, "HEAD"), "D new");
    }

    #[test]
    fn the_dialog_rejects_a_moved_head_and_the_commit_on_a_protected_branch() {
        let (harness, hashes) = four();
        let _remote = with_origin(&harness);
        let top = harness.repo.commit("D.txt", b"D", "D");
        let problem = |hashes: Vec<String>, head: String| {
            harness.run(move |repo| ok_or_text(dialog_problem(repo, &hashes, &head)))
        };
        let head = harness.head();

        assert_eq!(problem(vec![top.clone()], head.clone()), Ok(None));
        assert_eq!(
            problem(vec![top.clone(), hashes[3].clone()], head.clone()),
            Ok(Some(format!(
                "Cannot edit selected commits: commit {} was pushed to protected branch 'origin/main'",
                hashes[3]
            )))
        );
        assert_eq!(
            problem(vec![hashes[3][..10].to_string()], head),
            Ok(Some(format!(
                "Cannot edit selected commit: commit {} was pushed to protected branch 'origin/main'",
                hashes[3]
            )))
        );
        assert_eq!(
            problem(vec![top], hashes[2].clone()),
            Ok(Some(
                "Cannot edit selected commit: repository state was changed".to_string()
            ))
        );
    }

    // A rebase stopped at an `edit` of commit `A`, with git and a fake sequence editor.
    fn stopped_at_edit(harness: &RepoHarness, base: &str) {
        let script = harness.path().join(".git/edit.sh");
        fs::write(
            &script,
            "#!/bin/sh\nsed -e '1s/^pick/edit/' \"$1\" > \"$1.new\" && mv \"$1.new\" \"$1\"\n",
        )
        .unwrap();
        let status = Command::new("chmod")
            .arg("+x")
            .arg(&script)
            .status()
            .unwrap();
        assert!(status.success());
        let output = crate::test_support::git_command()
            .current_dir(harness.path())
            .args(["rebase", "-i", base])
            .env("GIT_SEQUENCE_EDITOR", &script)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(harness.in_git_dir("rebase-merge"));
    }

    #[test]
    fn during_a_merge_gives_cannot_reword_during_merge() {
        let harness = RepoHarness::new();
        harness.diverged();
        harness.git_fails(&["merge", "side"]);
        let head = harness.head();

        assert_eq!(
            checked(&harness, &[&head], Kind::Reword),
            Err("Cannot reword during merge".to_string())
        );
        // Drop and Squash are not checked here: it is `rebase::Process` that refuses.
        harness.git(&["merge", "--abort"]);

        let (harness, hashes) = four();
        stopped_at_edit(&harness, &hashes[0]);
        let current = harness.head();
        assert_eq!(current, hashes[1]);
        assert_eq!(
            checked(&harness, &[&hashes[0]], Kind::Reword),
            Err("Selected commit has 0 parents".to_string())
        );
        // With the rebase stopped, HEAD can be edited, with amend; another commit cannot.
        assert_eq!(checked(&harness, &[&current], Kind::Reword), Ok(()));
        let rewritten = reworded(&harness, &current, "A new").unwrap();
        assert_eq!(rewritten.outcome, Outcome::Done);
        assert_eq!(message_of(&harness, "HEAD"), "A new");
        assert!(harness.in_git_dir("rebase-merge"));
        harness.git(&["rebase", "--continue"]);
        assert_eq!(subjects(&harness), ["C", "B", "A new", "base"]);

        let (harness, hashes) = four();
        stopped_at_edit(&harness, &hashes[1]);
        assert_eq!(
            checked(&harness, &[&hashes[1]], Kind::Reword),
            Err("Cannot reword during rebase".to_string())
        );
    }

    fn rewriter(strict: bool, todo: Vec<Entry>, reworded: Vec<Reworded>) -> Rewriter {
        Rewriter::new(
            move |_| Ok((todo.clone(), reworded.clone())),
            strict,
            Arc::new(Trivial),
        )
    }

    #[test]
    fn rewriter_writes_the_todo_answers_the_messages_and_delegates_the_rest() {
        let entry = Entry {
            action: Action::Reword,
            hash: "a1b2c3d".to_string(),
            subject: "# A".to_string(),
        };
        let saved = vec![Reworded {
            original: "A\n\nbody".to_string(),
            message: "A new".to_string(),
        }];
        let strict = rewriter(true, vec![entry.clone()], saved.clone());

        assert_eq!(
            strict.edit(EditKind::Todo, "pick a1b2c3d # A\n\u{1} help\n"),
            Some("reword a1b2c3d # A\n".to_string())
        );
        assert_eq!(
            strict.edit(EditKind::Message, "A\n\nbody\n\u{1} help\n"),
            Some("A new".to_string())
        );
        // Without more saved messages, or with another text, the strict one refuses.
        assert_eq!(strict.edit(EditKind::Message, "A\n\nbody\n"), None);
        let other = rewriter(true, vec![entry.clone()], saved.clone());
        other.edit(EditKind::Todo, "pick a1b2c3d # A\n");
        assert_eq!(other.edit(EditKind::Message, "X\nA\n\nbody\n"), None);
        // The one that is not strict asks (here, `Trivial` leaves it the same), and a second list too.
        let lenient = rewriter(false, vec![entry], saved);
        lenient.edit(EditKind::Todo, "pick a1b2c3d # A\n");
        assert_eq!(
            lenient.edit(EditKind::Message, "another\n"),
            Some("another\n".to_string())
        );
        assert_eq!(
            lenient.edit(EditKind::Todo, "pick 1 # x\n"),
            Some("pick 1 # x\n".to_string())
        );
        // `noop` and what the model does not know go to the one that asks; a model failure cancels.
        let noop = rewriter(true, Vec::new(), Vec::new());
        assert_eq!(
            noop.edit(EditKind::Todo, "noop\n"),
            Some("noop\n".to_string())
        );
        let failing = Rewriter::new(
            |_| Err(TodoError::Invalid("bad".into())),
            true,
            Arc::new(Trivial),
        );
        assert_eq!(failing.edit(EditKind::Todo, "pick 1 # x\n"), None);
        assert_eq!(failing.failure(), Some("bad".to_string()));
        let unstructured = Rewriter::new(|_| Err(TodoError::Unstructured), true, Arc::new(Trivial));
        assert_eq!(
            unstructured.edit(EditKind::Todo, "exec make\n"),
            Some("exec make\n".to_string())
        );
    }

    fn uncommitted(harness: &RepoHarness, hash: &str) -> Result<Vec<String>, String> {
        let (hash, head) = (hash.to_string(), harness.head());
        harness.run(move |repo| ok_or_text(uncommit(repo, &hash, &head)))
    }

    fn single(harness: &RepoHarness, hash: &str, action: Single) -> Result<(), String> {
        let hash = hash.to_string();
        harness.run(move |repo| check_single(repo, &hash, action))
    }

    const NOT_LAST: &str = "The selected commit is not the last in the current branch";

    #[test]
    fn undo_commit_keeps_the_changes_and_moves_the_head() {
        let (harness, hashes) = four();
        harness.write("loose.txt", "untracked\n");

        let paths = uncommitted(&harness, &hashes[3]).unwrap();

        // `reset --soft` to the parent: nothing changes on disk, and the commit stays in the stage.
        assert_eq!(paths, ["C.txt"]);
        assert_eq!(harness.head(), hashes[2]);
        assert!(harness.ran(&format!("reset --soft {} --", hashes[2])));
        assert_eq!(harness.read("C.txt"), "C");
        assert_eq!(harness.read("loose.txt"), "untracked\n");
        let status = harness.git(&["status", "--porcelain"]);
        assert!(status.contains("A  C.txt"), "{status}");
        assert!(status.contains("?? loose.txt"), "{status}");
        // There is nothing to save: no copy is left.
        assert_eq!(harness.snapshots(), 0);
    }

    #[test]
    fn undo_commit_returns_the_paths_from_before_and_after_a_rename() {
        let (harness, hashes) = four();
        harness.git(&["mv", "B.txt", "B2.txt"]);
        harness.write("C.txt", "C changed\n");
        harness.git(&["add", "-A"]);
        harness.git(&["commit", "-q", "-m", "renames and changes"]);
        let top = harness.head();

        let mut paths = uncommitted(&harness, &top).unwrap();

        paths.sort();
        assert_eq!(paths, ["B.txt", "B2.txt", "C.txt"]);
        assert_eq!(harness.head(), hashes[3]);
    }

    #[test]
    fn undo_commit_outside_head_says_it_is_not_the_last_and_moves_nothing() {
        let (harness, hashes) = four();

        assert_eq!(
            single(&harness, &hashes[2], Single::Uncommit),
            Err(NOT_LAST.to_string())
        );
        assert_eq!(single(&harness, &hashes[3], Single::Uncommit), Ok(()));
        // Fixup… and Squash Into… are valid for any of the branch.
        assert_eq!(single(&harness, &hashes[2], Single::Autosquash), Ok(()));
        assert_eq!(
            uncommitted(&harness, &hashes[2]).map(std::mem::drop),
            Err(NOT_LAST.to_string())
        );
        assert_eq!(harness.head(), hashes[3]);
    }

    #[test]
    fn undo_commit_of_a_merge_or_the_root_is_rejected_with_its_parents() {
        let (harness, hashes) = four();
        harness.git(&["checkout", "-q", "-b", "side", &hashes[1]]);
        harness.repo.commit("side.txt", b"side", "side");
        harness.git(&["checkout", "-q", "main"]);
        harness.git(&["merge", "-q", "--no-ff", "-m", "merge", "side"]);
        let merge = harness.head();

        assert_eq!(
            single(&harness, &merge, Single::Uncommit),
            Err("Selected commit has 2 parents".to_string())
        );
        assert_eq!(
            single(&harness, &hashes[0], Single::Autosquash),
            Err("Selected commit has 0 parents".to_string())
        );
    }

    #[test]
    fn undo_commit_on_protected_branch_is_rejected() {
        let (harness, hashes) = four();
        let _remote = with_origin(&harness);
        let pushed =
            Err("The commit is already pushed to protected branch 'origin/main'".to_string());

        assert_eq!(single(&harness, &hashes[3], Single::Uncommit), pushed);
        assert_eq!(single(&harness, &hashes[3], Single::Autosquash), pushed);
        assert_eq!(
            uncommitted(&harness, &hashes[3]).map(std::mem::drop),
            pushed
        );
        assert_eq!(harness.head(), hashes[3]);
        // A new commit, not pushed, can be done.
        let top = harness.repo.commit("D.txt", b"D", "D");
        assert_eq!(uncommitted(&harness, &top).unwrap(), ["D.txt"]);
    }

    #[test]
    fn undo_commit_with_a_moved_head_touches_nothing() {
        let (harness, hashes) = four();
        let (hash, stale) = (hashes[3].clone(), hashes[2].clone());

        let refused = harness.run(move |repo| ok_or_text(uncommit(repo, &hash, &stale)));

        assert_eq!(
            refused.map(std::mem::drop),
            Err("Repository has already been changed".to_string())
        );
        assert_eq!(harness.head(), hashes[3]);
    }

    #[test]
    fn undo_commit_does_not_prohibit_any_state_in_progress() {
        // With a rebase stopped at `edit`, HEAD can be undone.
        let (harness, hashes) = four();
        stopped_at_edit(&harness, &hashes[0]);
        let current = harness.head();
        assert_eq!(current, hashes[1]);
        assert_eq!(single(&harness, &current, Single::Uncommit), Ok(()));
        assert_eq!(uncommitted(&harness, &current).unwrap(), ["A.txt"]);
        assert_eq!(harness.head(), hashes[0]);

        // With a half-done merge the check passes and it is git that refuses: its error is reported.
        let harness = RepoHarness::new();
        harness.diverged();
        harness.git_fails(&["merge", "side"]);
        let head = harness.head();
        assert_eq!(single(&harness, &head, Single::Uncommit), Ok(()));
        let failed = uncommitted(&harness, &head).unwrap_err();
        assert!(failed.contains("soft reset"), "{failed}");
        assert_eq!(harness.head(), head);
    }

    #[test]
    fn the_menu_offers_undo_commit_and_fixup_only_with_one_commit() {
        let (harness, hashes) = four();
        let checks = |selected: Vec<String>| harness.run(move |repo| menu_checks(repo, &selected));

        let head = checks(vec![hashes[3].clone()]);
        assert_eq!((head.uncommit, head.autosquash), (None, None));
        let older = checks(vec![hashes[2].clone()]);
        assert_eq!(older.uncommit.as_deref(), Some(NOT_LAST));
        assert_eq!(older.autosquash, None);
        let several = checks(vec![hashes[3].clone(), hashes[2].clone()]);
        assert_eq!((several.uncommit, several.autosquash), (None, None));
        // The root cannot be rewritten: both are turned off with the same reason.
        let root = checks(vec![hashes[0].clone()]);
        let parents = Some("Selected commit has 0 parents".to_string());
        assert_eq!((root.uncommit, root.autosquash), (parents.clone(), parents));
    }

    // A `fixup!` commit on `A`, starting from `four()`.
    fn with_fixup() -> (RepoHarness, Vec<String>) {
        let (harness, hashes) = four();
        harness.write("A.txt", "A fixed\n");
        harness.git(&["add", "A.txt"]);
        harness.git(&["commit", "-q", "-m", "fixup! A"]);
        (harness, hashes)
    }

    fn autosquashed(harness: &RepoHarness, hash: &str) -> Result<rebase::Rebased, String> {
        let base = format!("{hash}~");
        harness.run(move |repo| ok_or_text(autosquash_with(repo, &base, Arc::new(Trivial))))
    }

    #[test]
    fn autosquash_launches_interactive_rebase_without_autosquash_flags() {
        let (harness, hashes) = with_fixup();

        // Without an editor server git does not get to ask, but the command is left in view. What it
        // does with the fixup is decided by the user's `rebase.autoSquash` (`tests/editor.rs`).
        let rebased = autosquashed(&harness, &hashes[1]).unwrap();

        assert_ne!(rebased.outcome, Outcome::Done);
        assert!(harness.ran(&format!("rebase --interactive {}", hashes[0])));
        for flag in ["--autosquash", "--no-autosquash"] {
            assert!(!harness.ran(&format!("{flag} {}", hashes[0])));
        }
        assert_eq!(subjects(&harness), ["fixup! A", "C", "B", "A", "base"]);
        assert!(!harness.in_git_dir("rebase-merge"));
    }

    #[test]
    fn autosquash_does_not_start_with_a_half_done_merge() {
        let harness = RepoHarness::new();
        harness.diverged();
        harness.git_fails(&["merge", "side"]);
        let before = harness.head();
        let base = format!("{before}~");

        let rebased = harness
            .run(move |repo| ok_or_text(autosquash_with(repo, &base, Arc::new(Trivial))))
            .unwrap();

        assert_eq!(rebased.title.as_deref(), Some("Rebase not allowed"));
        assert_eq!(harness.head(), before);
    }

    #[test]
    fn autosquash_with_a_merge_in_the_range_refuses_with_the_parents() {
        let (harness, hashes) = four();
        harness.git(&["checkout", "-q", "-b", "side", &hashes[1]]);
        harness.repo.commit("side.txt", b"side", "side");
        harness.git(&["checkout", "-q", "main"]);
        harness.git(&["merge", "-q", "--no-ff", "-m", "merge", "side"]);
        let merge = harness.head();
        let before = harness.head();

        // The merge stays between the base and HEAD: git would flatten it without `--rebase-merges`.
        let refused = harness
            .run(move |repo| {
                ok_or_text(autosquash_with(
                    repo,
                    &format!("{}~", hashes[2]),
                    Arc::new(Trivial),
                ))
            })
            .map(|_| ());

        assert_eq!(refused, Err(format!("Commit {merge} has 2 parents")));
        assert_eq!(harness.head(), before);
        assert!(!harness.in_git_dir("rebase-merge"));
    }

    #[test]
    fn autosquash_with_the_merge_below_the_base_does_start() {
        let (harness, hashes) = four();
        harness.git(&["checkout", "-q", "-b", "side", &hashes[1]]);
        harness.repo.commit("side.txt", b"side", "side");
        harness.git(&["checkout", "-q", "main"]);
        harness.git(&["merge", "-q", "--no-ff", "-m", "merge", "side"]);
        let above = harness.repo.commit("E.txt", b"E", "E");

        let started = harness.run(move |repo| {
            ok_or_text(autosquash_with(
                repo,
                &format!("{above}~"),
                Arc::new(Trivial),
            ))
        });

        assert!(started.is_ok(), "{:?}", started.map(|_| ()));
        assert!(harness.ran(&format!(
            "rebase --interactive {}",
            harness.git(&["rev-parse", "HEAD~"]).trim()
        )));
    }

    #[test]
    fn autosquash_does_not_accept_a_base_that_starts_with_a_dash() {
        let (harness, _) = four();

        let refused =
            harness.run(|repo| ok_or_text(autosquash_with(repo, "--root", Arc::new(Trivial))));

        assert!(refused.is_err());
    }

    // `commits_of` runs git without a task: with signed commits and `log.showSignature` on, the
    // signature check git prints must not get into the hashes or the messages.
    #[test]
    fn signatures_stay_out_of_the_interactive_rows() {
        let harness = RepoHarness::new();
        harness.repo.sign_commits();
        let hashes: Vec<String> = ["base", "A", "B"]
            .iter()
            .map(|name| {
                harness
                    .repo
                    .commit(&format!("{name}.txt"), name.as_bytes(), name)
            })
            .collect();
        assert!(harness
            .git(&["log", "-1", "--format=%H"])
            .contains("No signature"));

        let first = hashes[1].clone();
        let start = harness
            .run(move |repo| ok_or_text(interactive_start(repo, &first)))
            .unwrap();

        let rows: Vec<(String, String)> = start
            .commits
            .unwrap()
            .into_iter()
            .map(|commit| (commit.full, commit.message))
            .collect();
        assert_eq!(
            rows,
            [
                (hashes[1].clone(), "A".to_string()),
                (hashes[2].clone(), "B".to_string())
            ]
        );
    }

    #[test]
    fn log_entries_from_head_to_the_commit() {
        let (harness, hashes) = four();
        let entries = |commit: &str| {
            let commit = commit.to_string();
            harness.run(move |repo| ok_or_text(interactive_entries(repo, &commit)))
        };

        // From the oldest to the most recent, like git's list: `pick`, the full hash and the subject.
        let picked = entries(&hashes[1]).unwrap().unwrap();
        let expected: Vec<Entry> = [(1, "A"), (2, "B"), (3, "C")]
            .iter()
            .map(|(index, subject)| Entry {
                action: Action::Pick,
                hash: hashes[*index].clone(),
                subject: subject.to_string(),
            })
            .collect();
        assert_eq!(picked, expected);
        assert_eq!(entries(&hashes[3]).unwrap().unwrap().len(), 1);
        // A hash that does not resolve or the root: to native mode.
        assert_eq!(entries(&"0".repeat(40)).unwrap(), None);
        assert_eq!(entries(&hashes[0]).unwrap(), None);
        assert!(entries("--all").is_err());

        // A `fixup!` or `squash!` subject in the range, too.
        harness.repo.commit("D.txt", b"D", "squash! A");
        assert_eq!(entries(&hashes[1]).unwrap(), None);
        assert_eq!(entries(&harness.head()).unwrap(), None);
    }

    #[test]
    fn interactively_rebase_is_off_during_a_rebase_and_with_a_merge() {
        let (harness, hashes) = four();
        let checks = |selected: Vec<String>| harness.run(move |repo| menu_checks(repo, &selected));

        assert_eq!(checks(vec![hashes[2].clone()]).rebase, None);
        assert_eq!(
            checks(vec![hashes[3].clone(), hashes[2].clone()]).rebase,
            None
        );
        let root = checks(vec![hashes[0].clone()]).rebase;
        assert_eq!(root.as_deref(), Some("Selected commit has 0 parents"));
        harness.git(&["checkout", "-q", "-b", "side", &hashes[1]]);
        harness.repo.commit("side.txt", b"side", "side");
        harness.git(&["checkout", "-q", "main"]);
        harness.git(&["merge", "-q", "--no-ff", "-m", "merge", "side"]);
        let merge = checks(vec![harness.head()]).rebase;
        assert_eq!(merge.as_deref(), Some("Selected commit has 2 parents"));

        // `rebase.log.commit.editing.action.prohibit.state.rebasing`, also for HEAD.
        harness.write("C.txt", "clashes\n");
        harness.git(&["commit", "-qam", "touches C"]);
        harness.git(&["checkout", "-q", "-b", "other", &hashes[2]]);
        harness.write("C.txt", "other\n");
        harness.git(&["add", "C.txt"]);
        harness.git(&["commit", "-qm", "other C"]);
        harness.git_fails(&["rebase", "main"]);
        let during = checks(vec![harness.head()]).rebase;
        assert_eq!(during.as_deref(), Some("Cannot rebase during rebase"));
    }

    #[test]
    fn saved_messages_are_valid_for_their_rebase_and_are_forgotten() {
        let (harness, hashes) = four();
        let git_dir = PathBuf::from(harness.git(&["rev-parse", "--absolute-git-dir"]).trim());
        let state = git_dir.join("rebase-merge");
        fs::create_dir_all(&state).unwrap();
        fs::write(state.join("onto"), format!("{}\n", hashes[0])).unwrap();
        let mapping = |original: &str, message: &str| Reworded {
            original: original.to_string(),
            message: message.to_string(),
        };
        remember(&git_dir, vec![mapping("A", "A new"), mapping("C", "C new")]);

        // In order, and only if the text starts with the original.
        assert_eq!(recalled(&git_dir, "B\n"), None);
        assert_eq!(
            recalled(&git_dir, "A\n\u{1} help\n"),
            Some("A new".to_string())
        );
        // Another rebase (another `onto`) does not see them.
        fs::write(state.join("onto"), format!("{}\n", hashes[1])).unwrap();
        assert_eq!(recalled(&git_dir, "C\n"), None);
        fs::write(state.join("onto"), format!("{}\n", hashes[0])).unwrap();
        harness.run(forget);
        assert_eq!(recalled(&git_dir, "C\n"), None);
    }

    // The handler of the `using log` mode for `hash`, with the plan the test dictates.
    fn from_log(
        harness: &RepoHarness,
        hash: &str,
        plan: impl FnOnce(&[Entry]) -> Plan,
    ) -> Interactive {
        let hash = hash.to_string();
        let generated = harness
            .run(move |repo| ok_or_text(interactive_entries(repo, &hash)))
            .unwrap()
            .unwrap();
        let plan = plan(&generated);
        let log = FromLog { generated, plan };
        harness
            .run(move |repo| ok_or_text(Interactive::new(repo, Some(log))))
            .unwrap()
    }

    // The line git writes: the abbreviated hash and the subject.
    fn git_line(harness: &RepoHarness, hash: &str) -> String {
        let short = harness.git(&["rev-parse", "--short", hash]);
        format!("pick {} {}\n", short.trim(), message_of(harness, hash))
    }

    #[test]
    fn a_git_todo_with_an_extra_line_gives_invalid_todo() {
        let (harness, hashes) = four();
        let handler = from_log(&harness, &hashes[2], |generated| Plan {
            entries: generated.to_vec(),
            messages: Vec::new(),
        });
        let mut todo: String = hashes[2..]
            .iter()
            .map(|hash| git_line(&harness, hash))
            .collect();
        todo.push_str(&git_line(&harness, &hashes[1]));

        // Writing the plan would leave out the extra line, and git would take it for a `drop`.
        assert_eq!(handler.edit(EditKind::Todo, &todo), None);
        assert_eq!(handler.failure(), Some(INVALID_TODO.to_string()));

        // One fewer is not valid either.
        let handler = from_log(&harness, &hashes[2], |generated| Plan {
            entries: generated.to_vec(),
            messages: Vec::new(),
        });
        let short = git_line(&harness, &hashes[2]);
        assert_eq!(handler.edit(EditKind::Todo, &short), None);
        assert_eq!(handler.failure(), Some(INVALID_TODO.to_string()));
    }

    #[test]
    fn the_written_todo_carries_gits_subject_and_not_the_dialogs() {
        let (harness, hashes) = four();
        let handler = from_log(&harness, &hashes[2], |generated| {
            let mut entries = generated.to_vec();
            entries[0].subject = "x\nexec touch injected".to_string();
            entries[1].action = Action::Drop;
            entries[1].subject = "another subject".to_string();
            Plan {
                entries,
                messages: Vec::new(),
            }
        });
        let todo: String = hashes[2..]
            .iter()
            .map(|hash| git_line(&harness, hash))
            .collect();

        let written = handler.edit(EditKind::Todo, &todo);

        let expected = format!("pick {} B\ndrop {} C\n", hashes[2], hashes[3]);
        assert_eq!(written, Some(expected));
    }

    #[test]
    fn a_plan_with_a_line_break_in_a_row_is_rejected() {
        let entry = |subject: &str, hash: &str| Entry {
            action: Action::Pick,
            hash: hash.to_string(),
            subject: subject.to_string(),
        };
        let lines = vec![entry("A", "a1b2c3d")];
        let plan = |entries: Vec<Entry>| Plan {
            entries,
            messages: Vec::new(),
        };

        assert_eq!(check_plan(&plan(lines.clone()), &lines), Ok(()));
        for subject in ["A\nexec touch x", "A\rexec touch x"] {
            assert_eq!(
                check_plan(&plan(vec![entry(subject, "a1b2c3d")]), &lines),
                Err(LINE_BREAK.to_string())
            );
        }
        // The message of a `reword` goes to the message editor, not to the todo: its line breaks are valid.
        let reworded = Plan {
            entries: vec![Entry {
                action: Action::Reword,
                ..entry("A", "a1b2c3d")
            }],
            messages: vec![NewMessage {
                hash: "a1b2c3d".to_string(),
                message: "A new\n\nexec touch x".to_string(),
            }],
        };
        assert_eq!(check_plan(&reworded, &lines), Ok(()));
    }
}
