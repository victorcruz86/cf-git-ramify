# Phase 4: validation report

Date: 2026-11-04. Phase 4 is delivered in parts; each part adds its section to this report. Decisions of the phase: `docs/superpowers/plans/2026-11-03-phase-4-overview.md`. Decisions of part 1: `docs/superpowers/plans/2026-11-03-phase-4-1-modal-commit.md`. What changed when the reference moved from IntelliJ 2023.2.3 to 2024.1: `phase-4-reference-change.md`. Part 6 closes the phase; the `Phase 4 summary` at the end gathers every part's result, what stays open across the parts and the consumption.

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
- **Smaller pieces.** The message history keeps 25 entries and the last message is kept apart; `changelists_set_comment`, `messages_last`, `messages_push` and `repo_has_commit_hooks` are new commands that only touch Ramify's own files or read (`changelists_set_comment` takes the repository the comment was written for and refuses it if another one is open); the sign-off is remembered per repository.

### 2. Decisions and rulings that depart from IntelliJ

Each one with what it costs if it turns out to be wrong.

**Decisions of the plan (1 to 9)**

1. **The default is modal.** `non_modal_commit` is `false`; IntelliJ 2024.1 defaults to non-modal. It is how the user has IntelliJ. Cost: whoever did not choose it finds the dialog first.
2. **One `Git` stripe button in modal mode.** Ramify has no editor area, so the Git window fills the content area, and Log and Console are tabs instead of stripe buttons. Pressing ⌘9 with the window in front does nothing; IntelliJ would hide it. Cost: the layout changes for whoever used the earlier stripe.
3. **A double click in Local Changes opens the diff.** IntelliJ opens the source by default (`SHOW_DIFF_ON_DOUBLE_CLICK=false`); Ramify has no viewer until part 4, which adds the `Show on Double-Click` item and the viewer. Cost: until then a double click shows the diff, not the file. (Part 4 added the viewer, and its final fix wave the item: a double click or Enter now opens the source in the viewer, and `Show on Double-Click` ▸ `Diff` in the Git window's gear brings back the diff; part 4, section 3.)
4. **`Preview Diff` is included**, visible by default, as a read-only embedded diff without line checkboxes. Nothing loads while it is hidden. Cost: a diff that loads on every selection change.
5. **No `DiffCommitMessageEditor`.** The diff window opened from the dialog does not show a second editor for the message; Ramify edits the message only in the dialog. Cost: small.
6. **Dialog size.** First opening 900×700, centred; afterwards the size, both splitters and the `Diff` panel state are remembered in `localStorage`. Cost: the first opening may differ from IntelliJ's preferred size.
7. **The message history keeps 25 entries**, as IntelliJ does; it kept 20. Cost: none.
8. **No code checks and no `Commit Message Inspections` table.** `validate.ts` keeps its fixed rules; `Commit Checks` shows only `Run Git hooks`, and it is hidden when the repository has no hooks (IntelliJ shows the group title). Cost: none for Git-only use.
9. **The setting switches on Save**, not live. The views are remounted. Only the dialog's draft lives in the changelist comment; the draft of the non-modal Commit view and its line selections are handed over in memory when the mode switches (`App.svelte`, tested), so quitting the app while in modal mode loses that draft. Cost: the views blink once, and a non-modal draft does not survive quitting in modal mode.

**Rulings taken during the part**

- **R2: a tool window comes back on its last tab; the first content is selected only on a mode switch.** The task text said ⌘9 selects "the first content"; `ChangesViewContentManager` in 241 selects it only when the mappings change. In modal mode there is one window, so the visible behaviour is the same. Cost if wrong: one function (`activateWindow`).
- **R3: the read-only `Preview Diff` keeps the gutter menu with `Rollback Lines` and `Move Lines`.** Decision 4's "read-only" means no typing and no line checkboxes; the gutter actions go through the existing commands, which flush the diff window and take the repository locks. IntelliJ's preview offers move (editor popup) and revert (gutter arrows) with different texts and UI (ruling R10). Cost if wrong: hide two menu items in `FilePane` when it is embedded and read-only.
- **R5: `Author:` is a plain field.** There is no combo of known authors and no `Author differs from default` balloon, and no `list.author` on changelists. They need a command that reads authors from the log and an author model on the changelist. Cost if wrong: one combo and one command, later.
- **R8, corrected by R9: the size measure of the large-file check.** IntelliJ uses `VirtualFile.length` (the file on disk), and so does Ramify, with two differences: a symlink is measured as the link (git stores the link text) and, in staging area mode, the staged blob is measured. Deleted files are skipped. A file with a partial selection is measured whole on disk, as in IntelliJ: the application always sends `partial: []` and adds the files chosen by lines to the measured ones (`warnings_for`). The backend can measure a partial content by what goes into the index (`a_partial_file_is_measured_by_what_goes_into_the_index`), but nothing calls it that way. Cost: a size that differs from IntelliJ's disk length when a staged file was edited afterwards (stage mode only), or for a symlink to a large file; a file near the limit can warn in one and not in the other. A partial commit of a big file warns although the staged part is small, as in IntelliJ. The `Int` overflow of `limit * 1024 * 1024` in Kotlin is not ported (u64 math). Lines chosen in the `Diff` panel or the diff window are measured as whole files, as the CRLF check already does.

**Rulings that follow IntelliJ more closely or add a check (listed because they changed behaviour)**

- **R4: the request is rebuilt after the warnings**, in the dialog and in the Commit view, so Commit sends only what the dialog shows at that moment (a file deleted while a warning was open is not sent). Cost if wrong: one extra `chooseCommit` call.
- **R6: the HEAD guard of `Commit and Rebase…` is checked again after the warnings**, in the dialog and in the Commit view, and a `rebaseFrom` without the `head` that was seen counts as stale. Cost if wrong: two extra `repo_state` calls.
- **R7: Cancel on the file name warning and on the detached HEAD and rebase warnings closes the commit dialog**, keeping the draft as the changelist comment like any cancel (241 returns `CLOSE_WINDOW`); Cancel on the large file and CRLF warnings keeps it open. In the non-modal view there is no window to close: Cancel just stops. Cost if wrong: the user loses the open dialog (not the message) after cancelling those warnings.

**Ruling on the matrix only**

- **R1: `Use non-modal commit interface` is a row of phase 4**, not 1, because it is introduced in phase 4. It changes one cell of the matrix and no behaviour. Cost: one cell.

**Other differences that came with the tasks**

- The warning buttons of the large file and file name dialogs say `Cancel` and the commit action (`Commit`, `Commit and Push`…), as the 241 source of the modal dialog does. The plan said `Commit Anyway`, which in 241 is the text of the non-modal notification.
- `Sign-off commit` has an empty signature in its tooltip, because Ramify does not read the git user.
- The `Diff` panel shows the first file of the list when nothing is selected.
- Conflicts keep their node without checkbox in the dialog and in Local Changes.
- A draft written when the list is changed in the chooser is saved at once; IntelliJ saves it when the dialog closes.
- The dialog has its own frame instead of the shared `Dialog.svelte`, because it is resizable and holds nested dialogs of the panel; the focus trap and Esc handling are repeated there.
- ⌥⌘K outside the dialog in modal mode opens the dialog; `Commit and Push…` is then chosen in it. In 241 the action is invisible and disabled without a commit workflow (`BaseCommitExecutorAction.update`), so IntelliJ does nothing there. Cost: a shortcut that acts where IntelliJ's does not; whoever presses it by habit outside a commit gets the dialog instead of nothing.

### 3. Known gaps a user could notice

From the items the reviews deferred:

- **No user name check.** IntelliJ's `GitUserNameCommitProblem` is the first check of the handler order and Ramify has none. With no `user.name` the commit fails in git instead.
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
- **Local Changes:** `Local Changes shows the changelists without checkboxes`, `the toolbar follows the order of ChangesViewToolbar`, `the popup follows the order of ChangesViewPopupMenu`, `Preview Diff shows the selected file's diff without line checkboxes and is shown by default`, `double click opens the diff` (since part 4's final fix wave `double click opens the source in the viewer` and `double click opens the diff with Show on Double-Click set to Diff`), `Commit… with nothing selected asks for the whole list and Commit File… for the selected files`.
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
export PATH=$HOME/.cache/ramify-toolchain/node-v22.23.4-darwin-arm64/bin:$HOME/.cargo/bin:$PATH
pnpm tauri build --bundles app
open -n src-tauri/target/release/bundle/macos/Ramify.app --args /path/to/a/repository
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
- [ ] A double click on a file in Local Changes opens it in the viewer; with `Show on Double-Click` ▸ `Diff` (the Git window's gear, since part 4's final fix wave) it opens the diff window without line checkboxes. `Preview Diff` shows the diff without checkboxes and can be hidden; its right-click menu offers `Rollback Lines` and `Move Lines` (ruling R3).
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

Run on 2026-11-04 on `f02b72f` with the fixes of the final review, one suite at a time, with the toolchain of `~/.cache/ramify-toolchain`:

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

## Part 2: alignment with IntelliJ 2024.1

Date: 2026-11-07. Plan: `docs/superpowers/plans/2026-11-04-phase-4-2-reference-alignment.md`. Starting point: `phase-4-reference-change.md`. The part has 19 commits (`9f4e9c7..12b3aef`) in 12 tasks, each reviewed, plus this report, the code fix of Task 12 (`fix: exact spacer widgets and the MIT notice of the spacer model`) and the fixes of the final review. The commits from this report on are named by subject: their hashes changed when their trailers were corrected.

### Summary

- Part 2 is done in essence: what phases 1 to 3 built against 2023.2 behaves as 2024.1 in the copied texts, the new confirmations and options, the branches popup search, the merge and rebase toolbar widget and soft wraps in the diff.
- **`done` means tests, not use**, as in part 1. The Rust logic is tested with real git and throwaway repositories; the interface in jsdom with the Tauri API simulated. jsdom lays nothing out, so **no soft wrap was ever seen wrapping**: the wrapping itself, the level of the aligned diff and the jumps when estimated lines are measured are only proven for the pure arithmetic and for the CodeMirror configuration. The part cannot be accepted without the list in section 5.
- Matrix: 13 rows closed (the 3 of Task 1, `ChangesView.AddUnversioned` and `Git.Add` from `partial` to `done`, the popup's `git.branches.popup.filter.by.action`, `Vcs.ShowMessageHistory`, four new rows of section 2, one new setting row and the toolbar widget row that was listed out of scope), 18 rows got a part 2 note without changing status, and 5 rows stay pending with their reason (decision 11, section 2).
- The part had 11 rulings during execution that change behaviour or depart from the plan; the ones that change what the user sees are in section 2, each with its cost.

### 1. What was built

- **Copied texts (Task 1).** About 25 texts follow 241: `Could not …` instead of `Couldn't …`, `Dropped N commits`, `Cannot undo drop`, `Could not save uncommitted changes`, `View and reset filters`, `Filter by Commit Message or Hashes` with the placeholder `Text or hash`. The unmerged files and unstash conflict cards use 241's text and offer `Resolve conflicts…`. Rust constants are pinned by tests (Task 1 fix round, and the pinned texts of `5ad5c44`).
- **Drop Commits asks (Task 2).** `Drop Commits` / `Do you want to drop N commit(s)?` with OK by default and `Don't ask again`; setting `Ask for confirmation to drop commits` in a new `Confirmation` section; the success notification lists up to 10 subjects.
- **Merge and Rebase options (Task 3).** `--allow-unrelated-histories` in Merge and `--update-refs` in Rebase, last in their option lists.
- **Edit Commit Message (Task 4).** `Cancel Reword` in the interactive rebase dialog; message history button and ⌃M in the reword and squash dialog; `historyMenu` shared with the commit dialog.
- **`Rebasing Published Commit` (Task 5).** Update with rebase, `Update Selected`, `Pull into … Using Rebase` and the auto-update of a rejected push ask `Rebase Anyway` / `Cancel`, before anything is saved. `Pull…` does not ask, as in 241.
- **Popup merge and Push up to Here (Task 6).** The branches popup merges the full ref of the clicked item; `Push All up to Here…` no longer needs a local branch.
- **Branches popup search (Task 7).** It filters actions too, with `Show Actions in Search Results` in the gear, a matching action selected first, 375×300 minimum size.
- **Fetch and protected patterns (Task 8).** Fetch is disabled while any fetch of the app runs (menu, popup and Branches panel read one shared flag); protected branch patterns are checked while typing.
- **Add to VCS (Task 9).** `git add --ignore-errors -A -- <paths>` from Local Changes and from the commit dialog's browser, ⌥⌘A, files land in the active list (the dialog's list in the dialog) and are included in the dialog. The commit pipeline needed no fix: a file added this way stays added and out of the commit of another list.
- **Small alignments and the widget (Task 10).** `INTELLIJ_GIT_EXECUTABLE=true` in every git command; `hasExternalSSHAgent` for a symlinked launchd path; a row of icon buttons after the branch widget with `Resolve Conflicts…`, `Continue Rebase` and the aborts.
- **Soft-Wrap (Tasks 11 and 12).** The option `Soft-Wrap` in the gear of the diff window and in the gutter menu of every diff and merge pane, stored per place. **The places are five, not four:** `Default`, `CommitDialog`, `ChangesView`, `Merge`, plus `VcsLogView`, which Task 11's fix round added for the docked diff of the Log and the Push dialog (default off, as in 241). Task 12 replaces the line-count spacers of the aligned diff with spacers measured in pixels when wrap is on (`src/diff/spacers.ts`, a pure function, and a measuring pass in `editor.ts`), and gives both number gutters the same width so equal lines wrap at the same width. `NOTICE` credits the ported algorithm.

### 2. Decisions and rulings that depart from IntelliJ

Each one with what it costs if it turns out to be wrong.

**Decisions of the plan (1 to 11)**

1. **`Rebasing Published Commit` asks before saving.** 241 saves the changes, asks, and Cancel leaves them in the stash with `Local changes were not restored`. Ramify asks first, as it already does for `Rebasing Merge Commits`. Cost: the order differs from IntelliJ and there is no "not restored" card after Cancel.
2. **The unstash conflict card has 241's text and no stash message line.** `View saved changes…` comes with the Stash tab in part 3. Cost: until then the card does not name the stash; `git stash list` does.
3. **Protected patterns are checked while typing, saving is not blocked, and the notification on save is gone.** The cause is the Rust `regex` crate's text, not Java's. Cost: the cause reads differently from IntelliJ's.
4. **`INTELLIJ_GIT_EXECUTABLE=true` is set for every git command.** The constraint "do not use the name of IntelliJ" was read as covering user-facing text and logos, not an environment variable that only hooks read. Cost: a hook that identifies IntelliJ by this variable takes Ramify for IntelliJ; one line to remove in `git.rs`.
5. **Fetch is disabled for the whole Update, Pull included.** 241 disables it only while a fetch request is open. The flag is raised for the `git pull` call, not while the Pull dialog is open. Cost: during the merge or rebase phase of an Update, Fetch stays disabled a few seconds more.
6. **`Add to VCS` is `git add --ignore-errors -A -- <paths>`, as 241.** Cost: if the pipeline mishandled the added file, the task would fail review; it did not (`committing_another_changelist_leaves_the_added_file_out`).
7. **The merge and rebase widget is icon buttons after the branch widget,** with 241's tooltips and colours. The stop colour comes from `darcula.theme.json` because `expUI_dark` does not define it; the icons are drawn for Ramify. Cost: none expected.
8. **The branches popup is 375×300 at least.** Cost: none.
9. **Soft wrap is stored per place:** `Default` on, the others off (the user's values for the others were not read). Cost: if the user had wraps on in other places, they turn them on once per place.
10. **Wrapping details not copied:** no sticky line number inside a wrapped line, no forced wrap over 100000 characters, wrapped rows start at column 0. Cost: small visual differences on long lines.
11. **Rows that stay out, with their reason in the matrix:** `Git.Stage.ToggleCommitAll` (and `Git.Stage.Configure`, which the matrix has no row for) are non-modal staging only; `Git.Stage.Stash.Files` needs the stash of part 3; `Vcs.Log.OpenAnotherTabInEditor` and `Vcs.Diff.EnableDisableCombinedDiff` need editor tabs; `Vcs.Diff.ToggleSearchInChanges` needs a search in the diff. Cost: those rows stay pending.

**Rulings taken during the part**

- **Cancel Reword fires only on Esc and Reset** (the triggers research item 5 found in 241). Clicking another row applies the edit; the dialog's own Cancel keeps its `Cancel Rebase` question. Cost if wrong: one more confirmation path.
- **Reset followed by `Keep Editing` keeps editing and does not reset.** 241 resets afterwards. Cost: one line in `reset()`.
- **`published_confirmed` and `rebase_merges` are the branch that was asked about** (`Option<String>`), not a bool. If the current branch changes while `Rebase Anyway` or `Rebasing Merge Commits` is open, it asks again for the new one. Cost: one field type in Rust and TypeScript each, and the branch in the `RebasingMergeCommits` outcome.
- **The push that is rejected and updated automatically also asks `Rebasing Published Commit`,** because in 241 it goes through the same updater. Cost: two tests and `askPublished` in `push/model.ts` if it is to be removed.
- **`Pull…` passes the branch it pulls into as confirmed for both questions** (merge commits and published commits), so it never asks, as in 241. If another branch is current by the time Update checks, `Pull…` shows `Update canceled`.
- **`Add to VCS` failures are a notification,** `Failed to add files` / `The following problems have occurred when adding the files:`, in Local Changes and in the dialog; 241 shows an error dialog inside the commit dialog. No confirmation for ignored files, because Local Changes does not list them. Cost: a notification where 241 has a dialog.
- **A partial failure of `Add to VCS` does not move or include the files that were added anyway** (241 moves what was processed). They keep the list they had.
- **`Resolve Conflicts…` is hidden in the widget during a rebase** (241's `GitResolveConflictsAction.update` for the widget place); the Git menu item is not affected.
- **Equal number-gutter widths only when the diff is aligned and wrap is on,** not in every aligned diff. With wrap off the DOM is today's. Cost: one line in `equalNumbers`.
- **`VcsLogView` is a fifth Soft-Wrap place.** Without it the docked diff of the Log and the Push dialog had no way to wrap. Cost: one place and one gutter menu to remove.
- **`Couldn't read the commit(s)` became `Could not read the commit(s)`,** Ramify's own text changed for consistency. Cost: one string.

### 3. Known gaps a user could notice

From the items the reviews deferred:

- **Soft wrap of an aligned diff is a first version.** Lines below the viewport are estimated until they are measured, so scrolling to them can jump once (section 5). IntelliJ has no jump because its soft wrap model covers the whole document. The gutter menu has only `Soft-Wrap`, while 241's also splices the align item and other editor settings.
- **Combined `DiffWindow` placeholders** are measured with the previous wrap value until the file is remounted.
- **`LogPanel` and `PushDialog` with a docked diff each** would not follow a toggle live from one to the other (only if both are mounted together).
- **`.rm-old-line` wraps with `pre-wrap`,** CodeMirror lines with `break-spaces`: the wrap point can differ for trailing spaces.
- **Closing the Drop dialog by a repository change or Esc counts as Cancel;** a failing settings save after OK aborts the drop (IntelliJ remembers the checkbox and still drops).
- **The unmerged files card is fixed to `merge`** (the `{0}` is never another operation).
- **The widget's Abort/Continue split is detected by label,** not by action id.
- **The branches popup:** the highlight snippet is rendered for action rows with the toggle on also when the query is empty; no test filters the ongoing-operation rows by label.
- **An old invalid-pattern error stays visible** until the debounced check (300 ms) returns; `protected_branches_set` still returns the invalid list and the interface ignores it.
- **`Fetch` has no re-entry guard** in `fetch()` itself; its entry points are disabled instead.
- **`editor.ts` grew to 1,273 lines.** The arithmetic is in `spacers.ts`; the glue is three small methods and a listener.
- **`droppedSubjects` counts code points,** Java counts UTF-16 units (only astral characters differ).

### 4. What was proven with tests

Each feature and the tests that name it. The exact names are in the Result column of `docs/parity-matrix.md`.

- **Texts:** `unmerged_files_text_and_action_follow_241`, `unstash_with_conflicts_says_241s_text_and_offers_resolve`, `the_text_filter_shows_text_or_hash`; in Rust one test per constant (`a_failed_save_says_could_not_stash_or_shelve`, renamed in part 3, `a_failed_save_is_titled_could_not_save_uncommitted_changes`, `an_unidentifiable_git_names_git_with_a_capital`, `a_missing_remote_says_could_not_find_it`).
- **Drop:** `drop_asks_drop_commits_with_ok_and_cancel`, `cancelling_the_drop_question_runs_nothing`, `drop_asks_after_the_check_and_before_reading_the_state`, `dont_ask_again_is_saved_only_on_ok`, `without_the_setting_drop_does_not_ask`, `drop_success_lists_the_subjects`, `the Confirmation section asks to confirm dropping commits and saves it`, and in Rust `show_drop_commit_dialog_defaults_to_true`.
- **Options:** `update_refs_comes_out_as_a_button_and_travels_in_the_answer`, `update_refs_is_saved_last_and_travels_in_the_params`; in Rust `merge_with_each_option_launches_the_intellij_command`, `merge_with_allow_unrelated_histories_joins_histories_without_a_common_ancestor`, `rebase_with_update_refs_passes_the_flag`, `update_refs_is_remembered_per_repository`.
- **Edit Commit Message:** `escape_with_a_changed_reword_asks_cancel_reword`, `escape_with_an_unchanged_reword_closes_silently`, `reset_with_a_changed_reword_asks_first`, `the_reword_dialog_has_the_history_button`, `the_history_button_is_disabled_with_an_empty_history`, `a_changed_message_goes_to_history_on_any_close`.
- **Rebasing Published Commit (Review Focus 1):** in Rust `update_with_rebase_over_a_published_commit_asks_before_saving` (asks, nothing saved, no stash, no snapshot, HEAD and the dirty file untouched), `confirmed_update_rebases_as_before`, `merge_updates_and_pull_do_not_ask`, `a_confirmation_for_another_branch_asks_again`, `a_published_check_that_fails_asks_without_a_branch`; in the interface `the_outcome_opens_rebasing_published_commit_with_cancel_as_default`, `rebase_anyway_repeats_without_another_fetch_or_question`, `cancel_shows_update_canceled`, `it_does_not_continue_in_another_repository`, `pull_into_with_rebase_asks_and_repeats_towards_the_same_branch`, and for the push `rebasing_published_commits_asks_and_rebase_anyway_continues`.
- **Popup merge (Review Focus 2):** in Rust `popup_merge_of_a_tag_with_a_branchs_name_merges_the_tag`, `popup_merge_of_a_branch_with_a_tags_name_merges_the_branch`, `popup_merge_of_a_remote_branch_uses_refs_remotes`, `an_unknown_ref_kind_is_rejected`; `popup_merge_passes_the_kind_of_the_clicked_item_with_the_short_name`.
- **Push up to Here:** `push_all_up_to_here_outside_local_branches_opens_the_push_dialog`, `push_all_up_to_here_on_detached_head_targets_a_new_detached_branch`, `push_all_up_to_here_is_enabled_on_detached_head`.
- **Branches popup (Review Focus 3):** `typing_filters_the_actions_too`, `a_matching_action_is_selected_before_a_matching_branch`, `with_the_toggle_off_actions_stay_and_are_skipped`, `the_gear_starts_with_show_actions_in_search_results`, `the_placeholder_says_branches_and_actions`, `a_separator_sits_between_actions_and_branches_only_when_both_exist`, `the_minimum_width_is_375`, and `filter_actions_*` in `model.test.ts`.
- **Fetch and protected patterns:** `fetch_is_disabled_while_any_fetch_runs`, `the_git_menu_fetch_is_disabled_while_a_fetch_runs`, `the_popup_fetch_is_disabled_while_another_fetch_runs`, `the_panel_fetch_is_disabled_while_another_fetch_runs`, `an_invalid_pattern_is_shown_while_typing`, `a_bad_pattern_does_not_block_saving_and_leaves_no_notification`, and in Rust `a_bad_pattern_reports_the_first_error`.
- **Add to VCS (Review Focus 4):** in Rust `add_to_vcs_adds_the_new_file_to_the_index`, `adding_to_vcs_rejects_foreign_paths`, `committing_another_changelist_leaves_the_added_file_out` (HEAD has only the other file; the index still has the added one), and the two counting tests; in the interface the five tests named in the matrix row.
- **Small alignments and widget:** `a_symlinked_launchd_agent_counts_as_external`, `every_git_command_carries_intellij_git_executable`, the nine tests of `MergeRebaseWidget.test.ts` and three in `App.test.ts`.
- **Soft-Wrap:** `wrap_is_stored_per_place_with_its_default`, `the_gear_has_soft_wrap_before_align`, `right_click_on_the_gutter_offers_soft_wrap` (five files), `vcs_log_view_wraps_from_its_own_place_default_off`, `soft_wrap_wraps_both_sides_without_rebuilding_the_views`, `wrap_off_keeps_todays_configuration`; for the aligned diff (Review Focus 5) the seven tests of `spacers.test.ts` (including `an_unchanged_line_wrapping_only_on_one_side_gets_a_top_spacer_before_the_next_hunk` and `feeding_its_output_back_changes_nothing`) and `with_wrap_on_spacers_follow_the_measured_heights`, `a_second_measuring_pass_dispatches_nothing`, `the_two_number_gutters_have_the_same_width_when_aligned`, `with_wrap_off_spacers_are_line_counts_times_20`.

- **The final fix wave**, each test seen failing first except where it says so: in Rust `every_change_to_the_store_moves_its_generation_and_a_read_does_not`, `repo_changed_says_the_shelf_changed_only_after_a_write_to_its_store`, `the_files_of_a_shelve_are_stored_with_one_git_add`, `a_shelve_that_stores_nothing_makes_no_list`, `a_shelve_started_before_another_repository_was_opened_writes_nothing`, `one_shelf_cleaner_cleans_the_open_repository_on_open_and_then_once_a_period`, `new_files_named_like_pathspecs_are_added_and_ignored_ones_are_not`, `a_dropped_or_cleared_stash_is_recorded_first_and_can_be_stored_again`, `a_drop_whose_record_cannot_be_written_drops_nothing`, `an_erased_list_is_kept_in_shelf_erased_for_thirty_days`, `files_taken_out_of_a_deleted_list_are_kept_in_shelf_erased`, `a_binary_changed_in_head_can_be_overwritten_from_the_dialog`, `a_binary_target_written_after_the_copy_is_left`, `a_list_whose_folder_is_a_link_is_not_read`, `a_copy_of_a_file_with_a_name_of_255_bytes_gets_a_name_that_fits`, `lists_that_differ_only_in_their_binaries_are_both_imported`, `without_head_a_file_staged_then_edited_goes_back`, `a_list_that_is_gone_is_named_the_same_way_everywhere`, `a_copy_of_another_repository_says_whose_it_is`, `a_focus_gained_while_a_loss_settles_keeps_the_app_active`, `the_name_prefix_follows_the_shared_cases` (passed at once: the Rust cut was already right) and the menu tests `the_uncommitted_changes_group_holds_the_stash`, `shelve_changes_is_hidden_in_staging_mode` and `the_git_menu_follows_the_order_of_git_main_menu`, plus two cases added to `overwriting_a_file_of_the_worktree_keeps_a_copy_first` (passed at once); in the interface `a_worktree_save_does_not_read_the_shelf_and_a_change_to_the_store_does`, `a_binary_changed_in_head_offers_overwrite_in_the_unshelve_dialog`, `the_binaries_chosen_to_overwrite_reach_the_apply`, `the_git_menu_import_shelf_from_idea_imports_while_the_shelf_is_empty`, `the_git_menu_rollback_opens_the_rollback_of_local_changes`, `the_main_window_is_inert_while_an_apply_runs`, `a_cancelled_splitter_drag_ends_like_a_release`, `the_patch_name_prefix_follows_the_shared_cases` and the V:363 assertion in `failures_show_patch_applying_partly_failed_and_rollback_restores`.

**What the process left on record.** Several tasks wrote their tests together with the code and have no RED run of their own (Task 1's Rust constants, Task 2, which got a retroactive RED in a temporary worktree at `5ad5c44`, and the `commands.rs` test of Task 8). A weakened assertion (`cancelling_the_push_in_progress_is_not_a_failure`) was found by the review of Task 6 and restored.

### 5. Checklist to verify by hand

None of this could be checked automatically. Use a test repository first and then a real one. The `.app` was not rebuilt in this part. To build it and open it, the commands of part 1, section 5.

**Soft-Wrap in a real window** (research `research-soft-wraps.md` section 5, "Only in a real window")

- [ ] The actual wrapping and the row alignment in the aligned and embedded diffs: a file with a very long line on one side only, and one with the same long line on both sides.
- [ ] The jumps when estimated lines come into view: scroll quickly through a long aligned diff and note any jump of the rows below.
- [ ] Dragging the window width.
- [ ] Expanding folds.
- [ ] A vertical scrollbar appearing and disappearing as a spacer grows (non-overlay scrollbars).
- [ ] Smooth synchronised scrolling across wrapped lines.
- [ ] The curves between the sides spanning wrapped rows.
- [ ] Checkbox and merge-arrow positions on wrapped lines (they must stay on the first row).
- [ ] Performance on large files and on very long lines.
- [ ] A side-by-side comparison with IntelliJ on the same file.
- [ ] The five places keep their value apart: toggle it in the diff window, in the `Commit Changes` dialog's `Diff` panel, in the Local Changes preview, in the merge window and in the Log's docked diff; reopen each. `Align Changes Highlighting` stays shared.
- [ ] The combined `DiffWindow` (several files): the placeholder heights of the files after toggling wrap, with and without remounting.
- [ ] The residual jumps Task 12 lists: (1) scrolling to estimated hunks below the viewport, in the window and in the embedded panes; (2) a spacer growing until a scrollbar appears; (3) dragging the width, one recompute per frame; (4) expanding a fold; (5) typing in the editable side with a long line, which stays out of level until the draft hunks arrive; (6) the top and bottom spacers against IntelliJ on a file with wraps on one side.

**The merge and rebase widget**

- [ ] During a real rebase that stops: `Continue Rebase` and `Abort Rebase` appear after the branch widget, with no `Resolve Conflicts…`; both do what the Git menu does.
- [ ] During a real merge with conflicts: `Resolve Conflicts…` and `Abort Merge`; with a conflict-free merge only the abort. The colours against IntelliJ's run widget.

**The branches popup**

- [ ] Typing in the search on a physical keyboard: a word that matches an action and a branch (`push` with a branch `push-fix`) selects the action first, Enter runs it, the arrows go through actions and branches. With `Show Actions in Search Results` off, the popup behaves as before. The popup is at least 375×300.
- [ ] Fetch is disabled in the Git menu, the popup and the Branches panel while an Update runs.

**`Rebasing Published Commit`**

- [ ] Against a protected branch of a local remote (a bare repository with the branch pushed): Update with rebase over a commit that is already in `origin/main` asks; `Cancel` leaves the working tree as it was, `Rebase Anyway` rebases and restores the changes. Edit a file from outside while the dialog is open.
- [ ] With the dialog open, check out another branch that has an upstream and press `Rebase Anyway`: it must ask again for that branch.
- [ ] A push that is rejected with auto-update on asks the same.

**Commit and drop**

- [ ] `Add to VCS` on a new file, then a commit of another changelist: the new file stays added in the index and is not in the commit. ⌥⌘A on a physical keyboard, in Local Changes and in the dialog's browser.
- [ ] `Drop Commits` with `Don't ask again`: the question appears, the box is remembered only after OK, the next drop does not ask, and the setting in `Confirmation` is off. Undo.
- [ ] `Cancel Reword` on Esc and on Reset with a changed message; the history button and ⌃M in `Edit Commit Message…`.

**Smaller things**

- [ ] Protected branch patterns: a bad pattern shows its error under the field while typing and Save still works.
- [ ] A git hook that prints `$INTELLIJ_GIT_EXECUTABLE`: it prints `true`.

### 6. What could not be checked

- **Nothing was seen in a real window.** The widget, the popup, the dialogs and above all soft wrapping were tested in jsdom. CodeMirror's `lineWrapping` getter follows the class only after a measure pass, so the tests check the `cm-lineWrapping` class; the aligned diff's measuring loop was driven with `lineBlockAt` stubbed.
- **No physical keyboard.** The popup search, ⌥⌘A and ⌃M were driven by simulated key events.
- **No measurement.** The cost of the measuring pass on a large diff, memory and CPU were not measured.
- **No comparison with the real IntelliJ.** Texts were copied from the 241 bundles; behaviour comes from its sources. The colours of the widget come from the theme files of the installed IntelliJ, dark values only (Ramify is dark only).
- **The user's wrap values for the places other than `Default` were not read** (research: not determined), and it is not known whether IntelliJ's commit dialog shows a gear in its `Diff` panel.
- **A real remote.** `Rebasing Published Commit` was tested with a local `origin` in throwaway repositories, not over the network or against a protected branch of GitHub.
- **The real IPC of the new commands** (`changes_add`, `protected_patterns_check`, `branch_merge` with its new argument) has not been exercised through Tauri; the tests call the Rust functions and simulate `invoke`. A failure would be one of registration or argument names; check that no console message says `command … not found`.

### 7. Verification

Run on 2026-11-07, one suite at a time, with the toolchain of `~/.cache/ramify-toolchain`, after the fixes of the final review (the confirmation of `Rebasing Merge Commits` tied to its branch, `Pull…` with rebase answering it, the fetching flag of the push-rejected Update, the untracked menu sync).

| Command | Result |
|---|---|
| `pnpm exec tsc --noEmit -p .` | no errors (exit 0) |
| `pnpm exec vitest run --maxWorkers=2` | 83 files, 1,867 tests passed |
| `cargo test --lib -- --test-threads=4` | 1,637 passed, 0 failed, 5 ignored (on purpose) |
| `cargo clippy --all-targets -- -D warnings` | no warnings (finished, no output) |
| `python3 -I -m unittest discover -s scripts` | 65 tests, OK |

Against the close of part 1 (78 files and 1,761 tests in the interface, 1,610 in Rust, 65 in the scripts), part 2 adds 106 tests in the interface and 27 in Rust, and none in the scripts. `cargo fmt --check` was not part of the verification: it reports differences in files this part did not touch (`branch_info.rs`, `update.rs`, `push.rs`, `pull.rs`, `vcs_commands.rs` in older lines); the lines the tasks added were formatted.

### Conclusion

The behaviour that can be tested without a screen and without a keyboard is tested, with real git where it matters, and follows what the 241 sources say, except for the differences in section 2. Part 2 is not validated. It ends when you complete the list in section 5, with soft wrapping in a real window as its main risk, and settle the points in section 6 that need your machine.

## Part 3: patches, stash and shelf

Date: 2026-11-08. Plan: `docs/superpowers/plans/2026-11-08-phase-4-3-patches-stash-shelf.md`. The part has 28 commits after its plan (`48ae159`): `48ae159..21156cf`, 13 features, 14 fixes and one move-only refactor (`f719d19`), in 11 tasks, each reviewed, plus this report (`8c6b244`). A final review of the whole part (0 critical, 3 important, 29 minor findings) was followed by one fix wave: `eaf4010` (performance and safety), `667a246` (menu, texts and cleanups), `2c2904f` (`cargo fmt` on the part's lines), `c25f56b` (a clippy fix after the format) and the commits of these documents (`5240735` and the next one). Section 2 ends with its rulings.

### Summary

- Part 3 is done in essence: the user saves, moves and restores uncommitted work as in IntelliJ 2024.1. Copy as Patch to Clipboard, Create Patch, Apply Patch and Apply Patch from Clipboard; Stash Changes, Unstash and the Stash tab; Shelve, Unshelve, the Shelf tab and the import of `.idea/shelf`; and `Clean working tree using: Shelve`, the 241 default, for Update, rebase, checkout and Smart Merge.
- **`done` means tests, not use**, as in parts 1 and 2. The Rust logic is tested with real git and throwaway repositories, with IntelliJ's own fixtures for patches and shelves; the interface in jsdom with the Tauri API simulated. Nobody has seen the dialogs, the two tabs or the merge window in contents mode in a real window, nor used a real clipboard, the native choosers, a patch made by IntelliJ or a real `.idea/shelf`. The part cannot be accepted without the list in section 5.
- Matrix: 30 action rows of section 4 got their result (23 `done`, 1 `partial`, 6 `pending` with their reason), section 2 has 7 new rows (4 `done`, 3 not copied by plan decision 10), section 3 closed both `Clean working tree using` rows and has 2 new rows not copied (plan decisions 7 and 10), and 4 earlier rows got a part 3 note (the Git tool window, the Local Changes tab, Update Project and the patch location of section 1).
- 32 rulings were taken during the part (two more lines of the ledger only parked findings for the final review); section 2 lists them with their cost, a ruling and its later correction in one item, then the parked findings and the rulings of the final fix wave. The differences from IntelliJ that a user can notice are in section 3, by feature.
- The final fix wave: the Shelf tab reads its lists only when a command wrote the store, and every shelve reads its files with one `git add`; Drop and Clear keep a record of each stash and what the shelf erases is kept 30 days (how to recover, end of section 2); a binary that HEAD changed since shelving can be overwritten from the Unshelve dialog; `Import Shelf from .idea…` and `Rollback...` in Git › Uncommitted Changes; the main window is inert during an apply.

### 1. What was built

- **Infrastructure (Task 3).** Native file choosers through `tauri-plugin-dialog` 2.8.3 (plan decision 1), granted to the main window only; the clipboard read with `/usr/bin/pbpaste` and Reveal in Finder with `open -R` (plan decision 2); the merge window in a "contents" mode that answers with the merged text instead of writing a conflict; a read-only two-text diff (`TextsDiff`) extracted from the diff view.
- **Patch format (Tasks 1 and 4).** A reader and a writer of IntelliJ's patch format in `src-tauri/src/patch/`: `Index:` and `IDEA additional info:` headers, the `<+>` base and charset lines, git headers, quoting, and binary sections written and read through git (plan decision 3). The writer matches the 25 `PatchBuilderTest` cases byte for byte.
- **Copy as Patch and Create Patch (Task 4).** From Local Changes, the staging area, the Log's changes tree and the Git menu (`Patch` submenu). Create Patch runs as an executor of the commit dialog, with no commit checks (plan decision 8), then `Patch File Settings` (`To file:`, `To clipboard`, `Base path:`, `Reverse patch`, `Encoding:` UTF-8 disabled), the overwrite question and `Successfully created patch`.
- **Apply (Tasks 2 and 5).** `patch_apply.rs` applies exactly, then at an offset, then by a 3-way merge with the base, then through the merge window (plan decision 4), after a recoverable copy, writing a file only if it still holds what was read. `Apply Patch` and `Apply Patch from Clipboard` open the 241 dialog: tree, strip and map actions, Show Difference, the changelist chooser, `Overwrite Existing Files`, `Patch Applying Partly Failed`, `Abort Patch`, and the on-the-fly clipboard offer.
- **Stash (Tasks 6 and 7).** `Stash Changes…`, `Unstash Changes…` and `Unstash…` with both checkboxes honoured (overview decision 11), `Stash Silently` and `Stash Files` with the staging area, and the `Stash` tab: list with branch labels, changes, Apply/Pop, a diff preview, Compare with Local, Cherry-Pick Selected Changes, speed search and splitters. The list is read again only when the stash ref changes.
- **Shelf store and import (Task 8).** The shelf in IntelliJ's layout in Ramify's data folder (overview decision 4), with recycle, delete, restore, rename, the 7-day retention and a repeatable, read-only import of `.idea/shelf` (overview decision 5).
- **Shelving (Task 9).** `Shelve Changes...` (an executor of the commit dialog), `Shelve Silently` (⇧⌘H, one shelf per changelist) and `Save To Shelf`, with a copy before rolling back and an emptied changelist deleted.
- **The Shelf tab and unshelving (Task 10).** The vertical toolbar, the tree with `Recently Deleted`, `Already Unshelved` and `Group By Directory`, the 241 popup menu, inline rename, Delete with Undo, Import Patches…, Import from .idea, Show Diff and Compare with Local, Create Patch and Copy as Patch of a shelf, `Unshelve...` through the Apply dialog in its unshelve mode, `Unshelve Silently` (⌥⌘U), and `Import to Shelf` in the Apply dialog.
- **`Clean working tree using: Shelve` (Task 11).** The radio of the Git settings page is live, per repository, Shelve by default (overview decision 9). The shelf saver makes one system list per changelist and restores each into its list; the changes come back only after a successful update and otherwise stay saved, with `View saved changes…` (overview decision 10). The phase 2 and 3 decisions this overrides are marked in the spec.

### 2. Decisions and rulings that depart from IntelliJ

Each one with what it costs if it turns out to be wrong. In this part, "plan decision N" is one of the eleven below, from `docs/superpowers/plans/2026-11-08-phase-4-3-patches-stash-shelf.md`, and "overview decision N" one of `docs/superpowers/plans/2026-11-03-phase-4-overview.md`.

**Decisions of the plan (1 to 11)**

1. **Native file choosers through `tauri-plugin-dialog`**, for Apply Patch, Patch File Settings and Import Patches…. Cost: one more dependency; typed paths still work in every field.
2. **The clipboard is read with `pbpaste` (`LC_CTYPE=UTF-8`); Reveal in Finder is `open -R`.** Cost: none.
3. **Binary sections go through git** (`git diff --binary` between two blobs to write, `git apply --binary` on a one-entry patch to apply). Cost: the bytes of a written binary section differ from IntelliJ's; see the `delta` ruling below for the one case where IntelliJ reads them differently.
4. **The applier is exact, then at an offset within ±1000 lines, then already-applied, then `merge3` with the base, then the merge window; no base means the file fails.** Split hunks, trimmed context, the "Result of Patch Apply" viewer and context diffs are not copied. Cost: a patch that IntelliJ applies with fuzz fails here and is applied by hand (`partialApply` of `ApplyPatchTest` is such a case).
5. **Paths in Apply are relative to the repository root;** the strip and map actions are copied, `MatchPatchPaths` is not. Cost: a patch made in another layout needs the strip or map actions.
6. **One encoding: UTF-8,** written with `\n`. Cost: no other charsets.
7. **Separate `Stash` and `Shelf` tabs; `Combine stashes and shelves in one tab` is not copied.** Cost: a setting the user has off; the five `Vcs.Shelf.*` rows have no effect.
8. **No commit checks for Shelve and Create Patch,** as 241 skips them for a non-VCS executor. Cost: none.
9. **The patch folder starts at the repository root;** the user's `PATCH_STORAGE_LOCATION` is not read from IntelliJ, and Ramify remembers the last folder per repository and for the app. Cost: the first time, the user browses to `~/Downloads/PATCHS` once.
10. **Not copied:** drag and drop between Local Changes and the Shelf, the `Shelf` settings page, the `Getting started with Stash and Shelf` link and the `Help` buttons. Cost: a few seconds per use for drag and drop.
11. **The shelf base (`<+>` lines) is written from Ramify's reading of IntelliJ's code,** with no fixture at the tag. Cost: if the escape rules are wrong, imported bases fail and those files fall back to "no base" (plan decision 4, last step). The checklist imports a real shelf with a tab and an emoji.

**Rulings taken during the part**

Interfaces and the order of the work:
- Task 5 built the Apply dialog with its `apply` and `clipboard` modes and a `mode` prop typed with all three; Task 10 added the `unshelve` specifics. Cost: none, the work moved between tasks.
- Task 10 added `PatchInput::Shelf { name, files }`, so that unshelving carries the shelf's stored bases. Cost: one enum variant.
- Task 11 could build a minimal unshelve core in Rust before Task 10 (it did: `shelf/unshelve.rs`). Cost: Task 10 adapted to it.
- If the Shelf tab did not exist yet, Task 9 registered `Show Shelf` and tested the tab request that Task 10 consumes. Cost: one call to rewire.
- `Import to Shelf` moved from Task 5 to Task 10, which has the shelf import and the tab. Cost: Task 10 grew by one button.
- `SavedPatchesView` stays the Stash tab's view, and the Shelf tab is 241's classic `ShelvedChangesViewManager` tree with multi-selection, as 2024.1 shows it. Cost: Task 10 was larger than planned.
- The plan's `Missing` status is "the target file is absent" and `MissingBase` is "the hunks do not match and there is no base". Cost if wrong: two labels swapped.
- The applier never aborts or rolls back by itself: the caller calls `patch_rollback` for `Abort and Rollback` and `Rollback`, and folds the merge window's outcomes into the status. Cost: none found.
- `plan` returns a content hash per file and `patch_apply` takes them as `expected`; a file that changed since the plan fails. Cost: none beyond the signature.
- Implementers worked in this checkout, two at a time only with disjoint files, and staged only their own hunks. Cost if wrong: a mixed commit, caught by the review.

Behaviour:
- **Patch creation takes the repository locks.** The first ruling (flush the diff window, no lock, because the patch is written outside the repository) was withdrawn: the default target is inside the worktree, so `patch_create` takes `RepoLocks` and keeps a copy of a worktree file it overwrites. Cost: creating a patch waits behind a running operation; IntelliJ does not lock.
- **git's `delta` binary sections are kept in written patches,** because Ramify reads and applies them through `git apply --binary` and git has no switch to force `literal`. Cost: a Ramify patch with a modified binary, applied in IntelliJ 2024.1, writes an empty file for that entry (241 decodes `delta` as 0 bytes, confirmed by the Task 1 review).
- Task 1's minor findings were fixed at once (error-path tests, a patch-like alphabet in the property test, a binary entry that git prints as text fails instead of becoming text, hunk numbers past `u32` refused, built bases normalised to LF), because Tasks 2 and 4 build on them. Cost: one fix round.
- **Every path and mode from a patch is untrusted:** paths through `snapshots::check_path`, modes limited to 100644, 100755 and 120000, `--` before paths. Cost: an exotic valid mode (none known) fails its file.
- **A binary entry with mode 120000 fails its file;** no symbolic link is created from a patch. Cost: a binary symlink patch is applied by hand. When nothing reaches the writer, no copy is taken.
- **`check_path` refuses a real parent folder inside `.git`**, for every caller (the applier's `x -> .git` case). A first version also refused a final link pointing into `.git`; it was narrowed: a final link is copied as a link by every caller (commit, rollback, stash, snapshots), and the applier refuses to write through any final link. Cost if wrong: none found; the applier's own refusal is the guard.
- In the same round: `chmod` only on a regular file, a mode error after the write is a message and the move still runs, and the aside name of a binary overwrite never replaces an existing file. Cost: none.
- **When every file is skipped, the status is PARTIAL,** following `ApplyPatchStatus` and `PatchApplier` over the research note. Cost if wrong: one notification text.
- **Apply Patch from Clipboard:** an empty clipboard says `Cannot Apply Patch Now`, and a text that is not a patch opens the dialog with `No changes detected`. 241 enables the action on any clipboard text, and Ramify cannot disable a native menu item by the clipboard's content. Cost if wrong: two lines.
- **The Apply dialog's changelist chooser follows `ApplyPatchDifferentiatedDialog` with `CREATE_CHANGELISTS_AUTOMATICALLY` off,** not the brief's wording: with a message only the comment is set; without one, a list named like the file is selected if it exists. Cost: none known.
- **Copy as Patch to Clipboard is not in the Log's changes tree:** 241's `Vcs.RepositoryChangesBrowserMenu` has only `Create Patch...`. Cost if wrong: one menu entry.
- **Stash Files takes paths literally** (`--literal-pathspecs`), so `*.txt` or `:(glob)x` is a file name. Cost: none.
- **The unstash conflict texts come from a marker** that records HEAD and the conflicted paths; it holds only while HEAD is the same and the unmerged paths are among those recorded, and it is deleted when none is left. Cost if wrong: a later conflict on the very same file at the same HEAD still shows the unstash titles (texts only).
- **The Unstash dialog's `View` shows the stash's changes against local** (the Stash tab's changes browser), and only a load error becomes a notification. Cost: one task carried it.
- **The stash list is read again only when the stash ref changes:** the watcher marks a change of `.git/refs/stash`, `.git/logs/refs/stash` or `packed-refs` over its debounce window. Cost: none, but a change of the Git executable does not reload it until the next stash change.
- **The stash changes menu gets `Cherry-Pick Selected Changes`** through the existing `apply_paths` command; `Edit Source` and `Open Repository Version` stay out. Resizable splitters under IntelliJ's keys and ⌘D in the changes tree were added; next-difference inside a file was not. Cost: one shortcut missing.
- **Overview decision 10 goes fully into the code:** the saved changes come back only after a successful update, for both savers, and the three update tests that pinned phase 2's restore-after-error were changed. Cost if wrong: after a failed update the user restores from the Shelf tab or `View saved changes…`.
- **Shelf conflicts of the automatic restore go through the Shelf tab:** the conflicting file stays in its system list, `View saved changes…` selects the list, and `Unshelve...` opens the merge window (241 opens it at once). Cost: one more step; nothing is lost.
- **This build's stash saver writes no `Pending` record;** `Restore Changes` is left only for the records of earlier builds. Cost if wrong: one button less after reopening.

**Findings parked for the final review**
- Task 1's minors 7 (a duplicated regex and zero hash) and 8 (split `patch.rs`) were parked to Task 4. Cost: a larger file for one more task. Task 4 did both (`f719d19`).
- Task 2's M11 (move `patch_apply.rs`'s tests to their own file) was parked. Cost: a 3,400-line file. Done in the final fix wave (`patch_apply/tests.rs`).
- Task 8's M4 (the import key ignores the binary copies) and M6 (one sleeping cleanup thread per open), and the patch-name cut that differed between `shelf.rs` and `src/patch/flow.ts` at a surrogate pair, were parked. Cost: a list that differs only in its binaries not imported; a few idle threads; a different name between a shelf folder and a patch file. All three were fixed in the final fix wave.

**Rulings of the final review and its fix wave**
- **The Shelf tab reads its lists only when a command wrote the store** (findings 1 and 2 together). Every write to a store moves its generation; `repo-changed` carries `shelf: true` once after the open repository's store changed, and the daily cleanup sends it when it changed something. Cost: a store changed by another program (none is known; the store is Ramify's own) is not seen until the next write or the next open.
- **Shelve, Shelve Silently, Save To Shelf and the Update saver read their files with one patch reader:** one copy of the index and one `git add` for all of them (`create::changelist_contents`), in the new `src-tauri/src/shelf/shelve.rs`, which also holds the user's shelving moved out of `vcs_commands.rs`. Results are the same as one file at a time, and the pre-rollback content check is kept. Cost: none; if storing all the files together fails, each one is stored again on its own.
- **`Import Shelf from .idea…` in Git › Uncommitted Changes after Show Shelf** (P26), Ramify's own text and id (`Shelf.ImportFromIdea`), running the Shelf tab's import. Cost: one menu entry IntelliJ does not have.
- **A trail of what Drop, Clear and the shelf's erase remove** (finding 5): IntelliJ's confirmations stay, each dropped stash's `{sha, message, time}` is appended to `<data>/dropped-stashes.json` before the drop or clear (nothing is dropped without it), and an erased list's folder and XML are moved to `<data>/shelf-erased/` and pruned after 30 days, like the snapshots. Cost: a little disk; no interface for the recovery; a record that cannot be written (a full disk) blocks Drop and Clear.
- **`Overwrite` per row in the Unshelve dialog for a binary that HEAD changed since shelving** (finding 6); the recoverable copy is taken first. Cost: one choice more than IntelliJ, which writes it without asking.
- **`ChangesView.Revert` in Git › Uncommitted Changes, routed to Local Changes' Rollback** (finding 8), with `action.ChangesView.Revert.text` as its text (`Rollback...`, three dots like the menu's other ActionsBundle items). `Vcs.UmlDiff`, an empty group in the Community Edition, is not shown. Cost: none.
- **V:363 is kept and a second line names Restore Snapshot… of Local Changes** (finding 9). Cost: a line IntelliJ does not show.
- **`cargo fmt` only on the lines this part wrote** (finding 10): rustfmt's changes were kept where the lines they replace come from `48ae159^..HEAD`. Cost: `cargo fmt --check` still reports 27 places in 14 files, all older code; formatting is no gate.
- **`check-ignore` gets `./<path>`** (finding 3), so a new file named like pathspec magic (`:(glob)x`) no longer keeps every new file of an apply out of the index. Cost: none.
- **One lookup of a list by name, with one text, `"<name>" is no longer in the shelf`** (finding 7), and one `millis`. Cost: none.
- **A shelve that stores nothing makes no list** (P21): Shelve Changes says V:489 (`only directories`) and Shelve Silently skips that changelist. Cost: V:489 names directories also when the cause is a submodule whose folder is gone.
- **Restore names the repository a copy belongs to** (P16) for the patch overwrite's Restore and Local Changes' Restore Snapshot… and Rollback notifications. Cost: none; the diff window's restores keep the generic error.
- **The main window is inert while an apply runs** (P22), the toolbar and the tool windows; the dialogs, the notifications and the status bar stay live. Cost: the log cannot be looked at while a conflict waits in the merge window, as in IntelliJ.
- **A focus gained while a loss settles wins** (P22, a generation counter). Cost: none.
- **Without HEAD, `git rm --cached -f`** (P24), on the index only and only for paths found as they were read. Cost: none.
- **A binary target is checked again right before it is written** (P25). Cost: a much smaller window remains between that check and the rename.
- **The import key counts the binary copies** (P8); a record of an earlier build matches whatever copies a list has. Cost: none.
- **A list whose folder is a link is not read** (P11); **a copy's stem is cut so that its numbered name fits in 255 bytes** (P14). Cost: such a copy loses the end of its stem.
- **The patch file name keeps a surrogate pair whole at the 90-unit cut**, as the shelf does, pinned by `src-tauri/testdata/patch-names.json`, which both suites read (P10). Cost: the shelf still also stops at 220 UTF-8 bytes and a patch file name does not, so 90 three-byte characters give a longer patch name than shelf folder.
- **One shelf cleaner thread for the app** (P9); a repository opened again starts its day again. Cost: none.
- **`clipboard_read` and its wrapper are removed** (finding 4); its tests are now `paste_with_returns_utf8` and `paste_with_reports_failure`. Cost: none.
- Not fixed: P7 (the watcher assumes `.git` is a folder; its fix belongs with linked worktrees, which Ramify does not open yet; cost: such a repository is not watched for index and stash changes; **closed in part 6** by Task 1, `3c72a38` and `2d06c52`: the watcher learns the git folder and the common folder from git, and a linked worktree sees its index and stash changes, test `a_linked_worktree_sees_index_branch_and_stash_changes_from_outside`), P20 #5 (squashing `052fe5b` into `81c48cc`: history is never rewritten; cost: `052fe5b` alone does not build), P23 (process notes, no code).

**How to recover what was dropped or erased.** `<data>` is the repository's data folder, `~/Library/Application Support/dev.ramify.app/repos/<SHA-256 of the repository's canonical path>/`.
- A dropped stash: find its line in `<data>/dropped-stashes.json` and run, in the repository, `git stash store -m "<message>" <sha>` with its `message` and `sha`. It is back in the Stash tab with its message. This works until git prunes the unreachable objects (`gc.pruneExpire`, two weeks by default).
- An erased shelf list: `<data>/shelf-erased/<time>-<name>/` holds its folder `<name>/` and `<name>.xml`. Move that `<time>-<name>/` folder into `<data>/shelf-restore/` (create it if it is not there) and open the repository again. Before its cleanup, Ramify puts the list back in the store whole, binary copies included, no longer deleted and dated now, so the weekly cleanup does not erase it again; it shows in the Shelf tab. While a list has that name, the folder stays in `shelf-restore/`. Each one is kept 30 days after it was erased. (Copying the two back into `<data>/shelf/` by hand does not work: the list is still deleted and old, and the cleanup of that open erases it again.)

### 3. Differences from IntelliJ that a user could notice, by feature

From the task reports and their reviews. Each one is accepted, with its cost.

**Patch format**
- git's `delta` binary sections are kept (ruling above): IntelliJ 2024.1 writes an empty file for such an entry.
- The `a/` and `b/` prefixes of a git header are stripped once; IntelliJ strips them twice, so for IntelliJ a new empty file `a/x` reads back as `x`.
- Context diffs are refused (`Context diffs are not supported`), and inputs that would make IntelliJ crash or misread (a binary section with no `literal`/`delta` line, a non-UTF-8 name, both names `/dev/null`, a bad escape, additional info at the very end) are errors.
- Names with `\`, `"` or control characters are written git-quoted; IntelliJ writes every name raw.
- A binary file that is only renamed goes out as a rename with no section and reads back as a text entry without hunks.
- The line diff is the Myers algorithm of the `similar` crate; all 25 writer fixtures still match byte for byte.

**Copy as Patch and Create Patch**
- Errors are notifications (`Error creating patch: …`, `Patch Creation Failed`), not modal dialogs; field problems are red text under the field, not a balloon.
- `Creating Patch…` is a task in the status bar, not a modal progress.
- The suggested extension is always `.patch` (`DEFAULT_PATCH_EXTENSION` is not copied); cancelling `Save Patch File` does not save `To clipboard`.
- The after side of a local change is what `git add` would store, so with LFS the patch shows pointers; binary means a NUL byte in the content; a symbolic link goes out as its target text; submodules are skipped.
- The file is written through a temporary file and a rename, so a symbolic or hard link at the target becomes a regular file.
- Patch creation waits for the repository lock and keeps a copy of a worktree file it overwrites (ruling above).

**Apply Patch and Apply Patch from Clipboard**
- No fuzzy applier (plan decision 4): `partialApply` and patches with trimmed context fail; the 6 context-diff cases of `ApplyPatchTest` are refused.
- A binary modification or deletion whose file differs from the patch's `index` line fails; IntelliJ writes the new bytes without checking.
- `(revision h)` loads only that commit as the base; IntelliJ also falls back to the file history and reads `(date N)`.
- Paths with `..` are refused (a strip is needed); targets that are links are refused; a read-only file fails instead of aborting the group.
- The dialog is modal, has no `Group By`, and its Show Difference shows two sides for a file that needs the merge window. `Select missing base` takes the chosen file's folder as the base and does not rename the patch's file.
- A clipboard text that IntelliJ parses although `isPatchContent` rejects it (a rename-only `diff --git` with no hunk) shows `No changes detected` here.
- `Rollback failed` keeps V:363 (`Try using 'Local History' dialog to perform revert manually.`) and adds Ramify's own second line, `The copy taken before applying is in Restore Snapshot… of Local Changes.`, since Ramify has no Local History.
- From its OK to its end (a conflict waiting in the merge window included), an apply makes the main window inert, as IntelliJ's modal merge window does; a patch dialog asked for from outside it meanwhile is still refused as if cancelled.
- The patch's author (`ChangeListData`) is not kept with the changelist: Ramify's lists have no author.
- A folder outside the repository in `Map Base Directory…` is refused with Ramify's own text, `The folder must be inside the repository`.
- An answer of the merge window in contents mode that arrives more than 500 ms after its session was cleared (a repository switch) is lost (Task 5).
- `-s,0` in a hunk inserts after line `s`, as git does, not after `s-1` (IntelliJ's quirk). Cost: none for a patch with context.

**Stash**
- The Unstash dialog is modal and stays open until the unstash ends; IntelliJ's is non-modal.
- **Show Diff of a stash file opens a dialog inside the app** titled `Stash@{0}: <file>`, with Previous File and Next File, not a diff window or an editor tab; `Show Diff in a New Tab or Window` does the same.
- Single selection in the list and the changes; `Edit Source` and `Open Repository Version` are not in the changes menu.
- Speed search matches the file names of stashes already opened only; the help icon is the `info` glyph; the branch label is a small flag and the name.
- The hiding step of `Git.Show.Stash` and `Vcs.Show.Shelf` does nothing.
- Ramify's own: before `Drop` and `Clear`, each entry's hash, message and time are written to `dropped-stashes.json` in the repository's data folder; if that record cannot be written, nothing is dropped. IntelliJ keeps nothing (git prints the hash once).
- The unstash conflict texts come from a marker file (ruling above).

**Shelf store and import**
- The store lives in Ramify's data folder with absolute paths, so it cannot be moved by copying the folder.
- **A folder name in a 3-byte script is cut at 73 characters, where IntelliJ keeps 90,** so that `<name>.xml` stays under APFS's 255 bytes (90 × `变` gives 73 × `变`). The description is kept whole.
- A recycled or deleted part copies its binaries with numbered names and keeps deleted binaries; the copy is written before the original is rewritten (a crash leaves a duplicate, not a loss).
- An invalid list is left out of the tab but its files stay on disk; characters XML 1.0 cannot hold are dropped from descriptions.
- The retention runs both of its steps daily, and "7 days" is 7×24 h.
- The import reads only the scheme files of the shelf folder (not the old lists inside `workspace.xml`), skips a list with any bad reference whole, and refuses a `CUSTOM_SHELF_PATH` that is still relative after expansion.
- **`Import from .idea` is also Ramify's own `Import Shelf from .idea…` in Git › Uncommitted Changes,** after Show Shelf, because the Shelf tab exists only while the shelf has a list: with an empty shelf it is the only entry point. IntelliJ has no such action.
- Ramify's own: what the store erases (Delete of a list in Recently Deleted, the weekly cleanup, the files taken out of a list deleted already) is moved to `<data>/shelf-erased/<time>-<name>/` and removed after 30 days; IntelliJ deletes it at once.
- A name is taken if its folder or its `<name>.xml` exists (IntelliJ checks the folder); the 90-unit cut never splits a surrogate pair, in the shelf and in a patch file name; the XML `name` must equal the file name, or the list is not read; a list whose folder is a link is not read.
- A binary copy beside one whose name already takes 255 bytes gets its stem cut so that the number fits.
- Two IntelliJ lists that differ only in their binary copies are both imported (the import key counts the copies).

**Shelving**
- A directory (submodule) among the chosen changes is neither shelved nor rolled back, and what the shelf did not store is reported; IntelliJ rolls it back.
- A file written between reading it and rolling it back is left on disk and reported in `Shelved, but not rolled back`, with the copy named.
- The copy is taken only when rolling back: `Save To Shelf` takes none.
- A conflicted file can be shelved, but its rollback fails and is notified.
- `Save To Shelf` has no entry point without Find Action.

**The Shelf tab and unshelving**
- **`Remove successfully applied files from the shelf` recycles nothing after an abort or a rollback;** 241 moves the rolled-back files into a recycled copy. Cost: none (safer).
- A binary file of a list is never written over by itself: a taken place or a changed file makes its row invalid, with the reason. `.base-ids.json`, Ramify's own record, marks a binary whose HEAD blob changed since shelving; its row offers Ramify's own `Overwrite`, which writes the shelved version after the recoverable copy (IntelliJ writes it without asking), and `Unshelve Silently` and the automatic restore leave it in the list. Lists without the record (imported, or made before `b2f2c33`) have the git-status check only. The target is checked once more right before it is written.
- The unshelve changelist starts on the list named like the shelf, else the active one; none is created unless a new name is typed. `Import to Shelf` neither creates nor edits the chosen list.
- `Import Patches…` picks files only; the diff preview is always shown; a shelved text with no usable base shows the local file against the local file with the hunks, or `Cannot show diff for '<path>'`.
- The Delete message is plain text; the `Import from .idea` result text is Ramify's own; a list asked for by name that is gone says `"<name>" is no longer in the shelf` (Ramify's own).
- `Restore` of a copy offered by another repository's notification says `The copy belongs to <repository>: open that repository to restore it` (Ramify's own).

**`Clean working tree using: Shelve`**
- A shelf conflict during the automatic restore stays in its system list and is merged from the Shelf tab (ruling above).
- **Without HEAD, only the checked paths are rolled back** (`git rm --cached -f` of the shelved paths, index only, so a file staged and then edited goes too); 241 runs `reset --hard`. Cost: an empty folder of a shelved new file can stay, and what another program staged meanwhile stays.
- **A deletion that was only on disk comes back staged,** because the unshelve applies it as `patch_apply` does.
- If a file changed after it was read, the saver rolls nothing back, sends its lists to Recently Deleted and fails the save with `"<path>" changed while it was being shelved and was left as it is. Nothing was shelved.` Between the last re-hash and `git restore` a small window remains (it needs file locks; `git stash` has the same).
- The card of an update that finished in a previous repository says where the changes are and has no button: `View saved changes…` would open the tab of the repository open now.

### 4. What was proven with tests

Each feature and the tests that name it; the full lists are in the Result column of `docs/parity-matrix.md`.

- **Patch format (Review Focus 2):** in Rust `writer_matches_intellij_fixtures` (25 cases byte for byte), `reader_parses_headers_and_subject` (the `PatchReaderTest` cases), `patch_text_detection_matches_intellij`, `base_and_charset_lines_round_trip` (a tab, `\`, a newline, an emoji, a BOM and U+2028 in a base), `property_create_then_read_is_identity` (2,000 seeded pairs), `binary_section_comes_from_git_and_reads_back`, `rename_new_deleted_and_mode_headers`, `cr_never_reaches_hunk_lines`, `ramify_errors_and_garbage_never_panic` (6 MB of seeded random bytes), `hunk_numbers_past_u32_are_refused`, `a_binary_entry_that_git_diffs_as_text_fails`.
- **Create Patch and Copy as Patch:** in Rust `a_partial_changelist_writes_only_its_hunks`, `staging_versions_read_head_the_index_and_the_disk`, `the_disk_versions_are_stored_with_one_git_add`, `patch_create_writes_utf8_with_lf`, `patch_create_writes_under_the_repository_lock`, `overwriting_a_file_of_the_worktree_keeps_a_copy_first`; in the interface `create_patch_opens_the_dialog_with_create_patch_button_then_the_settings`, `copy_as_patch_puts_the_selected_changes_on_the_clipboard_and_says_so`, `the_settings_validation_texts`, `an_existing_file_asks_before_it_is_overwritten`, `the_created_dialog_offers_reveal_and_remembers_dont_ask_again`, `a_repository_switch_during_create_patch_writes_for_the_first_one` (Review Focus 5).
- **Apply (Review Focus 1, 2 and 5):** in Rust `applies_intellij_fixtures_with_their_status` (39 of 46 `ApplyPatchTest` cases as IntelliJ; the 6 context diffs are refused and `partialApply` fails, by plan decision 4, as asserted), `a_snapshot_is_taken_before_any_write_and_rollback_restores_it`, `a_file_changed_after_reading_is_not_overwritten`, `a_file_changed_between_the_copy_and_the_read_fails`, `a_file_edited_after_the_plan_is_not_overwritten`, `patch_conflict_resolve_refuses_when_the_local_hash_changed`, `with_a_base_try_resolve_writes_the_merge`, `with_a_base_and_a_real_conflict_returns_a_conflict`, `a_link_to_git_is_never_followed`, `untrusted_paths_and_modes_fail_their_file`, `a_repository_switch_is_refused_before_any_write`, `rollback_gives_the_index_back`; in the interface `a_conflict_opens_the_merge_window_and_abort_asks_abort_and_rollback_skip_or_continue`, `failures_show_patch_applying_partly_failed_and_rollback_restores`, `a_repository_switch_during_apply_is_refused_before_writing`, `the_merge_window_gone_without_an_answer_ends_the_wait`, `abort_and_rollback_closes_the_patch_merge_as_aborted`.
- **Apply from Clipboard (Review Focus 3):** in Rust `the_clipboard_patch_is_given_only_when_it_is_one` and `a_20_mb_clipboard_that_is_not_a_patch_answers_at_once` (bound 5 s; about 0.1 s in debug on a quiet machine); in the interface `from_clipboard_without_a_patch_says_cannot_apply_patch_now` and `on_the_fly_offers_the_dialog_once_per_clipboard_text`.
- **Stash:** in Rust the 27 tests of `stash.rs`, among them `unstash_with_reinstate_index_passes_index`, `a_snapshot_precedes_unstash`, `stash_branch_takes_a_snapshot_first`, `drop_by_sha_when_positions_moved`, `stash_files_takes_names_with_pathspec_magic_literally` `the_unstash_texts_end_with_their_conflicts` and `a_dropped_or_cleared_stash_is_recorded_first_and_can_be_stored_again`, plus `a_stash_push_is_a_stash_change` and `a_worktree_save_is_not_a_stash_change`; in the interface the 29 tests of `stash.test.ts` and the 26 of `StashTab.test.ts`, among them `apply_and_pop_never_pass_index`, `show_git_stash_toggles` and `compare_with_local_puts_the_stash_against_the_local_file`.
- **Shelf store and import (Review Focus 2):** in Rust `reads_every_intellij_shelf_fixture`, `writes_xml_byte_identical_to_the_fixtures` (11 files), `round_trip_with_binary_rename_delete_no_newline_crlf_non_utf8`, `recycle_and_partial_recycle_follow_241`, `cleanup_erases_deleted_older_than_7_days_and_turns_system_recycled_into_deleted`, `import_is_read_only_and_repeatable`, `import_reads_a_custom_shelf_path`, `untrusted_names_and_paths_stay_inside_the_shelf`, `a_long_cjk_description_gives_a_usable_list`.
- **Shelving (Review Focus 5):** in Rust `shelve_changes_opens_the_dialog_titled_shelve_changes_and_shelves_the_included_part`, `shelve_silently_makes_one_shelf_per_list_and_rolls_back`, `save_to_shelf_does_not_roll_back`, `a_failed_shelve_rolls_nothing_back`, `a_file_written_between_shelving_and_rollback_survives`, `a_change_the_shelf_did_not_store_is_not_rolled_back`, `a_repository_switch_during_shelve_is_refused_before_writing`; in the interface `shelve_changes_runs_no_commit_checks`, `an_emptied_changelist_is_deleted`, `the_shelf_tab_opens_with_the_new_list_selected`.
- **The Shelf tab and unshelving (Review Focus 1):** in Rust `devin_writing_during_unshelve_is_not_overwritten`, `unshelve_uses_the_stored_base_for_conflicts`, `put_back_takes_the_copy_before_it_checks`, `a_binary_move_leaves_a_source_written_during_the_move`, `a_binary_changed_in_head_since_shelving_stays_in_the_list`, `remove_applied_recycles_or_splits_as_241`, `delete_then_undo_restores_the_list`, `a_system_list_with_a_conflict_is_merged_and_recycled`; in the interface `the_shelf_tab_lists_newest_first_with_count_and_pretty_date`, `the_toolbar_and_the_menu_follow_shelved_changes_toolbar_and_popup_menu`, `unshelve_with_one_list_opens_unshelve_changes`, `a_conflict_skipped_in_the_merge_window_stays_in_the_shelf`, `a_list_asked_for_before_it_arrives_is_selected_when_it_does`, `the_shelf_settings_follow_the_open_repository`.
- **`Clean working tree using: Shelve` (Review Focus 4):** in Rust `update_with_shelve_saves_one_shelf_per_list_and_restores_into_it`, `a_stopped_update_leaves_the_shelf_and_offers_view_saved_changes`, `a_pending_stash_from_an_earlier_build_still_restores`, `rebase_with_shelve_says_local_changes_were_shelved_before_rebase_with_view_shelf`, `smart_checkout_with_shelve`, `index_only_content_is_kept_in_a_snapshot`, `a_conflict_stays_in_the_shelf_and_nothing_is_overwritten`, `cancelling_merge_does_reset_merge_and_keeps_the_changes_saved`, `without_head_only_the_checked_paths_go_back`, `a_held_index_lock_is_waited_for`, `the_stash_policy_keeps_the_stash`; in the interface the same names for the cards and the radio, plus `view_saved_changes_then_unshelve_merges_the_conflict_and_recycles_it`, the end-to-end path from the stopped update's card to the merge window and the recycled list (the tab click and the merge window itself are simulated).
- **The counts of the guard tests** (`mutating_without_saving_the_diff_is_not_allowed`, `whatever_touches_the_repository_saves_the_diff_window_first`) follow every new mutating command: the command count went from 79 to 107 in this part.

**What the process left on record.** Several tasks wrote code before its tests and took the failing run afterwards by putting the earlier version of the module in place for one run: the Rust core and two flows of Task 10, the saver and its flows in Task 11, and parts of Task 9 (the App and menu tests have no failing run). Tests that pin behaviour that was already right passed at once and are marked so in their task reports. The Rust lib suite of Task 8 hung twice in `commit::tests` hook tests during full runs under load (every filtered run passed); it did not recur in any later full run, including this one.

### 5. Checklist to verify by hand

None of this could be checked automatically. Use a test repository first and then a real one. The `.app` was not rebuilt in this part; to build it and open it, the commands of part 1, section 5.

**Patches between Ramify and IntelliJ**
- [ ] A patch made by IntelliJ 2024.1 (`Create Patch` with a text change, a new file, a deletion, a rename and a binary file) applied in Ramify with `Apply Patch`: the tree and `git status` equal IntelliJ's own apply.
- [ ] The other way: a patch made by Ramify applied in IntelliJ. With a modified binary, expect the known difference (IntelliJ writes an empty file for a `delta` section); everything else must apply.
- [ ] A patch with the `<+>` base (Create Patch from a shelf) applied over a file that changed since: the 3-way merge or the merge window.

**The clipboard**
- [ ] `Copy as Patch to Clipboard` from Local Changes, then paste into a text editor: the same text as Create Patch writes.
- [ ] `Apply Patch from Clipboard…` with an empty clipboard (`Cannot Apply Patch Now`), with plain text (`No changes detected`), with a patch, and with a large text (20 MB): no freeze.
- [ ] `Detect patch on the fly` on: copy a patch in another app and come back to Ramify: the dialog is offered once for that text, not again for the same text.

**The native choosers**
- [ ] `Select Patch File`, `Map Base Directory…`, `Select missing base`, `To file:` (`Save Patch to`), `Base path:` and `Import Patches…`: each opens the macOS panel, Cancel leaves the field as it was, and a typed path still works.
- [ ] `Reveal in Finder` after `Successfully created patch`.

**The shelf**
- [ ] A real `.idea/shelf` from IntelliJ 2024.1 with a shelved file that holds a tab and an emoji, into a repository whose shelf is empty: Git › Uncommitted Changes › `Import Shelf from .idea…`; the Shelf tab appears; the list shows with its date, `Show Diff` shows the base against the shelved version, and `Unshelve...` over a changed file merges through the base. A second import adds nothing.
- [ ] A shelved binary that an Update changed upstream: the Unshelve dialog shows its reason and `Overwrite`; with it, the shelved version is written and Restore Snapshot… has the upstream one.
- [ ] Git › Uncommitted Changes › `Rollback...` opens Local Changes' Rollback; during an apply that waits in the merge window, the main window does not react.
- [ ] `Shelve Changes...` and `Shelve Silently` (physical ⇧⌘H) from Local Changes; the Shelf tab appears with the new list selected; `Unshelve Silently` (⌥⌘U), `Unshelve...` (⇧⌘U), F2 and ⇧F6 rename, Delete then Undo, `Already Unshelved`.
- [ ] **Update with Shelve on a real conflict:** a local remote, a local change that conflicts with the incoming commit, `Update Project` with Merge. The update stops; the changes stay in the shelf; `View saved changes…` opens the Shelf tab with that list selected; `Unshelve...` resolves the conflict in the merge window and the list is recycled. Repeat with a stash saved by an earlier build (`Restore Changes` must still show).

**The merge window in contents mode**
- [ ] An Apply Patch conflict: the window is titled `Patch Conflict for <name> (<folder>)` with `Local version`, `Merge result` and `Patched version`; Accept Left, Accept Right and Apply; `Abort…` asks `Abort Patch` with `Abort and Rollback`, `Skip` and `Continue Resolve`, and each does what it says; closing the window asks the same.

**Stash**
- [ ] **Recover what was dropped:** drop a stash, then bring it back with the line of `dropped-stashes.json` (end of section 2); delete a list twice from the Shelf tab and find it in `shelf-erased/`.
- [ ] **Unstash with `Reinstate index`:** stage part of a change, stash it, then `Unstash Changes…` with `Reinstate index` on: the staged part comes back staged. With it off, nothing comes back staged.
- [ ] The Stash tab in a real window: Apply, Pop, ⌘⏎, Drop with Delete, Backspace and ⌘Backspace, the splitters, Compare with Local, and `Unstash Changes…` toggling the tab.

### 6. What could not be checked

- **Nothing was seen in a real window.** The two tabs, the Apply dialog, Patch File Settings, the merge window in contents mode and the choosers were tested in jsdom with the Tauri API simulated.
- **No interoperability with the real IntelliJ.** Patches and shelves were checked against the fixtures at the tag, not against files made by the installed IntelliJ, and no Ramify patch was opened in IntelliJ. The `<+>` base escapes (plan decision 11) have no fixture at the tag.
- **No real clipboard, no native panel.** The tests replace `pbpaste` with a fake program and the clipboard writer with a closure; the choosers were mocked. The `dialog:allow-open` and `dialog:allow-save` capability is validated by `tauri-build` only.
- **No physical keyboard:** ⇧⌘H, ⌥⌘U, ⇧⌘U, ⌘⏎, F2, ⇧F6 and Delete were simulated key events.
- **The real IPC of the new commands** has not been exercised through Tauri; a failure would be one of registration or argument names (look for `command … not found` in the console).
- **The native menu changes with the staging area** (`sync_patch_menu`, `sync_local_changes_menu`) have no automated test: they need an `AppHandle`. The pure menu builders are tested.
- **No measurement.** The cost of reading every shelf list (now only after a command wrote the store), of shelving many files with one `git add`, of `plan` on a large patch, and memory and CPU were not measured. The 20 MB clipboard test only bounds the time.
- **App activation** for the on-the-fly offer is decided with a 150 ms settle between Ramify's windows, a guess at how long macOS takes to move the focus; a slower handover would only read the clipboard again.

### 7. Verification

Run on 2026-11-08 at `21156cf`, one suite at a time (`pgrep -fl "vitest|cargo (test|clippy)|rustc"` empty before each, load average under 7), with the toolchain of `~/.cache/ramify-toolchain`.

| Command | Result |
|---|---|
| `pnpm exec tsc --noEmit -p .` | no errors (exit 0) |
| `pnpm exec vitest run --maxWorkers=2` | 92 files, 2,070 tests passed |
| `cargo test --lib -- --test-threads=4` | 1,824 passed, 0 failed, 5 ignored (on purpose), 161 s |
| `cargo clippy --all-targets -- -D warnings` | no warnings (finished, no output) |
| `python3 -I -m unittest discover -s scripts` | 65 tests, OK |

Against the close of part 2 (83 files and 1,867 tests in the interface, 1,637 in Rust, 65 in the scripts), part 3 adds 9 test files and 203 tests in the interface and 187 in Rust, and none in the scripts. No timing test failed in this run. `cargo fmt --check` was not part of the verification, as in part 2.

After the final fix wave, run on 2026-11-08 at `c25f56b` (the code of this wave; this commit only adds the documents), one suite at a time, `pgrep -fl "vitest|cargo (test|clippy)|rustc"` empty before each and the load average under 10:

| Command | Result |
|---|---|
| `pnpm exec tsc --noEmit -p .` | no errors (exit 0) |
| `pnpm exec vitest run --maxWorkers=2` | 92 files, 2,078 tests passed |
| `cargo test --lib -- --test-threads=4` (`CARGO_BUILD_JOBS=4`) | 1,845 passed, 0 failed, 5 ignored (on purpose), 166 s |
| `cargo clippy --all-targets -- -D warnings` | no warnings |
| `python3 -I -m unittest discover -s scripts` | 65 tests, OK |

The wave adds 8 tests in the interface and 21 in Rust. `cargo fmt --check` reports 27 places in 14 files, all on lines older than this part (finding 10).

### Conclusion

What can be tested without a screen, a keyboard, a real clipboard or the real IntelliJ is tested, with real git and IntelliJ's own fixtures where it matters, and follows the 241 sources except for the differences in sections 2 and 3. Part 3 is not validated. It ends when you complete the list in section 5, with the exchange of patches and shelves with IntelliJ and Update with Shelve on a real conflict as its main risks.

## Part 4: history and comparisons

Date: 2026-11-09. Plan: `docs/superpowers/plans/2026-11-08-phase-4-4-history-compare.md`. The part has 35 commits after its plan (`d591fed`): `d591fed..ba8c6e1`, 15 features, 19 fixes and one refactor (`fea6033`), in 12 tasks, each reviewed, plus this report. No final review of the whole part has run yet; the findings parked for it are at the end of section 2.

### Summary

- Part 4 is done in essence: the user looks back at their files as in IntelliJ 2024.1. The `Project` tool window (⌘1) and a read-only viewer, also for a file opened from the terminal; file history as a Log tab; `Show Repository at Revision`; `Copy Revision Number`; `Compare with Local`, the comparisons of the Git submenu, the log, the branches popup and the log's changes panel, and Compare Branches; `Get`; `Not Cherry-Picked Commits`, `Select in Tree…` and the merge window's `Show Details`; `Annotate` and `Show History for Selection…`.
- **`done` means tests, not use**, as in parts 1 to 3. The Rust logic is tested with real git and throwaway repositories, the selection history also with every case of IntelliJ's `SelectedBlockHistoryTest`; the interface in jsdom with the Tauri API simulated. Nobody has seen the new tool windows, the viewer, the history and compare tabs, the annotation gutter or the selection history window in a real window, nor opened a file from the terminal, nor tried any of it on a real repository. The part cannot be accepted without the list in section 5.
- Matrix: 35 action rows of section 4 got their result (22 `done`, 2 `partial`, 11 `pending` with their reason: 5 not in the user's mode, 4 with no entry point without Find Action, 2 with no effect for Git); 3 rows of section 2 were filled, one was added (`Show Details` of the merge window) and two got a part 4 note; 3 `done` rows of section 4 got a part 4 note; `Vcs.Log.ShowOtherBranches` stays in the "Removed in 241" list, which now names the `Branch` filter that replaces it. Six earlier citations named tests that part 4 renamed; they now name the new tests. In the final fix wave, of the 16 phase 2 and 3 citations thought stale, 14 name tests of `src-tauri/tests`, which exist (the earlier check searched only `src-tauri/src`); the other two now name `branch_submenu_in_241_order_and_compare_off_for_current`. The two `CommitView.ShowOnDoubleClick.*` rows got their result.
- 18 rulings were taken during the part, and four more for its final fix wave; section 2 lists them with their cost, and then the findings parked for the final review. The differences from IntelliJ that a user can notice are in section 3, by feature.
- The final review (0 critical, 4 important, 6 minor) was followed by one fix wave, described after section 7: signatures out of every `log` and `show` that runs without a task, targeted listens for the viewer and the diff window, `Get` never overwriting a write that came after its copy, test repositories isolated from the machine's git configuration, and `Show on Double-Click`. Five of the review's minors stay parked (section 2).

### 1. What was built

- **Groundwork (Task 1).** A tool-window registry (`src/shell/toolWindows.ts`): `Project` (⌘1), `Commit`, `Changes` and `Repositories` (registered on first use, as 241's `ChangesBrowserToolWindow` and `RepositoryBrowser`) and `Git`; the Log's tab kinds (`log`, `update`, `history`, `compare`) with their find-or-create and persistence rules; the carried fixes N1 (a re-planned binary stays ticked), N2 (a failed Shelf read is retried) and N3 (`<data>/shelf-restore/`), and the six Svelte compile warnings of part 3.
- **Revision content and the viewer (Task 2).** `viewer.rs` reads a file on disk or at a revision through `git::exact`, with IntelliJ's 20 MiB content limit, a 2.56 MB preview and its binary rules (`DiffContentFactoryImpl.isBinaryContent`, `CharsetToolkit`). The viewer is one separate window with a tab per file, read-only, with soft wrap in its own place. `Ramify <file>` and `Ramify <file>:<line>` open the file's repository and the viewer at that line. `Edit Source` (⌘↓, F4) and `Open Repository Version` in the log's changes panel.
- **The Project tool window (Task 3).** `tree.rs` lists folders from disk on expand, folders first and in `NaturalComparator` order, hiding IntelliJ's default ignored names; the tree colours entries from `git status --ignored=matching` with the 241 dark New UI colours, has speed search, Expand All and Collapse All, and relists the folders a change touches, at most 16 at once.
- **Two-revision diff requests and file comparisons (Task 4).** `compare.rs` and a `revisions` request of the diff window, where the Local side keeps phase 1's save rules on either side, in both views. `Compare with the Same Repository Version`, `Compare with Revision...` (`File Revisions`), `Compare with Branch…` and the tag popup, with 241's titles.
- **The Changes tool window and `Get` (Task 5).** One tab at a time (plan decision 5): a change tree with `Refresh`, `Show Diff`, `Get`, `Group By` and `Create Patch...`. `get_version.rs` takes a recoverable copy first, refuses when HEAD moved, and leaves and reports a file that changed after its check, for every touched path.
- **The Git submenu, `Git > Current File`, Show Current Revision and Copy Revision Number (Task 6).** `gitFileMenu.ts` in the order of `Git.ContextMenu`, in the Project tree and the viewer; `Git > Current File` follows the focused window; `Show Git Log` in the Git menu; Show Current Revision as a plain-text popup; Copy Revision Number oldest first.
- **File history (Task 7).** `history.rs` ports `GitFileHistory`'s walk without an index (full history, every addition a candidate, a rename restarting from the parent with the old path, at most 1,000 per walk); `history_view.rs` puts the rows on the log graph. The `History: <name>` tab has the Branch filter, the toolbar and menu of 241, a diff preview, a merge row in three sides, and is remembered per repository (plan decision 4). A folder opens a Log tab with its path filter. `File Revisions` uses the same walk without full history.
- **The deferred log, branch and changes-panel items, and Compare Branches (Task 8).** `Compare Versions`, `Compare with Local` and `Create Patch...` in the log menu; `Compare with '<current>'` and `Show Diff with Working Tree` in the branch menus; `Compare with Current`, `Compare Branches`, `Show Files Diff`, `Update Selected` and `Delete branches` for several branches in the Branches panel; the log's changes panel menu in 241's order with `Compare (Before) with Local`, `Get from Revision` and `History Up to Here`; the `Compare: <a> and <b>` tab with two full logs; `Swap branches` and `Get from Branch` in the Changes window.
- **Show Repository at Revision (Task 9).** `ls_tree` one level at a time through `git::exact`, the `Repositories` tool window with one tab per revision, and a tree shared with the Project tree (`src/tree/navigation.svelte.ts`, `TreeView.svelte`).
- **Not Cherry-Picked Commits, `Select in Tree…` and `Show Details` (Task 10).** `log/cherry.rs` (`git cherry` on full hashes) and the View Options toggle with `Select Source Branch`; the `Select Paths to Filter by` chooser in the Paths popup; the merge window's `Show Details` with one commit or a range and `Filter by conflicted file`.
- **Annotate (Task 11).** `annotate.rs` (blame, the mapping of local edits, the history numbers, IntelliJ's two palettes) and the viewer's gutter with its popup, its `View` options, `Annotate Revision` and `Annotate Previous Revision`, the line-count check and the refresh rules of 241.
- **Show History for Selection (Task 12).** `block_history.rs` ports `Block` and streams the block of every revision; the `History for Selection` window with `Changes only`, the block diff and its popup.

### 2. Decisions and rulings that depart from IntelliJ

Each one with what it costs if it turns out to be wrong. In this part, "plan decision N" is one of the eight below, from `docs/superpowers/plans/2026-11-08-phase-4-4-history-compare.md`, and "overview decision N" one of `docs/superpowers/plans/2026-11-03-phase-4-overview.md`.

**Decisions of the plan (1 to 8)**

1. **Opening a file from outside starts its own Ramify process**, as opening a repository does: `Ramify <file>` and `Ramify <file>:<line>` find the repository from the file's folder, open it and show the file in the viewer at that line. Not copied: reaching a Ramify that is already running (it needs a new dependency) and Finder's "Open With". Cost: a file of a repository that is already open gives a second window on it.
2. **The viewer is one separate window with a tab per file**, since Ramify has no editor area; its tabs wrap in rows and it is read-only. Cost: files do not open beside the tree.
3. **The Project tree reads folders from disk on expand** and colours entries from `git status --ignored`, showing ignored files in their colour and hiding IntelliJ's default ignore patterns. Cost: a folder with tens of thousands of entries is listed whole when opened.
4. **History tabs are a tab kind of the Log, remembered per repository**, and Compare Branches is a Log tab with two stacked logs whose filters live in memory only. Cost: none.
5. **`Changes` and `Repositories` are tool windows on the left stripe, registered on first use**; `Changes` holds one tab and a new one replaces it; a new revision replaces the `Repositories` tab. Cost: none.
6. **`Get` writes only the files**, without `git add` or `git rm`, at the path the file had in the revision. Cost: one `Add` (⌥⌘A) after getting a deleted file back.
7. **Annotate and selection history run in the viewer**, with IntelliJ's default options (`Last Name`, colours by `Order`, `Date` and `Author` shown). Cost: no annotation inside the diff window.
8. **Not copied:** `Show Diff On Hover`, `Show Diff in a New Tab or Window`, `Choose Parent to Compare` in file history, F4 inside the file history table, and the `Git.Stage.Compare.*` and `Show Staged Version` rows, which exist only in staging mode ("not in the user's mode"). The rows that only Find Action reaches stay without an entry point (overview decision 3). Cost: none for this user.

**Rulings taken during the part**

The order of the work and the interfaces between tasks:
- The Project tree's context menu is Task 6's `gitFileMenu`; Task 3 shipped an empty `Git ▸` placeholder (the plan said "Task 4" by mistake). Cost: none.
- Task 4 opened the diff window for one file and called a `showChanges` stub for folders and several files until Task 5 built the Changes window. Cost: one task of delay for the multi-file path.
- Task 7 added `History Up to Here` and its test; Task 8 placed it in 241's order of the changes panel's menu and toolbar. Cost: none.
- Tasks 2 and 4 ran in parallel after Task 1, sharing `commands.rs`, `lib.rs` and `api.ts` through the rule for foreign hunks. Cost: one commit staged by the controller.
- Task 9 ran before Task 8, adding `Show Repository at Revision` at its 241 position after `Checkout`; Task 8 kept it when it reordered the menu. Cost: none.
- Task 11 ran before Task 10, in parallel with Task 8, and Task 12 followed Task 11 (both touch `gitFileMenu.ts` and the viewer). Cost: none.
- `925bf93` (Task 6) picked up Task 7's `pub mod history;` without `history.rs`; it was not amended, because history is not rewritten, and Task 7's next commit made the tree build again. Cost: `925bf93..ed38fdb` do not build on their own (three non-bisectable commits).
- **From the 08:24 freeze of the user's MacBook on: one agent at a time, no throwaway exports, `CARGO_BUILD_JOBS=3`, `--test-threads=3`, `vitest --maxWorkers=2`, everything under `nice -n 15`, one heavy command at a time.** Cost: the part took longer by the clock.

Behaviour:
- **N3's entry point is a request folder, `<data>/shelf-restore/`**, read on open before the cleanup, because the cleanup of that open would erase a list copied back by hand before Restore could reach it. Cost: a recovery folder known only from this report (part 3, section 2); a menu item can replace it later.
- **Only Project, Changes and Repositories hide on a second press of their key or stripe button**; Commit and Git keep phase 1's "nothing happens". Cost: ⌘0 and ⌘9 do not toggle as in IntelliJ (unchanged).
- **The viewer's content limit is IntelliJ's effective `LARGE_FOR_CONTENT_LOADING`, 20 MiB (20,971,520 bytes)**, not the brief's rounded 20,000 KB. Cost: one constant.
- **The file history's columns are 241's default `Author, Date, Commit`**, not the main log's order; the user's own per-project order is not known. Cost: a column offset in `draw.ts`.
- **The Branches panel's toolbar follows 241:** its slot holds `Compare with Current` (⌘D), and `Compare Branches` is a context menu item shown with exactly two branches chosen; the brief's toolbar button was a research error. Cost: a small move in `BranchesPanel` and its test.
- **The compare tab's halves got their changes, details and diff preview panes, and Show Diff with Working Tree its `Swap branches` label and `Get from Branch`**, which 241 has and which matter to whoever compares branches. Cost: a larger fix round. Parked from the same review: `Copy Commit Subject` stays in the log menu, where Ramify has had it since phase 2 (cost: one menu item more than 241), and a Show Diff with Working Tree error is a notification, not a dialog (low).
- **A commit that moves the annotated revision closes the annotation**, as 241's `isBaseRevisionChanged` does; an edit on disk with the same base maps the lines again. The plan's Review Focus 5 said "recompute"; 241 is the rule. Cost: after a commit from the terminal the user annotates again.
- **A failed refresh of an annotation closes it in every case**, as 241's edit-on-disk path does; 241's repository-change path keeps it. Ramify has one refresh, and its failures are not transient. Cost: after a git error or an unborn HEAD the user annotates again.
- **`Show in Git Log` is in the selection history window's popup**, at 241's position (git4idea `plugin.xml`); the brief missed it. Cost: one item and its test.
- **The last fix round of Task 10 fixed the races A to D of Not Cherry-Picked and its chooser, and parked the rest:** M5, a folder that holds only untracked files still shows, empty, in `Select in Tree…` (the reviewer found that one `git status --untracked-files=normal` would hide it); M8, the old `binding_property_non_reactive` warning of `Menu.svelte`; M9, a red run shown second-hand; M10, the not-picked logic still inside `LogPanel`. Cost: one more short round.

After this report was first written (four more rulings, for the final fix wave):
- **`Show on Double-Click` is built in the final fix wave:** part 1's decision 3 promised it for part 4 and no task built it; the viewer exists and parity is binding. Cost: one small item and its tests.
- **The phase 2 and 3 matrix citations thought stale go to the final fix wave.** Cost: a mechanical edit (two of them were really stale; section "Final fix wave").
- **The doc gaps of the Task 13 review fold into the final fix wave** instead of a round of their own. Cost: none.
- **One fix wave covers the four importants of the final review** (with the same signature rule in the older `commands.rs`, `apply_commits.rs` and `rewrite.rs`), `Show on Double-Click`, the citations and the doc gaps; the review's minors 1, 2, 3, 5 and 6 stay parked (listed below). Why: the importants touch the user's rules (no lost work, targeted listens, signatures out of the output). Cost: one larger wave.

Accepted deviations recorded in the reviews:
- The Changes window's `Show Diff` opens only the chosen files; with one row chosen 241 opens all, positioned at it. Cost: next and previous file stay among the chosen ones.
- Neither the Changes tab nor the Repositories tab has a close button; 241's closes and makes the window unavailable when it is the last. Cost: the stripe button stays until another repository is opened.
- `Get` of a folder or submodule entry of the revision is refused with `Cannot load revision: "<path>" is a folder|submodule in <short hash>`; 241 would try to write it as a file. Cost: none for files.

**Findings parked for the final review**

None of these blocks the part; each is small or needs a decision that spans more than one task.
- **Get (Task 5), marked to come first, because it is about never losing work:** its M1, M2 and M6 were fixed in the final fix wave (after section 7), and the final re-review's two Get observations (a link whose place was emptied, the stamp read back after a replace) in its second pass. Still parked: `storage::write_atomic_if` could use macOS's `renamex_np(temp, path, RENAME_EXCL)` when the target must be absent, which would close the create case completely (`rename(2)` cannot); optional hardening. Also: the expected content is read back after the rename; the one-question rule drops a second Get's error dialog; the warning says "after the snapshot" although it also counts changes between the check and the copy.
- **Groundwork (Task 1):** part 3's report says a list restored from `shelf-erased/` "shows in the Shelf tab"; a list that was recycled when it was erased comes back still recycled, so it shows only with Show Recycled on. No test that hiding Project falls back to Git when the previous window no longer exists, nor of `registerToolWindow` followed at once by `activateWindow`. The title `Log` is defined twice (`tabKinds.ts` and `LogPanel.svelte`).
- **Viewer (Task 2):** no command-level test for `viewer_openable`; a shadowed `request` constant in the context menu of `ChangesPanel.svelte`; the scroll test runs in jsdom, which lays nothing out; two quick repository notices may start two loads (the older is dropped); the file context menu waits for an asynchronous check.
- **Project tree (Task 3):** the Commit view keeps the older 232 status colours, the tree uses 241's (a global switch is a separate change); the listing limiter can reach 17 for a moment; a folder whose first read failed is read again quietly on every change of the rows; Expand All keeps a failed read as empty; links that are not followed have no icon of their own; the tree's selection provider is module state, so a second Project tree would take it over silently; some of its tests came after the code (its report says which).
- **Comparisons (Task 4):** a branch that vanished after the list was read says `D:102`; no `Loading Tags…`; the combined view is not restored after a revisions request; validation errors are reported as `GitError::Io`; a revision blob over the limit is read whole before it is cut; a deliberate reopen of the comparison already shown does nothing; in the unified view with a left Local side the number column shows the right side's lines; `diff::read_side`, used by older paths, still reads `<rev>:<path>` without `git::exact`.
- **Git file menu (Task 6):** a staging Rollback of a folder with only untracked changes opens with nothing checked; the `Current File` items stay enabled, doing nothing, while the diff or merge window has the focus; the viewer's context is read three times per event instead of cached; ignored files are not offered to `Add`; the root row's Compare with Revision and Compare with Branch are off where 241 enables them; the Changes window has no Copy Revision Number; `task-changed` reaches main and merge twice; a comment at `ChangesPanel.svelte:91` sits over constants that moved to `addFiles.ts`; `ChangesPanel.svelte` keeps its own `"Snapshot restored."` text next to the shared one.
- **File history (Task 7):** a merge row's three sides use the staging colours; history tabs saved by `da6abaa` keep the old column order (development data only); the file revisions popup does not apply `getLastCommitPath` (a file renamed in the index gives an empty popup); a merge row's selection walks every reachable commit to order its parents (slow on a huge repository).
- **Branches (Task 8):** a cancel while waiting for the write lock after the remote deletion's push ends the task as cancelled, silently, holding the remote's lock meanwhile; the selected branches count only the visible rows.
- **Repositories (Task 9):** every file has the generic icon; after a collapse the lead and anchor stay put where JTree makes the folder both; a comment overstates Swing's order after a ⌘-deselect; a folder that failed is asked of git again on every change of the rows.
- **Not Cherry-Picked (Task 10):** M5, M8, M9 and M10 above; E, a refresh that gives up because the filters changed to the same branch is not retried, so the highlight stays computed against the old references until the next refresh; F, the toggle's no-branch path does not ask again after a filter change.
- **Annotate (Task 11):** the test repositories of `TempRepo` were not isolated from the user's global and system git config; fixed in the final fix wave for the library tests (after section 7). The integration tests of `src-tauri/tests` are not covered (section 6).
- **Selection history (Task 12):** the diff page serialises both texts on every run of one effect (wasteful, rare); that effect no longer re-reads the wrap after a skipped rebuild (fine while `toggleWrap` drives `setWrap`); revisions are decoded as lossy UTF-8, like every other revision reader of Ramify.
- **Still parked after the final review** (its minors 1, 2, 3, 5 and 6; ruled to stay parked because they touch none of the user's rules, unlike its four importants): 1, `File Revisions` does not apply `getLastCommitPath`, so a file renamed in the index gives an empty popup (one line, `current_revision` already does it); 2, a merge row's selection runs `filter::reachable` over the whole store per parent (`in_parent_order`), fine at today's sizes; 3, a cancel while waiting for the write lock after a remote branch deletion's push ends as cancelled with no notification, although the remote branch is gone; 5, the timing flake of section 6, unexplained, which should get a ticket so it does not become "known flaky" by habit; 6, `parse_range_commits` splits records on `\x01` and `\x02`, so a subject with `\x01` would split one (practically unreachable).
- **Process:** `App.svelte` grew instead of shrinking (1,152 lines before part 4, 1,364 now); the `restore_requested` result of N3 is dropped, so a request that cannot be restored stays in `shelf-restore/` silently; N3's rollback paths are untested; the timing test of section 6.

### 3. Differences from IntelliJ that a user could notice, by feature

From the task reports and their reviews. Each one is accepted, with its reason; the parked ones of section 2 are marked "parked".

**Project tree and viewer**
- There is no editor: files open read-only in the viewer, and editing stays in the diff window (plan decision 2).
- The tree's menu has only `Edit Source` and the `Git` submenu; ⇧F4, the root row's `~` and the rest of `ProjectViewPopupMenu` are not copied. Why: the rest needs an editor or a project model Ramify does not have.
- The tree is built the first time its window comes to the front in a repository, so a user who never opens it pays no `git status --ignored`. Why: cost; nothing to see.
- Expand All reads every folder, ignored ones included (`node_modules`, `target`), as `promiseExpandAll` does; Collapse All stops it.
- The Commit view's status colours are 232's and the tree's are 241's (parked: a global switch is a separate change).
- A file a link points to outside the repository is listed as a file and never opened; it has no icon of its own (parked).
- The viewer's preview notice has no `Hide notification` or `Don't show again`; a binary below the limit says `<name> is a binary file (<size>).`, Ramify's own text, where IntelliJ opens an image or archive editor; sizes always use a dot as the decimal separator; `Edit Source` stays enabled on a deleted change and opens nothing.
- A double click or Enter in Local Changes opens the source of the selection in the viewer, 241's default (`SHOW_DIFF_ON_DOUBLE_CLICK=false`); `Show on Double-Click` ▸ `Diff` / `Source` is in the gear (`Options`) of the Git window, and `Diff` brings back the diff (built in the final fix wave). Difference: 241 installs `LocalChangesView.GearActions` as the gear of the whole window (`vcsToolWindowFactories.kt:35`, `setAdditionalGearActions`), so the item is there whatever tab is selected; Ramify shows the gear only while Local Changes is selected. Cost: from the Log or the Console tab, the user switches to Local Changes first to change it. The gear has only that group: 241's own tool window items (view mode, move, resize) are not copied, and the Commit view of the non-modal mode has no gear (`CommitView.GearActions`). A file that is not on disk opens nothing.
- Opening a file of a repository that is already open from the terminal gives a second window (plan decision 1).

**Comparisons and the Changes window**
- In the unified view with a left Local side the number column shows the right side's lines (parked).
- A side is "large" above Ramify's 5 MiB diff limit, not 241's 20,000 KB, and two previews show `DB:42` instead of a diff of the two cut texts. Why: Ramify's diff limit, consistent with phases 1 to 3.
- The combined view is off for a revisions request and is not restored after one (parked).
- `Compare with Revision` and `Compare with Branch` are off on the repository's root row, where 241 enables them. Why: the commands take a path inside the repository (parked).
- The Changes window's `Show Diff` opens only the chosen files; the tab has no close button (accepted deviations of section 2).
- `File Revisions` dates are the commit time and it has no branch column.
- A `Show Diff with Working Tree` error is a notification and no tab opens (parked at the Task 8 review); `Get from Branch` writes files only and does not stage (plan decision 6); the `Swap branches` choice lives in the page's local storage, not in the application settings.
- The compare tab's halves have fixed splits and no View Options.
- `Delete branches` with several local branches chosen deletes them one at a time, as separate tasks, each with its own notification (`Restore`, `View Commits`); 241 runs one `Deleting a, b…` task (`GitBrancherImpl.deleteBranches`) whose `GitBranchWorker` also deletes them one by one with a notification each. Neither asks before deleting an unmerged branch: its commits are offered from `View Commits`. Cost: one task per branch in the status bar instead of one. (`Update Selected` fetches each chosen branch into itself one after the other, as 241's `GitBranchActionsUtil.updateBranches` does.)
- A cancel while waiting for the write lock after a remote branch deletion's push ends with no notification, although the remote branch is gone (parked, final review minor 3).
- `Copy Commit Subject` stays in the log menu (ruling of Task 8).

**Get**
- No `git add` or `git rm` (plan decision 6); a folder or submodule entry is refused.
- A link that cannot be made after its place was emptied is reported written, with Restore; after a replace the path is expected to hold what was just written, so a write in the next instant is left by a later item of the same Get (second pass of the final fix wave).
- A file changed between the `Get Revision` question and the write is left and reported, including a write while its content is compared for the last time (the `lstat` before and after that read must agree); only a write between that last `lstat` and the rename stays out of reach, which POSIX cannot close.
- An item written at the old path of a rename whose local file at the new path changed (or could not be removed) is reported as written at the old path, with Restore, and its new path as left.
- After a Get, Ramify's own notices: "Get of N files. A snapshot was saved." with Restore, and a warning naming the files left because they changed.

**Changes saved before a rebase (found by the integration tests)**
- With `Clean working tree using: Shelve` (the default since part 3), a saved change whose file the rebase removed does not come back: the unshelve fails with 241's `Cannot find file to patch: <path>` (`PathsVerifier`), what applies comes back, and the failed change stays in its shelf list, as 241's `ShelveChangesManager.unshelveChangeList` keeps its failed patches. Nothing is lost. Ramify says it with `Local changes were not restored`, the reason, `They are in the shelf: <message>` and `View saved changes…` (part 2's decision 10); 241 shows the reason in an `Apply Patch` error dialog and then asks, in `UndoApplyPatchDialog`, whether to roll back the changes that did apply (`Failed to apply patch to file: <path>` / `Other changes from the patch have been applied successfully, would you like to rollback them?`). Ruled: Ramify keeps part 2's decision 10 (the sticky card, nothing modal). 241's rollback relies on Local History, which Ramify does not have, and the failed change is never lost because it stays in its shelf. Cost: no modal question, and no rollback of what applied; the user undoes it by hand if they want the applied part gone.

**Git file menu, Current File and Show Current Revision**
- `Current File` follows the Project tree only while the Project window is the active tool window; any other focus in the main window gives `Current File` with everything off. Why: Ramify's main window has no editor.
- `Add` is not offered for ignored files (no confirmation dialog in Ramify).
- Show Current Revision is plain text, without issue links.

**File history**
- A merge row opens three read-only sides with the staging colours, not the merge-result ones, and without arrows (parked).
- A revision asked for that is not in the history says `L:165`/`L:166` with both links; 241 jumps to the nearest commit silently. Why: the brief asked for those texts; Ramify finds the commit, not its nearest visible ancestor.
- `Show All Affected Files` on a root commit compares with the empty tree (to check by hand).
- The file revisions popup of a file renamed in the index is empty (parked).

**Show Repository at Revision**
- No close button on the tab; the generic file icon for every file (parked); no `loading…` node while a folder is read.
- The Compare with Local title copies 241 literally: `a.txt (src -> /repo/src)`.

**Not Cherry-Picked, Select in Tree and Show Details**
- Commits match by patch only: Ramify has no log index, and with its index 241 also matches author, time and message.
- The toggle is only in the Log's own tabs, as in 241.
- After a refresh that raced a change of the filters to the same branch, the highlight can stay one refresh old (parked E).
- In `Select in Tree…` a folder that holds only untracked files shows, empty (parked M5).

**Annotate**
- Only in the viewer (plan decision 7); no `Show Diff On Hover` (plan decision 8); extension actions of the popup (GitHub's) are not copied.
- A commit that moves the annotated revision closes the annotation, and so does a failed refresh (rulings of section 2).
- A file over the content limit opens as a preview and cannot be annotated, as in 241 (`Cannot create text editor for {0}`).

**Selection history**
- The popup's first entry is `Compare`, as 241's `MyDiffAction` sets it, not `Show Diff`.
- No `Choose Parent to Compare` on a merge row; it is compared with the next older row.
- One reused window with fixed splits whose size is not remembered; 241 opens a frame each time.
- The stream is a Ramify task with Cancel in the main window's status bar; 241's loader shows no progress.
- A right click outside the selection still uses the selection, not the caret (241's `contextMenuInvokedOutsideOfSelection` is not copied); the commit message has no issue links.

### 4. What was proven with tests

Each feature and the tests that name it; the full lists are in the Result column of `docs/parity-matrix.md`.

- **Writes while Get or a comparison saves (Review Focus 1):** in Rust `get_takes_the_copy_first_and_leaves_a_file_devin_changed_meanwhile`, `a_write_between_the_check_and_the_get_is_reported_not_overwritten`, `a_path_the_check_did_not_read_is_left`, `a_write_right_before_the_replace_is_left_and_reported`, `a_write_right_before_the_move_back_removes_is_left_and_reported`, `a_failed_write_keeps_the_copy_and_says_what_was_written`, and from the final fix wave `a_write_while_the_file_is_compared_is_left_and_reported`, `a_write_while_the_moved_file_is_compared_is_not_removed`, `a_written_target_with_a_left_move_back_is_reported_written`, `a_failed_move_back_says_the_target_was_written`, `a_file_removed_after_its_lstat_has_no_stamp` and `a_file_removed_after_the_check_is_left_and_the_others_are_written`, `get_runs_no_git_add_or_rm`, `the_local_side_saves_with_the_hash_check_and_refuses_a_changed_disk`, `a_revision_is_compared_with_the_local_file_and_the_local_side_is_edited_on_either_side`; in the interface `get_over_a_modified_file_asks_overwrite_modified_file_and_cancel_stops`, `an item written at the old path and left at the new one offers Restore`, `a local side on the left is edited and saved with the hash check` and the tests of `a local side on the left in the unified view`.
- **A long history with renames and merges (Review Focus 2):** in Rust `history_follows_a_rename_from_the_parent_with_the_old_path`, `a_rename_on_a_side_branch_and_a_delete_and_readd_give_241_rows` (rows written by hand from 241's algorithm), `more_than_1000_commits_stop_at_1000_per_walk_without_a_notification` (1,100 commits, a rename and 100 more), `merge_parents_without_a_record_ask_git_diff_for_the_rename`, `the_file_revisions_walk_stops_at_the_first_addition_of_each_walk`, `a_merge_row_opens_three_sides`, `closing_the_tab_cancels_the_walk`, `signatures_do_not_reach_the_history`; in the interface `history_tabs_are_reused_and_reopened_after_the_log_reloads`, `closing_a_history_tab_closes_its_view_and_the_walk_with_it` and `a_selection_that_arrives_after_the_table_is_gone_does_nothing`.
- **Paths git could read another way (Review Focus 3):** `a_name_with_glob_characters_and_a_leading_dash_lists_and_opens` (tree), `a_revision_is_read_at_its_own_path_through_exact` (viewer), `compare_reaches_a_glob_named_and_nfd_file`, `get_reaches_a_glob_named_a_dash_named_and_an_nfd_file`, `a_path_with_glob_characters_has_its_own_history`, `a_ref_named_like_an_option_is_a_starting_revision_not_an_option`, `ls_tree_reaches_exactly_the_folder_it_names`, `blame_reaches_a_glob_named_and_nfd_file` and `a_renamed_file_is_read_at_each_revisions_path`, all with NFC and NFD twins under `core.precomposeunicode=true`; several were checked to fail without `git::exact`.
- **Large and binary files (Review Focus 4):** `thirty_megabytes_give_a_preview_with_the_241_text` (Rust and the viewer), `the_limit_is_twenty_mebibytes`, `a_binary_over_the_limit_says_it_is_too_large`, `a_png_is_binary`, `a_large_or_binary_revision_shows_the_241_texts` (Rust and the diff window), `a_large_binary_is_compared_by_its_object_without_reading_the_disk_whole`, `a_binary_file_cannot_be_annotated`, `a_binary_or_a_preview_is_not_annotated`. Task 11 also timed the annotation of a large file in a debug build with the ignored test `large_file_annotation_time`: 19 MB (311,296 lines), blame 0.80 s and local annotation 1.05 s, 306 MB at most; 30 MB (491,520 lines), 1.23 s and 1.65 s, 468 MB at most (its report; the peaks include the test's own copies of the text).
- **The repository changes while something is open (Review Focus 5):** `get_refuses_before_writing_when_head_moved`, `a_commit_from_the_terminal_refreshes_an_open_history_tab`, `compare_tab_refreshes_with_the_log_after_a_checkout`, `a_commit_that_moves_the_base_closes_the_annotation` (Rust and the viewer), `an_edit_on_disk_with_the_same_base_maps_the_lines_again`, `a_change_on_disk_recolours_and_relists_open_folders`, `a_folder_read_then_collapsed_shows_what_changed_on_disk_when_expanded_again`, `local_tabs_read_the_disk_again_only_when_the_file_changed`, `a_refresh_compares_again_unless_the_current_branch_changed`, `a_refresh_while_it_computes_cancels_that_task_and_drops_its_answer`.
- **241's own test cases:** every `doTest` call and `testContent` of `SelectedBlockHistoryTest` (`selected_block_history_cases_from_intellij`), and the 30 expectations of `FileNameSplittingTest.testSplitByLetter` for the `Get Revision` question (`the path of the question is shortened by letter`).
- **Menus in 241's order:** `the_git_menu_follows_the_order_of_git_main_menu`, `the_current_file_actions_follow_the_focused_window`, `file_several_and_folder_texts_and_enablement_as_241`, `menu_in_intellij_order`, `branch_submenu_in_241_order_and_compare_off_for_current`, `changes_panel_menu_in_241_order`, `file_history_menu_in_241_order`, `the gutter popup has 241's entries in 241's order`, and `the_popup_acts_on_the_selected_revisions`.
- **Events reach the window they are for:** `a_targeted_git_action_reaches_its_own_window_only` (with `the viewer and the diff window hear their own requests only` since the final fix wave), `the_viewer_acts_on_a_git_action_only_for_current_file_while_focused`.
- **Signed commits with `log.showSignature` on** (final fix wave): `signatures_stay_out_of_the_revisions` (comparisons), `signatures_do_not_close_a_refreshed_annotation`, `last_commit_message_without_the_signature_check`, `signatures_stay_out_of_the_proposed_messages` (cherry-pick and revert), `signatures_stay_out_of_the_interactive_rows` and `signatures_stay_out_of_the_cherry_pick_description`, next to the earlier `signatures_do_not_reach_the_history` and `signatures_stay_out_of_the_messages`.
- **Test repositories read no configuration of the machine** (final fix wave): `the_tests_git_reads_only_the_repository_configuration`, through `git::run` and through the git a test spawns.
- **`Show on Double-Click`** (final fix wave): `double click opens the source in the viewer`, `double click opens the diff with Show on Double-Click set to Diff` and `in modal mode a double click in Local Changes opens the source, or the diff with Show on Double-Click set to Diff` (the gear, its one group and the order `Diff`, `Source`).
- **The counts of the guard tests** (`mutating_without_saving_the_diff_is_not_allowed`, `whatever_touches_the_repository_saves_the_diff_window_first`) follow the new commands: `revision_get` and `revision_get_check` flush the diff window first, and `merge_range_commits` is classified as a read.

**What the process left on record.** Several tasks wrote code before its tests and took the failing run afterwards by stubbing the code for one run (Tasks 4, 5 and 7, the TypeScript of Task 12, and some tests of Task 11); Task 10's first Rust red run was not shown (its tests do not compile at the base), and the red run of its fix for merges was seen by another implementer's full run, not shown by its own. Tests that pin behaviour that was already right passed at once and are marked so in their task reports. Two agents worked in this checkout at a time until the freeze of 08:24, so several tasks ran their final suites in a throwaway export of HEAD plus their files; after the freeze every suite ran in the checkout, one at a time.

### 5. Checklist to verify by hand

None of this could be checked automatically. Use a test repository first and then a real one. The `.app` was not rebuilt in this part; to build it and open it, the commands of part 1, section 5.

**The seven of the plan**
- [ ] **History of a real renamed file against IntelliJ's:** `Show History` on a file of a real repository that was renamed, ideally on a side branch, and the same file in IntelliJ 2024.1: the same rows, with the old path on the rows before the rename; the Branch filter at `HEAD`, and empty for every reference.
- [ ] **Opening a file from the terminal:** `Ramify <file>` and `Ramify <file>:<line>` on a file of a repository, then on a file outside any repository (`<file> is not inside a Git repository`), and on a folder (the repository opens as before).
- [ ] **`Get` over a modified file:** from the Changes window and from the file history; `Get Revision` asks, Cancel writes nothing, Overwrite writes the revision; `Restore` of the notice brings the edit back.
- [ ] **Compare Branches on a real branch:** from the branches popup (`Compare with '<current>'`) and from the Branches panel (`Compare Branches` with two chosen): both logs, their changes, details and diff preview; a checkout refreshes them; `Show Diff with Working Tree`, `Swap branches` and `Get from Branch`.
- [ ] **Annotate on a real file:** colours by order, a click selects the commit in the Log, `Annotate Previous Revision`, an edit on disk maps the lines again and a commit from the terminal closes the annotation.
- [ ] **Show Repository at Revision:** from the log menu; the `Repositories` window and its tab; Enter, double click, ⌘↓ and F4 open files at the revision; `Compare with Local`.
- [ ] **Not Cherry-Picked on a branch with picks:** a branch with some commits cherry-picked into the current one; the toggle greys the picked ones and every other row; changing the Branch filter stops it with the balloon.

**Collected during the tasks**
- [ ] **A 19 to 30 MB single-line file with soft wrap in the viewer:** it opens (whole under 20 MiB, as a preview above it) without freezing the window, and the wrap and the scroll behave.
- [ ] **The viewer keeps the focus across a native menu click:** with the viewer in front, Git › Current File › Show Current Revision acts on the viewer's file (WKWebView must keep `document.hasFocus()` true while the menu tracks).
- [ ] **A commit made in the terminal refreshes an open history tab** with the new row.
- [ ] **Show All Affected Files on a root commit:** from the history of a file added in the first commit; the Changes window lists that commit's files, with the file chosen.
- [ ] **241's parent order for merge sides:** a merge row of a file history in Ramify and in IntelliJ: the first parent on the left in both.
- [ ] **The WebView cost of an annotation gutter on a file with 300,000 lines or more:** annotate it and scroll; the window stays usable.
- [ ] **The selection history window's own capability and its close:** the window has only `core:event:default`; it opens, streams, runs its popup (including `Show in Git Log`), and closes with Esc and with its close button, cancelling the stream.

### 6. What could not be checked

- **Nothing was seen in a real window.** The Project, Changes and Repositories tool windows, the viewer, the history and compare tabs, the annotation gutter, the selection history window, `Select Paths to Filter by` and the `Show Details` dialogs were tested in jsdom with the Tauri API simulated.
- **Everything in the second half of section 5:** the 19 to 30 MB single-line files with soft wrap; the viewer's focus across a native menu click (checked against the Tauri sources and in jsdom only); a terminal commit refreshing an open history tab in the running app; Show All Affected Files on a root commit; 241's parent order for merge sides, compared with IntelliJ itself; the WebView cost of a gutter of 300,000 lines or more (only the Rust side was timed, section 4); the selection history window's capability (`core:event:default` only, validated by `cargo build` alone) and its close path, since `request_close` and the window's destruction need an `AppHandle` and the project has no Tauri test feature.
- **No real IPC** of the new commands (`viewer_*`, `tree_list`, `tree_list_at`, `compare_*`, `revision_get*`, `log_history_*`, `log_not_picked`, `annotate_*`, `selection_history_*`, `git_menu_current_file`, `git_file_action`); a failure would be one of registration or argument names.
- **No physical keyboard:** ⌘1, ⌘↓, F4, ⌘W, ⌘D, ⌘R, ⌃⌘A, ⌥⇧⌘C, ⌘= and ⌘− were simulated key events.
- **No comparison with the installed IntelliJ:** the history rows, the annotation colours and the selection blocks follow the sources at the tag and its tests, not the running IntelliJ.
- **The timing test `task::tests::on_quit_the_writer_is_cancelled_and_waited_for`** failed once in a full run of Task 10's last fix round and passed on its own and in the rerun; it passed in the runs of section 7. Its cause was not looked for; it deserves a ticket of its own (final review minor 5).
- **No other measurement:** memory and CPU of the tree, the history walk on a repository with hundreds of thousands of commits, Expand All on a large tree and the selection history over 1,000 revisions were not measured.
- **The integration tests of `src-tauri/tests` were not run in parts 1 to 4 until the final fix wave**, only `cargo test --lib`. Run then, one of them failed: `rewrite::local_changes_are_saved_and_restored` still expected the stash after part 3 made the shelf the default (`45777c1`); section "Final fix wave". They still read the machine's git configuration: they link the library built without `cfg(test)`, and only `crlf_fix.rs` sets its own `GIT_CONFIG_GLOBAL`.
- **The library tests' isolation, checked twice.** The first pass of the final fix wave set `GIT_CONFIG_NOSYSTEM` and `GIT_CONFIG_GLOBAL` but not `XDG_CONFIG_HOME`, so git still read `~/.config/git/ignore` and `attributes`, its defaults for `core.excludesFile` and `core.attributesFile`; on this machine that is the user's `core.excludesfile`, with `.claude/` in it (the final re-review reproduced it with `git check-ignore -v`). The second pass sets `XDG_CONFIG_HOME` too. With it, no library test changed result (section 7), so none was found depending on the user's `core.autocrlf`, `core.excludesfile` or `filter.lfs.*` on this machine; a machine with other settings was not tried.
- **The Repositories tab's `Compare with Local` title** (`a.txt (src -> /repo/src)`, `README.md -> /repo/README.md`) copies 241's format; what `RemoteFilePath.getParentPath` gives for a revision's relative path was not checked, because that class is not in the fetched sources (Task 9 review).
- **The cost of a merge row's selection in a file history** on a large repository (`in_parent_order` walks the whole store per parent, final review minor 2) was not measured.

### 7. Verification

Run on 2026-11-09 after the final fix wave, at the code of `5ab9301` (the commit after it only changes the documents), one suite at a time (`pgrep -fl "vitest|cargo (test|clippy)"` empty before each, the 1-minute load average between 6.2 and 7.3), everything under `nice -n 15`, with the toolchain of `~/.cache/ramify-toolchain`. The Rust library suite ran on the working tree that became `5ab9301`, which was committed with no further change.

| Command | Result |
|---|---|
| `pnpm exec tsc --noEmit -p .` | no errors (exit 0) |
| `pnpm vitest run --maxWorkers=2` | 115 files, 2,368 tests passed |
| `CARGO_BUILD_JOBS=3 cargo test --lib -- --test-threads=3` | 1,989 passed, 0 failed, 6 ignored (on purpose), 309 s |
| `CARGO_BUILD_JOBS=3 cargo clippy --all-targets -- -D warnings` | no warnings |
| `rustfmt --edition 2021 --check` on the 34 Rust files the wave changed | no difference the wave added (the older differences of part 3's finding 10 stay) |
| `python3 -I -m unittest discover -s scripts` | 65 tests, OK |

Against the close of part 3 (92 files and 2,078 tests in the interface, 1,845 in Rust with 5 ignored, 65 in the scripts, after its final fix wave), part 4 adds 23 test files and 290 tests in the interface, 144 tests in Rust and one more ignored one (`large_file_annotation_time`, a timing probe), and none in the scripts. Of those, the final fix wave adds 4 in the interface and 13 in Rust; before it, at `ba8c6e1`, the counts were 2,364 and 1,976. No timing test failed in this run. `cargo fmt --check` was not part of the verification, as in parts 2 and 3.

After the second pass of the final fix wave (the integration tests and the final re-review), run on 2026-11-09, one suite at a time, under `nice -n 15` with `CARGO_BUILD_JOBS=3`:

| Command | Result |
|---|---|
| `cargo test --lib -- --test-threads=3` | 1,992 passed, 0 failed, 6 ignored (on purpose), 284 s. The run before it had 1,991 passed and 1 failed: the timing test `task::tests::on_quit_the_writer_is_cancelled_and_waited_for` (`it already finished and saw the cancellation`), which passed on its own and in this run (section 6). The second run started with the 1-minute load at 17.7, above the limit of 15 |
| `cargo test --test '*' --no-fail-fast -- --test-threads=3` | 5 files: `askpass` 11, `crlf_fix` 1, `editor` 41, `git_path` 2 passed, `phase_3_trace` 1 ignored (on purpose); 55 passed, 0 failed, 1 ignored |
| `cargo clippy --all-targets -- -D warnings` | no warnings |
| `rustfmt --edition 2021 --check` on the files of the second pass | no difference it added |

The interface was not changed by the second pass; its counts are the ones above (2,368). The second pass adds 3 tests in the library and turns one integration test into two (the stash and the shelf).

### Final fix wave

The final review of the part (range `d591fed..5983c37`: 0 critical, 4 important, 6 minor) was followed by one wave, ruled in the ledger: its four importants, `Show on Double-Click`, the matrix citations and the report's gaps. Five of its minors stay parked (section 2).

- **Signatures out of every `log` and `show` that runs without a task** (`c0e439c`). `git::run` adds no `-c` options, so with signed commits and `log.showSignature` on, git printed the signature check on stdout before each record. Fixed with `history::NO_SIGNATURES` in the comparisons (`compare::log`, behind `head_revision_of` and `last_revision`, `current_revision` and `file_revisions`), the annotation refresh (`annotate::last_revision`), and the older `commands::last_message` (Amend), `apply_commits::message_of` (cherry-pick and revert messages), `rewrite::commits_of` (the interactive rebase rows) and `conflicts::picked_of` (the conflict description of a cherry-pick or revert). `rewrite::full_message` runs in a task, which already passes the option. Each fix has a test with signed commits that failed without it.
- **Targeted listens for `viewer-open` and `diff-request`** (`526afd2`): both now listen with the window's own label; the emitters already used `emit_to`.
- **`Get` never overwrites a write that came after its copy** (`479bc6d`). M1: the last check of a path reads the whole file, and the `lstat` before and after that read (device, inode, size, times, mode) must now agree, so a write during the read or right after it leaves the file and reports it. M2: an item written at the old path of a rename whose local file at the new path changed, or could not be removed, is reported written at the old path (so the page offers Restore) and left at the new one; before, it was only "left", with no Restore. M6 (Task 5): a path removed between its `lstat` and the read of its content now holds nothing, like one the `lstat` did not find, and is left; before, it interrupted the whole Get (with the copy kept). A deletion between the check and the copy was already reported as left; a test now pins it.
- **Test repositories isolated from the machine's git configuration** (`5ab9301`): every git of the library tests, the code under test included, gets `GIT_CONFIG_NOSYSTEM=1` and `GIT_CONFIG_GLOBAL` set to an empty file of the tests' own in the temporary folder; `HOME` is left alone. One place, `test_support::isolate`, called by `git::command` under `cfg(test)` and by `test_support::git_command`, which every test module now uses instead of `Command::new("git")`. Production builds are unchanged. No test changed result: all 1,989 pass isolated.
- **`Show on Double-Click`** (`40c2b4d`): part 1's decision 3, as the ruling of the ledger asked (section 3).
- **Second pass** (after the coordinator ran the integration tests and the final re-review):
  - **The integration test of changes saved before a Drop** (`tests/editor.rs`) became two: `local_changes_are_saved_and_restored_with_the_stash`, with `Clean working tree using: Stash` set explicitly and the stash expectations of before (`Conflicts`, the entry kept), and `local_changes_are_saved_and_restored_with_the_shelf`, the default, with 241's outcome: `Cannot find file to patch: B.txt`, `D.txt` back, no stash entry, and the list still alive in the shelf with the `B.txt` change and without the `D.txt` one. Ramify already kept the failed change: no code changed (the notification differs from 241's, section 3).
  - **Library tests isolated from `~/.config/git` too:** `test_support::isolate` sets `XDG_CONFIG_HOME` to an empty folder of the tests' own, `HOME` untouched. Test `the_tests_git_reads_no_ignore_or_attributes_file_of_the_user`.
  - **Get, a link whose place was emptied:** if the link cannot be made after its place was emptied, that path is reported written, so the page offers Restore. Test `a_link_that_cannot_be_made_after_its_place_was_emptied_is_reported_written`.
  - **Get, the stamp after a replace:** the path is expected to hold the bytes just written, not what the disk holds a moment later, so a write in that instant is left by a later item of the same Get. Test `a_write_right_after_the_replace_is_not_overwritten_by_a_later_item`.
  - **Documents:** the gear's placement is a difference (section 3), and the isolation claim of section 6 is corrected.
- **Documents:** the matrix's citations checked again with a script over `src`, `src-tauri/src`, `src-tauri/tests` and `scripts` (every test name cited by a `done` or `partial` row exists; names written as sentences with a capital first letter are not checked by it), the `Show on Double-Click` rows and texts, the parked minors that were missing from section 2, the sentence on deleting several branches, and this section.

### Conclusion

What can be tested without a screen, a keyboard or a real repository is tested, with real git, IntelliJ's own test cases for the selection history and the rows of 241's algorithms for the history, and follows the 241 sources except for the differences in sections 2 and 3. Part 4 is not validated. It ends when you complete the list in section 5, with the history of a real renamed file, `Get` over a modified file and the viewer opened from the terminal as its main risks. The final review and its fix wave are done; what they left parked is in section 2.

## Part 5: GitHub

Date: 2026-11-09. Plan: `docs/superpowers/plans/2026-11-09-phase-4-5-github.md`, with decisions 13 to 18 of the overview. The part has 15 commits after its plan (`1b714e8`): `1b714e8..f0ad129`, 9 features and 6 fixes, in 9 tasks, each reviewed, plus this report (`de159db`). The final review of the whole part (`1b714e8..de159db`) found nothing critical or important of its own and asked for one wave of fixes; that wave and what it fixed are in the section `Final fix wave`, and what it left parked is at the end of section 2.

### Summary

- Part 5 is done in essence: the user works with their github.com repositories as in IntelliJ 2024.1, without OAuth and without review. GitHub accounts with their tokens in the macOS Keychain, `Log In with Token…` and `Log In to GitHub Enterprise…`; protected-branch rules loaded from GitHub after every fetch; the `Pull Requests` tool window with its selector, a list with search, filters and quick filters, and a details tab per pull request with its description, status, commits and changes; `Checkout` and `Show in Git Log` of the pull request's branch; `Open on GitHub` and `Copy Link`.
- **`done` means tests, not use**, as in parts 1 to 4. The Rust logic is tested with real git, throwaway repositories and local bare remotes (pull request refs made with `git update-ref refs/pull/N/head`), GitHub through a fake server on `127.0.0.1` that answers scripted replies and records every request, and the Keychain through an in-memory store; the interface in jsdom with the Tauri API simulated. Nothing was run in the real app or against the real GitHub, and nobody has seen any of it in a real window. The part cannot be accepted without the list in section 5.
- Matrix: one row of section 2 got its result (`Pull requests and merge requests…`, `partial`, because GitLab merge requests wait for overview decision 13) and one of section 3 (`Protected branches: rules loaded from GitHub`, `done`); the push dialog row got a part 5 note; 6 rows were added to section 2 (2 `done`, 4 `out`) and 4 to section 3 (2 `done`, 2 not copied); and a new section 5 lists the 23 GitHub actions of 241 (8 `done`, 15 out of scope). One of the `done` ones, `Github.View.Pull.Request` (`View Pull Requests`, in a `GitHub` submenu of the Git menu), was built by the final fix wave: no task had built it.
- 20 rulings were taken during the part; section 2 lists them with their cost, and then the findings that stay parked after the final review and its fix wave. The differences from IntelliJ that a user can notice are in section 3, by feature.

### 1. What was built

- **HTTP client, servers and remote mapping (Task 1).** `hosting/server.rs` ports `GithubServerPath` (parse, API, GraphQL and web URLs); `hosting/mapping.rs` ports `GitHostingUrlUtil` and `GHHostedRepositoriesManager` (which remote points to which GitHub repository, the parts of `java.net.URI` that decide the host); `hosting/http.rs` is the client on `ureq` with `native-tls`: User-Agent `Ramify`, no gzip, redirects followed by hand (at most 10, the `Authorization` header kept only on the same scheme, host and port as the `url` crate reads them), REST paging by `Link`, GraphQL paging by `endCursor`, a cancellation check between pages, and 241's error texts. `secrets.rs` holds `Secret` (no `Display`, no `Serialize`, no `Clone`, zeroed on drop) and the in-memory store; `test_support::FakeGitHub` is the fake server.
- **Keychain and accounts (Task 2).** `KeychainStore` on `security-framework`; `hosting/accounts.rs` with the list in `<app data>/github-accounts.json` (server and login only), the token under service `ramify GitHub <server>` and account `<login>`, the login that asks `GET /user` before saving anything, re-login, removal, the default account per repository and the `Generate…` URL. The commands run off the main thread, `github_login` as a task.
- **GitHub settings and the login dialog (Task 3).** A `GitHub` section at the end of the settings dialog with the accounts list (`+` popup, `−`, `Set as Default`, `Log in`, rows that load their details one by one) and `Connection timeout:`; `LoginDialog.svelte` with 241's four variants, `Generate…` and errors under the field the backend names; the `Missing default GitHub account` notification with `Configure…`.
- **Open on GitHub and Copy Link (Task 4).** `hosting/links.rs` builds commit and file URLs per GitHub remote, the file at the last commit in HEAD that touched it, with the line range; the items are in the log menu, the file history menu and toolbar, the selection history popup, the annotation gutter, a new `Open In ▸ GitHub` of the Project tree and the viewer, and a new `Copy / Paste Special ▸ Copy Link to GitHub Repository` of the viewer.
- **Protected branches from GitHub (Task 5).** `hosting/protection.rs` ports `GHProtectedBranchRulesLoader` and `PatternUtil.convertToRegex`; it runs inside the task of every fetch, pull and branch update through a new after-fetch hook of `BranchSync`, and stores the rules per remote in `<repo data>/github-protection.json`. The force push check, the remote branch deletion, the rebase, cherry-pick and rewrite checks and the push dialog use the effective list. The settings get `Load branch protection rules from GitHub` and the grey read-only suffix.
- **The Pull Requests tool window (Task 6).** A first-use tool window on the left stripe while a remote points to GitHub, with closable tabs; `hosting/pulls/mod.rs` (`known`, `connect`, the saved choice in `<repo data>/github-pr-connection.json`), and the repository and account selector of 241 with its automatic pair, its saved choice, its login buttons and its errors. Connecting waits until the window is first shown, as in 241.
- **The pull request list (Task 7).** `hosting/pulls/list.rs` (the search string, `issueSearch` pages of 50, the ETag check, the choosers of collaborators, assignees and labels, the last search and ten history entries per repository), `reviewers.rs`, and `hosting/pulls/connections.rs`, which keeps one connection per repository with its token in memory. `List.svelte`, `search.ts` and `poll.ts`: the search field and its history, the filters in 241's order, quick filters, rows with labels, state tags, initials and the comment counter, paging at 70 %, the outdated bar, the context menu.
- **Pull request details (Task 8).** `hosting/pulls/details.rs` (`findPullRequest`, the status from `findPullRequestMergeability` as `GHPRMergeabilityStateBuilder` builds it, the commits from `pullRequestCommits`) and `hosting/pulls/changes.rs` (the two fetches of 241, then the changes with local git: the whole pull request from the merge base, one commit by 241's `before` rule). `Details.svelte`: the title row, the description as plain text, the commits dropdown, the status lines, the changes tree and its menu, the diff window titled `Diff for Pull Request #<n>`.
- **Checkout and Show in Git Log (Task 9).** `hosting/pulls/checkout.rs` ports `GitRemoteBranchesUtil`: find the head's remote or add one named after its owner (`<login>_1`, `<login>_2`… when taken, overview decision 17), HTTP or SSH after `origin`, the fetch of the head branch, then `branches::checkout_remote` with a suggested name and its usual questions and recoverable copy. `Show '…' in Git Log` opens a `Log: <range>` tab. The branch label of the details gets its chooser and the checked-out icon.
- Six GraphQL texts are copied byte for byte from 241 (`getProtectionRules`, `findRepository`, `issueSearch`, `findPullRequest`, `findPullRequestMergeability`, `pullRequestCommits`), and `NOTICE` lists them with the ported classes. Part 5 adds 23 commands; `mutating_without_saving_the_diff_is_not_allowed` now counts 133, with 52 reads and 6 commands that write only app data (`APP_DATA`), and the network commands are in `NOT_RUN` of `scripts/smoke_ipc.py`.

### 2. Decisions and rulings that depart from IntelliJ

Each one with what it costs if it turns out to be wrong. In this part, "plan decision N" is one of the thirteen below, from `docs/superpowers/plans/2026-11-09-phase-4-5-github.md`, and "overview decision N" one of `docs/superpowers/plans/2026-11-03-phase-4-overview.md`.

**Decisions of the overview that this part carries (13 to 18)**

13. **Only GitHub in phase 4; GitLab later.** GitHub Enterprise comes through the `Server:` field. Cost: the merge requests of the spec wait.
14. **Token login only, and no Create Pull Request.** `Log In via GitHub…` goes through JetBrains' OAuth service, which the app does not use; `Create Pull Request…` and the push notification's link are left for later. Cost: the user creates a token once.
15. **Two new dependencies:** `security-framework` for the Keychain and `ureq` with `native-tls`. Cost: a larger binary, and the Keychain may ask for permission after rebuilds that are not signed.
16. **Pull requests without review:** no comments, approve, merge, timeline or reviewer editing; the description is plain text under the title; avatars become initials; the list is polled only while its window is visible and the app has the focus; the changes are computed with local git after 241's two fetches. Cost: visual differences and no Markdown.
17. **Checkout with 241's remote name bug fixed:** when a remote with the owner's name exists with another URL, Ramify uses `<login>_1`, `<login>_2`… where 241 fails. Cost: a small departure.
18. **No Sync Fork, no clone from an account and no Share Project.** Cost: they are done from the terminal or added later.

**Decisions of the plan (1 to 13)**

1. **Account changes apply at once, not on Save.** `+`, `−`, `Set as Default` and `Log in` act immediately, because a token must never ride in `Settings`. Cost: Cancel does not undo an added or removed account.
2. **Where accounts live:** identity `<server>/<login>`, the list in `<app data>/github-accounts.json`, the default account per repository in `RepoSettings.github_default_account`. Cost: none.
3. **Re-login goes straight to the token dialog**; 241 first asks OAuth or token on github.com. Cost: none, there is no OAuth.
4. **No server discovery:** the known servers are github.com and the servers of the accounts. Cost: a GitHub Enterprise remote shows no `Pull Requests` window and no `Open on GitHub` until an account on that server exists.
5. **Connect skips `findUser("ghost")`.** Cost: none; a deleted author shows `ghost`.
6. **Protected rules are stored per remote:** a successful fetch of a remote replaces that remote's entry, and the effective list is the user's patterns plus every entry; 241 replaces the whole list with the remotes of the last fetch. Cost: a remote's rules stay in force until that remote is fetched again (which forbids more, not less), and a fetch can wait for the loader's requests.
7. **No unseen dot, no tool-window badge and no access lines.** 241 clears the dot only from the timeline, which is left out, and the access lines are about merge rights. Cost: visual only.
8. **Where `Open on GitHub` and `Copy Link` go:** 241's places in the log, the file history, the selection history and the annotation gutter; a new `Open In ▸ GitHub` in the Project tree and the viewer; `Copy Link to GitHub Repository` in a new `Copy / Paste Special` submenu of the viewer; several GitHub remotes make a submenu named by URL with `_` shown as a space. Cost: two small submenus that 241 fills with other items, and no Copy Link from the tree.
9. **The changes tree waits for both fetches;** 241 draws the tree from REST and only the diff waits. Cost: a failed fetch shows `Can't load changes` where 241 shows the tree and fails the diff.
10. **The list's last search and its history are kept per repository** in `github-pr-search.json`. Cost: none.
11. **Settings:** `Connection timeout:` is an app setting (`0..60` s, default 5) and reads stay at 60 s; `Clone git repositories using ssh` and `Automatically mark opened files as viewed` are not copied. Cost: none.
12. **HTTP details:** User-Agent `Ramify`, no gzip, redirects by hand up to 10 with the token only on the same origin, and the `Generate…` page's description `ramify GitHub integration plugin`. Cost: none.
13. **The pull request diff opens in Ramify's diff window**, titled `Diff for Pull Request #<n>`. Cost: none.

**Rulings taken during the part**

The order of the work and the interfaces between tasks:
- Task 4 also reads Task 2's `accounts::list`, since the known servers are github.com plus the accounts' servers (plan decision 4); the plan's interfaces left it out. Cost if wrong: none, it is a read.
- One date helper: Task 7 adds `formatPrettyDate` (`Today`, `Yesterday` or a short date, no time) beside the pretty dates of `src/log/draw.ts`, and Task 8 uses the same module. Cost if wrong: a moved function.
- `github_login` is a task whose value is the account's view, not a plain result: Ramify's task pattern, and it lets the dialog cancel. Cost: none.
- The accounts list makes no request: a new read-only command `github_account_details(id)` loads each row, because the brief asks for per-row loading and the default-account check on opening a repository must stay off the network. Cost: one more command.
- `RemoteOptions.suggested` replaces the plan's `name: Some(suggested)` for the checkout: in Ramify `name` is the user's answer and checks out with `-B`, which would skip `NeedsName` and `LocalHasCommits` and silently move a local branch with its own commits. The plan text was wrong and would lose work. Cost: one new optional field.

The process:
- Task 1's fix round also took the review's minors M2 (structured error data and `is_authentication`, which Task 7 needs), M3, M4, M5 and M7. Cost: a slightly larger round.
- The Sonnet 5.5 trailer stays on `73ea667` and on every commit a Sonnet agent made: the attribution states which model wrote it, and history is not rewritten. Cost: mixed trailers across the part.
- The literal texts of Task 3 were listed with their bundle lines in its fix report and checked against the research by its re-review. Cost: none.
- Task 6's fix round also took the minors on Keychain reads on every `repo-changed`, the re-evaluation of the pair while the selector is shown, a cancelled-connect test, the gear's style and the notification body. Cost: a slightly larger round.

Security and the network:
- **The server must read the same in the `url` crate.** The re-review of Task 1 found that 241's host regex accepts `\` and `@`, so the host shown could differ from the one `ureq` connects to; the login, the account details and `Generate…` refuse such a server before any request (Task 2). Cost: one validation and a test.
- **`open_url` accepts any `http(s)` URL with a host, no leading dash and no credentials**, and the callers check the URLs that come from GitHub's data against the connection's server before calling it, since `Generate…` must open a server that is not yet an account. Cost if wrong: an https URL from GitHub's API data could open another site in the browser.
- **The guard `mutating_without_saving_the_diff_is_not_allowed` also scans `hosting/commands.rs`**, where Task 9's checkout lands. Cost: none.
- **`github_pr_chooser` is a cancellable task; `github_pr_etag` stays a plain command**: one bounded request every 30 s whose failures are ignored. Cost: a slow poll cannot be cancelled and waits out the timeouts.
- **The token is cached in Rust memory with the open connection**, never on disk or in the webview, and dropped when the connection is forgotten, on every account change and on an authentication error. The rule "tokens only in the Keychain" is about persistence and exposure, and a Keychain prompt every 30 s on an unsigned build is unusable. Cost: a token revoked elsewhere is used until GitHub answers 401.
- **Check-run URLs on another host open only after a confirmation.** A job page on the connection's GitHub host opens at once. An `https://` page on any other host (an external CI: CircleCI, Jenkins) opens only after Ramify shows `Open External Link` with `This link leaves GitHub:` and the full address as selectable plain text, and the user presses `Open`; `Cancel`, Esc or closing opens nothing. Any other URL (`http://` elsewhere, `javascript:`, `file:`) keeps its row disabled with the tooltip of `refusedUrlText`. `open_url` in Rust is unchanged. Source: the user's answer of 2026-11-09, "open them, but only https:// links, and before opening them Ramify should show you the full address" (translated). This replaces the earlier ruling that refused those links. Cost: one confirmation click that 241 does not ask for; the dialog and its texts are Ramify's own.
- **The checkout runs in three locked steps:** `git remote add` under the write lock, the fetch under the remote's lock with no write lock held, then the checkout under the write lock (Update Project's order); Show in Git Log flushes the diff window. The brief's single write task would have held the write lock while waiting for the remote's lock, the reverse of `branch_delete_remote`, which deadlocks. Cost: the work is split, and a change of the repository between the steps is caught by `checkout_remote`'s own checks.

Behaviour:
- **A DNS failure in an account row reads `Server is unreachable`**, where 241 shows the host's error text; the login dialog matches 241. Cost: a less specific message for a missing host.
- **With the checkbox off, the next fetch clears every remote's entry** (241's first step); with no account or no token on a remote, only that remote's entry is cleared (plan decision 6). Cost: none.
- **A hand-picked account with no token shows its login button after one click on `View Pull Requests`**, which sends nothing: `github_known` reads no token, so tokens stay unread until a connect, and the state ends as 241's. Cost: one extra click.
- **Accepted as written differences of Task 9:** the range tab is titled `Log: <range>`; the checked-out icon is `currentBranch`; an SSH URL on another host than the server is refused before `git remote add`. Cost: on a GitHub Enterprise server whose SSH host differs from its web host, a repository whose `origin` is SSH ends in `Branch checkout failed`, where 241 adds the SSH remote; Ramify does not fall back to HTTP, because 241's rule does not either.

**Findings still parked after the final review and its fix wave**

None of these blocks the part; the final review triaged them and found none that must be fixed before merge. From the ledger's `minor (deferred)` lines and the reviews:
- **Fixed by the final fix wave, no longer parked:** the progress text of the loader that was never cleared (Task 5 M1), the external CI rows that were enabled but always refused (Task 8 M2), the Keychain read of every 30 s poll after an authentication refusal (Task 7), the cancelled chooser's late rejection (Task 7), the token cleared by a mistyped server (Task 3), `tokenless` lost on a remount (Task 6), the Show in Git Log notification titled with the whole error (Task 9 M4), and the final review's five minors. Nothing user-visible is left to fix first.
- **HTTP and accounts (Tasks 1 and 2):** the connect timeout has no test; `GithubMapping`'s equality also compares remote and URL; the body is read as strict UTF-8 with a 10 MB cap; the zeroing of a `Secret` covers only its length, and `ureq` keeps its own copy of the header; `secrets.rs` was not test-first; a comment calls port 1 "privileged"; `Invalid redirect location` is Ramify's own text; a null `/user` body gives other texts; cancel only before and after a request; one thread per account row; the grep guard depends on literals; the `CFData` copy of a Keychain read is not wiped; a removal whose Keychain delete fails after the list was saved leaves an orphan item, with an error (tested); a possibly unneeded `#[allow(too_many_arguments)]`; the guard's cut at the first `#[cfg(test)]` is fragile (only the count catches it).
- **Settings and login (Task 3):** ⌘N works only while the list is empty; Escape cannot cancel a busy login (Cancel can); a rejected details call drops the `Log in` link; no test of the server-before-token order inside `github_login` (it needs an `AppHandle`); a transient row shows `Log in` without its error text until its details arrive.
- **Links (Task 4):** the Project tree's `Open In` sits before `Git`, not last; the menus wait for a git round trip (remote list, `git log`) before showing, with no staleness guard; the viewer passes non-hex revisions to `check_hash`; style in `HistoryTab`; `references` swallows the git cause into `Cannot retrieve latest revision for file`; no red run; Ramify's own GitHub icon.
- **Protection (Task 5):** `protected_branches_set` saves twice; `remotes::list` runs on every fetch even with no accounts; coverage gaps; the suffix is `aria-hidden`; only the affected tests were rerun after a last fix; a corrupt `github-protection.json` reads as no rules in the lenient list; the loader uses the lenient `accounts::token`, reads the Keychain after every fetch, and a denied read passes silently.
- **Window and selector (Task 6):** the extras beyond the brief (`github_forget_connection`, the extra fields, the `View Pull Requests` task title).
- **List (Task 7):** the per-repository connection map keeps the tokens of repositories left behind until the accounts change; the test `the_last_search_and_ten_history_entries_are_kept_per_repository` uses one folder only; the `missing_token` branch of the list's error panel has no interface test; `List.svelte` is about 1,000 lines; the rows' `onkeydown` only silences the lint and the tooltips open on hover only; a user who switches apps more often than every 30 s is never polled; the `Search History` popup opens empty when there is no history.
- **Details (Task 8):** `Retry` on one commit does not fetch again after a failure that came after the fetch; a `+x` base ref passes `check-ref-format` (as in 241); `github_pr_changes`' decision logic has no test; `Details.svelte` is about 850 lines; TDD by mutation, and the full library suite was not rerun after its last test; small 241 differences not written down by the task (the chooser's position, the expanded state, the status error text).
- **Checkout (Task 9):** the `listen` mock of `Details.test.ts` is not reset; a deleted fork whose base fails validation shows an error, where 241 does nothing; `eq_ignore_ascii_case` where Kotlin folds non-ASCII letters too; the test constants `CHECKOUT_KIND` and `SHOW_KIND` are not tied to the commands; the deadlock test depends on a 300 ms window; out of scope, Ramify's `remote_add`, `remote_edit` and `remote_remove` run as remote tasks without the write lock.

### 3. Differences from IntelliJ that a user could notice, by feature

From the task reports, their reviews and the rulings. Each one is accepted, with its reason and its cost; the parked ones of section 2 are marked "parked".

**Accounts, login and the Keychain**
- **Account changes apply at once** (plan decision 1). Why: a token never rides in `Settings`, and the dialog saves `Settings`. Cost: Cancel does not undo an added or removed account.
- **The token is cached in memory** beside the open connection, read from the Keychain once by the connect (ruling of Task 7). Why: a Keychain read per request and per 30 s poll prompts on unsigned builds. Cost: a token revoked on GitHub keeps being used until GitHub answers 401; the tokens of repositories left behind stay in memory until the accounts change (parked). After GitHub refuses it, the list's poll stops until a re-login (see the list below), so the Keychain is not read again every 30 s.
- **Opening a repository reads no token.** The `Missing default GitHub account` check asks for the accounts without their tokens (final fix wave). Why: on an unsigned build every Keychain read can prompt. Cost: none.
- **A denied Keychain read says so:** `Could not read the access token from the Keychain: <the Keychain's text>`; only a missing item is `Missing access token`. 241 counts any failure as no token. Why: telling a refused prompt from a missing token. Cost: none.
- No OAuth (`Log In via GitHub…`), and re-login opens the token dialog straight away (overview decision 14, plan decision 3). Cost: the user creates a token once.
- An unsigned rebuild may make macOS ask for access to the Keychain item (overview decision 15). Cost: one prompt per rebuild until "Always Allow".
- A row whose host cannot be resolved reads `Server is unreachable` (ruling); a Unicode host is refused as `Server URL is not valid`, because the `url` crate reads it as punycode; a port above 65535 is refused at once. Cost: less specific texts in rare cases.
- The default account is a per-repository id: after a re-login with another login or a removal, only the open repository's default follows; other repositories keep an id that matches no account and show no default. 241 keys it by UUID. Cost: the user sets the default again there.
- Ramify's own texts where 241 swallows the error: `Could not save the account: …` and `The account <id> is no longer in the list`. A token that GitHub echoes back in an error reads `***`.
- No avatars: the rows and the selector show the login, the list shows initials (overview decision 16). Escape cannot cancel a busy login, and ⌘N works only while the list is empty (parked).
- **A row whose details call is rejected loses its `Log in` link**, and until a row's details arrive a row with no token shows `Log in` without `Missing access token` (parked, Task 3). Why: the details are loaded row by row and a rejected call (an account that went away meanwhile) writes only its error. Cost: in that rare case the user logs in from the `+` popup instead; the transient row fixes itself within a request.
- **A mistyped server keeps the pasted token** in the login dialog: only a server error leaves it (no server was asked); success, Cancel and a token error clear it, and each Log In sends it once (final fix wave). 241 does the same, since it checks the server before sending anything.
- **The details of a REST error are on their own lines**, where 241 joins them with `<br/>` for its HTML panel (final fix wave). Cost: none; it reads as 241's.

**Open on GitHub and Copy Link**
- **The `Open In` and `Copy / Paste Special` submenus** are new in Ramify and hold only the GitHub items, because Ramify copies none of 241's other items there (`Reveal in Finder`, `File Path`, `Copy Reference`, the plain-text copy and paste); they show only while there is a GitHub reference (plan decision 8). Cost: two small submenus with one item each; no Copy Link from the Project tree, which has no `Copy Path/Reference…` popup. The tree's `Open In` sits before `Git`, not last (parked).
- A GitHub Enterprise remote shows no item until an account on its server exists (plan decision 4).
- The menus wait for a `git remote` and, for a file, a `git log` before they open (parked); Ramify draws its own GitHub icon.
- A failure to find the references still shows the item, which says `Cannot open in browser` when used, as 241 does.

**Protected branches**
- Rules are stored per remote (plan decision 6): fetching a non-GitHub remote no longer empties the GitHub rules, and a remote's rules stay until that remote is fetched again.
- A fetch waits for the loader's requests, within the connect timeout. The progress text `Loading GitHub branch protection rules` shows while the loader asks, then the task's own text comes back (final fix wave). A `Pull…` that merges lets go of the repository's write lock before the loader asks GitHub, so a commit or rollback does not wait for GitHub (final fix wave); its result is the same.
- **A damaged `github-protection.json` reads as no rules** in the lenient list until the next fetch of that remote rewrites it (parked, Task 5 M7). Why: the lenient read is the one the checks share. Cost: until then a force push to a branch protected only on GitHub is not refused by Ramify (GitHub itself still refuses it), and the user's own patterns still apply.
- A loader failure writes one line to stderr, not to Ramify's Console; 241 writes it to `idea.log`.
- The grey suffix is an overlay that does not scroll with a long text, and it is hidden from screen readers (parked).

**Pull Requests window and selector**
- **No unseen dot** on new or updated pull requests and no badge on the stripe button (plan decision 7). Why: 241 clears them only from the timeline, which is left out. Cost: visual only. No access lines (`Repository access required…`, `Merging is disabled…`) either.
- **A hand-picked account with no token needs one click:** its login button shows after one click on `View Pull Requests`, which sends nothing (ruling of Task 6). Cost: one extra click. The window remembers it while the repository stays open, also when the selector comes back from the gear, and a login of that account anywhere, Settings included, forgets it (final fix wave).
- The window connects only once it is first shown, as 241's lazy selector; a connect appears in the status bar as `View Pull Requests`, where 241 shows only the busy selector.
- **`View Pull Requests` is in a `GitHub` submenu of the Git menu**, its only item there, and the submenu is in the menu only while the window is available (final fix wave). 241's `GitHub.MainMenu` also holds Share, Create Pull Request, Sync Fork, Gist and `Open on GitHub`, which Ramify does not copy into that submenu; Ramify has no `Git.Menu` (the old UI's VCS popup), the action's other place. Cost: none for the window; `Open on GitHub` is reached from the context menus only.

**Pull request list**
- The list is polled only while its window is visible and the app has the focus (overview decision 16); a user who switches apps more often than every 30 s is never polled (parked).
- **A poll that GitHub refuses stops the polling** and shows 241's list error, `Can't load pull requests`, `User authorization failed` (or `Missing access token`) and `Re-Login`; `Re-Login` or a refresh loads the list again and polling starts again (final fix wave). 241 ignores every poll failure and keeps polling. Why: each poll would read the Keychain again, a prompt every 30 s on an unsigned build, for GitHub to refuse it again. Cost: the rows give way to the error panel at once, where 241 would show it only at the next page load or refresh. Other poll failures are still ignored.
- The outdated bar is decided again on every poll, as in 241, so it can go away 30 s later without a refresh.
- The Author, Assignee and Label choosers show the login alone and ask GitHub's default page size, as 241's source does (the research said otherwise).
- A page and each chooser appear in the status bar as tasks (`Loading pull requests`, `Loading collaborators`…), Ramify's own texts.
- A missing token shows `Can't load pull requests`, `Missing access token` and `Re-Login`.
- Requested reviewers that are neither users nor teams (bots, mannequins) are left out of the row.
- `Search History` opens an empty popup when there is no history; the hover lists of labels, assignees and reviewers are not reachable from the keyboard (parked).

**Pull request details**
- **Check-run links to an external CI open after a confirmation** (the user's decision of 2026-11-09, replacing the ruling of Task 8): `Details…` opens at once a URL on the connection's GitHub host; an `https://` URL elsewhere is enabled and, when chosen, shows `Open External Link` (`This link leaves GitHub:`, the full address, `Open` and `Cancel`), Ramify's own dialog; any other URL stays disabled with the tooltip `Only an https:// address on <host> can be opened` (or `an https:// or http://` one for an `http://` server). Why: the user wants those links to open, but only `https://` and only after seeing the whole address. Cost: one confirmation click, which 241 does not ask for.
- Small differences of the details that Task 8 did not write down (parked, Task 8 M7): the `Open in Browser` chooser opens below its link, where 241 shows it above; `Show details`/`Hide details` stays as it was when another commit is chosen; and the status's own `Could not find pull request <n>` differs from the details' `Pull request <n> does not exist`, but nobody sees it, since the details' text is the one shown. Cost: cosmetic.
- The description is plain text, without Markdown (overview decision 16); the commits show the plain headline.
- The changes tree waits for both fetches, so a failed fetch shows `Can't load changes` (plan decision 9); the diff opens in Ramify's diff window, `Diff for Pull Request #<n>` (plan decision 13).
- The changes menu has no `Mark as Viewed` and no `Open in Separate Window`; `Jump to Source` opens the working-tree file in the viewer and skips deleted files.
- The status asks `findPullRequest` again for the base branch's rule (one more request); the jobs chooser has no status icon per row, only a tooltip, and the status lines use dots, not 241's icons; a state GitHub may add later counts as pending.
- One commit reuses the fetches of the whole pull request, so a commit's diff relies on objects fetched earlier in the session; `Retry` on one commit does not fetch again after a failure that came after the fetch (parked).
- Ramify's own texts: `Loading pull request details`, `commits` and `changes`, `The pull request has changed since it was loaded` and `Commits … have no common ancestor`.

**Checkout and Show in Git Log**
- **An SSH origin whose SSH host differs fails the checkout** (ruling of Task 9): a URL from GitHub is added as a remote only when it is on the server's host (and port, for HTTP), with no credentials. Cost: on a GitHub Enterprise server whose SSH host is not its web host, a repository with an SSH `origin` ends in `Branch checkout failed` with the URLs, where 241 adds the remote and goes on.
- A taken remote name becomes `<login>_1`, `<login>_2`… (overview decision 17), where 241 fails.
- A failed `git remote add` is reported under `Branch checkout failed`; 241 ignores it. A head or base branch with a leading `+` is refused as not valid. Other failures say `Branch checkout failed` with the message, for Show in Git Log too: 241 gives it that title when the remote cannot be resolved (`notifyRemoteError`), and the message is the body (final fix wave).
- The range tab is titled `Log: <range>` and has no root filter; the checked-out icon is `currentBranch`.
- The checkout uses the details the tab shows, so a pull request changed on GitHub since the tab loaded is checked out as shown, as 241 does with its view model.
- A deleted fork whose base name fails the check shows an error, where 241 does nothing (parked).

**Settings**
- `Connection timeout:` is an app setting, and 0 means no connect timeout; `Clone git repositories using ssh` and `Automatically mark opened files as viewed` are not shown (plan decision 11).

### 4. What was proven with tests

Each feature and the tests that name it; the full lists are in the Result column of `docs/parity-matrix.md` (sections 2, 3 and 5).

- **A token leaks (Review Focus 1):** in Rust `the_token_never_shows_in_an_error_or_debug` (a token in a JSON message, a rate limit, a raw body, GraphQL errors, a GHE header, a `Link` header and a URL query, formatted with `{}`, `{:?}` and `{:#?}`), `a_redirect_to_another_host_drops_the_token`, `a_redirect_that_hides_another_host_behind_a_backslash_drops_the_token` (seen failing before its fix), `a_server_the_url_crate_reads_as_another_host_is_refused_before_any_request` (seen failing before its fix), `no_command_returns_a_token` (a grep of the commands' return types, then end to end: the token only in `Authorization`, never in a target, a body, another header, a view or the list file), `the_token_is_not_in_the_console_after_a_failed_load`, `a_secret_prints_no_text`, `the_token_is_read_once_for_many_pages_and_polls`, `the_kept_token_goes_after_a_relogin_or_a_refused_request` and `a_pull_request_tab_request_refused_by_github_drops_the_kept_token`; in the interface `the_token_is_sent_once_and_cleared`.
- **Remote URLs of every shape (Review Focus 2):** `remote_urls_map_as_241` (scp, `ssh://` with a port, trailing slash, upper case and `.git`, a GHE prefix, a local path, an underscore host, a space, no owner), `remotes_on_one_repository_give_one_mapping_and_a_fork_gives_two`, `the_first_server_that_matches_wins`, `the_web_base_keeps_the_remote_case_and_the_server_prefix`, `server_paths_follow_241`, `two_github_remotes_give_a_submenu_named_by_url` (Rust and interface), `checkout_finds_the_heads_remote_or_adds_one_named_after_its_owner`, `http_or_ssh_follows_origin` and `a_taken_name_becomes_login_1_then_login_2`.
- **The network fails (Review Focus 3):** `errors_read_like_241` (401 `Bad credentials`, 403 rate limit, 404, 422 with errors, non-JSON bodies, GraphQL `NOT_FOUND`, `INSUFFICIENT_SCOPES` and several errors, an HTML body with 200, GHE versions below and above 3.0, a refused connection), `a_slow_server_times_out_within_the_connect_and_read_limits`, `a_zero_connect_timeout_means_none` (seen failing before its fix), `a_redirect_loop_stops_at_ten`, `paging_stops_when_the_task_is_cancelled`, `a_client_without_tls_says_so_on_https`, `login_checks_user_and_stores_the_token_only_on_success`, `a_loader_error_keeps_the_entry_and_the_fetch_still_succeeds`, `an_unreachable_server_does_not_fail_update_project`, `a_page_that_is_not_a_search_fails_with_241s_texts`, `errors_show_cant_load_with_re_login_for_auth_and_retry_otherwise` (Rust and interface), `a_list_failure_keeps_whether_it_is_an_authentication_error`, `a_denied_keychain_read_is_not_reported_as_a_missing_token`, `a_missing_pull_request_says_241s_text`, `a_failed_fetch_fails_and_the_next_one_fetches_again`, `cant_load_details_offers_retry_or_re_login` and `the_status_says_loading_until_it_loads_and_after_an_error`.
- **Untrusted text from GitHub (Review Focus 4):** `a_title_with_html_is_text_and_a_bad_label_colour_is_dropped` (Rust and interface: `<img onerror>` as text, `red;background:url(x)` dropped), `details_ask_find_pull_request_and_keep_its_texts_as_text`, `the_description_is_plain_text_and_a_same_host_job_opens_at_once`, `an_external_link_is_shown_as_text` and `an_external_http_job_and_a_javascript_job_stay_disabled` (`<img onerror>`, `<b>`, a right-to-left mark; `javascript:`, `file:` and `http://` refused), `a_url_from_github_opens_only_on_the_connections_server`, `commits_read_every_page_with_the_plain_headline_and_the_presentable_author`, `a_head_branch_named_dash_x_never_reaches_git_as_an_option` (also an owner `-x` and a base `--output=x`), `a_base_ref_that_git_would_not_take_as_a_branch_is_refused` (`-x`, `--upload-pack=…`, `@{-1}`, `a..b`, `+main`), `a_hash_that_is_not_one_reaches_no_git` and `a_hash_that_could_be_an_option_is_refused`.
- **The repository changes while a tab is open or loads (Review Focus 5):** `an_answer_for_an_old_head_or_another_repository_is_dropped`, `refresh_reloads_details_status_and_changes_for_the_new_head`, `commits_for_another_head_are_dropped`, `the_checked_out_icon_follows_repo_changed` (with `the_branch_label_has_the_checkout_chooser_and_the_checked_out_icon`), `an_answer_for_a_forgotten_window_is_dropped`, `a_connect_that_finishes_after_another_repository_opened_is_dropped`, `a_login_that_finishes_after_another_repository_opened_connects_nothing`, `a_late_connect_never_replaces_a_later_one`, `checkout_fetches_the_head_branch_and_keeps_a_recoverable_copy` (an ignored file in the way is saved in a snapshot), `a_checkout_and_a_remote_branch_deletion_on_the_same_remote_both_finish` and `concurrent_shows_in_git_log_pick_distinct_remote_names` (both seen failing before their fix), and `nothing_here_touches_the_work_tree_or_the_index`.
- **Protected branches:** `convert_to_regex_is_241s`, `every_page_is_loaded_with_the_default_account_else_the_first_on_that_server`, `fetching_a_non_github_remote_clears_only_its_entry`, `the_checkbox_off_ignores_the_loaded_rules_and_the_next_fetch_clears_them`, `pull_merge_and_update_branch_also_load_the_rules`, `a_fetch_loads_the_remotes_rules_and_force_push_to_them_is_prohibited` (a fetch, then a forced push refused with `Force push to release/1 is prohibited` and the remote ref unchanged), and in the interface `the_github_rules_follow_the_users_in_grey_and_are_not_saved` and `a_blank_field_shows_the_github_rules_without_a_separator_after_focus_loss`.
- **241's own rules ported and checked case by case:** the search string (`the_search_string_is_241s`), the quick filters and the filter order (`the_quick_filters_are_241s`, `the_filter_dropdowns_are_in_241s_order_with_their_values`), the reviewer map (`the_reviewer_map_follows_241`), the CI line (`the_ci_line_follows_241`, every rule including "nothing required and failed"), the required approvals (`required_approvals_show_only_when_blocked_and_not_failing`), the jobs (`the_jobs_are_the_last_commits_contexts_then_its_check_runs_with_241s_states`), the commit order (`commits_are_in_a_post_order_from_the_last_one`), the per-commit `before` (`per_commit_changes_follow_241s_before_rule`), the display names (`the_repository_combo_is_sorted_and_names_the_server_when_there_are_several`, `the_list_tab_is_named_as_241_names_the_repository`) and the URLs (`urls_follow_241`, `a_file_url_uses_the_last_commit_that_touched_it_not_head`).
- **Menus in 241's order:** `each_menu_has_open_on_github_at_241s_place_log`, `…_file_history`, `…_history`, `…_selection_history` and `…_annotate`, `enter_or_a_double_click_opens_and_the_context_menu_is_241s`, `the_plus_popup_offers_token_then_a_separator_then_enterprise` and `the_popup_texts_are_241s`.
- **The guard tests follow the new commands:** `mutating_without_saving_the_diff_is_not_allowed` (133 commands, 52 reads, 6 app-data writers), and `whatever_touches_the_repository_saves_the_diff_window_first`, which now reads `hosting/commands.rs` and lists `github_pr_checkout` and `github_pr_show_in_log`.

**What the process left on record.** Tasks 3 and 4 wrote their tests with the code and showed no red run (Task 3 showed one in its fix round); Task 8 wrote its Rust and component tests with the code and proved them by breaking the code on purpose (each break failed a test; three tests had to be strengthened first); Task 1's `secrets.rs` was not test-first. Task 3's first full interface run came before its last additions (the controller reran it); Task 8's last full library run came before its last test (its module and the guard module were rerun). Task 4 started its final suites in a background chain, against the load rules; the controller checked it ran alone. In Tasks 5, 7 and 9, and in this report's verification, the harness moved the full library run to the background after 180 s by itself; each time nothing else was started until it finished. Task 1's `cargo fetch` downloaded the crates of every target of the lock, beyond the approved ones; only the needed entries entered `Cargo.lock`. `cargo fmt` reformatted unrelated files in Tasks 1, 2 and 3; each time the change was saved to the scratchpad and reverted.

### 5. Checklist to verify by hand

None of this could be checked automatically. Use a test repository with a throwaway GitHub repository first, then a real one. The `.app` was not rebuilt in this part; to build it and open it, the commands of part 1, section 5.

**The seven of the plan**
- [ ] **A real github.com account with a real token, and the Keychain item after a restart:** `+` ▸ `Log In with Token…`, `Generate…` opens the pre-filled page, the token logs in; quit and reopen Ramify: the account is still there. In Keychain Access the generic password `ramify GitHub github.com` has the login as its account, and `~/Library/Application Support/dev.ramify.app/github-accounts.json` holds no token.
- [ ] **The list, filters and paging on a real repository with pull requests:** the default `Assigned to you`, each filter and quick filter, the search history, scrolling past 50 rows, the outdated bar after a pull request opens elsewhere.
- [ ] **A pull request from a fork:** its details, its description, the status lines and `Details…`, the commits dropdown, the changes and their diff.
- [ ] **Checkout of that pull request and Show in Git Log:** the remote named after the fork's owner (and `<login>_1` when the name is taken by another URL), the suggested `fork/<branch>`, the checked-out icon, and the `Log: <range>` tab.
- [ ] **A force push to a protected branch after Fetch:** a branch protected on GitHub and not in the user's patterns; after a Fetch, the force push is refused with `Force push to <branch> is prohibited`.
- [ ] **Open on GitHub from each menu:** the log, the file history menu and its toolbar button, the selection history, the annotation gutter, the Project tree's and the viewer's `Open In ▸ GitHub`, and `Copy / Paste Special ▸ Copy Link to GitHub Repository` with a line range; the browser opens the right page.
- [ ] **An unsigned dev build asking for Keychain access:** after a rebuild, the first token read asks; "Always Allow" stops it for that build.

**Asked for this report**
- [ ] **A real fetch loading protection rules, and the grey suffix in Settings:** the hook that the app's setup installs (`start_protection_loader`) runs after a Fetch; the progress text shows; reopening Settings shows the GitHub rules in grey after the user's text, and with the checkbox off they are gone after the next Fetch.
- [ ] **The Pull Requests window connecting only once shown:** open a repository whose single GitHub remote and single account form the automatic pair; nothing is sent to GitHub until the window first comes to the front.
- [ ] **A denied Keychain prompt showing the Keychain's error:** deny the prompt; the selector and the list say `Could not read the access token from the Keychain: …`, not `Missing access token`.

**Collected during the tasks**
- [ ] The ignored test `the_keychain_store_keeps_replaces_and_deletes_a_token`, run by hand: `cargo test --manifest-path src-tauri/Cargo.toml --lib keychain -- --ignored`. It writes and deletes only `ramify GitHub test-<pid>.invalid` in the login Keychain.
- [ ] The `Pull Requests` window, its gear and `Change Repository or Account…`, the selector's layout, and the closable tabs, in a real window.
- [ ] A long protected-branches text with the grey suffix: the overlay stays aligned when the field scrolls.
- [ ] A pull request whose checks run on an external CI: in `Details…` those rows are enabled; choosing one shows `Open External Link` with the full address; `Cancel` or Esc opens nothing, `Open` opens the browser at that address. A job whose URL is `http://` on another host stays disabled with the tooltip `Only an https:// address on github.com can be opened`, and a job on github.com opens with no dialog.
- [ ] `Git ▸ GitHub ▸ View Pull Requests` in the real menu bar: the submenu is there, before `Manage Remotes…`, only while a remote points to GitHub; it goes when the last such remote is removed and comes back when one is added; it brings the `Pull Requests` window to the front on its list.
- [ ] A token revoked on GitHub while the list is shown: within 30 s the list shows `Can't load pull requests`, `User authorization failed` and `Re-Login`, and an unsigned build asks for the Keychain no more until `Re-Login`.
- [ ] `scripts/smoke_ipc.py` against a built app: the commands it runs, with the network ones in `NOT_RUN`.

### 6. What could not be checked

- **Nothing was run in the real app or against the real GitHub.** No real token, account, pull request, fork, check run or protection rule was used; every answer came from the fake server, with replies written from the research and 241's sources. Nothing was seen in a real window: the settings section, the login dialog, the `Pull Requests` window and the new menu items were tested in jsdom.
- **The hook that the app's setup installs is untested.** `start_protection_loader` (called from `lib.rs`) needs an `AppHandle`; the tests drive the loader and the after-fetch hook directly.
- **The real Keychain was checked only through an ignored test**, `the_keychain_store_keeps_replaces_and_deletes_a_token`, which the suites do not run and which was not run for this report; every other test uses the in-memory store. The prompt of an unsigned build, a denied prompt and "Always Allow" were not seen.
- **The timing test `task::tests::on_quit_the_writer_is_cancelled_and_waited_for` has flaked before** (part 4, section 6, unexplained); it passed in the run of section 7.
- **No real IPC** of the 23 new commands (`github_accounts`, `github_account_details`, `github_login`, `github_remove`, `github_set_default`, `github_generate_url`, `github_links`, `github_known`, `github_connect`, `github_forget_connection`, `github_pr_page`, `github_pr_etag`, `github_pr_chooser`, `github_pr_search_saved`, `github_pr_search_save`, `github_pr_details`, `github_pr_status`, `github_pr_commits`, `github_pr_changes`, `github_pr_checkout`, `github_pr_show_in_log`, `github_pr_checked_out`, `open_url`); `scripts/smoke_ipc.py` was not run, and a failure would be one of registration or argument names.
- **The connect timeout itself has no test:** on loopback macOS refuses a full listen queue at once instead of letting it time out, and an unroutable address would touch the network (Task 1). The read timeout and the zero timeout are tested.
- **Not compared with 241 running:** the port of `java.net.URI`'s host rules covers every shape of Review Focus 2 but not rare authorities; how 241's Jackson treats unknown reviewer types or states; the exact order of 241's `Copy / Paste Special` (derived from the XML anchors); the research's own "could not verify" list.
- **The order "server before token" inside `github_login`** follows from the command's code, which needs an `AppHandle` to test; the `LogActions` wiring of the log menu's GitHub items has no component test (the menu builder is tested).
- **No physical keyboard:** ⌘R in the list and the details, ⌘N in the accounts list, ⌘D, ⌘↓, Enter and the arrow keys were simulated key events.
- **The native GitHub submenu is not tested in a real menu.** Taking `GitHub ▸ View Pull Requests` out of the Git menu and putting it back before `Manage Remotes…` (`sync_github_menu`) needs an `AppHandle`; the tests check the menu's entries and order, the rule that shows it (`github_menu_shown`) and what the interface sends, not the native menu itself.
- **The `Mark as Viewed` and `Mark as Not Viewed` rows of matrix section 5** take their Feature texts from CollaborationToolsBundle keys, and their shortcuts and those of the other rows were derived from the default keymap's XML; neither was compared with the GitHub action classes or a running 241. Both rows are out of scope, so nothing in Ramify depends on them.
- **No measurement:** a list with thousands of pull requests, a pull request with hundreds of commits or files, and the cost of the fetches on a large repository were not measured.

### 7. Verification

Run on 2026-11-09 after the final fix wave, on the working tree that the wave's code commits hold (the documents commit after them changes no code), one suite at a time in the foreground, under `nice -n 15`, with `CARGO_BUILD_JOBS=3` and the toolchain of `~/.cache/ramify-toolchain`; `pgrep -fl "vitest|cargo (test|clippy)|rustc"` was empty before each. The 1-minute load average was 40.5 when the second library run started (nothing of this wave was running; it had fallen to 9.6 when the run ended) and 5 to 10 otherwise. The harness moved the library runs to the background after 180 s by itself; nothing else ran until each finished.

| Command | Result |
|---|---|
| `pnpm exec tsc --noEmit -p .` | no errors (exit 0) |
| `pnpm exec vitest run --maxWorkers=2` | 127 files, 2,509 tests passed |
| `CARGO_BUILD_JOBS=3 cargo test --lib -- --test-threads=3` | 2,117 passed, 0 failed, 7 ignored (on purpose), 313.97 s. The run before it had 2,116 passed and 1 failed: `the_uncommitted_changes_group_holds_the_stash`, which lists every group of the Git menu and did not have the new `GitHub` one yet; its list was completed and the suite run again |
| `CARGO_BUILD_JOBS=3 cargo test --no-fail-fast --test '*' -- --test-threads=3` | 5 files, 4 with tests that ran: `askpass` 11, `crlf_fix` 1, `editor` 41 and `git_path` 2 passed; `phase_3_trace` has 0 passed and 1 ignored (on purpose); 55 passed, 0 failed, 1 ignored. The run before it had 2 failures in `editor` (`pull_rebase_interactive_does_not_wait_for_a_terminal` and `cancelling_an_interactive_pull_with_the_todo_pending_leaves_no_rebase`): they ran the pull in a write task, and the pull now takes the write lock itself, so they waited for their own lock; they now start a read task, as `git_pull` does |
| `CARGO_BUILD_JOBS=3 cargo clippy --all-targets -- -D warnings` | no warnings (exit 0) |
| `python3 -I -m unittest discover -s scripts` | 65 tests, OK |

The 7 ignored library tests are the six of part 4 and `the_keychain_store_keeps_replaces_and_deletes_a_token`. Against the close of part 4 (115 files and 2,368 tests in the interface, 1,992 in Rust with 6 ignored, the same integration and script counts), part 5 adds 12 test files and 141 tests in the interface, 125 tests in Rust and one ignored one, and none in the integration tests or the scripts. Of those, the final fix wave adds 10 tests in the interface and 3 in Rust (4 new, and `the_task_kind_is_read_with_rebase_and_write_without_it` removed with `pull::task_kind`); before it, at `f0ad129`, the counts were 2,499 and 2,114. The timing test `on_quit_the_writer_is_cancelled_and_waited_for` passed. `cargo fmt --check` was not part of the verification, as in parts 2 to 4; `rustfmt --edition 2021 --check` on the Rust files the wave changed shows no difference that the wave added.

The citations of the matrix were checked, before the final fix wave, with a script over `src`, `src-tauri/src` and `src-tauri/tests`: of the 159 snake-case names in the rows that mention part 5 and in section 5, 158 are test names that exist there (a Rust `#[test]` function or a Vitest `test`/`it`); the other one, `start_protection_loader`, is cited as the function that has no test.

### Final fix wave

The final review of the part (range `1b714e8..de159db`, read only: 0 critical, 0 important of its own, 5 minors) confirmed the four items the controller had queued and suggested five of the deferred minors; the ledger ruled one wave for all of them and the Task 10 report gaps. Each code fix has a test that was run and seen failing before its fix (the red runs are in the wave's own report, outside the repository).

- **`View Pull Requests` (`Github.View.Pull.Request`)** (`6abdb52`): `GithubViewPullRequestsAction` in a `GitHub` submenu (`GitHub.MainMenu`) of the Git menu, before `Manage Remotes…` (github-core-config.xml:97-99 and 111-120), there only while the `Pull Requests` window is available; it brings the window to the front on its list tab. Tests `view_pull_requests_is_in_the_git_menu_only_with_the_window_and_brings_it_to_the_front`, `view_pull_requests_brings_the_window_to_the_front_on_its_list` and, in Rust, `view_pull_requests_is_in_a_github_submenu_shown_only_with_the_window`. Matrix section 5's row is `done`.
- **The protection loader** (`a2b5279`) gives the task back the progress text it showed before (test `the_loader_puts_back_the_progress_text_the_task_showed_before`), and a `Pull…` that merges now takes the write lock only around `git pull` and its counts, then runs the after-fetch hook without it, with the same result (test `a_merge_pull_holds_the_write_lock_for_git_only_and_runs_the_hook_without_it`; `pull::task_kind` is gone, the pull task is a read task that takes the lock itself, as Update does).
- **No Keychain read after a refusal** (`6614f78`): an authentication or missing-token answer to the 30 s poll stops the poll and shows the list's `Re-Login` panel; the re-login's reload starts it again (tests `a_refusal_stops_the_poll_until_it_is_turned_on_again`, `a_refused_poll_stops_polling_and_offers_re_login`, and `a_poll_that_fails_otherwise_goes_on_and_shows_nothing` for the other failures). In the same commit, a cancelled chooser's late rejection no longer reaches a dropdown opened again (test `a_cancelled_chooser_that_rejects_late_leaves_the_reopened_dropdown_alone`).
- **No Keychain read on repository open** (`f8626c5`): `github_accounts` takes `tokens: false`, which reads no token, for the default-account check (tests `the_rows_without_tokens_read_no_keychain_item` and `a_stored_default_that_is_not_an_account_warns_with_configure`).
- **REST error details on their own lines** (`754c8fb`): `getPresentableError`'s `<br/>` became a line break (test `errors_read_like_241`).
- **Small user-visible fixes** (`3367cf6`): one `github_links` look-up per right-click in the viewer (`the_viewer_copies_the_link_with_its_lines`); external CI rows disabled with a tooltip that says why (`the_description_is_plain_text_and_a_same_host_job_opens_at_once`); the login dialog keeps the token when only the server is wrong (`a_mistyped_server_keeps_the_token_for_the_next_try`, `server_errors_of_the_backend_show_under_the_server_field`); a Show in Git Log failure titled `Branch checkout failed` with the error in the body (`a_failure_is_shown_with_its_message`).
- **`tokenless` in the window's store** (`479e01b`): the accounts found without a token survive a remount of the selector, and any login of the account, Settings included, forgets it (`an_account_found_without_a_token_stays_so_when_the_selector_comes_back`, `a_login_in_settings_forgets_that_the_account_had_no_token`, `a_login_is_announced_to_whoever_listens`).
- **Documents** (this commit): the `NOT_RUN` reason of `github_known`, the matrix row and counts, and in this part of the report the parked list, sections 3, 5 and 6 (the open user-visible minors with their cost, the `Mark as Viewed` bullet, the native submenu) and the verification of section 7.

### Conclusion

What can be tested without a screen, a keyboard, the real Keychain or the real GitHub is tested, with real git, local bare remotes and a fake GitHub that records every request, and follows the 241 sources except for the differences in sections 2 and 3. Part 5 is not validated. It ends when you complete the list in section 5, with a real account and its Keychain item, a pull request from a fork and its checkout, and the protected rules after a real Fetch as its main risks. The final review and its fix wave are done; what they left parked is at the end of section 2.

## Part 6: signing, worktrees and submodules

Date: 2026-11-10. Plan: `docs/superpowers/plans/2026-11-09-phase-4-6-signing-submodules-worktrees.md`, with decisions 19 and 20 of the overview. The part has 11 commits after its plan (`aa48d58`): `aa48d58..56480b9`, 5 features and 6 fixes, in 6 tasks, each reviewed (Task 5 in three fix rounds), plus this report. The final review of the whole part (`aa48d58..47ff5da`, read only) found nothing critical or important of its own and six minors; one fix wave took its two queued must-fix items and five of the other findings (`Final fix wave` below), and what stays parked is at the end of section 2. The consumption measurement (the plan's Task 8) ran with the user's go-ahead on 2026-11-10, but its gate never opened and nothing was measured (`Consumption` in the phase summary below).

### Summary

- Part 6 is done in essence: the last pieces of IntelliJ 2024.1's Git support that the plan lists. Signing follows 241's model: `Configure GPG Key…` in the settings writes `commit.gpgSign` and `user.signingkey` to the repository's git config, and git signs by itself; Ramify's app-wide `Sign commits` setting and its `-S` are gone. The log has the `GPG Signature` column, hidden by default, and the details show the signature of the selected commits. A linked worktree or a submodule opens as a repository and sees what a terminal changes in it. Update Project updates submodules in detached HEAD afterwards, under a no-lost-work rule that is stricter than 241's. Submodule diffs show the commit hashes with 241's titles. Part 3's parked P7 (the watcher assumed `.git` is a folder) is closed.
- **`done` means tests, not use**, as in parts 1 to 5. The Rust logic is tested with real git, throwaway repositories, linked worktrees made with `git worktree add` and submodules from local bare repositories (`protocol.file.allow=always` set only in the tests); signatures are real SSH signatures made with `ssh-keygen` (`TempRepo::sign_commits`), and the key list comes from fake `gpg` scripts; the interface in jsdom with the Tauri API simulated. No real gpg, keyring or GPG signature was used, nothing was run in the real app, and nobody has seen the dialog, the column or a submodule diff in a real window. The part cannot be accepted without the list in section 5.
- Matrix: two rows of section 2 were renamed, as overview decisions 19 and 20 say, and got their result; the phase 1 row `Commit signing with GPG` of section 3 went from `partial` to `done`; two rows were added to section 2 (the `GPG Signature` column and submodule diffs) and one to section 3 (`Configure GPG Key…`). All six are `done`. The `Vcs.UpdateProject` and `Git.Pull` rows of section 4 got a part 6 note on the submodule update.
- The part's ledger records 16 rulings; section 2 lists them with their cost, and then the findings still parked after the final review and its fix wave. The two that had to be fixed before the merge are fixed: the per-submodule fetch refuses the transports git refuses for submodules, and a diff window always opens. The differences from IntelliJ that a user can notice are in section 3, by feature.
- Between part 5 and this plan, three commits (`c8b2458`, `c32f37c`, `996b302`) carried out the decision part 5 left to the user: an external CI link opens only after `Open External Link` shows its full address. Part 5's sections 2 and 3 describe it.

### 1. What was built

- **Worktrees and submodules watched as repositories (Task 1).** `git::git_dirs` asks git for the repository's own git folder and its common folder (`rev-parse --absolute-git-dir` and `--git-common-dir`, two separate calls). `watch.rs` has `WatchedPaths` (the work tree, both folders and the worktrees nested in the work tree, read from `git worktree list --porcelain`); `watch_roots` watches the work tree and each folder that is not already inside a watched one, so a normal clone is still one watch, a linked worktree two and a submodule two. `relevant` keeps the own git folder (without `objects/`, `*.lock` and other worktrees), and in a linked worktree only the shared entries of the common folder; `touches_stash` reads the stash in the common folder. `TempRepo` gained `worktree`, `with_submodule` and `in_git_dir`.
- **gpg and the signing config (Task 2).** A new `gpg.rs`: `program` (`gpg.program`, read with `--path`, then `gpg` on `PATH`, then `/opt/homebrew/bin`, `/usr/local/bin` and `/usr/local/MacGPG2/bin`), `secret_keys` (241's `--list-secret-keys --with-colons --fixed-list-mode --batch --no-tty`, no stdin, no shell, parsed and unescaped as `GitGpgConfigUtils` does), `config` and `write` (local `git config`, through the Console). `git::command` appends the three folders to the `PATH` of every git child that lacks them. `commit.rs` lost `sign`, `signing_key` and the `-S`, and `settings.rs` lost `sign_commits` and `signing_key`. Three new commands: `gpg_config`, `gpg_keys` (reads) and `gpg_config_set` (a write task titled `Configure GPG Key`).
- **`Configure GPG Key…` (Task 3).** `GpgKeyRow.svelte`, the button and its status label at the end of the second Commit section of the settings, refreshed on `repo-changed`; `GpgKeyDialog.svelte`, with the checkbox, a key combo that greys the user ID, the spinner, `Cannot find suitable private key` and `Synced with gitconfig`. The dialog is not part of the settings' Save: `OK` writes at once.
- **The `GPG Signature` column and the details (Task 4).** `log/signatures.rs` reads `%G?`, `%GS` and `%GF` for given hashes only, with hashes checked and sent on stdin, keeps an app-wide cache of 1,000 found signatures that forgets them 60 s after their last read, and runs each load as a read task that cancels the same requester's previous one. The command `log_signatures(hashes, requester)`. In the interface, `src/log/signatures.ts` (241's texts, the loader and the column's debounce), the column in both `Columns` menus, the badge drawn on the canvas, the signature line in `DetailsPanel.svelte`, and two icons of Ramify's own.
- **The submodule update after Update Project (Task 5).** A new `submodules.rs` reads `git submodule status --recursive` before and after the repository's own update, decides which submodules may move (section 2), fetches the commits they miss, and runs `git --literal-pathspecs submodule update --no-fetch --recursive -- <paths>`. It runs inside the Update task and its write lock, after the repository's own changes are restored. `UpdateResult` has `submodule_error`, and `src/update/flow.ts` shows it as the notification `Submodule update failed` with git's output as plain text. `git submodule status` is a quiet read in the Console.
- **Submodule diffs (Task 6).** `diff.rs` shows a gitlink as its bare hash and reads a submodule's checked-out HEAD (`submodule_head`, empty when the folder has no `.git`, since git in an empty folder answers with the parent's HEAD); `worktree_diff.rs` uses it for the local side; `compare.rs` gives the window its `{0} (Submodule)` title; `src/diff/sides.ts` and `FilePane.svelte` give the sides their titles.
- Part 6 adds 4 commands; `mutating_without_saving_the_diff_is_not_allowed` now counts 137, with 55 reads and 6 commands that write only app data. `gpg_config` and `gpg_keys` are in `NOT_RUN` of `scripts/smoke_ipc.py`; `src/perf/smoke.ts` runs `gpg_config_set(null)` (which only writes `commit.gpgSign false`) and `log_signatures`. `NOTICE` lists the 241 sources read for each file.

### 2. Decisions and rulings that depart from IntelliJ

Each one with what it costs if it turns out to be wrong. In this part, "plan decision N" is one of the ten below, from `docs/superpowers/plans/2026-11-09-phase-4-6-signing-submodules-worktrees.md`, and "overview decision N" one of `docs/superpowers/plans/2026-11-03-phase-4-overview.md`.

**Decisions of the overview that this part carries (19 and 20)**

19. **Worktrees and submodules as 241 has them: no management window.** A linked worktree or a submodule opens as a normal repository, with the watcher on its real git folder and on the common one; Update Project runs `git submodule update --recursive` when submodules are in detached HEAD, as `GitSubmoduleUpdater` does. Cost: `git worktree add` stays in the terminal.
20. **SSH signing through git, as in 241.** 241 only configures GPG keys; with `gpg.format=ssh` git signs by itself and the `GPG Signature` column shows what git reports. Cost: no interface to choose an SSH key.

**Decisions of the plan (1 to 10)**

1. **Signing moves to 241's model:** `Configure GPG Key…` writes `commit.gpgSign` and `user.signingkey` to the repository's local config, and Ramify's app-wide `Sign commits` checkbox, its `Signing key` field and the `-S<key>` are removed. Cost: anyone who relied on the app-wide toggle has to set it per repository (and the change is silent; see the rulings).
2. **`Configure GPG Key…` goes in the settings,** in the commit section, where 241 has it, with 241's status label; only the single-root dialog, titled `Configure GPG Key` without the trailing space that 241's empty `mention` leaves. Cost: no multi-root table.
3. **Finding gpg:** `gpg.program`, else `gpg` on `PATH`, else the three folders; and every git Ramify starts gets those folders at the end of its `PATH` when they are missing, because an app opened from Finder has `/usr/bin:/bin:/usr/sbin:/sbin`. Cost: a gpg installed elsewhere needs `gpg.program`.
4. **SSH signing keeps working, and 241's trap is closed:** with `gpg.format=ssh` the configured key is shown as it is, and the dialog refuses to write a GPG key ID over it with Ramify's text `Commits are signed with SSH (gpg.format=ssh). Change gpg.format to use a GPG key.`; unchecking still writes `commit.gpgSign false`. Cost: one Ramify text.
5. **No `See GPG setup guide` link,** which opens a JetBrains help topic; a failed signature keeps Ramify's `Could not sign the commit: …` with git's output. Cost: no guide link.
6. **The signature icons are Ramify's own drawings:** a green check badge for verified, an amber badge for the other signed states, with 241's texts and states. Cost: a visual difference.
7. **The submodule update fits one repository per window:** 241 runs `GitSubmoduleUpdater` only when the submodule and its parent are both registered roots, which never happens in Ramify; Ramify decides from the open repository, after an Update Project that ends `Success` or `NothingToUpdate`, in the same task and lock, after the repository's own changes are restored. Ramify does not save the submodules' local changes first, as 241 does: git refuses to overwrite them, and Ramify shows the error as the notification `Submodule update failed` (Ramify's own title), where 241 only logs it and then shows a misleading `Local changes were not restored`. Cost: a submodule with local changes is not updated until they are committed or stashed in it.
8. **Every gitlink is shown as a submodule** (241: only registered roots), with the commit hash as its contents, the checked-out HEAD as the local side and 241's titles. Cost: none.
9. **The watcher watches the git folder and the common folder recursively** when they are outside the work tree, with the `objects/` and `*.lock` filters. Cost: none expected; the consumption measurement would check it, and it was not run.
10. **The consumption measurement waits for the user** (the phase summary below). Cost: the phase summary waits for that number.

**Rulings taken during the part**

The order of the work:
- Task 2 also removed the old `Sign commits` and `Signing key` fields from `SettingsView.svelte`, which the plan had put in Task 3, so that the type check stayed green between the tasks. Cost if wrong: none, the work moved one task earlier.

Worktrees (Task 1):
- **A broken worktree `gitdir` keeps Ramify's text** `<path> is not a Git repository`, instead of git's `fatal: not a git repository: (null)`: the mapping is shared by every folder and is clearer. Cost: the text differs from git's own.
- **Ramify refreshes only on what concerns the open checkout.** In every repository it ignores `<common>/worktrees/**` except its own git folder; in a linked worktree it keeps only the shared entries of the common folder (`config`, `packed-refs`, `shallow`, `refs/**` without `refs/bisect`, `refs/worktree` and `refs/rewritten`, `logs/**` without `logs/HEAD`, `info/**`) and ignores `<common>/modules/**`; a normal clone and a submodule keep their own `modules/**`. Other worktrees may be in use by another tool at the same time. Cost: no refresh on other worktrees' or the main checkout's `HEAD`, index and locks (their commits still show through the shared refs).
- **A worktree nested in the work tree** (for example `.worktrees/x`) no longer refreshes the main checkout on its file edits; the `objects` and `*.lock` filters apply at any depth (an `objects` component after `refs` or `logs` still counts). Cost: one more git call (`worktree list`) per open; a submodule named with an `objects` component loses its events in the parent (accepted as rare).

Signing (Tasks 2 and 3):
- **With `gpg.format=ssh`, only a key that differs from the configured `user.signingkey` is refused**, so `OK` with nothing changed, or turning signing back on with the same SSH key, works. Cost: none.
- **The silent unsigned path, recorded here as the plan asked.** A user who had Ramify's `signCommits: true` and no `commit.gpgSign` in git now makes unsigned commits with no notice, and the next settings save drops `signCommits` and `signingKey`, so the old key text is lost too. This is decision 1's accepted cost: the user does not sign today, as their IntelliJ settings and global git config show. Cost: a one-time surprise for such a user, who sets `Configure GPG Key…` in each repository that should sign.
- **Checking the box with no configured key preselects the first listed key,** instead of what 241 probably does (writing `commit.gpgSign false` after the user checked the box). 241's case is inferred from a JDK combo default and was not verified, and writing `false` contradicts the click. Cost: a small departure.

Signatures (Task 4):
- **`log_signatures(hashes, requester)`: one cancellation slot per loader** (the column of each table, each details panel), as 241 has one loader per table and per panel; with a single slot, selecting a commit would cancel the column's load. Cost: one small entry per loader for the session, never pruned (parked).

The submodule update (Task 5), from the first ruling to the invariant that replaced the case patches:
- **`Pull…` with `--rebase` also updates submodules,** because Ramify sends it through `update_project`, as it does since phase 2, while 241's `Pull…` runs `git pull` and never runs `GitSubmoduleUpdater`. Cost: a written difference; a rebase `Pull…` may detach submodules that 241's would leave. A `Pull…` that merges does not update them, as in 241.
- **Only a submodule that was at its recorded commit is moved.** `git submodule status --recursive` is read before the repository's own update; afterwards a submodule is updated, with its path after `--`, only when it was detached at its recorded commit (state ` `), is still detached with the same HEAD, and its recorded commit moved; one with a moved submodule inside is skipped too, since `--recursive` would move that one. A submodule left alone gets a Console line of Ramify's own: `Submodule <path> was not updated: its HEAD is not at the commit the repository records.` The user's no-lost-work rule beats 241 here: 241 checks out over a detached HEAD whose own commits then live only in the reflog. Cost: **a submodule the user moved is never updated by Ramify;** it has to be updated by hand.
- **No `.gitmodules`, no step;** a failing `git submodule status` (an orphan gitlink) is logged quietly, and only a failing `git submodule update` is shown. Cost: none.
- **The invariant.** Right before `git submodule update`, every submodule it would touch, at any depth, must be safe: its HEAD is contained in one of its own branches, tags or remote branches (`for-each-ref --contains HEAD refs/heads refs/tags refs/remotes`; the stash does not count, since a `pop` drops it), or it equals the commit its parent recorded before the update. Otherwise the submodule is skipped with the same Console line. Cost: **a submodule whose HEAD only the reflog holds is never updated by Ramify.**
- **The fetch window is closed.** Each qualifying submodule, and each one nested in it, is fetched first when it lacks the commit to check out (`git -C <path> fetch --recurse-submodules=no`, with askpass and in the Console); the invariant is checked again; then `git submodule update --no-fetch` runs. Cost: one fetch per such submodule. Ramify does not repeat git's second attempt (a fetch of the hash itself): a commit the plain fetch does not bring fails with git's `Unable to checkout '<hash>' in submodule path '<path>'`.
- **A window of a few milliseconds remains** between the last check and `git submodule update --no-fetch`; for a nested submodule it also spans the outer one's checkout, still with no network. git cannot check again inside the command. Cost: a commit made in a submodule in those milliseconds could be checked out over.
- **Two known limits, accepted:** (1) a nested submodule that only the newly recorded commit adds is not checked, since the check reads the submodules of the current commit; if its folder and its `.git/modules` entry survived an earlier removal and the user committed on its detached HEAD, that commit can end up only in its reflog. (2) A staged gitlink to a commit that a remote branch contains comes back unstaged after the stash, passes the check, and is checked out to the recorded commit; the staged change disappears with no Console line, and the commit stays on that remote branch, so nothing is lost. Cost: rare setups.

Submodule diffs (Task 6):
- **The Local Changes window of a submodule is titled `<path> (Submodule)`.** Ramify's worktree diff windows are titled by the bare path for every file, and the log's already get 241's form (`lib (/x/libs) (Submodule)`); giving submodules alone `<name> (<parent folder>)` would make them differ from the other files of the same window. Cost: a cosmetic difference.

**Findings still parked after the final review and its fix wave**

None of these was found to block a task or the merge. From the ledger's `minor (deferred)` lines and the reviews:
- **Fixed by the final fix wave, no longer parked:** the per-submodule fetch without `GIT_PROTOCOL_FROM_USER=0` (Task 5, security), and the diff window that did not open when a revision's object was missing (Task 6); with them, the submodule fetch held inside the write lock (final review, Minor 1), a failing `git worktree list` that failed the open and left a reopened window unwatched (Task 1, final review Minor 2), the dialog's inert `OK` after a failed config read, its double error after a failed key list and the missing preselect test (Task 3), and the stale wording at `update.rs:666-668` (Task 5). `Final fix wave` below has each one with its test.
- **From the final review:** the signature output is split by line count (`%H%n%G?%n%GS%n%GF`), so a signer name with a newline would drop the signatures of the commits after it in that batch, never spoof one; the signing config is written `gpgSign` first, as 241 does, so a failed `user.signingkey` write leaves signing on with the previous key; the key list is keyed by key ID, which a key listed twice would break; the settings row lists the keys again on every `repo-changed`, with no bound on a hanging gpg.
- **Worktrees (Task 1):** a worktree added after the open is not filtered until the repository is opened again; the nested worktree's root folder itself is excluded; the real-watcher tests depend on timing; the relative and space `gitdir:` is tested only at the `git_dirs` level, and no stash test runs in a submodule; a double `canonicalize` in `start`. Out of scope: a terminal `git worktree add` or `remove` no longer refreshes a normal clone on its own.
- **gpg (Task 2):** nothing bounds a hanging gpg (241 has no bound either); the fake gpg's check that it got no stdin would also pass with an inherited empty pipe.
- **The dialog (Task 3):** the key combo has no keyboard navigation; while the settings are open the row refreshes on every `repo-changed`, which can start gpg, and the row and the dialog each list the keys.
- **Signatures (Task 4):** a commit that becomes unsigned (an allowed-signers file removed) keeps its badge in that table until the entry is evicted; the backend's per-requester map is never pruned, and the requester ids are per webview, so a second webview would collide; the column's tooltip is not cleared on a wheel scroll; the details' tooltip opens on hover only, is not tied to the icon for screen readers and may clip a long fingerprint; the `showBranches` prop also decides whether signatures load; test gaps (a burst of scrolls, the order against a real committer line, the unknown-letter log); two code paths reach git; the extra prefetch of the ±15 rows is not debounced.
- **The submodule update (Task 5):** it also runs when the repository's own restore ended in conflicts; a cancel during it shows `Submodule update failed` with `The operation was cancelled`; no test with credentials, and none with one detached submodule beside one on a branch; submodules with `update=none` are fetched although git then skips them; the comment on the update command does not state the `--no-fetch` and direct-hash differences. Out of scope: Ramify passes no `--checkout`, so a `submodule.<name>.update` of `merge`, `rebase` or `!command` in the repository's config decides what git does, as with git itself, and `submodule.recurse=true` is git's own behaviour (the same in 241).
- **Submodule diffs (Task 6):** `gitlink_at` and `head_hash` return git's hashes without `check_hash` (they come from git's output, not from the interface); pre-existing, `read_revision` fails with `bad file` when a gitlink's commit also exists in the parent repository's objects (for example a folder that became a submodule); `src/compare/actions.ts` needed no change.

### 3. Differences from IntelliJ that a user could notice, by feature

From the task reports, their reviews and the rulings. Each one is accepted, with its reason and its cost; the parked ones of section 2 are marked "parked".

**Signing and `Configure GPG Key…`**
- **A user of Ramify's old `Sign commits` gets unsigned commits with no notice** (ruling of Task 2): the app-wide setting is gone, and git signs only where the repository's config says so. Why: 241's model (plan decision 1). Cost: a one-time surprise; that user sets `Configure GPG Key…` in each repository that should sign.
- **With `gpg.format=ssh`, a different GPG key is refused** with `Commits are signed with SSH (gpg.format=ssh). Change gpg.format to use a GPG key.` (plan decision 4); 241 writes it and breaks every commit.
- **Checking the box with no configured key preselects the first key** (ruling of Task 3).
- No `See GPG setup guide` link (plan decision 5) and no multi-root dialog (plan decision 2).
- **When no gpg is found** the key list says `Cannot run program "gpg": No such file or directory (os error 2)`: the shape of 241's Java message, with Rust's wording for the system error. A gpg outside `PATH` and the three folders needs `gpg.program` (plan decision 3).
- **Every git that Ramify starts has the three gpg folders at the end of its `PATH`** when they were missing, and so do the hooks and programs git starts. 241 inherits the login shell's environment instead.
- **`commit.gpgSign` follows git's own boolean rules:** `[commit] gpgSign` written without a value reads as signing (241 reads the raw text and says it does not sign), and an invalid value is git's error. An empty `user.signingkey` reads as no key. A key that starts with `-` is refused as `The GPG key '-x' is not valid`.
- The config writes show in the Console, as in 241.
- **A config that cannot be read** shows its error in the dialog, which starts unchecked, and `OK` writes what the user then chooses (final fix wave). Cost: with nothing changed, that `OK` writes `commit.gpgSign=false`, which turns off signing that was on; Cancel writes nothing. **A key list that fails** shows only its error, not also `Cannot find suitable private key`.
- **Parked (Task 3):** the key combo cannot be used from the keyboard; a gpg that hangs leaves the dialog's spinner turning, as in 241.

**The `GPG Signature` column and the details**
- The icons are Ramify's own drawings (plan decision 6); the column's hover says `Verified GPG signature`, `Unverified GPG signature`, `Bad GPG signature` or `No GPG signature`.
- **The details' tooltip** with the fingerprint and the signer opens on hover only and may clip a long fingerprint in a narrow pane (parked). 241 shows it as the icon's tooltip too.
- Signatures show in the log's own details panels: the Log tabs, File History and Compare Branches; the push, interactive rebase and merge details dialogs show none, as in 241. Compare Branches was not checked against 241.
- A commit that becomes unsigned keeps its badge in that table for a while, and the column's tooltip stays on a wheel scroll until the mouse moves (parked).
- A failed or cancelled load is dropped silently; 241 logs `Failed to load commit signatures` at info level. An unknown `%G?` letter shows nothing and writes one line to stderr.

**Linked worktrees and submodules opened as repositories**
- No window to manage worktrees or submodules (overview decision 19), as in 241.
- **A worktree whose main repository moved** fails to open with `<path> is not a Git repository`, not git's `fatal: not a git repository: (null)` (ruling).
- **Ramify does not refresh** when another worktree's or the main checkout's `HEAD`, index or lock files change, nor on file edits in a worktree nested in the work tree (rulings); their commits still show through the shared refs. 241 matches exact files and also ignores them. A submodule named with an `objects` component loses its events in the parent.
- A terminal `git worktree add` or `remove` no longer refreshes a normal clone on its own; a worktree added after the open is not filtered until the repository is opened again (parked). If `git worktree list` fails, the nested worktrees are not filtered and stderr gets one line, but the repository opens and is watched, also after a Reopen (final fix wave).
- Snapshots, the shelf and the repository locks are kept per work tree, while the stash is shared by all the worktrees of a repository, as git keeps it.
- Checking out a branch used by another worktree ends in `Could not checkout <branch>` with git's text, as in 241.

**The submodule update after Update Project**
- **A submodule the user moved off its recorded commit is never updated by Ramify,** nor one with such a submodule inside (ruling of Task 5). The Console says `Submodule <path> was not updated: its HEAD is not at the commit the repository records.` on every Update while it stays so, also when nothing else was updated. 241 checks it out. Cost: the user updates it by hand.
- **A submodule whose HEAD only the reflog holds is never updated by Ramify** (the invariant); a stash does not count as keeping it. 241 checks out over it, and its own commits then live only in the reflog.
- **A window of a few milliseconds remains** between the check and the update, and **two rare setups are accepted known limits** (section 2).
- **`Pull…` with rebase also updates submodules;** 241's `Pull…` never does (ruling). A merge `Pull…` does not, as in 241 (tests `a_rebase_pull_updates_the_submodules` and `a_merge_pull_does_not_update_the_submodules`).
- **A failure is the notification `Submodule update failed`** with git's output, and the Update result and summary stay as they were; 241 logs it, and when it had saved local changes before the update, it leaves them saved and shows `Local changes were not restored`. A submodule with local changes is not updated (git refuses), and nothing is saved for it first (plan decision 7).
- Before the update, each submodule that lacks its commit is fetched once, and that fetch shows in the Console with its own credentials prompt; a commit the plain fetch does not bring fails with git's `Unable to checkout '<hash>' in submodule path '<path>'`, where git would also try a fetch of the hash. That fetch runs as git's own submodule fetch does, with `GIT_PROTOCOL_FROM_USER=0`: a local-path submodule URL fails with git's `transport 'file' not allowed` unless the submodule's config allows it (final fix wave). It runs without the write lock, so a commit or a diff-window save made meanwhile does not wait for the network or a credentials prompt; the check runs again under the lock, right before the update (final fix wave).
- Inside a submodule that is updated, `--recursive` also detaches a nested submodule that sat on a branch, as in 241; its commits stay on the branch.
- The Update's counts do not include the submodules' files (241 adds them); `git submodule status` does not show in the Console.
- **Parked:** a cancel shows `Submodule update failed` with `The operation was cancelled`; the step also runs after the repository's restore ended in conflicts; `update=none` submodules are fetched.

**Submodule diffs**
- Every gitlink is shown as a submodule, also when 241 would not know it as a root (plan decision 8).
- **The Local Changes window of a submodule is titled `<path> (Submodule)`,** where 241 has `<name> (<parent folder>) (Submodule)`; the log's windows have 241's form (ruling of Task 6).
- **A revision whose object is missing** (a commit that was garbage-collected) opens the window with the plain title, and the window shows git's error instead of the sides (final fix wave). It is never shown as an absent side: against the local file, reverting that one inserted hunk would empty the file.
- **Parked:** `bad file` when a gitlink's commit is also in the parent repository's objects (pre-existing).

### 4. What was proven with tests

Each feature and the tests that name it; the full lists are in the Result column of `docs/parity-matrix.md` (sections 2 and 3).

- **Worktree and submodule layouts (Review Focus 1):** `the_git_folders_of_a_normal_clone_are_both_its_dot_git`, `the_git_folders_of_a_linked_worktree_are_its_own_and_the_common_one` (absolute and relative `gitdir:`, folders with spaces), `the_git_folders_of_a_submodule_are_both_its_modules_folder`, `the_git_folders_of_a_submodule_inside_a_worktree_are_in_the_worktrees_folder` and `a_worktree_with_a_broken_gitdir_fails_to_open_as_not_a_repository` (no panic); with a real watcher, `a_linked_worktree_sees_index_branch_and_stash_changes_from_outside`, `a_submodule_sees_its_own_index_and_branch_changes`, `a_submodule_inside_a_worktree_is_watched_on_its_own_git_dir`, `activity_in_the_other_checkouts_does_not_refresh_a_linked_worktree`, `a_normal_clone_ignores_activity_in_its_linked_worktrees` and `a_worktree_inside_the_work_tree_does_not_refresh_the_main_checkout`; and `a_linked_worktree_ignores_the_private_files_of_the_other_checkouts` (20 private paths ignored, 13 shared ones kept).
- **gpg in every state (Review Focus 2):** `a_failing_gpg_gives_an_empty_list_and_a_missing_one_an_error` (failing, failing after listing, missing), `an_empty_key_list_says_cannot_find_suitable_private_key`, `the_key_list_keeps_signing_keys_and_their_first_uid` (several keys, a disabled one, an encrypt-only one, `\x3c`, `\x3e` and `\x3a` escapes, the exact argv and no stdin), `user_ids_are_unescaped_as_241_does`, `a_user_id_with_markup_is_text`, `with_gpg_format_ssh_writing_a_gpg_key_is_refused_and_nothing_changes` (`.git/config` byte for byte), `with_gpg_format_ssh_the_configured_key_can_be_kept_and_turned_back_on` and `with_ssh_the_dialog_shows_the_key_and_ok_with_a_gpg_key_shows_ramifys_refusal`. The signing itself: `a_commit_is_signed_by_git_from_the_config_without_dash_s` (a real SSH signature from the config, none with it off, and a recording gpg that is never called), `signing_comes_from_the_config_and_its_key` and `git_children_get_the_gpg_folders_on_their_path`.
- **Signatures on a large history (Review Focus 3):** `signatures_load_only_while_shown_for_the_visible_rows_plus_fifteen_and_a_new_request_cancels_the_old` (nothing while hidden, rows 0 to 24 after the debounce, rows 25 to 64 after a scroll, a late answer dropped, hiding cancels), `the_all_commits_log_format_has_no_signature_placeholder`, `the_details_ask_for_at_most_the_shown_commits_and_the_dialogs_for_none`, `unsigned_and_unknown_letters_give_no_entry` (`N`, `P` and all seven letters), `a_new_request_cancels_the_previous_one_of_the_same_requester_only` and `ssh_signed_commits_read_as_241_with_and_without_allowed_signers`. The scroll of `git/git` itself was not measured.
- **A submodule update that cannot run cleanly (Review Focus 4):** local changes (`a_submodule_with_local_changes_keeps_them_and_the_error_reaches_the_notification`: the tracked change and an untracked file kept, HEAD unchanged, git's error in `submodule_error`), not initialized (`a_submodule_on_a_branch_or_not_initialized_is_left_alone`), nested (`a_nested_submodule_is_updated_by_the_recursive_call`, `a_nested_submodule_s_missing_commit_is_fetched_before_the_update`), a commit missing from its remote (`a_failed_submodule_update_does_not_change_the_update_outcome`), and no lost work: `a_detached_submodule_with_a_new_commit_on_its_head_keeps_it_and_is_not_updated`, `a_submodule_with_a_moved_one_inside_is_left_alone`, `a_staged_commit_on_a_detached_submodule_head_stays_after_the_stash`, `a_detached_head_that_only_a_stash_holds_is_not_moved`, `a_nested_submodule_that_gets_a_commit_during_the_merge_is_not_checked_out_over`, `a_commit_made_in_the_submodule_during_the_fetch_is_not_checked_out_over` and `a_commit_during_the_submodule_fetch_does_not_wait_for_it`, with `a_submodule_at_a_commit_its_remote_branch_contains_is_still_updated` for the other side of the check; `an_orphan_gitlink_without_gitmodules_is_not_an_error` and `an_orphan_gitlink_beside_a_submodule_is_not_an_error`; in the interface `git_s_output_is_shown_as_plain_text`; the transports git refuses for submodule URLs in `a_submodule_with_a_local_path_url_is_refused_by_the_fetch`. A URL that needs credentials is not tested.
- **A gitlink whose submodule is not checked out (Review Focus 5):** `a_submodule_that_is_not_checked_out_gives_an_empty_side` and `a_submodule_that_is_not_checked_out_gives_an_empty_local_side`, and a missing revision in `a_missing_revision_gives_the_plain_title`, `a_missing_revision_against_the_local_file_is_an_error_and_the_file_is_untouched` and `a_missing_revision_shows_git_s_error_and_nothing_to_revert`; the titles in `the_diff_title_ends_with_submodule_and_the_sides_are_full_hashes`, `the_window_title_of_a_submodule_ends_with_the_word` and the four interface tests of the matrix row.
- **The guard tests follow the new commands:** `mutating_without_saving_the_diff_is_not_allowed` (137 commands, 55 reads, 6 app-data writers) and `whatever_touches_the_repository_saves_the_diff_window_first`, which lists `gpg_config_set`.

**What the process left on record.** Task 3 wrote its tests with the components and has no red run; Task 6 wrote its Rust and interface tests with the code and ran no separate red step; Task 1's `git_dirs` came just before its tests. Tasks 1, 2, 4 and 5 showed red runs against stubs or the old code, Task 2 also a mutation check (an `-S` put back made its test fail), and Task 5 a moment's `{@html}` that made `git_s_output_is_shown_as_plain_text` fail. A few tests guard a removal or the normal case and could not fail before their fix; their reports say which. Task 5 needed three fix rounds after its review, each re-reviewed, before the invariant held. In Tasks 4 and 5 and in this report's verification the harness moved the full library run to the background after 180 s by itself; each time nothing else was started until it finished. `cargo fmt` rewrote unrelated files in Task 6; they were restored from a saved patch.

### 5. Checklist to verify by hand

None of this could be checked automatically. Use throwaway repositories first, then a real one. The `.app` was not rebuilt in this part; to build it and open it, the commands of part 1, section 5.

**The six of the plan**
- [ ] **A real GPG key:** in `Settings > Version Control > Git`, `Configure GPG Key…` lists your secret keys with their user IDs; choose one and press `OK`; the status label says `Commits are signed with GPG key: <uid> (<id>)`, and `git config --local --get user.signingkey` shows it. Quit Ramify, open it from Finder (not from a terminal), commit: gpg's pinentry asks for the passphrase unless gpg-agent has it cached, and `git log --show-signature -1` shows a good signature. In the log, show `Columns ▸ GPG Signature`: the commit has the green badge, and its details show the signature line with the fingerprint and the signer in the icon's tooltip.
- [ ] **A real SSH-signed repository with an allowed-signers file:** with `gpg.format=ssh`, the row shows the SSH key as it is; choosing a GPG key in the dialog shows the refusal; the column shows `Verified` for your commits once `gpg.ssh.allowedSignersFile` lists your key, and nothing without it.
- [ ] **A linked worktree and a submodule opened in Ramify, with terminal changes showing up:** open each; from a terminal, `git add`, `git commit`, `git checkout -b` and `git stash push` in it show in Local Changes, the branch widget and the Stash tab; work in the main checkout or in another worktree does not refresh it, but a commit there still shows in the log.
- [ ] **Update Project on a repository with a submodule in detached HEAD, including one with local changes:** a colleague's commit moves the gitlink; Update Project moves the submodule to the new commit, with the fetch and the update in the Console; with local changes in the submodule, the notification `Submodule update failed` shows git's text and the changes are still there; a submodule with a commit of yours on its detached HEAD is left alone with the Console line.
- [ ] **A submodule diff from Local Changes and from the log:** the window is titled `<path> (Submodule)` from Local Changes and `<name> (<parent folder>) (Submodule)` from the log; the sides are the full hashes (the parent's HEAD and `Your version` in Local Changes).
- [ ] **Scrolling the `git/git` log with the `GPG Signature` column shown:** it scrolls as with the column hidden, and signatures appear for the rows on screen (this is also the consumption measurement's second run).

**Collected during the tasks**
- [ ] With no gpg installed (or `gpg.program` pointing to a missing file), the dialog says `Cannot run program "gpg": …` and nothing hangs.
- [ ] In a narrow details pane, the tooltip of a verified signature shows the whole `SHA256:` or GPG fingerprint.
- [ ] A submodule whose URL needs credentials: the fetch before the update asks through Ramify's askpass, and a wrong answer ends in `Submodule update failed` with no credential in the text.
- [ ] `Pull…` with rebase on a repository with a detached submodule updates it; `Pull…` with merge does not.
- [ ] `scripts/smoke_ipc.py` against a built app: the `gpg_config_set` and `log_signatures` steps run, and `gpg_config` and `gpg_keys` are in `NOT_RUN`.

### 6. What could not be checked

- **No real gpg.** No real keyring, key list or GPG signature was used: the key list came from fake `gpg` scripts (the research could not start gpg-agent either, so the parsing follows 241's code and gnupg's documentation), and the `%G?` path of a real OpenPGP signature is tested only by parsing. Real SSH signatures were made and verified.
- **Nothing was run in the real app,** and nothing was seen in a real window: the settings row, the dialog, the column, the details line and the diff titles were tested in jsdom, and the watcher with real files but outside the app. No real IPC of the 4 new commands (`gpg_config`, `gpg_keys`, `gpg_config_set`, `log_signatures`); `scripts/smoke_ipc.py` was not run, and a failure would be one of registration or argument names.
- **No measurement.** The scroll of the `git/git` log with the column shown, the cost of the second recursive watch of a linked worktree and of the extra git call per open, and the `--contains` check on a large submodule were not measured; the consumption measurement's gate never opened (the phase summary below).
- **Not tested:** a submodule URL that needs credentials, a cancel during the submodule update, and a gpg that hangs.
- **Not compared with 241 running:** which key 241's dialog selects when none is configured; what 241's Update Project shows after a failed submodule update; 241 on `%G?` = `P`; submodule changes in 241 when the submodule is not a registered root; signatures in 241's Compare Branches. These come from reading the sources.
- **The real-watcher tests depend on timing** (parked); they passed in the run of section 7.

### 7. Verification

Run on 2026-11-10 on the tree of the re-review's fix (`fix: a missing revision is an error in the diff again, never an absent side`, the last commit of the final fix wave), before it was committed, one suite at a time in the foreground, under `nice -n 15`, with `CARGO_BUILD_JOBS=3` and the toolchain of `~/.cache/ramify-toolchain`; `pgrep -fl "vitest|cargo (test|clippy)|rustc"` was empty before each. The 1-minute load average was 3.8 when the library run started and 7.0 when it ended. The harness moved the library run to the background after 180 s by itself; nothing else ran until it finished.

| Command | Result |
|---|---|
| `pnpm exec tsc --noEmit -p .` | no errors (exit 0) |
| `pnpm exec vitest run --maxWorkers=2` | 130 files, 2,546 tests passed |
| `CARGO_BUILD_JOBS=3 cargo test --lib -- --test-threads=3` | 2,194 passed, 0 failed, 7 ignored (on purpose), 292.31 s |
| `CARGO_BUILD_JOBS=3 cargo test --no-fail-fast --test '*' -- --test-threads=3` | 5 files: `askpass` 11, `crlf_fix` 1, `editor` 41 and `git_path` 2 passed; `phase_3_trace` 0 passed and 1 ignored (on purpose); 55 passed, 0 failed, 1 ignored |
| `CARGO_BUILD_JOBS=3 cargo clippy --all-targets -- -D warnings` | no warnings (exit 0) |
| `python3 -I -m unittest discover -s scripts` | 65 tests, OK |

The 7 ignored library tests are the six of part 4 and `the_keychain_store_keeps_replaces_and_deletes_a_token`, as at the close of part 5. The timing test `on_quit_the_writer_is_cancelled_and_waited_for` passed. Against the run before the final review (on `56480b9`: 2,542 interface tests and 2,187 in Rust), the wave adds 4 interface tests and 7 Rust tests, and none in the integration tests or the scripts. `cargo fmt --check` was not part of the verification, as in parts 2 to 5.

The citations of the matrix were checked with a script over `src`, `src-tauri/src` and `src-tauri/tests`: the snake-case names in the rows that mention part 6 (its six rows, and the `Vcs.UpdateProject` and `Git.Pull` rows with all their earlier citations), 108 after the wave, are all test names that exist there (a Rust `#[test]` function or a Vitest `test`/`it`).

### Final fix wave

The final review of the part (range `aa48d58..47ff5da`, read only: nothing critical or important of its own, 6 minors) confirmed the two must-fix items the ledger had queued and added one rule finding (Minor 1, the submodule fetch inside the write lock); the ledger ruled one wave for them, three of the deferred minors and the Task 7 documents minors. Each code fix has a test that was run and seen failing before its fix (the red runs are in the wave's own report, outside the repository).

- **The submodule fetch** (`d5fd0c8`):
  - It runs with `GIT_PROTOCOL_FROM_USER=0`, as git's own submodule fetch does, so a submodule URL cannot use a transport that `protocol.allow` leaves to the user. A local-path URL fails with git's `transport 'file' not allowed` unless the submodule's config allows it (test `a_submodule_with_a_local_path_url_is_refused_by_the_fetch`). The tests' own submodules set `protocol.file.allow=always` in their config only.
  - It runs without the write lock, as `task.rs` asks, under a remote lock of its own per submodule. The Update keeps the lock for its own update, the restore and the first check, lets it go for the fetch, and takes it again for the second check and `submodule update --no-fetch`. A commit made while the fetch waits does not wait for it (test `a_commit_during_the_submodule_fetch_does_not_wait_for_it`), and the no-lost-work tests of Task 5 pass unchanged.
  - The stale `submodules::to_update` comment now names `decide`.
- **The diff window always opens** (`954ce58`): a side that cannot be read is not a submodule, so the title is the plain one (test `a_missing_revision_gives_the_plain_title`). A revision whose object is missing stays git's error, which the window shows instead of the sides; the wave first read it as an empty side, and the re-review's ruling took that back, since reverting its one inserted hunk against the local file would empty the file (tests `a_missing_revision_against_the_local_file_is_an_error_and_the_file_is_untouched` and `a_missing_revision_shows_git_s_error_and_nothing_to_revert`).
- **A failing `git worktree list`** (`5047d99`) only loses the filter of the nested worktrees, with one line on stderr: the repository opens and a Reopen watches it again (test `a_failing_worktree_list_watches_without_nested_worktrees`, with a fake git).
- **The `Configure GPG Key` dialog** (`5e43e66`): a config that cannot be read shows its error, the keys are still listed and `OK` writes what the user chose (`a_failed_config_read_shows_its_error_and_ok_still_writes_the_choice`); a key list that fails shows only its error (`a_failed_key_list_shows_only_its_error`); and the first-key preselect has its test (`checking_the_box_preselects_the_first_key`).
- **Pull and submodules** (`c0a690b`): `a_merge_pull_does_not_update_the_submodules` and, with the same setup, `a_rebase_pull_updates_the_submodules`. The matrix's `Git.Pull` row cites them, with `pull_rebase_goes_through_update` for the routing.
- **The re-review's ruling** (the commit `fix: a missing revision is an error in the diff again, never an absent side`) took back the empty side above and wrote the dialog's cost in section 3.
- **Documents** (`ee7513b`): the matrix rows of part 6, and in this part of the report the summary, the parked list, sections 3 and 4, this verification and the phase summary.

### Conclusion

What can be tested without a screen, a keyboard, a real gpg or the real app is tested, with real git, real worktrees and submodules from local bare repositories, and real SSH signatures, and follows the 241 sources except for the differences in sections 2 and 3. Part 6 is not validated. It ends when you complete the list in section 5, with a real GPG key opened from Finder, a real worktree and submodule, and Update Project with a detached submodule as its main risks. Its final review and fix wave are done; what they left parked is at the end of section 2.

## Multi-window part: several project windows

Date: 2026-11-11. Spec: `docs/superpowers/specs/2026-11-10-multi-window-design.md`; plan: `docs/superpowers/plans/2026-11-10-multi-window.md`. The part has 16 commits after its start (`04f6ae0`): `04f6ae0..897b180`, 12 features or refactors and 4 fixes, in 13 tasks, each reviewed, plus this report. It was built on the branch `multi-window`, after phase 4.

### Summary

- Ramify now runs several project windows in one process, as IntelliJ IDEA CE 2024.1 does: every window has its own repository, log, watcher, tasks, console, diff, merge, viewer and selection history windows, and its own askpass and editor prompts. One-window use is unchanged.
- Opening a project from Open…, Recent Projects, the project widget, a path handed to the running app or Clone asks `This Window` or `New Window` when the window already has a repository (`Open project in`, with `Don't ask again`); a repository already open elsewhere is focused instead of opened twice.
- Closing a window waits for its own writes and remote tasks only; quitting runs the leave flow in every window and one refusal stops it; the open projects are written continuously and reopened at the next plain start.
- The menu bar follows the focused window, the remote checks and the shelf cleaning serve every open repository, and the toolbar has the project widget.
- **`done` means tests, not use**, as in the earlier parts. Nothing was run as a real multi-window app by the agents; section 5 is for the user.
- The differences from IntelliJ that a user can notice are in section 3, each with its cost; the rulings are in section 2.

### 1. What was built

- **Window labels and owners (Task 1).** `windows.rs` maps a label (`main`, `main-2`, `diff-2`, `merge-2`, `viewer-2`, `selection-history-2`) to its owner and back, and `src/lib/windows.ts` has the frontend side (`listenOwn`, `emitToOwner`). Per-window events go out with `emit_to` per label, and every per-window listener passes its own label as target (Tauri delivers `emit_to` also to listeners registered with no target).
- **Per-window sessions (Tasks 2 and 3).** `AppState` keeps one `WindowSession` per owner (repository, log, watcher, diff, merge, viewer, selection); every vcs and hosting command acts on its own window's repository and takes `RepoLocks` and `flush_diff_window` for that window.
- **Events, tasks, console and prompts per window (Tasks 4 and 5).** `repo-changed`, tasks and console lines carry their owner; askpass and editor prompts are routed by token to the window that started the git process. `Runner::finish_writes_of(owner, limit)` waits for one owner's writes and remote tasks and refuses new ones.
- **Auxiliary windows per owner (Task 6).** Diff, merge, viewer and selection history windows are built, found and destroyed per owner; the block history slot is per owner.
- **Settings (Task 7).** `Open project in` (`New window`, `Current window`, `Ask`) and `Reopen projects on startup`, in the Settings view's `System` group.
- **This Window / New Window and focusing (Task 8).** `project_resolve`, `project_focus_open` and `project_new_window`; the open flow in `src/project/open.ts` and `OpenProjectDialog.svelte`; `RunEvent::Opened` for a path handed to the running app; claims for opens still loading.
- **Closing, quitting and reopening (Task 9).** Closing a project window runs the leave flow for it, saves its commit message draft to the changelist comment, waits for its tasks, destroys its auxiliary windows, drops its session and writes `open-projects.json`. Quit runs the flow in every window. Start reopens the listed projects, skipping and reporting missing folders, and focuses the last focused one once.
- **The menu bar (Task 10).** The Git menu state, Recent Projects and the actions follow the focused window, or the owner of a focused auxiliary window; when the owner changes, Rust asks the new owner's windows to send their menu state again.
- **Background loops (Task 11).** The remote check and the shelf cleaning serve every open repository whose window is visible.
- **The project widget (Task 12).** `ProjectWidget.svelte`, `projectIcon.ts` and the `projects_list` command: icon and name first in the toolbar, with the sections of `ProjectToolbarWidgetAction`.
- **Documentation and the release build (Task 13).** This part, `docs/reports/opening-projects.md`, two rows in section 2 of `docs/parity-matrix.md` and the README.

### 2. Rulings that depart from IntelliJ

Each one with what it costs if it turns out to be wrong.

1. **The open list is a snapshot, never written on quit.** Cost: after a refused quit, the held-back list is written at the next change, not at once.
2. **A closing window waits for every remote task, fetch included, up to 60 s, without cancelling it.** Why: no lost work, and a push waiting for a password must keep its prompt. Cost: a hung fetch holds that window's close for up to 60 s.
3. **A multi-remote fetch caught between two remotes when a close starts is cancelled** (a read in a remote turn is waited for, a new one is not started). A cancelled read loses no work. Cost: a fetch skips its remaining remotes on close.
4. **New remote tasks of a closing window are refused like writes.** Cost: none with one window.
5. **`connection_of` has no window parameter**: it is keyed by repository path. Cost: one parameter added later if that changes.
6. **Menu File > Open, Recent and Settings go to the last focused main window from Task 9**, ahead of the menu work of Task 10. Cost: none.
7. **Opening the repository that is already open in the calling window focuses that window and opens nothing again**, as IntelliJ's `findAndFocusExistingProjectForPath` does, since it matches the calling frame too. Its auxiliary windows and its commit message draft stay. Until the final review's fix wave it reopened the repository there without asking. Cost: none; a reload of the window's state needs another repository opened first.
8. **A stored initial target is dropped after the window's first open**, so a finished open stops counting as in progress. Cost: reloading a `main-<n>` page (development only) shows an empty window.
9. **A window that is minimized is unminimized when it is focused** (opening an open project), otherwise the user would see nothing happen. Cost: none.
10. **`Get from Version Control…` is in the widget** because 2024.1 has it in `ProjectWidget.Actions` (`JavaActions.xml`, lines 24 to 27 at the tag); it opens Clone. `New Project…` is not shown, since Ramify creates no projects.
11. **The project icon's colour is `ColorPalette.select`** (`abs(path.hashCode()) % 9`), not 2024.1's persisted rotating counter, because Ramify keeps no per-project store. Cost: the same project can have another colour than in IntelliJ.
12. **Only the recent section is capped at 100** (`MAX_RECENT_COUNT` caps open and recent together in IntelliJ). Cost: at most 100 rows plus the open ones; the stored list keeps 50, so in practice 50.
13. **Menu sync from a window that is not the menu owner is ignored**, and a change of owner makes the new owner resend its state. Cost: one extra event per focus change between projects.

**Parked after the reviews, none a correctness failure.** A failed or panicking close step can skip `resume_writes_of`; a late `Opened` event after the first project asked for its target opens the list and a folder; the reopen focus has no timeout; a `This Window` open of a closing window's repository focuses that closing window; the commit draft is saved only at close, so an old comment can come back after list round-trips; the pre-existing `log::signatures` cancel test failed once under a load average of about 15 and passed on rerun.

**Fixed in the final review's fix wave** (`4c91ce3^..` this report's commit): the commit message draft is saved again after the close's wait, and a closing window opens no merge window; Apply Patch on the fly asks only in the activated window; opening the repository a window already has only focuses it (ruling 7); a focused window watches again; New Window resolves its root in Rust; a second Quit while one runs does nothing, and quit skips windows that closed; a Fixup or Squash Into proposal at close keeps the user's earlier draft; a panicking task ends as Failed instead of holding its window's close (in test and development builds only: the release build has `panic = "abort"` in `src-tauri/Cargo.toml`, so there a panic ends the whole process, as before); `Git > Current File` never keeps the previous owner's entries.

### 3. Differences from IntelliJ that a user could notice

- **`open -n` still starts a second process.** There is no single-instance plugin. The two processes do not know each other, and the open list and recent list are shared files where the last writer wins. Cost: the same repository can be open in two processes.
- **No Welcome screen, no Attach and no window tabs.**
- **IntelliJ's recent and open projects are not imported.**
- **No feedback while a close waits.** IntelliJ shows `Closing project` progress. Cost: a window with a slow push seems to ignore the red button.
- **A closing window waits for remote tasks, fetch included, up to 60 s** (ruling 2), and a multi-remote fetch caught between remotes is cancelled (ruling 3).
- **Reloading a `main-<n>` page shows an empty window** (ruling 8; development only).
- **`Git > Current File` with a diff, merge or selection history window in front** has every entry off, also when that window belongs to the project that just took the menu. IntelliJ fills it from the focused frame's selection. Cost: Current File actions need the main window or the viewer in front.
- **The project icon's colour** can differ from IntelliJ's (ruling 11).
- **Recent projects:** capped at 100 apart from the open ones, stored at 50, and a project open in a window can still fall off the stored list at the 51st open (`docs/reports/opening-projects.md`).
- **The Dock's Quit and logout cannot be stopped.** They keep the open list as it was, which is already right because it is written continuously.
- **A start with a path argument** opens only that path; the earlier list is not kept for the next plain start beyond what the snapshot of that start holds.

### 4. What was proven with tests

- **Labels, owners and sessions (Review Focus 1, 3):** the label-to-owner tests of `windows.rs`, two sessions with two throwaway repositories keeping repository, tasks, console lines and `repo-changed` apart, and a write in window A flushing only A's diff window while B's unsaved text stays.
- **Per-window events:** the frontend helper tests in `src/lib/windows.test.ts` and `src/App.test.ts` (a `main-2` listener ignores events for `main`), and the emit target tests in Rust.
- **Closing (Review Focus 2):** a closing window's write is waited for, its reads are cancelled, its remote tasks (a push waiting for a password included) are waited for and no new ones start, while another window's tasks run on; concurrent closes keep the last window; a `repo_open` for a closing owner is refused; the diff window is flushed again before it is destroyed; the commit draft comes back in the Commit tool window.
- **Quitting (Review Focus 4):** the leave flow in every window in turn, a refusal in an unfocused window's merge window stopping the quit, and the `quitting` flag keeping the open list from being written.
- **The open list and reopening:** order, close removes, quit keeps, missing paths skipped and reported, the last focused one focused once.
- **Opening (Review Focus 3):** `src/project/open.test.ts` for each `Open project in` value, the question, `Don't ask again`, no question without a repository, a subfolder, a trailing slash or a symlink of a repository open elsewhere focusing that window, and an open that is still loading counting as open.
- **The menu (Review Focus 5):** the action going to the focused window's main window or viewer, never another project's; `menu_accepts` before and after the owner changes; the resync event.
- **Background loops:** the remote check and the shelf cleaning for every open repository.
- **The widget:** `ProjectWidget.test.ts` (name, tooltip, sections, focusing an open project, opening a recent one, text rendered as text) and `projectIcon.test.ts` (initials and colour against cases taken from the tag's algorithm).
- **The guard tests follow the new shapes:** the source-pinning tests (`flush_diff_window`, the first line of `repo_open`, the `pub async fn` count, the shelve guards, `owns_watcher`, `action_window`) were rewritten, not deleted.

**What the process left on record.** Several tasks wrote their Rust tests with the code and have no separate red run (Tasks 4, 10 and 11 most clearly); Task 5's fix was not followed by a full verification until Task 6.

### 5. Checklist to verify by hand

None of this could be checked automatically. Use throwaway repositories only, for example two folders made with `git init` under a temporary folder (`/tmp/ramify-multi/one` and `/tmp/ramify-multi/two`, each with a commit), a bare local remote for the push, and nothing from your real work. Build and open the app as in part 1, section 5.

- [ ] **Two windows with two repositories:** open `one`, then `two` with `New Window`; each window shows its own branch, changes and log, and a change made in `one` from a terminal shows only in `one`.
- [ ] **Cmd + backtick** cycles between the two project windows, and the title and focus follow.
- [ ] **The Dock:** with two windows, Dock > right click lists both in the `Window` menu; clicking the Dock icon with every window hidden or closed shows the last focused one; closing the last window hides it and the Dock brings it back with the repository still open.
- [ ] **This Window / New Window / Don't ask again:** with `one` open, `File > Open…` on `two` asks `Open project in`; `This Window` replaces `one`, `New Window` opens a second window, `Cancel` does nothing; `Don't ask again` with `New Window` makes the next open go straight to a new window, and the setting `Open project in` in the Settings view shows it and can be set back to `Ask`.
- [ ] **A repository already open elsewhere:** open `one` again by typing its path with a trailing slash, by a subfolder and by a symlink; the window that has `one` is focused (also when minimized) and nothing opens twice.
- [ ] **Closing with a running push:** start a push to the bare remote in window A that waits (for a password prompt, or a `pre-push` hook that sleeps); press the red button; the prompt shows in A, the window stays until the push ends, window B is not touched; there is no `Closing project` progress, as written in section 3.
- [ ] **A fetch while closing:** close a window while a fetch runs; it closes within the 60 s limit.
- [ ] **Quitting with a half-done merge in the unfocused window:** make a merge conflict in B's repository, open the merge window, focus A and press Cmd + Q; the question appears in B's merge window, refusing it stops the quit for every window, and accepting it quits.
- [ ] **Unsaved text in a diff window:** type in B's diff window and write in A; B's text stays. Close B with unsaved text; it is saved first.
- [ ] **Reopening after quit:** with two windows open, quit with Cmd + Q and start the app again with no path; both repositories open, the first in the initial window, and the last focused one ends focused. With `Reopen projects on startup` off, only an empty window opens. Rename one folder before the start: it is skipped, removed from the list and named in the notification.
- [ ] **Window frames:** open two throwaway projects in two windows, resize them and arrange them overlapped at different places (one maximized is a good variant); quit with Cmd + Q and start again; both come back where they were. Optional: save a frame on an external monitor, disconnect it and start again; the window opens at the default place, not off-screen.
- [ ] **The commit message draft** typed in a window comes back when that repository is opened again after the window closed.
- [ ] **The project widget popup:** it opens under the widget with the focus in the list, closes with Esc (also with the focus on the widget's button) and on a click outside, and shows `Open…`, `Get from Version Control…`, `Open Projects` and `Recent Projects` with the icon, the name and the path from `~`; choosing an open project focuses its window; choosing a recent one goes through the question.
- [ ] **A recent project whose folder is gone:** rename a recent throwaway folder, choose it in the widget popup; `Reopen Project` appears with `OK` / `Remove From List`; `Remove From List` takes it out of the widget and of `File > Recent Projects`.
- [ ] **`Get from Version Control…`** opens the Clone dialog.
- [ ] **The menu following focus:** with `one` in A and `two` in B, focus A and see its Git menu state; focus B and see it change; click another project's viewer window (and a merge window) and see the menu follow that project; with a project that has a repository in a state that disables an action (for example a merge in progress) the enabled state matches the focused project; a Git action from the menu in B goes to B, never A.
- [ ] **A path handed to the running app:** `open -a <the .app> /tmp/ramify-multi/two` while the app runs asks `Open project in` and opens or focuses `two`.
- [ ] **A second process:** `open -n` on the app starts a second process (known gap); confirm it does not corrupt the lists beyond the last writer winning.
- [ ] **Window titles and ⌘W:** ⌘W closes only the focused project window.

### 6. What could not be checked

- **No real multi-window run.** The agents made no run of the built app with several windows: real window creation, the Dock, Cmd + backtick, focus changes, the menu bar following focus (the native menu cannot be built without a running app), native `Opened` events for a path handed to the running app, the project widget's popup placement and focus, and reopening after a real quit were not seen. The tests simulate the Tauri API and use mock windows.
- **No measurement.** Memory, start-up time and CPU with several windows and several watchers were not measured. No figure is given.
- **Not compared with IntelliJ running:** the exact wording and timing of its close wait, the colour of an icon for a given project, and where it puts a new window.
- **Not tested:** a close wait that hits the 60 s limit in a real hung fetch, two Ramify processes running at once, and a failing `destroy()` of a window.
- **Load.** The tests run with 6 build jobs and 6 test threads; a timing-sensitive test failed once under a much higher load (ledger of Task 10 and 11) and passed on rerun.

### 7. Verification

Run on 2026-11-11 after the final review's fix wave, on the tree of `ad64e92` (the part's commits are `04f6ae0..` the commit of this report: the 16 of the part, the fixes and documentation that followed its first report, and the 8 commits of the fix wave, `4c91ce3^..ad64e92`, before this report's update), one suite at a time in the foreground, under `nice -n 15`, with `CARGO_BUILD_JOBS=6` and `--test-threads=6`.

| Command | Result |
|---|---|
| `pnpm exec tsc --noEmit -p .` | no errors (exit 0) |
| `pnpm vitest run --maxWorkers=2` | 135 files, 2,618 tests passed |
| `cargo clippy --all-targets -- -D warnings` | no warnings (exit 0) |
| `cargo test -- --test-threads=6` | library: 2,284 passed, 0 failed, 7 ignored (on purpose), 266.00 s; `askpass` 11, `crlf_fix` 1, `editor` 41 and `git_path` 2 passed; `phase_3_trace` 0 passed and 1 ignored (on purpose); `main.rs` and doc tests 0 tests; every `test result` line says `0 failed` |
| `python3 -I -m unittest discover -s scripts` | 65 tests, OK |

The 7 ignored library tests are the same as at the close of part 6.

**Release build.** Not run in this wave. The controller runs `pnpm tauri build --bundles app` after this fix wave and will update this result; until then no `Ramify.app` of this part exists, so section 5 cannot start.

### Conclusion

The multi-window part is done in tests and not validated by hand: it ends when section 5 is completed on a built app with throwaway repositories.

## Part: drag between changelists

Plan: `docs/superpowers/plans/2026-11-11-changes-drag-and-drop.md`. Branch `changes-drag-and-drop`.

### 1. What was built

In the Local Changes tree, which is also the tree of the non-modal Commit tool window, the selection is dragged onto a changelist, as 241's `ChangesViewDnDSupport`:

- A drag carries every file under the selected rows: files, folders, or a whole changelist. A label `1 file` / `N files` follows the pointer, above and to its left.
- The row under the pointer is walked up to its top-level row, so a drop on a file inside a list lands on that list. The target row gets a rectangle; where a drop is refused the pointer shows no-drop.
- Only a changelist accepts, and not when any dragged change is already in it; `Unversioned Files` and `Merge Conflicts` accept nothing.
- A drop first moves the changes (`changelists_move`, as `Move to Another Changelist...`), then adds the unversioned files to Git in that list (`changes_add`, as `Add to VCS`, which flushes the diff window, takes the repository lock and keeps a recoverable copy of the index). Nothing asks first, as in 241.
- A file split between lists (some hunks in another list) moves only the hunks of the list it was dragged from, with `changelists_move_hunks`, as 241's `moveChangesTo` with a `ChangeListChange`; the other lists keep theirs.
- The files are read again at the drop: a file removed during the drag is skipped, a file added meanwhile is only moved, and a list removed meanwhile runs nothing.
- Only a point on the tree is a target: releasing beside it or below it runs nothing, and the edge scroll runs only inside it. A move with no button held, or the window losing focus, ends the drag. A selection with no files, such as an empty list, starts no drag.

No Rust was changed.

### 2. Ramify's calls, with their cost

1. **Pointer events, not the platform's drag and drop.** Tauri's window-level file drop and WebKit's drag image make HTML5 drag and drop unreliable in the webview, and jsdom cannot test it. Cost: none for the user.
2. **A 5-point threshold** before a press becomes a drag (the AWT default). Cost: a drag that starts slightly earlier or later than in IntelliJ.
3. **A press on a row that is not selected, then a drag, drags that row alone**; a press on a selected row keeps the whole selection, and the click after a drop does not reselect. Only a plain press drags: with ⌘, ⇧ or ⌃ held the press only selects.
4. **Edge scrolling:** within one row of the top or bottom edge the tree scrolls half a row every 50 ms. Cost: a speed different from IntelliJ's.
5. **Out of scope:** the staging area's drag between Staged and Unstaged, dragging onto or from the Shelf, the `Ignored Files` node (Ramify does not show it) and the copy gesture (⌥), which in 241 only matters for shelves.
6. **Escape cancels** a drag, and releasing outside a list runs nothing.
7. **`Move to Another Changelist...` (⇧⌘M) still moves a split file whole**, with all its hunks, as before this part; only the drag moves just the dragged list's hunks. Cost: ⇧⌘M on a split file takes the hunks of the other lists with it, where 241 moves only the selected list's ranges.
8. **A conflicted file is dragged as belonging to no list,** so a drag that mixes it with files of other lists is not refused when it is dropped on the conflicted file's own list, where 241 refuses it. The extra move changes nothing. Recorded as a deferred minor.

### 3. Checklist to verify by hand

Use a throwaway repository with two changelists and a few unversioned files, not your real work.

- [ ] Drag a file from `Changes` onto another list's row: the row gets the rectangle while hovering, and the file moves there.
- [ ] Drop a file onto a file row inside another list: it lands on that list.
- [ ] Drag a list's header onto another list: every file moves, and the empty list stays.
- [ ] Select two files with ⌘ and drag one of them: both move, and the label says `2 files`.
- [ ] Drag `Unversioned Files` (or one unversioned file) onto a list that is not the active one: the files are added to Git and end up in that list.
- [ ] Hover the source list: no rectangle, the pointer shows no-drop, and releasing does nothing.
- [ ] Press Escape during a drag: the label goes away and nothing moves.
- [ ] Drag a file sideways onto the diff preview at the height of another list, and release: nothing moves.
- [ ] With a file split between two lists (move one hunk to another list from the diff), drag the file's own list onto a third list: only that list's hunks move; the hunk in the other list stays there.
- [ ] With a long `Unversioned Files` expanded, scroll down, then drag a file up to the top edge: the tree scrolls until the lists show.
- [ ] A plain click, a ⌘ click, a ⇧ click and a double click still select and open as before.

### 4. What could not be checked

- **No run of the built app.** The gesture was tested in jsdom with synthetic pointer events and a stubbed geometry. Not seen: the real cursor, the label's look, the scroll speed, and how WebKit treats a press on a row with a trackpad.
- **Not compared with IntelliJ running:** the look of its drag image and the exact rectangle.

### 5. Verification

Run on 2026-11-11, under `nice -n 15`, with `CARGO_BUILD_JOBS=6` and `--test-threads=6`. The frontend suite and `tsc` ran again on the final code (`e17b810`), after the final review's fixes; `clippy` and `cargo test` ran once, before those fixes, which changed no Rust.

| Command | Result |
|---|---|
| `pnpm exec tsc --noEmit -p .` | no errors (exit 0), on `e17b810` |
| `pnpm vitest run --maxWorkers=2` | 136 files, 2,661 tests passed, on `e17b810` |
| `cargo clippy --all-targets -- -D warnings` | no warnings (exit 0) |
| `cargo test -- --test-threads=6` | library: 2,297 passed, 0 failed, 7 ignored (on purpose), 353.69 s; `askpass` 11, `crlf_fix` 1, `editor` 41 and `git_path` 2 passed; `phase_3_trace` 0 passed and 1 ignored (on purpose); `main.rs` and doc tests 0 tests |
| `python3 -I -m unittest discover -s scripts` | 65 tests, OK |

**Final review.** One independent reviewer read the branch against the plan and 241's source. It found 2 Important and 5 Minor issues. Both Important ones (a drop beside the tree landed on a list; a split file moved whole) and four Minor ones were fixed, each with a test that failed first. The fifth Minor issue is decision 8.

### Conclusion

Dragging between changelists is done in tests and not validated by hand: it ends when section 3 is completed on a built app.

## Part: resizable log columns

Plan: `docs/superpowers/plans/2026-11-11-log-column-resize.md`. Branch `log-column-resize`.

### 1. What was built

In the Log's commit table, as 241's `VcsLogGraphTable` with its `InvisibleResizableHeader`:

- The table has no visible header. Hovering within 3 points of a border between columns, on any row, shows the `col-resize` cursor.
- Dragging that border changes Author, Date or Hash, and Subject takes the rest. A column left of Subject is resized by its right border, one right of it by its left border. The signature column is fixed.
- A double click on a border sends that column back to its automatic width.
- Each log tab keeps the widths it was given in its saved options (`columnWidths`, 241's `COLUMN_ID_WIDTH`). They come back when the repository opens again. A saved width wider than the table is ignored, and the column is automatic until the table is wide enough again.
- The graph and the labels follow Subject's new left edge while dragging.

No Rust was changed. Nothing is written to `.idea/` or to IntelliJ's settings.

### 2. Ramify's calls, with their cost

1. **Only the column next to the border changes.** Between Subject and a column, that column changes and Subject absorbs the difference. Between two resizable columns, the left one takes what the right one gives. Swing spreads a resize over every column to the right. Cost: a slightly different feel when Author and Date are side by side.
2. **Minimum width 15 points**, Swing's `TableColumn` default. Subject never goes under 15 points through a drag.
3. **The width is saved when the drag ends,** in one write. 241 saves during the drag. Cost: none visible.
4. **A press within 3 points of a border starts a resize and does not change the selection.** Cost: a click right on a border no longer selects that row.
5. **Each tab keeps its own widths, file history tabs included.** 241 shares one set among all file history tabs. Cost: a width set in one file history tab does not carry to the next one.
6. **Out of scope.** Reordering columns by dragging is not done: the class that would do it in 241 is not in our copy of the source, so it is unverified. A Compare Branches tab keeps its widths only while it is open, because its options are not saved, and so do the update and range tabs.
7. **Compare Branches' upper and lower logs share one set of widths.** 241 keeps the top and bottom logs apart. Cost: resizing a column in one half also resizes it in the other.
8. **Two saved widths that together are wider than the table still squeeze Subject to nothing,** as automatic widths already could. Swing would shrink the other columns instead. Cost: in a narrow window the subjects can disappear until a border is double-clicked.
9. **A double click on the border of a column that is already automatic** writes the options again without measuring the column anew; 241 measures it again from the data. Cost: none visible.

### 3. Checklist to verify by hand

Use a throwaway repository, not your real work.

- [ ] Hover between Subject and Author: the cursor becomes a column resize cursor. Drag left: Author gets wider and the subjects narrower.
- [ ] Drag the border between Author and Date: one gets wider, the other narrower.
- [ ] Double-click the Author border: it goes back to its automatic width.
- [ ] Quit and start again: the widths you set come back.
- [ ] Make the window narrow, so that a saved width is wider than the whole table: the column goes back to automatic, and Subject is not squeezed to nothing.
- [ ] Open a file history (Author, Date, Subject): its borders are on the right of Author and of Date, and its widths are its own.
- [ ] A click and a double click on a row away from a border still select and open as before, and keyboard navigation still works after a resize.

### 4. What could not be checked

- **No run of the built app.** The gesture was tested in jsdom with synthetic mouse events and a stubbed width (jsdom has no canvas and no layout). Not seen: the real cursor, the repaint while dragging, the feel of the 3-point zone on a trackpad.
- **Not compared with IntelliJ running:** the exact feel of Swing's resize, and whether its log lets columns be reordered by dragging.
- **A resize in a file history tab runs the diff preview's effect again** (it asks the backend for the change of the selection, not git), as the splitter drag already did. Not measured.

### 5. Verification

Run on 2026-11-11, under `nice -n 15`, with `CARGO_BUILD_JOBS=6` and `--test-threads=6`. The frontend suite and `tsc` ran again on the final code (`fb415d3`), after the final review's fixes; `clippy` and `cargo test` ran on `d99ba54`. The branch changes no Rust.

| Command | Result |
|---|---|
| `pnpm exec tsc --noEmit -p .` | no errors (exit 0), on `fb415d3` |
| `pnpm vitest run --maxWorkers=2` | 137 files, 2,692 tests passed, on `fb415d3` |
| `cargo clippy --all-targets -- -D warnings` | no warnings (exit 0) |
| `cargo test -- --test-threads=6` | library: 2,297 passed, 0 failed, 7 ignored (on purpose), 341.71 s; `askpass` 11, `crlf_fix` 1, `editor` 41 and `git_path` 2 passed; `phase_3_trace` 0 passed and 1 ignored (on purpose); `main.rs` and doc tests 0 tests |
| `python3 -I -m unittest discover -s scripts` | 65 tests, OK |

**Final review.** One independent reviewer read the branch against the plan and 241's source. It found 1 Important issue and 8 Minor ones.

- **Fixed, each with a test that failed first:** the Important one (in a file history, a double click on a border also opened the diff) and four Minor ones (a press on a border did not focus the table; a lost release kept the resize going; a narrow table snapped a column on the first pixel; two columns could grow past the table).
- **Covered by a test added after the review:** the click that ends a drag away from the border. That test passed already, as a guard.
- **Recorded, not fixed:** the other three, as decisions 7, 8 and 9. The re-run of the diff preview is listed in section 4.

### Conclusion

Resizable log columns are done in tests and not validated by hand: the part ends when section 3 is completed on a built app.

## Part: diff change stripe and branch sync icons

Plan: `docs/superpowers/plans/2026-11-12-diff-stripe-and-branch-sync.md`. Branch `diff-stripe-and-branch-sync`.

### 1. What was built

**Change stripe**, as 241's `DiffDrawUtil` stripe highlighters:

- Each diff editor has a thin column at its scrollbar edge, with one mark per changed block, in the block's colour: inserted, deleted or modified.
- The left editor's column is on its left edge, and the others' on their right edge, as 241 puts the left editor's scrollbar on the left.
- A side with no lines for a change still marks it at its place, one line tall, as 241 adds the stripe highlighter of an empty range. The unified view does the same for lines that are only in the other version.
- Hunks of another changelist get no mark: they have no background, like 241's excluded changes.
- With `Do not highlight` there are no marks, as 241 draws none without a background.
- A click on the column scrolls that editor to the line at that height. In side-by-side, the other editor follows through the existing scroll sync.
- Where: the two-side and unified views (the diff window and every diff preview) and the three-version views (HEAD, Staged, Local; and the three-way view of a merge's history preview). There a side pane marks only the changes it shows, and the centre pane has no marks, as 241's `withHideStripeMarkers(side == BASE)`.
- A click on any editor's column makes that editor lead the synchronised scrolling.

**Branch widget**, as 241's `GitToolbarWidgetAction`:

- After the current branch's name in the title bar: the incoming icon when it has commits to pull, and the outgoing icon when it has commits to push.
- None on a detached HEAD. A failed read shows nothing.
- They are read from the existing `branch_incoming_outgoing` on open and on `branch-sync-changed`. On `repo-changed`, which arrives for any file under the work tree, they are read again only when the refs changed (`branch_refs_fingerprint`, which includes HEAD), as the branches popup does. The `Check incoming commits on remotes` setting is honoured there.
- The status is kept with its branch, so after a checkout the old branch's icons never show next to the new name.

No Rust was changed.

### 2. Ramify's calls, with their cost

1. **The stripe is its own 8-point column beside the editor,** not part of its scrollbar. Ramify uses the system scrollbar, which macOS hides when idle, and a lane on top of it would block dragging it. Cost: each editor is 8 points narrower.
2. **A mark is at least 3 points tall.** The minimum is Ramify's.
3. **Marks follow document lines, not visual heights.** Cost: with unchanged lines folded or long lines wrapped, a mark sits where the lines are in the file, not where they show.
4. **A click puts the line at the top of the editor;** 241 centres it.
5. **Not yet:** the merge window, the texts diff (`Compare with Clipboard`) and the two-way view of a merge's history preview. Embedded diffs, which have no scrollbar of their own, have no stripe.
6. **The widget's icons refresh** on open, on `branch-sync-changed`, and on `repo-changed` only when the refs changed.
7. **In the unified view, a change whose lines are only in the other version is one line tall,** because Ramify's unified document does not contain those lines. 241's does. Cost: a long deletion looks like a one-line mark in the unified view.

### 3. Checklist to verify by hand

Use a throwaway repository, not your real work.

- [ ] Open the diff of a long file with a change near the end, side by side: a green, grey or blue mark shows at the right edge of the right editor, and the matching one at the left edge of the left editor.
- [ ] Click a mark: that editor scrolls to the change, and the other follows.
- [ ] Switch to the unified view: one column at the right, a mark for every change, a deletion included.
- [ ] Choose `Do not highlight`: the marks go away. Choose `Highlight words` again: they come back.
- [ ] Side by side, a file with an inserted line: the left editor has a short mark at the insertion point, as in IntelliJ.
- [ ] Open a file with staged and unstaged changes in the three-version view: HEAD's column is on its left, the Staged pane has no marks, and HEAD and Local mark only their own changes.
- [ ] Fetch a branch whose upstream has new commits: the incoming arrow shows next to the branch name in the title bar. Make a local commit: the outgoing arrow shows. Push: it goes away.
- [ ] Check out a commit (detached HEAD): no arrows.

### 4. What could not be checked

- **No run of the built app.** jsdom has no layout: the lane height was stubbed and the click geometry faked. Not seen: the real look of the column beside the editor, its colours on screen, and the flex layout with soft wrap and with aligned spacers in a real window.
- **Not compared with IntelliJ running:** the exact width and minimum height of its stripe marks, and its icons' exact colours.

### 5. Verification

Run on 2026-11-12, under `nice -n 15`, with `CARGO_BUILD_JOBS=6` and `--test-threads=6`. The frontend suite and `tsc` ran again on the final code (`15f09a9`), after the final review's fixes; `clippy` and `cargo test` ran on `8205c0c`. The branch changes no Rust.

| Command | Result |
|---|---|
| `pnpm exec tsc --noEmit -p .` | no errors (exit 0), on `15f09a9` |
| `pnpm vitest run --maxWorkers=2` | 138 files, 2,720 tests passed, on `15f09a9` |
| `cargo clippy --all-targets -- -D warnings` | no warnings (exit 0) |
| `cargo test -- --test-threads=6` | library: 2,297 passed, 0 failed, 7 ignored (on purpose), 329.05 s; `askpass` 11, `crlf_fix` 1, `editor` 41 and `git_path` 2 passed; `phase_3_trace` 0 passed and 1 ignored (on purpose); `main.rs` and doc tests 0 tests |
| `python3 -I -m unittest discover -s scripts` | 65 tests, OK |

**Final review.** One independent reviewer read the branch against the plan and 241's source. It found 4 Important and 5 Minor issues.

- **Fixed, each with a test that failed first:**
  - a click on a column did not lead the synchronised scrolling;
  - the centre pane had marks, where 241 hides them;
  - a side with no lines for a change had no mark, where 241 adds one (the user's own IntelliJ screenshot shows it);
  - every `repo-changed` ran the incoming and outgoing check;
  - after a checkout, the old branch's icons showed next to the new name;
  - hunks of another changelist had marks;
  - a mark could spill out of its column while typing.
- **Recorded, not fixed:** the one-line marks of the unified view (decision 7) and the history preview's three-way view having the stripe (scope, now in decision 5 and section 1).
- **Deferred:** the column's height is read on every geometry update.

### Conclusion

The part is done in tests and not validated by hand: it ends when section 3 is completed on a built app.

## Phase 4 summary

Date: 2026-11-10. Phase 4 was delivered in six parts, from 2026-11-03 to 2026-11-10, on the branch `phase-4`. Each part had its plan, its tasks reviewed one by one, its rows in `docs/parity-matrix.md`, its entries in `NOTICE` and its section above. The reference is IntelliJ IDEA CE 2024.1 (`IC-241.14494.240`); `phase-4-reference-change.md` says what changed from 2023.2.3 and what each part closed of it.

### Result of each part

Every part is done in tests and none is validated by hand: each part's conclusion says it ends when its section 5 list is completed.

| Part | What the user gets | Matrix | Review |
|---|---|---|---|
| 1. Modal commit | The `Commit Changes` dialog (⌘K), the `Git` tool window with Local Changes, Log and Console, `Use non-modal commit interface`, and 241's large-file and file-name checks | 22 rows (21 `done`, 1 `partial`) and 4 `Git.Stage.*` rows edited | Each task, then a final review and its fixes |
| 2. Alignment with 2024.1 | Phases 1 to 3 as 2024.1 has them: the copied texts, the new confirmations and options, the branches popup search, the merge and rebase widget, soft wraps in the diff | 13 rows closed, 18 notes, 5 `pending` with their reason | Each task, then a final review and its fixes |
| 3. Patches, stash and shelf | Create Patch, Copy as Patch, Apply Patch (also from the clipboard); Stash Changes, Unstash and the Stash tab; Shelve, Unshelve, the Shelf tab and the import of `.idea/shelf`; `Clean working tree using: Shelve` | 30 action rows (23 `done`, 1 `partial`, 6 `pending`), 7 new rows in section 2 and 2 in section 3, both `Clean working tree using` rows | Each task, then a final review (0 critical, 3 important, 29 minor) and one fix wave |
| 4. History and comparisons | The `Project` tree and the viewer, file history, `Show Repository at Revision`, the comparisons and Compare Branches, `Get`, `Not Cherry-Picked Commits`, `Select in Tree…`, `Show Details`, `Annotate` and `Show History for Selection…` | 35 action rows (22 `done`, 2 `partial`, 11 `pending`), 3 rows of section 2 filled and 1 added | Each task, then a final review (0 critical, 4 important, 6 minor) and one fix wave |
| 5. GitHub | Accounts with their tokens in the Keychain, token login, protected-branch rules from GitHub, the `Pull Requests` window with its list, details, checkout and `Show in Git Log`, `Open on GitHub` and `Copy Link` | 12 rows of sections 2 and 3, and a new section 5 with 23 rows (8 `done`, 15 out of scope) | Each task, then a final review (nothing critical or important of its own) and one fix wave; afterwards the user's decision on external CI links (`c8b2458`, `c32f37c`, `996b302`) |
| 6. Signing, worktrees and submodules | `Configure GPG Key…`, the `GPG Signature` column and the signature in the details, linked worktrees and submodules opened as repositories, the submodule update after Update Project, submodule diffs | 6 rows, all `done` | Each task, then a final review (nothing critical or important of its own, 6 minors) and one fix wave |

At the close, on the tree of the final fix wave's last commit: 130 interface test files with 2,546 tests, 2,194 Rust library tests (7 ignored on purpose), 55 integration tests (1 ignored), 65 script tests, no type error and no clippy warning (part 6, section 7).

### Open across the parts

- **Nothing of phase 4 has been used.** No part was run in the real app on a real repository, seen in a real window or tried with a physical keyboard. The six section 5 lists are the acceptance of the phase, and each conclusion names its main risks: part 1, the dialog and the Git window in a real window, and the points of its section 6 that need your machine; part 2, soft wrapping in a real window; part 3, exchanging patches and shelves with IntelliJ, and Update with Shelve on a real conflict; part 4, the history of a real renamed file, `Get` over a modified file and the viewer opened from the terminal; part 5, a real account and its Keychain item, a pull request from a fork and its checkout, and the protected rules after a real Fetch; part 6, a real GPG key with Ramify opened from Finder, a real worktree and submodule, and Update Project with a detached submodule.
- **No consumption number for phase 4:** the measurement ran, but its gate never opened (next section).
- **Part 6's final review and its fix wave are done.** The two findings that blocked the merge are fixed: the per-submodule fetch refuses the transports git refuses for submodule URLs (`GIT_PROTOCOL_FROM_USER=0`), and a diff window always opens, also when a revision's object is missing (part 6, `Final fix wave`).
- **Parked findings, none of them blocking,** each with its cost in its part's section: part 1's deferred items (for example no user-name check before the commit, section 3); part 2's (section 3); part 3's items that its final review declined to judge (no snapshot before `git stash`, the window before `git restore`, three snapshots per Shelve), and `cargo fmt` still unapplied to older code; part 4's minors left after its fix wave (section 2), including the timing test `on_quit_the_writer_is_cancelled_and_waited_for`, which has flaked before and passed in the verifications of parts 5 and 6; part 5's list at the end of its section 2; part 6's at the end of its section 2.
- **Closed during the phase:** part 3's three minors left open after its final re-review (N1 to N3) by part 4, Task 1; part 3's P7 (the watcher and linked worktrees) by part 6, Task 1; part 5's question on external CI links, answered by the user on 2026-11-09 and done before part 6 began.
- **Left out on purpose:** the four matrix rows that `phase-4-reference-change.md` lists (`Git.Stage.ToggleCommitAll`, `Vcs.Log.OpenAnotherTabInEditor`, `Vcs.Diff.EnableDisableCombinedDiff`, `Vcs.Diff.ToggleSearchInChanges`), GitLab merge requests (overview decision 13), OAuth login, review and Create Pull Request (overview decisions 14 and 16), and worktree and submodule management (overview decision 19, as in 241).

### Consumption

**Not measured.** No part of phase 4 measured memory, CPU or startup (each part's section 6 says so), and the measurement of the whole phase, Task 8 of the part 6 plan, ran on 2026-11-10 with the user's go-ahead but its gate never opened. There are no memory, CPU or startup figures for phase 4.

What was done:
- the release build (`nice -n 15 pnpm tauri build --bundles app`, with `CARGO_BUILD_JOBS=3`) of `d50011a`, which built `Ramify.app` (10.13 MiB);
- `~/ramify-measure/git` prepared with the phase 3 recipe (`builtin/commit.c` in conflict), and restored afterwards to `ebdc087dc9` with `measurement-side` deleted;
- the gate, then `scripts/measure.py`'s `measure_run` one run at a time, for the series with the `GPG Signature` column hidden. The gate is the phase 3 one: input idle for at least 180 s (`HIDIdleTime`), a 1-minute load under 15 and no `CGSSessionScreenIsLocked`, checked every 10 s; a run with input or a lock while it ran is discarded; it gives up after 30 minutes. The script that ran it is not in the repository, as in phase 3.

Why nothing was measured: from 08:40:33 to 09:10:27 the gate made 179 checks and opened in none. The screen was unlocked in all of them and the load was from 2.88 to 15.94, but the time without keyboard or mouse never reached 180 s (at most 59 s): the Mac was in use. The series with the column shown was not started. An earlier wait the same morning (until 08:27) was stopped by hand because the user was in a meeting; it did not open either. `docs/reports/phase-4-consumption.json` keeps every check of the 30-minute wait, with `valid_runs: 0`. The user chose to record it as not measured instead of trying again.

Phase 3 has no figures either (`docs/reports/phase-3-validation.md`, section 1: its gate did not open), so what phase 4 costs against the targets of phase 0 is not known. The points the measurement was meant to settle stay open: the scroll of the `git/git` log with the `GPG Signature` column shown (part 6, Review Focus 3), the second recursive watch of a linked worktree (part 6, plan decision 9), and the parts' own unmeasured costs in their sections 6. To repeat it, the phase 3 recipe (`phase-3-validation.md`, "To repeat the measurement") with the Mac unlocked and untouched for about 20 minutes; with the column shown, the measuring repository's `log-options.json` gets `{"0": {"columns": ["subject", "author", "date", "signature"]}}` and loses it afterwards.
