// The commands of branches, tags and reset. Each one first saves what was written in the diff window and
// launches a task, which delivers its result to `taskResult`.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::apply_commits::{self, Kind, PathsResult, Selection};
use crate::branch_info::{self, BranchKind, BranchList, FavoriteSet, Sync};
use crate::branches::{
    self, CheckoutMode, CheckoutOutcome, Deleted, DeletedRemote, DeletedTag, LostCommit, Overwrite,
    RemoteOptions, Repo, ResetMode,
};
use crate::commands::{
    blocking_api, current_repo, current_settings, failed, flush_diff_window, repo_changed,
    repo_data_dir, ApiError, AppState, RepoLocks, MAIN_WINDOW, SAVE_ACTION,
};
use crate::commit;
use crate::conflicts;
use crate::fetch::{self, FetchResult};
use crate::git::GitError;
use crate::log::store::refs_fingerprint;
use crate::merge::{self, MergeOption};
use crate::merge3::{self, Block, Whitespace};
use crate::operation::{self, Operation};
use crate::preserve::{RestoreOutcome, Saved};
use crate::pull::{self, PullInfo, PullOption};
use crate::push::{self, PushInfo, PushResult, PushSpec, PushTags, UpdatedPush};
use crate::rebase::{self, RebaseParams};
use crate::remotes::{self, Cloned, InitOutcome, Remote};
use crate::rewrite::{self, MenuChecks, Rewritten, Undone};
use crate::settings::{self, RepoSettings, UpdateMethod};
use crate::task::{TaskId, TaskKind};
use crate::update::{self, Aborted, UpdateOptions, UpdateResult};
use crate::worktree_diff::{self, ThreeSides, Written};

const BRANCH_SYNC_EVENT: &str = "branch-sync-changed";
// Per remote. One that does not answer must not leave anyone waiting: ssh takes minutes to give up.
const CHECK_LIMIT: Duration = Duration::from_secs(60);
const ADDING_REMOTE: &str = "Adding Remote…";
const CHANGING_REMOTE: &str = "Changing Remote…";
const COMMITTING: &str = "Committing…";
const COMMITTING_MERGE: &str = "Committing Merge…";
const FETCHING: &str = "Fetching…";
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
    let path = current_repo(state)?;
    let data = repo_data_dir(app, state)?;
    let root = path.clone();
    Ok(state
        .tasks
        .start(&root, TaskKind::Read, title.to_string(), move |ctx| {
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

// `Merge 'x' into 'y'` of the branches popup. With `smart`, the Smart Merge of `Git Merge Problem`.
#[tauri::command]
pub async fn branch_merge(
    branch: String,
    smart: bool,
    expected_head: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    // `branch.merging.process`.
    let title = format!("Merging {branch}…");
    start(&app, &state, TaskKind::Write, title, move |repo| {
        merge::merge_into(repo, &branch, smart, &expected_head)
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
pub async fn conflicts_session(state: State<'_, AppState>) -> Result<TaskId, ApiError> {
    let path = current_repo(&state)?;
    let title = LOADING_CONFLICTS.to_string();
    Ok(state
        .tasks
        .start(&path, TaskKind::Read, title, conflicts::session))
}

#[tauri::command]
pub async fn conflict_sides(path: String, state: State<'_, AppState>) -> Result<TaskId, ApiError> {
    let repo = current_repo(&state)?;
    let title = LOADING_REVISIONS.to_string();
    Ok(state.tasks.start(&repo, TaskKind::Read, title, move |ctx| {
        conflicts::sides(ctx, &path)
    }))
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
        split.ok_or_else(|| failed(format!("Couldn't find remote by name: {remote_branch}")))?;
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

// `git pull` changes the tree from start to finish and takes the write lock. With `--rebase` it is
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
        pull::task_kind(&options),
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
    let repo_settings = settings::load_repo(&repo_data_dir(&app, &state)?);
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
) -> Result<RepoSettings, ApiError> {
    Ok(settings::load_repo(&repo_data_dir(&app, &state)?))
}

// The patterns that are not valid: they are saved, protect nothing, and the interface says so.
#[tauri::command]
pub fn protected_branches_set(
    patterns: Vec<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<String>, ApiError> {
    settings::save_protected_branches(&repo_data_dir(&app, &state)?, patterns.clone())
        .map_err(|error| failed(error.to_string()))?;
    Ok(push::invalid_patterns(&patterns))
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

#[cfg(test)]
mod tests {
    // Reads change nothing: there is nothing to save beforehand.
    const READS: [&str; 30] = [
        "merge_message",
        "staging_three_sides",
        "conflicts_session",
        "conflict_sides",
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
    ];
    // They work in another folder: the open repository does not change.
    const ELSEWHERE: [&str; 2] = ["clone_repository", "init_repository"];
    // They are called by the diff window while it saves, like `file_write`: waiting for it to save would be
    // waiting for themselves.
    const WHILE_SAVING: [&str; 2] = ["index_write", "staging_revert_range"];
    const FLUSH: &str = "flush_diff_window(&app, &state, SAVE_ACTION).await?;";

    #[test]
    fn mutating_without_saving_the_diff_is_not_allowed() {
        let source = include_str!("vcs_commands.rs");
        let source = &source[..source.find("#[cfg(test)]").unwrap()];
        let commands: Vec<&str> = source
            .split("pub async fn ")
            .skip(1)
            .map(|rest| &rest[..rest.find('(').unwrap()])
            .collect();
        assert_eq!(commands.len(), 78, "{commands:?}");
        let unsaved = |name: &&&str| {
            !READS.contains(name) && !ELSEWHERE.contains(name) && !WHILE_SAVING.contains(name)
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
}
