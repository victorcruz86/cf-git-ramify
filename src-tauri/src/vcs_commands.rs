// The commands of branches, tags and reset. Each one first saves what was written in the diff window and
// launches a task, which delivers its result to `taskResult`.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::apply_commits::{self, Kind, PathsResult, Selection};
use crate::branch_info::{self, BranchKind, BranchList, FavoriteSet, Sync};
use crate::branches::{
    self, CheckoutMode, CheckoutOutcome, Deleted, DeletedRemote, DeletedTag, LostCommit, Overwrite,
    RemoteOptions, Repo, ResetMode,
};
use crate::commands::{
    base_dir, blocking_api, current_repo, current_settings, data_dir_of, failed, flush_diff_window,
    repo_changed, repo_data_dir, ApiError, AppState, RepoLocks, MAIN_WINDOW, SAVE_ACTION,
};
use crate::commit;
use crate::conflicts;
use crate::fetch::{self, FetchResult};
use crate::git::GitError;
use crate::gpg::{self, GpgKey, SigningConfig};
use crate::log::store::refs_fingerprint;
use crate::merge::{self, MergeOption};
use crate::merge3::{self, Block, Whitespace};
use crate::operation::{self, Operation};
use crate::patch::create::{self, CreateOptions, PatchEntry, PatchTarget};
use crate::patch::patch_failure;
use crate::patch_apply::{ApplyChoice, PatchInput, PathMapping};
use crate::preserve::{RestoreOutcome, Saved};
use crate::pull::{self, PullInfo, PullOption};
use crate::push::{self, PushInfo, PushResult, PushSpec, PushTags, UpdatedPush};
use crate::rebase::{self, RebaseParams};
use crate::remotes::{self, Cloned, InitOutcome, Remote};
use crate::rewrite::{self, MenuChecks, Rewritten, Undone};
use crate::settings::{self, AppPatch, PatchDefaults, RepoSettings, UpdateMethod};
use crate::shelf;
use crate::shelf::shelve::{
    shelve, shelve_silently, ShelveEntry, ShelveGroup, SWITCHED_BEFORE_SHELVING,
};
use crate::shelf::tab::{ListFiles, Restorable, ShelfFile};
use crate::snapshots::SnapshotId;
use crate::staging;
use crate::stash;
use crate::task::{TaskId, TaskKind};
use crate::update::{self, Aborted, UpdateOptions, UpdateResult};
use crate::worktree_diff::{self, ThreeSides, Written};

const BRANCH_SYNC_EVENT: &str = "branch-sync-changed";
// Per remote. One that does not answer must not leave anyone waiting: ssh takes minutes to give up.
const CHECK_LIMIT: Duration = Duration::from_secs(60);
// `merge.dialog.customizer.collecting.details.progress` (B:1293).
const COLLECTING_DETAILS: &str = "Collecting Commits Details…";
const ADDING_FILES: &str = "Adding Files to VCS…";
const ADDING_REMOTE: &str = "Adding Remote…";
const CHANGING_REMOTE: &str = "Changing Remote…";
const COMMITTING: &str = "Committing…";
const COMMITTING_MERGE: &str = "Committing Merge…";
const FETCHING: &str = "Fetching…";
// The dialog's title (`settings.configure.sign.gpg.for.repo.dialog.title`, decision 2): 241 writes
// without a progress title of its own.
const CONFIGURING_GPG: &str = "Configure GPG Key";
const LOADING_COMMITS: &str = "Loading Commits";
const LOADING_CONFLICTS: &str = "Loading merge conflicts…";
const LOADING_REVISIONS: &str = "Loading Revisions…";
const LOADING_UNMERGED: &str = "Loading unmerged branches…";
const REBASING: &str = "Rebasing…";
const PUSHING: &str = "Pushing…";
const REMOVING_REMOTE: &str = "Removing Remote…";
// `rebase.log.multiple.commit.editing.action.progress.indicator.action.possibility.check`.
const CHECKING_EDITING: &str = "Checking if it is allowed to edit selected commits…";
const RESOLVING_CONFLICTS: &str = "Resolving Conflicts…";
const TESTING_URL: &str = "Checking URL…";
const UPDATING: &str = "Update";
const UPDATING_BRANCHES: &str = "Updating branches…";
// `unstash.clearing.stashes`, `unstash.dialog.stash.list.load.progress.indicator.title`,
// `unstash.dialog.remove.stash.progress.indicator.title` (without the name if the entry is already gone),
// `stashing.progress.title`, `stash.files.progress.title` and `unstash.unstashing`.
const CLEARING_STASHES: &str = "Clearing stashes…";
const LOADING_STASHES: &str = "Loading list of stashes…";
const LOADING_STASH: &str = "Loading...";
const REMOVING_STASH: &str = "Removing Stash…";
const STASHING: &str = "Stashing changes…";
const STASHING_FILES: &str = "Stashing Files";
const UNSTASHING: &str = "Unstashing…";
// `patch.apply.dialog.title` and `patch.apply.rollback.progress.title`.
const APPLYING_PATCH: &str = "Apply Patch";
const LOADING_PATCH: &str = "Loading...";
const ROLLING_BACK_PATCH: &str = "Rollback applied changes…";
// `create.patch.commit.action.progress`.
const CREATING_PATCH: &str = "Creating Patch…";
// The reason of the copy of a worktree file a patch is written over.
const BEFORE_OVERWRITING_PATCH: &str = "before writing a patch over it";
// `shelve.changes.progress.text`, `shelve.changes.progress.title` and `activity.name.shelve`.
const SHELVING: &str = "Shelving changes…";
const SHELVING_SILENTLY: &str = "Shelving Changes";

// The signal is also emitted on failure: a half-done operation may have changed something.
fn start<T: Serialize + 'static>(
    app: &AppHandle,
    state: &State<'_, AppState>,
    kind: TaskKind,
    title: String,
    work: impl FnOnce(&Repo) -> Result<T, GitError> + Send + 'static,
) -> Result<TaskId, ApiError> {
    let path = current_repo(state)?;
    let data = repo_data_dir(app, state)?;
    let app = app.clone();
    let root = path.clone();
    Ok(state.tasks.start(&path, kind, title, move |ctx| {
        let result = work(&Repo {
            ctx,
            path: &root,
            data: &data,
        });
        repo_changed(&app);
        result
    }))
}

// A read with `Repo`: it changes nothing, so it does not announce that the repository changed.
fn read<T: Serialize + 'static>(
    app: &AppHandle,
    state: &State<'_, AppState>,
    title: &str,
    work: impl FnOnce(&Repo) -> Result<T, GitError> + Send + 'static,
) -> Result<TaskId, ApiError> {
    read_as(app, state, TaskKind::Read, title, work)
}

// A read that waits for the repository's writes, as they do: what it reads is never half written.
fn read_locked<T: Serialize + 'static>(
    app: &AppHandle,
    state: &State<'_, AppState>,
    title: &str,
    work: impl FnOnce(&Repo) -> Result<T, GitError> + Send + 'static,
) -> Result<TaskId, ApiError> {
    read_as(app, state, TaskKind::Write, title, work)
}

fn read_as<T: Serialize + 'static>(
    app: &AppHandle,
    state: &State<'_, AppState>,
    kind: TaskKind,
    title: &str,
    work: impl FnOnce(&Repo) -> Result<T, GitError> + Send + 'static,
) -> Result<TaskId, ApiError> {
    let path = current_repo(state)?;
    let data = repo_data_dir(app, state)?;
    let root = path.clone();
    Ok(state
        .tasks
        .start(&root, kind, title.to_string(), move |ctx| {
            work(&Repo {
                ctx,
                path: &path,
                data: &data,
            })
        }))
}

#[tauri::command]
pub async fn branch_checkout(
    target: String,
    mode: CheckoutMode,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let title = format!("Checkout {target}");
    start(&app, &state, TaskKind::Write, title, move |repo| {
        branches::checkout(repo, &target, mode)
    })
}

#[tauri::command]
pub async fn branch_checkout_remote(
    remote_branch: String,
    name: Option<String>,
    suggested: Option<String>,
    drop_commits: bool,
    mode: CheckoutMode,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let title = format!("Checkout {remote_branch}");
    start(&app, &state, TaskKind::Write, title, move |repo| {
        let options = RemoteOptions {
            name: name.as_deref(),
            suggested: suggested.as_deref(),
            drop_commits,
            mode,
        };
        branches::checkout_remote(repo, &remote_branch, options)
    })
}

#[tauri::command]
pub async fn branch_rebase_onto(
    start_point: String,
    branch: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let title = format!("Rebasing {branch}");
    start(&app, &state, TaskKind::Write, title, move |repo| {
        rebase::rebase_onto(repo, &start_point, &branch)
    })
}

// `Merge 'x' into 'y'` of the branches popup; `kind` is `local`, `remote` or `tag`. With `smart`, the Smart Merge of `Git Merge Problem`.
#[tauri::command]
pub async fn branch_merge(
    branch: String,
    kind: String,
    smart: bool,
    expected_head: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    // `branch.merging.process`.
    let title = format!("Merging {branch}…");
    start(&app, &state, TaskKind::Write, title, move |repo| {
        merge::merge_into(repo, &branch, &kind, smart, &expected_head)
    })
}

// `Rebase 'y' onto 'x'` (without `branch`) and `Checkout and Rebase onto 'y'` of a local one (`onto` is HEAD).
#[tauri::command]
pub async fn branch_rebase(
    onto: String,
    branch: Option<String>,
    expected_head: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let title = match &branch {
        // `branches.checkout.and.rebase.onto.current.process`.
        Some(branch) => format!("Checking out {branch} out and rebasing onto current"),
        // `branch.rebasing.onto.process`.
        None => format!("Rebasing onto {onto}…"),
    };
    start(&app, &state, TaskKind::Write, title, move |repo| {
        rebase::branch_rebase(repo, &onto, branch.as_deref(), &expected_head)
    })
}

// `Checkout and Rebase onto 'y'` of a remote one: the local one comes from it and is rebased onto the current one.
#[tauri::command]
pub async fn branch_rebase_remote(
    remote_branch: String,
    name: Option<String>,
    drop_commits: bool,
    expected_head: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let title = format!("Checking out {remote_branch} out and rebasing onto current");
    start(&app, &state, TaskKind::Write, title, move |repo| {
        rebase::rebase_remote(
            repo,
            &remote_branch,
            name.as_deref(),
            drop_commits,
            &expected_head,
        )
    })
}

// The remote protected branch that already has commits of those the rebase would rewrite. Without `branch`, the
// current one; without it (detached HEAD) there is nothing to ask.
#[tauri::command]
pub async fn rebase_published(
    upstream: Option<String>,
    branch: Option<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    let path = current_repo(&state)?;
    let data = repo_data_dir(&app, &state)?;
    let title = LOADING_COMMITS.to_string();
    let root = path.clone();
    Ok(state.tasks.start(&root, TaskKind::Read, title, move |ctx| {
        let repo = Repo {
            ctx,
            path: &path,
            data: &data,
        };
        let branch = match branch {
            Some(branch) => branch,
            None => match branches::current_name(&path)? {
                Some(branch) => branch,
                None => return Ok(None),
            },
        };
        rebase::published(&repo, upstream.as_deref(), &branch)
    }))
}

// `Git > Merge…`. `expected_head` is the HEAD the dialog saw: if it no longer matches, nothing is touched.
#[tauri::command]
pub async fn git_merge(
    branch: String,
    options: Vec<MergeOption>,
    message: Option<String>,
    expected_head: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let paths = current_settings(&app)?.update_paths;
    // `merging.title`.
    let title = format!("Merging changes to {}", current_repo(&state)?.display());
    start(&app, &state, TaskKind::Write, title, move |repo| {
        let outcome = merge::merge(repo, &branch, &options, message.as_deref(), &expected_head)?;
        Ok(merge::with_matching(repo.ctx, outcome, &paths))
    })
}

// `Git > Rebase…`. Like `git_merge`, it only acts if HEAD is still the one the dialog saw.
#[tauri::command]
pub async fn git_rebase(
    params: RebaseParams,
    expected_head: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    start(
        &app,
        &state,
        TaskKind::Write,
        REBASING.to_string(),
        move |repo| rebase::rebase(repo, &params, &expected_head),
    )
}

// What a merge with conflicts brought, after resolving and committing it, from the HEAD there was before.
#[tauri::command]
pub async fn merge_summary(
    branch: String,
    before_head: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    let paths = current_settings(&app)?.update_paths;
    let path = current_repo(&state)?;
    let title = LOADING_COMMITS.to_string();
    Ok(state.tasks.start(&path, TaskKind::Read, title, move |ctx| {
        let outcome = merge::summary(ctx, &before_head, &branch)?;
        Ok(merge::with_matching(ctx, outcome, &paths))
    }))
}

// The branches that are not yet in HEAD: the Merge… dialog warns about those that already are.
#[tauri::command]
pub async fn unmerged_branches(state: State<'_, AppState>) -> Result<TaskId, ApiError> {
    let path = current_repo(&state)?;
    let title = LOADING_UNMERGED.to_string();
    Ok(state
        .tasks
        .start(&path, TaskKind::Read, title, merge::unmerged_branches))
}

// Which of these revisions git resolves: the validation of the Rebase… dialog.
#[tauri::command]
pub async fn revisions_valid(
    revisions: Vec<String>,
    state: State<'_, AppState>,
) -> Result<Vec<bool>, ApiError> {
    let repo = current_repo(&state)?;
    blocking_api(move || Ok(rebase::valid_revisions(&repo, &revisions)?)).await
}

#[tauri::command]
pub async fn operation_continue(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let title = "Continue Rebase".to_string();
    start(
        &app,
        &state,
        TaskKind::Write,
        title,
        rebase::continue_rebase,
    )
}

#[tauri::command]
pub async fn operation_skip(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let title = "Skip commit during rebase…".to_string();
    start(&app, &state, TaskKind::Write, title, rebase::skip)
}

// `expected_head` is the HEAD the confirmation dialog saw: if it no longer matches, nothing is touched.
#[tauri::command]
pub async fn operation_abort(
    kind: Operation,
    expected_head: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let title = format!("Aborting {} Process", operation::name(kind));
    start(&app, &state, TaskKind::Write, title, move |repo| {
        operation::abort_expecting(repo, kind, &expected_head)
    })
}

// The conflicts and the three versions of one only read: they do not announce changes.
#[tauri::command]
pub async fn conflicts_session(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    read(&app, &state, LOADING_CONFLICTS, conflicts::session)
}

#[tauri::command]
pub async fn conflict_sides(
    path: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    read(&app, &state, LOADING_REVISIONS, move |repo| {
        conflicts::sides(repo, &path)
    })
}

// The commits of a range of the merge window's `Show Details` (`SideDetails::Range`).
#[tauri::command]
pub async fn merge_range_commits(
    from: String,
    to: String,
    path: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    read(&app, &state, COLLECTING_DETAILS, move |repo| {
        conflicts::range_commits(repo, &from, &to, &path)
    })
}

// `side` is the user's, as they see it in the window: `accept` is what inverts it in a rebase.
#[tauri::command]
pub async fn conflicts_accept(
    paths: Vec<String>,
    side: conflicts::Side,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let title = RESOLVING_CONFLICTS.to_string();
    start(&app, &state, TaskKind::Write, title, move |repo| {
        conflicts::accept(repo, &paths, side)
    })
}

// `expected_hash` is the one from `conflict_sides`: if the file changed since then, it is not written.
#[tauri::command]
pub async fn conflict_resolve(
    path: String,
    content: String,
    expected_hash: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let title = RESOLVING_CONFLICTS.to_string();
    start(&app, &state, TaskKind::Write, title, move |repo| {
        conflicts::resolve(repo, &path, content.as_bytes(), &expected_hash).map_err(GitError::from)
    })
}

// The stash. An entry goes by its hash: `stash` finds its `stash@{n}` when it acts. The list, the
// changes and the sides only read (`stash list` and `stash show` are reading forms for the console),
// so they do not wait for the repository's lock.
#[tauri::command]
pub async fn stash_list(app: AppHandle, state: State<'_, AppState>) -> Result<TaskId, ApiError> {
    read(&app, &state, LOADING_STASHES, |repo| stash::list(repo.ctx))
}

#[tauri::command]
pub async fn stash_changes(
    sha: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    read(&app, &state, LOADING_STASH, move |repo| {
        stash::changes(repo.ctx, &sha)
    })
}

#[tauri::command]
pub async fn stash_file_sides(
    sha: String,
    path: String,
    old_path: Option<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    read(&app, &state, LOADING_REVISIONS, move |repo| {
        stash::file_sides(repo.ctx, &sha, &path, old_path.as_deref())
    })
}

// `Stash Changes…` and `Stash Silently` (an empty message).
#[tauri::command]
pub async fn stash_save(
    message: String,
    keep_index: bool,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    start(
        &app,
        &state,
        TaskKind::Write,
        STASHING.to_string(),
        move |repo| stash::save(repo.ctx, &message, keep_index),
    )
}

// `Stash Files` of the staging area.
#[tauri::command]
pub async fn stash_push_paths(
    paths: Vec<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let title = STASHING_FILES.to_string();
    start(&app, &state, TaskKind::Write, title, move |repo| {
        stash::push_paths(repo.ctx, &paths)
    })
}

// `expected_head` is the HEAD the dialog saw. A recoverable copy is taken before anything is written.
#[tauri::command]
pub async fn stash_unstash(
    sha: String,
    pop: bool,
    reinstate_index: bool,
    branch: Option<String>,
    expected_head: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    start(
        &app,
        &state,
        TaskKind::Write,
        UNSTASHING.to_string(),
        move |repo| {
            stash::unstash(
                repo,
                &sha,
                pop,
                reinstate_index,
                branch.as_deref(),
                &expected_head,
            )
        },
    )
}

#[tauri::command]
pub async fn stash_drop(
    sha: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let path = current_repo(&state)?;
    let looked = sha.clone();
    let label = blocking_api(move || Ok(stash::label_of(&path, &looked))).await?;
    let title = match label {
        Some(label) => format!("Removing Stash {label}…"),
        None => REMOVING_STASH.to_string(),
    };
    start(&app, &state, TaskKind::Write, title, move |repo| {
        stash::drop(repo, &sha)
    })
}

#[tauri::command]
pub async fn stash_clear(app: AppHandle, state: State<'_, AppState>) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    start(
        &app,
        &state,
        TaskKind::Write,
        CLEARING_STASHES.to_string(),
        stash::clear,
    )
}

// Apply Patch. Each command reads the patch again from its source: only paths and texts travel.
// The plan writes nothing; `mapping` is the dialog's strip and base directory.
#[tauri::command]
pub async fn patch_plan(
    source: PatchInput,
    mapping: Option<PathMapping>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    read_locked(&app, &state, LOADING_PATCH, move |repo| {
        crate::patch_apply::plan_input(repo.path, repo.data, &source, &mapping.unwrap_or_default())
    })
}

// `Show Difference` of the file at `index` of the plan, with the same mapping. It writes nothing.
#[tauri::command]
pub async fn patch_preview(
    source: PatchInput,
    mapping: Option<PathMapping>,
    index: usize,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    read_locked(&app, &state, LOADING_PATCH, move |repo| {
        crate::patch_apply::preview_input(
            repo.path,
            repo.data,
            &source,
            &mapping.unwrap_or_default(),
            index,
        )
    })
}

// `expected_head` is the HEAD the dialog saw: if it no longer matches, nothing is touched.
// `choice` has the selected files, the new files to write over and the plan's `hashes` (path to
// content hash, `absent`): a file that changed since the plan is not written. A recoverable copy
// is taken before anything is written.
#[tauri::command]
pub async fn patch_apply(
    source: PatchInput,
    mapping: PathMapping,
    choice: ApplyChoice,
    expected_head: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let title = APPLYING_PATCH.to_string();
    start(&app, &state, TaskKind::Write, title, move |repo| {
        crate::patch_apply::apply_input(repo, &source, &mapping, &choice, &expected_head)
    })
}

// The merge window's result for a conflict of `patch_apply`, written only over the content it
// was shown (`local_hash`); `moved_to` and `new_mode` come from the conflict.
#[tauri::command]
pub async fn patch_conflict_resolve(
    path: String,
    content: String,
    local_hash: String,
    moved_to: Option<String>,
    new_mode: Option<u32>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let title = RESOLVING_CONFLICTS.to_string();
    start(&app, &state, TaskKind::Write, title, move |repo| {
        crate::patch_apply::resolve_conflict(
            repo.path,
            &path,
            &content,
            &local_hash,
            moved_to.as_deref(),
            new_mode,
        )
    })
}

// `Rollback` of `Patch Applying Partly Failed` and `Abort and Rollback`: the copy `patch_apply`
// took. A copy of the current state is taken first.
#[tauri::command]
pub async fn patch_rollback(
    snapshot: SnapshotId,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let title = ROLLING_BACK_PATCH.to_string();
    start(&app, &state, TaskKind::Write, title, move |repo| {
        crate::patch_apply::rollback(repo.data, repo.path, &snapshot)
    })
}

// The commit of a merge that has just been resolved (`GitMerger.mergeCommit`).
#[tauri::command]
pub async fn merge_commit(app: AppHandle, state: State<'_, AppState>) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let title = COMMITTING_MERGE.to_string();
    start(&app, &state, TaskKind::Write, title, commit::merge_commit)
}

// Cherry-Pick and Revert Commit of the log. `hashes` are in the log's order; the cherry-pick applies them
// from the oldest to the most recent.
#[tauri::command]
pub async fn apply_commits(
    kind: Kind,
    hashes: Vec<String>,
    expected_head: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let title = match kind {
        // `cherry.picking.process`.
        Kind::CherryPick => "Cherry-Picking".to_string(),
        // `progress.title.reverting.n.commits`.
        Kind::Revert if hashes.len() == 1 => "Reverting commit".to_string(),
        Kind::Revert => "Reverting commits".to_string(),
    };
    start(&app, &state, TaskKind::Write, title, move |repo| {
        apply_commits::run(repo, kind, &hashes, &expected_head)
    })
}

// The commit of a cherry-pick or a revert whose conflicts have already been resolved.
#[tauri::command]
pub async fn apply_commit_after_conflicts(
    kind: Kind,
    hash: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    start(
        &app,
        &state,
        TaskKind::Write,
        COMMITTING.to_string(),
        move |repo| apply_commits::commit_after_conflicts(repo, kind, &hash),
    )
}

// Cherry-Pick Selected Changes and Revert Selected Changes of the log's changes panel. The task's result
// is a `PathsResult`; moving the changes to the chosen changelist is done by the caller.
#[tauri::command]
pub async fn apply_paths(
    selection: Vec<Selection>,
    reverse: bool,
    expected_head: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    // `changes.progress.title.choice.revert.apply.changes`.
    let title = if reverse {
        "Revert Changes"
    } else {
        "Apply Changes"
    };
    start(
        &app,
        &state,
        TaskKind::Write,
        title.to_string(),
        move |repo| -> Result<PathsResult, GitError> {
            apply_commits::apply_selection(repo, &expected_head, &selection, reverse)
        },
    )
}

// `ChangesView.AddUnversioned` with changelists. `repo` is the repository the interface saw: the files are
// not added to another one opened in the meantime. The task's result is the snapshot of what was only in
// the stage, if there was any.
#[tauri::command]
pub async fn changes_add(
    repo: String,
    paths: Vec<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    if current_repo(&state)? != Path::new(&repo) {
        return Err(failed(
            "Another repository was opened before the files were added".to_string(),
        ));
    }
    start(
        &app,
        &state,
        TaskKind::Write,
        ADDING_FILES.to_string(),
        move |repo| -> Result<Option<SnapshotId>, GitError> { staging::add_to_vcs(repo, &paths) },
    )
}

// What the log menu needs to know about Edit Commit Message, Drop and Squash for the selection.
#[tauri::command]
pub async fn rewrite_menu(
    hashes: Vec<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    read(
        &app,
        &state,
        CHECKING_EDITING,
        move |repo| -> Result<MenuChecks, GitError> { Ok(rewrite::menu_checks(repo, &hashes)) },
    )
}

// `lastCheckCommitsEditingAvailability`: what is looked at on click, with the history walked.
#[tauri::command]
pub async fn rewrite_check(
    hashes: Vec<String>,
    kind: rewrite::Kind,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    read(
        &app,
        &state,
        CHECKING_EDITING,
        move |repo| -> Result<Option<String>, GitError> {
            Ok(rewrite::check(repo, &hashes, kind).err())
        },
    )
}

// Undo Commit…, Fixup… and Squash Into… on click: the reason why it cannot be done, or `None`.
#[tauri::command]
pub async fn rewrite_check_single(
    hash: String,
    action: rewrite::Single,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    read(
        &app,
        &state,
        CHECKING_EDITING,
        move |repo| -> Result<Option<String>, GitError> {
            Ok(rewrite::check_single(repo, &hash, action).err())
        },
    )
}

// Undo Commit… of the log: `reset --soft` to the parent. The task's result is the commit's paths;
// moving them to the chosen changelist is done by the caller.
#[tauri::command]
pub async fn rewrite_uncommit(
    hash: String,
    expected_head: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    // `git.undo.action.undoing.last.commit.process`.
    start(
        &app,
        &state,
        TaskKind::Write,
        "Undoing Last Commit…".to_string(),
        move |repo| rewrite::uncommit(repo, &hash, &expected_head),
    )
}

// The rebase of `Commit and Rebase…`, after the commit. The task's result is a `Rebased`.
#[tauri::command]
pub async fn rewrite_autosquash(
    base: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    // `rebase.progress.indicator.title`.
    start(
        &app,
        &state,
        TaskKind::Write,
        "Rebasing…".to_string(),
        move |repo| rewrite::autosquash(repo, &base),
    )
}

// Interactively Rebase from Here…: the base and, if the log is good for generating the list, the rows of the
// dialog. The task's result is an `InteractiveStart`.
#[tauri::command]
pub async fn rewrite_interactive_start(
    hash: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    // `rebase.progress.indicator.preparing.title`.
    read(&app, &state, "Preparing to rebase…", move |repo| {
        rewrite::interactive_start(repo, &hash)
    })
}

// The interactive rebase from the commit: with `plan`, what the dialog chose over the log's list;
// without it, git generates the list and the window shows it. The task's result is a `Rebased`.
#[tauri::command]
pub async fn rewrite_interactive(
    hash: String,
    plan: Option<rewrite::Plan>,
    expected_head: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    start(
        &app,
        &state,
        TaskKind::Write,
        REBASING.to_string(),
        move |repo| rewrite::interactive(repo, &hash, plan, &expected_head),
    )
}

// What the message dialog checks on pressing OK: the error text, or `None`.
#[tauri::command]
pub async fn rewrite_validate(
    hashes: Vec<String>,
    expected_head: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    read(&app, &state, CHECKING_EDITING, move |repo| {
        rewrite::dialog_problem(repo, &hashes, &expected_head)
    })
}

// Edit Commit Message of the log. The task's result is a `Rewritten`.
#[tauri::command]
pub async fn rewrite_reword(
    hash: String,
    message: String,
    expected_head: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    // `rebase.log.reword.action.progress.indicator.title`.
    start(
        &app,
        &state,
        TaskKind::Write,
        "Rewording…".to_string(),
        move |repo| rewrite::reword(repo, &hash, &message, &expected_head),
    )
}

// Drop Commits of the log, without confirmation like IntelliJ.
#[tauri::command]
pub async fn rewrite_drop(
    hashes: Vec<String>,
    expected_head: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    // `rebase.log.drop.progress.indicator.title`.
    let title = if hashes.len() == 1 {
        "Dropping commit…"
    } else {
        "Dropping commits…"
    };
    start(
        &app,
        &state,
        TaskKind::Write,
        title.to_string(),
        move |repo| rewrite::drop(repo, &hashes, &expected_head),
    )
}

// Squash Commits of the log, with the dialog's message.
#[tauri::command]
pub async fn rewrite_squash(
    hashes: Vec<String>,
    message: String,
    expected_head: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    // `rebase.log.squash.progress.indicator.title`.
    start(
        &app,
        &state,
        TaskKind::Write,
        "Squashing…".to_string(),
        move |repo| rewrite::squash(repo, &hashes, &message, &expected_head),
    )
}

// The Undo of the success notification. The task's result is an `Undone`.
#[tauri::command]
pub async fn rewrite_undo(
    kind: rewrite::Kind,
    old_head: String,
    new_head: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    // `rebase.log.reword.action.progress.indicator.undo.title` and its siblings.
    let title = match kind {
        rewrite::Kind::Reword => "Undoing reword…",
        rewrite::Kind::Drop => "Undoing drop…",
        rewrite::Kind::Squash => "Undoing squash…",
    };
    start(
        &app,
        &state,
        TaskKind::Write,
        title.to_string(),
        move |repo| -> Result<Undone, GitError> {
            rewrite::undo_reporting(repo, &Rewritten::between(old_head, new_head))
        },
    )
}

// Whether the Undo is still valid: the notification expires when it no longer is.
#[tauri::command]
pub async fn rewrite_undo_check(
    old_head: String,
    new_head: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    read(&app, &state, CHECKING_EDITING, move |repo| {
        rewrite::undo_impossible(repo, &Rewritten::between(old_head, new_head))
    })
}

// The message proposed for the next commit: IntelliJ's if there is a half-done cherry-pick or revert,
// and otherwise the one git left (merge or squash). It reads: it changes nothing.
#[tauri::command]
pub async fn merge_message(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<String>, ApiError> {
    let repo = current_repo(&state)?;
    let data = repo_data_dir(&app, &state)?;
    let suffix = current_settings(&app)?.cherry_pick_suffix;
    blocking_api(move || {
        if let Some(text) = apply_commits::proposal_in_progress(&repo, &data, suffix)? {
            return Ok(Some(text));
        }
        Ok(commit::merge_message(&repo)?)
    })
    .await
}

// The blocks of the three texts with another whitespace policy, without rereading them: it does not touch the
// repository.
#[tauri::command]
pub async fn merge_blocks(
    left: String,
    base: String,
    right: String,
    policy: Whitespace,
) -> Result<Vec<Block>, ApiError> {
    blocking_api(move || Ok(merge3::blocks(&left, &base, &right, policy))).await
}

// What is written from the diff window while it saves: like `file_write_locked`, without
// going through `flush_diff_window`, which would wait for that very save.
pub fn index_write_locked(
    locks: &RepoLocks,
    repo: &Path,
    dir: &Path,
    path: &str,
    content: &str,
    expected_hash: &str,
) -> Result<Written, ApiError> {
    locks.run(repo, || {
        commit::ensure_idle(repo, dir)?;
        Ok(worktree_diff::write_index_copying(
            repo,
            dir,
            path,
            content,
            expected_hash,
        )?)
    })
}

pub fn revert_range_locked(
    locks: &RepoLocks,
    repo: &Path,
    dir: &Path,
    path: &str,
    content: &str,
    expected_hash: &str,
) -> Result<Written, ApiError> {
    locks.run(repo, || {
        commit::ensure_idle(repo, dir)?;
        Ok(worktree_diff::write_reverting(
            repo,
            dir,
            path,
            content,
            expected_hash,
        )?)
    })
}

// `Compare HEAD, Staged and Local Versions`. It reads: it changes nothing.
#[tauri::command]
pub async fn staging_three_sides(
    path: String,
    state: State<'_, AppState>,
) -> Result<ThreeSides, ApiError> {
    let repo = current_repo(&state)?;
    blocking_api(move || Ok(worktree_diff::three_sides(&repo, &path)?)).await
}

// The Staged panel saved, or a `Stage` or `Unstage` arrow. `expected_hash` is the blob the entry had
// when it was read: if it changed, it is not written.
#[tauri::command]
pub async fn index_write(
    path: String,
    content: String,
    expected_hash: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Written, ApiError> {
    let repo = current_repo(&state)?;
    let dir = repo_data_dir(&app, &state)?;
    let locks = state.operations.clone();
    blocking_api(move || index_write_locked(&locks, &repo, &dir, &path, &content, &expected_hash))
        .await
}

// The `Revert` arrow: the Local panel saved after a copy of the file.
#[tauri::command]
pub async fn staging_revert_range(
    path: String,
    content: String,
    expected_hash: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Written, ApiError> {
    let repo = current_repo(&state)?;
    let dir = repo_data_dir(&app, &state)?;
    let locks = state.operations.clone();
    blocking_api(move || revert_range_locked(&locks, &repo, &dir, &path, &content, &expected_hash))
        .await
}

#[tauri::command]
pub async fn branch_create(
    name: String,
    start_point: Option<String>,
    checkout: bool,
    overwrite: Overwrite,
    mode: CheckoutMode,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let title = format!("New Branch {name}");
    start(&app, &state, TaskKind::Write, title, move |repo| {
        branches::create(
            repo,
            &name,
            start_point.as_deref(),
            checkout,
            overwrite,
            mode,
        )
    })
}

#[tauri::command]
pub async fn branch_rename(
    old: String,
    new: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let title = format!("Rename {old}");
    start(&app, &state, TaskKind::Write, title, move |repo| {
        branches::rename(repo, &old, &new)
    })
}

#[tauri::command]
pub async fn branch_delete(
    name: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let title = format!("Deleting {name}");
    start(&app, &state, TaskKind::Write, title, move |repo| {
        branches::delete(repo, &name)
    })
}

#[tauri::command]
pub async fn branch_restore(
    deleted: Deleted,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let title = format!("Restoring Branch {}…", deleted.name);
    start(&app, &state, TaskKind::Write, title, move |repo| {
        branches::restore(repo, &deleted)
    })
}

fn remote_not_found(remote_branch: &str) -> String {
    format!("Could not find remote by name: {remote_branch}")
}

// It talks to a remote, without touching the index: a commit does not have to wait.
#[tauri::command]
pub async fn branch_delete_remote(
    remote_branch: String,
    also_local: bool,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let path = current_repo(&state)?;
    let name = remote_branch.clone();
    let split = blocking_api(move || Ok(branches::split_remote(&path, &name)?)).await?;
    let (remote, branch) =
        split.ok_or_else(|| failed(remote_not_found(&remote_branch)))?;
    let title = format!("Deleting {remote_branch}");
    let kind = TaskKind::Remote(remote.clone());
    start(
        &app,
        &state,
        kind,
        title,
        move |repo| -> Result<DeletedRemote, GitError> {
            branches::delete_remote(repo, &remote, &branch, also_local)
        },
    )
}

#[tauri::command]
pub async fn branch_reset(
    target: String,
    mode: ResetMode,
    how: CheckoutMode,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let title = "Git Reset".to_string();
    start(
        &app,
        &state,
        TaskKind::Write,
        title,
        move |repo| -> Result<CheckoutOutcome, GitError> {
            branches::reset(repo, &target, mode, how)
        },
    )
}

#[tauri::command]
pub async fn tag_create(
    name: String,
    target: String,
    message: String,
    force: bool,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let title = "Tagging…".to_string();
    start(&app, &state, TaskKind::Write, title, move |repo| {
        branches::tag(repo, &name, &target, &message, force)
    })
}

#[tauri::command]
pub async fn tag_delete(
    name: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let title = format!("Deleting {name}");
    start(
        &app,
        &state,
        TaskKind::Write,
        title,
        move |repo| -> Result<DeletedTag, GitError> { branches::delete_tag(repo, &name) },
    )
}

#[tauri::command]
pub async fn tag_restore(
    deleted: DeletedTag,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let title = format!("Restoring Tag {}…", deleted.name);
    start(&app, &state, TaskKind::Write, title, move |repo| {
        branches::restore_tag(repo, &deleted)
    })
}

// Each remote takes its turn inside the task, like `TaskKind::Remote`: there are several, and a commit does not
// have to wait for any.
#[tauri::command]
pub async fn fetch_all(app: AppHandle, state: State<'_, AppState>) -> Result<TaskId, ApiError> {
    let handle = app.clone();
    start(
        &app,
        &state,
        TaskKind::Read,
        FETCHING.to_string(),
        move |repo| -> Result<FetchResult, GitError> {
            fetch::fetch_all(repo.ctx, &handle.state::<AppState>().branch_sync)
        },
    )
}

// The remote of the Pull dialog, to load its branches. Like `fetch_all`, the remote's turn is taken by
// `fetch_remote` inside the task.
#[tauri::command]
pub async fn fetch_remote(
    remote: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    let handle = app.clone();
    start(
        &app,
        &state,
        TaskKind::Read,
        FETCHING.to_string(),
        move |repo| -> Result<Vec<String>, GitError> {
            pull::check_remote(repo.ctx, &remote)?;
            fetch::fetch_remote(repo.ctx, &handle.state::<AppState>().branch_sync, &remote)
        },
    )
}

#[tauri::command]
pub async fn pull_info(state: State<'_, AppState>) -> Result<PullInfo, ApiError> {
    let repo = current_repo(&state)?;
    blocking_api(move || Ok(pull::info(&repo)?)).await
}

// `git pull` changes the tree from start to finish and takes the write lock for it, and the
// protection loader asks GitHub after it, without the lock (`pull::merge`). With `--rebase` it is
// Update, which takes it only from saving to restoring: see `update_project`.
#[tauri::command]
pub async fn git_pull(
    remote: String,
    branch: String,
    options: Vec<PullOption>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let paths = current_settings(&app)?.update_paths;
    let handle = app.clone();
    let title = format!("Pulling changes from {remote}");
    start(
        &app,
        &state,
        TaskKind::Read,
        title,
        move |repo| -> Result<UpdateResult, GitError> {
            let state = handle.state::<AppState>();
            let flush = || {
                tauri::async_runtime::block_on(flush_diff_window(&handle, &state, SAVE_ACTION))
                    .map_err(|error| GitError::Io(error.message))
            };
            let result = pull::pull(repo, &state.branch_sync, &remote, &branch, &options, flush);
            Ok(update::with_matching(repo.ctx, result, &paths))
        },
    )
}

#[tauri::command]
pub async fn push_info(
    branch: Option<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<PushInfo, ApiError> {
    let repo = current_repo(&state)?;
    let data = repo_data_dir(&app, &state)?;
    // The dialog greys out what `check_force` refuses: the user's patterns and GitHub's.
    let repo_settings = RepoSettings {
        protected_branches: settings::effective_protected(&data),
        ..settings::load_repo(&data)
    };
    blocking_api(move || Ok(push::info(&repo, repo_settings, branch.as_deref())?)).await
}

// The branch to push a commit from: `Push All up to Here`.
#[tauri::command]
pub async fn push_branch_of(
    hash: String,
    state: State<'_, AppState>,
) -> Result<Option<String>, ApiError> {
    let repo = current_repo(&state)?;
    blocking_api(move || Ok(push::branch_of_commit(&repo, &hash)?)).await
}

// A task, because it validates with `git check-ref-format` what the user writes. It does not announce changes:
// it only reads, and is launched every time the destination changes.
#[tauri::command]
pub async fn push_outgoing(
    source: String,
    remote: String,
    target: String,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    let path = current_repo(&state)?;
    let title = LOADING_COMMITS.to_string();
    Ok(state.tasks.start(&path, TaskKind::Read, title, move |ctx| {
        push::outgoing(ctx, &source, &remote, &target)
    }))
}

#[tauri::command]
pub async fn git_push(
    spec: PushSpec,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let handle = app.clone();
    start(
        &app,
        &state,
        push::task_kind(&spec),
        PUSHING.to_string(),
        move |repo| -> Result<PushResult, GitError> {
            push::push(repo, &handle.state::<AppState>().branch_sync, &spec)
        },
    )
}

// The retry of a rejected push. Like `update_project`, without a task lock: Update takes it
// from saving to restoring, and the push its remote's turn.
#[tauri::command]
pub async fn git_push_update(
    spec: PushSpec,
    method: UpdateMethod,
    options: UpdateOptions,
    attempts: u32,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let paths = current_settings(&app)?.update_paths;
    let handle = app.clone();
    start(
        &app,
        &state,
        TaskKind::Read,
        PUSHING.to_string(),
        move |repo| -> Result<UpdatedPush, GitError> {
            let state = handle.state::<AppState>();
            let flush = || {
                tauri::async_runtime::block_on(flush_diff_window(&handle, &state, SAVE_ACTION))
                    .map_err(|error| GitError::Io(error.message))
            };
            let sync = &state.branch_sync;
            let result = push::update_and_push(repo, sync, &spec, method, options, attempts, flush);
            Ok(UpdatedPush {
                update: result
                    .update
                    .map(|update| update::with_matching(repo.ctx, update, &paths)),
                ..result
            })
        },
    )
}

// Without a task lock: `update_project` takes it only from saving to restoring, so that a slow
// fetch does not stop the commits. What was written in the diff during that fetch is saved beforehand.
#[tauri::command]
pub async fn update_project(
    method: UpdateMethod,
    options: UpdateOptions,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let paths = current_settings(&app)?.update_paths;
    let handle = app.clone();
    start(
        &app,
        &state,
        TaskKind::Read,
        UPDATING.to_string(),
        move |repo| -> Result<UpdateResult, GitError> {
            let state = handle.state::<AppState>();
            let flush = || {
                tauri::async_runtime::block_on(flush_diff_window(&handle, &state, SAVE_ACTION))
                    .map_err(|error| GitError::Io(error.message))
            };
            let result = update::update_project(repo, &state.branch_sync, method, options, flush);
            Ok(update::with_matching(repo.ctx, result, &paths))
        },
    )
}

// `Pull into 'y' Using Merge` and `Using Rebase`: an Update towards the chosen remote. Like
// `update_project`, the task does not take the lock: Update asks for it from saving to restoring.
#[tauri::command]
pub async fn branch_pull_into(
    remote_branch: String,
    method: UpdateMethod,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let paths = current_settings(&app)?.update_paths;
    let handle = app.clone();
    start(
        &app,
        &state,
        TaskKind::Read,
        UPDATING.to_string(),
        move |repo| -> Result<UpdateResult, GitError> {
            let state = handle.state::<AppState>();
            let flush = || {
                tauri::async_runtime::block_on(flush_diff_window(&handle, &state, SAVE_ACTION))
                    .map_err(|error| GitError::Io(error.message))
            };
            let sync = &state.branch_sync;
            let result = pull::pull_into(repo, sync, &remote_branch, method, flush);
            Ok(update::with_matching(repo.ctx, result, &paths))
        },
    )
}

// It does not touch the directory: it talks to the remote and moves a branch that is not the current one.
#[tauri::command]
pub async fn update_branch(
    local: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let path = current_repo(&state)?;
    let name = local.clone();
    let tracked = blocking_api(move || Ok(update::tracked_branch(&path, &name)?)).await?;
    let (remote, branch) =
        tracked.ok_or_else(|| failed(format!("Tracked branch is not configured for {local}")))?;
    let handle = app.clone();
    let kind = TaskKind::Remote(remote.clone());
    start(
        &app,
        &state,
        kind,
        UPDATING_BRANCHES.to_string(),
        move |repo| -> Result<(), GitError> {
            let sync = &handle.state::<AppState>().branch_sync;
            update::update_branch(repo.ctx, sync, &local, &remote, &branch)
        },
    )
}

#[tauri::command]
pub async fn update_abort(app: AppHandle, state: State<'_, AppState>) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    start(
        &app,
        &state,
        TaskKind::Write,
        "Aborting…".to_string(),
        |repo| -> Result<Aborted, GitError> { update::abort(repo) },
    )
}

// Decision 5: that stash entry, by its hash.
#[tauri::command]
pub async fn update_restore(
    saved: Saved,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    start(
        &app,
        &state,
        TaskKind::Write,
        "Restoring Changes…".to_string(),
        move |repo| -> Result<RestoreOutcome, GitError> {
            Ok(update::restore_requested(repo, &saved))
        },
    )
}

#[tauri::command]
pub async fn branch_current(state: State<'_, AppState>) -> Result<Option<String>, ApiError> {
    let repo = current_repo(&state)?;
    blocking_api(move || Ok(branches::current_name(&repo)?)).await
}

#[tauri::command]
pub async fn branch_trackers(
    remote_branch: String,
    state: State<'_, AppState>,
) -> Result<Vec<String>, ApiError> {
    let repo = current_repo(&state)?;
    blocking_api(move || Ok(branches::trackers(&repo, &remote_branch)?)).await
}

#[tauri::command]
pub async fn branch_unmerged_commits(
    base: String,
    tip: String,
    state: State<'_, AppState>,
) -> Result<Vec<LostCommit>, ApiError> {
    let repo = current_repo(&state)?;
    blocking_api(move || Ok(branches::unmerged_commits(&repo, &base, &tip)?)).await
}

#[tauri::command]
pub async fn branch_list(state: State<'_, AppState>) -> Result<BranchList, ApiError> {
    let repo = current_repo(&state)?;
    blocking_api(move || Ok(branch_info::list(&repo)?)).await
}

#[tauri::command]
pub async fn branch_incoming_outgoing(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<HashMap<String, Sync>, ApiError> {
    let repo = current_repo(&state)?;
    let strategy = current_settings(&app)?.incoming_check;
    let seen = state.branch_sync.seen(&repo);
    blocking_api(move || Ok(branch_info::incoming_outgoing(&repo, strategy, &seen)?)).await
}

#[tauri::command]
pub async fn remote_list(state: State<'_, AppState>) -> Result<Vec<Remote>, ApiError> {
    let repo = current_repo(&state)?;
    blocking_api(move || Ok(remotes::list(&repo)?)).await
}

// The three change only the configuration: like `fetch`, they take their remote's turn and do not wait for
// the commits.
#[tauri::command]
pub async fn remote_add(
    name: String,
    url: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let kind = TaskKind::Remote(name.clone());
    start(&app, &state, kind, ADDING_REMOTE.to_string(), move |repo| {
        remotes::add(repo.ctx, &name, &url)
    })
}

#[tauri::command]
pub async fn remote_edit(
    name: String,
    new_name: String,
    url: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let kind = TaskKind::Remote(name.clone());
    start(
        &app,
        &state,
        kind,
        CHANGING_REMOTE.to_string(),
        move |repo| remotes::edit(repo.ctx, &name, &new_name, &url),
    )
}

#[tauri::command]
pub async fn remote_remove(
    name: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let kind = TaskKind::Remote(name.clone());
    start(
        &app,
        &state,
        kind,
        REMOVING_REMOTE.to_string(),
        move |repo| remotes::remove(repo.ctx, &name),
    )
}

// `Configure GPG Key…`: the repository's signing config and the keys gpg can sign with. Both only
// read, and listing the keys runs gpg, never git.
#[tauri::command]
pub async fn gpg_config(state: State<'_, AppState>) -> Result<SigningConfig, ApiError> {
    let repo = current_repo(&state)?;
    blocking_api(move || Ok(gpg::config(&repo)?)).await
}

#[tauri::command]
pub async fn gpg_keys(state: State<'_, AppState>) -> Result<Vec<GpgKey>, ApiError> {
    let repo = current_repo(&state)?;
    blocking_api(move || {
        let program = gpg::program(&repo)?.ok_or_else(gpg::not_found)?;
        Ok(gpg::secret_keys(&program)?)
    })
    .await
}

// The `GPG Signature` column and the details: a read task of `git log --no-walk`, which changes
// nothing. Each table and details panel is a `requester`, and its new request cancels its previous one.
#[tauri::command]
pub async fn log_signatures(
    hashes: Vec<String>,
    requester: u32,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    let repo = current_repo(&state)?;
    Ok(state
        .signatures
        .start(&state.tasks, &repo, requester, hashes)?)
}

// It writes `.git/config`, which every worktree of the repository shares.
#[tauri::command]
pub async fn gpg_config_set(
    key: Option<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let title = CONFIGURING_GPG.to_string();
    start(&app, &state, TaskKind::Write, title, move |repo| {
        gpg::write(repo.ctx, key.as_deref())
    })
}

// `ls-remote` of a URL, with the name it is to be saved with if known.
#[tauri::command]
pub async fn remote_test_url(
    url: String,
    remote: Option<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    let handle = app.clone();
    let kind = remote.clone().map_or(TaskKind::Read, TaskKind::Remote);
    start(&app, &state, kind, TESTING_URL.to_string(), move |repo| {
        let sync = &handle.state::<AppState>().branch_sync;
        remotes::test_url(repo.ctx, sync, &url, remote.as_deref())
    })
}

// The folder of a task that does not belong to the open repository: in its parent, or in the directory itself.
// A leading `~` is the user's folder.
fn working_dir(path: &str) -> Result<PathBuf, ApiError> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let dir = remotes::expand_home(path, home.as_deref());
    if !dir.is_absolute() {
        return Err(failed(format!("The path \"{path}\" is not absolute")));
    }
    Ok(dir)
}

// `git clone` runs in the destination's parent and does not touch the open repository, so it neither saves the
// diff nor takes its locks. It is `Write` so that quitting cancels it and waits for git to remove what it
// left half-done.
#[tauri::command]
pub async fn clone_repository(
    url: String,
    directory: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    let dir = working_dir(&directory)?;
    remotes::create_parent(&dir).map_err(|error| {
        failed(format!(
            "Could not create the folder to clone into \"{directory}\": {error}"
        ))
    })?;
    let parent = dir
        .parent()
        .filter(|parent| parent.is_dir())
        .ok_or_else(|| {
            failed(format!(
                "The folder to clone into \"{directory}\" does not exist"
            ))
        })?
        .to_path_buf();
    let title = match dir.file_name() {
        Some(name) => format!("Cloning into {}…", name.to_string_lossy()),
        None => "Cloning…".to_string(),
    };
    let handle = app.clone();
    Ok(state.tasks.start(
        &parent,
        TaskKind::Write,
        title,
        move |ctx| -> Result<Cloned, GitError> {
            remotes::clone(ctx, &handle.state::<AppState>().branch_sync, &url, &dir)
        },
    ))
}

// Outside the open repository, like `clone_repository`. Without `confirmed`, a folder that is already
// under Git is not touched: the `underGit` result asks for IntelliJ's confirmation.
#[tauri::command]
pub async fn init_repository(
    path: String,
    confirmed: bool,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    let dir = working_dir(&path)?;
    if !dir.is_dir() {
        return Err(failed(format!("The folder \"{path}\" does not exist")));
    }
    Ok(state.tasks.start(
        &dir,
        TaskKind::Write,
        "Refreshing files".to_string(),
        move |ctx| -> Result<InitOutcome, GitError> { remotes::init(ctx, confirmed) },
    ))
}

// `true` if it opened the file; if it does not exist, it does nothing.
#[tauri::command]
pub async fn open_exclude_file(state: State<'_, AppState>) -> Result<bool, ApiError> {
    let repo = current_repo(&state)?;
    blocking_api(move || {
        Ok(remotes::open_exclude_file(
            &repo,
            remotes::open_in_text_editor,
        )?)
    })
    .await
}

// The app has one repository per window and does not yet open several: another window is another process.
#[tauri::command]
pub fn open_in_new_window(path: String) -> Result<(), ApiError> {
    remotes::open_in_new_window(&working_dir(&path)?).map_err(failed)
}

// As text: a `u64` does not fit in a JavaScript number.
#[tauri::command]
pub async fn branch_refs_fingerprint(state: State<'_, AppState>) -> Result<String, ApiError> {
    let repo = current_repo(&state)?;
    blocking_api(move || Ok(refs_fingerprint(&repo)?.to_string())).await
}

#[tauri::command]
pub fn favorites_get(app: AppHandle, state: State<'_, AppState>) -> Result<FavoriteSet, ApiError> {
    Ok(branch_info::favorites(&repo_data_dir(&app, &state)?))
}

#[tauri::command]
pub fn favorites_toggle(
    name: String,
    kind: BranchKind,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<FavoriteSet, ApiError> {
    branch_info::toggle_favorite(&repo_data_dir(&app, &state)?, &name, kind)
        .map_err(|error| failed(error.to_string()))
}

#[tauri::command]
pub fn pull_options_get(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<PullOption>, ApiError> {
    Ok(pull::read_options(&repo_data_dir(&app, &state)?))
}

#[tauri::command]
pub fn pull_options_set(
    options: Vec<PullOption>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), ApiError> {
    pull::write_options(&repo_data_dir(&app, &state)?, &options)
        .map_err(|error| failed(error.to_string()))
}

// What is remembered is saved on accepting the dialog, like `doOKAction`. The merge message is not saved.
#[tauri::command]
pub fn merge_settings_set(
    branch: Option<String>,
    options: Vec<MergeOption>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), ApiError> {
    settings::save_merge(&repo_data_dir(&app, &state)?, branch, options)
        .map_err(|error| failed(error.to_string()))
}

#[tauri::command]
pub fn rebase_settings_set(
    options: Vec<String>,
    new_base: Option<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), ApiError> {
    settings::save_rebase(&repo_data_dir(&app, &state)?, options, new_base)
        .map_err(|error| failed(error.to_string()))
}

#[tauri::command]
pub fn push_tags_get(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<PushTags>, ApiError> {
    Ok(push::read_tags(&repo_data_dir(&app, &state)?))
}

#[tauri::command]
pub fn push_tags_set(
    tags: Option<PushTags>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), ApiError> {
    push::write_tags(&repo_data_dir(&app, &state)?, tags).map_err(|error| failed(error.to_string()))
}

#[tauri::command]
pub fn repo_settings_get(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<settings::RepoSettingsView, ApiError> {
    Ok(settings::load_repo_view(&repo_data_dir(&app, &state)?))
}

// The user's patterns and `Load branch protection rules from GitHub`. The patterns that are not
// valid: they are saved, protect nothing, and the interface says so.
#[tauri::command]
pub fn protected_branches_set(
    patterns: Vec<String>,
    load_github_protection: bool,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<String>, ApiError> {
    let dir = repo_data_dir(&app, &state)?;
    settings::save_protected_branches(&dir, patterns.clone())
        .and_then(|()| settings::save_load_github_protection(&dir, load_github_protection))
        .map_err(|error| failed(error.to_string()))?;
    Ok(push::invalid_patterns(&patterns))
}

// `Clean working tree using:` of the open repository (`GitVcsSettings`, per project).
#[tauri::command]
pub fn clean_tree_set(
    clean_tree: settings::CleanTree,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), ApiError> {
    settings::save_clean_tree(&repo_data_dir(&app, &state)?, clean_tree)
        .map_err(|error| failed(error.to_string()))
}

// The Shelf tab's options of the open repository (`ShelveChangesManager.State`).
#[tauri::command]
pub fn shelf_settings_set(
    shelf: settings::ShelfSettings,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), ApiError> {
    settings::save_shelf_settings(&repo_data_dir(&app, &state)?, shelf)
        .map_err(|error| failed(error.to_string()))
}

// The error of the first pattern of the field that does not compile, while it is typed. It reads
// and writes nothing.
#[tauri::command]
pub fn protected_patterns_check(text: String) -> Option<String> {
    push::first_pattern_error(&text)
}

#[tauri::command]
pub fn force_push_target_remember(
    remote: String,
    branch: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), ApiError> {
    settings::remember_force_target(&repo_data_dir(&app, &state)?, &remote, &branch)
        .map_err(|error| failed(error.to_string()))
}

#[tauri::command]
pub fn reset_mode_get(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<ResetMode>, ApiError> {
    Ok(branch_info::read_reset_mode(&repo_data_dir(&app, &state)?))
}

#[tauri::command]
pub fn reset_mode_set(
    mode: ResetMode,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), ApiError> {
    branch_info::write_reset_mode(&repo_data_dir(&app, &state)?, mode)
        .map_err(|error| failed(error.to_string()))
}

#[tauri::command]
pub fn branch_options_get(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, ApiError> {
    Ok(branch_info::read_options(&repo_data_dir(&app, &state)?))
}

#[tauri::command]
pub fn branch_options_set(
    options: serde_json::Value,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), ApiError> {
    branch_info::write_options(&repo_data_dir(&app, &state)?, &options)
        .map_err(|error| failed(error.to_string()))
}

fn app_visible(app: &AppHandle) -> bool {
    app.get_webview_window(MAIN_WINDOW).is_some_and(|window| {
        window.is_visible().unwrap_or(false) && !window.is_minimized().unwrap_or(false)
    })
}

fn check_remotes(app: &AppHandle) {
    let state = app.state::<AppState>();
    let (Ok(repo), Ok(settings)) = (current_repo(&state), current_settings(app)) else {
        return;
    };
    let found = branch_info::check_remotes(
        &state.tasks,
        &repo,
        settings.incoming_check,
        &state.branch_sync,
        CHECK_LIMIT,
    );
    if let Ok(found) = found {
        state.branch_sync.store(&repo, found);
        let _ = app.emit_to(MAIN_WINDOW, BRANCH_SYNC_EVENT, ());
    }
}

pub fn start_remote_check(app: &AppHandle) {
    let (seen, checked) = (app.clone(), app.clone());
    app.state::<AppState>()
        .branch_sync
        .start(move || app_visible(&seen), move || check_remotes(&checked));
}

// What `patch_create` does once the diff window is saved, without the app: `repo` must be a
// repository root, and `clipboard` gets the text for `To clipboard`. With `remember` (Patch File
// Settings), `To clipboard` is saved first and the folder before the file is written, as
// `CreatePatchCommitSession.execute` and `validateAndWritePatchToFile` do.
#[allow(clippy::too_many_arguments)]
fn create_patch(
    app_dir: &Path,
    repo: &Path,
    repo_dir: &Path,
    entries: &[PatchEntry],
    options: &CreateOptions,
    to: &PatchTarget,
    remember: bool,
    clipboard: impl FnOnce(&str) -> std::io::Result<()>,
) -> Result<PatchCreated, GitError> {
    let text = || create::text(repo, entries, options).map_err(patch_failure);
    deliver_patch(app_dir, repo, repo_dir, to, remember, text, clipboard)
}

// The patch `text` makes, to the file or the clipboard of `to`, with the folder and the choice
// remembered when `remember`.
fn deliver_patch(
    app_dir: &Path,
    repo: &Path,
    repo_dir: &Path,
    to: &PatchTarget,
    remember: bool,
    text: impl FnOnce() -> Result<Vec<u8>, GitError>,
    clipboard: impl FnOnce(&str) -> std::io::Result<()>,
) -> Result<PatchCreated, GitError> {
    let io = |error: std::io::Error| GitError::Io(error.to_string());
    let real = |path: &Path| std::fs::canonicalize(path).map_err(io);
    if real(&crate::commands::repo_root(repo)?)? != real(repo)? {
        return Err(GitError::Io(format!(
            "{} is not the root of a repository",
            repo.display()
        )));
    }
    if remember {
        settings::remember_patch_to_clipboard(repo_dir, matches!(to, PatchTarget::Clipboard))
            .map_err(io)?;
    }
    let file = match to {
        PatchTarget::File { path } => {
            let file = PathBuf::from(path);
            let folder = file
                .parent()
                .filter(|_| file.is_absolute())
                .ok_or_else(|| {
                    GitError::Io(format!("Cannot write patch to specified file: {path}"))
                })?;
            std::fs::create_dir_all(folder).map_err(io)?;
            if remember {
                settings::remember_patch_folder(app_dir, repo_dir, folder, repo).map_err(io)?;
            }
            Some(file)
        }
        PatchTarget::Clipboard => None,
    };
    let text = text()?;
    match file {
        Some(file) => {
            let snapshot = copy_before_overwriting(repo, repo_dir, &file)?;
            create::write_file(&file, &text).map_err(io)?;
            Ok(PatchCreated {
                file: Some(file.display().to_string()),
                snapshot,
            })
        }
        None => {
            clipboard(&String::from_utf8_lossy(&text)).map_err(io)?;
            Ok(PatchCreated {
                file: None,
                snapshot: None,
            })
        }
    }
}

// The file written, and the recoverable copy of what was there when it was a file of the worktree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchCreated {
    file: Option<String>,
    snapshot: Option<SnapshotId>,
}

// Never lose work: a file of the worktree that the patch replaces (after Overwrite) is copied
// first. Outside the worktree, and inside `.git`, the Overwrite question is all there is, as in
// IntelliJ.
fn copy_before_overwriting(
    repo: &Path,
    repo_dir: &Path,
    file: &Path,
) -> Result<Option<SnapshotId>, GitError> {
    let io = |error: std::io::Error| GitError::Io(error.to_string());
    if std::fs::symlink_metadata(file).is_err() {
        return Ok(None);
    }
    let (Some(folder), Some(name)) = (file.parent(), file.file_name()) else {
        return Ok(None);
    };
    let root = std::fs::canonicalize(repo).map_err(io)?;
    let folder = std::fs::canonicalize(folder).map_err(io)?;
    let Ok(inside) = folder.strip_prefix(&root) else {
        return Ok(None);
    };
    let relative = inside.join(name).to_string_lossy().into_owned();
    if crate::snapshots::check_path(repo, &relative).is_err() {
        return Ok(None);
    }
    crate::snapshots::create(repo_dir, repo, &[relative], BEFORE_OVERWRITING_PATCH)
        .map(Some)
        .map_err(io)
}

// `FileUtil.expandUserHome` of the trimmed text (`getFileName`, `getBaseDirName`).
fn expand_home(path: &str) -> String {
    let path = path.trim();
    let home = || std::env::var("HOME").unwrap_or_default();
    if path == "~" {
        return home();
    }
    match path.strip_prefix("~/") {
        Some(rest) => format!("{}/{rest}", home()),
        None => path.to_string(),
    }
}

// Copy as Patch and Create Patch. It saves the diff window first (so its unsaved text goes in, as
// IntelliJ saves the documents). Writing a file takes the repository's lock like every command that
// writes files (the default folder is the worktree); the clipboard only reads. `expected_repo` is
// the repository the changes were read from, which may no longer be the open one: the patch is
// still made from it.
#[tauri::command]
pub async fn patch_create(
    entries: Vec<PatchEntry>,
    options: CreateOptions,
    to: PatchTarget,
    remember: bool,
    expected_repo: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let repo = PathBuf::from(&expected_repo);
    let data = data_dir_of(&app, &repo)?;
    let base = base_dir(&app)?;
    let root = repo.clone();
    let title = CREATING_PATCH.to_string();
    let kind = match &to {
        PatchTarget::File { .. } => TaskKind::Write,
        PatchTarget::Clipboard => TaskKind::Read,
    };
    Ok(state.tasks.start(&root, kind, title, move |_| {
        create_patch(
            &base,
            &repo,
            &data,
            &entries,
            &options,
            &to,
            remember,
            crate::log::commands::copy_to_clipboard,
        )
    }))
}

// What Patch File Settings starts with, for the repository the changes are in.
#[tauri::command]
pub fn patch_settings_get(repo: String, app: AppHandle) -> Result<PatchDefaults, ApiError> {
    let data = data_dir_of(&app, Path::new(&repo))?;
    Ok(settings::patch_defaults(&base_dir(&app)?, &data))
}

// `PATCH_STORAGE_LOCATION` and `DETECT_PATCH_ON_THE_FLY`, which belong to the app.
#[tauri::command]
pub fn patch_app_settings_get(app: AppHandle) -> Result<AppPatch, ApiError> {
    Ok(settings::app_patch(&base_dir(&app)?))
}

// The folder of the patch chosen in `Select Patch File`.
#[tauri::command]
pub fn patch_storage_set(folder: String, app: AppHandle) -> Result<(), ApiError> {
    settings::remember_patch_storage(&base_dir(&app)?, Path::new(&folder))
        .map_err(|error| failed(error.to_string()))
}

// `Analyze and apply patch from clipboard on the fly`.
#[tauri::command]
pub fn patch_on_the_fly_set(on: bool, app: AppHandle) -> Result<(), ApiError> {
    settings::save_detect_patch_on_the_fly(&base_dir(&app)?, on)
        .map_err(|error| failed(error.to_string()))
}

// `Don't ask again` of `Successfully created patch`.
#[tauri::command]
pub fn patch_show_in_finder_set(repo: String, show: bool, app: AppHandle) -> Result<(), ApiError> {
    let data = data_dir_of(&app, Path::new(&repo))?;
    settings::save_show_patch_in_finder(&data, show).map_err(|error| failed(error.to_string()))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchPaths {
    file: String,
    file_exists: bool,
    base: String,
    base_exists: bool,
}

// The `To file:` and `Base path:` fields as Patch File Settings checks them (`File.exists`).
#[tauri::command]
pub fn patch_paths(file: String, base: String) -> PatchPaths {
    let file = expand_home(&file);
    let base = expand_home(&base);
    let exists = |path: &str| !path.is_empty() && Path::new(path).exists();
    PatchPaths {
        file_exists: exists(&file),
        base_exists: exists(&base),
        file,
        base,
    }
}

fn shelf_io(error: std::io::Error) -> GitError {
    GitError::Io(error.to_string())
}

// Whether `repo` is still the open repository, for a task that started on it.
fn still_open(app: &AppHandle, repo: &Path) -> bool {
    is_open(&app.state::<AppState>(), repo)
}

fn is_open(state: &AppState, repo: &Path) -> bool {
    let open = state.repo.lock().unwrap();
    open.as_ref().is_some_and(|open| open.path == repo)
}

// `Shelve Changes...` (`ShelveChangesCommitExecutor`). It saves the diff window first and works
// under the repository's lock. `expected_repo` is the repository the dialog showed: if another one
// is open, before the task starts or before it writes anything, nothing is done.
#[tauri::command]
pub async fn shelf_shelve(
    entries: Vec<ShelveEntry>,
    description: String,
    rollback: bool,
    expected_repo: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let repo = PathBuf::from(&expected_repo);
    if current_repo(&state)? != repo {
        return Err(failed(SWITCHED_BEFORE_SHELVING.to_string()));
    }
    let data = data_dir_of(&app, &repo)?;
    let root = repo.clone();
    let title = SHELVING.to_string();
    Ok(state.tasks.start(&root, TaskKind::Write, title, move |_| {
        let result = shelve(&repo, &data, &entries, &description, rollback, || {
            still_open(&app, &repo)
        });
        repo_changed(&app);
        result
    }))
}

// `Shelve Silently` (rollback) and `Save To Shelf` (none): one list per changelist, named after it
// (`ShelveChangesManager.shelveSilentlyUnderProgress`).
#[tauri::command]
pub async fn shelf_shelve_silently(
    lists: Vec<ShelveGroup>,
    rollback: bool,
    expected_repo: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let repo = PathBuf::from(&expected_repo);
    if current_repo(&state)? != repo {
        return Err(failed(SWITCHED_BEFORE_SHELVING.to_string()));
    }
    let data = data_dir_of(&app, &repo)?;
    let root = repo.clone();
    let title = SHELVING_SILENTLY.to_string();
    Ok(state.tasks.start(&root, TaskKind::Write, title, move |_| {
        let result = shelve_silently(&repo, &data, &lists, rollback, || still_open(&app, &repo));
        repo_changed(&app);
        result
    }))
}

// The Shelf tab. Every command that changes the store saves the diff window first and works under
// the repository's lock on `expected_repo`, the repository the tab showed: with another one open,
// before the task starts or inside it, nothing is done.
const LOADING_SHELF: &str = "Loading...";
// `shelve.deletion.title`, `shelve.undo.deletion`, `vcs.shelf.action.restore.text`,
// `action.ShelvedChanges.Rename.text`, `unshelve.changes.progress.title`, `looking.for.patch.files`,
// `import.patches.into.shelf` and `patch.import.to.shelf.progress.title`.
const DELETING_SHELF: &str = "Shelf Deletion";
const UNDOING_SHELF_DELETION: &str = "Undo Shelf Deletion";
const RESTORING_SHELF: &str = "Restore";
const RENAMING_SHELF: &str = "Rename...";
const UNSHELVING: &str = "Unshelving Changes…";
const LOOKING_FOR_PATCHES: &str = "Looking for Patch Files…";
const IMPORTING_PATCHES: &str = "Import Patches into Shelf…";
const IMPORTING_TO_SHELF: &str = "Import Patch to Shelf";
// Ramify's own (decision 5 of the overview).
const IMPORTING_IDEA_SHELF: &str = "Import from .idea";
const SWITCHED_BEFORE_SHELF: &str =
    "Another repository was opened: the shelf of the previous one was not changed";

fn start_on_shelf<T: Serialize + 'static>(
    app: &AppHandle,
    state: &State<'_, AppState>,
    expected_repo: &str,
    title: &str,
    work: impl FnOnce(&Path, &Path) -> Result<T, GitError> + Send + 'static,
) -> Result<TaskId, ApiError> {
    let repo = PathBuf::from(expected_repo);
    if current_repo(state)? != repo {
        return Err(failed(SWITCHED_BEFORE_SHELF.to_string()));
    }
    let data = data_dir_of(app, &repo)?;
    let app = app.clone();
    let root = repo.clone();
    Ok(state
        .tasks
        .start(&root, TaskKind::Write, title.to_string(), move |_| {
            if !still_open(&app, &repo) {
                return Err(GitError::Io(SWITCHED_BEFORE_SHELF.to_string()));
            }
            let result = work(&repo, &data);
            repo_changed(&app);
            result
        }))
}

// Every list with its changes, newest first. Only the app's own store is read.
#[tauri::command]
pub async fn shelf_lists(app: AppHandle, state: State<'_, AppState>) -> Result<TaskId, ApiError> {
    read(&app, &state, LOADING_SHELF, |repo| {
        Ok(shelf::tab::entries(repo.data))
    })
}

// `Show Diff` and `Compare with Local` of one file of a list. It writes nothing.
#[tauri::command]
pub async fn shelf_sides(
    name: String,
    file: ShelfFile,
    with_local: bool,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    read(&app, &state, LOADING_REVISIONS, move |repo| {
        shelf::tab::sides(repo.data, repo.path, &name, &file, with_local).map_err(GitError::Io)
    })
}

// `gatherPatchFiles` over what the chooser gave: files and folders the user picked. Only read.
#[tauri::command]
pub async fn shelf_find_patches(
    paths: Vec<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    read(&app, &state, LOOKING_FOR_PATCHES, move |_| {
        let chosen: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
        let found = shelf::tab::find_patches(&chosen);
        Ok(found
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect::<Vec<_>>())
    })
}

// `Delete` of the tab: whole lists and the files chosen in others. No question is asked.
#[tauri::command]
pub async fn shelf_delete(
    lists: Vec<String>,
    files: Vec<ListFiles>,
    expected_repo: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    start_on_shelf(
        &app,
        &state,
        &expected_repo,
        DELETING_SHELF,
        move |_, data| {
            shelf::tab::delete(data, &lists, &files, SystemTime::now()).map_err(shelf_io)
        },
    )
}

// `Undo` of `Shelf Deletion`: how many lists could not come back.
#[tauri::command]
pub async fn shelf_undo_delete(
    lists: Vec<Restorable>,
    expected_repo: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    start_on_shelf(
        &app,
        &state,
        &expected_repo,
        UNDOING_SHELF_DELETION,
        move |_, data| shelf::tab::undo_delete(data, &lists).map_err(shelf_io),
    )
}

// `Restore` of the deleted lists, dated now.
#[tauri::command]
pub async fn shelf_restore(
    names: Vec<String>,
    expected_repo: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    start_on_shelf(
        &app,
        &state,
        &expected_repo,
        RESTORING_SHELF,
        move |_, data| shelf::tab::restore(data, &names, SystemTime::now()).map_err(shelf_io),
    )
}

// The inline `Rename...`: the description only.
#[tauri::command]
pub async fn shelf_rename(
    name: String,
    description: String,
    expected_repo: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    start_on_shelf(
        &app,
        &state,
        &expected_repo,
        RENAMING_SHELF,
        move |_, data| shelf::rename(data, &name, &description).map_err(shelf_io),
    )
}

// After an unshelve with `Remove successfully applied files from the shelf`: the files of the
// list that went through are recycled.
#[tauri::command]
pub async fn shelf_recycle(
    name: String,
    files: Vec<ShelfFile>,
    expected_repo: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    start_on_shelf(&app, &state, &expected_repo, UNSHELVING, move |_, data| {
        shelf::unshelve::recycle_applied(data, &name, &files, SystemTime::now())
    })
}

// `Import Patches…`: the patch files found, each one copied into a new list.
#[tauri::command]
pub async fn shelf_import_patches(
    files: Vec<String>,
    expected_repo: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    start_on_shelf(
        &app,
        &state,
        &expected_repo,
        IMPORTING_PATCHES,
        move |_, data| {
            let files: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
            Ok(shelf::tab::import_patches(data, &files))
        },
    )
}

// `Import from .idea` (decision 5 of the overview): the lists of the project's `.idea/shelf`
// not imported before. `.idea` is only read.
#[tauri::command]
pub async fn shelf_import_idea(
    expected_repo: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    start_on_shelf(
        &app,
        &state,
        &expected_repo,
        IMPORTING_IDEA_SHELF,
        move |repo, data| shelf::import(data, repo).map_err(shelf_io),
    )
}

// `Import to Shelf` of the Apply Patch dialog: the included files (by their planned path), mapped.
#[tauri::command]
pub async fn shelf_import_to_shelf(
    source: PatchInput,
    mapping: PathMapping,
    selected: Vec<String>,
    expected_repo: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    start_on_shelf(
        &app,
        &state,
        &expected_repo,
        IMPORTING_TO_SHELF,
        move |repo, data| {
            shelf::tab::import_to_shelf(data, repo, &source, &mapping, &selected)
                .map_err(GitError::Io)
        },
    )
}

// `Create Patch...` and `Copy as Patch to Clipboard` of the tab, as `patch_create` does them.
#[tauri::command]
pub async fn shelf_patch_create(
    chosen: Vec<ListFiles>,
    options: CreateOptions,
    to: PatchTarget,
    remember: bool,
    expected_repo: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let repo = PathBuf::from(&expected_repo);
    let data = data_dir_of(&app, &repo)?;
    let base = base_dir(&app)?;
    let root = repo.clone();
    let title = CREATING_PATCH.to_string();
    let kind = match &to {
        PatchTarget::File { .. } => TaskKind::Write,
        PatchTarget::Clipboard => TaskKind::Read,
    };
    Ok(state.tasks.start(&root, kind, title, move |_| {
        let text = || {
            let sources = shelf::tab::patch_sources(&data, &repo, &chosen).map_err(GitError::Io)?;
            create::text_of_sources(&repo, &sources, &options).map_err(patch_failure)
        };
        deliver_patch(
            &base,
            &repo,
            &data,
            &to,
            remember,
            text,
            crate::log::commands::copy_to_clipboard,
        )
    }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_missing_remote_says_could_not_find_it() {
        assert_eq!(super::remote_not_found("origin/main"), "Could not find remote by name: origin/main");
    }

    // Reads change nothing: there is nothing to save beforehand.
    const READS: [&str; 55] = [
        "merge_message",
        "shelf_lists",
        "shelf_sides",
        "shelf_find_patches",
        "stash_list",
        "stash_changes",
        "stash_file_sides",
        "staging_three_sides",
        "conflicts_session",
        "conflict_sides",
        "merge_range_commits",
        "merge_blocks",
        "fetch_all",
        "fetch_remote",
        "remote_list",
        "remote_test_url",
        "open_exclude_file",
        "pull_info",
        "push_info",
        "push_branch_of",
        "push_outgoing",
        "branch_current",
        "branch_trackers",
        "branch_unmerged_commits",
        "branch_list",
        "branch_incoming_outgoing",
        "branch_refs_fingerprint",
        "unmerged_branches",
        "merge_summary",
        "revisions_valid",
        "rebase_published",
        "rewrite_menu",
        "rewrite_check",
        "rewrite_check_single",
        "rewrite_undo_check",
        "rewrite_validate",
        "rewrite_interactive_start",
        "github_accounts",
        "github_account_details",
        "github_generate_url",
        "open_url",
        "github_links",
        "github_known",
        "github_pr_page",
        "github_pr_etag",
        "github_pr_chooser",
        "github_pr_search_saved",
        "github_pr_details",
        "github_pr_status",
        "github_pr_commits",
        // Like `fetch_remote`, its fetches change remote refs and FETCH_HEAD only.
        "github_pr_changes",
        "github_pr_checked_out",
        "gpg_config",
        "gpg_keys",
        "log_signatures",
    ];
    // They write the app's own data (the GitHub accounts, the Keychain, a repository's default
    // account, the Pull Requests window's saved choice and its list's searches), never the
    // repository's files or index.
    const APP_DATA: [&str; 6] = [
        "github_login",
        "github_remove",
        "github_set_default",
        "github_connect",
        "github_forget_connection",
        "github_pr_search_save",
    ];
    // They work in another folder: the open repository does not change.
    const ELSEWHERE: [&str; 2] = ["clone_repository", "init_repository"];
    // They are called by the diff window while it saves, like `file_write`: waiting for it to save would be
    // waiting for themselves.
    const WHILE_SAVING: [&str; 2] = ["index_write", "staging_revert_range"];
    const FLUSH: &str = "flush_diff_window(&app, &state, SAVE_ACTION).await?;";

    #[test]
    fn mutating_without_saving_the_diff_is_not_allowed() {
        let code = |source: &'static str| &source[..source.find("#[cfg(test)]").unwrap()];
        let source = [
            code(include_str!("vcs_commands.rs")),
            code(include_str!("get_version.rs")),
            code(include_str!("hosting/commands.rs")),
        ]
        .concat();
        let source = source.as_str();
        let commands: Vec<&str> = source
            .split("pub async fn ")
            .skip(1)
            .map(|rest| &rest[..rest.find('(').unwrap()])
            .collect();
        assert_eq!(commands.len(), 137, "{commands:?}");
        let unsaved = |name: &&&str| {
            !READS.contains(name)
                && !ELSEWHERE.contains(name)
                && !WHILE_SAVING.contains(name)
                && !APP_DATA.contains(name)
        };
        for name in commands.iter().filter(unsaved) {
            let start = source.find(&format!("pub async fn {name}(")).unwrap();
            let mut lines = source[start..].lines();
            lines.find(|line| line.ends_with(" {")).unwrap();
            assert_eq!(lines.next().unwrap().trim(), FLUSH, "{name}");
        }
    }

    #[test]
    fn what_the_diff_window_saves_does_not_wait_for_it() {
        let source = include_str!("vcs_commands.rs");
        let source = &source[..source.find("#[cfg(test)]").unwrap()];
        for name in WHILE_SAVING {
            let start = source.find(&format!("pub async fn {name}(")).unwrap();
            let body = &source[start..];
            let body = &body[..body.find("\n}\n").unwrap()];
            assert!(!body.contains("flush_diff_window"), "{name}");
            assert!(body.contains("_locked(&locks"), "{name}");
        }
    }

    #[test]
    fn writing_the_index_goes_with_the_operations_lock() {
        use super::*;
        use crate::test_support::TempRepo;
        use std::fs;
        use tempfile::TempDir;

        let repo = TempRepo::new();
        repo.commit("a.txt", b"one\n", "first");
        fs::write(repo.path().join("a.txt"), b"two\n").unwrap();
        let dir = TempDir::new().unwrap();
        let locks = RepoLocks::default();
        let sides = worktree_diff::three_sides(repo.path(), "a.txt").unwrap();

        let written = index_write_locked(
            &locks,
            repo.path(),
            dir.path(),
            "a.txt",
            "two\n",
            &sides.staged_hash,
        )
        .unwrap();
        let reverted = revert_range_locked(
            &locks,
            repo.path(),
            dir.path(),
            "a.txt",
            "one\n",
            &sides.local_hash,
        )
        .unwrap();

        assert_eq!(repo.git(&["show", ":a.txt"]), "two\n");
        assert_eq!(written.copy, None);
        assert_eq!(fs::read(repo.path().join("a.txt")).unwrap(), b"one\n");
        assert!(reverted.copy.is_some());
    }
    // `PatchWriter.writePatches`: UTF-8 with `\n` whatever the file has, the `Subject:` header, the
    // folder and `To clipboard` remembered, and the clipboard getting the same text.
    #[test]
    fn patch_create_writes_utf8_with_lf() {
        use super::*;
        use crate::patch::create::{PatchEntry, Version};
        use crate::test_support::TempRepo;
        use std::fs;
        use tempfile::TempDir;

        let repo = TempRepo::new();
        repo.commit(".gitattributes", b"* -text\n", "attributes");
        repo.commit("a.txt", "uno\r\ndos\r\n".as_bytes(), "a");
        fs::write(repo.path().join("a.txt"), "uno\r\ndós ñ\r\n").unwrap();
        let app = TempDir::new().unwrap();
        let data = TempDir::new().unwrap();
        let out = TempDir::new().unwrap();
        let file = out.path().join("patches/fix.patch");
        let entries = [PatchEntry {
            path: "a.txt".to_string(),
            old_path: None,
            before: Some(Version::Head),
            after: Some(Version::Disk),
        }];
        let options = CreateOptions {
            base_path: repo.path().display().to_string(),
            reverse: false,
            message: Some("Fix ñ".to_string()),
        };
        let to_file = PatchTarget::File {
            path: file.display().to_string(),
        };
        let unused = |_: &str| -> std::io::Result<()> { panic!("not the clipboard") };

        let written = create_patch(
            app.path(),
            repo.path(),
            data.path(),
            &entries,
            &options,
            &to_file,
            true,
            unused,
        )
        .unwrap();

        let bytes = fs::read(&file).unwrap();
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert_eq!(
            written.file.as_deref(),
            Some(file.display().to_string().as_str())
        );
        assert_eq!(written.snapshot, None);
        assert!(!bytes.contains(&b'\r'), "{text}");
        assert!(
            text.starts_with("Subject: [PATCH] Fix ñ\n---\nIndex: a.txt\n"),
            "{text}"
        );
        assert!(text.contains("\n-dos\n+dós ñ\n"), "{text}");
        let defaults = settings::patch_defaults(app.path(), data.path());
        assert_eq!(
            defaults.folder,
            Some(out.path().join("patches").display().to_string())
        );
        assert!(!defaults.to_clipboard);

        let mut copied = String::new();
        let none = create_patch(
            app.path(),
            repo.path(),
            data.path(),
            &entries,
            &options,
            &PatchTarget::Clipboard,
            true,
            |text: &str| {
                copied = text.to_string();
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(none.file, None);
        assert_eq!(copied, text);
        assert!(settings::patch_defaults(app.path(), data.path()).to_clipboard);

        // Copy as Patch remembers nothing, and a folder that is not a repository root is refused
        // before anything is read or written.
        let silent = TempDir::new().unwrap();
        create_patch(
            app.path(),
            repo.path(),
            silent.path(),
            &entries,
            &options,
            &PatchTarget::Clipboard,
            false,
            |_: &str| Ok(()),
        )
        .unwrap();
        assert_eq!(
            settings::patch_defaults(app.path(), silent.path())
                .folder
                .as_deref(),
            defaults.folder.as_deref()
        );
        assert!(!settings::patch_defaults(app.path(), silent.path()).to_clipboard);
        fs::create_dir(repo.path().join("sub")).unwrap();
        let elsewhere = out.path().join("elsewhere.patch");
        let refused = create_patch(
            app.path(),
            &repo.path().join("sub"),
            data.path(),
            &entries,
            &options,
            &PatchTarget::File {
                path: elsewhere.display().to_string(),
            },
            true,
            unused,
        );
        assert!(refused.is_err());
        assert!(!elsewhere.exists());
    }

    // Never lose work: a patch written over a file of the worktree leaves a recoverable copy of
    // that file first. Outside the worktree the Overwrite question is all there is.
    #[test]
    fn overwriting_a_file_of_the_worktree_keeps_a_copy_first() {
        use super::*;
        use crate::patch::create::{PatchEntry, Version};
        use crate::snapshots;
        use crate::test_support::TempRepo;
        use std::fs;
        use tempfile::TempDir;

        let repo = TempRepo::new();
        repo.commit("a.txt", b"one\n", "a");
        repo.commit("notes.patch", b"my notes\n", "notes");
        fs::write(repo.path().join("a.txt"), b"two\n").unwrap();
        let app = TempDir::new().unwrap();
        let data = TempDir::new().unwrap();
        let entries = [PatchEntry {
            path: "a.txt".to_string(),
            old_path: None,
            before: Some(Version::Head),
            after: Some(Version::Disk),
        }];
        let options = CreateOptions::default();
        let none = |_: &str| -> std::io::Result<()> { panic!("not the clipboard") };
        let target = |path: &Path| PatchTarget::File {
            path: path.display().to_string(),
        };

        let inside = repo.path().join("notes.patch");
        let created = create_patch(
            app.path(),
            repo.path(),
            data.path(),
            &entries,
            &options,
            &target(&inside),
            true,
            none,
        )
        .unwrap();

        let snapshot = created.snapshot.expect("a copy of notes.patch");
        assert!(fs::read_to_string(&inside).unwrap().contains("+two"));
        let kept = snapshots::list(data.path());
        assert_eq!(kept[0].id, snapshot);
        assert_eq!(kept[0].reason, BEFORE_OVERWRITING_PATCH);
        assert_eq!(kept[0].entries.len(), 1);
        assert_eq!(kept[0].entries[0].path, "notes.patch");
        snapshots::restore(data.path(), repo.path(), &snapshot).unwrap();
        assert_eq!(fs::read(&inside).unwrap(), b"my notes\n");

        // A new file in the worktree has nothing to keep, and one outside it is not copied.
        let fresh = repo.path().join("fresh.patch");
        let outside = TempDir::new().unwrap();
        let elsewhere = outside.path().join("x.patch");
        fs::write(&elsewhere, b"old").unwrap();
        for file in [&fresh, &elsewhere] {
            let created = create_patch(
                app.path(),
                repo.path(),
                data.path(),
                &entries,
                &options,
                &target(file),
                true,
                none,
            )
            .unwrap();
            assert_eq!(created.snapshot, None, "{}", file.display());
            assert_eq!(
                created.file.as_deref(),
                Some(file.display().to_string().as_str())
            );
        }
        // The restore above keeps its own copy; the patch made only the first one.
        let made = snapshots::list(data.path())
            .into_iter()
            .filter(|snapshot| snapshot.reason == BEFORE_OVERWRITING_PATCH)
            .count();
        assert_eq!(made, 1);

        // P19: through a linked folder the copy is of the file the link reaches in the worktree;
        // a file inside `.git` is not copied.
        repo.commit("sub/kept.patch", b"kept\n", "kept");
        std::os::unix::fs::symlink(repo.path().join("sub"), repo.path().join("link")).unwrap();
        let in_git = repo.path().join(".git/x.patch");
        fs::write(&in_git, b"old").unwrap();
        let through_link = repo.path().join("link/kept.patch");
        let mut copied = Vec::new();
        for file in [&through_link, &in_git] {
            let created = create_patch(
                app.path(),
                repo.path(),
                data.path(),
                &entries,
                &options,
                &target(file),
                true,
                none,
            )
            .unwrap();
            copied.push(created.snapshot);
        }
        let linked = copied[0].clone().expect("a copy of sub/kept.patch");
        let taken = snapshots::list(data.path());
        let taken = taken.iter().find(|snapshot| snapshot.id == linked).unwrap();
        assert_eq!(taken.entries.len(), 1);
        assert_eq!(taken.entries[0].path, "sub/kept.patch");
        assert_eq!(copied[1], None);
        assert_ne!(fs::read(&in_git).unwrap(), b"old");
    }

    // Writing a file takes the repository's lock, as every command that writes files does; the
    // clipboard only reads.
    #[test]
    fn patch_create_writes_under_the_repository_lock() {
        let source = include_str!("vcs_commands.rs");
        let start = source.find("pub async fn patch_create(").unwrap();
        let body = &source[start..];
        let body = &body[..body.find("\n}\n").unwrap()];

        assert!(
            body.contains("PatchTarget::File { .. } => TaskKind::Write"),
            "{body}"
        );
        assert!(
            body.contains("PatchTarget::Clipboard => TaskKind::Read"),
            "{body}"
        );
    }

    #[test]
    fn patch_paths_expand_the_home_folder() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(super::expand_home("~"), home);
        assert_eq!(super::expand_home(" ~/x.patch "), format!("{home}/x.patch"));
        assert_eq!(super::expand_home("/a/~/b"), "/a/~/b");
        assert_eq!(super::expand_home("~other/b"), "~other/b");
    }

    // The check both commands give `shelve` and `shelve_silently`: a shelve started on one
    // repository writes nothing once another one is open.
    #[test]
    fn a_shelve_started_before_another_repository_was_opened_writes_nothing() {
        use super::*;
        use crate::test_support::TempRepo;
        use tempfile::TempDir;
        let repo = TempRepo::new();
        repo.commit("a.txt", b"one\n", "a");
        std::fs::write(repo.path().join("a.txt"), b"two\n").unwrap();
        let other = TempRepo::new();
        let data = TempDir::new().unwrap();
        let state = AppState::default();
        let open = |path: &Path| {
            *state.repo.lock().unwrap() = Some(crate::commands::RepoSession {
                path: path.to_path_buf(),
            });
        };
        let entry = ShelveEntry {
            path: "a.txt".to_string(),
            old_path: None,
            selection: None,
        };
        open(repo.path());
        assert!(is_open(&state, repo.path()));

        open(other.path());
        let shelved = shelve(repo.path(), data.path(), &[entry], "Work", true, || {
            is_open(&state, repo.path())
        });

        assert_eq!(
            shelved.unwrap_err(),
            GitError::Io(SWITCHED_BEFORE_SHELVING.to_string())
        );
        assert!(crate::shelf::list(data.path()).is_empty());
        assert_eq!(std::fs::read(repo.path().join("a.txt")).unwrap(), b"two\n");
    }

    // Both commands are writes under the lock, tied to the repository they were asked for.
    #[test]
    fn shelving_runs_under_the_repository_lock_on_the_expected_repository() {
        let source = include_str!("vcs_commands.rs");
        for name in ["shelf_shelve", "shelf_shelve_silently"] {
            let start = source.find(&format!("pub async fn {name}(")).unwrap();
            let body = &source[start..];
            let body = &body[..body.find("\n}\n").unwrap()];
            assert!(body.contains("TaskKind::Write"), "{name}");
            assert!(body.contains("current_repo(&state)? != repo"), "{name}");
            assert!(body.contains("still_open(&app, &repo)"), "{name}");
        }
    }
}
