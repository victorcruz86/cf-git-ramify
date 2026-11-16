use tauri::{Manager, RunEvent, WindowEvent};

pub mod annotate;
pub mod apply_commits;
pub mod askpass;
pub mod block_history;
pub mod branch_info;
pub mod branches;
pub mod changelists;
pub mod commands;
pub mod commit;
pub mod compare;
pub mod conflicts;
pub mod console;
pub mod diff;
pub mod editor;
pub mod external;
pub mod fetch;
pub mod get_version;
pub mod git;
pub mod gpg;
pub mod history;
pub mod hosting;
pub mod hunks;
pub mod idea_import;
pub mod log;
pub mod merge;
pub mod merge3;
pub mod messages;
pub mod migration;
pub mod open_projects;
pub mod operation;
pub mod patch;
pub mod patch_apply;
pub mod preserve;
pub mod pull;
pub mod push;
pub mod rebase;
pub mod recent;
pub mod remotes;
pub mod rewrite;
pub mod rollback;
pub mod secrets;
pub mod settings;
pub mod shelf;
pub mod snapshots;
pub mod staging;
pub mod stash;
pub mod status;
pub mod storage;
pub mod submodules;
pub mod task;
pub mod terminal;
#[cfg(test)]
mod test_support;
pub mod todo;
pub mod tree;
pub mod unversioned;
pub mod update;
pub mod vcs_commands;
pub mod viewer;
pub mod watch;
pub mod window_frames;
pub mod windows;
pub mod worktree_diff;

#[tauri::command]
fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

// A failed copy starts with empty data and is tried again on the next start (`migration`).
fn migrate_data_folder(app: &tauri::AppHandle) {
    let Some(support) = app
        .path()
        .app_data_dir()
        .ok()
        .and_then(|dir| dir.parent().map(|parent| parent.to_path_buf()))
    else {
        return;
    };
    match migration::migrate_data_folder(&support) {
        Ok(migration::Migration::Copied) => eprintln!("Copied rama's data folder for Ramify"),
        Ok(migration::Migration::NotNeeded) => {}
        Err(error) => eprintln!("Could not copy rama's data folder: {error}"),
    }
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(commands::AppState::default())
        .menu(commands::app_menu)
        .on_menu_event(|app, event| {
            if event.id() == commands::QUIT_MENU_ID {
                commands::quit(app.clone());
            } else if event.id() == commands::SETTINGS_MENU_ID {
                commands::open_settings(app);
            } else if event.id() == commands::OPEN_MENU_ID {
                commands::open_project_chooser(app);
            } else if let Some(path) = commands::recent_menu_path(&event.id().0) {
                commands::open_recent(app, path);
            } else if commands::is_git_menu_item(&event.id().0) {
                commands::git_menu_action(app, &event.id().0);
            }
        })
        .setup(|app| {
            // Before anything reads the data folder.
            migrate_data_folder(app.handle());
            commands::load_settings(app.handle());
            commands::refresh_recent_menu(app.handle());
            commands::forward_task_events(app.handle());
            commands::start_askpass(app.handle());
            commands::start_editor(app.handle());
            vcs_commands::start_remote_check(app.handle());
            hosting::commands::start_protection_loader(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_version,
            commands::initial_repo,
            commands::recent_projects_remove,
            viewer::initial_target,
            viewer::viewer_open,
            viewer::viewer_targets,
            viewer::viewer_openable,
            viewer::viewer_stamp,
            viewer::viewer_content,
            block_history::selection_history_open,
            block_history::selection_history_target,
            block_history::selection_history_start,
            block_history::selection_history_close,
            block_history::selection_history_show_affected,
            annotate::annotate_start,
            annotate::annotate_refresh,
            annotate::annotation_show_in_log,
            tree::tree_list,
            tree::tree_list_at,
            commands::show_window,
            commands::hide_window,
            commands::repo_open,
            commands::project_resolve,
            commands::project_focus_open,
            commands::projects_list,
            commands::project_new_window,
            commands::open_projects_reopen,
            commands::commit_draft_saved,
            commands::repo_state,
            commands::git_menu_enable,
            commands::git_menu_current_file,
            commands::git_file_action,
            log::commands::log_open,
            log::commands::log_rows,
            log::commands::log_view_set,
            log::commands::log_graph_click,
            log::commands::log_graph_hover,
            log::commands::log_view_open,
            log::commands::log_users,
            log::commands::log_refs,
            log::commands::log_view_close,
            log::commands::log_history_open,
            log::commands::log_history_entries,
            log::commands::log_history_change,
            log::commands::log_find_rows,
            log::commands::log_row_of,
            log::commands::log_adjacent_rows,
            log::commands::log_current_branch,
            log::commands::clipboard_write,
            log::commands::patch_clipboard,
            log::commands::log_refs_changed,
            log::commands::log_options_get,
            log::commands::log_options_set,
            log::commands::commit_details,
            log::commands::branches_containing,
            log::commands::commit_changes_of,
            log::commands::commit_changes_start,
            log::commands::log_recent_filters,
            log::commands::log_remember_filter,
            log::commands::log_compared_branch,
            log::commands::log_not_picked,
            vcs_commands::branch_checkout,
            vcs_commands::branch_checkout_remote,
            vcs_commands::branch_rebase_onto,
            vcs_commands::branch_merge,
            vcs_commands::branch_rebase,
            vcs_commands::branch_rebase_remote,
            vcs_commands::rebase_published,
            vcs_commands::branch_pull_into,
            vcs_commands::operation_continue,
            vcs_commands::operation_skip,
            vcs_commands::operation_abort,
            vcs_commands::conflicts_session,
            vcs_commands::conflict_sides,
            vcs_commands::merge_range_commits,
            vcs_commands::conflicts_accept,
            vcs_commands::conflict_resolve,
            vcs_commands::stash_list,
            vcs_commands::stash_changes,
            vcs_commands::stash_file_sides,
            vcs_commands::stash_save,
            vcs_commands::stash_push_paths,
            vcs_commands::stash_unstash,
            vcs_commands::stash_drop,
            vcs_commands::stash_clear,
            vcs_commands::patch_plan,
            vcs_commands::patch_apply,
            vcs_commands::patch_conflict_resolve,
            vcs_commands::patch_rollback,
            vcs_commands::patch_preview,
            vcs_commands::merge_commit,
            vcs_commands::apply_commits,
            vcs_commands::rewrite_menu,
            vcs_commands::rewrite_check,
            vcs_commands::rewrite_check_single,
            vcs_commands::rewrite_uncommit,
            vcs_commands::rewrite_autosquash,
            vcs_commands::rewrite_validate,
            vcs_commands::rewrite_interactive_start,
            vcs_commands::rewrite_interactive,
            vcs_commands::rewrite_reword,
            vcs_commands::rewrite_drop,
            vcs_commands::rewrite_squash,
            vcs_commands::rewrite_undo,
            vcs_commands::rewrite_undo_check,
            vcs_commands::apply_commit_after_conflicts,
            vcs_commands::apply_paths,
            vcs_commands::changes_add,
            vcs_commands::unversioned_delete,
            vcs_commands::merge_message,
            vcs_commands::merge_blocks,
            vcs_commands::staging_three_sides,
            vcs_commands::index_write,
            vcs_commands::staging_revert_range,
            vcs_commands::branch_create,
            vcs_commands::branch_rename,
            vcs_commands::branch_delete,
            vcs_commands::branch_restore,
            vcs_commands::branch_delete_remote,
            vcs_commands::branch_reset,
            vcs_commands::tag_create,
            vcs_commands::tag_delete,
            vcs_commands::tag_restore,
            vcs_commands::remote_list,
            vcs_commands::remote_add,
            vcs_commands::remote_edit,
            vcs_commands::remote_remove,
            vcs_commands::remote_test_url,
            vcs_commands::gpg_config,
            vcs_commands::gpg_keys,
            vcs_commands::log_signatures,
            vcs_commands::gpg_config_set,
            vcs_commands::clone_repository,
            vcs_commands::init_repository,
            vcs_commands::open_exclude_file,
            vcs_commands::fetch_all,
            vcs_commands::fetch_remote,
            vcs_commands::pull_info,
            vcs_commands::git_pull,
            vcs_commands::push_info,
            vcs_commands::push_branch_of,
            vcs_commands::push_outgoing,
            vcs_commands::git_push,
            vcs_commands::git_push_update,
            vcs_commands::update_project,
            vcs_commands::update_branch,
            vcs_commands::update_abort,
            vcs_commands::update_restore,
            vcs_commands::branch_current,
            vcs_commands::branch_trackers,
            vcs_commands::branch_unmerged_commits,
            vcs_commands::branch_list,
            vcs_commands::branch_incoming_outgoing,
            vcs_commands::branch_refs_fingerprint,
            vcs_commands::favorites_get,
            vcs_commands::favorites_toggle,
            vcs_commands::git_merge,
            vcs_commands::git_rebase,
            vcs_commands::unmerged_branches,
            vcs_commands::merge_summary,
            vcs_commands::revisions_valid,
            vcs_commands::merge_settings_set,
            vcs_commands::rebase_settings_set,
            vcs_commands::pull_options_get,
            vcs_commands::pull_options_set,
            vcs_commands::push_tags_get,
            vcs_commands::push_tags_set,
            vcs_commands::repo_settings_get,
            vcs_commands::protected_branches_set,
            vcs_commands::clean_tree_set,
            vcs_commands::protected_patterns_check,
            vcs_commands::force_push_target_remember,
            vcs_commands::reset_mode_get,
            vcs_commands::reset_mode_set,
            vcs_commands::branch_options_get,
            vcs_commands::branch_options_set,
            commands::commit_files,
            commands::file_sides,
            commands::worktree_sides,
            commands::worktree_preview,
            commands::file_write,
            commands::diff_window_open,
            commands::diff_window_path,
            commands::diff_window_open_request,
            commands::diff_window_request,
            compare::compare_sides,
            compare::compare_preview,
            compare::compare_local_state,
            compare::compare_last_revision,
            compare::compare_resolve,
            compare::compare_file_revisions,
            compare::compare_changes_with,
            compare::compare_changes_between,
            compare::compare_changes_from_working_tree,
            compare::compare_tags,
            compare::compare_current_revision,
            get_version::revision_get_check,
            get_version::revision_get,
            commands::diff_flushed,
            commands::merge_window_open,
            commands::merge_window_open_contents,
            commands::merge_window_path,
            commands::merge_window_contents,
            commands::merge_window_done,
            commands::merge_working_file,
            commands::merge_left,
            commands::merge_leave_received,
            commands::merge_window_ready,
            commands::reveal_in_finder,
            vcs_commands::patch_create,
            vcs_commands::patch_settings_get,
            vcs_commands::patch_show_in_finder_set,
            vcs_commands::patch_paths,
            vcs_commands::patch_app_settings_get,
            vcs_commands::patch_storage_set,
            vcs_commands::patch_on_the_fly_set,
            vcs_commands::shelf_shelve,
            vcs_commands::shelf_shelve_silently,
            vcs_commands::shelf_settings_set,
            vcs_commands::shelf_lists,
            vcs_commands::shelf_sides,
            vcs_commands::shelf_find_patches,
            vcs_commands::shelf_delete,
            vcs_commands::shelf_undo_delete,
            vcs_commands::shelf_restore,
            vcs_commands::shelf_rename,
            vcs_commands::shelf_recycle,
            vcs_commands::shelf_import_patches,
            vcs_commands::shelf_import_idea,
            vcs_commands::shelf_import_to_shelf,
            vcs_commands::shelf_patch_create,
            commands::repo_status,
            commands::changelists_get,
            commands::changelists_lists,
            commands::changelists_create,
            commands::changelists_rename,
            commands::changelists_set_comment,
            commands::changelists_remove,
            commands::changelists_set_active,
            commands::changelists_move,
            commands::changelists_import,
            commands::changelists_move_hunks,
            commands::rollback_files,
            commands::commit_changes,
            commands::commit_changes_selected,
            commands::file_hunks,
            commands::lines_rollback,
            commands::staging_stage,
            commands::staging_unstage,
            commands::staging_revert,
            commands::staging_stage_hunks,
            commands::staging_unstage_hunks,
            commands::staging_sides,
            commands::staging_preview,
            commands::commit_staged,
            commands::commit_warnings,
            commands::commit_warnings_staged,
            commands::commit_fix_autocrlf,
            commands::settings_get,
            commands::settings_save,
            commands::git_version,
            commands::messages_list,
            commands::messages_last,
            commands::messages_push,
            commands::repo_has_commit_hooks,
            commands::head_message,
            commands::snapshots_list,
            commands::snapshots_restore,
            commands::task_cancel,
            commands::askpass_answer,
            commands::editor_answer,
            commands::console_lines,
            commands::perf_enabled,
            commands::perf_report,
            hosting::commands::github_accounts,
            hosting::commands::github_account_details,
            hosting::commands::github_login,
            hosting::commands::github_remove,
            hosting::commands::github_set_default,
            hosting::commands::github_generate_url,
            hosting::commands::open_url,
            hosting::commands::github_links,
            hosting::commands::github_known,
            hosting::commands::github_connect,
            hosting::commands::github_forget_connection,
            hosting::commands::github_pr_page,
            hosting::commands::github_pr_etag,
            hosting::commands::github_pr_chooser,
            hosting::commands::github_pr_search_saved,
            hosting::commands::github_pr_search_save,
            hosting::commands::github_pr_details,
            hosting::commands::github_pr_status,
            hosting::commands::github_pr_commits,
            hosting::commands::github_pr_changes,
            hosting::commands::github_pr_checkout,
            hosting::commands::github_pr_show_in_log,
            hosting::commands::github_pr_checked_out,
            terminal::terminal_open,
            terminal::terminal_write,
            terminal::terminal_resize,
            terminal::terminal_busy,
            terminal::terminal_close,
            terminal::terminal_reset,
        ])
        .on_window_event(|window, event| {
            if let WindowEvent::Focused(true) = event {
                window
                    .app_handle()
                    .state::<commands::AppState>()
                    .branch_sync
                    .visible();
            }
            if let WindowEvent::Focused(true) = event {
                commands::main_focused(window.app_handle(), window.label());
            }
            if let WindowEvent::Focused(focused) = event {
                commands::focus_changed(window.app_handle(), window.label(), *focused);
            }
            if matches!(event, WindowEvent::Moved(_) | WindowEvent::Resized(_)) {
                commands::frame_changed(window.app_handle(), window.label());
            }
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let app = window.app_handle().clone();
                if commands::owns_watcher(window.label()) {
                    commands::close_project_window(app, window.label());
                } else {
                    commands::request_close(&app, window.label());
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("could not start the app")
        .run(|app, event| match event {
            RunEvent::ExitRequested {
                code: None, api, ..
            } => api.prevent_exit(),
            RunEvent::Reopen { .. } => commands::show_last_window(app),
            // `open -a Ramify <folder>` while it runs, or the folder it was launched with.
            #[cfg(target_os = "macos")]
            RunEvent::Opened { urls } => {
                let paths = urls
                    .iter()
                    .filter_map(|url| url.to_file_path().ok())
                    .collect();
                commands::open_paths(app, paths);
            }
            RunEvent::Exit => {
                app.state::<commands::AppState>().terminals.close_all();
                commands::stop_askpass(app);
                commands::stop_editor(app);
            }
            _ => {}
        });
}
