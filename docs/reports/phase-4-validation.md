# Phase 4: validation report

Date: 2026-11-04. Phase 4 is delivered in parts; each part adds its section to this report. Decisions of the phase: `docs/superpowers/plans/2026-11-03-phase-4-overview.md`. Decisions of part 1: `docs/superpowers/plans/2026-11-03-phase-4-1-modal-commit.md`. What changed when the reference moved from IntelliJ 2023.2.3 to 2024.1: `phase-4-reference-change.md`.

## Part 1: modal commit

### Summary

- Part 1 is done in essence: the user commits as in IntelliJ 2024.1 in modal mode with changelists. ⌘K opens the `Commit Changes` dialog; the `Git` tool window has the Local Changes, Log and Console tabs; the setting `Use non-modal commit interface` brings back the Commit tool window of phases 1 to 3. The default is modal.
- **`done` means tests, not use.** The Rust logic is tested with real git and throwaway repositories, and the interface in jsdom with the Tauri API simulated. Nobody has seen the dialog, the Git window or the diff window in a real window, nor pressed ⌘K, ⌥⌘K, ⌘9 or ⌘0 on a physical keyboard. The part cannot be accepted without the list in section 5.
- Rows of the matrix filled in this part: 22 (21 `done`, 1 `partial`), plus 4 `Git.Stage.*` rows edited and not counted in the 22. The `partial` one is `ChangesView.ToggleCommitUi`: the interface switches from Settings, but no action toggles it. The four `Git.Stage.*` rows keep their `done` of phase 1 and now say in the Feature column that in modal mode they are not shown.
- The part has 14 commits (`971e4e1..d0b039a`), plus the report (`f02b72f`) and the fixes of the final review. Nothing was measured: memory, CPU and startup with the dialog open are not known (section 6).
- Ten rulings were taken during the part (R9 and R10 in the final review, which correct R8 and R3); the ones that depart from IntelliJ are in section 2, each with its cost.

### 1. What was built

- **Settings.** `Use non-modal commit interface` (default off, so modal) and `Clear initial commit message`, in a new `Commit` section of the settings, with the texts of 241. `Enable staging area` is greyed out in modal mode and keeps its saved value; every reader of the staging area (the commit view, the diff window, the log flows, the backend that preserves the index) uses the effective value `nonModalCommit && stagingArea`.
- **Tool windows.** In modal mode the stripe has one `Git` button (⌘9) with Local Changes, Log and Console; there is no Commit window and ⌘0 does nothing. In non-modal mode the stripe has `Commit` (⌘0) and `Git` (⌘9), as before. A window comes back on the tab it had.
- **`CommitView` split in three.** A `ChangesPanel` that both modes share (tree, toolbar, menus, changelists, rollback, notices), `LocalChangesView` for modal mode (no checkboxes, `Commit…` and `Commit File…`, a `Preview Diff` splitter with a read-only embedded diff, shown by default) and the non-modal view, which keeps its checkboxes and message panel.
- **The `Commit Changes` dialog.** Changelist chooser, change tree with checkboxes and `Unversioned Files`, legend, commit message with history, options (`Author:`, `Amend commit`, `Sign-off commit`, `Run Git hooks` only if the repository has hooks), a collapsible `Diff` panel with line checkboxes, and `Commit` with a `Commit and Push…` drop-down (⌥⌘K). Its model is pure TypeScript (`src/commit/dialog.ts`) and follows `CheckinActionUtil` and `SingleChangeListCommitMessagePolicy`: initial changelist, initial inclusion, initial message (list comment, then a non-default list name, then `MERGE_MSG`, then the last commit message), what Cancel saves, what a full commit clears. Size, splitters and the state of the `Diff` panel are remembered under IntelliJ's keys. A commit made from the dialog runs after it closes, as a background task, tied to the repository it was made in: if another repository is opened while it runs, its outcome is still notified, but the draft, the changelists, the log, the push and the rebase of the new repository are left alone.
- **Entry points.** ⌘K, `Commit…`, `Commit File…`, the Git menu, the branches popup, ⌥⌘K and the Fixup / Squash Into flows of the log go through one function (`requestCommit`) that opens the dialog in modal mode and the Commit view in non-modal mode. Fixup and Squash Into arrive with the text `fixup! subject` or `squash! subject` and, in the dialog, `Commit and Rebase…` in the drop-down.
- **Diff window.** It knows who owns the line checkboxes: nobody, the non-modal Commit view or the open dialog. With no owner it shows none; the dialog, when it closes, takes its checkboxes away.
- **Checks before the commit.** Large files (default over 50 MB, off with Git LFS or a limit of 0) and file names that Windows does not accept (reserved names and bad characters, only for files that are new), with the setting rows `Warn when committing files larger than` and `Warn when committing files with names that might cause issues on other systems`. They come in IntelliJ's order: CRLF, large files, detached HEAD or rebase, file names. They work in the dialog and in the non-modal view.
- **Smaller pieces.** The message history keeps 25 entries and the last message is kept apart; `changelists_set_comment`, `messages_last`, `messages_push` and `repo_has_commit_hooks` are new commands that only touch rama's own files or read (`changelists_set_comment` takes the repository the comment was written for and refuses it if another one is open); the sign-off is remembered per repository.

### 2. Decisions and rulings that depart from IntelliJ

Each one with what it costs if it turns out to be wrong.

**Decisions of the plan (1 to 9)**

1. **The default is modal.** `non_modal_commit` is `false`; IntelliJ 2024.1 defaults to non-modal. It is how the user has IntelliJ. Cost: whoever did not choose it finds the dialog first.
2. **One `Git` stripe button in modal mode.** rama has no editor area, so the Git window fills the content area, and Log and Console are tabs instead of stripe buttons. Pressing ⌘9 with the window in front does nothing; IntelliJ would hide it. Cost: the layout changes for whoever used the earlier stripe.
3. **A double click in Local Changes opens the diff.** IntelliJ opens the source by default (`SHOW_DIFF_ON_DOUBLE_CLICK=false`); rama has no viewer until part 4, which adds the `Show on Double-Click` item and the viewer. Cost: until then a double click shows the diff, not the file.
4. **`Preview Diff` is included**, visible by default, as a read-only embedded diff without line checkboxes. Nothing loads while it is hidden. Cost: a diff that loads on every selection change.
5. **No `DiffCommitMessageEditor`.** The diff window opened from the dialog does not show a second editor for the message; rama edits the message only in the dialog. Cost: small.
6. **Dialog size.** First opening 900×700, centred; afterwards the size, both splitters and the `Diff` panel state are remembered in `localStorage`. Cost: the first opening may differ from IntelliJ's preferred size.
7. **The message history keeps 25 entries**, as IntelliJ does; it kept 20. Cost: none.
8. **No code checks and no `Commit Message Inspections` table.** `validate.ts` keeps its fixed rules; `Commit Checks` shows only `Run Git hooks`, and it is hidden when the repository has no hooks (IntelliJ shows the group title). Cost: none for Git-only use.
9. **The setting switches on Save**, not live. The views are remounted. Only the dialog's draft lives in the changelist comment; the draft of the non-modal Commit view and its line selections are handed over in memory when the mode switches (`App.svelte`, tested), so quitting the app while in modal mode loses that draft. Cost: the views blink once, and a non-modal draft does not survive quitting in modal mode.

**Rulings taken during the part**

- **R2: a tool window comes back on its last tab; the first content is selected only on a mode switch.** The task text said ⌘9 selects "the first content"; `ChangesViewContentManager` in 241 selects it only when the mappings change. In modal mode there is one window, so the visible behaviour is the same. Cost if wrong: one function (`activateWindow`).
- **R3: the read-only `Preview Diff` keeps the gutter menu with `Rollback Lines` and `Move Lines`.** Decision 4's "read-only" means no typing and no line checkboxes; the gutter actions go through the existing commands, which flush the diff window and take the repository locks. IntelliJ's preview offers move (editor popup) and revert (gutter arrows) with different texts and UI (ruling R10). Cost if wrong: hide two menu items in `FilePane` when it is embedded and read-only.
- **R5: `Author:` is a plain field.** There is no combo of known authors and no `Author differs from default` balloon, and no `list.author` on changelists. They need a command that reads authors from the log and an author model on the changelist. Cost if wrong: one combo and one command, later.
- **R8, corrected by R9: the size measure of the large-file check.** IntelliJ uses `VirtualFile.length` (the file on disk), and so does rama, with two differences: a symlink is measured as the link (git stores the link text) and, in staging area mode, the staged blob is measured. Deleted files are skipped. A file with a partial selection is measured whole on disk, as in IntelliJ: the application always sends `partial: []` and adds the files chosen by lines to the measured ones (`warnings_for`). The backend can measure a partial content by what goes into the index (`a_partial_file_is_measured_by_what_goes_into_the_index`), but nothing calls it that way. Cost: a size that differs from IntelliJ's disk length when a staged file was edited afterwards (stage mode only), or for a symlink to a large file; a file near the limit can warn in one and not in the other. A partial commit of a big file warns although the staged part is small, as in IntelliJ. The `Int` overflow of `limit * 1024 * 1024` in Kotlin is not ported (u64 math). Lines chosen in the `Diff` panel or the diff window are measured as whole files, as the CRLF check already does.

**Rulings that follow IntelliJ more closely or add a check (listed because they changed behaviour)**

- **R4: the request is rebuilt after the warnings**, in the dialog and in the Commit view, so Commit sends only what the dialog shows at that moment (a file deleted while a warning was open is not sent). Cost if wrong: one extra `chooseCommit` call.
- **R6: the HEAD guard of `Commit and Rebase…` is checked again after the warnings**, in the dialog and in the Commit view, and a `rebaseFrom` without the `head` that was seen counts as stale. Cost if wrong: two extra `repo_state` calls.
- **R7: Cancel on the file name warning and on the detached HEAD and rebase warnings closes the commit dialog**, keeping the draft as the changelist comment like any cancel (241 returns `CLOSE_WINDOW`); Cancel on the large file and CRLF warnings keeps it open. In the non-modal view there is no window to close: Cancel just stops. Cost if wrong: the user loses the open dialog (not the message) after cancelling those warnings.

**Ruling on the matrix only**

- **R1: `Use non-modal commit interface` is a row of phase 4**, not 1, because it is introduced in phase 4. It changes one cell of the matrix and no behaviour. Cost: one cell.

**Other differences that came with the tasks**

- The warning buttons of the large file and file name dialogs say `Cancel` and the commit action (`Commit`, `Commit and Push`…), as the 241 source of the modal dialog does. The plan said `Commit Anyway`, which in 241 is the text of the non-modal notification.
- `Sign-off commit` has an empty signature in its tooltip, because rama does not read the git user.
- The `Diff` panel shows the first file of the list when nothing is selected.
- Conflicts keep their node without checkbox in the dialog and in Local Changes.
- A draft written when the list is changed in the chooser is saved at once; IntelliJ saves it when the dialog closes.
- The dialog has its own frame instead of the shared `Dialog.svelte`, because it is resizable and holds nested dialogs of the panel; the focus trap and Esc handling are repeated there.
- ⌥⌘K outside the dialog in modal mode opens the dialog; `Commit and Push…` is then chosen in it. In 241 the action is invisible and disabled without a commit workflow (`BaseCommitExecutorAction.update`), so IntelliJ does nothing there. Cost: a shortcut that acts where IntelliJ's does not; whoever presses it by habit outside a commit gets the dialog instead of nothing.

### 3. Known gaps a user could notice

From the items the reviews deferred:

- **No user name check.** IntelliJ's `GitUserNameCommitProblem` is the first check of the handler order and rama has none. With no `user.name` the commit fails in git instead.
- **`Ignored Files` is missing in `View Options`**, and under the New UI there is an extra separator before `View Options` (`ChangesViewManager.java:788-790` at 241).
- **The selection order is the tree order, not the click order.** It shows in the file the diff opens first when several are selected and in the initial list of the Move dialog.
- **The Local Changes menu lacks** `Rollback File…`, `Show Diff in a New Tab or Window`, `Jump to Source`, `Delete`, `Add to VCS`, `Remove from VCS`, `Check Out` and the Git submenu; `Shelve Changes…` and the patch items come in part 3.
- **A blank message does not become the "last commit message"** (IntelliJ sets it even for blank), and the last message is written after a successful commit, not before it as IntelliJ does. The reword dialog does not save to the history either (part 2 item).
- **Line checkboxes in the dialog for a file that belongs to another changelist** (a "part" row) do not appear; the part still commits whole.
- **The large files are not named in the warning**, only their count and total size, as in IntelliJ. If git fails while measuring sizes or checking names, the check passes without a log line.
- **A failed commit saves the draft even with a partial inclusion.** Rename and Set Default shortcuts also act inside the dialog's browser.
- **If the diff window has unsaved typed text when the owner of the line checkboxes changes**, the checkboxes update after the text is saved.
- **Size of two files:** `CommitDialog.svelte` has 1,175 lines and `ChangesPanel.svelte` 1,277; they work but are the next candidates to split.

### 4. What was proven with tests

Each feature and the tests that name it. The exact names are in the Result column of `docs/parity-matrix.md`.

- **Settings:** `the Commit section shows Use non-modal commit interface with its comment and saves it`, `Enable staging area is greyed out while the commit interface is modal` and, in Rust, `non_modal_commit_and_clear_message_default_to_false`, `a_settings_file_without_the_new_fields_reads_modal`, `staging_is_effective_only_in_non_modal_mode`. The log flows and the application read the effective staging area (`a saved staging area is not in effect while the commit interface is modal`, `staging area saved while the commit interface is modal is not in effect`).
- **Tool windows:** `in modal mode the app opens on Local Changes without loading the log`, `the stripe has a single Git button in modal mode`, `cmd 0 does nothing in modal mode`, `cmd 9 shows the Git window with Local Changes, Log and Console in that order`, `in non-modal mode cmd 9 and cmd 0 switch between the Git and Commit windows`, `switching the setting moves Local Changes between the Commit and Git windows`, `a draft and a line selection of the Commit view survive switching to modal and back`.
- **Local Changes:** `Local Changes shows the changelists without checkboxes`, `the toolbar follows the order of ChangesViewToolbar`, `the popup follows the order of ChangesViewPopupMenu`, `Preview Diff shows the selected file's diff without line checkboxes and is shown by default`, `double click opens the diff`, `Commit… with nothing selected asks for the whole list and Commit File… for the selected files`.
- **Dialog model:** the 16 tests of `dialog.test.ts` (initial list and inclusion, message policy, Clear initial, draft saved on list change and on cancel, a full commit clears the default list's comment, directories expand to the changes under them).
- **Dialog:** `opens titled Commit Changes with the focus in the message`, the chooser, the tree and the legend, `Run Git hooks is shown only when the repository has hooks`, amend and its history (`turning amend on saves the earlier text to the message history`), `sign-off is remembered between openings`, `an empty message shows Comment Is Empty and keeps the dialog open`, `passing the checks closes the dialog and then commits`, `Commit and Push… commits then opens the push`, `the Diff panel shows line checkboxes and toggling them changes what is committed`, `the Diff panel, the size and the splitters are remembered`, `a file that disappears while the dialog is open leaves the inclusion and a new one comes in unchecked`, `a file deleted while a warning is open is not sent`, `Cancel keeps the message as the list comment`, `a commit that fails after closing does not push`, and for another repository opened while the commit runs `a commit made leaves the lists alone and is still notified`, `a commit that fails does not write its draft`, `neither the push nor the rebase run`.
- **Entry points:** in `App.test.ts` `in modal mode Commit… and cmd K open the Commit Changes dialog, which keeps the keys while open`, `cmd K opens the dialog from Local Changes and from the Log in modal mode`, `cmd alt K in modal mode opens the dialog`, `in non-modal mode cmd K still focuses the Commit tool window message`, `the commit of the dialog runs after the dialog has closed` (a bug of the dialog found by the first test through the application: no commit ran from the real dialog), `Commit and Rebase… of the dialog rebases from the base once the commit after closing is made`, `a commit of the dialog that fails after closing does not push`, and `a dialog commit made after another repository was opened leaves that repository alone` with its `failed` twin; in `dialogStore.test.ts` four tests of the routing.
- **Fixup, Squash Into, Commit and Rebase…:** `Fixup opens the commit dialog with fixup! subject and Commit and Rebase…`, `without changes in the active list Fixup says Nothing to Commit`, in the dialog `Commit and Rebase… commits and then rebases from the base`, `a commit that fails is not followed by the rebase`, `with HEAD moved since proposing nothing is committed and the rebase is no longer offered`, `HEAD moving while a warning is open stops the commit and the rebase`, and the same case for the Commit view.
- **Who owns the line checkboxes:** `without a commit session the diff shows no line checkboxes`, `before the application says who owns them there are no line checkboxes`, `line checkboxes in the window change the inclusion of the open dialog`, `closing the dialog clears the window's checkboxes`, `in non-modal mode the commit view still owns the line checkboxes`, and in the application `a diff window opened from the Commit Changes dialog follows the dialog and is cleared when it closes`, `switching to modal mode takes the line checkboxes away from an open diff window`, `a line lost by a commit that failed after the dialog closed reaches the diff window`.
- **Checks before the commit:** in Rust 21 tests (for example `a_file_over_the_limit_warns_with_its_size`, `lfs_turns_off_the_size_check`, `a_file_chosen_by_lines_is_measured_whole_on_disk`, `in_stage_mode_the_size_of_the_staged_blob_is_checked`, `an_added_file_named_aux_warns`, `a_renamed_file_with_a_colon_warns`, `warnings_come_in_intellijs_order`); in the interface the warnings over the dialog, `cancelling the file name warning closes the dialog and keeps the draft`, `cancelling the large file warning keeps the dialog open`, `cancelling the detached HEAD warning closes the dialog`, and the sequence in the non-modal view (with `with every file gone while a warning is open nothing is committed`).
- **Support:** `set_comment_keeps_name_and_id`, `a_list_comment_for_another_repository_is_not_saved`, `a_rejected_list_comment_writes_nothing`, `history_keeps_twenty_five_messages`, `the_last_message_is_kept_apart_from_the_history`, `commit_hooks_are_an_executable_pre_commit_or_commit_msg`, `commit_hooks_are_looked_for_where_git_says`; the two tests that count commands (`mutating_without_saving_the_diff_is_not_allowed`, `whatever_touches_the_repository_saves_the_diff_window_first`) did not need new counts, because the new commands do not touch the repository.

Mutation checks that the tasks did: removing `readonly` from the preview, making the arrow keys always focus the tree, reverting the `selectionLost` gate and reverting the fix of the commit after close each make a test fail. In the final review's fixes, removing the `sameRepo` predicate that the application gives the dialog makes both tests of another repository fail, and removing the empty-request guard of the Commit view makes its test fail.

### 5. Checklist to verify by hand

None of this could be checked automatically. Use a test repository first and then a real one. The `.app` was not rebuilt in this part. To build it and open it:

```bash
export PATH=$HOME/.cache/rama-toolchain/node-v22.23.4-darwin-arm64/bin:$HOME/.cargo/bin:$PATH
pnpm tauri build --bundles app
open -n src-tauri/target/release/bundle/macos/rama.app --args /path/to/a/repository
```

**The dialog in a real window**

- [ ] ⌘K opens `Commit Changes` at 900×700, centred, with the focus in the message. Resize it from the corner, move both splitters, close the `Diff` panel, cancel and reopen: size, splitters and the panel state come back.
- [ ] The look next to IntelliJ's with screenshots: the chooser, the tree, the legend, the options column, the split `Commit` button and its drop-down.
- [ ] Tab stays inside the dialog; Esc cancels; Esc inside the chooser's list or a nested dialog (Rollback, Move) closes only that.
- [ ] Changelist chooser: switching the list saves the message to the old list and loads the other's; a long name is cut with `...`.
- [ ] A commit with a message, with the message history (⌃M), with `Amend commit` (⌃⌥M), with `Sign-off commit` and with a failing `pre-commit` hook (the draft must survive); `Comment Is Empty`.
- [ ] Commit File… of a file, of a directory and of an unversioned file; the list and the inclusion must be the ones IntelliJ picks.
- [ ] Change files from outside (as Devin does) while the dialog is open: a deleted file leaves the inclusion, a new one comes in unchecked, and Commit sends only what is shown.
- [ ] After a commit: the notification, the log reloads, and an emptied inactive list is offered for removal.

**⌘K and the other shortcuts with a real keyboard**

- [ ] ⌘K from Local Changes, from the Log and Console tabs, from the Git menu `Commit…` and from the branches popup `Commit…`.
- [ ] ⌘⏎ commits inside the dialog; ⌥⌘K in the dialog opens `Commit and Push…`; ⌥⌘K outside it opens the dialog.
- [ ] ⌘9 shows the Git window; ⌘9 again with it in front does nothing; ⌘0 does nothing in modal mode and shows the Commit window in non-modal mode. The arrow keys in the tab strip keep the focus on the strip.

**The Diff panel and the diff window with line checkboxes**

- [ ] In the dialog's `Diff` panel, check and uncheck lines, and make a real partial commit; compare what git committed with the checkboxes.
- [ ] A double click on a file of the dialog's tree opens the diff window with line checkboxes; the ones changed there reach the dialog's tree (the checkbox turns mixed); closing the dialog (commit or cancel) takes the checkboxes away.
- [ ] A double click on a file in Local Changes opens the diff window without line checkboxes. `Preview Diff` shows the diff without checkboxes and can be hidden; its right-click menu offers `Rollback Lines` and `Move Lines` (ruling R3).
- [ ] The diff window when the file changes from outside while a line is chosen: the warning for a lost selection.

**Switching the mode**

- [ ] Settings > Version Control > Commit: tick `Use non-modal commit interface` and Save. The stripe changes to `Commit` and `Git`; the views blink once; `Enable staging area` is enabled. Untick and Save: back to one `Git` button.
- [ ] A message typed in the Commit view and a line chosen in the diff window survive the switch to modal and back.
- [ ] With the staging area saved as on, modal mode must show changelists and `Git.Stage.*` actions must not appear.
- [ ] `Clear initial commit message`: with it on, the dialog opens with an empty message (but a Fixup text is kept).

**Fixup and Squash Into with Commit and Rebase…**

- [ ] On a branch of yours that you can rewrite, with changes in the active list: Fixup… and Squash Into… from the log open the dialog with `fixup! subject` or `squash! subject`, and `Commit and Rebase…` is in the drop-down; it commits and then rebases for real.
- [ ] Move HEAD from a terminal before pressing it: nothing is committed and the item goes away. The same with a rebase in progress.

**The large file and file name warnings**

- [ ] A commit with a file of 60 MB: `Commit Check Failed` with the count and size over the dialog; Cancel keeps the dialog open; `Commit` continues. With Git LFS configured it must not appear. The limit and the two checkboxes in Settings.
- [ ] A new file called `aux.txt`, and one with `:` in the name: `File Name Is Not Allowed on Windows`; Cancel closes the dialog and keeps the message as the list comment; Commit continues.
- [ ] A detached HEAD and a rebase in progress: the warnings, and Cancel closes the dialog. In the non-modal view Cancel just stops.

**Side by side with IntelliJ**

- [ ] Open the same repository in IntelliJ 2024.1 in modal mode and compare the initial list, the initial inclusion and the initial message in each entry point (⌘K, Commit File…, Fixup).

### 6. What could not be checked

- **Nothing was seen in a real window.** The dialog, the Git window, the Local Changes tab with its splitter, the diff window with checkboxes and the warnings were tested in jsdom, which does not lay out a page: sizes, splitters, the grip, focus after a window opens and the look of the split button have not been seen.
- **No physical keyboard.** ⌘K, ⌥⌘K, ⌘9, ⌘0, ⌃⌥M and ⌃M were driven by simulated key events.
- **No measurement.** Memory, CPU at rest, startup and the cost of opening the dialog on a large repository were not measured. The measurement gate of phases 2 and 3 needs the Mac unlocked and untouched, and it was not run.
- **No comparison with the real IntelliJ.** The rules (inclusion, list, message, warnings, order of the checks) come from reading the 241 sources, not from running both programs on the same repository. The 241 texts were copied from its bundles.
- **The real IPC of the new commands** (`changelists_set_comment`, `messages_last`, `messages_push`, `repo_has_commit_hooks`) has not been exercised through Tauri; the tests call the Rust functions and simulate `invoke`. A failure there would be one of registration in `lib.rs` or of argument names. Check that the dialog opens with the last message, that a commit adds to the history and that no console message says `command … not found`.
- **The file name check** is skipped on Windows and runs everywhere else, like IntelliJ's handler (`cfg!(windows)`): it warns about names that Windows would not accept. It was tested with throwaway repositories on this Mac only.
- **The conflict nodes and the Merge flow inside the dialog** were tested only through the shared panel.

### 7. Verification

Run on 2026-11-04 on `f02b72f` with the fixes of the final review, one suite at a time, with the toolchain of `~/.cache/rama-toolchain`:

| Command | Result |
|---|---|
| `pnpm exec tsc --noEmit -p .` | no errors (exit 0) |
| `pnpm exec vitest run --maxWorkers=2` | 78 files, 1,761 tests passed |
| `cargo test --lib -- --test-threads=4` | 1,610 passed, 0 failed, 5 ignored (on purpose) |
| `cargo clippy --all-targets -- -D warnings` | no warnings (exit 0) |
| `python3 -I -m unittest discover -s scripts` | 65 tests, OK (at `d0b039a`; no script changed afterwards) |

The tests of phase 3 at its close were 1,574 in the Rust library, 1,637 in the interface and 55 of the scripts; part 1 adds 36 tests in Rust, 124 in the interface and 10 in the scripts (the classifier of Task 1). `cargo fmt --check` was not part of the verification: it reports differences in files this part did not touch (`branch_info.rs` among them), and the files the part changed were formatted.

### Conclusion

The behaviour that can be tested without a screen and without a keyboard is tested, with real git where it matters, and follows what the 241 sources say, except for the differences in section 2. Part 1 is not validated. It ends when you complete the list in section 5 and the points in section 6 that need your machine are settled.
