use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
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
use crate::block_history::{self, SelectionRequest};
use crate::branch_info::BranchSync;
use crate::changelists::{ChangelistError, Changelists};
use crate::commit::{
    self, CommitError, CommitOptions, CommitRequest, CommitWarning, RecoveryReport,
};
use crate::compare::{self, DiffRequest};
use crate::conflicts;
use crate::console::{self, Console, ConsoleLine};
use crate::diff::{self, ChangedFile, FileSides};
use crate::editor::{self, Prompt};
use crate::git::{self, GitError};
use crate::history;
use crate::hosting::commands::HostingState;
use crate::hunks::{self, Hunk, HunkError, LineSelection};
use crate::idea_import;
use crate::log::commands::SharedLog;
use crate::log::signatures::SignatureLoads;
use crate::messages;
use crate::open_projects::{self, OpenProjects};
use crate::operation::{self, RepoState};
use crate::preserve::{self, Pending};
use crate::recent;
use crate::remotes;
use crate::rollback::{self, RollbackError};
use crate::settings::{self, Settings};
use crate::shelf;
use crate::snapshots::{self, Snapshot, SnapshotId};
use crate::staging;
use crate::status::{self, FileChange};
use crate::storage;
use crate::task::{Runner, TaskId};
use crate::viewer::{self, ViewerTarget};
use crate::watch::{self, RepoChange, WatchedPaths, Watcher};
use crate::windows::{self, InitialTargets, ReopenFocus, WindowKind};
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
            GitError::LoginField { server, message } => ApiError {
                kind: if server {
                    "login_server"
                } else {
                    "login_token"
                },
                message,
                details: None,
            },
            // CollaborationToolsBundle 8.
            GitError::MissingToken => ApiError {
                kind: "missing_token",
                message: "Missing access token".to_string(),
                details: None,
            },
            GitError::Authentication(message) => ApiError {
                kind: "authentication",
                message,
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
const DIFF_REQUEST_EVENT: &str = "diff-request";
const DIFF_CLOSE_EVENT: &str = "diff-close";
const DIFF_FLUSH_EVENT: &str = "diff-flush";
const DIFF_FLUSH_WAIT: Duration = Duration::from_secs(10);
const QUIT_WAIT: Duration = Duration::from_secs(60);
const LEAVE_REFUSED_EVENT: &str = "leave-refused";
const SETTINGS_CHANGED_EVENT: &str = "settings-changed";
const OPEN_SETTINGS_EVENT: &str = "open-settings";
// The project window became the menu's owner: its pages send their menu state again.
const MENU_OWNER_GAINED_EVENT: &str = "menu-owner-gained";
const OPEN_PROJECT_CHOOSER_EVENT: &str = "open-project-chooser";
pub const QUIT_MENU_ID: &str = "ramify-quit";
pub const SETTINGS_MENU_ID: &str = "ramify-settings";
pub const OPEN_MENU_ID: &str = "ramify-open";
// IntelliJ's File > Open…, with no accelerator: in its macOS keymap ⌘O is Go to Class.
const OPEN_MENU_TEXT: &str = "Open…";
const MAC_FILE_MENU: &str = "File";
pub const RECENT_MENU_ID: &str = "ramify-recent";
// `group.$LRU.text`.
const RECENT_MENU_TEXT: &str = "Recent Projects";
// A row's id is this prefix and the repository's path: the click needs nothing else.
const RECENT_ITEM_PREFIX: &str = "ramify-recent:";
const OPEN_RECENT_EVENT: &str = "open-recent";
const OPEN_PATH_EVENT: &str = "open-path";
const OPEN_ELSEWHERE: &str = "The project is already open in another window";
const SETTINGS_MENU_POSITION: usize = 2;
const GIT_ACTION_EVENT: &str = "git-action";
const WINDOW_MENU: &str = "Window";

enum GitMenuEntry {
    // Id of the action (the same one the shortcut uses in the interface) and text. No native accelerator: the
    // shortcuts are handled by the interface, and the order between menu and keyboard cannot be checked.
    Item(&'static str, &'static str),
    // A submenu with its id and text, like `Git.MainMenu.RebaseActions`.
    Group(&'static str, &'static str, &'static [GitMenuEntry]),
    Separator,
}

use GitMenuEntry::{Group, Item, Separator};

const REBASE_GROUP: &str = "Git.MainMenu.RebaseActions";
const LOCAL_CHANGES_GROUP: &str = "Git.MainMenu.LocalChanges";
const PATCH_GROUP: &str = "Patch.MainMenu";
// `ChangesView.CreatePatch`, an executor action: not there with the staging area.
const CREATE_PATCH: &str = "ChangesView.CreatePatch";
const CREATE_PATCH_TEXT: &str = "Create Patch from Local Changes...";
// `ChangesView.Shelve`, the same kind of action.
const SHELVE_CHANGES: &str = "ChangesView.Shelve";
const SHELVE_CHANGES_TEXT: &str = "Shelve Changes...";
// Ramify's own (IntelliJ has no such action): the Shelf tab's `Import from .idea`, reachable while
// the shelf is still empty and the tab is not there.
const IMPORT_IDEA_SHELF: &str = "Shelf.ImportFromIdea";
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

// `GitHub.MainMenu` (github-core-config.xml:111-120), a popup group added before
// `Git.Configure.Remotes`. Of its actions Ramify has only `Github.View.Pull.Request` (:97-99,
// `GithubViewPullRequestsAction`), which is visible only while the Pull Requests window is
// available; 241 hides a popup group whose actions are all hidden. The native menu cannot hide
// an entry: the group is taken out and put back, like Create Patch, and it starts out.
const GITHUB_GROUP: &str = "GitHub.MainMenu";
const VIEW_PULL_REQUESTS: &str = "Github.View.Pull.Request";

fn starts_hidden(id: &str) -> bool {
    id == GITHUB_GROUP
}

// The interface names `Github.View.Pull.Request` among the entries that apply while the window
// is available.
fn github_menu_shown(ids: &[String]) -> bool {
    ids.iter().any(|id| id == VIEW_PULL_REQUESTS)
}

// `Git.MainMenu.FileActions` (`Git > Current File`), the group of `Git.FileActions`: its title and
// items follow the selection of the focused window, which sets them with `git_menu_current_file`.
const FILE_GROUP: &str = "Git.MainMenu.FileActions";
const FILE_MENU: [&str; 8] = [
    "CheckinFiles",
    "Git.Add",
    "Annotate",
    "Compare.SameVersion",
    "Compare.Selected",
    "Git.CompareWithBranch",
    "Vcs.ShowTabbedFileHistory",
    "Vcs.ShowHistoryForBlock",
];

// Without a selection every file action is off (`GitFileActionGroup` titles it `Current File`).
fn starts_disabled(id: &str) -> bool {
    ONGOING_MENU.contains(&id) || FILE_MENU.contains(&id)
}

// What only applies with the repository in a normal state (`GitMerge.update`, `GitRebase.update`): it starts
// enabled and the interface disables it with an operation in progress or a detached HEAD. `Git.Fetch` also
// starts enabled and is disabled while any fetch of the app runs (`GitFetch.update`).
const NORMAL_MENU: [&str; 3] = ["Git.Merge", "Git.Rebase", "Git.Fetch"];

// The file actions are not: `git_menu_current_file` sets them.
fn driven_by_state(id: &str) -> bool {
    ONGOING_MENU.contains(&id) || NORMAL_MENU.contains(&id)
}

// The entries with an action, groups included and in order.
fn menu_items(entries: &[GitMenuEntry]) -> Vec<(&'static str, &'static str)> {
    entries
        .iter()
        .flat_map(|entry| match entry {
            Item(id, text) => vec![(*id, *text)],
            Group(_, _, children) => menu_items(children),
            Separator => Vec::new(),
        })
        .collect()
}

// `Git.MainMenu` of plugin.xml, in its order. What the app does not have yet comes in with its phase.
const GIT_MENU: [GitMenuEntry; 31] = [
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
        "Rebase",
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
    // `Vcs.Show.Log`: `Show {0} Log` with the name of the VCS.
    Item("Vcs.Show.Log", "Show Git Log"),
    // `Patch.MainMenu`.
    Group(
        PATCH_GROUP,
        "Patch",
        &[
            Item(CREATE_PATCH, CREATE_PATCH_TEXT),
            Item("ChangesView.ApplyPatch", "Apply Patch..."),
            Item(
                "ChangesView.ApplyPatchFromClipboard",
                "Apply Patch from Clipboard...",
            ),
        ],
    ),
    // `Git.MainMenu.LocalChanges`, with Ramify's own `Import Shelf from .idea…` after Show Shelf.
    // `Vcs.UmlDiff` is an empty group in the Community Edition.
    Group(
        LOCAL_CHANGES_GROUP,
        "Uncommitted Changes",
        &[
            Item(SHELVE_CHANGES, SHELVE_CHANGES_TEXT),
            Item("Vcs.Show.Shelf", "Show Shelf"),
            Item(IMPORT_IDEA_SHELF, "Import Shelf from .idea…"),
            Item("Git.Stash", "Stash Changes…"),
            Item("Git.Unstash", "Unstash Changes…"),
            // `action.ChangesView.Revert.text`.
            Item("ChangesView.Revert", "Rollback..."),
        ],
    ),
    // `Git.MainMenu.FileActions` = `Git.FileActions`, with the texts of a selected file; the focused
    // window gives the ones of its selection.
    Group(
        FILE_GROUP,
        "Current File",
        &[
            Item("CheckinFiles", "Commit File…"),
            Item("Git.Add", "Add"),
            Separator,
            Item("Annotate", "Annotate with Git Blame"),
            Item(
                "Compare.SameVersion",
                "Compare with the Same Repository Version",
            ),
            Item("Compare.Selected", "Compare with Revision..."),
            Item("Git.CompareWithBranch", "Compare with Branch…"),
            Item("Vcs.ShowTabbedFileHistory", "Show History"),
            Item("Vcs.ShowHistoryForBlock", "Show History for Selection…"),
        ],
    ),
    Separator,
    // `GitHub.MainMenu`, with the text `group.GitHub.MainMenu.text` (GithubBundle:5) and
    // `pull.request.view.list` (GithubBundle:167).
    Group(
        GITHUB_GROUP,
        "GitHub",
        &[Item(VIEW_PULL_REQUESTS, "View Pull Requests")],
    ),
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
const MERGE_OPEN_ACTION: &str = "the merge window was not opened";
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
// Each main window owns its session's watcher, and closing it is closing its project window.
pub fn owns_watcher(label: &str) -> bool {
    windows::kind_of(label) == Some(WindowKind::Main)
}

// Closing the diff window saves what was written; closing the merge one is its Cancel, with its questions.
fn close_event_of(label: &str) -> &'static str {
    if windows::kind_of(label) == Some(WindowKind::Merge) {
        MERGE_CLOSE_EVENT
    } else {
        DIFF_CLOSE_EVENT
    }
}

// The viewer and the selection history have nothing to save: they close at once.
pub fn request_close(app: &AppHandle, label: &str) {
    let owner = windows::owner_of(label).unwrap_or_else(|| MAIN_WINDOW.to_string());
    match windows::kind_of(label) {
        Some(WindowKind::Viewer) => {
            viewer::close(app, &owner);
            return;
        }
        Some(WindowKind::SelectionHistory) => {
            block_history::close(app, &owner);
            return;
        }
        _ => {}
    }
    let _ = app.emit_to(label, close_event_of(label), ());
}

// When the last project window is closed (it is only hidden) no one is looking: the watcher of
// its session, `owner`'s, is released.
pub fn stop_watching(state: &AppState, owner: &str) {
    if let Ok(session) = session_of(state, owner) {
        replace_watcher(&session, None);
    }
}

// When the window is shown again, watching the session's repository resumes.
pub fn resume_watching(
    state: &AppState,
    owner: &str,
    on_change: impl Fn(RepoChange) + Send + 'static,
) -> Result<(), GitError> {
    let Ok(session) = session_of(state, owner) else {
        return Ok(());
    };
    if session.watcher.lock().unwrap().is_some() {
        return Ok(());
    }
    let Ok(path) = session_repo(&session) else {
        return Ok(());
    };
    let watcher = watch::start(WatchedPaths::of(&path)?, on_change)?;
    replace_watcher(&session, Some(watcher));
    Ok(())
}

// The old watcher is dropped outside the lock, which must not stay held while it finishes.
fn replace_watcher(session: &WindowSession, watcher: Option<Watcher>) {
    let old = std::mem::replace(&mut *session.watcher.lock().unwrap(), watcher);
    drop(old);
}

// After a command: the watcher reports for itself whether the stash was touched.
pub(crate) fn repo_changed(app: &AppHandle, owner: &str) {
    emit_repo_changed(app, owner, RepoChange::default());
}

// The shelf generation each store had at the last `repo-changed`.
static SHELF_SEEN: Mutex<BTreeMap<PathBuf, u64>> = Mutex::new(BTreeMap::new());

// Whether the store of `data` was written since the last time this was asked (`SHELF_TOPIC`).
fn shelf_moved(seen: &mut BTreeMap<PathBuf, u64>, data: &Path) -> bool {
    let now = shelf::generation(data);
    seen.insert(data.to_path_buf(), now).unwrap_or(0) != now
}

// Every signal says whether the open repository's shelf changed since the previous one: the
// commands that write the store emit `repo-changed` once they are done, and the Shelf tab reads
// the lists only then.
// It goes to the windows of `owner`, whose session holds the repository.
fn emit_repo_changed(app: &AppHandle, owner: &str, change: RepoChange) {
    let state = app.state::<AppState>();
    let data = session_of(&state, owner)
        .and_then(|session| session_repo(&session))
        .and_then(|repo| data_dir_of(app, &repo));
    let shelf = data.is_ok_and(|data| {
        let mut seen = SHELF_SEEN.lock().unwrap_or_else(PoisonError::into_inner);
        shelf_moved(&mut seen, &data)
    });
    let change = RepoChange {
        shelf: change.shelf || shelf,
        ..change
    };
    emit_to_owner(app, owner, REPO_CHANGED_EVENT, change);
}

fn notifier(app: AppHandle, owner: String) -> impl Fn(RepoChange) + Send + 'static {
    move |change| emit_repo_changed(&app, &owner, change)
}

// If watching cannot be resumed there is no other error path to the interface: the
// event is emitted anyway and the interface reads the state again.
// `owner` is the project window being shown.
pub fn resume_and_notify(app: &AppHandle, owner: &str) {
    let _ = resume_watching(
        &app.state::<AppState>(),
        owner,
        notifier(app.clone(), owner.to_string()),
    );
    // Nobody watched while it was hidden: the stash may have changed too.
    notifier(app.clone(), owner.to_string())(RepoChange {
        stash: true,
        shelf: false,
    });
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

// What belongs to one project window: its repository, its log, its watcher and what its
// auxiliary windows show. Its auxiliary windows use their owner's.
#[derive(Default)]
pub struct WindowSession {
    pub repo: Mutex<Option<RepoSession>>,
    // Shared with the tasks that open views: they finish after the command returns.
    pub log: SharedLog,
    // Dropping it stops watching the previous repository.
    pub watcher: Mutex<Option<Watcher>>,
    // What the diff window must show. It does not go in the URL: a reused window is not
    // loaded again.
    pub diff_request: Mutex<Option<DiffRequest>>,
    // Whoever waits for the diff window to save what was written.
    pub diff_flush: Mutex<VecDeque<Sender<bool>>>,
    // What the merge window shows, while it is open.
    pub merge_session: Mutex<Option<MergeSession>>,
    // Whoever waits for the merge window to say whether it can be closed.
    pub merge_leave: Mutex<VecDeque<Sender<LeaveReply>>>,
    // The merge page already listens to its notices. Before that it has loaded nothing, and there is no merge to lose.
    pub merge_ready: Mutex<bool>,
    // What `viewer_open` left for the viewer page, until it takes it.
    pub viewer_pending: Mutex<Vec<ViewerTarget>>,
    // What `selection_history_open` left for the page, and the stream it runs, with the project
    // window that started it, which is the only one that may cancel it.
    pub selection_pending: Mutex<Option<SelectionRequest>>,
    pub selection_running: Mutex<Option<(String, TaskId)>>,
    // The root a `repo_open` of this window is opening, until it ends however it ends.
    pub opening: Mutex<Option<PathBuf>>,
    // Whoever waits for the main page to save its commit message draft (`commit_draft_saved`).
    pub draft_saved: Mutex<VecDeque<Sender<()>>>,
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
    // Each project window's session, by the label of its main window.
    pub sessions: Mutex<HashMap<String, Arc<WindowSession>>>,
    pub branch_sync: BranchSync,
    // Whether the app has the focus, for the on-the-fly patch detection.
    pub activation: Mutex<Activation>,
    // GitHub's client and where its tokens are. Shared with the after-fetch hook, which loads the
    // protected branch rules.
    pub hosting: Arc<HostingState>,
    // The `GPG Signature` column's loads and their app-wide cache.
    pub signatures: SignatureLoads,
    // The label of the main window that last had the focus: where a path handed to the app goes.
    pub last_focused_main: Mutex<String>,
    // What each main window's page opens on start.
    pub initial_targets: Mutex<InitialTargets>,
    // The last number given to a main window (`windows::next_main_label`).
    pub main_labels: AtomicU32,
    // Held while a window's claim on a root is checked and made: a `repo_open` starting, or a
    // new main window getting its label and being built.
    pub open_claims: Mutex<()>,
    // The main windows in the order they were opened, `main` first: the order of the open list.
    // A window keeps its place when This Window replaces its repository, and leaves it on close.
    pub open_order: Mutex<Vec<String>>,
    // Held while the open list is computed and written, so two writes do not cross.
    pub open_list_write: Mutex<()>,
    // The main windows whose close is under way: a second click on the red button waits for it.
    pub closing_windows: Mutex<HashSet<String>>,
    // What the start reopens from the open list (`reopen_first`, `open_projects_reopen`).
    pub reopen: Mutex<ReopenStage>,
    // The reopened windows, until the last focused one comes to the front.
    pub reopen_focus: Mutex<Option<ReopenFocus>>,
    // The main windows closed for good: their labels never get a session again.
    pub closed_owners: Mutex<HashSet<String>>,
    // Set while a quit runs: the open list keeps what was open at quit.
    pub quitting: AtomicBool,
}

// Reopening happens once per process, at the first `main` page's start.
#[derive(Debug, Default)]
pub enum ReopenStage {
    #[default]
    NotAsked,
    Planned(open_projects::Reopen),
    Done,
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
            sessions: Mutex::default(),
            branch_sync: BranchSync::default(),
            activation: Mutex::default(),
            hosting: Arc::new(HostingState::default()),
            signatures: SignatureLoads::default(),
            last_focused_main: Mutex::new(MAIN_WINDOW.to_string()),
            initial_targets: Mutex::default(),
            main_labels: AtomicU32::new(0),
            open_claims: Mutex::new(()),
            open_order: Mutex::new(vec![MAIN_WINDOW.to_string()]),
            open_list_write: Mutex::new(()),
            closing_windows: Mutex::default(),
            reopen: Mutex::default(),
            reopen_focus: Mutex::default(),
            closed_owners: Mutex::default(),
            quitting: AtomicBool::new(false),
        }
    }
}

// The session of the project window `label` belongs to, created on first use. A label of no
// project window has none.
pub(crate) fn session_of(state: &AppState, label: &str) -> Result<Arc<WindowSession>, ApiError> {
    let owner = windows::owner_of(label).ok_or_else(|| {
        failed(format!(
            "The window \"{label}\" belongs to no project window"
        ))
    })?;
    let mut sessions = state.sessions.lock().unwrap();
    // A closed window's label is never used again: a late call from its pages gets no session,
    // which would outlive it.
    if state.closed_owners.lock().unwrap().contains(&owner) {
        return Err(failed(format!("The window \"{label}\" was closed")));
    }
    Ok(sessions.entry(owner).or_default().clone())
}

// The window of `kind` in the project window `label` belongs to.
fn owned_label(kind: WindowKind, label: &str) -> String {
    windows::label_for(kind, &windows::owner_of(label).unwrap_or_default())
}

// The first project window's session, for the tests that have no window to ask.
#[cfg(test)]
fn main_session(state: &AppState) -> Arc<WindowSession> {
    session_of(state, MAIN_WINDOW).expect("the main window has an owner")
}

// The owner of the project window `window` belongs to: whose windows an event of its command goes to.
pub(crate) fn owner_label(window: &WebviewWindow) -> String {
    windows::owner_of(window.label()).unwrap_or_else(|| MAIN_WINDOW.to_string())
}

// The labels of `owner`'s five windows that exist: where its events go.
fn emit_targets(owner: &str, exists: impl Fn(&str) -> bool) -> Vec<String> {
    windows::owner_group(owner)
        .into_iter()
        .filter(|label| exists(label))
        .collect()
}

// Sends `event` to each window of the project window `owner` that exists: its main window, diff,
// merge, viewer and selection history. `emit_to` also reaches listeners without a target, so the
// page listens with its own label (`listenOwn`). A window that does not exist is skipped. The
// first error is returned, after every window was tried.
pub fn try_emit_to_owner<S: Serialize + Clone>(
    app: &AppHandle,
    owner: &str,
    event: &str,
    payload: S,
) -> tauri::Result<()> {
    let mut first = Ok(());
    for label in emit_targets(owner, |label| app.get_webview_window(label).is_some()) {
        if let Err(error) = app.emit_to(label.as_str(), event, payload.clone()) {
            first = first.and(Err(error));
        }
    }
    first
}

// Like `try_emit_to_owner`, for events nobody can answer for: the error is only logged.
pub fn emit_to_owner<S: Serialize + Clone>(app: &AppHandle, owner: &str, event: &str, payload: S) {
    if let Err(error) = try_emit_to_owner(app, owner, event, payload) {
        eprintln!("Could not send the event {event} to {owner}: {error}");
    }
}

fn session_repo(session: &WindowSession) -> Result<PathBuf, ApiError> {
    session
        .repo
        .lock()
        .unwrap()
        .as_ref()
        .map(|repo| repo.path.clone())
        .ok_or_else(|| failed("No repository is open".to_string()))
}

// The repository open in the project window `label` belongs to.
pub(crate) fn repo_of(state: &AppState, label: &str) -> Result<PathBuf, ApiError> {
    let session = session_of(state, label)?;
    session_repo(&session)
}

// What each project window has open or is about to have, by the label of its main window: its
// repository, the root a `repo_open` of it is opening, and, while it has no repository yet, the
// initial target it was built for. The sessions' lock is let go before any other is taken.
fn claimed_roots(state: &AppState) -> Vec<(String, PathBuf)> {
    let sessions: Vec<(String, Arc<WindowSession>)> = state
        .sessions
        .lock()
        .unwrap()
        .iter()
        .map(|(label, session)| (label.clone(), session.clone()))
        .collect();
    let mut claims = Vec::new();
    let mut with_repo = HashSet::new();
    for (label, session) in sessions {
        if let Some(repo) = session.repo.lock().unwrap().as_ref() {
            with_repo.insert(label.clone());
            claims.push((label.clone(), repo.path.clone()));
        }
        if let Some(opening) = session.opening.lock().unwrap().clone() {
            claims.push((label, opening));
        }
    }
    let targets = state.initial_targets.lock().unwrap();
    for (label, target) in targets.stored() {
        if !with_repo.contains(label) {
            claims.push((label.to_string(), PathBuf::from(target)));
        }
    }
    claims
}

// The project window other than `except` whose repository is `root`, compared as canonical
// paths: a symbolic link or a trailing slash is the same repository. A path that cannot be
// resolved matches nothing.
fn window_with_root(open: &[(String, PathBuf)], root: &Path, except: &str) -> Option<String> {
    let root = fs::canonicalize(root).ok()?;
    open.iter()
        .filter(|(label, _)| label != except)
        .find(|(_, path)| fs::canonicalize(path).is_ok_and(|path| path == root))
        .map(|(label, _)| label.clone())
}

// The main window other than `except` that has `root` open or is opening it, among the windows
// that `exists`. Whoever acts on the answer holds `open_claims`.
fn claimed_by(
    state: &AppState,
    root: &Path,
    except: &str,
    exists: impl Fn(&str) -> bool,
) -> Option<String> {
    let claims: Vec<(String, PathBuf)> = claimed_roots(state)
        .into_iter()
        .filter(|(label, _)| exists(label))
        .collect();
    window_with_root(&claims, root, except)
}

// A `repo_open` that is opening a root. Dropping it, however the open ends, takes the root out
// of the session's `opening`, and the initial target the window was built for: from then on its
// repository, or nothing, is what it has open.
pub(crate) struct OpeningClaim<'a> {
    state: &'a AppState,
    session: Arc<WindowSession>,
    owner: String,
}

impl Drop for OpeningClaim<'_> {
    fn drop(&mut self) {
        *self.session.opening.lock().unwrap() = None;
        self.state
            .initial_targets
            .lock()
            .unwrap()
            .remove(&self.owner);
    }
}

// `owner` starts opening `root`, unless another window has it open or is opening it: then that
// window's label. Checking and claiming go under one lock, so two opens of one root cannot both
// go ahead.
fn claim_opening<'a>(
    state: &'a AppState,
    owner: &str,
    root: &Path,
    exists: impl Fn(&str) -> bool,
) -> Result<OpeningClaim<'a>, String> {
    let _claims = state
        .open_claims
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if let Some(label) = claimed_by(state, root, owner, exists) {
        return Err(label);
    }
    let session = session_of(state, owner).map_err(|_| owner.to_string())?;
    *session.opening.lock().unwrap() = Some(root.to_path_buf());
    Ok(OpeningClaim {
        state,
        session,
        owner: owner.to_string(),
    })
}

#[derive(Debug, PartialEq, Eq)]
enum NewWindowPlan {
    // Another window already has the root open, or is opening it.
    Existing(String),
    // The label to build, with `root` already stored as its initial target.
    Build(String),
}

// Called with `open_claims` held, until the window is built.
fn plan_new_window(
    state: &AppState,
    root: &str,
    taken: &HashSet<String>,
    exists: impl Fn(&str) -> bool,
) -> NewWindowPlan {
    if let Some(label) = claimed_by(state, Path::new(root), "", exists) {
        return NewWindowPlan::Existing(label);
    }
    let label = windows::next_main_label(taken, &state.main_labels);
    state
        .initial_targets
        .lock()
        .unwrap()
        .store(&label, root.to_string());
    NewWindowPlan::Build(label)
}

// The open flow's first step: the repository root, as `repo_info` gives it to `repo_open`. A
// subfolder gives its root; a folder outside any repository fails as `repo_open` does.
fn resolve_project(target: &Path) -> Result<String, ApiError> {
    Ok(repo_root(target)?.display().to_string())
}

#[tauri::command]
pub async fn project_resolve(target: String) -> Result<String, ApiError> {
    blocking_api(move || resolve_project(Path::new(&target))).await
}

// `ProjectUtil.focusProjectWindow`: if another project window has `root` open, or is opening it,
// it comes to the front (shown, out of the Dock, focused) and nothing opens twice.
#[tauri::command]
pub async fn project_focus_open(
    root: String,
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<bool, ApiError> {
    let own = owner_label(&window);
    let found = {
        let _claims = state
            .open_claims
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        claimed_by(&state, Path::new(&root), &own, |label| {
            app.get_webview_window(label).is_some()
        })
    };
    Ok(found.is_some_and(|label| focus_main(&app, &label)))
}

// Brings the main window `label` to the front; `false` if it does not exist. A window hidden as
// the last one stopped watching: it watches again and reads its state, as the Dock icon does.
fn focus_main(app: &AppHandle, label: &str) -> bool {
    let Some(window) = app.get_webview_window(label) else {
        return false;
    };
    if let Err(error) = reveal(&window) {
        eprintln!("Could not bring {label} to the front: {error}");
    }
    resume_and_notify(app, label);
    true
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewWindow {
    pub label: String,
    // `false`: another window already had the root open or was opening it, and was focused.
    pub created: bool,
}

// New Window: the next `main-<n>`, built like the configured first window, whose page opens
// `root` on start (`initial_target`). `root` comes from the page and is resolved to its
// repository root here, before any claim is checked or stored. `open_claims` covers checking that no window has the root,
// choosing the label and building the window, so two calls at once never get the same label nor
// both build a window for one root. Async: building a window from a synchronous command, which
// runs on the main thread, can block.
#[tauri::command]
pub async fn project_new_window(
    root: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<NewWindow, ApiError> {
    let root = blocking_api(move || resolve_project(Path::new(&root))).await?;
    new_main_window(&app, &state, &root)
}

fn new_main_window(app: &AppHandle, state: &AppState, root: &str) -> Result<NewWindow, ApiError> {
    let built = build_main_window(app, state, root)?;
    if built.created {
        // It counts as open from now on, with the target it was built for.
        write_open_projects(app);
    }
    Ok(built)
}

fn build_main_window(app: &AppHandle, state: &AppState, root: &str) -> Result<NewWindow, ApiError> {
    let _claims = state
        .open_claims
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let mut config = app
        .config()
        .app
        .windows
        .first()
        .cloned()
        .ok_or_else(|| failed("The app has no window configuration".to_string()))?;
    let taken: HashSet<String> = app
        .webview_windows()
        .into_keys()
        .filter(|label| windows::kind_of(label) == Some(WindowKind::Main))
        .collect();
    let exists = |label: &str| app.get_webview_window(label).is_some();
    let label = match plan_new_window(state, root, &taken, exists) {
        NewWindowPlan::Existing(label) => {
            focus_main(app, &label);
            return Ok(NewWindow {
                label,
                created: false,
            });
        }
        NewWindowPlan::Build(label) => label,
    };
    config.label = label.clone();
    config.url = WebviewUrl::App("index.html".into());
    if let Err(error) =
        WebviewWindowBuilder::from_config(app, &config).and_then(|builder| builder.build())
    {
        state.initial_targets.lock().unwrap().remove(&label);
        return Err(failed(format!("Could not open a new window: {error}")));
    }
    // Still under `open_claims`: the order is the order the windows were built in.
    state.open_order.lock().unwrap().push(label.clone());
    Ok(NewWindow {
        label,
        created: true,
    })
}

// A project window, the main one or an auxiliary one, gained the focus. A focus change between
// project windows rewrites the open list and hands the menu to the new owner, which is asked to
// send its menu state again. The event says which of its windows got the focus: the main page
// says Current File itself unless its viewer does.
pub fn main_focused(app: &AppHandle, label: &str) {
    if let Some(owner) = menu_follows(&app.state::<AppState>(), label) {
        write_open_projects(app);
        refresh_recent_menu(app);
        // What the menu shows is the previous owner's until this one says its own state.
        emit_to_owner(app, &owner, MENU_OWNER_GAINED_EVENT, label.to_string());
    }
}

// The project window whose state the menu bar shows: the one that had the focus last, an
// auxiliary window of it included.
pub fn menu_owner(state: &AppState) -> String {
    state.last_focused_main.lock().unwrap().clone()
}

// `label` gained the focus: its project window owns the menu now. The new owner if that changed
// it; a window that belongs to no project window, or the owner it already was, gives `None`.
fn menu_follows(state: &AppState, label: &str) -> Option<String> {
    let owner = windows::owner_of(label)?;
    let mut last = state.last_focused_main.lock().unwrap();
    if *last == owner {
        return None;
    }
    *last = owner.clone();
    Some(owner)
}

// Whether the menu takes what `label` says about its state: only the owner's windows do.
fn menu_accepts(state: &AppState, label: &str) -> bool {
    windows::owner_of(label).is_some_and(|owner| owner == menu_owner(state))
}

// The repository of the menu's owner: the one the Recent Projects submenu leaves out.
fn menu_repo(state: &AppState) -> Option<String> {
    let session = state
        .sessions
        .lock()
        .unwrap()
        .get(&menu_owner(state))
        .cloned()?;
    session_repo(&session)
        .ok()
        .map(|path| path.to_string_lossy().into_owned())
}

// The last focused main window's label, or another main window's if it has gone. Without any,
// the first window's, which may not exist yet.
fn last_main_label(app: &AppHandle) -> String {
    let last = menu_owner(&app.state::<AppState>());
    if app.get_webview_window(&last).is_some() {
        return last;
    }
    let mut mains: Vec<String> = app
        .webview_windows()
        .into_keys()
        .filter(|label| windows::kind_of(label) == Some(WindowKind::Main))
        .collect();
    mains.sort_by_key(|label| (label != MAIN_WINDOW, label.len(), label.clone()));
    mains.into_iter().next().unwrap_or(last)
}

// macOS's open-documents event (`open -a Ramify <folder>`, a folder dropped on the Dock icon):
// the last focused main window runs the open flow with it. A page that has not yet asked for its
// initial target takes it as that target instead, which covers the event that comes with the
// app's launch. Only the first path is used: each would ask its own question.
pub fn open_paths(app: &AppHandle, paths: Vec<PathBuf>) {
    let Some(path) = paths.into_iter().next() else {
        return;
    };
    let path = path.display().to_string();
    let label = last_main_label(app);
    {
        let state = app.state::<AppState>();
        let mut targets = state.initial_targets.lock().unwrap();
        if !targets.asked(&label) {
            targets.store(&label, path);
            return;
        }
    }
    if focus_main(app, &label) {
        emit_to_owner(app, &label, OPEN_PATH_EVENT, path);
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

// Shown, out of the Dock if it was minimized, and focused.
pub fn reveal(window: &WebviewWindow) -> tauri::Result<()> {
    window.show()?;
    if window.is_minimized()? {
        window.unminimize()?;
    }
    window.set_focus()
}

// A page shows its window once it has started. After the windows a start reopened have all shown
// themselves, the one focused last in the previous session comes to the front.
#[tauri::command]
pub fn show_window(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<(), ApiError> {
    reveal(&window).map_err(|error| failed(error.to_string()))?;
    let due = state
        .reopen_focus
        .lock()
        .unwrap()
        .as_mut()
        .and_then(|focus| focus.shown(window.label()));
    if let Some(label) = due {
        focus_main(&app, &label);
    }
    Ok(())
}

// Reopening on start (§5.8), for the first `main` page's initial target when the command line
// gives none: with `Reopen projects on startup`, the open list's first project, which `main`
// opens. The others wait for `open_projects_reopen`. The paths that no longer exist are taken out
// of the list now. Only once per process: a later call gives nothing.
pub(crate) fn reopen_first(app: &AppHandle) -> Option<String> {
    let state = app.state::<AppState>();
    let mut stage = state.reopen.lock().unwrap();
    if !matches!(*stage, ReopenStage::NotAsked) {
        return None;
    }
    *stage = ReopenStage::Done;
    if !current_settings(app).is_ok_and(|settings| settings.reopen_projects) {
        return None;
    }
    let base = app.path().app_data_dir().ok()?;
    let open = match open_projects::load(&base) {
        Ok(open) => open,
        Err(error) => {
            eprintln!("Could not read the open projects: {error}");
            return None;
        }
    };
    let plan = open_projects::plan(&open, |path| Path::new(path).exists());
    if !plan.missing.is_empty() {
        if let Err(error) = open_projects::save(&base, &plan.kept()) {
            eprintln!("Could not save the open projects: {error}");
        }
    }
    let first = plan.first.clone();
    if first.is_some() || !plan.missing.is_empty() {
        // From here on `main` showing itself counts.
        *state.reopen_focus.lock().unwrap() = Some(ReopenFocus::default());
        *stage = ReopenStage::Planned(plan);
    }
    first
}

#[derive(Debug, Default, Serialize)]
pub struct Reopened {
    // The paths that no longer exist, which the initial window reports.
    pub missing: Vec<String>,
}

// The rest of reopening, asked by the first `main` page once it has its initial target: each
// other project of the open list in a new main window, in order, and at the end the one focused
// last comes to the front. Nothing for any other window, or once it has run.
#[tauri::command]
pub async fn open_projects_reopen(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Reopened, ApiError> {
    if window.label() != MAIN_WINDOW {
        return Ok(Reopened::default());
    }
    let taken = {
        let mut stage = state.reopen.lock().unwrap();
        match std::mem::take(&mut *stage) {
            ReopenStage::Planned(plan) => {
                *stage = ReopenStage::Done;
                Some(plan)
            }
            other => {
                *stage = other;
                None
            }
        }
    };
    let Some(plan) = taken else {
        return Ok(Reopened::default());
    };
    let mut labels: HashMap<String, String> = HashMap::new();
    let mut waiting = HashSet::new();
    if let Some(first) = &plan.first {
        labels.insert(first.clone(), MAIN_WINDOW.to_string());
        waiting.insert(MAIN_WINDOW.to_string());
    }
    for root in &plan.rest {
        match new_main_window(&app, &state, root) {
            Ok(NewWindow { label, created }) => {
                if created {
                    waiting.insert(label.clone());
                }
                labels.entry(root.clone()).or_insert(label);
            }
            Err(error) => eprintln!("Could not reopen {root}: {}", error.message),
        }
    }
    let focus = plan
        .last_focused
        .as_ref()
        .and_then(|path| labels.get(path).cloned());
    let due = state
        .reopen_focus
        .lock()
        .unwrap()
        .as_mut()
        .and_then(|reopen| reopen.planned(waiting, focus));
    if let Some(label) = due {
        focus_main(&app, &label);
    }
    Ok(Reopened {
        missing: plan.missing,
    })
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
// Only the diff window of `window`'s project window is asked.
pub(crate) async fn flush_diff_window(
    app: &AppHandle,
    state: &State<'_, AppState>,
    window: &WebviewWindow,
    action: &'static str,
) -> Result<(), ApiError> {
    flush_diff_of(app, state, window.label(), action).await
}

// Queues the sender of one flush in the session of `label`'s owner, and says which diff window
// to ask. `None` when that window does not exist: there is nothing to save.
fn flush_request(
    state: &AppState,
    label: &str,
    exists: impl FnOnce(&str) -> bool,
) -> Result<Option<(String, Receiver<bool>)>, ApiError> {
    let session = session_of(state, label)?;
    let diff = owned_label(WindowKind::Diff, label);
    if !exists(&diff) {
        return Ok(None);
    }
    let (send, answer) = mpsc::channel();
    session.diff_flush.lock().unwrap().push_back(send);
    Ok(Some((diff, answer)))
}

async fn flush_diff_of(
    app: &AppHandle,
    state: &AppState,
    label: &str,
    action: &'static str,
) -> Result<(), ApiError> {
    let Some((diff, answer)) =
        flush_request(state, label, |diff| app.get_webview_window(diff).is_some())?
    else {
        return Ok(());
    };
    let window = app.get_webview_window(&diff);
    app.emit_to(diff.as_str(), DIFF_FLUSH_EVENT, ())
        .map_err(|error| failed(error.to_string()))?;
    let flushed = blocking_api(move || wait_for_diff(&answer, DIFF_FLUSH_WAIT, action)).await;
    if flushed.is_err() {
        // The error is in that window, which may be hidden.
        if let Some(window) = window {
            let _ = reveal(&window);
        }
    }
    flushed
}

#[tauri::command]
pub fn diff_flushed(
    saved: bool,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<(), ApiError> {
    let session = session_of(&state, window.label())?;
    answer_flush(&mut session.diff_flush.lock().unwrap(), saved);
    Ok(())
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
// Only the merge window of `window`'s project window is asked.
pub(crate) async fn leave_merge_window(
    app: &AppHandle,
    state: &State<'_, AppState>,
    window: &WebviewWindow,
    action: &'static str,
) -> Result<(), ApiError> {
    leave_merge_of(app, state, window.label(), action).await
}

async fn leave_merge_of(
    app: &AppHandle,
    state: &AppState,
    label: &str,
    action: &'static str,
) -> Result<(), ApiError> {
    let Some((merge, answer)) = merge_leave_request(state, label, |merge| {
        app.get_webview_window(merge).is_some()
    })?
    else {
        return Ok(());
    };
    // The question is in that window, which may be hidden or behind another project window.
    if let Some(window) = app.get_webview_window(&merge) {
        let _ = reveal(&window);
    }
    app.emit_to(merge.as_str(), MERGE_LEAVE_EVENT, ())
        .map_err(|error| failed(error.to_string()))?;
    blocking_api(move || wait_for_merge(&answer, MERGE_ACK_WAIT, MERGE_ANSWER_WAIT, action)).await
}

// The acknowledgement does not take anyone out of the queue: the answer arrives afterwards to that same one.
#[tauri::command]
pub fn merge_leave_received(
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<(), ApiError> {
    let session = session_of(&state, window.label())?;
    answer_merge_received(&session);
    Ok(())
}

#[tauri::command]
pub fn merge_left(
    closed: bool,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<(), ApiError> {
    let session = session_of(&state, window.label())?;
    answer_merge_leave(&session, closed);
    Ok(())
}

#[tauri::command]
pub fn merge_window_ready(
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<(), ApiError> {
    let session = session_of(&state, window.label())?;
    *session.merge_ready.lock().unwrap() = true;
    Ok(())
}

// Hiding the last project window stops watching its repository; what is unsaved is saved first,
// and if that is not possible everything stays as it was and the reason is given in that window.
// Its merge window is only hidden, so it is not asked.
fn leave(
    app: AppHandle,
    owner: String,
    action: &'static str,
    proceed: impl FnOnce(&AppHandle, &str) + Send + 'static,
) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let flushed = flush_diff_of(&app, &state, &owner, action).await;
        after_flush(
            flushed,
            || proceed(&app, &owner),
            |message| emit_to_owner(&app, &owner, LEAVE_REFUSED_EVENT, message),
        );
    });
}

// The leave flow of the project window `owner`, for closing it or quitting: its diff window saves
// what was written, its merge window is left (with its `Cancel Merge` question, shown in that
// merge window), and its commit message draft goes to its changelist. A refusal is the error.
async fn leave_window(app: &AppHandle, owner: &str, action: &'static str) -> Result<(), ApiError> {
    let state = app.state::<AppState>();
    flush_diff_of(app, &state, owner, action).await?;
    leave_merge_of(app, &state, owner, action).await?;
    save_commit_draft(app, &state, owner).await;
    Ok(())
}

// The main page answers `commit-draft-save` once the draft is saved; one that does not answer in
// time (it hangs, or is gone) does not keep the window open.
const COMMIT_DRAFT_SAVE_EVENT: &str = "commit-draft-save";
const COMMIT_DRAFT_WAIT: Duration = Duration::from_secs(5);

// Step 2 of §5.8: the commit message draft of the window `owner` (the Commit Changes dialog's, or
// the Commit tool window's) is saved to the changelist comment, so it comes back when that
// repository is opened again.
async fn save_commit_draft(app: &AppHandle, state: &AppState, owner: &str) {
    if app.get_webview_window(owner).is_none() {
        return;
    }
    let Ok(session) = session_of(state, owner) else {
        return;
    };
    let (send, answer) = mpsc::channel();
    session.draft_saved.lock().unwrap().push_back(send);
    if let Err(error) = app.emit_to(owner, COMMIT_DRAFT_SAVE_EVENT, ()) {
        eprintln!("Could not ask {owner} to save its commit message: {error}");
        return;
    }
    let _ =
        tauri::async_runtime::spawn_blocking(move || answer.recv_timeout(COMMIT_DRAFT_WAIT)).await;
}

#[tauri::command]
pub fn commit_draft_saved(
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<(), ApiError> {
    let session = session_of(&state, window.label())?;
    let mut waiting = session.draft_saved.lock().unwrap();
    while let Some(answer) = waiting.pop_front() {
        if answer.send(()).is_ok() {
            break;
        }
    }
    Ok(())
}

// The main windows that exist, in the order they were opened.
fn main_windows(app: &AppHandle) -> Vec<String> {
    let mut mains: Vec<String> = app
        .webview_windows()
        .into_keys()
        .filter(|label| windows::kind_of(label) == Some(WindowKind::Main))
        .collect();
    let order = app.state::<AppState>().open_order.lock().unwrap().clone();
    mains.sort_by_key(|label| {
        let place = order.iter().position(|known| known == label);
        (place.is_none(), place, label.len(), label.clone())
    });
    mains
}

// Every project window's leave flow, in turn; a refusal in any of them stops the quit for all,
// and its reason is shown in that window. Then a task between stashing and restoring must finish
// first: cancelled, it gives back what was stashed. With the deadline passed it quits anyway, and
// what was stashed is noted down for next time. The open list is not written: it keeps what was
// open at quit.
pub fn quit(app: AppHandle) {
    // From here on the open list is not written: focus changes while windows go must not leave
    // a partial list. A quit that does not go ahead writes again. A second Quit while one runs
    // does nothing.
    if !begin_quit(&app.state::<AppState>()) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let exiting = app.clone();
        let closed = app.clone();
        let left = quit_flow(
            main_windows(&app),
            |owner| {
                let app = app.clone();
                async move { leave_window(&app, &owner, QUIT_ACTION).await }
            },
            move |owner| {
                closed
                    .state::<AppState>()
                    .closed_owners
                    .lock()
                    .unwrap()
                    .contains(owner)
            },
            move || {
                thread::spawn(move || {
                    exiting.state::<AppState>().tasks.finish_writes(QUIT_WAIT);
                    exiting.exit(0);
                });
            },
        )
        .await;
        if let Err((owner, message)) = left {
            app.state::<AppState>()
                .quitting
                .store(false, Ordering::SeqCst);
            emit_to_owner(&app, &owner, LEAVE_REFUSED_EVENT, message);
        }
    });
}

// Queues a waiter for the merge window of `label`'s owner, and says which window to ask. `None`
// when that window does not exist or its page does not listen yet: there is no merge to lose.
fn merge_leave_request(
    state: &AppState,
    label: &str,
    exists: impl FnOnce(&str) -> bool,
) -> Result<Option<(String, Receiver<LeaveReply>)>, ApiError> {
    let session = session_of(state, label)?;
    let merge = owned_label(WindowKind::Merge, label);
    let ready = *session.merge_ready.lock().unwrap();
    if !must_ask_merge(exists(&merge), ready) {
        return Ok(None);
    }
    let (send, answer) = mpsc::channel();
    session.merge_leave.lock().unwrap().push_back(send);
    Ok(Some((merge, answer)))
}

// The merge page's acknowledgement of `merge-leave`.
// The acknowledgement does not take anyone out of the queue: the answer arrives afterwards to that
// same one.
fn answer_merge_received(session: &WindowSession) {
    let mut waiting = session.merge_leave.lock().unwrap();
    while let Some(front) = waiting.front() {
        if front.send(LeaveReply::Received).is_ok() {
            break;
        }
        waiting.pop_front();
    }
}

// The merge page's answer to `merge-leave`: whether it can be closed.
fn answer_merge_leave(session: &WindowSession, closed: bool) {
    let mut waiting = session.merge_leave.lock().unwrap();
    while let Some(answer) = waiting.pop_front() {
        if answer.send(LeaveReply::Answer(closed)).is_ok() {
            break;
        }
    }
}

// Marks the app as quitting; `false` if a quit already runs.
fn begin_quit(state: &AppState) -> bool {
    state
        .quitting
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}

// Quit's leave flow: each project window in turn, as `leave` runs it. The first refusal stops it,
// with its window and its reason, and `finish` (waiting for writes and exiting) is not called;
// the windows after it are not asked. A window whose close has finished (`closed`) is not asked,
// and a refusal from one that finished closing while it was asked does not count: it has gone.
async fn quit_flow<F, Fut>(
    owners: Vec<String>,
    leave: F,
    closed: impl Fn(&str) -> bool,
    finish: impl FnOnce(),
) -> Result<(), (String, String)>
where
    F: FnMut(String) -> Fut,
    Fut: std::future::Future<Output = Result<(), ApiError>>,
{
    let mut leave = leave;
    for owner in owners {
        if closed(&owner) {
            continue;
        }
        if let Err(error) = leave(owner.clone()).await {
            if closed(&owner) {
                continue;
            }
            return Err((owner, error.message));
        }
    }
    finish();
    Ok(())
}

// Steps 3 to 5 of closing the project window `owner` (§5.8), once its leave flow went ahead:
// 3. its reads are cancelled and its writes and remote tasks waited for, up to `limit`, while it
//    starts no new one. A question they ask meanwhile still reaches the window, which is there
//    until the end. Other windows go on.
// 4. its auxiliary windows are destroyed (`destroy_aux`);
// 5. its session goes, which stops its watcher, with its console lines and its place in the
//    open list.
// Before step 4 the diff window saves again (`flush`), and then the commit message draft
// (`save_draft`): the window stays usable during the wait, and what was typed in either meanwhile
// would go with it. Then, if every other main window went meanwhile (`others_remain`), this one is
// the last: it is hidden instead, keeping its session and its place in the open list. The check
// is here, before anything is destroyed, so that hiding still has everything to keep.
fn finish_closing(
    state: &AppState,
    owner: &str,
    limit: Duration,
    flush: impl FnOnce() -> Result<(), ApiError>,
    save_draft: impl FnOnce(),
    others_remain: impl FnOnce() -> bool,
    destroy_aux: impl FnOnce(),
) -> CloseOutcome {
    if !state.tasks.finish_writes_of(owner, limit) {
        state.tasks.resume_writes_of(owner);
        return CloseOutcome::TimedOut;
    }
    if let Err(error) = flush() {
        state.tasks.resume_writes_of(owner);
        return CloseOutcome::Refused(error.message);
    }
    save_draft();
    if !others_remain() {
        state.tasks.resume_writes_of(owner);
        return CloseOutcome::Hide;
    }
    destroy_aux();
    forget_window(state, owner);
    CloseOutcome::Closed
}

#[derive(Debug, PartialEq, Eq)]
enum CloseOutcome {
    Closed,
    // Its writes did not finish in time: nothing was closed and the window writes again.
    TimedOut,
    // The diff window could not save: the same, with the reason.
    Refused(String),
    // Every other main window went meanwhile: the window is hidden, as the last one is.
    Hide,
}

#[derive(Debug, PartialEq, Eq)]
enum CloseStart {
    Close,
    // The last main window, not counting those already closing: hidden, as today.
    Hide,
    AlreadyClosing,
}

// How a close of `owner` starts, among the main windows `mains` that exist. Deciding and marking
// it as closing go under one lock, so two closes at once never both destroy their window.
fn start_close(state: &AppState, owner: &str, mains: &[String]) -> CloseStart {
    let mut closing = state.closing_windows.lock().unwrap();
    if closing.contains(owner) {
        return CloseStart::AlreadyClosing;
    }
    let others = mains
        .iter()
        .any(|main| main != owner && !closing.contains(main));
    if !others {
        return CloseStart::Hide;
    }
    closing.insert(owner.to_string());
    CloseStart::Close
}

// Whether a main window other than `owner` stays: one of `mains` that is not closing itself.
fn other_main_remains(state: &AppState, owner: &str, mains: &[String]) -> bool {
    let closing = state.closing_windows.lock().unwrap();
    mains
        .iter()
        .any(|main| main != owner && !closing.contains(main))
}

// A window that is closing, or closed, opens no repository: its session would outlive it.
fn refuse_if_closing(state: &AppState, owner: &str) -> Result<(), ApiError> {
    refuse_while_closing(state, owner, OPEN_ACTION)
}

fn refuse_while_closing(state: &AppState, owner: &str, action: &str) -> Result<(), ApiError> {
    let closed = state.closed_owners.lock().unwrap().contains(owner);
    if closed || state.closing_windows.lock().unwrap().contains(owner) {
        return Err(failed(format!("The window is closing: {action}")));
    }
    Ok(())
}

// The session a merge window of `label`'s project window opens in. A closing window opens none:
// its leave flow has already asked its merge window, and a new one would be destroyed with it.
fn merge_open_session(state: &AppState, label: &str) -> Result<Arc<WindowSession>, ApiError> {
    let owner = windows::owner_of(label).unwrap_or_else(|| MAIN_WINDOW.to_string());
    refuse_while_closing(state, &owner, MERGE_OPEN_ACTION)?;
    session_of(state, label)
}

// Writes the open list into `base`, unless the app is quitting: the list keeps what was open at
// quit, whatever the windows do while the app goes. `false` if nothing was written.
fn write_open_list(
    state: &AppState,
    base: &Path,
    exists: impl Fn(&str) -> bool,
) -> std::io::Result<bool> {
    let _writing = state
        .open_list_write
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if state.quitting.load(Ordering::SeqCst) {
        return Ok(false);
    }
    open_projects::save(base, &open_snapshot(state, exists))?;
    Ok(true)
}

fn forget_window(state: &AppState, owner: &str) {
    let session = {
        let mut sessions = state.sessions.lock().unwrap();
        state
            .closed_owners
            .lock()
            .unwrap()
            .insert(owner.to_string());
        sessions.remove(owner)
    };
    if let Some(session) = session {
        replace_watcher(&session, None);
        viewer::release(&session);
        block_history::release(&state.tasks, &session);
    }
    state.console.forget(owner);
    SHELF_FOLDERS.close(owner);
    state.initial_targets.lock().unwrap().remove(owner);
    let first = {
        let mut order = state.open_order.lock().unwrap();
        order.retain(|label| label != owner);
        order.first().cloned()
    };
    // The window macOS focuses next says so itself (`main_focused`); until then, another one.
    let mut last = state.last_focused_main.lock().unwrap();
    if *last == owner {
        *last = first.unwrap_or_else(|| MAIN_WINDOW.to_string());
    }
}

// Each main window of `open_order` that `exists`, in opening order, with its repository or, while
// its first open has not ended, the target it was built for (a project counts as open from the
// moment it starts opening).
fn window_repos(state: &AppState, exists: impl Fn(&str) -> bool) -> Vec<(String, String)> {
    let order: Vec<String> = state
        .open_order
        .lock()
        .unwrap()
        .iter()
        .filter(|label| exists(label))
        .cloned()
        .collect();
    let sessions: HashMap<String, Arc<WindowSession>> = state.sessions.lock().unwrap().clone();
    let mut open = Vec::new();
    for label in order {
        let repo = sessions
            .get(&label)
            .and_then(|session| session_repo(session).ok())
            .map(|path| path.display().to_string());
        let target = || {
            let targets = state.initial_targets.lock().unwrap();
            let target = targets
                .stored()
                .find(|(stored, _)| *stored == label)
                .map(|(_, target)| target.to_string());
            target
        };
        if let Some(repo) = repo.or_else(target) {
            open.push((label, repo));
        }
    }
    open
}

// The open list as the windows are now (`window_repos`), and the one focused last.
fn open_snapshot(state: &AppState, exists: impl Fn(&str) -> bool) -> OpenProjects {
    let open = window_repos(state, exists);
    let focused = state.last_focused_main.lock().unwrap().clone();
    open_projects::snapshot(&open, Some(&focused))
}

// The project widget's popup: the other windows' repositories and the recent ones that are not
// open in any window (`recent::projects_list`).
#[tauri::command]
pub async fn projects_list(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<recent::ProjectsList, ApiError> {
    let own = owner_label(&window);
    let windows = window_repos(&state, |label| app.get_webview_window(label).is_some());
    let base = app
        .path()
        .app_data_dir()
        .map_err(|error| failed(error.to_string()))?;
    let home = std::env::var_os("HOME").map(PathBuf::from);
    blocking_api(move || {
        let recent = recent::load(&base).map_err(|error| failed(error.to_string()))?;
        Ok(recent::projects_list(
            &windows,
            &own,
            &recent,
            home.as_deref(),
        ))
    })
    .await
}

// Rewrites the open list (§5.8): on an open, This Window, a window close and a focus change
// between main windows, never on quit. A failure is only logged: the list is an aid.
pub(crate) fn write_open_projects(app: &AppHandle) {
    let Ok(base) = app.path().app_data_dir() else {
        return;
    };
    let state = app.state::<AppState>();
    if let Err(error) = write_open_list(&state, &base, |label| {
        app.get_webview_window(label).is_some()
    }) {
        eprintln!("Could not save the open projects: {error}");
    }
}

// What opening another repository destroys in the project window `owner`: its auxiliary windows,
// which showed the previous repository.
fn aux_labels(owner: &str) -> Vec<String> {
    [
        WindowKind::Diff,
        WindowKind::Merge,
        WindowKind::Viewer,
        WindowKind::SelectionHistory,
    ]
    .map(|kind| windows::label_for(kind, owner))
    .to_vec()
}

// What closing the project window `owner` hides: its auxiliary windows and then itself.
fn close_labels(owner: &str) -> Vec<String> {
    let mut labels = aux_labels(owner);
    labels.push(windows::label_for(WindowKind::Main, owner));
    labels
}

// The red button or ⌘W on the main window `label` (§5.8). The last main window keeps today's
// behaviour: it is hidden with its auxiliary windows, its repository stays open (and in the open
// list), and the Dock shows it again. Any other one is closed: its leave flow, its commit message
// draft, the wait for its writes, its auxiliary windows, its session, the open list, and then the
// window itself. A refusal or a wait past the limit keeps everything open and says why there.
pub fn close_project_window(app: AppHandle, label: &str) {
    let owner = label.to_string();
    let mains = existing_mains(&app);
    match start_close(&app.state::<AppState>(), &owner, &mains) {
        // A second click while the first close runs adds nothing.
        CloseStart::AlreadyClosing => {}
        CloseStart::Hide => hide_last_window(app, owner),
        CloseStart::Close => {
            tauri::async_runtime::spawn(async move {
                let closed = close_steps(&app, &owner).await;
                app.state::<AppState>()
                    .closing_windows
                    .lock()
                    .unwrap()
                    .remove(&owner);
                if let Err(message) = closed {
                    emit_to_owner(&app, &owner, LEAVE_REFUSED_EVENT, message);
                }
            });
        }
    }
}

// The main windows that exist now.
fn existing_mains(app: &AppHandle) -> Vec<String> {
    app.webview_windows()
        .into_keys()
        .filter(|label| windows::kind_of(label) == Some(WindowKind::Main))
        .collect()
}

// Today's close of the last window: everything is hidden after the diff window saves.
fn hide_last_window(app: AppHandle, owner: String) {
    leave(app, owner, CLOSE_ACTION, |app, owner| {
        hide_windows(app, owner)
    });
}

fn hide_windows(app: &AppHandle, owner: &str) {
    for label in close_labels(owner) {
        if let Some(window) = app.get_webview_window(&label) {
            let _ = window.hide();
        }
    }
    stop_watching(&app.state::<AppState>(), owner);
}

async fn close_steps(app: &AppHandle, owner: &str) -> Result<(), String> {
    // 1 and 2: the leave flow and the commit message draft.
    leave_window(app, owner, CLOSE_ACTION)
        .await
        .map_err(|error| error.message)?;
    // 3 to 5, with the diff window and the commit message draft saved again right before they go.
    let (handle, closing) = (app.clone(), owner.to_string());
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<AppState>();
        finish_closing(
            &state,
            &closing,
            QUIT_WAIT,
            // A thread of the blocking pool may wait for a future.
            || {
                tauri::async_runtime::block_on(flush_diff_of(
                    &handle,
                    &handle.state::<AppState>(),
                    &closing,
                    CLOSE_ACTION,
                ))
            },
            || {
                tauri::async_runtime::block_on(save_commit_draft(
                    &handle,
                    &handle.state::<AppState>(),
                    &closing,
                ))
            },
            || other_main_remains(&state, &closing, &existing_mains(&handle)),
            || {
                for label in aux_labels(&closing) {
                    if let Some(window) = handle.get_webview_window(&label) {
                        let _ = window.destroy();
                    }
                }
            },
        )
    })
    .await
    .unwrap_or(CloseOutcome::TimedOut);
    match outcome {
        CloseOutcome::Closed => {}
        CloseOutcome::TimedOut => {
            return Err(format!(
                "The window's tasks did not finish in time: {CLOSE_ACTION}"
            ))
        }
        CloseOutcome::Refused(message) => return Err(message),
        // Every other main window went meanwhile: this one is the last, and stays hidden.
        CloseOutcome::Hide => {
            hide_windows(app, owner);
            return Ok(());
        }
    }
    // 6. The open list without it.
    write_open_projects(app);
    // 7.
    if let Some(window) = app.get_webview_window(owner) {
        window.destroy().map_err(|error| error.to_string())?;
    }
    Ok(())
}

// `terminate:`, what the stock Quit does, does not let the exit be stopped: it is replaced with one of our own
// with the same place, name and shortcut. Dock Quit and logout still go through `terminate:`.
pub fn app_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let menu = Menu::default(app)?;
    replace_quit(app, &menu)?;
    add_open_item(app, &menu)?;
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

fn is_file_menu(text: &str) -> bool {
    text == MAC_FILE_MENU
}

// First in `File`, above `Close Window`, as IntelliJ's Open… heads its File menu.
fn add_open_item(app: &AppHandle, menu: &Menu<Wry>) -> tauri::Result<()> {
    for item in menu.items()? {
        let MenuItemKind::Submenu(submenu) = item else {
            continue;
        };
        if is_file_menu(&submenu.text()?) {
            let open = MenuItem::with_id(app, OPEN_MENU_ID, OPEN_MENU_TEXT, true, None::<&str>)?;
            // Off until `refresh_recent_menu` fills it, as IntelliJ's with an empty list.
            let recent = Submenu::with_id(app, RECENT_MENU_ID, RECENT_MENU_TEXT, false)?;
            submenu.insert(&PredefinedMenuItem::separator(app)?, 0)?;
            submenu.insert(&recent, 0)?;
            submenu.insert(&open, 0)?;
            return Ok(());
        }
    }
    Ok(())
}

fn recent_menu_id(path: &str) -> String {
    format!("{RECENT_ITEM_PREFIX}{path}")
}

pub fn recent_menu_path(id: &str) -> Option<&str> {
    id.strip_prefix(RECENT_ITEM_PREFIX)
}

fn recent_submenu(app: &AppHandle) -> tauri::Result<Option<Submenu<Wry>>> {
    let Some(menu) = app.menu() else {
        return Ok(None);
    };
    for item in menu.items()? {
        let MenuItemKind::Submenu(submenu) = item else {
            continue;
        };
        for entry in submenu.items()? {
            if let MenuItemKind::Submenu(recent) = entry {
                if recent.id().0 == RECENT_MENU_ID {
                    return Ok(Some(recent));
                }
            }
        }
    }
    Ok(None)
}

// Reads the list again (another window may have added to it) and rebuilds the submenu, without
// this window's repository.
pub fn refresh_recent_menu(app: &AppHandle) {
    let Ok(base) = app.path().app_data_dir() else {
        return;
    };
    let projects = match recent::load(&base) {
        Ok(projects) => projects,
        Err(error) => {
            eprintln!("Could not read the recent projects: {error}");
            return;
        }
    };
    let current = menu_repo(&app.state::<AppState>());
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let entries = recent::menu_entries(&projects, current.as_deref(), home.as_deref());
    if let Err(error) = fill_recent_menu(app, &entries) {
        eprintln!("Could not update the Recent Projects menu: {error}");
    }
}

fn fill_recent_menu(app: &AppHandle, entries: &[recent::Entry]) -> tauri::Result<()> {
    let Some(submenu) = recent_submenu(app)? else {
        return Ok(());
    };
    for item in submenu.items()? {
        submenu.remove(&item)?;
    }
    for entry in entries {
        let id = recent_menu_id(&entry.path);
        let item = MenuItem::with_id(app, id, &entry.label, true, None::<&str>)?;
        submenu.append(&item)?;
    }
    submenu.set_enabled(!entries.is_empty())
}

fn remember_recent(app: &AppHandle, path: &str) {
    let Ok(base) = app.path().app_data_dir() else {
        return;
    };
    if let Err(error) = recent::record(&base, path) {
        eprintln!("Could not save the recent projects: {error}");
    }
    refresh_recent_menu(app);
}

#[derive(Clone, Serialize)]
struct OpenRecent {
    path: String,
    exists: bool,
}

// `ReopenProjectAction`: the main window opens it, or asks about a path that no longer exists.
// The last focused main window: `main` may have been closed.
pub fn open_recent(app: &AppHandle, path: &str) {
    let label = last_main_label(app);
    if let Some(window) = app.get_webview_window(&label) {
        let _ = reveal(&window);
        resume_and_notify(app, &label);
        let payload = OpenRecent {
            path: path.to_string(),
            exists: Path::new(path).exists(),
        };
        emit_to_owner(app, &label, OPEN_RECENT_EVENT, payload);
    }
}

// `Remove From List` in the dialog for a path that does not exist.
#[tauri::command]
pub async fn recent_projects_remove(path: String, app: AppHandle) -> Result<(), ApiError> {
    let base = app
        .path()
        .app_data_dir()
        .map_err(|error| failed(error.to_string()))?;
    blocking_api(move || {
        recent::remove(&base, &path)
            .map(drop)
            .map_err(|error| failed(error.to_string()))
    })
    .await?;
    refresh_recent_menu(&app);
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
            Group(id, _, _) if starts_hidden(id) => {}
            Group(id, text, children) => into.append(&group_of(app, id, text, children)?)?,
            Separator => into.append(&PredefinedMenuItem::separator(app)?)?,
        }
    }
    Ok(())
}

fn group_of(
    app: &AppHandle,
    id: &str,
    text: &str,
    children: &[GitMenuEntry],
) -> tauri::Result<Submenu<Wry>> {
    let group = Submenu::with_id(app, id, text, !starts_disabled(id))?;
    append_entries(app, &group, children)?;
    Ok(group)
}

// Puts the GitHub submenu in, right before `Manage Remotes…`, or takes it out.
fn sync_github_menu(app: &AppHandle, shown: bool) -> tauri::Result<()> {
    let Some(menu) = app.menu() else {
        return Ok(());
    };
    let mut git = None;
    for item in menu.items()? {
        if let MenuItemKind::Submenu(submenu) = item {
            if submenu.get("Git.Configure.Remotes").is_some() {
                git = Some(submenu);
                break;
            }
        }
    }
    let Some(git) = git else {
        return Ok(());
    };
    match (shown, git.get(GITHUB_GROUP)) {
        (false, Some(group)) => git.remove(&group),
        (true, None) => {
            let Some(Group(id, text, children)) = GIT_MENU
                .iter()
                .find(|entry| matches!(entry, Group(id, _, _) if *id == GITHUB_GROUP))
            else {
                return Ok(());
            };
            let items = git.items()?;
            let Some(position) = items
                .iter()
                .position(|item| item.id().0 == "Git.Configure.Remotes")
            else {
                return Ok(());
            };
            git.insert(&group_of(app, id, text, children)?, position)
        }
        _ => Ok(()),
    }
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

// What the Patch submenu shows: `Create Patch from Local Changes...` only without the staging area.
fn patch_menu(staging: bool) -> Vec<(&'static str, &'static str)> {
    let items = GIT_MENU
        .iter()
        .find_map(|entry| match entry {
            Group(id, _, children) if *id == PATCH_GROUP => Some(menu_items(children)),
            _ => None,
        })
        .unwrap_or_default();
    items
        .into_iter()
        .filter(|(id, _)| !(staging && *id == CREATE_PATCH))
        .collect()
}

fn find_submenu(items: Vec<MenuItemKind<Wry>>, id: &str) -> tauri::Result<Option<Submenu<Wry>>> {
    for item in items {
        if let MenuItemKind::Submenu(submenu) = item {
            if submenu.id().0 == id {
                return Ok(Some(submenu));
            }
            if let Some(found) = find_submenu(submenu.items()?, id)? {
                return Ok(Some(found));
            }
        }
    }
    Ok(None)
}

// The native menu cannot hide an entry: it is taken out of the submenu with the staging area and
// put back without it. A submenu left empty is disabled.
fn sync_patch_menu(app: &AppHandle, staging: bool) -> tauri::Result<()> {
    let Some(menu) = app.menu() else {
        return Ok(());
    };
    let Some(group) = find_submenu(menu.items()?, PATCH_GROUP)? else {
        return Ok(());
    };
    let shown = group.get(CREATE_PATCH);
    match (staging, shown) {
        (true, Some(item)) => group.remove(&item)?,
        (false, None) => {
            let item = MenuItem::with_id(app, CREATE_PATCH, CREATE_PATCH_TEXT, true, None::<&str>)?;
            group.insert(&item, 0)?;
        }
        _ => {}
    }
    group.set_enabled(!patch_menu(staging).is_empty())
}

// What `Uncommitted Changes` shows: `Shelve Changes...` only without the staging area.
fn local_changes_menu(staging: bool) -> Vec<(&'static str, &'static str)> {
    let items = GIT_MENU
        .iter()
        .find_map(|entry| match entry {
            Group(id, _, children) if *id == LOCAL_CHANGES_GROUP => Some(menu_items(children)),
            _ => None,
        })
        .unwrap_or_default();
    items
        .into_iter()
        .filter(|(id, _)| !(staging && *id == SHELVE_CHANGES))
        .collect()
}

// As for the Patch submenu: `Shelve Changes...` is taken out with the staging area and put back,
// first, without it.
fn sync_local_changes_menu(app: &AppHandle, staging: bool) -> tauri::Result<()> {
    let Some(menu) = app.menu() else {
        return Ok(());
    };
    let Some(group) = find_submenu(menu.items()?, LOCAL_CHANGES_GROUP)? else {
        return Ok(());
    };
    let wanted = local_changes_menu(staging)
        .iter()
        .any(|(id, _)| *id == SHELVE_CHANGES);
    match (wanted, group.get(SHELVE_CHANGES)) {
        (false, Some(item)) => group.remove(&item),
        (true, None) => {
            let item =
                MenuItem::with_id(app, SHELVE_CHANGES, SHELVE_CHANGES_TEXT, true, None::<&str>)?;
            group.insert(&item, 0)
        }
        _ => Ok(()),
    }
}

pub fn is_git_menu_item(id: &str) -> bool {
    menu_items(&GIT_MENU).iter().any(|(item, _)| *item == id)
}

// The entries of the operation in progress that apply now: the interface computes them from the repository
// state every time it changes. `Github.View.Pull.Request` among them puts the GitHub submenu in.
#[tauri::command]
pub fn git_menu_enable(
    ids: Vec<String>,
    app: AppHandle,
    window: WebviewWindow,
) -> Result<(), ApiError> {
    if !menu_accepts(&app.state::<AppState>(), window.label()) {
        return Ok(());
    }
    let Some(menu) = app.menu() else {
        return Ok(());
    };
    let items = menu.items().map_err(|error| failed(error.to_string()))?;
    set_ongoing_enabled(items, &ids).map_err(|error| failed(error.to_string()))?;
    sync_github_menu(&app, github_menu_shown(&ids)).map_err(|error| failed(error.to_string()))
}

// An entry of `Git > Current File` as the focused window computes it for its selection.
#[derive(Debug, Clone, Deserialize)]
pub struct CurrentFileItem {
    id: String,
    text: String,
    enabled: bool,
}

// `GitFileActionGroup.update` and the update of each file action: the window that has the focus
// says the title and its entries; an entry it does not name is off.
#[tauri::command]
pub fn git_menu_current_file(
    title: String,
    items: Vec<CurrentFileItem>,
    app: AppHandle,
    window: WebviewWindow,
) -> Result<(), ApiError> {
    if !menu_accepts(&app.state::<AppState>(), window.label()) {
        return Ok(());
    }
    let set = || -> tauri::Result<()> {
        let Some(menu) = app.menu() else {
            return Ok(());
        };
        let Some(group) = find_submenu(menu.items()?, FILE_GROUP)? else {
            return Ok(());
        };
        group.set_text(&title)?;
        for kind in group.items()? {
            if let MenuItemKind::MenuItem(item) = kind {
                match items.iter().find(|wanted| wanted.id == item.id().0) {
                    Some(wanted) => {
                        item.set_text(&wanted.text)?;
                        item.set_enabled(wanted.enabled)?;
                    }
                    None => item.set_enabled(false)?,
                }
            }
        }
        Ok(())
    };
    set().map_err(|error| failed(error.to_string()))
}

// The entries of `Current File` act on the selection of the focused window: the viewer's own file
// when the menu owner's viewer has the focus. The rest go to the owner's main window, never to
// another project window's.
fn action_window(id: &str, owner: &str, focused: Option<&str>) -> String {
    let viewer = windows::label_for(WindowKind::Viewer, owner);
    if focused == Some(viewer.as_str()) && FILE_MENU.contains(&id) {
        viewer
    } else {
        owner.to_string()
    }
}

// It does not raise the window: the interface does that if the action is going to be handled, and a modal dialog or
// the lack of a repository leave it as it was.
pub fn git_menu_action(app: &AppHandle, id: &str) {
    let owner = menu_owner(&app.state::<AppState>());
    let viewer = windows::label_for(WindowKind::Viewer, &owner);
    let viewer_focused = app
        .get_webview_window(&viewer)
        .and_then(|window| window.is_focused().ok())
        .unwrap_or(false);
    let focused = viewer_focused.then_some(viewer.as_str());
    let _ = app.emit_to(action_window(id, &owner, focused), GIT_ACTION_EVENT, id);
}

const GIT_FILE_ACTION_EVENT: &str = "git-file-action";

// A file action of the viewer's editor menu that the main window carries out (Commit File…, Add,
// Rollback…, Resolve Conflicts…), with the files it applies to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitFileAction {
    id: String,
    paths: Vec<String>,
}

#[tauri::command]
pub fn git_file_action(
    action: GitFileAction,
    app: AppHandle,
    window: WebviewWindow,
) -> Result<(), ApiError> {
    try_emit_to_owner(&app, &owner_label(&window), GIT_FILE_ACTION_EVENT, action)
        .map_err(|error| failed(error.to_string()))
}

#[tauri::command]
pub async fn repo_open(
    repo: String,
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<RepoInfo, ApiError> {
    flush_diff_window(&app, &state, &window, OPEN_ACTION).await?;
    leave_merge_window(&app, &state, &window, OPEN_ACTION).await?;
    let owner = owner_label(&window);
    refuse_if_closing(&state, &owner)?;
    let session = session_of(&state, window.label())?;
    let target = PathBuf::from(repo);
    let path = PathBuf::from(blocking_api(move || resolve_project(&target)).await?);
    // Another window that has it open, or is opening it, comes to the front instead.
    let _claim = claim_opening(&state, &owner, &path, |label| {
        app.get_webview_window(label).is_some()
    })
    .map_err(|label| {
        focus_main(&app, &label);
        ApiError {
            kind: "open_elsewhere",
            message: OPEN_ELSEWHERE.to_string(),
            details: None,
        }
    })?;
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
        prune_snapshots(data_base.clone(), root.clone());
        clean_shelf(app.clone(), data_base, owner.clone(), root.clone());
        let watcher = watch::start(WatchedPaths::of(&root)?, notifier(app, owner))?;
        Ok((info, watcher))
    })
    .await?;
    *session.repo.lock().unwrap() = Some(RepoSession {
        path: PathBuf::from(&info.path),
    });
    *session.log.lock().unwrap() = None;
    replace_watcher(&session, Some(watcher));
    state.branch_sync.request();
    remember_recent(&app_for_diff, &info.path);
    // It belonged to another repository; what it showed, and its selection, no longer apply.
    *session.merge_session.lock().unwrap() = None;
    *session.merge_ready.lock().unwrap() = false;
    viewer::release(&session);
    block_history::release(&state.tasks, &session);
    for label in aux_labels(&owner_label(&window)) {
        if let Some(aux) = app_for_diff.get_webview_window(&label) {
            let _ = aux.destroy();
        }
    }
    // An open, or This Window replacing the repository: the window keeps its place in the list.
    drop(_claim);
    write_open_projects(&app_for_diff);
    Ok(info)
}

// The half-done operation and the widget text. `repo-changed` makes the interface read it again.
#[tauri::command]
pub async fn repo_state(
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<RepoState, ApiError> {
    let repo = current_repo(&state, &window)?;
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

// `ShelveChangesManager`: the shelf's retention runs when the project opens, then once a day
// (`scheduleWithFixedDelay(…, 1, 1, DAYS)`) while that repository stays open. Like the pruning,
// it never delays the opening and its result is not looked at. One thread does it for the app: an
// open hands it the repository's data folder.
const SHELF_CLEANING_PERIOD: Duration = Duration::from_secs(24 * 60 * 60);

// The data folder of each open project window's repository, by owner: what the cleaner serves.
#[derive(Default)]
struct ShelfFolders(Mutex<BTreeMap<String, PathBuf>>);

impl ShelfFolders {
    // A window opened `dir`; it takes the place of the one it had.
    fn open(&self, owner: &str, dir: PathBuf) {
        self.lock().insert(owner.to_string(), dir);
    }

    // The window closed.
    fn close(&self, owner: &str) {
        self.lock().remove(owner);
    }

    fn dir_of(&self, owner: &str) -> Option<PathBuf> {
        self.lock().get(owner).cloned()
    }

    fn owners(&self) -> Vec<String> {
        self.lock().keys().cloned().collect()
    }

    #[cfg(test)]
    fn dirs(&self) -> Vec<PathBuf> {
        self.lock().values().cloned().collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, PathBuf>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

static SHELF_FOLDERS: ShelfFolders = ShelfFolders(Mutex::new(BTreeMap::new()));
static SHELF_CLEANER: Mutex<Option<Sender<String>>> = Mutex::new(None);

fn clean_shelf(app: AppHandle, base: Option<PathBuf>, owner: String, repo: PathBuf) {
    let Some(base) = base else { return };
    SHELF_FOLDERS.open(&owner, storage::repo_dir(&base, &repo));
    let mut cleaner = SHELF_CLEANER.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(sender) = cleaner.as_ref() {
        if sender.send(owner.clone()).is_ok() {
            return;
        }
    }
    let (sender, receiver) = mpsc::channel();
    let _ = sender.send(owner);
    *cleaner = Some(sender);
    thread::spawn(move || {
        run_shelf_cleaner(
            receiver,
            SHELF_CLEANING_PERIOD,
            || SHELF_FOLDERS.owners(),
            |owner| {
                // A window closed since it was asked for has no folder any more.
                let Some(dir) = SHELF_FOLDERS.dir_of(owner) else {
                    return;
                };
                let before = shelf::generation(&dir);
                // What the user moved into `shelf-restore/` comes back before the cleanup looks.
                let _ = shelf::tab::restore_requested(&dir, SystemTime::now());
                let _ = shelf::cleanup(&dir, SystemTime::now());
                // What the Shelf tab shows changed: that window reads the lists again.
                if shelf::generation(&dir) != before {
                    repo_changed(&app, owner);
                }
            },
        )
    });
}

// Cleans the window that opened at once, then every window in `owners()` each `period`.
fn run_shelf_cleaner(
    receiver: Receiver<String>,
    period: Duration,
    owners: impl Fn() -> Vec<String>,
    mut clean: impl FnMut(&str),
) {
    // An open does not push the periodic round back.
    let mut next = Instant::now() + period;
    loop {
        match receiver.recv_timeout(next.saturating_duration_since(Instant::now())) {
            Ok(owner) => clean(&owner),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                next = Instant::now() + period;
                for owner in owners() {
                    clean(&owner);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

const PERF_OUT_VARIABLE: &str = "RAMIFY_PERF_OUT";

const TASK_CHANGED_EVENT: &str = "task-changed";
const CONSOLE_LINE_EVENT: &str = "console-line";
const ASKPASS_PROMPT_EVENT: &str = "askpass-prompt";
const ASKPASS_DISMISSED_EVENT: &str = "askpass-dismissed";
const EDITOR_REQUEST_EVENT: &str = "editor-request";
const EDITOR_DISMISSED_EVENT: &str = "editor-dismissed";
const OTHER_WINDOWS_PROMPT: &str = "The question belongs to another window";

// Tasks and the console report what happens to the windows of the project window whose command
// started them, and to no other. The merge one launches its own and needs their result.
pub fn forward_task_events(app: &AppHandle) {
    let state = app.state::<AppState>();
    let handle = app.clone();
    state.tasks.listen(move |event| {
        emit_to_owner(&handle, &event.owner, TASK_CHANGED_EVENT, event);
    });
    let handle = app.clone();
    state.console.listen(move |line| {
        emit_to_owner(&handle, &line.owner, CONSOLE_LINE_EVENT, line);
    });
}

#[derive(Clone, Serialize)]
struct AskpassPrompt {
    id: u64,
    kind: PromptKind,
}

// The question goes to the project window of the task that asks (its token says which). That
// window may be hidden with a task running, and the dialog lives in it. If the question does not
// reach the window, no one will answer: `false` is returned and git fails instead of waiting.
pub fn start_askpass(app: &AppHandle) {
    let (asked, dismissed) = (app.clone(), app.clone());
    let server = Server::start(
        move |owner, id, kind| {
            let Some(window) = asked.get_webview_window(owner) else {
                return false;
            };
            let _ = reveal(&window);
            resume_and_notify(&asked, owner);
            try_emit_to_owner(
                &asked,
                owner,
                ASKPASS_PROMPT_EVENT,
                AskpassPrompt { id, kind },
            )
            .is_ok()
        },
        move |owner, id| {
            emit_to_owner(&dismissed, owner, ASKPASS_DISMISSED_EVENT, id);
        },
    );
    let state = app.state::<AppState>();
    match server {
        Ok(server) => {
            state.tasks.use_askpass(server.handle());
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
// Only the project window the question went to may answer it; another window's answer is refused
// and the question keeps waiting.
#[tauri::command]
pub fn askpass_answer(
    id: u64,
    answer: Option<String>,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<(), ApiError> {
    let server = state.askpass.lock().unwrap_or_else(PoisonError::into_inner);
    match server.as_ref() {
        Some(server) if !server.answer(&owner_label(&window), id, answer) => {
            Err(failed(OTHER_WINDOWS_PROMPT.to_string()))
        }
        _ => Ok(()),
    }
}

#[derive(Clone, Serialize)]
struct EditorRequest {
    id: u64,
    #[serde(flatten)]
    prompt: Prompt,
}

// Like `start_askpass`: the request goes to the project window of the task's registration, whose
// main window holds the dialog and may be hidden with the task running. If the request does not
// arrive, no one will answer and git fails instead of waiting.
pub fn start_editor(app: &AppHandle) {
    let asked = app.clone();
    let dismissed = app.clone();
    let server = editor::Server::start(
        move |owner, id, prompt| {
            let Some(window) = asked.get_webview_window(owner) else {
                return false;
            };
            let _ = reveal(&window);
            resume_and_notify(&asked, owner);
            try_emit_to_owner(
                &asked,
                owner,
                EDITOR_REQUEST_EVENT,
                EditorRequest { id, prompt },
            )
            .is_ok()
        },
        move |owner, id| {
            emit_to_owner(&dismissed, owner, EDITOR_DISMISSED_EVENT, id);
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
// repository. The text continues through the socket to git's editor, which writes it. Like
// `askpass_answer`, only the project window the request went to may answer it.
#[tauri::command]
pub fn editor_answer(
    id: u64,
    text: Option<String>,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<(), ApiError> {
    let server = state.editor.lock().unwrap_or_else(PoisonError::into_inner);
    match server.as_ref() {
        Some(server) if !server.answer(&owner_label(&window), id, text) => {
            Err(failed(OTHER_WINDOWS_PROMPT.to_string()))
        }
        _ => Ok(()),
    }
}

// Only a task of the calling project window: another window's goes on.
#[tauri::command]
pub fn task_cancel(id: TaskId, window: WebviewWindow, state: State<'_, AppState>) {
    state.tasks.cancel(&owner_label(&window), id);
}

// The lines of the calling project window, like IntelliJ's console per project.
#[tauri::command]
pub fn console_lines(window: WebviewWindow, state: State<'_, AppState>) -> Vec<ConsoleLine> {
    state.console.lines_of(&owner_label(&window))
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

pub(crate) fn current_repo(
    state: &State<'_, AppState>,
    window: &WebviewWindow,
) -> Result<PathBuf, ApiError> {
    repo_of(state, window.label())
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
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Vec<ChangedFile>, ApiError> {
    let repo = current_repo(&state, &window)?;
    blocking(move || diff::commit_files(&repo, &hash)).await
}

#[tauri::command]
pub async fn file_sides(
    hash: String,
    path: String,
    old_path: Option<String>,
    base: Option<String>,
    no_base: bool,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<FileSides, ApiError> {
    let repo = current_repo(&state, &window)?;
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
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Vec<FileChange>, ApiError> {
    let repo = current_repo(&state, &window)?;
    blocking(move || status::read(&repo, include_ignored)).await
}

#[tauri::command]
pub async fn worktree_sides(
    path: String,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<WorktreeSides, ApiError> {
    let repo = current_repo(&state, &window)?;
    blocking_api(move || Ok(worktree_diff::sides(&repo, &path)?)).await
}

#[tauri::command]
pub async fn worktree_preview(
    path: String,
    content: String,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Vec<Hunk>, ApiError> {
    let repo = current_repo(&state, &window)?;
    blocking_api(move || Ok(worktree_diff::preview(&repo, &path, &content)?)).await
}

// Returns the hash of what was saved, which is what the next save expects.
#[tauri::command]
pub async fn file_write(
    path: String,
    content: String,
    expected_hash: String,
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<String, ApiError> {
    let repo = current_repo(&state, &window)?;
    let dir = repo_data_dir(&app, &state, &window)?;
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
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<(), ApiError> {
    diff_window_open_request(DiffRequest::Worktree { path }, app, window, state).await
}

// The same window, also for two revisions of one or more files (`compare::DiffRequest`).
#[tauri::command]
pub async fn diff_window_open_request(
    request: DiffRequest,
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<(), ApiError> {
    let repo = current_repo(&state, &window)?;
    let checked = request.clone();
    let title = blocking_api(move || {
        compare::check_request(&repo, &checked)?;
        Ok(compare::window_title(&repo, &checked))
    })
    .await?;
    let session = session_of(&state, window.label())?;
    *session.diff_request.lock().unwrap() = Some(request.clone());
    let owner = owner_label(&window);
    let notify = || match &request {
        DiffRequest::Worktree { path } => try_emit_to_owner(&app, &owner, DIFF_OPEN_EVENT, path),
        DiffRequest::Revisions { .. } => {
            try_emit_to_owner(&app, &owner, DIFF_REQUEST_EVENT, &request)
        }
    };
    let diff_label = windows::label_for(WindowKind::Diff, &owner);
    let opened = match app.get_webview_window(&diff_label) {
        Some(diff) => diff
            .set_title(&title)
            .and_then(|()| reveal(&diff))
            .and_then(|()| notify()),
        None => {
            WebviewWindowBuilder::new(&app, &diff_label, WebviewUrl::App(DIFF_WINDOW_URL.into()))
                .title(&title)
                .inner_size(1200.0, 800.0)
                .visible(false)
                .background_color(tauri::webview::Color(0x1e, 0x1f, 0x22, 0xff))
                .build()
                .map(drop)
        }
    };
    opened.map_err(|error| failed(error.to_string()))
}

// The file of a working-tree request; `None` with two revisions.
#[tauri::command]
pub fn diff_window_path(
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Option<String>, ApiError> {
    let session = session_of(&state, window.label())?;
    let request = session.diff_request.lock().unwrap();
    Ok(match request.as_ref() {
        Some(DiffRequest::Worktree { path }) => Some(path.clone()),
        _ => None,
    })
}

#[tauri::command]
pub fn diff_window_request(
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Option<DiffRequest>, ApiError> {
    let session = session_of(&state, window.label())?;
    let request = session.diff_request.lock().unwrap().clone();
    Ok(request)
}

// `multiple.file.merge.request.title`, with the full path as `presentableUrl`.
fn merge_title(root: &Path, path: &str) -> String {
    format!("Merge Revisions for {}", root.join(path).display())
}

// What the merge window shows: a file in conflict, or three texts that whoever opened it gave (a patch
// that does not apply, `ApplyPatchMergeRequest`), with no file in conflict behind them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeSession {
    Conflict(String),
    Contents(MergeContents),
}

// `ApplicationActivationListener`: whether one of Ramify's windows has the focus. Moving between them
// is not leaving the app: a window that loses the focus only makes it inactive if, a moment later,
// none of them has it and no focus event came since. `generation` counts the focus events.
#[derive(Debug)]
pub struct Activation {
    active: bool,
    generation: u64,
}

impl Default for Activation {
    fn default() -> Self {
        Self {
            active: true,
            generation: 0,
        }
    }
}

impl Activation {
    // A window gained the focus: true if the app was not active.
    pub fn gained(&mut self) -> bool {
        self.generation += 1;
        !std::mem::replace(&mut self.active, true)
    }

    // A window lost the focus: what `settled` is given once the others were asked.
    pub fn lost(&mut self) -> u64 {
        self.generation += 1;
        self.generation
    }

    // True if the app is left now: nobody gained the focus since the loss of `since`, and no
    // window has it.
    pub fn settled(&mut self, since: u64, any_focused: bool) -> bool {
        if since != self.generation || any_focused || !self.active {
            return false;
        }
        self.active = false;
        true
    }
}

pub const APP_ACTIVATED_EVENT: &str = "app-activated";
pub const APP_DEACTIVATED_EVENT: &str = "app-deactivated";
// How long the focus may take to arrive in another window of Ramify.
const FOCUS_SETTLE: Duration = Duration::from_millis(150);

// Every main window hears when the app as a whole gains and loses the focus (`emit_to` each
// existing main label).
fn emit_to_every_main(app: &AppHandle, event: &str, payload: impl Serialize + Clone) {
    for label in app.webview_windows().into_keys() {
        if windows::kind_of(&label) == Some(WindowKind::Main) {
            if let Err(error) = app.emit_to(label.as_str(), event, payload.clone()) {
                eprintln!("Could not send the event {event} to {label}: {error}");
            }
        }
    }
}

// What `app-activated` says: the project window that was activated, the owner of the window
// `label` that got the focus. Only that one looks at the clipboard for a patch on the fly
// (IntelliJ's `PatchClipboardListener` asks in the activated frame).
fn activated_owner(label: &str) -> Option<String> {
    windows::owner_of(label)
}

// The window `label` gained (`focused`) or lost the focus.
pub fn focus_changed(app: &AppHandle, label: &str, focused: bool) {
    let state = app.state::<AppState>();
    if focused {
        if state.activation.lock().unwrap().gained() {
            emit_to_every_main(app, APP_ACTIVATED_EVENT, activated_owner(label));
            refresh_recent_menu(app);
        }
        return;
    }
    let since = state.activation.lock().unwrap().lost();
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(FOCUS_SETTLE);
        let any = app
            .webview_windows()
            .values()
            .any(|window| window.is_focused().unwrap_or(false));
        let state = app.state::<AppState>();
        if state.activation.lock().unwrap().settled(since, any) {
            emit_to_every_main(&app, APP_DEACTIVATED_EVENT, ());
        }
    });
}

// The texts are shown as they are; `path` only picks the syntax highlighting, and nothing reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeContents {
    // Whoever opened it recognises its `merge-done` by it.
    pub id: String,
    pub title: String,
    pub titles: [String; 3],
    pub left: String,
    pub base: String,
    pub right: String,
    pub path: String,
    // A patch conflict: Cancel is `Abort…` and asks `Abort and Rollback`, `Skip` or `Continue
    // Resolve` (`ApplyPatchAction.applyContent`).
    #[serde(default)]
    pub abort: bool,
}

// Decision 3: with the window open, what it shows does not change. True if it has to be created.
fn claim_merge(shown: &mut Option<MergeSession>, open: bool, session: MergeSession) -> bool {
    if open {
        return false;
    }
    *shown = Some(session);
    true
}

fn session_path(shown: &Option<MergeSession>) -> Option<String> {
    match shown {
        Some(MergeSession::Conflict(path)) => Some(path.clone()),
        _ => None,
    }
}

fn session_contents(shown: &Option<MergeSession>) -> Option<MergeContents> {
    match shown {
        Some(MergeSession::Contents(contents)) => Some(contents.clone()),
        _ => None,
    }
}

// Hidden until its page calls `showWindow`.
fn build_merge_window(app: &AppHandle, owner: &str, title: &str) -> tauri::Result<()> {
    let label = windows::label_for(WindowKind::Merge, owner);
    WebviewWindowBuilder::new(app, &label, WebviewUrl::App(MERGE_WINDOW_URL.into()))
        .title(title)
        .inner_size(1200.0, 800.0)
        .visible(false)
        .background_color(tauri::webview::Color(0x1e, 0x1f, 0x22, 0xff))
        .build()
        .map(drop)
}

// A single merge window. It does not touch files: there is no need to save the diff one first. The lock
// is held while it is created, so that two consecutive requests do not open two.
#[tauri::command]
pub async fn merge_window_open(
    path: String,
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<(), ApiError> {
    merge_open_session(&state, window.label())?;
    let repo = current_repo(&state, &window)?;
    let checked = path.clone();
    let root = repo.clone();
    blocking_api(move || Ok(worktree_diff::check_inside(&root, &checked)?)).await?;
    let session = merge_open_session(&state, window.label())?;
    let mut shown = session.merge_session.lock().unwrap();
    let existing = app.get_webview_window(&windows::label_for(
        WindowKind::Merge,
        &owner_label(&window),
    ));
    let title = merge_title(&repo, &path);
    if !claim_merge(&mut shown, existing.is_some(), MergeSession::Conflict(path)) {
        return existing
            .map_or(Ok(()), |window| reveal(&window))
            .map_err(|error| failed(error.to_string()));
    }
    *session.merge_ready.lock().unwrap() = false;
    if let Err(error) = build_merge_window(&app, &owner_label(&window), &title) {
        *shown = None;
        return Err(failed(error.to_string()));
    }
    Ok(())
}

// The contents mode. With the window already open it brings it forward and refuses: whoever asked would
// wait for a `merge-done` with its id that never comes.
#[tauri::command]
pub async fn merge_window_open_contents(
    input: MergeContents,
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<(), ApiError> {
    let session = merge_open_session(&state, window.label())?;
    let mut shown = session.merge_session.lock().unwrap();
    let existing = app.get_webview_window(&windows::label_for(
        WindowKind::Merge,
        &owner_label(&window),
    ));
    let title = input.title.clone();
    if !claim_merge(
        &mut shown,
        existing.is_some(),
        MergeSession::Contents(input),
    ) {
        if let Some(merge) = existing {
            let _ = reveal(&merge);
        }
        return Err(failed("Another merge is in progress".to_string()));
    }
    *session.merge_ready.lock().unwrap() = false;
    if let Err(error) = build_merge_window(&app, &owner_label(&window), &title) {
        *shown = None;
        return Err(failed(error.to_string()));
    }
    Ok(())
}

#[tauri::command]
pub fn merge_window_path(
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Option<String>, ApiError> {
    let session = session_of(&state, window.label())?;
    let path = session_path(&session.merge_session.lock().unwrap());
    Ok(path)
}

#[tauri::command]
pub fn merge_window_contents(
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Option<MergeContents>, ApiError> {
    let session = session_of(&state, window.label())?;
    let contents = session_contents(&session.merge_session.lock().unwrap());
    Ok(contents)
}

// `MergeResult`, with the side's name as the user sees it. `Abort` and `Skip` are the answers of a
// patch conflict's `Abort…` (`ApplyPatchStatus.ABORT` and `SKIP`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MergeResult {
    Resolved,
    Yours,
    Theirs,
    Cancelled,
    Abort,
    Skip,
}

// A conflict arrives with its path. The contents mode, with its id and the text of the result, which
// whoever opened it writes: the window saves nothing.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum MergeDone {
    Conflict {
        path: String,
        result: MergeResult,
    },
    Contents {
        id: String,
        result: MergeResult,
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<String>,
    },
}

// A cancelled merge has no result text.
fn merge_done(session: MergeSession, result: MergeResult, content: Option<String>) -> MergeDone {
    match session {
        MergeSession::Conflict(path) => MergeDone::Conflict { path, result },
        MergeSession::Contents(contents) => MergeDone::Contents {
            id: contents.id,
            result,
            content: content.filter(|_| {
                matches!(
                    result,
                    MergeResult::Resolved | MergeResult::Yours | MergeResult::Theirs
                )
            }),
        },
    }
}

// The window finishes: the main one receives how, and the window is destroyed.
#[tauri::command]
pub async fn merge_window_done(
    result: MergeResult,
    content: Option<String>,
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<(), ApiError> {
    let session = session_of(&state, window.label())?;
    let mut shown = session.merge_session.lock().unwrap();
    if let Some(merge) = shown.take() {
        let done = merge_done(merge, result, content);
        emit_to_owner(&app, &owner_label(&window), MERGE_DONE_EVENT, done);
    }
    *session.merge_ready.lock().unwrap() = false;
    if let Some(merge) = app.get_webview_window(&windows::label_for(
        WindowKind::Merge,
        &owner_label(&window),
    )) {
        merge.destroy().map_err(|error| failed(error.to_string()))?;
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
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<WorkingFile, ApiError> {
    let repo = current_repo(&state, &window)?;
    blocking_api(move || working_file(&repo, &path)).await
}

// `Show in Finder`, with the file selected. Refused if the path is not absolute.
#[tauri::command]
pub async fn reveal_in_finder(path: String) -> Result<(), ApiError> {
    blocking_api(move || remotes::reveal_in_finder(Path::new(&path)).map_err(failed)).await
}

// The repository's own data (changelists, copies) lives in the app's data folder,
// not in the repository.
pub(crate) fn repo_data_dir(
    app: &AppHandle,
    state: &State<'_, AppState>,
    window: &WebviewWindow,
) -> Result<PathBuf, ApiError> {
    data_dir_of(app, &current_repo(state, window)?)
}

pub(crate) fn data_dir_of(app: &AppHandle, repo: &Path) -> Result<PathBuf, ApiError> {
    let base = app
        .path()
        .app_data_dir()
        .map_err(|error| failed(error.to_string()))?;
    Ok(storage::repo_dir(&base, repo))
}

// If the file exists but cannot be read we do not continue: saving over it would lose its data.
fn load_changelists(dir: &Path) -> Result<Changelists, ApiError> {
    Changelists::try_load(dir)
        .map_err(|error| failed(format!("Could not read the changelists: {error}")))
}

// Reading, changing and saving the changelists do not cross: the savers edit them from their task,
// beside the synchronous commands.
static CHANGELISTS_LOCK: Mutex<()> = Mutex::new(());

fn changelists_lock() -> std::sync::MutexGuard<'static, ()> {
    CHANGELISTS_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

// Reconciles with the files that have changes and saves only if something changed.
pub fn read_changelists(dir: &Path, paths: &[String]) -> Result<Changelists, ApiError> {
    let _held = changelists_lock();
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
    let _held = changelists_lock();
    let mut lists = load_changelists(dir)?;
    edit(&mut lists)?;
    lists.save(dir).map_err(|error| failed(error.to_string()))?;
    Ok(lists)
}

// `read_changelists` holds `CHANGELISTS_LOCK`: reading, changing and saving do not cross the edits of
// another command or of a saver's task.
#[tauri::command]
pub fn changelists_get(
    paths: Vec<String>,
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Changelists, ApiError> {
    read_changelists(&repo_data_dir(&app, &state, &window)?, &paths)
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
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Changelists, ApiError> {
    let repo = current_repo(&state, &window)?;
    import_changelists(&repo, &repo_data_dir(&app, &state, &window)?)
}

#[tauri::command]
pub fn changelists_create(
    name: String,
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Changelists, ApiError> {
    edit_changelists(&repo_data_dir(&app, &state, &window)?, |lists| {
        lists.create(&name).map(drop)
    })
}

#[tauri::command]
pub fn changelists_rename(
    id: String,
    name: String,
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Changelists, ApiError> {
    edit_changelists(&repo_data_dir(&app, &state, &window)?, |lists| {
        lists.rename(&id, &name)
    })
}

// The Commit Changes dialog saves its draft once its commit has finished, after it has closed: by then
// another repository may be open, whose lists (the default one included) are not the ones it wrote for.
pub fn set_list_comment(
    open: &Path,
    expected: &str,
    dir: &Path,
    id: &str,
    comment: &str,
) -> Result<Changelists, ApiError> {
    if open != Path::new(expected) {
        return Err(failed(
            "Another repository was opened before the changelist comment was saved".to_string(),
        ));
    }
    edit_changelists(dir, |lists| lists.set_comment(id, comment))
}

// Only the comment of the list: it writes neither the index nor the work tree, so it needs no
// `flush_diff_window` or `RepoLocks`. `repo` is the repository the comment was written for.
#[tauri::command]
pub fn changelists_set_comment(
    id: String,
    comment: String,
    repo: String,
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Changelists, ApiError> {
    let open = current_repo(&state, &window)?;
    set_list_comment(&open, &repo, &data_dir_of(&app, &open)?, &id, &comment)
}

#[tauri::command]
pub fn changelists_remove(
    id: String,
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Changelists, ApiError> {
    edit_changelists(&repo_data_dir(&app, &state, &window)?, |lists| {
        lists.remove(&id)
    })
}

#[tauri::command]
pub fn changelists_set_active(
    id: String,
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Changelists, ApiError> {
    edit_changelists(&repo_data_dir(&app, &state, &window)?, |lists| {
        lists.set_active(&id)
    })
}

#[tauri::command]
pub fn changelists_move(
    paths: Vec<String>,
    id: String,
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Changelists, ApiError> {
    edit_changelists(&repo_data_dir(&app, &state, &window)?, |lists| {
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
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<String, ApiError> {
    flush_diff_window(&app, &state, &window, SAVE_ACTION).await?;
    let repo = current_repo(&state, &window)?;
    let dir = repo_data_dir(&app, &state, &window)?;
    let locks = state.operations.clone();
    blocking_api(move || rollback_locked(&locks, &repo, &dir, &paths, delete_added)).await
}

// The file's hunks against HEAD; empty if it is treated as a single piece (binary or very large).
#[tauri::command]
pub async fn file_hunks(
    path: String,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Vec<Hunk>, ApiError> {
    let repo = current_repo(&state, &window)?;
    blocking_api(move || Ok(hunks::file_hunks(&repo, &path)?)).await
}

// Undoes the chosen lines of the file, after saving a copy; returns its identifier.
#[tauri::command]
pub async fn lines_rollback(
    path: String,
    selection: Vec<LineSelection>,
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<String, ApiError> {
    flush_diff_window(&app, &state, &window, SAVE_ACTION).await?;
    let repo = current_repo(&state, &window)?;
    let dir = repo_data_dir(&app, &state, &window)?;
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
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Changelists, ApiError> {
    let repo = current_repo(&state, &window)?;
    move_hunks_in(
        &repo,
        &repo_data_dir(&app, &state, &window)?,
        &path,
        &hunks,
        &id,
    )
}

// Returns the commit hash. The stage of what was not selected stays as it was, and everything goes back
// to its previous state if the commit fails.
#[tauri::command]
pub async fn commit_changes(
    request: CommitRequest,
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<String, ApiError> {
    flush_diff_window(&app, &state, &window, SAVE_ACTION).await?;
    let repo = current_repo(&state, &window)?;
    let dir = repo_data_dir(&app, &state, &window)?;
    let locks = state.operations.clone();
    blocking_api(move || commit_locked(&locks, &repo, &dir, &request)).await
}

// Like `commit_changes`, with the selection of hunks or lines of the partial files instead of their
// content.
#[tauri::command]
pub async fn commit_changes_selected(
    request: CommitRequest,
    selections: Vec<FileSelection>,
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<String, ApiError> {
    flush_diff_window(&app, &state, &window, SAVE_ACTION).await?;
    let repo = current_repo(&state, &window)?;
    let dir = repo_data_dir(&app, &state, &window)?;
    let locks = state.operations.clone();
    blocking_api(move || commit_selected_locked(&locks, &repo, &dir, request, &selections)).await
}

// The lock, the repository and the folder of its data.
type Operation = (Arc<RepoLocks>, PathBuf, PathBuf);

fn staging_context(
    app: &AppHandle,
    state: &State<'_, AppState>,
    window: &WebviewWindow,
) -> Result<Operation, ApiError> {
    Ok((
        state.operations.clone(),
        current_repo(state, window)?,
        repo_data_dir(app, state, window)?,
    ))
}

#[tauri::command]
pub async fn staging_stage(
    paths: Vec<String>,
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Option<SnapshotId>, ApiError> {
    flush_diff_window(&app, &state, &window, SAVE_ACTION).await?;
    let (locks, repo, dir) = staging_context(&app, &state, &window)?;
    blocking_api(move || stage_locked(&locks, &repo, &dir, &paths)).await
}

#[tauri::command]
pub async fn staging_unstage(
    paths: Vec<String>,
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Option<SnapshotId>, ApiError> {
    flush_diff_window(&app, &state, &window, SAVE_ACTION).await?;
    let (locks, repo, dir) = staging_context(&app, &state, &window)?;
    blocking_api(move || unstage_locked(&locks, &repo, &dir, &paths)).await
}

#[tauri::command]
pub async fn staging_revert(
    paths: Vec<String>,
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Option<SnapshotId>, ApiError> {
    flush_diff_window(&app, &state, &window, SAVE_ACTION).await?;
    let (locks, repo, dir) = staging_context(&app, &state, &window)?;
    blocking_api(move || revert_locked(&locks, &repo, &dir, &paths)).await
}

#[tauri::command]
pub async fn staging_stage_hunks(
    path: String,
    selection: Vec<LineSelection>,
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Option<SnapshotId>, ApiError> {
    flush_diff_window(&app, &state, &window, SAVE_ACTION).await?;
    let (locks, repo, dir) = staging_context(&app, &state, &window)?;
    blocking_api(move || stage_hunks_locked(&locks, &repo, &dir, &path, &selection)).await
}

#[tauri::command]
pub async fn staging_unstage_hunks(
    path: String,
    selection: Vec<LineSelection>,
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Option<SnapshotId>, ApiError> {
    flush_diff_window(&app, &state, &window, SAVE_ACTION).await?;
    let (locks, repo, dir) = staging_context(&app, &state, &window)?;
    blocking_api(move || unstage_hunks_locked(&locks, &repo, &dir, &path, &selection)).await
}

#[tauri::command]
pub async fn staging_sides(
    path: String,
    comparison: Comparison,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<WorktreeSides, ApiError> {
    let repo = current_repo(&state, &window)?;
    blocking_api(move || Ok(worktree_diff::stage_sides(&repo, &path, comparison)?)).await
}

#[tauri::command]
pub async fn staging_preview(
    path: String,
    content: String,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Vec<Hunk>, ApiError> {
    let repo = current_repo(&state, &window)?;
    blocking_api(move || Ok(worktree_diff::stage_preview(&repo, &path, &content)?)).await
}

// The index as it is. Returns the commit hash.
#[tauri::command]
pub async fn commit_staged(
    options: CommitOptions,
    message: String,
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<String, ApiError> {
    flush_diff_window(&app, &state, &window, SAVE_ACTION).await?;
    let (locks, repo, dir) = staging_context(&app, &state, &window)?;
    blocking_api(move || commit_staged_locked(&locks, &repo, &dir, &options, &message)).await
}

#[tauri::command]
pub async fn commit_warnings(
    request: CommitRequest,
    selections: Vec<FileSelection>,
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Vec<CommitWarning>, ApiError> {
    let repo = current_repo(&state, &window)?;
    let settings = current_settings(&app)?;
    blocking_api(move || Ok(warnings_for(&repo, request, &selections, &settings))).await
}

#[tauri::command]
pub async fn commit_warnings_staged(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Vec<CommitWarning>, ApiError> {
    let repo = current_repo(&state, &window)?;
    let settings = current_settings(&app)?;
    blocking_api(move || Ok(commit::staged_warnings(&repo, &settings))).await
}

#[tauri::command]
pub async fn commit_fix_autocrlf(
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<(), ApiError> {
    let repo = current_repo(&state, &window)?;
    blocking(move || commit::fix_autocrlf(&repo)).await
}

// The settings belong to the app, not to a repository.
pub(crate) fn base_dir(app: &AppHandle) -> Result<PathBuf, ApiError> {
    app.path()
        .app_data_dir()
        .map_err(|error| failed(error.to_string()))
}

pub(crate) fn current_settings(app: &AppHandle) -> Result<Settings, ApiError> {
    Ok(settings::load(&base_dir(app)?))
}

fn apply_settings(app: &AppHandle, settings: &Settings) {
    git::set_binary(settings.git_path.as_deref());
    if let Err(error) = sync_patch_menu(app, settings.effective_staging()) {
        eprintln!("Could not update the Patch menu: {error}");
    }
    if let Err(error) = sync_local_changes_menu(app, settings.effective_staging()) {
        eprintln!("Could not update the Uncommitted Changes menu: {error}");
    }
    app.state::<AppState>()
        .tasks
        .set_credential_helper(settings.use_credential_helper);
    app.state::<AppState>()
        .tasks
        .set_staging_area(settings.effective_staging());
    app.state::<AppState>()
        .tasks
        .set_cherry_pick_suffix(settings.cherry_pick_suffix);
    app.state::<AppState>()
        .hosting
        .set_connect_timeout(settings.github_connection_timeout_ms);
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

// File > Open…: the chooser is shown by the last focused main window, brought to the front first.
pub fn open_project_chooser(app: &AppHandle) {
    let label = last_main_label(app);
    if let Some(window) = app.get_webview_window(&label) {
        let _ = reveal(&window);
        resume_and_notify(app, &label);
        emit_to_owner(app, &label, OPEN_PROJECT_CHOOSER_EVENT, ());
    }
}

// The main window may be hidden, and the diff one in front.
pub fn open_settings(app: &AppHandle) {
    let label = last_main_label(app);
    if let Some(window) = app.get_webview_window(&label) {
        let _ = reveal(&window);
        resume_and_notify(app, &label);
        emit_to_owner(app, &label, OPEN_SETTINGS_EVENT, ());
    }
}

// The Dock icon clicked (`RunEvent::Reopen`): the last focused main window comes back, hidden or
// not, and watches its repository again.
pub fn show_last_window(app: &AppHandle) {
    let label = last_main_label(app);
    if let Some(window) = app.get_webview_window(&label) {
        let _ = reveal(&window);
    }
    resume_and_notify(app, &label);
}

#[tauri::command]
pub fn messages_list(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Vec<String>, ApiError> {
    Ok(messages::list(&repo_data_dir(&app, &state, &window)?))
}

// `LAST_COMMIT_MESSAGE`, the last resort of the Commit Changes dialog's message.
#[tauri::command]
pub fn messages_last(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Option<String>, ApiError> {
    Ok(messages::last_commit_message(&repo_data_dir(
        &app, &state, &window,
    )?))
}

// `VcsConfiguration.saveCommitMessage`: the text written before amend loads the last commit's
// message goes to the history (and becomes `LAST_COMMIT_MESSAGE`). It writes only the app's data.
#[tauri::command]
pub fn messages_push(
    message: String,
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<(), ApiError> {
    messages::push(&repo_data_dir(&app, &state, &window)?, &message)
        .map_err(|error| failed(error.to_string()))
}

// Whether `Run Git hooks` is offered. It reads: it changes nothing.
#[tauri::command]
pub async fn repo_has_commit_hooks(
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<bool, ApiError> {
    let repo = current_repo(&state, &window)?;
    blocking(move || Ok(commit::has_commit_hooks(&repo))).await
}

// The whole message of HEAD, for the Amend checkbox.
pub fn last_message(repo: &Path) -> Result<String, GitError> {
    let [no, signatures] = history::NO_SIGNATURES;
    let output = git::run(repo, &[no, signatures, "log", "-1", "--format=%B"])?;
    Ok(String::from_utf8_lossy(&output).trim_end().to_string())
}

#[tauri::command]
pub async fn head_message(
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<String, ApiError> {
    let repo = current_repo(&state, &window)?;
    blocking(move || last_message(&repo)).await
}

#[tauri::command]
pub fn snapshots_list(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Vec<Snapshot>, ApiError> {
    Ok(snapshots::list(&repo_data_dir(&app, &state, &window)?))
}

// Ramify's own: a copy offered by a notification of another repository is not looked for in the
// open one, where it is not.
fn restorable_here(open: &Path, expected_repo: Option<&str>) -> Result<(), ApiError> {
    match expected_repo {
        Some(expected) if Path::new(expected) != open => Err(failed(format!(
            "The copy belongs to {expected}: open that repository to restore it"
        ))),
        _ => Ok(()),
    }
}

// Returns the identifier of the copy of the state that existed just before restoring.
// `expected_repo`: the repository the copy was taken in, when the caller knows it.
#[tauri::command]
pub async fn snapshots_restore(
    id: String,
    expected_repo: Option<String>,
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<String, ApiError> {
    let repo = current_repo(&state, &window)?;
    restorable_here(&repo, expected_repo.as_deref())?;
    let dir = repo_data_dir(&app, &state, &window)?;
    let locks = state.operations.clone();
    blocking_api(move || restore_locked(&locks, &repo, &dir, &id)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{in_task, TempRepo};
    use std::ffi::{OsStr, OsString};
    use std::os::unix::ffi::OsStrExt;
    use std::sync::mpsc;
    use std::time::Duration;
    use tempfile::TempDir;

    #[test]
    fn emit_targets_are_the_owner_group() {
        let group = [
            "main-2",
            "diff-2",
            "merge-2",
            "viewer-2",
            "selection-history-2",
        ];
        assert_eq!(emit_targets("main-2", |_| true), group);
        // Only the windows that exist, and never another project window's.
        assert_eq!(
            emit_targets("main-2", |label| label == "diff-2" || label == "diff"),
            ["diff-2"]
        );
        // With no window at all there is nothing to send to, and nothing fails.
        assert!(emit_targets("main-2", |_| false).is_empty());
        assert_eq!(
            emit_targets("main", |_| true),
            ["main", "diff", "merge", "viewer", "selection-history"]
        );
    }

    #[test]
    fn opening_a_repository_closes_only_its_own_auxiliary_windows() {
        assert_eq!(
            aux_labels("main-2"),
            ["diff-2", "merge-2", "viewer-2", "selection-history-2"]
        );
        assert_eq!(
            aux_labels("main"),
            ["diff", "merge", "viewer", "selection-history"]
        );
    }

    #[test]
    fn closing_the_last_window_hides_the_selection_history_too() {
        assert_eq!(
            close_labels("main-2"),
            [
                "diff-2",
                "merge-2",
                "viewer-2",
                "selection-history-2",
                "main-2"
            ]
        );
        assert!(close_labels("main").contains(&"selection-history".to_string()));
    }

    // Each project window reads its own repository; an auxiliary window reads its owner's.
    #[test]
    fn two_sessions_keep_their_repository() {
        let first = TempRepo::new();
        let second = TempRepo::new();
        let state = AppState::default();
        for (label, repo) in [("main", &first), ("main-2", &second)] {
            *session_of(&state, label).unwrap().repo.lock().unwrap() = Some(RepoSession {
                path: repo.path().to_path_buf(),
            });
        }

        assert!(Arc::ptr_eq(
            &session_of(&state, "diff-2").unwrap(),
            &session_of(&state, "main-2").unwrap()
        ));
        assert_eq!(repo_of(&state, "diff-2").unwrap(), second.path());
        assert_eq!(repo_of(&state, "main").unwrap(), first.path());
        assert_eq!(repo_of(&state, "merge").unwrap(), first.path());
    }

    // Review Focus 1: a write in one window asks only that window's diff window to save.
    #[test]
    fn a_write_flushes_only_its_own_diff_window() {
        let state = AppState::default();
        let first = session_of(&state, "main").unwrap();
        let second = session_of(&state, "main-2").unwrap();

        // Its diff window does not exist: nothing is asked and nothing is queued.
        let missing = flush_request(&state, "main-2", |_| false).unwrap();
        assert!(missing.is_none());
        assert!(second.diff_flush.lock().unwrap().is_empty());

        let mut asked = Vec::new();
        let (target, _answer) = flush_request(&state, "main-2", |label| {
            asked.push(label.to_string());
            true
        })
        .unwrap()
        .unwrap();

        assert_eq!(target, "diff-2");
        assert_eq!(asked, ["diff-2"]);
        assert_eq!(second.diff_flush.lock().unwrap().len(), 1);
        assert!(first.diff_flush.lock().unwrap().is_empty());
        // Window `main` still asks its own `diff`.
        let (own, _kept) = flush_request(&state, "main", |_| true).unwrap().unwrap();
        assert_eq!(own, "diff");
        assert_eq!(first.diff_flush.lock().unwrap().len(), 1);
        assert_eq!(second.diff_flush.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_label_without_owner_has_no_session() {
        let state = AppState::default();

        assert!(session_of(&state, "other").is_err());
        assert!(state.sessions.lock().unwrap().is_empty());
    }

    // `SHELF_TOPIC`: `repo-changed` says the shelf changed only after a write to the open
    // repository's store, once; a worktree save does not make the Shelf tab read its lists again.
    #[test]
    fn repo_changed_says_the_shelf_changed_only_after_a_write_to_its_store() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"one\n", "a");
        let data = TempDir::new().unwrap();
        let mut seen = BTreeMap::new();

        assert!(!shelf_moved(&mut seen, data.path()));
        std::fs::write(repo.path().join("a.txt"), b"two\n").unwrap();
        assert!(!shelf_moved(&mut seen, data.path()));

        let patch = crate::patch::Patch::default();
        shelf::create(data.path(), repo.path(), "Work", &patch, &[], false).unwrap();
        assert!(shelf_moved(&mut seen, data.path()));
        assert!(!shelf_moved(&mut seen, data.path()));
    }

    // P16: a copy asked for by the repository it was taken in is not looked for in another one:
    // the user is told whose it is, and nothing is written.
    #[test]
    fn a_copy_of_another_repository_says_whose_it_is() {
        let open = Path::new("/repos/b");

        assert!(restorable_here(open, None).is_ok());
        assert!(restorable_here(open, Some("/repos/b")).is_ok());
        let error = restorable_here(open, Some("/repos/a")).unwrap_err();
        assert_eq!(
            error.message,
            "The copy belongs to /repos/a: open that repository to restore it"
        );
    }

    // P9: one thread for every open. It cleans the repository just opened at once, then once a
    // period, and a repository opened meanwhile joins the first one instead of replacing it.
    #[test]
    fn one_shelf_cleaner_cleans_every_open_repository_on_open_and_then_once_a_period() {
        let (open, opened) = mpsc::channel::<String>();
        let (cleaned_tx, cleaned) = mpsc::channel::<String>();
        let owners = Arc::new(Mutex::new(vec!["main".to_string()]));
        let listed = owners.clone();
        let cleaner = thread::spawn(move || {
            run_shelf_cleaner(
                opened,
                Duration::from_millis(150),
                move || listed.lock().unwrap().clone(),
                |owner| cleaned_tx.send(owner.to_string()).unwrap(),
            )
        });
        let next = || cleaned.recv_timeout(Duration::from_secs(5)).unwrap();

        open.send("main".to_string()).unwrap();
        assert_eq!(next(), "main");
        assert_eq!(next(), "main");
        owners.lock().unwrap().push("main-2".to_string());
        open.send("main-2".to_string()).unwrap();
        // Both are cleaned in the periods that follow.
        let mut seen = HashSet::new();
        while seen.len() < 2 {
            seen.insert(next());
        }
        assert_eq!(
            seen,
            HashSet::from(["main".to_string(), "main-2".to_string()])
        );

        drop(open);
        cleaner.join().unwrap();
    }

    // The folders the cleaner serves: one per open window, added on open and removed on close.
    #[test]
    fn the_shelf_cleaner_keeps_every_open_repository() {
        let folders = ShelfFolders::default();
        folders.open("main", PathBuf::from("/data/a"));
        folders.open("main-2", PathBuf::from("/data/b"));
        assert_eq!(
            folders.dirs(),
            [PathBuf::from("/data/a"), PathBuf::from("/data/b")]
        );

        folders.close("main");
        assert_eq!(folders.dirs(), [PathBuf::from("/data/b")]);
        assert_eq!(folders.dir_of("main"), None);
        assert_eq!(folders.dir_of("main-2"), Some(PathBuf::from("/data/b")));

        // A window that opens another repository (This Window) takes its old one's place.
        folders.open("main-2", PathBuf::from("/data/c"));
        assert_eq!(folders.dirs(), [PathBuf::from("/data/c")]);
        folders.close("main-2");
        assert!(folders.dirs().is_empty());
    }

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

    // With signed commits and `log.showSignature` on, the signature check stays out of the message.
    #[test]
    fn last_commit_message_without_the_signature_check() {
        let repo = TempRepo::new();
        repo.sign_commits();
        repo.commit("a.txt", b"one\n", "subject\n\nbody");
        assert!(repo
            .git(&["log", "-1", "--format=%B"])
            .contains("No signature"));

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
    fn only_the_main_windows_stop_the_watcher() {
        assert!(owns_watcher(MAIN_WINDOW));
        assert!(owns_watcher("main-2"));
        assert!(!owns_watcher(DIFF_WINDOW));
        assert!(!owns_watcher("diff-2"));
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

    // The saver edits the changelists from its task, beside the commands: no edit is lost.
    #[test]
    fn changelist_edits_from_several_threads_are_not_lost() {
        let dir = TempDir::new().unwrap();
        let threads: Vec<_> = (0..8)
            .map(|thread| {
                let dir = dir.path().to_path_buf();
                std::thread::spawn(move || {
                    for n in 0..10 {
                        edit_changelists(&dir, |lists| {
                            lists.create(&format!("list {thread} {n}")).map(|_| ())
                        })
                        .unwrap();
                        read_changelists(&dir, &[]).unwrap();
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }

        assert_eq!(Changelists::load(dir.path()).lists.len(), 81);
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
        let saved = saved.unwrap().unwrap();
        // What an earlier build recorded: this one records nothing (decision 10).
        preserve::record_earlier_build(dir.path(), &saved.sha, "Update", &saved.message);
        let mut info = repo_info(repo.path()).unwrap();

        find_pending(&mut info, repo.path(), dir.path());

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
        let popped = crate::test_support::git_command()
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
        let merged = crate::test_support::git_command()
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
    // so as not to continue if it could not be saved. Every command saves its own window's diff.
    #[test]
    fn whatever_touches_the_repository_saves_the_diff_window_first() {
        let own = "flush_diff_window(&app, &state, &window, SAVE_ACTION).await?;";
        let sources = [
            (include_str!("commands.rs"), own),
            (include_str!("vcs_commands.rs"), own),
            (include_str!("get_version.rs"), own),
            (include_str!("hosting/commands.rs"), own),
        ];
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
            "changes_add",
            "stash_save",
            "stash_push_paths",
            "stash_unstash",
            "stash_drop",
            "stash_clear",
            "patch_plan",
            "patch_apply",
            "patch_conflict_resolve",
            "patch_rollback",
            "patch_create",
            "patch_preview",
            "shelf_shelve",
            "shelf_shelve_silently",
            "shelf_delete",
            "shelf_undo_delete",
            "shelf_restore",
            "shelf_rename",
            "shelf_recycle",
            "shelf_import_patches",
            "shelf_import_idea",
            "shelf_import_to_shelf",
            "shelf_patch_create",
            "revision_get_check",
            "revision_get",
            "github_pr_checkout",
            "github_pr_show_in_log",
            "gpg_config_set",
        ] {
            let header = format!("pub async fn {name}(");
            let (source, expected) = sources
                .iter()
                .find(|(source, _)| source.contains(&header))
                .unwrap();
            let start = source.find(&header).unwrap();
            let mut lines = source[start..].lines();
            lines.find(|line| line.ends_with(" {")).unwrap();
            let first = lines.next().unwrap().trim();
            assert_eq!(first, *expected, "{name}");
        }
    }

    #[test]
    fn the_menu_quit_item_is_recognized() {
        assert!(is_quit_item("Quit Ramify"));
        assert!(!is_quit_item("Hide Ramify"));
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

    // Review Focus 3: the root through a symbolic link, or with a trailing slash, is the one
    // another window has open; the calling window's own session does not count.
    #[test]
    fn focus_open_matches_canonical_roots() {
        let repo = TempRepo::new();
        let other = TempRepo::new();
        let links = TempDir::new().unwrap();
        let link = links.path().join("link");
        std::os::unix::fs::symlink(repo.path(), &link).unwrap();
        let state = AppState::default();
        for (label, path) in [("main", other.path()), ("main-2", repo.path())] {
            *session_of(&state, label).unwrap().repo.lock().unwrap() = Some(RepoSession {
                path: path.to_path_buf(),
            });
        }
        // A session without a repository holds nothing.
        session_of(&state, "main-3").unwrap();

        let open = claimed_roots(&state);
        let slash = PathBuf::from(format!("{}/", repo.path().display()));
        let canonical = fs::canonicalize(repo.path()).unwrap();
        for root in [repo.path().to_path_buf(), link.clone(), slash, canonical] {
            let found = window_with_root(&open, &root, "main");
            assert_eq!(found.as_deref(), Some("main-2"), "{root:?}");
            assert_eq!(window_with_root(&open, &root, "main-2"), None, "{root:?}");
            let found = window_with_root(&open, &root, "main-3");
            assert_eq!(found.as_deref(), Some("main-2"), "{root:?}");
        }
        assert_eq!(window_with_root(&open, links.path(), "main"), None);
        assert_eq!(
            window_with_root(&open, &links.path().join("missing"), "main"),
            None
        );
    }

    // A window built for a root holds it only as its initial target until its page opens it: in
    // that gap the root is already open there.
    #[test]
    fn a_window_still_loading_its_target_counts_as_open() {
        let repo = TempRepo::new();
        let links = TempDir::new().unwrap();
        let link = links.path().join("link");
        std::os::unix::fs::symlink(repo.path(), &link).unwrap();
        let state = AppState::default();
        let root = repo.path().display().to_string();
        state
            .initial_targets
            .lock()
            .unwrap()
            .store("main-2", root.clone());
        session_of(&state, "main-2").unwrap();

        let found = claimed_by(&state, &link, "main", |_| true);
        assert_eq!(found.as_deref(), Some("main-2"));
        // A window that is gone holds nothing.
        assert_eq!(
            claimed_by(&state, &link, "main", |label| label != "main-2"),
            None
        );

        // Once its first open is over, its repository (or nothing) is what counts.
        let claim = claim_opening(&state, "main-2", repo.path(), |_| true).unwrap();
        drop(claim);
        assert_eq!(claimed_by(&state, &link, "main", |_| true), None);
    }

    #[test]
    fn a_new_window_is_not_built_for_a_root_already_loading() {
        let repo = TempRepo::new();
        let other = TempRepo::new();
        let state = AppState::default();
        let taken = HashSet::from(["main".to_string()]);
        let root = repo.path().display().to_string();

        let first = plan_new_window(&state, &root, &taken, |_| true);
        assert_eq!(first, NewWindowPlan::Build("main-2".to_string()));
        let slash = format!("{root}/");
        let second = plan_new_window(&state, &slash, &taken, |_| true);
        assert_eq!(second, NewWindowPlan::Existing("main-2".to_string()));

        let elsewhere = other.path().display().to_string();
        let third = plan_new_window(&state, &elsewhere, &taken, |_| true);
        assert_eq!(third, NewWindowPlan::Build("main-3".to_string()));
        let mut targets = state.initial_targets.lock().unwrap();
        assert_eq!(targets.target_for("main-3", || None), Some(elsewhere));
    }

    // Two This Window opens of one root in two windows: the second finds the first opening it.
    #[test]
    fn two_opens_of_one_root_cannot_both_proceed() {
        let repo = TempRepo::new();
        let state = AppState::default();
        let slash = PathBuf::from(format!("{}/", repo.path().display()));

        let first = claim_opening(&state, "main", repo.path(), |_| true).unwrap();
        let second = claim_opening(&state, "main-2", &slash, |_| true);
        assert_eq!(second.err().as_deref(), Some("main"));

        // Whatever way the open ends, the claim goes with it.
        drop(first);
        assert!(session_of(&state, "main")
            .unwrap()
            .opening
            .lock()
            .unwrap()
            .is_none());
        let second = claim_opening(&state, "main-2", &slash, |_| true).unwrap();
        assert_eq!(
            claim_opening(&state, "main", repo.path(), |_| true)
                .err()
                .as_deref(),
            Some("main-2")
        );
        drop(second);
    }

    // The body of the function `signature` starts in `source`, up to its closing brace.
    fn body_of<'a>(source: &'a str, signature: &str) -> &'a str {
        let start = source.find(signature).unwrap();
        let end = source[start..].find("\n}\n").unwrap();
        &source[start..start + end]
    }

    // Final review: a window brought to the front may have been hidden as the last one, with its
    // watcher stopped: it watches again and reads its state, as the Dock icon does.
    #[test]
    fn focusing_a_project_window_watches_it_again() {
        let body = body_of(include_str!("commands.rs"), "fn focus_main(");
        let reveal = body.find("reveal(&window)").unwrap();
        let resume = body.find("resume_and_notify(app, label);").unwrap();
        assert!(reveal < resume, "{body}");
    }

    // Final review: New Window gets a path from the page; Rust resolves it to the repository
    // root before checking the claims and storing it as the new window's target.
    #[test]
    fn a_new_window_resolves_its_root_first() {
        let body = body_of(
            include_str!("commands.rs"),
            "pub async fn project_new_window(",
        );
        let mut lines = body.lines().skip_while(|line| !line.ends_with(" {"));
        lines.next();
        assert_eq!(
            lines.next().unwrap().trim(),
            "let root = blocking_api(move || resolve_project(Path::new(&root))).await?;"
        );
        assert_eq!(
            lines.next().unwrap().trim(),
            "new_main_window(&app, &state, &root)"
        );
    }

    #[test]
    fn resolving_a_project_gives_its_root() {
        let repo = TempRepo::new();
        repo.commit("sub/deep/a.txt", b"1", "one");
        let links = TempDir::new().unwrap();
        let link = links.path().join("link");
        std::os::unix::fs::symlink(repo.path(), &link).unwrap();

        for target in [
            repo.path().to_path_buf(),
            repo.path().join("sub/deep"),
            PathBuf::from(format!("{}/", repo.path().display())),
            link.join("sub"),
        ] {
            assert_eq!(
                resolve_project(&target).unwrap(),
                root_of(&repo),
                "{target:?}"
            );
        }
        let outside = TempDir::new().unwrap();
        let error = resolve_project(outside.path()).unwrap_err();
        assert_eq!(error.kind, "not_a_repository");
        assert_eq!(
            error.message,
            ApiError::from(repo_info(outside.path()).unwrap_err()).message
        );
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
            OsString::from("ramify"),
            OsStr::from_bytes(b"/repo/\xff").to_os_string(),
        ];

        let first = first_argument(args.into_iter()).unwrap();

        assert_eq!(first, "/repo/\u{fffd}");
    }

    #[test]
    fn without_arguments_there_is_no_initial_repository() {
        assert_eq!(first_argument([OsString::from("ramify")].into_iter()), None);
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
        *main_session(&state).repo.lock().unwrap() = Some(RepoSession {
            path: repo.path().to_path_buf(),
        });
        let (sender, receiver) = mpsc::channel();
        resume_watching(&state, MAIN_WINDOW, move |_| {
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
        assert!(main_session(&state).watcher.lock().unwrap().is_some());

        stop_watching(&state, MAIN_WINDOW);
        fs::write(repo.path().join("a.txt"), b"hello").unwrap();

        assert!(main_session(&state).watcher.lock().unwrap().is_none());
        assert!(receiver.recv_timeout(Duration::from_millis(800)).is_err());
    }

    #[test]
    fn reopening_restarts_the_watcher() {
        let repo = TempRepo::new();
        let (state, _receiver) = watching_state(&repo);
        stop_watching(&state, MAIN_WINDOW);

        let (sender, receiver) = mpsc::channel();
        resume_watching(&state, MAIN_WINDOW, move |_| {
            let _ = sender.send(());
        })
        .unwrap();
        assert!(main_session(&state).watcher.lock().unwrap().is_some());
        while receiver.recv_timeout(Duration::from_millis(500)).is_ok() {}
        fs::write(repo.path().join("a.txt"), b"hello").unwrap();

        assert!(receiver.recv_timeout(Duration::from_secs(2)).is_ok());
    }

    #[test]
    fn reopening_without_repository_does_not_watch() {
        let state = AppState::default();
        resume_watching(&state, MAIN_WINDOW, |_| {}).unwrap();
        assert!(main_session(&state).watcher.lock().unwrap().is_none());
    }

    #[test]
    fn reopening_with_an_active_watcher_does_not_duplicate_it() {
        let repo = TempRepo::new();
        let (state, receiver) = watching_state(&repo);

        resume_watching(&state, MAIN_WINDOW, |_| {
            panic!("must not create another watcher")
        })
        .unwrap();
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

    #[test]
    fn a_list_comment_for_another_repository_is_not_saved() {
        let dir = TempDir::new().unwrap();
        let open = Path::new("/repos/b");

        let error =
            set_list_comment(open, "/repos/a", dir.path(), "default", "Draft of a").unwrap_err();
        let saved =
            set_list_comment(open, "/repos/b", dir.path(), "default", "Draft of b").unwrap();

        assert_eq!(error.kind, "failed");
        assert_eq!(saved.lists[0].comment, "Draft of b");
        assert_eq!(Changelists::load(dir.path()).lists[0].comment, "Draft of b");
    }

    #[test]
    fn a_rejected_list_comment_writes_nothing() {
        let dir = TempDir::new().unwrap();

        set_list_comment(
            Path::new("/repos/b"),
            "/repos/a",
            dir.path(),
            "default",
            "x",
        )
        .unwrap_err();

        assert!(!dir.path().join("changelists.json").exists());
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

        let blob = crate::test_support::git_command()
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
        crate::test_support::git_command()
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
        let merge = crate::test_support::git_command()
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

    // Ruling R9: lines chosen in the Diff panel or the diff window do not change the size that is
    // measured, the file on disk (`VirtualFile.length` in IntelliJ).
    #[test]
    fn a_file_chosen_by_lines_is_measured_whole_on_disk() {
        let repo = TempRepo::new();
        repo.git(&["config", "core.autocrlf", "false"]);
        repo.commit("a.txt", b"small\n", "base");
        let size = 1024 * 1024 + 1;
        fs::write(repo.path().join("a.txt"), vec![b'x'; size]).unwrap();
        let selections = [FileSelection {
            path: "a.txt".to_string(),
            selection: Vec::new(),
        }];
        let settings = Settings {
            large_files_limit_mb: 1,
            ..Default::default()
        };

        let found = warnings_for(
            repo.path(),
            CommitRequest::default(),
            &selections,
            &settings,
        );

        assert_eq!(
            found,
            vec![CommitWarning::LargeFiles(vec![(
                "a.txt".to_string(),
                size as u64
            )])]
        );
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
                "Show Git Log",
                "Create Patch from Local Changes...",
                "Apply Patch...",
                "Apply Patch from Clipboard...",
                "Shelve Changes...",
                "Show Shelf",
                "Import Shelf from .idea…",
                "Stash Changes…",
                "Unstash Changes…",
                "Rollback...",
                "Commit File…",
                "Add",
                "Annotate with Git Blame",
                "Compare with the Same Repository Version",
                "Compare with Revision...",
                "Compare with Branch…",
                "Show History",
                "Show History for Selection…",
                "View Pull Requests",
                "Manage Remotes…",
                "Clone…",
                "Create Git Repository…",
                "Abort Revert",
                "Abort Cherry-Pick",
                "Open .git/info/exclude",
            ]
        );
    }

    // `Git.MainMenu.LocalChanges` (`_Uncommitted Changes`) after Reset HEAD…, its own submenu, and the
    // Rebase group keeps its text.
    #[test]
    fn the_uncommitted_changes_group_holds_the_stash() {
        let groups: Vec<(&str, &str, Vec<&str>)> = GIT_MENU
            .iter()
            .filter_map(|entry| match entry {
                Group(id, text, children) => Some((
                    *id,
                    *text,
                    menu_items(children).iter().map(|item| item.1).collect(),
                )),
                _ => None,
            })
            .collect();

        assert_eq!(
            groups,
            [
                (
                    REBASE_GROUP,
                    "Rebase",
                    vec!["Abort Rebase", "Continue Rebase", "Skip Commit"]
                ),
                (
                    PATCH_GROUP,
                    "Patch",
                    vec![
                        "Create Patch from Local Changes...",
                        "Apply Patch...",
                        "Apply Patch from Clipboard..."
                    ]
                ),
                (
                    LOCAL_CHANGES_GROUP,
                    "Uncommitted Changes",
                    vec![
                        "Shelve Changes...",
                        "Show Shelf",
                        "Import Shelf from .idea…",
                        "Stash Changes…",
                        "Unstash Changes…",
                        "Rollback..."
                    ]
                ),
                (
                    FILE_GROUP,
                    "Current File",
                    vec![
                        "Commit File…",
                        "Add",
                        "Annotate with Git Blame",
                        "Compare with the Same Repository Version",
                        "Compare with Revision...",
                        "Compare with Branch…",
                        "Show History",
                        "Show History for Selection…"
                    ]
                ),
                (GITHUB_GROUP, "GitHub", vec!["View Pull Requests"]),
            ]
        );
        assert!(!starts_disabled(LOCAL_CHANGES_GROUP));
    }

    // `Git.MainMenu.FileActions` between Uncommitted Changes and Manage Remotes…: it is always there,
    // its entries start off and only the focused window's selection turns them on, never the
    // repository state; with the viewer focused they act on its file.
    #[test]
    fn the_current_file_actions_follow_the_focused_window() {
        let position = |id: &str| {
            GIT_MENU.iter().position(|entry| match entry {
                Item(item, _) | Group(item, _, _) => *item == id,
                Separator => false,
            })
        };
        let file = position(FILE_GROUP).unwrap();
        assert_eq!(position(LOCAL_CHANGES_GROUP), Some(file - 1));
        assert!(matches!(GIT_MENU[file + 1], Separator));
        assert_eq!(position(GITHUB_GROUP), Some(file + 2));
        assert_eq!(position("Git.Configure.Remotes"), Some(file + 3));
        assert_eq!(
            position("Vcs.Show.Log"),
            Some(position(PATCH_GROUP).unwrap() - 1)
        );
        assert!(matches!(
            GIT_MENU[position("Vcs.Show.Log").unwrap() - 1],
            Separator
        ));

        assert!(!starts_disabled(FILE_GROUP));
        for id in FILE_MENU {
            assert!(is_git_menu_item(id), "{id}");
            assert!(starts_disabled(id), "{id}");
            assert!(!driven_by_state(id), "{id}");
            assert_eq!(
                action_window(id, MAIN_WINDOW, Some("viewer")),
                "viewer",
                "{id}"
            );
            assert_eq!(action_window(id, MAIN_WINDOW, None), MAIN_WINDOW, "{id}");
            assert_eq!(
                action_window(id, MAIN_WINDOW, Some("main")),
                MAIN_WINDOW,
                "{id}"
            );
        }
        assert!(!is_git_menu_item(FILE_GROUP));
        assert!(is_git_menu_item("Vcs.Show.Log"));
        assert!(!starts_disabled("Vcs.Show.Log"));
        assert_eq!(
            action_window("Vcs.Show.Log", MAIN_WINDOW, Some("viewer")),
            MAIN_WINDOW
        );
        assert_eq!(
            action_window("ChangesView.Revert", MAIN_WINDOW, Some("viewer")),
            MAIN_WINDOW
        );
    }

    // `ChangesView.Shelve` is an executor action, hidden with the staging area like Create Patch;
    // `Vcs.Show.Shelf` stays.
    #[test]
    fn shelve_changes_is_hidden_in_staging_mode() {
        let ids = |staging| -> Vec<&str> {
            local_changes_menu(staging)
                .iter()
                .map(|item| item.0)
                .collect()
        };

        assert_eq!(
            ids(false),
            [
                "ChangesView.Shelve",
                "Vcs.Show.Shelf",
                IMPORT_IDEA_SHELF,
                "Git.Stash",
                "Git.Unstash",
                "ChangesView.Revert"
            ]
        );
        assert_eq!(
            ids(true),
            [
                "Vcs.Show.Shelf",
                IMPORT_IDEA_SHELF,
                "Git.Stash",
                "Git.Unstash",
                "ChangesView.Revert"
            ]
        );
        assert!(is_git_menu_item("ChangesView.Shelve"));
        assert!(is_git_menu_item("Vcs.Show.Shelf"));
        assert!(is_git_menu_item(IMPORT_IDEA_SHELF));
        assert!(is_git_menu_item("ChangesView.Revert"));
    }

    // `Patch.MainMenu` after Reset HEAD…, before Uncommitted Changes, in `VcsActions.xml`'s order.
    // `Create Patch from Local Changes...` is an executor action: hidden with the staging area
    // (`CheckinActionUtil.updateCommonCommitAction`, `ExternalCommitMode.disableDefaultCommitAction`);
    // the two Apply actions stay, so the submenu is never empty.
    #[test]
    fn create_patch_from_local_changes_is_hidden_in_staging_mode() {
        let group = GIT_MENU
            .iter()
            .find_map(|entry| match entry {
                Group(id, text, children) if *id == PATCH_GROUP => {
                    Some((*text, menu_items(children)))
                }
                _ => None,
            })
            .unwrap();

        let apply = [
            ("ChangesView.ApplyPatch", "Apply Patch..."),
            (
                "ChangesView.ApplyPatchFromClipboard",
                "Apply Patch from Clipboard...",
            ),
        ];
        let create = (
            "ChangesView.CreatePatch",
            "Create Patch from Local Changes...",
        );
        let all = [[create].as_slice(), &apply].concat();
        assert_eq!(group, ("Patch", all.clone()));
        assert_eq!(patch_menu(false), all);
        assert_eq!(patch_menu(true), apply);
        assert!(is_git_menu_item("ChangesView.CreatePatch"));
        assert!(is_git_menu_item("ChangesView.ApplyPatch"));
        assert!(is_git_menu_item("ChangesView.ApplyPatchFromClipboard"));
        assert!(!starts_disabled(PATCH_GROUP));
    }

    // `Git.MainMenu.MergeActions` and `RebaseActions` are only visible in their state, and so are the two below
    // (`Git.Revert.Abort`, `Git.CherryPick.Abort`): the native menu does not hide, it disables them.
    #[test]
    fn the_ongoing_actions_start_disabled() {
        let ongoing: Vec<&str> = ONGOING_MENU.to_vec();
        for (id, _) in git_menu_items() {
            if FILE_MENU.contains(&id) {
                continue;
            }
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
        for id in ["Git.Merge", "Git.Rebase", "Git.Fetch"] {
            assert!(!starts_disabled(id), "{id}");
            assert!(driven_by_state(id), "{id}");
            assert!(is_git_menu_item(id), "{id}");
        }
        assert!(driven_by_state("Git.Rebase.Skip"));
    }

    // `GitHub.MainMenu`, added before `Git.Configure.Remotes` (github-core-config.xml:111-120), with
    // `Github.View.Pull.Request` (:97-99), the only one of its actions Ramify has. The action is
    // visible only while the Pull Requests window is available (`GithubViewPullRequestsAction.update`),
    // and the group goes with it: the submenu starts out of the menu and is put in while the
    // interface names the action among the ones that apply.
    #[test]
    fn view_pull_requests_is_in_a_github_submenu_shown_only_with_the_window() {
        let group = GIT_MENU
            .iter()
            .find_map(|entry| match entry {
                Group(id, text, children) if *id == GITHUB_GROUP => {
                    Some((*text, menu_items(children)))
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(
            group,
            ("GitHub", vec![(VIEW_PULL_REQUESTS, "View Pull Requests")])
        );
        assert_eq!(VIEW_PULL_REQUESTS, "Github.View.Pull.Request");
        assert!(is_git_menu_item(VIEW_PULL_REQUESTS));
        assert!(!is_git_menu_item(GITHUB_GROUP));
        assert!(starts_hidden(GITHUB_GROUP));
        assert!(!starts_hidden(FILE_GROUP));
        assert!(!starts_disabled(VIEW_PULL_REQUESTS));
        assert_eq!(
            action_window(VIEW_PULL_REQUESTS, MAIN_WINDOW, Some("viewer")),
            MAIN_WINDOW
        );

        let ids =
            |names: &[&str]| -> Vec<String> { names.iter().map(|id| id.to_string()).collect() };
        assert!(github_menu_shown(&ids(&["Git.Merge", VIEW_PULL_REQUESTS])));
        assert!(!github_menu_shown(&ids(&["Git.Merge", "Git.Fetch"])));
        assert!(!github_menu_shown(&ids(&[GITHUB_GROUP])));
    }

    #[test]
    fn only_the_git_menu_ids_come_out_as_git_action() {
        assert!(is_git_menu_item("Git.Fetch"));
        assert!(is_git_menu_item("Git.Init"));
        assert!(is_git_menu_item("Git.Rebase.Skip"));
        assert!(!is_git_menu_item(REBASE_GROUP));
        assert!(!is_git_menu_item(LOCAL_CHANGES_GROUP));
        assert!(is_git_menu_item("Git.Stash"));
        assert!(is_git_menu_item("Git.Unstash"));
        assert!(!is_git_menu_item(QUIT_MENU_ID));
        assert!(!is_git_menu_item(SETTINGS_MENU_ID));
        assert!(!is_git_menu_item(OPEN_MENU_ID));
    }

    #[test]
    fn open_goes_first_in_the_file_menu() {
        assert!(is_file_menu("File"));
        assert!(!is_file_menu("Edit"));
        assert_eq!(OPEN_MENU_TEXT, "Open…");
    }

    #[test]
    fn a_recent_project_row_carries_its_path_and_is_not_a_git_action() {
        let id = recent_menu_id("/Users/me/work/app");
        assert_eq!(recent_menu_path(&id), Some("/Users/me/work/app"));
        assert!(!is_git_menu_item(&id));
        assert_eq!(recent_menu_path(OPEN_MENU_ID), None);
        assert_eq!(recent_menu_path(RECENT_MENU_ID), None);
        assert_eq!(RECENT_MENU_TEXT, "Recent Projects");
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

        assert!(claim_merge(&mut shown, false, conflict("a.txt")));
        assert_eq!(session_path(&shown).as_deref(), Some("a.txt"));

        assert!(!claim_merge(&mut shown, true, conflict("b.txt")));
        assert_eq!(session_path(&shown).as_deref(), Some("a.txt"));
    }

    fn conflict(path: &str) -> MergeSession {
        MergeSession::Conflict(path.to_string())
    }

    fn contents(id: &str) -> MergeContents {
        MergeContents {
            id: id.to_string(),
            title: "Merge Revisions for /repo/a.txt".to_string(),
            titles: [
                "Your version".to_string(),
                "Result".to_string(),
                "Patched version".to_string(),
            ],
            left: "a\nX\n".to_string(),
            base: "a\nb\n".to_string(),
            right: "a\nY\n".to_string(),
            path: "a.txt".to_string(),
            abort: false,
        }
    }

    #[test]
    fn contents_mode_keeps_the_single_window() {
        let mut shown = None;

        assert!(claim_merge(
            &mut shown,
            false,
            MergeSession::Contents(contents("1"))
        ));
        assert!(!claim_merge(&mut shown, true, conflict("b.txt")));
        assert!(!claim_merge(
            &mut shown,
            true,
            MergeSession::Contents(contents("2"))
        ));

        assert_eq!(session_contents(&shown), Some(contents("1")));
        assert_eq!(session_path(&shown), None);
    }

    #[test]
    fn a_conflict_session_has_no_contents() {
        let shown = Some(conflict("a.txt"));

        assert_eq!(session_contents(&shown), None);
        assert_eq!(session_path(&shown).as_deref(), Some("a.txt"));
    }

    #[test]
    fn contents_mode_arrives_with_its_id_and_the_text() {
        let done = merge_done(
            MergeSession::Contents(contents("7")),
            MergeResult::Resolved,
            Some("a\nZ\n".to_string()),
        );

        assert_eq!(
            serde_json::to_value(&done).unwrap(),
            serde_json::json!({ "id": "7", "result": "resolved", "content": "a\nZ\n" })
        );
    }

    #[test]
    fn a_cancelled_contents_merge_carries_no_text() {
        let done = merge_done(
            MergeSession::Contents(contents("7")),
            MergeResult::Cancelled,
            Some("ignored".to_string()),
        );

        assert_eq!(
            serde_json::to_value(&done).unwrap(),
            serde_json::json!({ "id": "7", "result": "cancelled" })
        );
    }

    // A patch conflict's `Abort…` (`MERGE_CANCEL_HANDLER` of `ApplyPatchAction.applyContent`):
    // `Abort and Rollback` and `Skip` close the window with their own result and no text.
    #[test]
    fn a_patch_merge_closes_as_aborted_or_skipped_without_text() {
        let input: MergeContents = serde_json::from_value(serde_json::json!({
            "id": "7",
            "title": "Patch Conflict for a.txt (/repo)",
            "titles": ["Local version", "Merge result", "Patched version"],
            "left": "a\nX\n",
            "base": "a\nb\n",
            "right": "a\nY\n",
            "path": "a.txt",
            "abort": true,
        }))
        .unwrap();
        assert!(input.abort);
        assert!(!contents("7").abort);

        for (result, text) in [(MergeResult::Abort, "abort"), (MergeResult::Skip, "skip")] {
            let done = merge_done(
                MergeSession::Contents(input.clone()),
                result,
                Some("ignored".to_string()),
            );
            assert_eq!(
                serde_json::to_value(&done).unwrap(),
                serde_json::json!({ "id": "7", "result": text })
            );
        }
    }

    #[test]
    fn the_input_of_contents_mode_is_read_in_camel_case() {
        let input: MergeContents = serde_json::from_value(serde_json::json!({
            "id": "7",
            "title": "Merge Revisions for /repo/a.txt",
            "titles": ["Your version", "Result", "Patched version"],
            "left": "a\nX\n",
            "base": "a\nb\n",
            "right": "a\nY\n",
            "path": "a.txt",
        }))
        .unwrap();

        assert_eq!(input, contents("7"));
    }

    // `ApplicationActivationListener` (on-the-fly patch detection): going from one of Ramify's
    // windows to another is not leaving the app; it is left once a window lost the focus and none
    // has it a moment later, and comes back with the next focus.
    #[test]
    fn the_app_is_activated_only_when_the_focus_comes_from_outside() {
        let mut activation = Activation::default();

        assert!(!activation.gained(), "it starts active");
        let other_window = true;
        let lost = activation.lost();
        assert!(!activation.settled(lost, other_window), "main to merge");
        assert!(!activation.gained());
        let lost = activation.lost();
        assert!(
            activation.settled(lost, false),
            "no window of Ramify has the focus"
        );
        assert!(!activation.settled(lost, false), "said once");
        assert!(activation.gained(), "back from another app");
        assert!(!activation.gained());
    }

    // P22: a window that gains the focus while a loss is being settled wins over that loss, which
    // asked about the windows before it.
    #[test]
    fn a_focus_gained_while_a_loss_settles_keeps_the_app_active() {
        let mut activation = Activation::default();

        let lost = activation.lost();
        // The thread of `lost` asked the windows (none focused yet), then a window gained it.
        assert!(!activation.gained());
        assert!(!activation.settled(lost, false));
        let lost = activation.lost();
        assert!(activation.settled(lost, false));
    }

    // Task 3's choosers only live in the main window: the diff and merge windows, which show
    // repository text, get no dialog permission.
    #[test]
    fn the_choosers_are_granted_to_the_main_window_only() {
        let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("capabilities");
        let mut granted = Vec::new();
        for entry in std::fs::read_dir(folder).unwrap() {
            let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
            let capability: serde_json::Value = serde_json::from_str(&text).unwrap();
            let windows: Vec<String> =
                serde_json::from_value(capability["windows"].clone()).unwrap();
            let permissions: Vec<String> =
                serde_json::from_value(capability["permissions"].clone()).unwrap();
            for permission in permissions {
                granted.push((permission, windows.clone()));
            }
        }
        granted.sort();

        let all = vec![
            "diff*".to_string(),
            "main*".to_string(),
            "merge*".to_string(),
            "viewer*".to_string(),
        ];
        let main = vec!["main*".to_string()];
        let sorted = |mut windows: Vec<String>| {
            windows.sort();
            windows
        };
        let granted: Vec<(String, Vec<String>)> = granted
            .into_iter()
            .map(|(permission, windows)| (permission, sorted(windows)))
            .collect();
        assert_eq!(
            granted,
            [
                ("core:default".to_string(), all),
                // The selection history only listens to its updates.
                (
                    "core:event:default".to_string(),
                    vec!["selection-history*".to_string()]
                ),
                ("dialog:allow-open".to_string(), main.clone()),
                ("dialog:allow-save".to_string(), main),
            ]
        );
    }

    #[test]
    fn closing_the_merge_window_is_asked_of_its_page() {
        assert_eq!(close_event_of(MERGE_WINDOW), MERGE_CLOSE_EVENT);
        assert_eq!(close_event_of(DIFF_WINDOW), DIFF_CLOSE_EVENT);
        assert_eq!(close_event_of("merge-2"), MERGE_CLOSE_EVENT);
        assert_eq!(close_event_of("diff-2"), DIFF_CLOSE_EVENT);
        assert!(!owns_watcher("merge-2"));
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
            let status = crate::test_support::git_command()
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
        assert_eq!(
            lines.next().unwrap().trim(),
            "flush_diff_window(&app, &state, &window, OPEN_ACTION).await?;"
        );
        assert_eq!(
            lines.next().unwrap().trim(),
            "leave_merge_window(&app, &state, &window, OPEN_ACTION).await?;"
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
        let done = merge_done(
            conflict("a.txt"),
            MergeResult::Cancelled,
            Some("ignored".to_string()),
        );

        assert_eq!(
            serde_json::to_value(&done).unwrap(),
            serde_json::json!({ "path": "a.txt", "result": "cancelled" })
        );
        let result: MergeResult = serde_json::from_str("\"theirs\"").unwrap();
        assert_eq!(result, MergeResult::Theirs);
    }

    fn open_repo(state: &AppState, label: &str, repo: &Path) {
        *session_of(state, label).unwrap().repo.lock().unwrap() = Some(RepoSession {
            path: repo.to_path_buf(),
        });
    }

    // Review Focus 2: closing `main-2` while its push waits for a password. The question goes
    // to `main-2`, which is still there (its session too); the close waits for the push, and
    // only then are its auxiliary windows and its session dropped. `main` is not touched.
    #[test]
    fn closing_a_window_waits_for_its_writes_and_shows_its_prompts() {
        const WAIT: Duration = Duration::from_secs(20);
        let first = TempRepo::new();
        let second = TempRepo::new();
        let state = Arc::new(AppState::default());
        open_repo(&state, "main", first.path());
        open_repo(&state, "main-2", second.path());
        state.open_order.lock().unwrap().push("main-2".to_string());
        state.console.output("main", "a line of main");
        state.console.output("main-2", "a line of main-2");
        // What happened, in order.
        let events = Arc::new(Mutex::new(Vec::<String>::new()));
        let (asked, prompts) = mpsc::channel();
        let asked = Mutex::new(asked);
        let (seen_state, seen_events) = (state.clone(), events.clone());
        let server = Server::start_with_program(
            PathBuf::from("/the/app"),
            move |owner: &str, id, _| {
                // `start_askpass` shows the question in the window with this label.
                let open = seen_state.sessions.lock().unwrap().contains_key(owner);
                seen_events
                    .lock()
                    .unwrap()
                    .push(format!("prompt in {owner}, session: {open}"));
                asked.lock().unwrap().send(id).is_ok()
            },
            |_: &str, _| {},
        )
        .unwrap();
        state.tasks.use_askpass(server.handle());
        let (entered, inside) = mpsc::channel();
        let (ask, asking) = mpsc::channel::<()>();
        let pushed = events.clone();
        let push = state.tasks.start(
            "main-2",
            second.path(),
            crate::task::TaskKind::Write,
            "push".to_string(),
            move |ctx| {
                entered.send(()).unwrap();
                asking.recv_timeout(WAIT).unwrap();
                let answer = ctx
                    .ask_as_git("Password for 'https://example.com':")
                    .map_err(|error| GitError::Io(error.to_string()))?;
                let answered = !answer.is_empty();
                pushed
                    .lock()
                    .unwrap()
                    .push(format!("push answered: {answered}"));
                Ok(())
            },
        );
        inside.recv_timeout(WAIT).unwrap();

        thread::scope(|scope| {
            let closing = scope.spawn(|| {
                finish_closing(
                    &state,
                    "main-2",
                    WAIT,
                    || Ok(()),
                    || {},
                    || true,
                    || {
                        let open = state.sessions.lock().unwrap().contains_key("main-2");
                        events
                            .lock()
                            .unwrap()
                            .push(format!("auxiliary windows destroyed, session: {open}"));
                    },
                ) == CloseOutcome::Closed
            });
            // The question comes during the wait.
            ask.send(()).unwrap();
            let id = prompts.recv_timeout(WAIT).unwrap();
            assert!(!closing.is_finished(), "it did not wait for the push");
            // Another window cannot answer it; the closing one can.
            assert!(!server.answer("main", id, Some("wrong".to_string())));
            assert!(server.answer("main-2", id, Some("secret".to_string())));
            assert!(closing.join().unwrap());
        });

        assert_eq!(
            *events.lock().unwrap(),
            [
                "prompt in main-2, session: true",
                "push answered: true",
                "auxiliary windows destroyed, session: true",
            ]
        );
        assert!(!state.tasks.cancel("main-2", push), "the push had ended");
        assert!(!state.sessions.lock().unwrap().contains_key("main-2"));
        assert!(state.console.lines_of("main-2").is_empty());
        assert_eq!(*state.open_order.lock().unwrap(), ["main"]);
        // `main` keeps its repository, its console and its writes.
        assert_eq!(repo_of(&state, "main").unwrap(), first.path());
        assert_eq!(state.console.lines_of("main").len(), 1);
        let (done, finished) = mpsc::channel();
        state.tasks.listen(move |event| {
            if event.state != crate::task::TaskState::Running {
                let _ = done.send(event.state);
            }
        });
        state.tasks.start(
            "main",
            first.path(),
            crate::task::TaskKind::Write,
            "write".to_string(),
            |_| Ok(()),
        );
        assert_eq!(
            finished.recv_timeout(WAIT).unwrap(),
            crate::task::TaskState::Done
        );
    }

    // Past the limit the window stays as it was, and writes again.
    #[test]
    fn a_close_whose_writes_do_not_finish_keeps_the_window() {
        let repo = TempRepo::new();
        let state = AppState::default();
        open_repo(&state, "main-2", repo.path());
        state.open_order.lock().unwrap().push("main-2".to_string());
        let (release, released) = mpsc::channel::<()>();
        let (entered, inside) = mpsc::channel();
        state.tasks.start(
            "main-2",
            repo.path(),
            crate::task::TaskKind::Write,
            "slow".to_string(),
            move |_| {
                entered.send(()).unwrap();
                let _ = released.recv_timeout(Duration::from_secs(20));
                Ok(())
            },
        );
        inside.recv_timeout(Duration::from_secs(20)).unwrap();
        let mut destroyed = false;

        let closed = finish_closing(
            &state,
            "main-2",
            Duration::from_millis(100),
            || Ok(()),
            || {},
            || true,
            || destroyed = true,
        );

        assert_eq!(closed, CloseOutcome::TimedOut);
        assert!(!destroyed);
        assert_eq!(repo_of(&state, "main-2").unwrap(), repo.path());
        assert_eq!(*state.open_order.lock().unwrap(), ["main", "main-2"]);
        release.send(()).unwrap();
        // It writes again: `resume_writes_of` lifted the refusal.
        let (done, finished) = mpsc::channel();
        state.tasks.listen(move |event| {
            if event.title == "after" && event.state != crate::task::TaskState::Running {
                let _ = done.send(event.state);
            }
        });
        state.tasks.start(
            "main-2",
            repo.path(),
            crate::task::TaskKind::Write,
            "after".to_string(),
            |_| Ok(()),
        );
        assert_eq!(
            finished.recv_timeout(Duration::from_secs(20)).unwrap(),
            crate::task::TaskState::Done
        );
    }

    // Review Focus 5: a Git menu action while an auxiliary window of `main-2` has the focus goes
    // to `main-2` or its viewer, never to `main`, whatever else is open.
    #[test]
    fn a_menu_action_goes_to_the_focused_owner() {
        let id = FILE_MENU[0];
        assert_eq!(action_window(id, "main-2", Some("viewer-2")), "viewer-2");
        assert_eq!(
            action_window("Vcs.Show.Log", "main-2", Some("viewer-2")),
            "main-2"
        );
        // The first window's viewer has the focus, but the menu belongs to `main-2`.
        assert_eq!(action_window(id, "main-2", Some("viewer")), "main-2");
        assert_eq!(action_window(id, "main-2", Some("main-2")), "main-2");
        assert_eq!(action_window(id, "main-2", Some("diff-2")), "main-2");
        assert_eq!(action_window(id, "main-2", None), "main-2");

        // An auxiliary window gaining the focus makes its owner the menu's.
        let state = AppState::default();
        assert_eq!(menu_owner(&state), "main");
        assert_eq!(menu_follows(&state, "viewer-2").as_deref(), Some("main-2"));
        assert_eq!(menu_owner(&state), "main-2");
        assert_eq!(menu_follows(&state, "viewer-2"), None);
        assert_eq!(menu_follows(&state, "diff").as_deref(), Some("main"));
        assert_eq!(menu_owner(&state), "main");
        // A window that belongs to no project window leaves it as it was.
        assert_eq!(menu_follows(&state, "settings"), None);
        assert_eq!(menu_owner(&state), "main");
    }

    // The menu state a window sends before the native focus event made it the owner is refused;
    // the owner change itself names who must send it again, and nobody else.
    #[test]
    fn a_new_menu_owner_is_asked_to_send_its_menu_state_again() {
        let state = AppState::default();
        assert!(!menu_accepts(&state, "main-2"));
        // The page's `focus` reached Rust first: refused, and nothing re-sends it by itself.
        assert_eq!(menu_follows(&state, "main-2").as_deref(), Some("main-2"));
        assert!(menu_accepts(&state, "main-2"));
        assert!(!menu_accepts(&state, "main"));
        // The same owner again, or a window of no project, asks nobody.
        assert_eq!(menu_follows(&state, "viewer-2"), None);
        assert_eq!(menu_follows(&state, "settings"), None);
        // An auxiliary window of another project: that project is asked, not the one focused.
        assert_eq!(menu_follows(&state, "viewer").as_deref(), Some("main"));
    }

    #[test]
    fn menu_state_from_an_unfocused_window_is_ignored() {
        let state = AppState::default();
        *state.last_focused_main.lock().unwrap() = "main-2".to_string();
        assert!(!menu_accepts(&state, "main"));
        assert!(!menu_accepts(&state, "viewer"));
        assert!(menu_accepts(&state, "main-2"));
        assert!(menu_accepts(&state, "viewer-2"));
        assert!(!menu_accepts(&state, "settings"));
    }

    // The recent list leaves out the repository of the window that owns the menu.
    #[test]
    fn recent_projects_leave_out_the_focused_windows_repository() {
        let state = AppState::default();
        open_repo(&state, "main", Path::new("/work/a"));
        open_repo(&state, "main-2", Path::new("/work/b"));
        *state.last_focused_main.lock().unwrap() = "main-2".to_string();
        let projects = vec!["/work/a".to_string(), "/work/b".to_string()];

        let current = menu_repo(&state);
        let entries = recent::menu_entries(&projects, current.as_deref(), None);
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            ["/work/a"]
        );

        *state.last_focused_main.lock().unwrap() = "main".to_string();
        let current = menu_repo(&state);
        let entries = recent::menu_entries(&projects, current.as_deref(), None);
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            ["/work/b"]
        );
    }

    // The open list: the windows' repositories in their opening order, a window being opened
    // with the target it was built for, and the last focused one; a closed window leaves it, and
    // when it was the last focused another window takes that place.
    #[test]
    fn the_open_list_follows_the_windows() {
        let state = AppState::default();
        open_repo(&state, "main", Path::new("/work/a"));
        open_repo(&state, "main-3", Path::new("/work/c"));
        state
            .initial_targets
            .lock()
            .unwrap()
            .store("main-4", "/work/d".to_string());
        *state.open_order.lock().unwrap() = ["main", "main-3", "main-2", "main-4"]
            .map(String::from)
            .to_vec();
        // `main-2` has no repository: it is not listed.
        open_repo(&state, "main-2", Path::new("/work/b"));
        *session_of(&state, "main-2").unwrap().repo.lock().unwrap() = None;
        *state.last_focused_main.lock().unwrap() = "main-3".to_string();
        let exists = |label: &str| label != "main-5";

        let open = open_snapshot(&state, exists);

        assert_eq!(open.projects, ["/work/a", "/work/c", "/work/d"]);
        assert_eq!(open.last_focused.as_deref(), Some("/work/c"));

        assert_eq!(
            finish_closing(
                &state,
                "main-3",
                Duration::from_secs(5),
                || Ok(()),
                || {},
                || true,
                || {}
            ),
            CloseOutcome::Closed
        );
        let open = open_snapshot(&state, exists);
        assert_eq!(open.projects, ["/work/a", "/work/d"]);
        assert_eq!(*state.last_focused_main.lock().unwrap(), "main");
        assert_eq!(open.last_focused.as_deref(), Some("/work/a"));
    }

    // Review Focus 4: quitting while `main-2`, not focused, has a half-done merge. The question
    // is asked in `merge-2`, the user keeps the merge, and the quit stops for every window:
    // nothing exits, `main-3` is not asked, and `main` goes on as before.
    #[test]
    fn quit_stops_when_any_window_refuses() {
        let first = TempRepo::new();
        let state = Arc::new(AppState::default());
        open_repo(&state, "main", first.path());
        for owner in ["main", "main-2", "main-3"] {
            *session_of(&state, owner)
                .unwrap()
                .merge_ready
                .lock()
                .unwrap() = true;
        }
        // Only `main-2` and `main-3` have a merge window.
        let exists = |label: &str| label == "merge-2" || label == "merge-3";
        let left = Arc::new(Mutex::new(Vec::<String>::new()));
        let asked = Arc::new(Mutex::new(Vec::<String>::new()));
        let mut exited = false;

        let quit = tauri::async_runtime::block_on(quit_flow(
            ["main", "main-2", "main-3"].map(String::from).to_vec(),
            |owner| {
                let (state, left, asked) = (state.clone(), left.clone(), asked.clone());
                async move {
                    left.lock().unwrap().push(owner.clone());
                    assert!(flush_request(&state, &owner, |_| false)?.is_none());
                    let Some((merge, answer)) = merge_leave_request(&state, &owner, exists)? else {
                        return Ok(());
                    };
                    asked.lock().unwrap().push(merge.clone());
                    // The page of that merge window acknowledges, then the user keeps the merge.
                    let session = session_of(&state, &merge)?;
                    answer_merge_received(&session);
                    answer_merge_leave(&session, false);
                    wait_for_merge(
                        &answer,
                        Duration::from_secs(5),
                        Duration::from_secs(5),
                        QUIT_ACTION,
                    )
                }
            },
            |_| false,
            || exited = true,
        ));

        let (owner, message) = quit.unwrap_err();
        assert_eq!(owner, "main-2");
        assert!(message.contains("half-done merge"), "{message}");
        assert!(message.contains(QUIT_ACTION), "{message}");
        assert!(!exited);
        assert_eq!(*left.lock().unwrap(), ["main", "main-2"]);
        assert_eq!(*asked.lock().unwrap(), ["merge-2"]);
        // `main` is untouched: its repository, nothing queued, and its writes still run.
        assert_eq!(repo_of(&state, "main").unwrap(), first.path());
        assert!(session_of(&state, "main")
            .unwrap()
            .merge_leave
            .lock()
            .unwrap()
            .is_empty());
        let (done, finished) = mpsc::channel();
        state.tasks.listen(move |event| {
            if event.state != crate::task::TaskState::Running {
                let _ = done.send(event.state);
            }
        });
        state.tasks.start(
            "main",
            first.path(),
            crate::task::TaskKind::Write,
            "write".to_string(),
            |_| Ok(()),
        );
        assert_eq!(
            finished.recv_timeout(Duration::from_secs(20)).unwrap(),
            crate::task::TaskState::Done
        );
    }

    // With every window agreeing, the quit goes on to wait for the writes and exit.
    #[test]
    fn quit_goes_on_when_every_window_agrees() {
        let mut exited = false;
        let mut left = Vec::new();

        let quit = tauri::async_runtime::block_on(quit_flow(
            ["main", "main-2"].map(String::from).to_vec(),
            |owner| {
                left.push(owner);
                async { Ok(()) }
            },
            |_| false,
            || exited = true,
        ));

        assert!(quit.is_ok());
        assert!(exited);
        assert_eq!(left, ["main", "main-2"]);
    }

    // Final review: `app-activated` names the project window of the window that got the focus,
    // an auxiliary one included, so that only that main page looks at the clipboard.
    #[test]
    fn activation_names_the_activated_project_window() {
        assert_eq!(activated_owner("main").as_deref(), Some("main"));
        assert_eq!(activated_owner("viewer-2").as_deref(), Some("main-2"));
        assert_eq!(activated_owner("merge-3").as_deref(), Some("main-3"));
        assert_eq!(activated_owner("settings"), None);
    }

    // Final review: a second Quit while one runs does nothing; the first one goes on.
    #[test]
    fn a_second_quit_while_one_runs_does_nothing() {
        let state = AppState::default();

        assert!(begin_quit(&state));
        assert!(!begin_quit(&state));
        assert!(state.quitting.load(Ordering::SeqCst));

        // A quit that did not go ahead lets the next one start.
        state.quitting.store(false, Ordering::SeqCst);
        assert!(begin_quit(&state));
    }

    // Final review: a window whose close is finishing is not asked, and one that closes while it
    // is asked does not stop the quit: its leave flow fails only because it has gone.
    #[test]
    fn quit_skips_the_windows_that_closed() {
        let state = AppState::default();
        open_repo(&state, "main", Path::new("/work/a"));
        open_repo(&state, "main-2", Path::new("/work/b"));
        open_repo(&state, "main-3", Path::new("/work/c"));
        assert_eq!(
            finish_closing(
                &state,
                "main-2",
                Duration::from_secs(5),
                || Ok(()),
                || {},
                || true,
                || {}
            ),
            CloseOutcome::Closed
        );
        let left = Mutex::new(Vec::<String>::new());
        let mut exited = false;

        let quit = tauri::async_runtime::block_on(quit_flow(
            ["main", "main-2", "main-3"].map(String::from).to_vec(),
            |owner| {
                left.lock().unwrap().push(owner.clone());
                let state = &state;
                async move {
                    if owner == "main-3" {
                        // It finishes closing while its leave flow runs.
                        forget_window(state, &owner);
                    }
                    session_of(state, &owner).map(drop)
                }
            },
            |owner| state.closed_owners.lock().unwrap().contains(owner),
            || exited = true,
        ));

        assert!(quit.is_ok(), "{quit:?}");
        assert!(exited);
        assert_eq!(*left.lock().unwrap(), ["main", "main-3"]);
    }

    // Fix round 1: closing `main` and then `main-2` before `main` is gone. Only `main` is
    // destroyed; `main-2`, the last one, is hidden and keeps its session and its place in the
    // open list. A close whose other windows have all gone meanwhile hides instead.
    #[test]
    fn two_closes_in_flight_destroy_one_and_hide_the_last() {
        let state = AppState::default();
        open_repo(&state, "main", Path::new("/work/a"));
        open_repo(&state, "main-2", Path::new("/work/b"));
        state.open_order.lock().unwrap().push("main-2".to_string());
        let mains = ["main", "main-2"].map(String::from).to_vec();

        assert_eq!(start_close(&state, "main", &mains), CloseStart::Close);
        assert_eq!(
            start_close(&state, "main", &mains),
            CloseStart::AlreadyClosing
        );
        assert_eq!(start_close(&state, "main-2", &mains), CloseStart::Hide);

        let closed = finish_closing(
            &state,
            "main",
            Duration::from_secs(5),
            || Ok(()),
            || {},
            || other_main_remains(&state, "main", &mains),
            || {},
        );
        assert_eq!(closed, CloseOutcome::Closed);
        assert_eq!(repo_of(&state, "main-2").unwrap(), Path::new("/work/b"));
        assert_eq!(
            open_snapshot(&state, |label| label == "main-2").projects,
            ["/work/b"]
        );

        // `main-3` starts closing with `main-4` there, which goes before `main-3` is done.
        open_repo(&state, "main-3", Path::new("/work/c"));
        state.open_order.lock().unwrap().push("main-3".to_string());
        let both = ["main-3", "main-4"].map(String::from).to_vec();
        assert_eq!(start_close(&state, "main-3", &both), CloseStart::Close);
        let alone = ["main-3"].map(String::from).to_vec();
        let mut destroyed = false;
        let kept = finish_closing(
            &state,
            "main-3",
            Duration::from_secs(5),
            || Ok(()),
            || {},
            || other_main_remains(&state, "main-3", &alone),
            || destroyed = true,
        );
        assert_eq!(kept, CloseOutcome::Hide);
        assert!(!destroyed);
        assert_eq!(repo_of(&state, "main-3").unwrap(), Path::new("/work/c"));
        assert!(state
            .open_order
            .lock()
            .unwrap()
            .contains(&"main-3".to_string()));
    }

    // Fix round 1: what was typed in the diff window during the wait is saved before that window
    // is destroyed; if it cannot be saved, nothing is closed and the window writes again.
    #[test]
    fn a_close_saves_the_diff_window_again_before_destroying_it() {
        let repo = TempRepo::new();
        let state = AppState::default();
        open_repo(&state, "main-2", repo.path());
        let events = Mutex::new(Vec::<String>::new());
        let (release, released) = mpsc::channel::<()>();
        let (entered, inside) = mpsc::channel();
        let written = Arc::new(AtomicBool::new(false));
        let done = written.clone();
        state.tasks.start(
            "main-2",
            repo.path(),
            crate::task::TaskKind::Write,
            "write".to_string(),
            move |_| {
                entered.send(()).unwrap();
                released.recv_timeout(Duration::from_secs(20)).unwrap();
                done.store(true, Ordering::SeqCst);
                Ok(())
            },
        );
        inside.recv_timeout(Duration::from_secs(20)).unwrap();
        release.send(()).unwrap();

        let closed = finish_closing(
            &state,
            "main-2",
            Duration::from_secs(20),
            || {
                let write = written.load(Ordering::SeqCst);
                events
                    .lock()
                    .unwrap()
                    .push(format!("diff saved, write done: {write}"));
                Ok(())
            },
            || {},
            || true,
            || events.lock().unwrap().push("destroyed".to_string()),
        );

        assert_eq!(closed, CloseOutcome::Closed);
        assert_eq!(
            *events.lock().unwrap(),
            ["diff saved, write done: true", "destroyed"]
        );

        // A refused save keeps everything.
        let other = TempRepo::new();
        open_repo(&state, "main-3", other.path());
        let mut destroyed = false;
        let refused = finish_closing(
            &state,
            "main-3",
            Duration::from_secs(5),
            || Err(failed("The diff window is not responding".to_string())),
            || {},
            || true,
            || destroyed = true,
        );
        assert_eq!(
            refused,
            CloseOutcome::Refused("The diff window is not responding".to_string())
        );
        assert!(!destroyed);
        assert_eq!(repo_of(&state, "main-3").unwrap(), other.path());
        let (send, finished) = mpsc::channel();
        state.tasks.listen(move |event| {
            if event.title == "after" && event.state != crate::task::TaskState::Running {
                let _ = send.send(event.state);
            }
        });
        state.tasks.start(
            "main-3",
            other.path(),
            crate::task::TaskKind::Write,
            "after".to_string(),
            |_| Ok(()),
        );
        assert_eq!(
            finished.recv_timeout(Duration::from_secs(20)).unwrap(),
            crate::task::TaskState::Done
        );
    }

    // Final review: the page stays usable during the wait, so what was typed in the commit
    // message then is saved again once the writes ended, after the diff window, before anything
    // is destroyed. A close that does not go ahead saves nothing more: the window stays.
    #[test]
    fn a_close_saves_the_commit_draft_again_after_the_wait() {
        let repo = TempRepo::new();
        let state = AppState::default();
        open_repo(&state, "main-2", repo.path());
        let events = Mutex::new(Vec::<String>::new());
        let (release, released) = mpsc::channel::<()>();
        let (entered, inside) = mpsc::channel();
        let written = Arc::new(AtomicBool::new(false));
        let done = written.clone();
        state.tasks.start(
            "main-2",
            repo.path(),
            crate::task::TaskKind::Write,
            "write".to_string(),
            move |_| {
                entered.send(()).unwrap();
                released.recv_timeout(Duration::from_secs(20)).unwrap();
                done.store(true, Ordering::SeqCst);
                Ok(())
            },
        );
        inside.recv_timeout(Duration::from_secs(20)).unwrap();
        release.send(()).unwrap();

        let closed = finish_closing(
            &state,
            "main-2",
            Duration::from_secs(20),
            || {
                events.lock().unwrap().push("diff saved".to_string());
                Ok(())
            },
            || {
                let write = written.load(Ordering::SeqCst);
                events
                    .lock()
                    .unwrap()
                    .push(format!("draft saved, write done: {write}"));
            },
            || true,
            || events.lock().unwrap().push("destroyed".to_string()),
        );

        assert_eq!(closed, CloseOutcome::Closed);
        assert_eq!(
            *events.lock().unwrap(),
            ["diff saved", "draft saved, write done: true", "destroyed"]
        );

        let other = TempRepo::new();
        open_repo(&state, "main-3", other.path());
        let mut saved = false;
        let refused = finish_closing(
            &state,
            "main-3",
            Duration::from_secs(5),
            || Err(failed("The diff window is not responding".to_string())),
            || saved = true,
            || true,
            || {},
        );
        assert!(matches!(refused, CloseOutcome::Refused(_)));
        assert!(!saved);
    }

    // Final review: a window that is closing, or closed, opens no merge window: it would come
    // after the leave flow asked its merge window, and be destroyed with a half-done merge.
    #[test]
    fn a_closing_window_opens_no_merge_window() {
        let state = AppState::default();
        open_repo(&state, "main-2", Path::new("/work/b"));
        let mains = ["main", "main-2"].map(String::from).to_vec();
        assert!(merge_open_session(&state, "main-2").is_ok());

        assert_eq!(start_close(&state, "main-2", &mains), CloseStart::Close);

        for label in ["main-2", "diff-2", "viewer-2"] {
            let refused = merge_open_session(&state, label).map(drop).unwrap_err();
            assert!(
                refused.message.contains(MERGE_OPEN_ACTION),
                "{}",
                refused.message
            );
        }
        assert!(merge_open_session(&state, "main").is_ok());
    }

    // Fix round 1: a closed window's label never gets a session again, and a window that is
    // closing opens no repository.
    #[test]
    fn a_closed_window_gets_no_session_and_a_closing_one_opens_nothing() {
        let state = AppState::default();
        open_repo(&state, "main-2", Path::new("/work/b"));
        assert_eq!(
            finish_closing(
                &state,
                "main-2",
                Duration::from_secs(5),
                || Ok(()),
                || {},
                || true,
                || {}
            ),
            CloseOutcome::Closed
        );

        assert!(session_of(&state, "main-2").is_err());
        assert!(session_of(&state, "diff-2").is_err());
        assert!(!state.sessions.lock().unwrap().contains_key("main-2"));
        assert!(refuse_if_closing(&state, "main-2").is_err());

        let mains = ["main", "main-3"].map(String::from).to_vec();
        assert!(refuse_if_closing(&state, "main-3").is_ok());
        assert_eq!(start_close(&state, "main-3", &mains), CloseStart::Close);
        let refused = refuse_if_closing(&state, "main-3").unwrap_err();
        assert!(refused.message.contains(OPEN_ACTION), "{}", refused.message);
        assert!(session_of(&state, "main").is_ok());
    }

    // Fix round 1: the open list is not written while the app quits.
    #[test]
    fn quitting_writes_no_open_list() {
        let dir = TempDir::new().unwrap();
        let state = AppState::default();
        open_repo(&state, "main", Path::new("/work/a"));

        assert!(write_open_list(&state, dir.path(), |_| true).unwrap());
        let file = dir.path().join(open_projects::FILE);
        assert_eq!(
            open_projects::load(dir.path()).unwrap().projects,
            ["/work/a"]
        );

        fs::remove_file(&file).unwrap();
        state.quitting.store(true, Ordering::SeqCst);
        assert!(!write_open_list(&state, dir.path(), |_| true).unwrap());
        assert!(!file.exists());
    }
}
