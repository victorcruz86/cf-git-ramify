use std::collections::{BTreeSet, HashMap, VecDeque};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError, TryLockError};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use tauri::menu::{Menu, MenuItem, MenuItemKind, PredefinedMenuItem, Submenu};
use tauri::{
    AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder, Wry,
};

use crate::askpass::{PromptKind, Server};
use crate::branch_info::BranchSync;
use crate::changelists::{ChangelistError, Changelists};
use crate::commit::{
    self, CommitError, CommitOptions, CommitRequest, CommitWarning, RecoveryReport,
};
use crate::conflicts;
use crate::console::{self, Console, ConsoleLine};
use crate::diff::{self, ChangedFile, FileSides};
use crate::editor::{self, Prompt};
use crate::git::{self, GitError};
use crate::hunks::{self, Hunk, HunkError, LineSelection};
use crate::idea_import;
use crate::log::commands::SharedLog;
use crate::messages;
use crate::operation::{self, RepoState};
use crate::preserve::{self, Pending};
use crate::rollback::{self, RollbackError};
use crate::settings::{self, Settings};
use crate::snapshots::{self, Snapshot, SnapshotId};
use crate::staging;
use crate::status::{self, FileChange};
use crate::storage;
use crate::task::{Runner, TaskId};
use crate::watch::{self, Watcher};
use crate::worktree_diff::{self, Comparison, WorktreeSides};

#[derive(Debug, Serialize)]
pub struct ApiError {
    pub kind: &'static str,
    pub message: String,
    // Data the interface reads without interpreting the message (today, which selection was lost).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl From<GitError> for ApiError {
    fn from(error: GitError) -> Self {
        match error {
            GitError::NotInstalled => ApiError {
                kind: "git_not_found",
                message: match git::configured_binary() {
                    Some(path) => format!(
                        "Git was not found at \"{}\": check the path in Settings",
                        path.display()
                    ),
                    None => "Git is not installed".to_string(),
                },
                details: None,
            },
            GitError::NotARepository { path } => ApiError {
                kind: "not_a_repository",
                message: format!("{path} is not a Git repository"),
                details: None,
            },
            // A secret never appears in an error: neither the one in the command nor the one git repeats.
            GitError::Failed { args, stderr, .. } => failed(console::hide_credentials(&format!(
                "git {} failed: {}",
                args.join(" "),
                stderr.trim()
            ))),
            GitError::Io(message) => failed(message),
            GitError::Outdated => ApiError {
                kind: "outdated",
                message: "The file has changed since it was read: nothing was written".to_string(),
                details: None,
            },
            GitError::Cancelled => ApiError {
                kind: "cancelled",
                message: "The operation was cancelled".to_string(),
                details: None,
            },
            GitError::IndexLocked => failed(
                "Another Git process is using the repository. Wait for it to finish and try again"
                    .to_string(),
            ),
        }
    }
}

impl From<ChangelistError> for ApiError {
    fn from(error: ChangelistError) -> Self {
        failed(error.to_string())
    }
}

impl From<RollbackError> for ApiError {
    fn from(error: RollbackError) -> Self {
        failed(error.to_string())
    }
}

impl From<HunkError> for ApiError {
    fn from(error: HunkError) -> Self {
        let message = error.to_string();
        match error {
            HunkError::Git(error) => error.into(),
            HunkError::Lost { path, hunks } => ApiError {
                kind: "selection_lost",
                message,
                details: Some(serde_json::json!({ "path": path, "hunks": hunks })),
            },
            HunkError::Unmerged(_) => ApiError {
                kind: "unmerged",
                message,
                details: None,
            },
            HunkError::NoThreeSides(_) => ApiError {
                kind: "no_three_sides",
                message,
                details: None,
            },
            _ => failed(message),
        }
    }
}

impl From<CommitError> for ApiError {
    fn from(error: CommitError) -> Self {
        let kind = match &error {
            CommitError::Unmerged => "unmerged",
            CommitError::PartialMerge => "partial_merge",
            CommitError::NothingToCommit => "nothing_to_commit",
            CommitError::Gpg(_) => "gpg_failed",
            CommitError::Hook(_) => "hook_failed",
            CommitError::Restore { .. } => "restore_failed",
            CommitError::Git(error) => return error.clone().into(),
            CommitError::CommitRunning => "commit_running",
            CommitError::OutsideRepository(_)
            | CommitError::Snapshot(_)
            | CommitError::Journal(_)
            | CommitError::IndexBusy
            | CommitError::UnreadableJournal(_)
            | CommitError::RecoverySkipped(_) => "failed",
        };
        ApiError {
            kind,
            message: error.to_string(),
            details: None,
        }
    }
}

pub(crate) fn failed(message: String) -> ApiError {
    ApiError {
        kind: "failed",
        message,
        details: None,
    }
}

const REPO_CHANGED_EVENT: &str = "repo-changed";
const DIFF_OPEN_EVENT: &str = "diff-open";
const DIFF_CLOSE_EVENT: &str = "diff-close";
const DIFF_FLUSH_EVENT: &str = "diff-flush";
const DIFF_FLUSH_WAIT: Duration = Duration::from_secs(10);
const QUIT_WAIT: Duration = Duration::from_secs(60);
const LEAVE_REFUSED_EVENT: &str = "leave-refused";
const SETTINGS_CHANGED_EVENT: &str = "settings-changed";
const OPEN_SETTINGS_EVENT: &str = "open-settings";
pub const QUIT_MENU_ID: &str = "rama-quit";
pub const SETTINGS_MENU_ID: &str = "rama-settings";
const SETTINGS_MENU_POSITION: usize = 2;
const GIT_ACTION_EVENT: &str = "git-action";
const WINDOW_MENU: &str = "Window";

enum GitMenuEntry {
    // Id of the action (the same one the shortcut uses in the interface) and text. No native accelerator: the
    // shortcuts are handled by the interface, and the order between menu and keyboard cannot be checked.
    Item(&'static str, &'static str),
    // A submenu, like `Git.MainMenu.RebaseActions`.
    Group(&'static str, &'static [GitMenuEntry]),
    Separator,
}

use GitMenuEntry::{Group, Item, Separator};

const REBASE_GROUP: &str = "Git.MainMenu.RebaseActions";
// What only applies with an operation in progress. The native menu cannot hide an entry: it starts
// disabled and the interface enables it according to the repository state.
const ONGOING_MENU: [&str; 8] = [
    "Git.Merge.Abort",
    REBASE_GROUP,
    "Git.Rebase.Abort",
    "Git.Rebase.Continue",
    "Git.Rebase.Skip",
    "Git.Revert.Abort",
    "Git.CherryPick.Abort",
    // Only applies with conflicts (`GitResolveConflictsAction.update`).
    "Git.ResolveConflicts",
];

fn starts_disabled(id: &str) -> bool {
    ONGOING_MENU.contains(&id)
}

// What only applies with the repository in a normal state (`GitMerge.update`, `GitRebase.update`): it starts
// enabled and the interface disables it with an operation in progress or a detached HEAD.
const NORMAL_MENU: [&str; 2] = ["Git.Merge", "Git.Rebase"];

fn driven_by_state(id: &str) -> bool {
    starts_disabled(id) || NORMAL_MENU.contains(&id)
}

// The entries with an action, groups included and in order.
fn menu_items(entries: &[GitMenuEntry]) -> Vec<(&'static str, &'static str)> {
    entries
        .iter()
        .flat_map(|entry| match entry {
            Item(id, text) => vec![(*id, *text)],
            Group(_, children) => menu_items(children),
            Separator => Vec::new(),
        })
        .collect()
}

// `Git.MainMenu` of plugin.xml, in its order. What the app does not have yet (Stash…) comes in with its phase.
const GIT_MENU: [GitMenuEntry; 25] = [
    Item("CheckinProject", "Commit…"),
    Item("Vcs.Push", "Push…"),
    Item("Vcs.UpdateProject", "Update Project…"),
    Item("Git.Pull", "Pull…"),
    Item("Git.Fetch", "Fetch"),
    Separator,
    Item("Git.Merge", "Merge…"),
    Item("Git.Merge.Abort", "Abort Merge"),
    Item("Git.Rebase", "Rebase…"),
    Group(
        REBASE_GROUP,
        &[
            Item("Git.Rebase.Abort", "Abort Rebase"),
            Item("Git.Rebase.Continue", "Continue Rebase"),
            Item("Git.Rebase.Skip", "Skip Commit"),
        ],
    ),
    Item("Git.ResolveConflicts", "Resolve Conflicts…"),
    Separator,
    Item("Git.Branches", "Branches…"),
    Item("Git.CreateNewBranch", "New Branch…"),
    Item("Git.Tag", "New Tag…"),
    Item("Git.Reset", "Reset HEAD…"),
    Separator,
    Item("Git.Configure.Remotes", "Manage Remotes…"),
    Item("Git.Clone", "Clone…"),
    Item("Git.Init", "Create Git Repository…"),
    Separator,
    Item("Git.Revert.Abort", "Abort Revert"),
    Item("Git.CherryPick.Abort", "Abort Cherry-Pick"),
    Separator,
    Item("Git.OpenExcludeFile", "Open .git/info/exclude"),
];
const OPEN_ACTION: &str = "the repository was not opened";
const QUIT_ACTION: &str = "the application was not quit";
const CLOSE_ACTION: &str = "the window was not closed";
pub(crate) const SAVE_ACTION: &str = "nothing was done";
pub const MAIN_WINDOW: &str = "main";
pub const DIFF_WINDOW: &str = "diff";
const DIFF_WINDOW_URL: &str = "index.html?window=diff";
pub const MERGE_WINDOW: &str = "merge";
const MERGE_WINDOW_URL: &str = "index.html?window=merge";
const MERGE_CLOSE_EVENT: &str = "merge-close";
const MERGE_DONE_EVENT: &str = "merge-done";
const MERGE_LEAVE_EVENT: &str = "merge-leave";
// The page acknowledges right away, like the diff one when saving; then the user decides, with the
// Cancel question in view.
const MERGE_ACK_WAIT: Duration = Duration::from_secs(10);
const MERGE_ANSWER_WAIT: Duration = Duration::from_secs(600);
const OPEN_LOCK_WAIT: Duration = Duration::from_millis(300);
const LOCK_POLL: Duration = Duration::from_millis(20);

// The diff window can be closed with the main one open: the main one still needs the notices.
pub fn owns_watcher(label: &str) -> bool {
    label == MAIN_WINDOW
}

// Closing the diff window saves what was written; closing the merge one is its Cancel, with its questions.
fn close_event_of(label: &str) -> &'static str {
    if label == MERGE_WINDOW {
        MERGE_CLOSE_EVENT
    } else {
        DIFF_CLOSE_EVENT
    }
}

pub fn request_close(app: &AppHandle, label: &str) {
    let _ = app.emit_to(label, close_event_of(label), ());
}

// When the window is closed (it is only hidden) no one is looking: the watcher is released.
pub fn stop_watching(state: &AppState) {
    replace_watcher(state, None);
}

// When the window is shown again, watching the session's repository resumes.
pub fn resume_watching(
    state: &AppState,
    on_change: impl Fn() + Send + 'static,
) -> Result<(), GitError> {
    if state.watcher.lock().unwrap().is_some() {
        return Ok(());
    }
    let path = state
        .repo
        .lock()
        .unwrap()
        .as_ref()
        .map(|repo| repo.path.clone());
    let Some(path) = path else { return Ok(()) };
    replace_watcher(state, Some(watch::start(path, on_change)?));
    Ok(())
}

// The old watcher is dropped outside the lock, which must not stay held while it finishes.
fn replace_watcher(state: &AppState, watcher: Option<Watcher>) {
    let old = std::mem::replace(&mut *state.watcher.lock().unwrap(), watcher);
    drop(old);
}

pub(crate) fn repo_changed(app: &AppHandle) {
    let _ = app.emit(REPO_CHANGED_EVENT, ());
}

fn notifier(app: AppHandle) -> impl Fn() + Send + 'static {
    move || repo_changed(&app)
}

// If watching cannot be resumed there is no other error path to the interface: the
// event is emitted anyway and the interface reads the state again.
pub fn resume_and_notify(app: &AppHandle) {
    let _ = resume_watching(&app.state::<AppState>(), notifier(app.clone()));
    notifier(app.clone())();
    app.state::<AppState>().branch_sync.visible();
}

pub struct RepoSession {
    pub path: PathBuf,
}

// Per repository: commit, rollback and restoring a copy go through the index in several steps, and
// another operation in between would take an intermediate state for the user's.
#[derive(Default)]
pub struct RepoLocks(Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>);

impl RepoLocks {
    fn lock_for(&self, repo: &Path) -> Arc<Mutex<()>> {
        let mut locks = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        locks.entry(repo.to_path_buf()).or_default().clone()
    }

    pub fn run<T>(&self, repo: &Path, work: impl FnOnce() -> T) -> T {
        let lock = self.lock_for(repo);
        let _held = lock.lock().unwrap_or_else(PoisonError::into_inner);
        work()
    }

    // `Mutex` cannot wait with a limit: it retries until `wait`.
    pub fn try_run<T>(&self, repo: &Path, wait: Duration, work: impl FnOnce() -> T) -> Option<T> {
        let lock = self.lock_for(repo);
        let deadline = Instant::now() + wait;
        let _held = loop {
            match lock.try_lock() {
                Ok(held) => break held,
                Err(TryLockError::Poisoned(error)) => break error.into_inner(),
                Err(TryLockError::WouldBlock) if Instant::now() >= deadline => return None,
                Err(TryLockError::WouldBlock) => thread::sleep(LOCK_POLL),
            }
        };
        Some(work())
    }
}

pub fn commit_locked(
    locks: &RepoLocks,
    repo: &Path,
    dir: &Path,
    request: &CommitRequest,
) -> Result<String, ApiError> {
    let hash = locks.run(repo, || commit::commit(repo, dir, request))?;
    remember_message(dir, &request.message);
    Ok(hash)
}

// The history is an aid: not being able to save it does not turn an already made commit into an error.
fn remember_message(dir: &Path, message: &str) {
    let _ = messages::push(dir, message);
}

pub fn rollback_locked(
    locks: &RepoLocks,
    repo: &Path,
    dir: &Path,
    paths: &[String],
    delete_added: bool,
) -> Result<String, ApiError> {
    locks.run(repo, || {
        commit::ensure_idle(repo, dir)?;
        Ok(rollback::rollback_paths(repo, dir, paths, delete_added)?)
    })
}

// Undoing lines writes to the file on disk: it goes with the other operations on the repository.
pub fn lines_rollback_locked(
    locks: &RepoLocks,
    repo: &Path,
    dir: &Path,
    path: &str,
    selection: &[LineSelection],
) -> Result<String, ApiError> {
    locks.run(repo, || {
        commit::ensure_idle(repo, dir)?;
        // With a conflict, undoing lines would break the markers: it is reported before looking at anything else.
        let unmerged = commit::conflicted(repo, &[path.to_string()]).map_err(HunkError::Git)?;
        if let Some(found) = unmerged.into_iter().next() {
            return Err(HunkError::Unmerged(found).into());
        }
        Ok(hunks::rollback_lines(repo, dir, path, selection)?)
    })
}

// Saving writes to the file on disk: it goes with the other operations on the repository. It does not keep a
// copy: the user is editing the file.
pub fn file_write_locked(
    locks: &RepoLocks,
    repo: &Path,
    dir: &Path,
    path: &str,
    content: &str,
    expected_hash: &str,
) -> Result<String, ApiError> {
    locks.run(repo, || {
        commit::ensure_idle(repo, dir)?;
        Ok(worktree_diff::write(repo, path, content, expected_hash)?)
    })
}

// A file's selections for a partial commit.
#[derive(Debug, Clone, Deserialize)]
pub struct FileSelection {
    pub path: String,
    pub selection: Vec<LineSelection>,
}

// Like `commit_locked`, and the partial content of each selected file is built here,
// with the lock already taken and from what is on disk at that moment: the bytes do not travel
// to the interface and back, and the commit cannot carry content older than the file.
pub fn commit_selected_locked(
    locks: &RepoLocks,
    repo: &Path,
    dir: &Path,
    mut request: CommitRequest,
    selections: &[FileSelection],
) -> Result<String, ApiError> {
    let hash = locks.run(repo, || {
        // Before looking at the hunks: on a file in conflict or with a half-done commit the
        // selection can say nothing, and the useful error is that one, not "lost".
        commit::ensure_idle(repo, dir)?;
        if commit::has_unmerged(repo).map_err(CommitError::Git)? {
            return Err(CommitError::Unmerged.into());
        }
        // Two contents for the same file make no sense: one is not chosen over the other.
        let mut taken: BTreeSet<String> = request
            .partial
            .iter()
            .map(|file| file.path.clone())
            .collect();
        for file in selections {
            if !taken.insert(file.path.clone()) {
                return Err(failed(format!(
                    "\"{}\" appears more than once in the partial commit",
                    file.path
                )));
            }
            request
                .partial
                .push(hunks::partial_file(repo, &file.path, &file.selection)?);
        }
        Ok::<_, ApiError>(commit::commit(repo, dir, &request)?)
    })?;
    remember_message(dir, &request.message);
    Ok(hash)
}

// Touching the stage goes with the other operations on the repository. Returns the saved copy, if there was
// something that was only in the index.
fn staging_locked(
    locks: &RepoLocks,
    repo: &Path,
    dir: &Path,
    work: impl FnOnce() -> Result<Option<SnapshotId>, HunkError>,
) -> Result<Option<SnapshotId>, ApiError> {
    locks.run(repo, || {
        commit::ensure_idle(repo, dir)?;
        Ok(work()?)
    })
}

pub fn stage_locked(
    locks: &RepoLocks,
    repo: &Path,
    dir: &Path,
    paths: &[String],
) -> Result<Option<SnapshotId>, ApiError> {
    staging_locked(locks, repo, dir, || staging::stage(repo, dir, paths))
}

pub fn unstage_locked(
    locks: &RepoLocks,
    repo: &Path,
    dir: &Path,
    paths: &[String],
) -> Result<Option<SnapshotId>, ApiError> {
    staging_locked(locks, repo, dir, || staging::unstage(repo, dir, paths))
}

pub fn revert_locked(
    locks: &RepoLocks,
    repo: &Path,
    dir: &Path,
    paths: &[String],
) -> Result<Option<SnapshotId>, ApiError> {
    staging_locked(locks, repo, dir, || staging::revert(repo, dir, paths))
}

pub fn stage_hunks_locked(
    locks: &RepoLocks,
    repo: &Path,
    dir: &Path,
    path: &str,
    selection: &[LineSelection],
) -> Result<Option<SnapshotId>, ApiError> {
    staging_locked(locks, repo, dir, || {
        staging::stage_hunks(repo, dir, path, selection)
    })
}

pub fn unstage_hunks_locked(
    locks: &RepoLocks,
    repo: &Path,
    dir: &Path,
    path: &str,
    selection: &[LineSelection],
) -> Result<Option<SnapshotId>, ApiError> {
    staging_locked(locks, repo, dir, || {
        staging::unstage_hunks(repo, dir, path, selection)
    })
}

pub fn commit_staged_locked(
    locks: &RepoLocks,
    repo: &Path,
    dir: &Path,
    options: &CommitOptions,
    message: &str,
) -> Result<String, ApiError> {
    let hash = locks.run(repo, || commit::commit_staged(repo, dir, options, message))?;
    remember_message(dir, message);
    Ok(hash)
}

pub fn with_signature(options: CommitOptions, settings: &Settings) -> CommitOptions {
    CommitOptions {
        sign: settings.sign_commits,
        signing_key: settings
            .signing_key
            .clone()
            .filter(|_| settings.sign_commits),
        ..options
    }
}

// What was chosen by lines is looked at with the whole file, like IntelliJ.
pub fn warnings_for(
    repo: &Path,
    mut request: CommitRequest,
    selections: &[FileSelection],
    settings: &Settings,
) -> Vec<CommitWarning> {
    request
        .added
        .extend(selections.iter().map(|file| file.path.clone()));
    commit::warnings(repo, &request, settings)
}

pub fn store_settings(base: &Path, settings: &Settings) -> Result<Settings, ApiError> {
    settings::save(base, settings)
        .map_err(|error| failed(format!("Could not save the settings: {error}")))?;
    Ok(settings::load(base))
}

// The ids of the file's current hunks bring the changelists up to date (the one that changed leaves its
// list) and then the requested ones are moved.
pub fn move_hunks_in(
    repo: &Path,
    dir: &Path,
    path: &str,
    hunks: &[String],
    id: &str,
) -> Result<Changelists, ApiError> {
    let current: Vec<String> = hunks::file_hunks(repo, path)?
        .into_iter()
        .map(|hunk| hunk.id)
        .collect();
    edit_changelists(dir, |lists| {
        lists.reconcile_hunks(path, &current);
        lists.move_hunks(path, hunks, id)?;
        // Without hunks in the file's list, the file already belongs to the other one.
        if lists.hunks_in(path, id).len() == current.len() {
            lists.move_paths(&[path.to_string()], id)?;
        }
        Ok(())
    })
}

pub fn restore_locked(
    locks: &RepoLocks,
    repo: &Path,
    dir: &Path,
    id: &str,
) -> Result<String, ApiError> {
    locks.run(repo, || {
        commit::ensure_idle(repo, dir)?;
        snapshots::restore(dir, repo, id)
            .map_err(|error| failed(format!("Could not restore the copy: {error}")))
    })
}

// Opening waits for another operation only a moment: whoever holds it may be in a long
// hook, and without the lock its journal would look like an interrupted commit's. It only prevents opening a
// journal that cannot be read; any other failure goes in the report and, if the journal remains,
// is retried on the next operation or when reopening.
pub fn recover_on_open(
    locks: &RepoLocks,
    repo: &Path,
    dir: &Path,
) -> Result<Option<RecoveryReport>, ApiError> {
    let report = match locks.try_run(repo, OPEN_LOCK_WAIT, || commit::recover(repo, dir)) {
        None => RecoveryReport {
            pending: true,
            ..Default::default()
        },
        Some(Ok(report)) => report,
        Some(Err(error @ CommitError::UnreadableJournal(_))) => return Err(error.into()),
        Some(Err(error)) => RecoveryReport {
            error: Some(error.to_string()),
            ..Default::default()
        },
    };
    Ok((!report.is_empty()).then_some(report))
}

pub struct AppState {
    pub operations: Arc<RepoLocks>,
    pub console: Arc<Console>,
    // Shares `operations`: a write task and a commit do not overlap.
    pub tasks: Runner,
    // `None` if the socket could not be opened: the console says why, and git fails when asking for
    // credentials instead of hanging.
    pub askpass: Mutex<Option<Server>>,
    // The same for git's editor: without it, an interactive rebase fails instead of waiting.
    pub editor: Mutex<Option<editor::Server>>,
    pub repo: Mutex<Option<RepoSession>>,
    // Shared with the tasks that open views: they finish after the command returns.
    pub log: SharedLog,
    // Dropping it stops watching the previous repository.
    pub watcher: Mutex<Option<Watcher>>,
    pub branch_sync: BranchSync,
    // The file the diff window must show. It does not go in the URL: a reused window is not
    // loaded again.
    pub diff_path: Mutex<Option<String>>,
    // Whoever waits for the diff window to save what was written.
    pub diff_flush: Mutex<VecDeque<Sender<bool>>>,
    // The merge window's file, while it is open.
    pub merge_path: Mutex<Option<String>>,
    // Whoever waits for the merge window to say whether it can be closed.
    pub merge_leave: Mutex<VecDeque<Sender<LeaveReply>>>,
    // The merge page already listens to its notices. Before that it has loaded nothing, and there is no merge to lose.
    pub merge_ready: Mutex<bool>,
}

impl Default for AppState {
    fn default() -> Self {
        let operations = Arc::<RepoLocks>::default();
        let console = Arc::<Console>::default();
        Self {
            tasks: Runner::new(operations.clone(), console.clone()),
            askpass: Mutex::default(),
            editor: Mutex::default(),
            operations,
            console,
            repo: Mutex::default(),
            log: SharedLog::default(),
            watcher: Mutex::default(),
            branch_sync: BranchSync::default(),
            diff_path: Mutex::default(),
            diff_flush: Mutex::default(),
            merge_path: Mutex::default(),
            merge_leave: Mutex::default(),
            merge_ready: Mutex::default(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct RepoInfo {
    pub path: String,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub recovery: Option<RecoveryReport>,
    // What was stashed that did not come back in an earlier session.
    pub pending: Vec<Pending>,
    // The half-done operation (a merge, a rebase…), which can be aborted, and the widget text.
    pub state: RepoState,
}

// The session always works on the repository root, even if opened from a subfolder:
// `status` gives paths relative to the root and the other operations must start from the same place.
pub fn repo_root(path: &Path) -> Result<PathBuf, GitError> {
    let output = git::run(path, &["rev-parse", "--show-toplevel"])?;
    let text = String::from_utf8_lossy(&output);
    Ok(PathBuf::from(text.strip_suffix('\n').unwrap_or(&text)))
}

pub fn repo_info(path: &Path) -> Result<RepoInfo, GitError> {
    let root = repo_root(path)?;
    Ok(RepoInfo {
        head: optional_line(&root, &["rev-parse", "-q", "--verify", "HEAD"])?,
        branch: optional_line(&root, &["symbolic-ref", "-q", "--short", "HEAD"])?,
        path: root.display().to_string(),
        recovery: None,
        pending: Vec::new(),
        state: operation::state(&root)?,
    })
}

// If it cannot be read it does not prevent opening: the entries remain in the stash.
fn find_pending(info: &mut RepoInfo, root: &Path, dir: &Path) {
    info.pending = preserve::pending(root, dir).unwrap_or_default();
}

// With -q, these commands exit with 1 and no message if HEAD does not exist or is detached.
fn optional_line(repo: &Path, args: &[&str]) -> Result<Option<String>, GitError> {
    match git::run(repo, args) {
        Ok(output) => Ok(Some(String::from_utf8_lossy(&output).trim().to_string())),
        Err(GitError::Failed { code: Some(1), .. }) => Ok(None),
        Err(error) => Err(error),
    }
}

pub fn first_argument(mut args: impl Iterator<Item = OsString>) -> Option<String> {
    args.nth(1).map(|arg| arg.to_string_lossy().into_owned())
}

#[tauri::command]
pub fn initial_repo() -> Option<String> {
    first_argument(env::args_os())
}

pub fn reveal(window: &WebviewWindow) -> tauri::Result<()> {
    window.show()?;
    window.set_focus()
}

#[tauri::command]
pub fn show_window(window: WebviewWindow) -> Result<(), ApiError> {
    reveal(&window).map_err(|error| failed(error.to_string()))
}

// Closing the diff window from inside, without going through `CloseRequested`.
#[tauri::command]
pub fn hide_window(window: WebviewWindow) -> Result<(), ApiError> {
    window.hide().map_err(|error| failed(error.to_string()))
}

// `action` says what was not done because saving was not possible.
pub fn wait_for_diff(
    answer: &Receiver<bool>,
    wait: Duration,
    action: &str,
) -> Result<(), ApiError> {
    match answer.recv_timeout(wait) {
        Ok(true) => Ok(()),
        Ok(false) => Err(failed(format!(
            "There are unsaved changes in the diff window that could not be saved: {action}"
        ))),
        Err(_) => Err(failed(format!(
            "The diff window is not responding: {action}"
        ))),
    }
}

// Each notice has an answer, in order. Whoever got tired of waiting no longer receives it.
pub fn answer_flush(waiting: &mut VecDeque<Sender<bool>>, saved: bool) {
    while let Some(answer) = waiting.pop_front() {
        if answer.send(saved).is_ok() {
            break;
        }
    }
}

pub fn after_flush(
    flushed: Result<(), ApiError>,
    proceed: impl FnOnce(),
    refuse: impl FnOnce(String),
) {
    match flushed {
        Ok(()) => proceed(),
        Err(error) => refuse(error.message),
    }
}

pub fn is_quit_item(text: &str) -> bool {
    text.starts_with("Quit")
}

// What is written in the diff must reach the disk before another operation reads it, like IntelliJ before
// a commit. Never with the repository lock held: saving needs it too.
pub(crate) async fn flush_diff_window(
    app: &AppHandle,
    state: &State<'_, AppState>,
    action: &'static str,
) -> Result<(), ApiError> {
    let Some(window) = app.get_webview_window(DIFF_WINDOW) else {
        return Ok(());
    };
    let (send, answer) = mpsc::channel();
    state.diff_flush.lock().unwrap().push_back(send);
    app.emit_to(DIFF_WINDOW, DIFF_FLUSH_EVENT, ())
        .map_err(|error| failed(error.to_string()))?;
    let flushed = blocking_api(move || wait_for_diff(&answer, DIFF_FLUSH_WAIT, action)).await;
    if flushed.is_err() {
        // The error is in that window, which may be hidden.
        let _ = reveal(&window);
    }
    flushed
}

#[tauri::command]
pub fn diff_flushed(saved: bool, state: State<'_, AppState>) {
    answer_flush(&mut state.diff_flush.lock().unwrap(), saved);
}

// What the merge page answers to `merge-leave`: the acknowledgement, and then whether it can be closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaveReply {
    Received,
    Answer(bool),
}

// Without an acknowledgement within `ack_wait`, the page is not attending and we do not leave: it may have a half-done merge. The
// answer may arrive without an acknowledgement, if there was nothing to ask.
pub fn wait_for_merge(
    answer: &Receiver<LeaveReply>,
    ack_wait: Duration,
    answer_wait: Duration,
    action: &str,
) -> Result<(), ApiError> {
    let reply = match answer.recv_timeout(ack_wait) {
        Ok(LeaveReply::Received) => answer
            .recv_timeout(answer_wait)
            .map_err(|_| failed(format!("The merge window did not answer in time: {action}")))?,
        Ok(reply) => reply,
        Err(_) => {
            return Err(failed(format!(
                "The merge window is not responding: {action}"
            )))
        }
    };
    match reply {
        LeaveReply::Answer(true) => Ok(()),
        _ => Err(failed(format!(
            "The merge window has a half-done merge that the user chose to keep: {action}"
        ))),
    }
}

// The window exists and its page already listens: only then can it have a half-done merge.
fn must_ask_merge(exists: bool, ready: bool) -> bool {
    exists && ready
}

// What would throw away the half-done merge goes first through the window's Cancel: with the result
// changed it asks `Cancel Merge`, and if the user carries on with the merge nothing is done.
pub(crate) async fn leave_merge_window(
    app: &AppHandle,
    state: &State<'_, AppState>,
    action: &'static str,
) -> Result<(), ApiError> {
    let window = app.get_webview_window(MERGE_WINDOW);
    let ready = *state.merge_ready.lock().unwrap();
    let Some(window) = window.filter(|_| must_ask_merge(true, ready)) else {
        return Ok(());
    };
    let (send, answer) = mpsc::channel();
    state.merge_leave.lock().unwrap().push_back(send);
    let _ = reveal(&window);
    app.emit_to(MERGE_WINDOW, MERGE_LEAVE_EVENT, ())
        .map_err(|error| failed(error.to_string()))?;
    blocking_api(move || wait_for_merge(&answer, MERGE_ACK_WAIT, MERGE_ANSWER_WAIT, action)).await
}

// The acknowledgement does not take anyone out of the queue: the answer arrives afterwards to that same one.
#[tauri::command]
pub fn merge_leave_received(state: State<'_, AppState>) {
    let mut waiting = state.merge_leave.lock().unwrap();
    while let Some(front) = waiting.front() {
        if front.send(LeaveReply::Received).is_ok() {
            break;
        }
        waiting.pop_front();
    }
}

#[tauri::command]
pub fn merge_left(closed: bool, state: State<'_, AppState>) {
    let mut waiting = state.merge_leave.lock().unwrap();
    while let Some(answer) = waiting.pop_front() {
        if answer.send(LeaveReply::Answer(closed)).is_ok() {
            break;
        }
    }
}

#[tauri::command]
pub fn merge_window_ready(state: State<'_, AppState>) {
    *state.merge_ready.lock().unwrap() = true;
}

// Quitting or closing the main one stops watching the repository and, on quit, throws away whatever is
// unsaved: it is saved first, and if that is not possible everything stays as it was and the reason is given.
// Quitting also throws away the half-done merge; closing the main one only hides its window.
fn leave(
    app: AppHandle,
    action: &'static str,
    ask_merge: bool,
    proceed: impl FnOnce(&AppHandle) + Send + 'static,
) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let mut flushed = flush_diff_window(&app, &state, action).await;
        if flushed.is_ok() && ask_merge {
            flushed = leave_merge_window(&app, &state, action).await;
        }
        after_flush(
            flushed,
            || proceed(&app),
            |message| {
                let _ = app.emit_to(MAIN_WINDOW, LEAVE_REFUSED_EVENT, message);
            },
        );
    });
}

// A task between stashing and restoring must finish first: cancelled, it gives back what was stashed. With the
// deadline passed it quits anyway, and what was stashed is noted down for next time.
pub fn quit(app: AppHandle) {
    leave(app, QUIT_ACTION, true, |app| {
        let app = app.clone();
        thread::spawn(move || {
            app.state::<AppState>().tasks.finish_writes(QUIT_WAIT);
            app.exit(0);
        });
    });
}

pub fn close_main(app: AppHandle) {
    leave(app, CLOSE_ACTION, false, |app| {
        for label in [DIFF_WINDOW, MERGE_WINDOW, MAIN_WINDOW] {
            if let Some(window) = app.get_webview_window(label) {
                let _ = window.hide();
            }
        }
        stop_watching(&app.state::<AppState>());
    });
}

// `terminate:`, what the stock Quit does, does not let the exit be stopped: it is replaced with one of our own
// with the same place, name and shortcut. Dock Quit and logout still go through `terminate:`.
pub fn app_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let menu = Menu::default(app)?;
    replace_quit(app, &menu)?;
    add_git_menu(app, &menu)?;
    Ok(menu)
}

fn replace_quit(app: &AppHandle, menu: &Menu<Wry>) -> tauri::Result<()> {
    for item in menu.items()? {
        let MenuItemKind::Submenu(submenu) = item else {
            continue;
        };
        for (position, entry) in submenu.items()?.iter().enumerate() {
            let MenuItemKind::Predefined(predefined) = entry else {
                continue;
            };
            let text = predefined.text()?;
            if is_quit_item(&text) {
                let quit = MenuItem::with_id(app, QUIT_MENU_ID, text, true, Some("CmdOrCtrl+Q"))?;
                submenu.remove(predefined)?;
                submenu.insert(&quit, position)?;
                let settings = MenuItem::with_id(
                    app,
                    SETTINGS_MENU_ID,
                    "Settings…",
                    true,
                    Some("CmdOrCtrl+,"),
                )?;
                submenu.insert(&PredefinedMenuItem::separator(app)?, SETTINGS_MENU_POSITION)?;
                submenu.insert(&settings, SETTINGS_MENU_POSITION)?;
                return Ok(());
            }
        }
    }
    Ok(())
}

// Before `Window`, where IntelliJ puts its own (after Tools). Each entry sends its id to the
// main window, which already listens to them through `git-action`.
fn add_git_menu(app: &AppHandle, menu: &Menu<Wry>) -> tauri::Result<()> {
    let git = Submenu::new(app, "Git", true)?;
    append_entries(app, &git, &GIT_MENU)?;
    let mut before_window = None;
    for (position, item) in menu.items()?.iter().enumerate() {
        if matches!(item, MenuItemKind::Submenu(submenu) if submenu.text()? == WINDOW_MENU) {
            before_window = Some(position);
        }
    }
    match before_window {
        Some(position) => menu.insert(&git, position),
        None => menu.append(&git),
    }
}

fn append_entries(
    app: &AppHandle,
    into: &Submenu<Wry>,
    entries: &[GitMenuEntry],
) -> tauri::Result<()> {
    for entry in entries {
        match entry {
            Item(id, text) => {
                let item = MenuItem::with_id(app, *id, *text, !starts_disabled(id), None::<&str>)?;
                into.append(&item)?;
            }
            Group(id, children) => {
                let rebase = Submenu::with_id(app, *id, "Rebase", !starts_disabled(id))?;
                append_entries(app, &rebase, children)?;
                into.append(&rebase)?;
            }
            Separator => into.append(&PredefinedMenuItem::separator(app)?)?,
        }
    }
    Ok(())
}

// Enables the Git menu entries that apply to the current state (`ids`) and disables the rest of the
// ones that depend on it.
fn set_ongoing_enabled(items: Vec<MenuItemKind<Wry>>, ids: &[String]) -> tauri::Result<()> {
    for kind in items {
        match &kind {
            MenuItemKind::MenuItem(item) if driven_by_state(&item.id().0) => {
                item.set_enabled(ids.iter().any(|id| *id == item.id().0))?
            }
            MenuItemKind::Submenu(submenu) => {
                if driven_by_state(&submenu.id().0) {
                    submenu.set_enabled(ids.iter().any(|id| *id == submenu.id().0))?;
                }
                set_ongoing_enabled(submenu.items()?, ids)?;
            }
            _ => {}
        }
    }
    Ok(())
}

pub fn is_git_menu_item(id: &str) -> bool {
    menu_items(&GIT_MENU).iter().any(|(item, _)| *item == id)
}

// The entries of the operation in progress that apply now: the interface computes them from the repository
// state every time it changes.
#[tauri::command]
pub fn git_menu_enable(ids: Vec<String>, app: AppHandle) -> Result<(), ApiError> {
    let Some(menu) = app.menu() else {
        return Ok(());
    };
    let items = menu.items().map_err(|error| failed(error.to_string()))?;
    set_ongoing_enabled(items, &ids).map_err(|error| failed(error.to_string()))
}

// It does not raise the window: the interface does that if the action is going to be handled, and a modal dialog or
// the lack of a repository leave it as it was.
pub fn git_menu_action(app: &AppHandle, id: &str) {
    let _ = app.emit_to(MAIN_WINDOW, GIT_ACTION_EVENT, id);
}

#[tauri::command]
pub async fn repo_open(
    repo: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<RepoInfo, ApiError> {
    flush_diff_window(&app, &state, OPEN_ACTION).await?;
    leave_merge_window(&app, &state, OPEN_ACTION).await?;
    let path = PathBuf::from(repo);
    let data_base = app.path().app_data_dir().ok();
    let locks = state.operations.clone();
    let app_for_diff = app.clone();
    let (info, watcher) = blocking_api(move || {
        let mut info = repo_info(&path)?;
        let root = PathBuf::from(&info.path);
        if let Some(base) = &data_base {
            let dir = storage::repo_dir(base, &root);
            info.recovery = recover_on_open(&locks, &root, &dir)?;
            find_pending(&mut info, &root, &dir);
        }
        prune_snapshots(data_base, root.clone());
        let watcher = watch::start(root, notifier(app))?;
        Ok((info, watcher))
    })
    .await?;
    *state.repo.lock().unwrap() = Some(RepoSession {
        path: PathBuf::from(&info.path),
    });
    *state.log.lock().unwrap() = None;
    replace_watcher(&state, Some(watcher));
    state.branch_sync.request();
    // It belonged to another repository; what it showed, and its selection, no longer apply.
    if let Some(window) = app_for_diff.get_webview_window(DIFF_WINDOW) {
        let _ = window.destroy();
    }
    *state.merge_path.lock().unwrap() = None;
    *state.merge_ready.lock().unwrap() = false;
    if let Some(window) = app_for_diff.get_webview_window(MERGE_WINDOW) {
        let _ = window.destroy();
    }
    Ok(info)
}

// The half-done operation and the widget text. `repo-changed` makes the interface read it again.
#[tauri::command]
pub async fn repo_state(state: State<'_, AppState>) -> Result<RepoState, ApiError> {
    let repo = current_repo(&state)?;
    blocking_api(move || Ok(operation::state(&repo)?)).await
}

// Removing old copies runs separately and without looking at the result: it must never delay or prevent
// the repository from opening.
fn prune_snapshots(base: Option<PathBuf>, repo: PathBuf) {
    let Some(base) = base else { return };
    std::thread::spawn(move || {
        let _ = snapshots::prune(&storage::repo_dir(&base, &repo), SystemTime::now());
    });
}

const PERF_OUT_VARIABLE: &str = "RAMA_PERF_OUT";

const TASK_CHANGED_EVENT: &str = "task-changed";
const CONSOLE_LINE_EVENT: &str = "console-line";
const ASKPASS_PROMPT_EVENT: &str = "askpass-prompt";
const ASKPASS_DISMISSED_EVENT: &str = "askpass-dismissed";
const EDITOR_REQUEST_EVENT: &str = "editor-request";
const EDITOR_DISMISSED_EVENT: &str = "editor-dismissed";

// Tasks and the console report what happens to the main window. The merge one launches its own
// and needs their result.
pub fn forward_task_events(app: &AppHandle) {
    let state = app.state::<AppState>();
    let handle = app.clone();
    state.tasks.listen(move |event| {
        for label in [MAIN_WINDOW, MERGE_WINDOW] {
            let _ = handle.emit_to(label, TASK_CHANGED_EVENT, event);
        }
    });
    let handle = app.clone();
    state.console.listen(move |line| {
        let _ = handle.emit_to(MAIN_WINDOW, CONSOLE_LINE_EVENT, line);
    });
}

#[derive(Clone, Serialize)]
struct AskpassPrompt {
    id: u64,
    kind: PromptKind,
}

// The window may be hidden with a task running, and the dialog lives in it. If the question
// does not reach the window, no one will answer: `false` is returned and git fails instead of waiting.
pub fn start_askpass(app: &AppHandle) {
    let (asked, dismissed) = (app.clone(), app.clone());
    let server = Server::start(
        move |id, kind| {
            let Some(window) = asked.get_webview_window(MAIN_WINDOW) else {
                return false;
            };
            let _ = reveal(&window);
            resume_and_notify(&asked);
            asked
                .emit_to(
                    MAIN_WINDOW,
                    ASKPASS_PROMPT_EVENT,
                    AskpassPrompt { id, kind },
                )
                .is_ok()
        },
        move |id| {
            let _ = dismissed.emit_to(MAIN_WINDOW, ASKPASS_DISMISSED_EVENT, id);
        },
    );
    let state = app.state::<AppState>();
    match server {
        Ok(server) => {
            state.tasks.use_askpass(server.client_env());
            *state.askpass.lock().unwrap_or_else(PoisonError::into_inner) = Some(server);
        }
        Err(error) => state.tasks.askpass_unavailable(&error),
    }
}

// Dropping the server deletes its temporary directory.
pub fn stop_askpass(app: &AppHandle) {
    app.state::<AppState>()
        .askpass
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
}

// The answer is neither stored nor returned: it continues through the socket to the process that asked.
#[tauri::command]
pub fn askpass_answer(id: u64, answer: Option<String>, state: State<'_, AppState>) {
    let server = state.askpass.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(server) = server.as_ref() {
        server.answer(id, answer);
    }
}

#[derive(Clone, Serialize)]
struct EditorRequest {
    id: u64,
    #[serde(flatten)]
    prompt: Prompt,
}

// Like `start_askpass`: the dialog lives in the main window, which may be hidden with the
// task running. If the request does not arrive, no one will answer and git fails instead of waiting.
pub fn start_editor(app: &AppHandle) {
    let asked = app.clone();
    let dismissed = app.clone();
    let server = editor::Server::start(
        move |id, prompt| {
            let Some(window) = asked.get_webview_window(MAIN_WINDOW) else {
                return false;
            };
            let _ = reveal(&window);
            resume_and_notify(&asked);
            asked
                .emit_to(
                    MAIN_WINDOW,
                    EDITOR_REQUEST_EVENT,
                    EditorRequest { id, prompt },
                )
                .is_ok()
        },
        move |id| {
            let _ = dismissed.emit_to(MAIN_WINDOW, EDITOR_DISMISSED_EVENT, id);
        },
    );
    let state = app.state::<AppState>();
    match server {
        Ok(server) => {
            state.tasks.use_editor(server.handle());
            *state.editor.lock().unwrap_or_else(PoisonError::into_inner) = Some(server);
        }
        Err(error) => state.tasks.editor_unavailable(&error),
    }
}

// Dropping the server deletes its temporary directory.
pub fn stop_editor(app: &AppHandle) {
    app.state::<AppState>()
        .editor
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
}

// It only answers a request that is already waiting, like `askpass_answer`: it does not touch the
// repository. The text continues through the socket to git's editor, which writes it.
#[tauri::command]
pub fn editor_answer(id: u64, text: Option<String>, state: State<'_, AppState>) {
    let server = state.editor.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(server) = server.as_ref() {
        server.answer(id, text);
    }
}

#[tauri::command]
pub fn task_cancel(id: TaskId, state: State<'_, AppState>) {
    state.tasks.cancel(id);
}

#[tauri::command]
pub fn console_lines(state: State<'_, AppState>) -> Vec<ConsoleLine> {
    state.console.lines()
}

#[tauri::command]
pub fn perf_enabled() -> bool {
    env::var_os(PERF_OUT_VARIABLE).is_some()
}

#[tauri::command]
pub fn perf_report(json: String) -> Result<(), ApiError> {
    let path = env::var_os(PERF_OUT_VARIABLE)
        .ok_or_else(|| failed("Measurement mode is not active".to_string()))?;
    fs::write(path, json).map_err(|error| failed(error.to_string()))
}

pub(crate) fn current_repo(state: &State<'_, AppState>) -> Result<PathBuf, ApiError> {
    let repo = state.repo.lock().unwrap();
    repo.as_ref()
        .map(|repo| repo.path.clone())
        .ok_or_else(|| failed("No repository is open".to_string()))
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, GitError> + Send + 'static,
) -> Result<T, ApiError> {
    let result = tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| failed(error.to_string()))?;
    Ok(result?)
}

#[tauri::command]
pub async fn commit_files(
    hash: String,
    state: State<'_, AppState>,
) -> Result<Vec<ChangedFile>, ApiError> {
    let repo = current_repo(&state)?;
    blocking(move || diff::commit_files(&repo, &hash)).await
}

#[tauri::command]
pub async fn file_sides(
    hash: String,
    path: String,
    old_path: Option<String>,
    base: Option<String>,
    no_base: bool,
    state: State<'_, AppState>,
) -> Result<FileSides, ApiError> {
    let repo = current_repo(&state)?;
    blocking(move || {
        diff::sides_against(
            &repo,
            &hash,
            &path,
            old_path.as_deref(),
            base.as_deref(),
            no_base,
        )
    })
    .await
}

#[tauri::command]
pub async fn repo_status(
    include_ignored: bool,
    state: State<'_, AppState>,
) -> Result<Vec<FileChange>, ApiError> {
    let repo = current_repo(&state)?;
    blocking(move || status::read(&repo, include_ignored)).await
}

#[tauri::command]
pub async fn worktree_sides(
    path: String,
    state: State<'_, AppState>,
) -> Result<WorktreeSides, ApiError> {
    let repo = current_repo(&state)?;
    blocking_api(move || Ok(worktree_diff::sides(&repo, &path)?)).await
}

#[tauri::command]
pub async fn worktree_preview(
    path: String,
    content: String,
    state: State<'_, AppState>,
) -> Result<Vec<Hunk>, ApiError> {
    let repo = current_repo(&state)?;
    blocking_api(move || Ok(worktree_diff::preview(&repo, &path, &content)?)).await
}

// Returns the hash of what was saved, which is what the next save expects.
#[tauri::command]
pub async fn file_write(
    path: String,
    content: String,
    expected_hash: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, ApiError> {
    let repo = current_repo(&state)?;
    let dir = repo_data_dir(&app, &state)?;
    let locks = state.operations.clone();
    blocking_api(move || file_write_locked(&locks, &repo, &dir, &path, &content, &expected_hash))
        .await
}

// A single diff window, which is reused. Creating it is asynchronous so as not to block the
// window's thread. The page shows it on mounting, like the main one.
#[tauri::command]
pub async fn diff_window_open(
    path: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), ApiError> {
    let repo = current_repo(&state)?;
    let checked = path.clone();
    blocking_api(move || Ok(worktree_diff::check_inside(&repo, &checked)?)).await?;
    *state.diff_path.lock().unwrap() = Some(path.clone());
    let opened = match app.get_webview_window(DIFF_WINDOW) {
        Some(window) => window
            .set_title(&path)
            .and_then(|()| reveal(&window))
            .and_then(|()| window.emit_to(DIFF_WINDOW, DIFF_OPEN_EVENT, &path)),
        None => {
            WebviewWindowBuilder::new(&app, DIFF_WINDOW, WebviewUrl::App(DIFF_WINDOW_URL.into()))
                .title(&path)
                .inner_size(1200.0, 800.0)
                .visible(false)
                .background_color(tauri::webview::Color(0x1e, 0x1f, 0x22, 0xff))
                .build()
                .map(drop)
        }
    };
    opened.map_err(|error| failed(error.to_string()))
}

#[tauri::command]
pub fn diff_window_path(state: State<'_, AppState>) -> Option<String> {
    state.diff_path.lock().unwrap().clone()
}

// `multiple.file.merge.request.title`, with the full path as `presentableUrl`.
fn merge_title(root: &Path, path: &str) -> String {
    format!("Merge Revisions for {}", root.join(path).display())
}

// Decision 3: with the window open, the file it shows does not change. True if it has to be created.
fn claim_merge(shown: &mut Option<String>, open: bool, path: String) -> bool {
    if open {
        return false;
    }
    *shown = Some(path);
    true
}

// A single merge window. It does not touch files: there is no need to save the diff one first. The lock
// is held while it is created, so that two consecutive requests do not open two.
#[tauri::command]
pub async fn merge_window_open(
    path: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), ApiError> {
    let repo = current_repo(&state)?;
    let checked = path.clone();
    let root = repo.clone();
    blocking_api(move || Ok(worktree_diff::check_inside(&root, &checked)?)).await?;
    let mut shown = state.merge_path.lock().unwrap();
    let existing = app.get_webview_window(MERGE_WINDOW);
    if !claim_merge(&mut shown, existing.is_some(), path.clone()) {
        return existing
            .map_or(Ok(()), |window| reveal(&window))
            .map_err(|error| failed(error.to_string()));
    }
    *state.merge_ready.lock().unwrap() = false;
    let built =
        WebviewWindowBuilder::new(&app, MERGE_WINDOW, WebviewUrl::App(MERGE_WINDOW_URL.into()))
            .title(merge_title(&repo, &path))
            .inner_size(1200.0, 800.0)
            .visible(false)
            .background_color(tauri::webview::Color(0x1e, 0x1f, 0x22, 0xff))
            .build();
    if let Err(error) = built {
        *shown = None;
        return Err(failed(error.to_string()));
    }
    Ok(())
}

#[tauri::command]
pub fn merge_window_path(state: State<'_, AppState>) -> Option<String> {
    state.merge_path.lock().unwrap().clone()
}

// `MergeResult`, with the side's name as the user sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MergeResult {
    Resolved,
    Yours,
    Theirs,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
pub struct MergeDone {
    pub path: String,
    pub result: MergeResult,
}

// The window finishes: the main one receives how, and the window is destroyed.
#[tauri::command]
pub async fn merge_window_done(
    result: MergeResult,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), ApiError> {
    let mut shown = state.merge_path.lock().unwrap();
    if let Some(path) = shown.take() {
        let _ = app.emit_to(MAIN_WINDOW, MERGE_DONE_EVENT, MergeDone { path, result });
    }
    *state.merge_ready.lock().unwrap() = false;
    if let Some(window) = app.get_webview_window(MERGE_WINDOW) {
        window
            .destroy()
            .map_err(|error| failed(error.to_string()))?;
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkingFile {
    // The one from `conflicts::sides`: empty if it does not exist.
    pub hash: String,
    pub separator: String,
    // Still unresolved in the index.
    pub conflicted: bool,
}

// `LoadTextUtil.ConvertResult.majorLineSeparator`; without line breaks, the system one.
fn line_separator(bytes: &[u8]) -> &'static str {
    let (mut cr, mut lf, mut crlf) = (0, 0, 0);
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => {
                crlf += 1;
                index += 1;
            }
            b'\r' => cr += 1,
            b'\n' => lf += 1,
            _ => {}
        }
        index += 1;
    }
    if crlf > cr && crlf > lf {
        "\r\n"
    } else if cr > lf {
        "\r"
    } else {
        "\n"
    }
}

fn working_file(root: &Path, path: &str) -> Result<WorkingFile, ApiError> {
    worktree_diff::check_inside(root, path)?;
    let conflicted = conflicts::is_unmerged(root, path)?;
    match fs::read(root.join(path)) {
        Ok(bytes) => Ok(WorkingFile {
            hash: worktree_diff::content_hash(&bytes),
            separator: line_separator(&bytes).to_string(),
            conflicted,
        }),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(WorkingFile {
                hash: String::new(),
                separator: "\n".to_string(),
                conflicted,
            })
        }
        Err(error) => Err(failed(error.to_string())),
    }
}

// The merge window's working file: its hash, to see whether it changed outside, and the line
// break with which the result is written.
#[tauri::command]
pub async fn merge_working_file(
    path: String,
    state: State<'_, AppState>,
) -> Result<WorkingFile, ApiError> {
    let repo = current_repo(&state)?;
    blocking_api(move || working_file(&repo, &path)).await
}

// The repository's own data (changelists, copies) lives in the app's data folder,
// not in the repository.
pub(crate) fn repo_data_dir(
    app: &AppHandle,
    state: &State<'_, AppState>,
) -> Result<PathBuf, ApiError> {
    let repo = current_repo(state)?;
    let base = app
        .path()
        .app_data_dir()
        .map_err(|error| failed(error.to_string()))?;
    Ok(storage::repo_dir(&base, &repo))
}

// If the file exists but cannot be read we do not continue: saving over it would lose its data.
fn load_changelists(dir: &Path) -> Result<Changelists, ApiError> {
    Changelists::try_load(dir)
        .map_err(|error| failed(format!("Could not read the changelists: {error}")))
}

// Reconciles with the files that have changes and saves only if something changed.
pub fn read_changelists(dir: &Path, paths: &[String]) -> Result<Changelists, ApiError> {
    let mut lists = load_changelists(dir)?;
    if lists.reconcile(paths) {
        lists.save(dir).map_err(|error| failed(error.to_string()))?;
    }
    Ok(lists)
}

pub fn edit_changelists(
    dir: &Path,
    edit: impl FnOnce(&mut Changelists) -> Result<(), ChangelistError>,
) -> Result<Changelists, ApiError> {
    let mut lists = load_changelists(dir)?;
    edit(&mut lists)?;
    lists.save(dir).map_err(|error| failed(error.to_string()))?;
    Ok(lists)
}

// Synchronous commands run one at a time, so reading, changing and saving do not clobber each other.
#[tauri::command]
pub fn changelists_get(
    paths: Vec<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Changelists, ApiError> {
    read_changelists(&repo_data_dir(&app, &state)?, &paths)
}

// Imports IntelliJ's changelists without writing to `.idea`. If the XML cannot be read or
// is not valid the error is returned before touching our own data.
pub fn import_changelists(repo: &Path, dir: &Path) -> Result<Changelists, ApiError> {
    let imported = idea_import::read_project(repo).map_err(failed)?;
    edit_changelists(dir, |lists| {
        lists.merge_import(imported);
        Ok(())
    })
}

#[tauri::command]
pub fn changelists_import(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Changelists, ApiError> {
    let repo = current_repo(&state)?;
    import_changelists(&repo, &repo_data_dir(&app, &state)?)
}

#[tauri::command]
pub fn changelists_create(
    name: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Changelists, ApiError> {
    edit_changelists(&repo_data_dir(&app, &state)?, |lists| {
        lists.create(&name).map(drop)
    })
}

#[tauri::command]
pub fn changelists_rename(
    id: String,
    name: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Changelists, ApiError> {
    edit_changelists(&repo_data_dir(&app, &state)?, |lists| {
        lists.rename(&id, &name)
    })
}

#[tauri::command]
pub fn changelists_remove(
    id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Changelists, ApiError> {
    edit_changelists(&repo_data_dir(&app, &state)?, |lists| lists.remove(&id))
}

#[tauri::command]
pub fn changelists_set_active(
    id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Changelists, ApiError> {
    edit_changelists(&repo_data_dir(&app, &state)?, |lists| lists.set_active(&id))
}

#[tauri::command]
pub fn changelists_move(
    paths: Vec<String>,
    id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Changelists, ApiError> {
    edit_changelists(&repo_data_dir(&app, &state)?, |lists| {
        lists.move_paths(&paths, &id)
    })
}

pub(crate) async fn blocking_api<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| failed(error.to_string()))?
}

// Before touching anything it saves a recoverable copy; returns its identifier.
#[tauri::command]
pub async fn rollback_files(
    paths: Vec<String>,
    delete_added: bool,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let repo = current_repo(&state)?;
    let dir = repo_data_dir(&app, &state)?;
    let locks = state.operations.clone();
    blocking_api(move || rollback_locked(&locks, &repo, &dir, &paths, delete_added)).await
}

// The file's hunks against HEAD; empty if it is treated as a single piece (binary or very large).
#[tauri::command]
pub async fn file_hunks(path: String, state: State<'_, AppState>) -> Result<Vec<Hunk>, ApiError> {
    let repo = current_repo(&state)?;
    blocking_api(move || Ok(hunks::file_hunks(&repo, &path)?)).await
}

// Undoes the chosen lines of the file, after saving a copy; returns its identifier.
#[tauri::command]
pub async fn lines_rollback(
    path: String,
    selection: Vec<LineSelection>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let repo = current_repo(&state)?;
    let dir = repo_data_dir(&app, &state)?;
    let locks = state.operations.clone();
    blocking_api(move || lines_rollback_locked(&locks, &repo, &dir, &path, &selection)).await
}

// It is synchronous like the rest of `changelists_*`, so that reading, changing and saving do not clobber each other.
#[tauri::command]
pub fn changelists_move_hunks(
    path: String,
    hunks: Vec<String>,
    id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Changelists, ApiError> {
    let repo = current_repo(&state)?;
    move_hunks_in(&repo, &repo_data_dir(&app, &state)?, &path, &hunks, &id)
}

// Returns the commit hash. The stage of what was not selected stays as it was, and everything goes back
// to its previous state if the commit fails.
#[tauri::command]
pub async fn commit_changes(
    request: CommitRequest,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let repo = current_repo(&state)?;
    let dir = repo_data_dir(&app, &state)?;
    let locks = state.operations.clone();
    let options = with_signature(request.options, &current_settings(&app)?);
    let request = CommitRequest { options, ..request };
    blocking_api(move || commit_locked(&locks, &repo, &dir, &request)).await
}

// Like `commit_changes`, with the selection of hunks or lines of the partial files instead of their
// content.
#[tauri::command]
pub async fn commit_changes_selected(
    request: CommitRequest,
    selections: Vec<FileSelection>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let repo = current_repo(&state)?;
    let dir = repo_data_dir(&app, &state)?;
    let locks = state.operations.clone();
    let options = with_signature(request.options, &current_settings(&app)?);
    let request = CommitRequest { options, ..request };
    blocking_api(move || commit_selected_locked(&locks, &repo, &dir, request, &selections)).await
}

// The lock, the repository and the folder of its data.
type Operation = (Arc<RepoLocks>, PathBuf, PathBuf);

fn staging_context(app: &AppHandle, state: &State<'_, AppState>) -> Result<Operation, ApiError> {
    Ok((
        state.operations.clone(),
        current_repo(state)?,
        repo_data_dir(app, state)?,
    ))
}

#[tauri::command]
pub async fn staging_stage(
    paths: Vec<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<SnapshotId>, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let (locks, repo, dir) = staging_context(&app, &state)?;
    blocking_api(move || stage_locked(&locks, &repo, &dir, &paths)).await
}

#[tauri::command]
pub async fn staging_unstage(
    paths: Vec<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<SnapshotId>, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let (locks, repo, dir) = staging_context(&app, &state)?;
    blocking_api(move || unstage_locked(&locks, &repo, &dir, &paths)).await
}

#[tauri::command]
pub async fn staging_revert(
    paths: Vec<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<SnapshotId>, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let (locks, repo, dir) = staging_context(&app, &state)?;
    blocking_api(move || revert_locked(&locks, &repo, &dir, &paths)).await
}

#[tauri::command]
pub async fn staging_stage_hunks(
    path: String,
    selection: Vec<LineSelection>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<SnapshotId>, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let (locks, repo, dir) = staging_context(&app, &state)?;
    blocking_api(move || stage_hunks_locked(&locks, &repo, &dir, &path, &selection)).await
}

#[tauri::command]
pub async fn staging_unstage_hunks(
    path: String,
    selection: Vec<LineSelection>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<SnapshotId>, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let (locks, repo, dir) = staging_context(&app, &state)?;
    blocking_api(move || unstage_hunks_locked(&locks, &repo, &dir, &path, &selection)).await
}

#[tauri::command]
pub async fn staging_sides(
    path: String,
    comparison: Comparison,
    state: State<'_, AppState>,
) -> Result<WorktreeSides, ApiError> {
    let repo = current_repo(&state)?;
    blocking_api(move || Ok(worktree_diff::stage_sides(&repo, &path, comparison)?)).await
}

#[tauri::command]
pub async fn staging_preview(
    path: String,
    content: String,
    state: State<'_, AppState>,
) -> Result<Vec<Hunk>, ApiError> {
    let repo = current_repo(&state)?;
    blocking_api(move || Ok(worktree_diff::stage_preview(&repo, &path, &content)?)).await
}

// The index as it is. Returns the commit hash.
#[tauri::command]
pub async fn commit_staged(
    options: CommitOptions,
    message: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let (locks, repo, dir) = staging_context(&app, &state)?;
    let options = with_signature(options, &current_settings(&app)?);
    blocking_api(move || commit_staged_locked(&locks, &repo, &dir, &options, &message)).await
}

#[tauri::command]
pub async fn commit_warnings(
    request: CommitRequest,
    selections: Vec<FileSelection>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<CommitWarning>, ApiError> {
    let repo = current_repo(&state)?;
    let settings = current_settings(&app)?;
    blocking_api(move || Ok(warnings_for(&repo, request, &selections, &settings))).await
}

#[tauri::command]
pub async fn commit_warnings_staged(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<CommitWarning>, ApiError> {
    let repo = current_repo(&state)?;
    let settings = current_settings(&app)?;
    blocking_api(move || Ok(commit::staged_warnings(&repo, &settings))).await
}

#[tauri::command]
pub async fn commit_fix_autocrlf(state: State<'_, AppState>) -> Result<(), ApiError> {
    let repo = current_repo(&state)?;
    blocking(move || commit::fix_autocrlf(&repo)).await
}

// The settings belong to the app, not to a repository.
fn base_dir(app: &AppHandle) -> Result<PathBuf, ApiError> {
    app.path()
        .app_data_dir()
        .map_err(|error| failed(error.to_string()))
}

pub(crate) fn current_settings(app: &AppHandle) -> Result<Settings, ApiError> {
    Ok(settings::load(&base_dir(app)?))
}

fn apply_settings(app: &AppHandle, settings: &Settings) {
    git::set_binary(settings.git_path.as_deref());
    app.state::<AppState>()
        .tasks
        .set_credential_helper(settings.use_credential_helper);
    app.state::<AppState>()
        .tasks
        .set_staging_area(settings.staging_area);
    app.state::<AppState>()
        .tasks
        .set_cherry_pick_suffix(settings.cherry_pick_suffix);
}

// Before any window opens: the first command must already use the configured path.
pub fn load_settings(app: &AppHandle) {
    if let Ok(settings) = current_settings(app) {
        apply_settings(app, &settings);
    }
}

#[tauri::command]
pub fn settings_get(app: AppHandle) -> Result<Settings, ApiError> {
    current_settings(&app)
}

// It takes effect without a restart and in both windows.
#[tauri::command]
pub fn settings_save(settings: Settings, app: AppHandle) -> Result<Settings, ApiError> {
    let check = current_settings(&app)?.incoming_check;
    let saved = store_settings(&base_dir(&app)?, &settings)?;
    apply_settings(&app, &saved);
    // `updateIncomingScheduling`: when turned on, it asks right away.
    if saved.incoming_check != check {
        app.state::<AppState>().branch_sync.request();
    }
    app.emit(SETTINGS_CHANGED_EVENT, &saved)
        .map_err(|error| failed(error.to_string()))?;
    Ok(saved)
}

// The version of a path that has not been saved yet.
#[tauri::command]
pub async fn git_version(path: Option<String>) -> Result<String, ApiError> {
    blocking(move || git::version_of(path.as_deref().filter(|path| !path.trim().is_empty()))).await
}

// The main window may be hidden, and the diff one in front.
pub fn open_settings(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = reveal(&window);
        resume_and_notify(app);
        let _ = window.emit_to(MAIN_WINDOW, OPEN_SETTINGS_EVENT, ());
    }
}

#[tauri::command]
pub fn messages_list(app: AppHandle, state: State<'_, AppState>) -> Result<Vec<String>, ApiError> {
    Ok(messages::list(&repo_data_dir(&app, &state)?))
}

// The whole message of HEAD, for the Amend checkbox.
pub fn last_message(repo: &Path) -> Result<String, GitError> {
    let output = git::run(repo, &["log", "-1", "--format=%B"])?;
    Ok(String::from_utf8_lossy(&output).trim_end().to_string())
}

#[tauri::command]
pub async fn head_message(state: State<'_, AppState>) -> Result<String, ApiError> {
    let repo = current_repo(&state)?;
    blocking(move || last_message(&repo)).await
}

#[tauri::command]
pub fn snapshots_list(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<Snapshot>, ApiError> {
    Ok(snapshots::list(&repo_data_dir(&app, &state)?))
}

// Returns the identifier of the copy of the state that existed just before restoring.
#[tauri::command]
pub async fn snapshots_restore(
    id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, ApiError> {
    let repo = current_repo(&state)?;
    let dir = repo_data_dir(&app, &state)?;
    let locks = state.operations.clone();
    blocking_api(move || restore_locked(&locks, &repo, &dir, &id)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{in_task, TempRepo};
    use std::ffi::{OsStr, OsString};
    use std::os::unix::ffi::OsStrExt;
    use std::process::Command;
    use std::sync::mpsc;
    use std::time::Duration;
    use tempfile::TempDir;

    #[test]
    fn a_git_error_hides_the_credentials() {
        let error = GitError::Failed {
            args: vec!["push".to_string(), "https://u:pass@host/r".to_string()],
            code: Some(128),
            stdout: String::new(),
            stderr: "fatal: unable to access 'https://u:pass@host/r/': 403\n".to_string(),
        };

        let message = ApiError::from(error).message;

        assert!(!message.contains("pass"), "{message}");
        assert_eq!(
            message.matches("https://u:***@host/r").count(),
            2,
            "{message}"
        );
    }

    fn commit_request(path: &str) -> CommitRequest {
        CommitRequest {
            added: vec![path.to_string()],
            message: format!("changes {path}"),
            ..Default::default()
        }
    }

    fn slow_hook(repo: &TempRepo) {
        let hook = repo.path().join(".git/hooks/pre-commit");
        fs::create_dir_all(hook.parent().unwrap()).unwrap();
        fs::write(&hook, "#!/bin/sh\nsleep 1\n").unwrap();
        let mode = std::os::unix::fs::PermissionsExt::from_mode(0o755);
        fs::set_permissions(&hook, mode).unwrap();
    }

    #[test]
    fn a_commit_saves_its_message_in_the_history() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"one\n", "initial");
        std::fs::write(repo.path().join("a.txt"), "two\n").unwrap();
        let dir = TempDir::new().unwrap();
        let locks = RepoLocks::default();

        commit_locked(&locks, repo.path(), dir.path(), &commit_request("a.txt")).unwrap();

        assert_eq!(messages::list(dir.path()), ["changes a.txt"]);
    }

    #[test]
    fn a_commit_with_selection_saves_its_message_in_the_history() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"one\n", "initial");
        std::fs::write(repo.path().join("a.txt"), "two\n").unwrap();
        let dir = TempDir::new().unwrap();
        let locks = RepoLocks::default();
        let hunks = hunks::file_hunks(repo.path(), "a.txt").unwrap();
        let selections = [FileSelection {
            path: "a.txt".to_string(),
            selection: vec![LineSelection {
                hunk: hunks[0].id.clone(),
                lines: None,
            }],
        }];
        let request = CommitRequest {
            message: "partial".to_string(),
            ..Default::default()
        };

        commit_selected_locked(&locks, repo.path(), dir.path(), request, &selections).unwrap();

        assert_eq!(messages::list(dir.path()), ["partial"]);
    }

    #[test]
    fn a_failed_commit_does_not_enter_the_history() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"one\n", "initial");
        let dir = TempDir::new().unwrap();
        let locks = RepoLocks::default();

        assert!(commit_locked(&locks, repo.path(), dir.path(), &commit_request("a.txt")).is_err());

        assert!(messages::list(dir.path()).is_empty());
    }

    #[test]
    fn last_commit_message_whole() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"one\n", "subject\n\nbody");

        assert_eq!(last_message(repo.path()).unwrap(), "subject\n\nbody");
    }

    #[test]
    fn two_commits_at_once_do_not_mix() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        repo.commit("b.txt", b"1\n", "two");
        slow_hook(&repo);
        fs::write(repo.path().join("a.txt"), b"2\n").unwrap();
        fs::write(repo.path().join("b.txt"), b"2\n").unwrap();
        let locks = RepoLocks::default();
        let dir = TempDir::new().unwrap();

        std::thread::scope(|scope| {
            let first = scope
                .spawn(|| commit_locked(&locks, repo.path(), dir.path(), &commit_request("a.txt")));
            std::thread::sleep(Duration::from_millis(500));
            let second = commit_locked(&locks, repo.path(), dir.path(), &commit_request("b.txt"));
            first.join().unwrap().unwrap();
            second.unwrap();
        });

        let files = repo.git(&["log", "-2", "--format=%s", "--name-only"]);
        assert_eq!(
            files.split_whitespace().collect::<Vec<_>>(),
            ["changes", "b.txt", "b.txt", "changes", "a.txt", "a.txt"]
        );
        assert_eq!(repo.git(&["status", "--porcelain"]), "");
    }

    // Checks that `work` does not finish while another one holds the repository, and does afterwards.
    fn assert_waits(locks: &RepoLocks, repo: &Path, work: impl FnOnce() + Send) {
        let (done, finished) = mpsc::channel();
        std::thread::scope(|scope| {
            locks.run(repo, || {
                scope.spawn(move || {
                    work();
                    done.send(()).unwrap();
                });
                let early = finished.recv_timeout(Duration::from_millis(300));
                assert!(early.is_err(), "it did not wait for the lock");
            });
            finished.recv_timeout(Duration::from_secs(10)).unwrap();
        });
    }

    #[test]
    fn rollback_waits_for_another_operation() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        fs::write(repo.path().join("a.txt"), b"2\n").unwrap();
        let locks = RepoLocks::default();
        let dir = TempDir::new().unwrap();
        let paths = vec!["a.txt".to_string()];

        assert_waits(&locks, repo.path(), || {
            rollback_locked(&locks, repo.path(), dir.path(), &paths, false).unwrap();
        });
    }

    #[test]
    fn saving_waits_for_another_operation() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        let locks = RepoLocks::default();
        let dir = TempDir::new().unwrap();
        let hash = worktree_diff::content_hash(b"1\n");

        assert_waits(&locks, repo.path(), || {
            file_write_locked(&locks, repo.path(), dir.path(), "a.txt", "2\n", &hash).unwrap();
        });

        assert_eq!(fs::read(repo.path().join("a.txt")).unwrap(), b"2\n");
    }

    #[test]
    fn saving_does_not_write_if_there_is_a_half_done_commit() {
        let dir = TempDir::new().unwrap();
        let repo = interrupted_repo(dir.path());
        let _held = hold_commit_lock(dir.path());
        let hash = worktree_diff::content_hash(b"1\n");
        let locks = RepoLocks::default();

        let result = file_write_locked(&locks, repo.path(), dir.path(), "a.txt", "2\n", &hash);

        assert_eq!(result.unwrap_err().kind, "commit_running");
        assert_eq!(fs::read(repo.path().join("a.txt")).unwrap(), b"1\n");
    }

    #[test]
    fn only_the_main_window_stops_the_watcher() {
        assert!(owns_watcher(MAIN_WINDOW));
        assert!(!owns_watcher(DIFF_WINDOW));
    }

    #[test]
    fn it_only_continues_if_the_diff_window_saved() {
        let (send, receive) = mpsc::channel();
        send.send(true).unwrap();
        assert!(wait_for_diff(&receive, Duration::from_secs(1), OPEN_ACTION).is_ok());

        send.send(false).unwrap();
        let refused = wait_for_diff(&receive, Duration::from_secs(1), QUIT_ACTION).unwrap_err();
        assert!(refused.message.contains("unsaved"));
        assert!(refused.message.contains(QUIT_ACTION));
    }

    #[test]
    fn if_the_diff_window_does_not_respond_it_does_not_continue() {
        let (_send, receive) = mpsc::channel::<bool>();

        let silent = wait_for_diff(&receive, Duration::from_millis(10), CLOSE_ACTION).unwrap_err();

        assert!(silent.message.contains("not responding"));
        assert!(silent.message.contains(CLOSE_ACTION));
    }

    #[test]
    fn the_diff_window_answer_goes_to_whoever_keeps_waiting() {
        let (gone, _) = mpsc::channel::<bool>();
        let (first, first_answer) = mpsc::channel();
        let (second, second_answer) = mpsc::channel();
        let mut waiting = VecDeque::from([gone, first, second]);

        answer_flush(&mut waiting, true);

        assert_eq!(first_answer.try_recv(), Ok(true));
        assert!(second_answer.try_recv().is_err());
        assert_eq!(waiting.len(), 1);
    }

    #[test]
    fn opening_finds_what_did_not_come_back() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        fs::write(repo.path().join("a.txt"), "local\n").unwrap();
        let dir = TempDir::new().unwrap();
        let (path, data) = (repo.path().to_path_buf(), dir.path().to_path_buf());
        let runner = Runner::new(Arc::default(), Arc::default());
        let saved = in_task(&runner, repo.path(), move |ctx| {
            preserve::save_with_index_copy(ctx, &path, &data, "Update")
        });
        let mut info = repo_info(repo.path()).unwrap();

        find_pending(&mut info, repo.path(), dir.path());

        let saved = saved.unwrap().unwrap();
        assert_eq!(info.pending.len(), 1);
        assert_eq!(info.pending[0].sha, saved.sha);
        assert_eq!(info.state.operation, None);
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["pending"][0]["title"], "Update");
        assert_eq!(json["state"]["operation"], serde_json::Value::Null);
        assert_eq!(json["state"]["label"], "main");
    }

    // Unresolved paths after a `pop` with conflicts: there is no merge or rebase to abort.
    #[test]
    fn with_only_unresolved_paths_there_is_nothing_to_abort() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        fs::write(repo.path().join("a.txt"), "local\n").unwrap();
        repo.git(&["stash", "save", "-q", "stashed"]);
        repo.commit("a.txt", b"other\n", "two");
        let popped = Command::new("git")
            .current_dir(repo.path())
            .args(["stash", "pop"])
            .output()
            .unwrap();
        assert!(!popped.status.success());
        assert_ne!(repo.git(&["ls-files", "--unmerged"]), "");
        let dir = TempDir::new().unwrap();
        let mut info = repo_info(repo.path()).unwrap();

        find_pending(&mut info, repo.path(), dir.path());

        assert_eq!(info.state.operation, None);
    }

    #[test]
    fn a_half_done_merge_can_be_aborted() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        repo.git(&["checkout", "-q", "-b", "side"]);
        repo.commit("a.txt", b"side\n", "side");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit("a.txt", b"main\n", "main");
        let merged = Command::new("git")
            .current_dir(repo.path())
            .args(["merge", "side"])
            .output()
            .unwrap();
        assert!(!merged.status.success());
        let info = repo_info(repo.path()).unwrap();

        assert_eq!(info.state.operation, Some(operation::Operation::Merging));
        assert_eq!(info.state.label, "Merging main");
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["state"]["operation"], "merging");
    }

    #[test]
    fn quit_or_close_only_if_saved() {
        let done = std::cell::RefCell::new(Vec::new());

        after_flush(
            Ok(()),
            || done.borrow_mut().push("proceeds".to_string()),
            |message| done.borrow_mut().push(message),
        );
        after_flush(
            Err(failed("unsaved".to_string())),
            || done.borrow_mut().push("proceeds".to_string()),
            |message| done.borrow_mut().push(message),
        );

        assert_eq!(*done.borrow(), ["proceeds", "unsaved"]);
    }

    // The first line of the body: outside the lock, because `file_write` also takes it, and with `?`
    // so as not to continue if it could not be saved.
    #[test]
    fn whatever_touches_the_repository_saves_the_diff_window_first() {
        let source = include_str!("commands.rs");
        for name in [
            "rollback_files",
            "lines_rollback",
            "commit_changes",
            "commit_changes_selected",
            "commit_staged",
            "staging_stage",
            "staging_unstage",
            "staging_revert",
            "staging_stage_hunks",
            "staging_unstage_hunks",
        ] {
            let start = source.find(&format!("pub async fn {name}(")).unwrap();
            let mut lines = source[start..].lines();
            lines.find(|line| line.ends_with(" {")).unwrap();
            let first = lines.next().unwrap().trim();
            assert_eq!(
                first, "flush_diff_window(&app, &state, SAVE_ACTION).await?;",
                "{name}"
            );
        }
    }

    #[test]
    fn the_menu_quit_item_is_recognized() {
        assert!(is_quit_item("Quit rama"));
        assert!(!is_quit_item("Hide rama"));
    }

    #[test]
    fn moving_all_hunks_takes_the_file_to_the_other_list() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n2\n3\n4\n5\n6\n7\n8\n9\n", "one");
        fs::write(
            repo.path().join("a.txt"),
            b"one\n2\n3\n4\n5\n6\n7\n8\nnine\n",
        )
        .unwrap();
        let dir = TempDir::new().unwrap();
        let other = edit_changelists(dir.path(), |lists| lists.create("Other").map(drop)).unwrap();
        let other = other
            .lists
            .iter()
            .find(|list| list.name == "Other")
            .unwrap()
            .id
            .clone();
        read_changelists(dir.path(), &["a.txt".to_string()]).unwrap();
        let ids: Vec<String> = hunks::file_hunks(repo.path(), "a.txt")
            .unwrap()
            .into_iter()
            .map(|hunk| hunk.id)
            .collect();

        let one = move_hunks_in(repo.path(), dir.path(), "a.txt", &ids[..1], &other).unwrap();
        assert_ne!(one.assignments["a.txt"], other);

        let all = move_hunks_in(repo.path(), dir.path(), "a.txt", &ids, &other).unwrap();
        assert_eq!(all.assignments["a.txt"], other);
    }

    #[test]
    fn restoring_a_copy_waits_for_another_operation() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        let locks = RepoLocks::default();
        let dir = TempDir::new().unwrap();
        let paths = vec!["a.txt".to_string()];
        let id = snapshots::create(dir.path(), repo.path(), &paths, "test").unwrap();

        assert_waits(&locks, repo.path(), || {
            restore_locked(&locks, repo.path(), dir.path(), &id).unwrap();
        });
    }

    #[test]
    fn commit_waits_for_another_operation() {
        let repo = TempRepo::new();
        fs::write(repo.path().join("a.txt"), b"1\n").unwrap();
        let locks = RepoLocks::default();
        let dir = TempDir::new().unwrap();
        let request = commit_request("a.txt");

        assert_waits(&locks, repo.path(), || {
            commit_locked(&locks, repo.path(), dir.path(), &request).unwrap();
        });
    }

    #[test]
    fn opening_does_not_wait_forever_for_another_operation() {
        let repo = TempRepo::new();
        let locks = RepoLocks::default();
        let dir = TempDir::new().unwrap();
        let journal = dir.path().join(commit::JOURNAL_FILE);
        fs::write(&journal, b"{}").unwrap();
        let (done, finished) = mpsc::channel();

        std::thread::scope(|scope| {
            locks.run(repo.path(), || {
                scope.spawn(|| {
                    done.send(recover_on_open(&locks, repo.path(), dir.path()))
                        .unwrap();
                });
                let report = finished.recv_timeout(Duration::from_secs(5));
                let report = report.expect("opening got stuck waiting").unwrap();
                assert!(report.unwrap().pending);
            });
        });
        assert!(journal.exists());
    }

    #[test]
    fn different_repositories_do_not_wait_for_each_other() {
        let locks = RepoLocks::default();
        let (first, second) = (TempDir::new().unwrap(), TempDir::new().unwrap());

        let value = locks.run(first.path(), || locks.run(second.path(), || 7));

        assert_eq!(value, 7);
    }

    #[test]
    fn a_failure_inside_does_not_leave_the_repository_locked() {
        let locks = RepoLocks::default();
        let repo = TempDir::new().unwrap();

        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            locks.run(repo.path(), || panic!("on purpose"))
        }));

        assert!(panicked.is_err());
        assert_eq!(locks.run(repo.path(), || 7), 7);
    }

    fn entry(hash: &str) -> serde_json::Value {
        serde_json::json!({ "Entry": { "mode": "100644", "hash": hash.trim() } })
    }

    fn write_journal(dir: &Path, path: &str, restore: serde_json::Value, left: serde_json::Value) {
        let journal = serde_json::json!({ path: { "restore": restore, "left": left } });
        fs::write(dir.join(commit::JOURNAL_FILE), journal.to_string()).unwrap();
    }

    // An interrupted commit with `new.txt` excluded: its blob is stored and it is out of the stage.
    fn interrupted_repo(dir: &Path) -> TempRepo {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        fs::write(repo.path().join("new.txt"), b"staged\n").unwrap();
        repo.git(&["add", "new.txt"]);
        let blob = repo.git(&["rev-parse", ":new.txt"]);
        repo.git(&["reset", "-q", "HEAD", "--", "new.txt"]);
        write_journal(dir, "new.txt", entry(&blob), "Absent".into());
        repo
    }

    // A process that another thread is launching holds a copy of the description until its `exec`:
    // closing the file does not release the lock.
    struct HeldLock(fs::File);

    impl Drop for HeldLock {
        fn drop(&mut self) {
            let _ = self.0.unlock();
        }
    }

    fn hold_commit_lock(dir: &Path) -> HeldLock {
        let lock = fs::File::create(dir.join(commit::LOCK_FILE)).unwrap();
        lock.try_lock().unwrap();
        HeldLock(lock)
    }

    #[test]
    fn dropping_the_lock_releases_it_even_if_an_open_copy_remains() {
        let dir = TempDir::new().unwrap();
        let held = hold_commit_lock(dir.path());
        let copy = held.0.try_clone().unwrap();

        drop(held);

        let lock = fs::File::open(dir.path().join(commit::LOCK_FILE)).unwrap();
        assert!(lock.try_lock().is_ok());
        drop(copy);
    }

    #[test]
    fn opening_recovers_the_stage_of_an_interrupted_commit() {
        let dir = TempDir::new().unwrap();
        let repo = interrupted_repo(dir.path());

        let report = recover_on_open(&RepoLocks::default(), repo.path(), dir.path()).unwrap();

        assert_eq!(report.unwrap().restored, ["new.txt"]);
        assert_eq!(repo.git(&["status", "--porcelain"]), "A  new.txt\n");
        assert!(!dir.path().join(commit::JOURNAL_FILE).exists());
    }

    #[test]
    fn opening_with_nothing_to_recover_gives_no_report() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();

        let report = recover_on_open(&RepoLocks::default(), repo.path(), dir.path()).unwrap();

        assert_eq!(report, None);
    }

    #[test]
    fn opening_fails_if_the_journal_is_unreadable_and_does_not_delete_it() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(commit::JOURNAL_FILE), b"not json").unwrap();

        let result = recover_on_open(&RepoLocks::default(), repo.path(), dir.path());

        assert!(result.is_err());
        assert!(dir.path().join(commit::JOURNAL_FILE).exists());
    }

    #[test]
    fn opening_does_not_fail_if_it_cannot_restore() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        let left = repo.git(&["rev-parse", "HEAD:a.txt"]);
        fs::write(repo.path().join("other"), b"staged\n").unwrap();
        let restore = repo.git(&["hash-object", "-w", "other"]);
        fs::write(repo.path().join(".git/index.lock"), b"").unwrap();
        let dir = TempDir::new().unwrap();
        write_journal(dir.path(), "a.txt", entry(&restore), entry(&left));

        let result = recover_on_open(&RepoLocks::default(), repo.path(), dir.path());

        fs::remove_file(repo.path().join(".git/index.lock")).unwrap();
        let report = result.unwrap().unwrap();
        assert!(report.skipped.is_empty(), "{report:?}");
        assert!(report.error.unwrap().contains("a.txt"));
        assert!(dir.path().join(commit::JOURNAL_FILE).exists());
    }

    #[test]
    fn recover_does_not_block_if_the_journal_is_not_deleted() {
        let dir = TempDir::new().unwrap();
        let repo = interrupted_repo(dir.path());
        let mode = |bits| std::os::unix::fs::PermissionsExt::from_mode(bits);
        fs::set_permissions(dir.path(), mode(0o500)).unwrap();

        let result = recover_on_open(&RepoLocks::default(), repo.path(), dir.path());

        fs::set_permissions(dir.path(), mode(0o700)).unwrap();
        let report = result.unwrap().unwrap();
        assert!(report.error.is_some());
        assert_eq!(repo.git(&["status", "--porcelain"]), "A  new.txt\n");
        assert!(dir.path().join(commit::JOURNAL_FILE).exists());
    }

    #[test]
    fn rollback_and_restore_call_recover() {
        let dir = TempDir::new().unwrap();
        let repo = interrupted_repo(dir.path());
        let paths = vec!["a.txt".to_string()];
        let id = snapshots::create(dir.path(), repo.path(), &paths, "test").unwrap();
        let locks = RepoLocks::default();
        let journal = dir.path().join(commit::JOURNAL_FILE);

        restore_locked(&locks, repo.path(), dir.path(), &id).unwrap();
        assert!(!journal.exists());
        assert_eq!(repo.git(&["status", "--porcelain"]), "A  new.txt\n");

        let dir = TempDir::new().unwrap();
        let repo = interrupted_repo(dir.path());
        fs::write(repo.path().join("a.txt"), b"2\n").unwrap();

        rollback_locked(&locks, repo.path(), dir.path(), &paths, false).unwrap();

        assert!(!dir.path().join(commit::JOURNAL_FILE).exists());
        assert_eq!(repo.git(&["status", "--porcelain"]), "A  new.txt\n");
    }

    #[test]
    fn rollback_and_restore_do_not_continue_with_a_live_commit() {
        let dir = TempDir::new().unwrap();
        let repo = interrupted_repo(dir.path());
        fs::write(repo.path().join("a.txt"), b"2\n").unwrap();
        let paths = vec!["a.txt".to_string()];
        let id = snapshots::create(dir.path(), repo.path(), &paths, "test").unwrap();
        let held = hold_commit_lock(dir.path());
        let locks = RepoLocks::default();

        let rollback = rollback_locked(&locks, repo.path(), dir.path(), &paths, false);
        let restore = restore_locked(&locks, repo.path(), dir.path(), &id);

        drop(held);
        assert_eq!(rollback.unwrap_err().kind, "commit_running");
        assert_eq!(restore.unwrap_err().kind, "commit_running");
        assert_eq!(fs::read(repo.path().join("a.txt")).unwrap(), b"2\n");
        assert!(dir.path().join(commit::JOURNAL_FILE).exists());
    }

    #[test]
    fn rollback_reports_what_recover_skipped() {
        let dir = TempDir::new().unwrap();
        let repo = interrupted_repo(dir.path());
        fs::write(repo.path().join("a.txt"), b"2\n").unwrap();
        fs::write(repo.path().join("new.txt"), b"other\n").unwrap();
        repo.git(&["add", "new.txt"]);
        let paths = vec!["a.txt".to_string()];
        let locks = RepoLocks::default();

        let first = rollback_locked(&locks, repo.path(), dir.path(), &paths, false);

        assert!(first.unwrap_err().message.contains("new.txt"));
        assert_eq!(fs::read(repo.path().join("a.txt")).unwrap(), b"2\n");
        rollback_locked(&locks, repo.path(), dir.path(), &paths, false).unwrap();
        assert_eq!(fs::read(repo.path().join("a.txt")).unwrap(), b"1\n");
    }

    #[test]
    fn recovery_report_for_the_interface() {
        let repo = TempRepo::new();
        let mut info = repo_info(repo.path()).unwrap();
        info.recovery = Some(RecoveryReport {
            restored: vec!["a.txt".to_string()],
            ..Default::default()
        });

        let json = serde_json::to_value(&info).unwrap();

        assert_eq!(json["recovery"]["restored"][0], "a.txt");
        assert_eq!(json["recovery"]["skipped"].as_array().unwrap().len(), 0);
        assert_eq!(json["recovery"]["pending"], false);
    }

    #[test]
    fn commit_errors_with_their_type() {
        let kind = |error| ApiError::from(error).kind;

        assert_eq!(kind(CommitError::Unmerged), "unmerged");
        assert_eq!(kind(CommitError::PartialMerge), "partial_merge");
        assert_eq!(kind(CommitError::CommitRunning), "commit_running");
        assert_eq!(kind(CommitError::NothingToCommit), "nothing_to_commit");
        assert_eq!(kind(CommitError::Gpg("x".to_string())), "gpg_failed");
        assert_eq!(kind(CommitError::Hook("x".to_string())), "hook_failed");
        assert_eq!(
            kind(CommitError::Git(GitError::NotInstalled)),
            "git_not_found"
        );
        assert_eq!(
            kind(CommitError::OutsideRepository("x".to_string())),
            "failed"
        );
        let restore = CommitError::Restore {
            paths: vec!["a.txt".to_string()],
            detail: "detail".to_string(),
            outcome: "outcome".to_string(),
        };
        let error = ApiError::from(restore);
        assert_eq!(error.kind, "restore_failed");
        assert!(error.message.contains("a.txt"));
        assert!(error.message.contains("detail"));
        assert!(error.message.contains("outcome"));
    }

    // Git gives the root with symbolic links resolved (/var is /private/var on macOS).
    fn root_of(repo: &TempRepo) -> String {
        fs::canonicalize(repo.path()).unwrap().display().to_string()
    }

    #[test]
    fn repo_open_from_subfolder_gives_the_root() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");
        repo.commit("sub/deep/b.txt", b"1", "two");

        for folder in ["sub", "sub/deep"] {
            let info = repo_info(&repo.path().join(folder)).unwrap();
            assert_eq!(info.path, root_of(&repo), "{folder}");
        }
    }

    #[test]
    fn rollback_from_subfolder_reverts_the_root_file() {
        let repo = TempRepo::new();
        let data = TempDir::new().unwrap();
        repo.commit("a.txt", b"1", "one");
        repo.commit("sub/a.txt", b"1", "two");
        fs::write(repo.path().join("a.txt"), "2").unwrap();
        fs::write(repo.path().join("sub/a.txt"), "2").unwrap();

        let info = repo_info(&repo.path().join("sub")).unwrap();
        rollback::rollback_paths(
            Path::new(&info.path),
            data.path(),
            &["a.txt".to_string()],
            false,
        )
        .unwrap();

        assert_eq!(fs::read(repo.path().join("a.txt")).unwrap(), b"1");
        assert_eq!(fs::read(repo.path().join("sub/a.txt")).unwrap(), b"2");
    }

    #[test]
    fn repo_open_without_commits() {
        let repo = TempRepo::new();

        let info = repo_info(repo.path()).unwrap();

        assert_eq!(info.path, root_of(&repo));
        assert_eq!(info.head, None);
        assert_eq!(info.branch.as_deref(), Some("main"));
    }

    #[test]
    fn repo_open_with_commits() {
        let repo = TempRepo::new();
        let hash = repo.commit("a.txt", b"1", "one");

        let info = repo_info(repo.path()).unwrap();

        assert_eq!(info.head, Some(hash));
        assert_eq!(info.branch.as_deref(), Some("main"));
    }

    #[test]
    fn repo_open_detached_head() {
        let repo = TempRepo::new();
        let hash = repo.commit("a.txt", b"1", "one");
        repo.git(&["checkout", "-q", "--detach"]);

        let info = repo_info(repo.path()).unwrap();

        assert_eq!(info.head, Some(hash));
        assert_eq!(info.branch, None);
    }

    #[test]
    fn repo_open_without_repository() {
        let dir = TempDir::new().unwrap();
        let error = ApiError::from(repo_info(dir.path()).unwrap_err());
        assert_eq!(error.kind, "not_a_repository");
    }

    #[test]
    fn non_utf8_argument_does_not_close_the_app() {
        let args = [
            OsString::from("rama"),
            OsStr::from_bytes(b"/repo/\xff").to_os_string(),
        ];

        let first = first_argument(args.into_iter()).unwrap();

        assert_eq!(first, "/repo/\u{fffd}");
    }

    #[test]
    fn without_arguments_there_is_no_initial_repository() {
        assert_eq!(first_argument([OsString::from("rama")].into_iter()), None);
    }

    #[test]
    fn error_without_git() {
        let error = ApiError::from(GitError::NotInstalled);
        assert_eq!(error.kind, "git_not_found");
    }

    #[test]
    fn locked_index_error() {
        let error = ApiError::from(GitError::IndexLocked);
        assert_eq!(error.kind, "failed");
        assert!(error.message.contains("Another Git process"));
    }

    fn watching_state(repo: &TempRepo) -> (AppState, mpsc::Receiver<()>) {
        let state = AppState::default();
        *state.repo.lock().unwrap() = Some(RepoSession {
            path: repo.path().to_path_buf(),
        });
        let (sender, receiver) = mpsc::channel();
        resume_watching(&state, move || {
            let _ = sender.send(());
        })
        .unwrap();
        // Discards whatever is left from creating the repository and lets the stream start.
        while receiver.recv_timeout(Duration::from_millis(500)).is_ok() {}
        (state, receiver)
    }

    #[test]
    fn closing_stops_the_watcher() {
        let repo = TempRepo::new();
        let (state, receiver) = watching_state(&repo);
        assert!(state.watcher.lock().unwrap().is_some());

        stop_watching(&state);
        fs::write(repo.path().join("a.txt"), b"hello").unwrap();

        assert!(state.watcher.lock().unwrap().is_none());
        assert!(receiver.recv_timeout(Duration::from_millis(800)).is_err());
    }

    #[test]
    fn reopening_restarts_the_watcher() {
        let repo = TempRepo::new();
        let (state, _receiver) = watching_state(&repo);
        stop_watching(&state);

        let (sender, receiver) = mpsc::channel();
        resume_watching(&state, move || {
            let _ = sender.send(());
        })
        .unwrap();
        assert!(state.watcher.lock().unwrap().is_some());
        while receiver.recv_timeout(Duration::from_millis(500)).is_ok() {}
        fs::write(repo.path().join("a.txt"), b"hello").unwrap();

        assert!(receiver.recv_timeout(Duration::from_secs(2)).is_ok());
    }

    #[test]
    fn reopening_without_repository_does_not_watch() {
        let state = AppState::default();
        resume_watching(&state, || {}).unwrap();
        assert!(state.watcher.lock().unwrap().is_none());
    }

    #[test]
    fn reopening_with_an_active_watcher_does_not_duplicate_it() {
        let repo = TempRepo::new();
        let (state, receiver) = watching_state(&repo);

        resume_watching(&state, || panic!("must not create another watcher")).unwrap();
        fs::write(repo.path().join("a.txt"), b"hello").unwrap();

        assert!(receiver.recv_timeout(Duration::from_secs(2)).is_ok());
    }

    fn paths(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn changelists_read_assigns_and_saves() {
        let dir = TempDir::new().unwrap();

        let lists = read_changelists(dir.path(), &paths(&["a.txt"])).unwrap();

        assert_eq!(lists.list_for("a.txt"), lists.active);
        assert_eq!(Changelists::load(dir.path()), lists);
    }

    #[test]
    fn changelists_read_without_changes_does_not_save() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        read_changelists(dir.path(), &paths(&["a.txt"])).unwrap();
        // With the folder read-only, any attempt to save would give an error.
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o555)).unwrap();

        let result = read_changelists(dir.path(), &paths(&["a.txt"]));

        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(result.is_ok());
    }

    #[test]
    fn changelists_without_changes_or_file_creates_nothing() {
        let dir = TempDir::new().unwrap();
        let data = dir.path().join("data");

        read_changelists(&data, &[]).unwrap();

        assert!(!data.exists());
    }

    #[test]
    fn changelists_edit_saves() {
        let dir = TempDir::new().unwrap();

        let lists = edit_changelists(dir.path(), |lists| lists.create("Other").map(drop)).unwrap();

        assert_eq!(lists.lists.len(), 2);
        assert_eq!(Changelists::load(dir.path()).lists.len(), 2);
    }

    #[test]
    fn changelists_name_error_does_not_save() {
        let dir = TempDir::new().unwrap();

        let error = edit_changelists(dir.path(), |lists| lists.create("  ").map(drop)).unwrap_err();

        assert_eq!(error.kind, "failed");
        assert!(error.message.contains("empty"));
        assert!(!dir.path().join("changelists.json").exists());
    }

    #[test]
    fn changelists_repeated_name_gives_error() {
        let dir = TempDir::new().unwrap();

        let error =
            edit_changelists(dir.path(), |lists| lists.create("Changes").map(drop)).unwrap_err();

        assert!(error.message.contains("Changes"));
    }

    #[test]
    fn changelists_unreadable_file_gives_error_and_is_not_overwritten() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("changelists.json");
        let original = br#"{"lists": [{"id": "x", "name": "Mine"}], "active": "x"}"#;
        fs::write(&file, original).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();

        let read = read_changelists(dir.path(), &paths(&["a.txt"]));
        let edit = edit_changelists(dir.path(), |lists| lists.create("Other").map(drop));

        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(read.unwrap_err().kind, "failed");
        assert_eq!(edit.unwrap_err().kind, "failed");
        assert_eq!(fs::read(&file).unwrap(), original);
    }

    #[test]
    fn changelists_if_the_copy_is_not_kept_it_is_not_saved() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("changelists.json");
        fs::write(&file, b"{ broken").unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o555)).unwrap();

        let read = read_changelists(dir.path(), &paths(&["a.txt"]));
        let edit = edit_changelists(dir.path(), |lists| lists.create("Other").map(drop));

        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(read.is_err());
        assert!(edit.is_err());
        assert_eq!(fs::read(&file).unwrap(), b"{ broken");
    }

    const IDEA_WORKSPACE: &str = include_str!("../testdata/workspace.xml");

    fn project_with_workspace(xml: &[u8]) -> TempDir {
        let project = TempDir::new().unwrap();
        fs::create_dir(project.path().join(".idea")).unwrap();
        fs::write(project.path().join(".idea/workspace.xml"), xml).unwrap();
        project
    }

    #[test]
    fn importing_without_workspace_is_not_an_error() {
        let project = TempDir::new().unwrap();
        let data = TempDir::new().unwrap();

        let lists = import_changelists(project.path(), data.path()).unwrap();

        assert_eq!(lists, Changelists::default());
    }

    #[test]
    fn importing_saves_the_lists() {
        let project = project_with_workspace(IDEA_WORKSPACE.as_bytes());
        let data = TempDir::new().unwrap();

        let lists = import_changelists(project.path(), data.path()).unwrap();

        assert_eq!(lists.lists.len(), 3);
        assert_eq!(Changelists::load(data.path()), lists);
    }

    #[test]
    fn importing_with_broken_xml_gives_error_and_does_not_save() {
        let project = project_with_workspace(b"<project><component name=\"ChangeListManager\">");
        let data = TempDir::new().unwrap();

        let error = import_changelists(project.path(), data.path()).unwrap_err();

        assert_eq!(error.kind, "failed");
        assert!(error.message.contains("XML"));
        assert!(!data.path().join("changelists.json").exists());
    }

    #[test]
    fn importing_with_unreadable_file_gives_error_and_does_not_save() {
        use std::os::unix::fs::PermissionsExt;
        let project = project_with_workspace(IDEA_WORKSPACE.as_bytes());
        let file = project.path().join(".idea/workspace.xml");
        fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();
        let data = TempDir::new().unwrap();

        let result = import_changelists(project.path(), data.path());

        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(result.unwrap_err().kind, "failed");
        assert!(!data.path().join("changelists.json").exists());
    }

    #[test]
    fn importing_with_unreadable_own_data_does_not_overwrite_it() {
        use std::os::unix::fs::PermissionsExt;
        let project = project_with_workspace(IDEA_WORKSPACE.as_bytes());
        let data = TempDir::new().unwrap();
        let file = data.path().join("changelists.json");
        let original = br#"{"lists": [{"id": "x", "name": "Mine"}], "active": "x"}"#;
        fs::write(&file, original).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();

        let result = import_changelists(project.path(), data.path());

        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(result.unwrap_err().kind, "failed");
        assert_eq!(fs::read(&file).unwrap(), original);
    }

    fn nine(repo: &TempRepo) {
        repo.git(&["config", "core.autocrlf", "input"]);
        repo.commit("a.txt", b"1\n2\n3\n4\n5\n6\n7\n8\n9\n", "one");
        fs::write(
            repo.path().join("a.txt"),
            b"1\ntwo\n3\n4\n5\n6\n7\neight\n9\n",
        )
        .unwrap();
    }

    fn whole_hunk(repo: &Path, index: usize) -> LineSelection {
        let hunk = &hunks::file_hunks(repo, "a.txt").unwrap()[index];
        LineSelection {
            hunk: hunk.id.clone(),
            lines: None,
        }
    }

    #[test]
    fn undoing_lines_waits_for_another_operation() {
        let repo = TempRepo::new();
        nine(&repo);
        let locks = RepoLocks::default();
        let dir = TempDir::new().unwrap();
        let selection = [whole_hunk(repo.path(), 0)];

        assert_waits(&locks, repo.path(), || {
            lines_rollback_locked(&locks, repo.path(), dir.path(), "a.txt", &selection).unwrap();
        });
    }

    #[test]
    fn rolling_back_lines_rejects_a_conflicted_file() {
        let repo = TempRepo::new();
        repo.conflict("a.txt");
        let before = fs::read(repo.path().join("a.txt")).unwrap();
        let selection = [whole_hunk(repo.path(), 0)];
        let locks = RepoLocks::default();
        let dir = TempDir::new().unwrap();

        let error = lines_rollback_locked(&locks, repo.path(), dir.path(), "a.txt", &selection)
            .unwrap_err();

        assert_eq!(error.kind, "unmerged");
        assert_eq!(fs::read(repo.path().join("a.txt")).unwrap(), before);
        assert!(snapshots::list(dir.path()).is_empty());
    }

    #[test]
    fn undoing_lines_calls_recover_and_does_not_continue_with_a_live_commit() {
        let dir = TempDir::new().unwrap();
        let repo = interrupted_repo(dir.path());
        fs::write(repo.path().join("a.txt"), b"2\n").unwrap();
        let selection = [whole_hunk(repo.path(), 0)];
        let locks = RepoLocks::default();
        let journal = dir.path().join(commit::JOURNAL_FILE);

        let held = hold_commit_lock(dir.path());
        let alive = lines_rollback_locked(&locks, repo.path(), dir.path(), "a.txt", &selection);
        drop(held);

        assert_eq!(alive.unwrap_err().kind, "commit_running");
        assert_eq!(fs::read(repo.path().join("a.txt")).unwrap(), b"2\n");
        assert!(journal.exists());

        lines_rollback_locked(&locks, repo.path(), dir.path(), "a.txt", &selection).unwrap();

        assert!(!journal.exists());
        assert_eq!(repo.git(&["status", "--porcelain"]), "A  new.txt\n");
        assert_eq!(fs::read(repo.path().join("a.txt")).unwrap(), b"1\n");
    }

    #[test]
    fn commit_with_selection_carries_only_what_was_chosen() {
        let repo = TempRepo::new();
        nine(&repo);
        let locks = RepoLocks::default();
        let dir = TempDir::new().unwrap();
        let request = CommitRequest {
            message: "partial".to_string(),
            ..Default::default()
        };
        let selections = [FileSelection {
            path: "a.txt".to_string(),
            selection: vec![whole_hunk(repo.path(), 1)],
        }];

        commit_selected_locked(&locks, repo.path(), dir.path(), request, &selections).unwrap();

        assert_eq!(
            repo.git(&["show", "HEAD:a.txt"]),
            "1\n2\n3\n4\n5\n6\n7\neight\n9\n"
        );
        assert_eq!(
            fs::read(repo.path().join("a.txt")).unwrap(),
            b"1\ntwo\n3\n4\n5\n6\n7\neight\n9\n"
        );
        assert_eq!(repo.git(&["status", "--porcelain"]), " M a.txt\n");
    }

    #[test]
    fn commit_with_selection_keeps_non_utf8_bytes() {
        let repo = TempRepo::new();
        repo.git(&["config", "core.autocrlf", "input"]);
        repo.commit("a.txt", b"a\n\xff\xfe\nc\n", "one");
        fs::write(repo.path().join("a.txt"), b"a\n\xff\xff\nc\nz\n").unwrap();
        let hunks = hunks::file_hunks(repo.path(), "a.txt").unwrap();
        let selection = vec![LineSelection {
            hunk: hunks[1].id.clone(),
            lines: None,
        }];
        let selections = [FileSelection {
            path: "a.txt".to_string(),
            selection,
        }];
        let request = CommitRequest {
            message: "partial".to_string(),
            ..Default::default()
        };

        let dir = TempDir::new().unwrap();
        commit_selected_locked(
            &RepoLocks::default(),
            repo.path(),
            dir.path(),
            request,
            &selections,
        )
        .unwrap();

        let blob = std::process::Command::new("git")
            .current_dir(repo.path())
            .args(["cat-file", "blob", "HEAD:a.txt"])
            .output()
            .unwrap()
            .stdout;
        assert_eq!(blob, b"a\n\xff\xfe\nc\nz\n");
    }

    #[test]
    fn commit_with_a_lost_hunk_does_not_commit() {
        let repo = TempRepo::new();
        nine(&repo);
        let selection = vec![whole_hunk(repo.path(), 1)];
        fs::write(
            repo.path().join("a.txt"),
            b"1\ntwo\n3\n4\n5\n6\n7\neighty\n9\n",
        )
        .unwrap();
        let before = repo.git(&["rev-parse", "HEAD"]);
        let request = CommitRequest {
            message: "partial".to_string(),
            ..Default::default()
        };
        let selections = [FileSelection {
            path: "a.txt".to_string(),
            selection,
        }];

        let dir = TempDir::new().unwrap();
        let result = commit_selected_locked(
            &RepoLocks::default(),
            repo.path(),
            dir.path(),
            request,
            &selections,
        );

        assert_eq!(result.unwrap_err().kind, "selection_lost");
        assert_eq!(repo.git(&["rev-parse", "HEAD"]), before);
    }

    #[test]
    fn commit_with_selection_rejects_a_repeated_path() {
        let repo = TempRepo::new();
        nine(&repo);
        let selection = vec![whole_hunk(repo.path(), 0)];
        let twice = [
            FileSelection {
                path: "a.txt".to_string(),
                selection: selection.clone(),
            },
            FileSelection {
                path: "a.txt".to_string(),
                selection: selection.clone(),
            },
        ];
        let own = CommitRequest {
            partial: vec![commit::PartialFile {
                path: "a.txt".to_string(),
                content: b"x\n".to_vec(),
            }],
            message: "partial".to_string(),
            ..Default::default()
        };
        let before = repo.git(&["rev-parse", "HEAD"]);
        let dir = TempDir::new().unwrap();
        let locks = RepoLocks::default();
        let request = || CommitRequest {
            message: "partial".to_string(),
            ..Default::default()
        };

        let repeated = commit_selected_locked(&locks, repo.path(), dir.path(), request(), &twice);
        let clash = commit_selected_locked(&locks, repo.path(), dir.path(), own, &twice[..1]);

        assert_eq!(repeated.unwrap_err().kind, "failed");
        assert_eq!(clash.unwrap_err().kind, "failed");
        assert_eq!(repo.git(&["rev-parse", "HEAD"]), before);
    }

    #[test]
    fn commit_with_selection_waits_for_another_operation() {
        let repo = TempRepo::new();
        nine(&repo);
        let locks = RepoLocks::default();
        let dir = TempDir::new().unwrap();
        let selections = [FileSelection {
            path: "a.txt".to_string(),
            selection: vec![whole_hunk(repo.path(), 0)],
        }];

        assert_waits(&locks, repo.path(), || {
            let request = CommitRequest {
                message: "partial".to_string(),
                ..Default::default()
            };
            commit_selected_locked(&locks, repo.path(), dir.path(), request, &selections).unwrap();
        });
    }

    #[test]
    fn moving_hunks_to_another_list() {
        let repo = TempRepo::new();
        nine(&repo);
        let dir = TempDir::new().unwrap();
        let other = edit_changelists(dir.path(), |lists| lists.create("Other").map(drop))
            .unwrap()
            .lists[1]
            .id
            .clone();
        let ids: Vec<String> = hunks::file_hunks(repo.path(), "a.txt")
            .unwrap()
            .into_iter()
            .map(|hunk| hunk.id)
            .collect();

        let lists = move_hunks_in(repo.path(), dir.path(), "a.txt", &ids[1..], &other).unwrap();

        assert_eq!(lists.hunks_in("a.txt", &other), ids[1..]);
        assert_eq!(lists.hunks_in("a.txt", &lists.active), ids[..1]);
        assert_eq!(Changelists::load(dir.path()), lists);

        // The moved hunk changes: it goes back to the file's list.
        fs::write(
            repo.path().join("a.txt"),
            b"1\ntwo\n3\n4\n5\n6\n7\neighty\n9\n",
        )
        .unwrap();
        let again = move_hunks_in(repo.path(), dir.path(), "a.txt", &ids[..1], &other).unwrap();

        assert_eq!(again.hunks_in("a.txt", &other), ids[..1]);
        assert_eq!(again.hunks_in("a.txt", &again.active).len(), 1);
    }

    #[test]
    fn moving_a_hunk_that_no_longer_exists_fails_without_saving() {
        let repo = TempRepo::new();
        nine(&repo);
        let dir = TempDir::new().unwrap();
        let other = edit_changelists(dir.path(), |lists| lists.create("Other").map(drop))
            .unwrap()
            .lists[1]
            .id
            .clone();
        let before = Changelists::load(dir.path());

        let result = move_hunks_in(
            repo.path(),
            dir.path(),
            "a.txt",
            &["ghost".to_string()],
            &other,
        );

        assert_eq!(result.unwrap_err().kind, "failed");
        assert_eq!(Changelists::load(dir.path()), before);
    }

    #[test]
    fn hunk_errors_reach_the_interface() {
        let outside: ApiError = HunkError::OutsideRepository("../x".to_string()).into();
        let git: ApiError = HunkError::Git(GitError::NotInstalled).into();

        assert_eq!(outside.kind, "failed");
        assert!(outside.message.contains("../x"));
        assert_eq!(git.kind, "git_not_found");
    }

    fn selected(path: &str, selection: Vec<LineSelection>) -> [FileSelection; 1] {
        [FileSelection {
            path: path.to_string(),
            selection,
        }]
    }

    fn partial_request() -> CommitRequest {
        CommitRequest {
            message: "partial".to_string(),
            ..Default::default()
        }
    }

    fn blob_at_head(repo: &TempRepo, path: &str) -> Vec<u8> {
        std::process::Command::new("git")
            .current_dir(repo.path())
            .args(["cat-file", "blob", &format!("HEAD:{path}")])
            .output()
            .unwrap()
            .stdout
    }

    #[test]
    fn partial_commit_of_a_crlf_file_with_autocrlf_input_changes_only_its_line() {
        let repo = TempRepo::new();
        repo.git(&["config", "core.autocrlf", "input"]);
        repo.commit("a.txt", b"1\n2\n3\n4\n5\n6\n7\n8\n", "one");
        fs::write(
            repo.path().join("a.txt"),
            b"1\r\n2\r\nthree\r\n4\r\n5\r\n6\r\n7\r\neight\r\n",
        )
        .unwrap();
        let hunks = hunks::file_hunks(repo.path(), "a.txt").unwrap();
        assert_eq!(hunks.len(), 2);
        let selection = vec![LineSelection {
            hunk: hunks[0].id.clone(),
            lines: None,
        }];
        let dir = TempDir::new().unwrap();

        commit_selected_locked(
            &RepoLocks::default(),
            repo.path(),
            dir.path(),
            partial_request(),
            &selected("a.txt", selection),
        )
        .unwrap();

        assert_eq!(
            blob_at_head(&repo, "a.txt"),
            b"1\n2\nthree\n4\n5\n6\n7\n8\n"
        );
        assert_eq!(
            repo.git(&["diff", "--numstat", "HEAD~1", "HEAD"]),
            "1\t1\ta.txt\n"
        );
        // What the user wrote stays on disk, and what was not chosen is still pending.
        assert_eq!(
            fs::read(repo.path().join("a.txt")).unwrap(),
            b"1\r\n2\r\nthree\r\n4\r\n5\r\n6\r\n7\r\neight\r\n"
        );
        assert_eq!(hunks::file_hunks(repo.path(), "a.txt").unwrap().len(), 1);
    }

    #[test]
    fn partial_commit_with_filter_stores_the_filtered_form_without_corrupting_what_was_not_chosen()
    {
        let repo = TempRepo::new();
        repo.git(&["config", "core.autocrlf", "input"]);
        let rot = "tr 'A-Za-z' 'N-ZA-Mn-za-m'";
        repo.git(&["config", "filter.rot.clean", rot]);
        repo.git(&["config", "filter.rot.smudge", rot]);
        fs::write(repo.path().join(".gitattributes"), "*.txt filter=rot\n").unwrap();
        repo.commit("a.txt", b"one\ntwo\nthree\nfour\nfive\nsix\nseven\n", "one");
        fs::write(
            repo.path().join("a.txt"),
            b"one\nTWO\nthree\nfour\nfive\nSIX\nseven\n",
        )
        .unwrap();
        let hunks = hunks::file_hunks(repo.path(), "a.txt").unwrap();
        let selection = vec![LineSelection {
            hunk: hunks[1].id.clone(),
            lines: None,
        }];
        let dir = TempDir::new().unwrap();

        commit_selected_locked(
            &RepoLocks::default(),
            repo.path(),
            dir.path(),
            partial_request(),
            &selected("a.txt", selection),
        )
        .unwrap();

        // `bar\ngjb\n...` is "one\ntwo\n..." through the filter; only the chosen line changes.
        assert_eq!(
            blob_at_head(&repo, "a.txt"),
            b"bar\ngjb\nguerr\nsbhe\nsvir\nFVK\nfrira\n"
        );
        assert_eq!(
            repo.git(&["diff", "--numstat", "HEAD~1", "HEAD"]),
            "1\t1\ta.txt\n"
        );
    }

    #[test]
    fn commit_with_selection_with_nothing_chosen_does_not_commit() {
        let repo = TempRepo::new();
        nine(&repo);
        let hunks = hunks::file_hunks(repo.path(), "a.txt").unwrap();
        let before = repo.git(&["rev-parse", "HEAD"]);
        let dir = TempDir::new().unwrap();

        for selection in [
            vec![],
            vec![LineSelection {
                hunk: hunks[0].id.clone(),
                lines: Some(vec![]),
            }],
        ] {
            let result = commit_selected_locked(
                &RepoLocks::default(),
                repo.path(),
                dir.path(),
                partial_request(),
                &selected("a.txt", selection),
            );
            assert_eq!(result.unwrap_err().kind, "failed");
        }
        assert_eq!(repo.git(&["rev-parse", "HEAD"]), before);
    }

    #[test]
    fn commit_with_selection_calls_recover_and_does_not_continue_with_a_live_commit() {
        let dir = TempDir::new().unwrap();
        let repo = interrupted_repo(dir.path());
        fs::write(repo.path().join("a.txt"), b"2\n").unwrap();
        let selection = vec![whole_hunk(repo.path(), 0)];
        let journal = dir.path().join(commit::JOURNAL_FILE);
        let locks = RepoLocks::default();

        let held = hold_commit_lock(dir.path());
        let alive = commit_selected_locked(
            &locks,
            repo.path(),
            dir.path(),
            partial_request(),
            &selected("a.txt", selection.clone()),
        );
        drop(held);

        assert_eq!(alive.unwrap_err().kind, "commit_running");
        assert!(journal.exists());

        commit_selected_locked(
            &locks,
            repo.path(),
            dir.path(),
            partial_request(),
            &selected("a.txt", selection),
        )
        .unwrap();

        assert!(!journal.exists());
    }

    #[test]
    fn commit_with_selection_of_a_conflicted_file_says_unmerged_and_not_lost() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n2\n3\n", "base");
        repo.git(&["checkout", "-q", "-b", "other"]);
        repo.commit("a.txt", b"1\nOTHER\n3\n", "other");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit("a.txt", b"1\nMAIN\n3\n", "main");
        let merge = std::process::Command::new("git")
            .current_dir(repo.path())
            .args(["merge", "other"])
            .output()
            .unwrap();
        assert!(!merge.status.success());
        let old = vec![LineSelection {
            hunk: "from-before-the-conflict".to_string(),
            lines: None,
        }];
        let dir = TempDir::new().unwrap();

        let result = commit_selected_locked(
            &RepoLocks::default(),
            repo.path(),
            dir.path(),
            partial_request(),
            &selected("a.txt", old),
        );

        assert_eq!(result.unwrap_err().kind, "unmerged");
    }

    #[test]
    fn commit_with_selection_during_a_merge_says_it_goes_whole() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n2\n3\n", "one");
        repo.git(&["checkout", "-q", "-b", "other"]);
        repo.commit("a.txt", b"1\nOTHER\n3\n", "other");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit("b.txt", b"1\n", "main");
        repo.git(&["merge", "-q", "--no-commit", "--no-ff", "other"]);
        let chosen = vec![whole_hunk(repo.path(), 0)];
        let dir = TempDir::new().unwrap();

        let result = commit_selected_locked(
            &RepoLocks::default(),
            repo.path(),
            dir.path(),
            partial_request(),
            &selected("a.txt", chosen),
        );

        assert_eq!(result.unwrap_err().kind, "partial_merge");
        repo.git(&["rev-parse", "-q", "--verify", "MERGE_HEAD"]);
    }

    #[test]
    fn a_lost_selection_arrives_with_its_path_and_its_hunks() {
        let repo = TempRepo::new();
        nine(&repo);
        let old = vec![LineSelection {
            hunk: "no-longer-exists".to_string(),
            lines: None,
        }];
        let dir = TempDir::new().unwrap();

        let error = commit_selected_locked(
            &RepoLocks::default(),
            repo.path(),
            dir.path(),
            partial_request(),
            &selected("a.txt", old),
        )
        .unwrap_err();

        assert_eq!(error.kind, "selection_lost");
        let json = serde_json::to_value(&error).unwrap();
        assert_eq!(json["kind"], "selection_lost");
        assert_eq!(json["details"]["path"], "a.txt");
        assert_eq!(
            json["details"]["hunks"],
            serde_json::json!(["no-longer-exists"])
        );
        // The other errors carry no `details`.
        let plain = serde_json::to_value(failed("x".to_string())).unwrap();
        assert!(plain.get("details").is_none());
    }

    // Staging area mode.

    fn stage_all_of_nine(repo: &TempRepo) {
        nine(repo);
        repo.git(&["add", "a.txt"]);
    }

    #[test]
    fn stage_waits_for_another_operation() {
        let repo = TempRepo::new();
        nine(&repo);
        let locks = RepoLocks::default();
        let dir = TempDir::new().unwrap();
        let paths = vec!["a.txt".to_string()];

        assert_waits(&locks, repo.path(), || {
            stage_locked(&locks, repo.path(), dir.path(), &paths).unwrap();
        });

        assert_eq!(repo.git(&["status", "--porcelain"]), "M  a.txt\n");
    }

    #[test]
    fn unstage_waits_for_another_operation() {
        let repo = TempRepo::new();
        stage_all_of_nine(&repo);
        let locks = RepoLocks::default();
        let dir = TempDir::new().unwrap();
        let paths = vec!["a.txt".to_string()];

        assert_waits(&locks, repo.path(), || {
            unstage_locked(&locks, repo.path(), dir.path(), &paths).unwrap();
        });

        assert_eq!(repo.git(&["status", "--porcelain"]), " M a.txt\n");
    }

    #[test]
    fn stage_of_hunks_waits_for_another_operation() {
        let repo = TempRepo::new();
        nine(&repo);
        let locks = RepoLocks::default();
        let dir = TempDir::new().unwrap();
        let selection = [whole_hunk(repo.path(), 0)];

        assert_waits(&locks, repo.path(), || {
            stage_hunks_locked(&locks, repo.path(), dir.path(), "a.txt", &selection).unwrap();
        });

        assert_eq!(repo.git(&["status", "--porcelain"]), "MM a.txt\n");
    }

    #[test]
    fn unstage_of_hunks_waits_for_another_operation() {
        let repo = TempRepo::new();
        stage_all_of_nine(&repo);
        let locks = RepoLocks::default();
        let dir = TempDir::new().unwrap();
        let selection = [whole_hunk(repo.path(), 0)];

        assert_waits(&locks, repo.path(), || {
            unstage_hunks_locked(&locks, repo.path(), dir.path(), "a.txt", &selection).unwrap();
        });

        assert_eq!(repo.git(&["status", "--porcelain"]), "MM a.txt\n");
    }

    #[test]
    fn touching_the_stage_calls_recover_and_does_not_continue_with_a_live_commit() {
        let dir = TempDir::new().unwrap();
        let repo = interrupted_repo(dir.path());
        fs::write(repo.path().join("a.txt"), b"2\n").unwrap();
        let locks = RepoLocks::default();
        let journal = dir.path().join(commit::JOURNAL_FILE);
        let paths = vec!["a.txt".to_string()];

        let held = hold_commit_lock(dir.path());
        let alive = stage_locked(&locks, repo.path(), dir.path(), &paths);
        drop(held);

        assert_eq!(alive.unwrap_err().kind, "commit_running");
        assert_eq!(
            repo.git(&["status", "--porcelain"]),
            " M a.txt\n?? new.txt\n"
        );
        assert!(journal.exists());

        stage_locked(&locks, repo.path(), dir.path(), &paths).unwrap();

        assert!(!journal.exists());
        assert_eq!(
            repo.git(&["status", "--porcelain"]),
            "M  a.txt\nA  new.txt\n"
        );
    }

    #[test]
    fn commit_of_the_stage_waits_and_saves_its_message() {
        let repo = TempRepo::new();
        stage_all_of_nine(&repo);
        let locks = RepoLocks::default();
        let dir = TempDir::new().unwrap();
        let options = CommitOptions::default();

        assert_waits(&locks, repo.path(), || {
            commit_staged_locked(&locks, repo.path(), dir.path(), &options, "staged").unwrap();
        });

        assert_eq!(repo.git(&["status", "--porcelain"]), "");
        assert_eq!(messages::list(dir.path()), vec!["staged".to_string()]);
    }

    #[test]
    fn the_signature_comes_from_the_settings() {
        let settings = Settings {
            sign_commits: true,
            signing_key: Some("ABC123".to_string()),
            ..Default::default()
        };

        let options = with_signature(CommitOptions::default(), &settings);
        assert!(options.sign);
        assert_eq!(options.signing_key.as_deref(), Some("ABC123"));

        let options = with_signature(options, &Settings::default());
        assert!(!options.sign);
        assert_eq!(options.signing_key, None);
    }

    #[test]
    fn warnings_also_look_at_what_was_chosen_by_lines() {
        let repo = TempRepo::new();
        repo.git(&["config", "core.autocrlf", "false"]);
        repo.commit("base.txt", b"base\n", "base");
        fs::write(repo.path().join("a.txt"), b"one\r\n").unwrap();
        let selections = [FileSelection {
            path: "a.txt".to_string(),
            selection: Vec::new(),
        }];

        let found = warnings_for(
            repo.path(),
            CommitRequest::default(),
            &selections,
            &Settings::default(),
        );

        assert_eq!(found, vec![CommitWarning::Crlf(vec!["a.txt".to_string()])]);
    }

    #[test]
    fn saving_settings_returns_what_was_saved() {
        let base = TempDir::new().unwrap();
        let sent = Settings {
            git_path: Some("  /opt/git  ".to_string()),
            ..Default::default()
        };

        let saved = store_settings(base.path(), &sent).unwrap();

        assert_eq!(saved.git_path.as_deref(), Some("/opt/git"));
        assert_eq!(settings::load(base.path()), saved);
    }

    #[test]
    fn an_error_when_saving_settings_reaches_the_interface() {
        let base = TempDir::new().unwrap();
        let file = base.path().join("file");
        fs::write(&file, b"").unwrap();

        let error = store_settings(&file, &Settings::default()).unwrap_err();

        assert_eq!(error.kind, "failed");
        assert!(error.message.contains("settings"), "{}", error.message);
    }

    #[test]
    fn reverting_waits_for_another_operation() {
        let repo = TempRepo::new();
        nine(&repo);
        let locks = RepoLocks::default();
        let dir = TempDir::new().unwrap();
        let paths = vec!["a.txt".to_string()];

        assert_waits(&locks, repo.path(), || {
            revert_locked(&locks, repo.path(), dir.path(), &paths).unwrap();
        });

        assert_eq!(repo.git(&["status", "--porcelain"]), "");
    }

    #[test]
    fn reverting_calls_recover_and_does_not_continue_with_a_live_commit() {
        let dir = TempDir::new().unwrap();
        let repo = interrupted_repo(dir.path());
        fs::write(repo.path().join("a.txt"), b"2\n").unwrap();
        let locks = RepoLocks::default();
        let paths = vec!["a.txt".to_string()];

        let held = hold_commit_lock(dir.path());
        let alive = revert_locked(&locks, repo.path(), dir.path(), &paths);
        drop(held);

        assert_eq!(alive.unwrap_err().kind, "commit_running");
        assert_eq!(fs::read(repo.path().join("a.txt")).unwrap(), b"2\n");
    }

    fn git_menu_items() -> Vec<(&'static str, &'static str)> {
        menu_items(&GIT_MENU)
    }

    #[test]
    fn the_git_menu_follows_the_order_of_git_main_menu() {
        let texts: Vec<&str> = git_menu_items().iter().map(|item| item.1).collect();

        assert_eq!(
            texts,
            [
                "Commit…",
                "Push…",
                "Update Project…",
                "Pull…",
                "Fetch",
                "Merge…",
                "Abort Merge",
                "Rebase…",
                "Abort Rebase",
                "Continue Rebase",
                "Skip Commit",
                "Resolve Conflicts…",
                "Branches…",
                "New Branch…",
                "New Tag…",
                "Reset HEAD…",
                "Manage Remotes…",
                "Clone…",
                "Create Git Repository…",
                "Abort Revert",
                "Abort Cherry-Pick",
                "Open .git/info/exclude",
            ]
        );
    }

    // `Git.MainMenu.MergeActions` and `RebaseActions` are only visible in their state, and so are the two below
    // (`Git.Revert.Abort`, `Git.CherryPick.Abort`): the native menu does not hide, it disables them.
    #[test]
    fn the_ongoing_actions_start_disabled() {
        let ongoing: Vec<&str> = ONGOING_MENU.to_vec();
        for (id, _) in git_menu_items() {
            let abort_or_resume = id.ends_with(".Abort")
                || id.starts_with("Git.Rebase.")
                || id == "Git.ResolveConflicts";
            assert_eq!(starts_disabled(id), abort_or_resume, "{id}");
            assert_eq!(ongoing.contains(&id), abort_or_resume, "{id}");
        }
        assert!(starts_disabled(REBASE_GROUP));
    }

    // `GitMerge.update` and `GitRebase.update`: they only apply with the repository in a normal state. The
    // native menu does not hide: they start enabled and the interface disables them with a merge, a rebase or a
    // detached HEAD.
    #[test]
    fn merge_and_rebase_start_enabled_and_the_state_governs_them() {
        for id in ["Git.Merge", "Git.Rebase"] {
            assert!(!starts_disabled(id), "{id}");
            assert!(driven_by_state(id), "{id}");
            assert!(is_git_menu_item(id), "{id}");
        }
        assert!(!driven_by_state("Git.Fetch"));
        assert!(driven_by_state("Git.Rebase.Skip"));
    }

    #[test]
    fn only_the_git_menu_ids_come_out_as_git_action() {
        assert!(is_git_menu_item("Git.Fetch"));
        assert!(is_git_menu_item("Git.Init"));
        assert!(is_git_menu_item("Git.Rebase.Skip"));
        assert!(!is_git_menu_item(REBASE_GROUP));
        assert!(!is_git_menu_item(QUIT_MENU_ID));
        assert!(!is_git_menu_item(SETTINGS_MENU_ID));
    }

    #[test]
    fn title_merge_revisions_for_path() {
        assert_eq!(
            merge_title(Path::new("/repo"), "src/a.txt"),
            "Merge Revisions for /repo/src/a.txt"
        );
    }

    #[test]
    fn asking_for_another_file_brings_the_window_forward_without_changing_it() {
        let mut shown = None;

        assert!(claim_merge(&mut shown, false, "a.txt".to_string()));
        assert_eq!(shown.as_deref(), Some("a.txt"));

        assert!(!claim_merge(&mut shown, true, "b.txt".to_string()));
        assert_eq!(shown.as_deref(), Some("a.txt"));
    }

    #[test]
    fn closing_the_merge_window_is_asked_of_its_page() {
        assert_eq!(close_event_of(MERGE_WINDOW), MERGE_CLOSE_EVENT);
        assert_eq!(close_event_of(DIFF_WINDOW), DIFF_CLOSE_EVENT);
        assert!(!owns_watcher(MERGE_WINDOW));
    }

    // `LoadTextUtil.ConvertResult.majorLineSeparator`: without line breaks, the system one.
    #[test]
    fn the_line_separator_is_the_one_that_appears_most() {
        assert_eq!(line_separator(b"a\r\nb\r\nc\n"), "\r\n");
        assert_eq!(line_separator(b"a\nb\nc\r\n"), "\n");
        assert_eq!(line_separator(b"a\rb\rc\n"), "\r");
        assert_eq!(line_separator(b"a\r\nb\n"), "\n");
        assert_eq!(line_separator(b"no break"), "\n");
        assert_eq!(line_separator(b""), "\n");
    }

    #[test]
    fn the_working_file_gives_its_hash_and_its_separator() {
        let repo = TempRepo::new();
        fs::write(repo.path().join("a.txt"), b"1\r\n2\r\n").unwrap();

        let file = working_file(repo.path(), "a.txt").unwrap();

        assert_eq!(file.hash, worktree_diff::content_hash(b"1\r\n2\r\n"));
        assert_eq!(file.separator, "\r\n");
        // Without stages in the index there is no longer a conflict to resolve.
        assert!(!file.conflicted);
        let missing = working_file(repo.path(), "missing.txt").unwrap();
        assert_eq!(missing.hash, "");
        assert_eq!(missing.separator, "\n");
        assert!(working_file(repo.path(), "../outside.txt").is_err());
    }

    #[test]
    fn the_working_file_says_whether_it_is_still_in_conflict() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"base\n", "base");
        let git = |args: &[&str]| {
            let status = Command::new("git")
                .args(args)
                .current_dir(repo.path())
                .output()
                .unwrap();
            assert!(status.status.success() || args[0] == "merge", "{args:?}");
        };
        git(&["checkout", "-q", "-b", "side"]);
        repo.commit("a.txt", b"side\n", "side");
        git(&["checkout", "-q", "-"]);
        repo.commit("a.txt", b"main\n", "main");
        git(&["merge", "-q", "side"]);

        assert!(working_file(repo.path(), "a.txt").unwrap().conflicted);
        git(&["add", "a.txt"]);
        assert!(!working_file(repo.path(), "a.txt").unwrap().conflicted);
    }

    // `repo_open` asks the merge window before anything else, and a merge that stays stops it.
    #[test]
    fn opening_another_repository_with_a_half_done_merge_asks() {
        let wait = |receive: &Receiver<LeaveReply>| {
            wait_for_merge(
                receive,
                Duration::from_secs(1),
                Duration::from_secs(1),
                OPEN_ACTION,
            )
        };
        let (send, receive) = mpsc::channel();
        send.send(LeaveReply::Answer(true)).unwrap();
        assert!(wait(&receive).is_ok());

        send.send(LeaveReply::Received).unwrap();
        send.send(LeaveReply::Answer(false)).unwrap();
        let kept = wait(&receive).unwrap_err();
        assert!(kept.message.contains("half-done merge"), "{}", kept.message);
        assert!(kept.message.contains(OPEN_ACTION));

        // With the acknowledgement, the user's answer has its own deadline.
        send.send(LeaveReply::Received).unwrap();
        let silent = wait_for_merge(
            &receive,
            Duration::from_secs(1),
            Duration::from_millis(10),
            OPEN_ACTION,
        )
        .unwrap_err();
        assert!(
            silent.message.contains("did not answer"),
            "{}",
            silent.message
        );

        let source = include_str!("commands.rs");
        let start = source.find("pub async fn repo_open(").unwrap();
        let mut lines = source[start..].lines();
        lines.find(|line| line.ends_with(" {")).unwrap();
        lines.next();
        assert_eq!(
            lines.next().unwrap().trim(),
            "leave_merge_window(&app, &state, OPEN_ACTION).await?;"
        );
    }

    // A page that does not receive the notice does not leave `repo_open` or Quit hanging: it refuses in time.
    #[test]
    fn without_an_acknowledgement_leaving_fails_in_time() {
        let (_send, receive) = mpsc::channel::<LeaveReply>();
        let start = Instant::now();

        let refused = wait_for_merge(
            &receive,
            Duration::from_millis(20),
            Duration::from_secs(600),
            QUIT_ACTION,
        )
        .unwrap_err();

        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(
            refused.message.contains("not responding"),
            "{}",
            refused.message
        );
        assert!(refused.message.contains(QUIT_ACTION));
    }

    // Without the page mounted there is no half-done merge to lose: nothing is asked.
    #[test]
    fn only_an_already_loaded_merge_window_is_asked() {
        assert!(!must_ask_merge(false, false));
        assert!(!must_ask_merge(true, false));
        assert!(must_ask_merge(true, true));
    }

    #[test]
    fn the_merge_result_arrives_with_its_path() {
        let done = MergeDone {
            path: "a.txt".to_string(),
            result: MergeResult::Cancelled,
        };

        assert_eq!(
            serde_json::to_value(&done).unwrap(),
            serde_json::json!({ "path": "a.txt", "result": "cancelled" })
        );
        let result: MergeResult = serde_json::from_str("\"theirs\"").unwrap();
        assert_eq!(result, MergeResult::Theirs);
    }
}
