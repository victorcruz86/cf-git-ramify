use tauri::{Manager, RunEvent, WindowEvent};

pub mod apply_commits;
pub mod askpass;
pub mod branch_info;
pub mod branches;
pub mod changelists;
pub mod commands;
pub mod commit;
pub mod conflicts;
pub mod console;
pub mod diff;
pub mod editor;
pub mod fetch;
pub mod git;
pub mod hunks;
pub mod idea_import;
pub mod log;
pub mod merge;
pub mod merge3;
pub mod messages;
pub mod operation;
pub mod preserve;
pub mod pull;
pub mod push;
pub mod rebase;
pub mod remotes;
pub mod rewrite;
pub mod rollback;
pub mod settings;
pub mod snapshots;
pub mod staging;
pub mod status;
pub mod storage;
pub mod task;
#[cfg(test)]
mod test_support;
pub mod todo;
pub mod update;
pub mod vcs_commands;
pub mod watch;
pub mod worktree_diff;

#[tauri::command]
fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

pub fn run() {
    tauri::Builder::default()
        .manage(commands::AppState::default())
        .menu(commands::app_menu)
        .on_menu_event(|app, event| {
            if event.id() == commands::QUIT_MENU_ID {
                commands::quit(app.clone());
            } else if event.id() == commands::SETTINGS_MENU_ID {
                commands::open_settings(app);
            } else if commands::is_git_menu_item(&event.id().0) {
                commands::git_menu_action(app, &event.id().0);
            }
        })
        .setup(|app| {
            commands::load_settings(app.handle());
            commands::forward_task_events(app.handle());
            commands::start_askpass(app.handle());
            commands::start_editor(app.handle());
            vcs_commands::start_remote_check(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_version,
            commands::initial_repo,
            commands::show_window,
            commands::hide_window,
            commands::repo_open,
            commands::repo_state,
            commands::git_menu_enable,
            log::commands::log_open,
            log::commands::log_rows,
            log::commands::log_view_set,
            log::commands::log_graph_click,
            log::commands::log_graph_hover,
            log::commands::log_view_open,
            log::commands::log_users,
            log::commands::log_refs,
            log::commands::log_view_close,
            log::commands::log_find_rows,
            log::commands::log_row_of,
            log::commands::log_adjacent_rows,
            log::commands::log_current_branch,
            log::commands::clipboard_write,
            log::commands::log_refs_changed,
            log::commands::log_options_get,
            log::commands::log_options_set,
            log::commands::commit_details,
            log::commands::branches_containing,
            log::commands::commit_changes_of,
            log::commands::commit_changes_start,
            log::commands::log_recent_filters,
            log::commands::log_remember_filter,
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
            vcs_commands::conflicts_accept,
            vcs_commands::conflict_resolve,
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
            vcs_commands::clone_repository,
            vcs_commands::init_repository,
            vcs_commands::open_exclude_file,
            vcs_commands::open_in_new_window,
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
            commands::diff_flushed,
            commands::merge_window_open,
            commands::merge_window_path,
            commands::merge_window_done,
            commands::merge_working_file,
            commands::merge_left,
            commands::merge_leave_received,
            commands::merge_window_ready,
            commands::repo_status,
            commands::changelists_get,
            commands::changelists_create,
            commands::changelists_rename,
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
            commands::head_message,
            commands::snapshots_list,
            commands::snapshots_restore,
            commands::task_cancel,
            commands::askpass_answer,
            commands::editor_answer,
            commands::console_lines,
            commands::perf_enabled,
            commands::perf_report,
        ])
        .on_window_event(|window, event| {
            if let WindowEvent::Focused(true) = event {
                window
                    .app_handle()
                    .state::<commands::AppState>()
                    .branch_sync
                    .visible();
            }
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let app = window.app_handle().clone();
                if commands::owns_watcher(window.label()) {
                    commands::close_main(app);
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
            RunEvent::Reopen { .. } => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = commands::reveal(&window);
                }
                commands::resume_and_notify(app);
            }
            RunEvent::Exit => {
                commands::stop_askpass(app);
                commands::stop_editor(app);
            }
            _ => {}
        });
}
