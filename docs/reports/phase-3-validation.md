# Phase 3: validation report

Date: 2026-11-02. Raw data: `phase-3-consumption.json` (the measurement gate and its log; there is no series, see section 1). Commands: `phase-3-commands.md`. Decisions and pending items: `phase-3-decisions.md`.

## Summary

- The phase's feature is essentially done: reading, accepting and resolving conflicts with the three-pane merge window and the Conflicts dialog; the Merge… and Rebase… dialogs and the branches popup actions (Merge, Rebase, Checkout and Rebase, Pull into…, Smart Merge); continue, skip and abort of merge, rebase, cherry-pick and revert; cherry-pick and revert from the log, with Cherry-Pick and Revert Selected Changes; Edit Commit Message, Drop, Squash, Undo Commit, Fixup and Squash Into; the interactive rebase with its dialog; and the three-pane comparison of HEAD, staged and local. All with the app's own git editor, which answers the requests of `git rebase` and of the messages.
- Of the 36 rows of phase 3, 30 are `done`, 6 `partial` and 0 `pending`. 13 of them also ask for a check by hand. The 15 `git4idea.actions.branch.*` rows that the classifier left out of scope are distributed: 5 to phase 3 (already counted in the 36), 8 to phase 2 (`done`) and 2 to phase 4 (`pending`).
- **`done` means tests, not use.** The Rust logic is tested with real git and throwaway repositories (real merges, rebases, cherry-picks and rewrites, with conflicts), and the interface with the Tauri API simulated in jsdom. Today 1,574 tests of the Rust library pass (5 ignored on purpose), 51 integration tests, 1,637 of the interface and 55 of the scripts; `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` say nothing. None of this has gone through the real IPC of the 51 new commands, nor through WebKit with a merge window or a dialog on screen, nor through a conflict or a rebase of one of your repositories. The phase cannot be accepted without the list in section 4.
- **The consumption could not be measured.** The gate did not open in the 30 minutes it waited, and the log that was saved (the last 40 checks) shows the screen locked. There are no memory, CPU or startup figures for phase 3. The CPU at rest and the memory with the merge window open, which was what the task asked for, remain unmeasured. The measurement is prepared and is repeated with one command (section 1).
- The validation found things that nobody had decided (section 5). The most important: the branch submenu of the log menu did not offer Merge, Rebase, Checkout and Rebase or Pull into…, which are in the popup and in the branches panel; and the interactive rebase with the dialog's plan accepted a git todo longer than the generated one, and the extra commits did not enter the list that is written. The first was a parity gap and the second a risk to the history. Both were fixed in the final review.
- Phase 3 is not validated. It ends when you complete the list in section 4 with your real repositories.

## 1. Measurements

### What was measured and what was not

**Nothing was measured.** The method of phase 2 ("Measurement with the visible window") requires the Mac unlocked, untouched for 3 minutes and with the load below 15. The screen was locked and the gate did not open even once during the wait (from 22:59 to 23:29; `valid_runs: 0`). The log that was saved only covers the last 40 checks (from 23:25:58 to 23:29:19): in all of them `CGSSessionScreenIsLocked` was `Yes`, the time without keyboard or mouse was from 2,411 to 2,612 s and the load from 0.9 to 2.2. The earlier checks were not saved.

The gate is a user script that is not in the repository (the one from phase 2 was lost): it checks `HIDIdleTime` from `ioreg -c IOHIDSystem` ≥ 180 s, `os.getloadavg()[0]` < 15 and that the screen is not locked; it discards a run if there was input while it ran; and it waits 30 minutes at most. `phase-3-consumption.json` keeps its log: the last 40 failed checks with their values, and `valid_runs: 0`.

The measurement was not done with the screen locked. In phase 1 and in the "Measurement of the debt" of phase 2 it was, and WebKit suspends the page a few seconds after it opens: the memory of a merge window that does not paint and a CPU of 0.0 % say nothing about the open window.

| Measurement | Goal | Phase 2 | Phase 3 |
|---|---|---|---|
| Memory at rest | About 300 MB | 219 MB with the diff window (362 MB in the visible series) | **Not measured** |
| CPU at rest | Close to 0 % | 3.1 % with the diff in front (3.6 % in the visible series) | **Not measured**, not even with the merge window open |
| Startup to first paint | Less than 1 s | 0.76 s (visible series) | **Not measured** |
| Log scroll, frames over 50 ms | Less than 1 % | 0 of 997 | **Not measured** |
| Opening the merge window | There is no goal | It did not exist | **Not measured** |

What did change, and can be looked at without waiting for the screen: the executable goes from 7.5 MB to 8.3 MB (8,335,712 bytes) and the `.app` takes 7.96 MiB. Between `93eeac8` (the phase's base) and `3b2dc75` (the last commit before this task) phase 3 adds 37,611 lines and removes 546, in 121 files and 34 commits, and 51 new Tauri commands and one removed (`branch_rebase_abort`, which replaces `operation_abort`); at startup none of it is executed: the merge window and the dialogs are mounted when requested, and `window.ts` only mounts `MergeWindow`.

### What `src/perf/run.ts` does now

`runPerf` opens, after the diff, the merge window with `builtin/commit.c` if it is in conflict, or with the first file in conflict (`measureMerge`), and leaves it open while `measure.py` waits 30 s and measures memory and CPU. The report carries `merge: { path, openMs }` or `{ path, error }`, and `measure.py` copies it to `merge_path`, `merge_open_ms` and `merge_error`. `openMs` is how long the command takes to create the window, not its painting (there is no event like `diff-drawn`). Without conflicts it opens nothing and `merge` is `null`. The tests are `opens_the_merge_window_with_the_commit_c_conflict`, `without_commit_c_in_conflict_opens_the_first_conflicted_file`, `without_conflicts_there_is_no_merge_window`, `a_failure_opening_the_merge_window_is_reported` and `the_report_carries_the_merge_measurement`, and in Python `test_merge_window_measurement`.

### What was not measured, besides

- Startup, scroll and memory of the app with the merge window open and with the diff window at the same time.
- The CPU with the text cursor of the merge window focused. In phase 2 WebKit's native cursor used between 2.8 and 3.9 % with the diff editor focused; the merge window has three CodeMirror editors and it is likely that the same happens, but it is not demonstrated.
- Any network operation, prolonged use or several repositories.

### To repeat the measurement

With the screen unlocked and the Mac untouched. `~/rama-medida/git` is a throwaway clone of `git/git`:

```bash
export PATH=$HOME/.nvm/versions/node/v22.23.4/bin:$PATH
pnpm tauri build --bundles app
cd ~/rama-medida/git
git checkout -q -b measurement-side && sed -i '' 's/static int use_editor = 1/static int use_editor = 1; \/* side *\//' builtin/commit.c
git -c user.name=m -c user.email=m@e.x commit -qam side && git checkout -q master
sed -i '' 's/static int use_editor = 1/static int use_editor = 1; \/* main *\//' builtin/commit.c
git -c user.name=m -c user.email=m@e.x commit -qam main
git merge measurement-side        # leaves builtin/commit.c in conflict
cd -
python3 scripts/measure.py --app src-tauri/target/release/bundle/macos/rama.app/Contents/MacOS/rama \
  --repo ~/rama-medida/git --out docs/reports/phase-3-consumption.json --runs 5
git -C ~/rama-medida/git merge --abort
git -C ~/rama-medida/git reset -q --hard ebdc087dc9 && git -C ~/rama-medida/git branch -D measurement-side
```

Expected: `window_visible: true`, `merge_path: builtin/commit.c` and `merge_open_ms`. With no modified file, `diff_path` comes out empty: the CPU measurement is that of the merge window alone. To measure both windows at once, first modify another file (`echo "// measurement" >> builtin/add.c`) and restore it with `git checkout -- builtin/add.c`.

## 2. Parity matrix

Row-by-row detail in the Result column of `docs/parity-matrix.md`. The legend above (what `done` is) is in the header of that file.

| Section | Phase 3 rows | `done` | `partial` | `pending` |
|---|---:|---:|---:|---:|
| 2. Interface parts | 2 | 1 | 1 | 0 |
| 3. Git settings | 1 | 1 | 0 | 0 |
| 4. IntelliJ actions | 33 | 28 | 5 | 0 |
| **Total** | **36** | **30** | **6** | **0** |

Thirteen rows ask to confirm something by hand: 2 of section 2 and 11 of section 4. They are the merge window and the interactive rebase dialog in WebKit, F2 and ⇧F6 with a physical keyboard, a Drop, a Squash, a cherry-pick and an interactive rebase in real repositories, and Pull into… against a real remote.

**The 6 `partial`:**

- **Interactive rebase dialog:** drag and drop, quick search and the diff preview of the changes panel are missing (Task 14).
- **`Git.Stage.AcceptTheirs`, `Git.Stage.AcceptYours` and `Git.Stage.Merge`:** the Staging view menu shares the code of the changelists menu (`conflictActions`), which does have tests, but none opens the Staging menu with a conflict. And the separator after Accept Yours is missing.
- **`Git.Stage.Compare.Three.Versions` and `Git.Stage.ThreeSideDiff`:** word highlighting, folding, unified view, combined view and chain of files in three panes are missing (it opens the first chosen file, not all of them), and with `autocrlf` or git filters it goes back to two panes (Task 15). Neither the editor's menu nor the local file's menu is there.
- The separator that is missing after Accept Yours in the Staging view menu is in the `Git.Stage.*` rows.

**Changes to rows of other phases:**

- The 15 `git4idea.actions.branch.*` rows came from "Out of scope" (decision 9): `GitMergeBranchAction`, `GitRebaseBranchAction`, `GitCheckoutWithRebaseAction` and `GitPullBranchAction$WithMerge` and `$WithRebase` to phase 3 (`done`); `GitCheckoutAction`, `GitCheckoutAsNewBranch`, `GitCheckoutFromInputAction`, `GitNewBranchAction`, `GitDeleteBranchAction`, `GitRenameBranchAction`, `GitPushBranchAction` and `GitUpdateSelectedBranchAction` to phase 2 (`done`, with the tests of the sibling rows); `GitCompareWithBranchAction` and `GitShowDiffWithBranchAction` to phase 4 (`pending`).
- The phase 2 row `Select in Tree…` cited `menu_in_intellij_order_without_phase_4_items`, a test name that no longer exists; it now cites `menu_in_intellij_order_without_phase_4_items`.

### Classifier

`scripts/extract_actions.py` sent everything under `git4idea.` to "Out of scope". Now it classifies the branches popup actions first. First `test_branches_popup_actions_in_their_phase` was written and seen to fail with `('Out of scope', None, 'out') != ('Conflicts and rebase', 3, 'include')`; with the change, and with `test_the_rest_of_git4idea_stays_out_of_scope`, the 55 tests of `scripts/` pass. The table was not regenerated from the XML (the Result column is manual, and would be lost): the 15 rows were moved by hand to their phase block, and the rest of the classification did not change.

## 3. Commands

`phase-3-commands.md` records, with `scripts/git-wrapper.sh` and throwaway repositories, what the app launches in merge (with and without conflict), Accept Yours, Accept Theirs and Apply, Smart Merge, rebase (with conflict, continue, skip and abort), interactive rebase with a plan, cherry-pick and revert (with and without conflict, and abort), Cherry-Pick and Revert Selected Changes, undo commit, reword (of HEAD and of an earlier one), drop, squash and an `edit` with its continue. They match the plan and what IntelliJ's code says, without live comparison, in the write commands, their arguments and their order, except for the differences in the report's table:

- **Decided:** Smart Merge with `stash save` instead of Shelve (decision 1); `git apply` instead of IntelliJ's own patch (decision 8); `CHERRY_PICK_HEAD` kept until committing (ruling of Task 10); `stash pop stash@{n}` of the entry the app saved.
- **That do not change the result:** the merge options go before the branch; `reset --soft <parent> --` with one more `--`; `revert --no-edit`; Apply with `add -f --` and not `add --ignore-errors -A -f --`.
- **That can change it:** `merge_commit` forces `i18n.commitEncoding=UTF-8` (with a repository in another encoding, the message of a merge with conflicts would be decoded wrongly; minor pending item of Task 7).
- **Extra reads**, which change nothing: 347 of the 394 lines are the app's state reads.
- **Not compared live with IntelliJ:** what it launches when committing a merge with conflicts, and its order for Cherry-Pick Selected Changes when the patch does not go in.

The trace does not go through the IPC or the interface, and does not cover `--onto`, `--root`, the native mode of the interactive rebase, Fixup… and Squash Into…, Accept during a rebase, Pull into…, `--squash` and `--no-commit`, nor the second attempt with `--3way`.

## 4. Checklist to verify by hand

None of this could be checked automatically. Use a test repository first (`git init`, two branches that change the same line) and then a real one. For the network, start with a repository of yours that you can break.

How to open the app:

```bash
open -n src-tauri/target/release/bundle/macos/rama.app --args /path/to/a/repository
```

The `.app` is already built at `src-tauri/target/release/bundle/macos/rama.app` (7.96 MiB, ad hoc signature). With a double click in Finder it opens without a repository: type the path in the bar. To rebuild it:

```bash
export PATH=$HOME/.nvm/versions/node/v22.23.4/bin:$PATH
pnpm tauri build --bundles app
```

### Real IPC of the new commands

`scripts/smoke_ipc.py` was not repeated in phase 3. There are 51 new commands since phase 2 that have not gone through Tauri's IPC: the tests call the Rust functions and simulate `invoke` in the interface. A failure here would be one of registration in `lib.rs`, of argument names in camelCase or of serialization. For each row, check that the action does something and that the console or a notification does not show `command … not found` or an argument error.

- [ ] **Conflicts and merge (14):** `conflicts_session`, `conflict_sides`, `conflicts_accept`, `conflict_resolve`, `merge_commit`, `merge_message`, `merge_blocks`, `merge_window_open`, `merge_window_path`, `merge_window_done`, `merge_working_file`, `merge_leave_received`, `merge_left`, `merge_window_ready`.
- [ ] **Merge, rebase and pull into (12):** `git_merge`, `git_rebase`, `branch_merge`, `branch_rebase`, `branch_rebase_remote`, `branch_pull_into`, `rebase_published`, `unmerged_branches`, `merge_summary`, `revisions_valid`, `merge_settings_set` and `rebase_settings_set`.
- [ ] **Operation in progress (6):** `repo_state`, `operation_continue`, `operation_skip`, `operation_abort` (replaces `branch_rebase_abort`), `git_menu_enable` and `editor_answer`.
- [ ] **Cherry-pick and revert (3):** `apply_commits`, `apply_commit_after_conflicts` and `apply_paths`.
- [ ] **Rewriting (13):** `rewrite_menu`, `rewrite_check`, `rewrite_check_single`, `rewrite_validate`, `rewrite_reword`, `rewrite_drop`, `rewrite_squash`, `rewrite_uncommit`, `rewrite_undo`, `rewrite_undo_check`, `rewrite_autosquash`, `rewrite_interactive_start` and `rewrite_interactive`.
- [ ] **Three panes of HEAD, staged and local (3):** `staging_three_sides`, `index_write` and `staging_revert_range`.

### Real conflicts and the merge window

With a merge that really collides, in a repository you can break.

- [ ] Merge… with a conflict: the Conflicts dialog opens by itself, with the Yours and Theirs columns and the name of the branches; Merge… opens the merge window with the chosen file.
- [ ] The merge window: the three panes, the bands between them, the accept and ignore arrows and the resolve wand. Apply the non-conflicting changes from the left and from the right; Accept Left and Accept Right; Apply with pending blocks (it must ask); Cancel with changes (it must ask); `⌘Enter` applies and `F7` and `⇧F7` jump between blocks.
- [ ] **That Apply destroys the window without hanging on macOS** (`merge_window_done` destroys the window inside its own IPC call; pending item of Task 6).
- [ ] A file that you change from outside while the window is open (like Devin): `Conflict is not valid…` must appear and nothing must be written. And `git add` from outside of the file in conflict.
- [ ] A file with CRLF, a binary one (only Accept Left/Right) and a deletion conflict (`deleted by you` / `deleted by them`).
- [ ] Accept Yours and Accept Theirs from the Commit view menu, with and without staging area; it leaves a copy of the file among the saved copies. In a rebase, Yours must be the branch you are rebasing.
- [ ] Opening another repository or quitting with a half-done merge window: it must ask, and cancelling must leave it.
- [ ] **Physical keyboard in the merge window:** `⌘Enter`, `F7`, `⇧F7`, `Esc`, the click with a modifier on the arrows (resolves the whole block) and typing in the center pane.
- [ ] The `Merge Conflicts` node of the Commit view takes the files out of their changelist; it could not be checked in IntelliJ's copy (`ChangesBrowserConflictsNode` is not built in 232), so compare it with your IntelliJ.
- [ ] Merge with `--no-commit` and with `--squash`: the Commit view must propose the message, and committing must close the merge.
- [ ] Smart Merge with overlapping local changes: the dialog, the stash `Uncommitted changes before Merge at <fecha>`, and that your changes come back (also after a resolved conflict).

### Rebase

- [ ] Rebase… without a conflict, with `--onto`, with a branch that is not the current one and with local changes (they are saved and come back).
- [ ] A rebase that stops at a conflict: the card with Resolve, Continue and Abort; resolve and Continue; Skip; Abort. Two conflicts in a row in the same rebase.
- [ ] A rebase stopped by something you type in the terminal: the app detects it and offers Continue and Abort.
- [ ] The Git menu of the menu bar: Merge…, Rebase…, Resolve Conflicts…, and those of Abort, Continue and Skip only with something in progress; the Rebase submenu has no automatic test (Task 2).
- [ ] Checkout and Rebase onto, Rebase and Merge from the branches popup and from the panel; Pull into… Using Merge and Rebase against a real remote.
- [ ] Rebase of published commits: it must ask `Rebasing Published Commit` with Cancel by default.

### Interactive rebase and rewriting of a branch of your own

On a branch of yours that you can rewrite.

- [ ] **Interactively Rebase from Here…**: the `Rebasing Commits` dialog with the oldest at the top; pick, reword, squash, fixup, drop and edit; reordering; `⌥P`, `⌥E`, `⌥D`, `⌥R`, `⌥S` and `⌥F`; View Git Commands; Reset; cancelling with changes. The real look.
- [ ] **Reopening the app with the rebase stopped at `edit`** (the detection of Task 2), and Continue.
- [ ] A range with a merge, a root or a `fixup!`: it must switch to native mode and show git's list.
- [ ] Edit Commit Message… (`F2` and `⇧F6` in the log, with a physical keyboard) of the last commit and of an earlier one; a commit already pushed to a protected branch must be rejected without losing the message.
- [ ] Drop Commits…, Squash Commits…, Undo Commit… and the card's Undo (which does not close by itself).
- [ ] Fixup… and Squash Into…, and Commit and Rebase… from the Commit view.
- [ ] The order of the message fill-in of a squash (`commitDetails(hashes)`), the order of the log menu entries and that `rewrite_menu` and `rewrite_undo_check` do not show a visible task on every right click (Task 12).
- [ ] The separator between Cherry-Pick and Checkout in the log menu (Task 10).

### Cherry-pick and revert

- [ ] **Cherry-pick between branches** of a real repository: one, several (in the log's order) and one with a conflict (resolve and have it commit, with the original author); the `(cherry picked from commit …)` suffix only with published commits and the setting on.
- [ ] Revert Commit with and without a conflict; Abort Cherry-Pick and Abort Revert.
- [ ] Cherry-Pick Selected Changes and Revert Selected Changes on files of the log's changes panel, also with several commits and a file that does not go in cleanly (`--3way`). The name of the list that IntelliJ proposes with several commits could not be checked.
- [ ] Compare HEAD, Staged and Local Versions: stage, unstage and revert of a range with the arrows, and that what you type in Staged or Local is not lost.

### Live comparison with IntelliJ

Close IntelliJ and open it from a terminal so that it inherits the variables:

```bash
export RAMA_GIT_REAL=$(command -v git)
export RAMA_GIT_LOG=~/intellij-commands-phase-3.log
"/Applications/IntelliJ IDEA CE.app/Contents/MacOS/idea"
```

Then set `scripts/git-wrapper.sh` as the Git executable path in its settings and do the same as in `phase-3-commands.md`.

- [ ] Compare the log with the tables of `phase-3-commands.md`, above all rows 2, 4, 6, 13, 14 and 16.
- [ ] The merge window and the interactive rebase dialog side by side with screenshots.
- [ ] Return IntelliJ's Git path to its previous value.

### Measurement

- [ ] Repeat the measurement of section 1 with the machine at rest and the screen unlocked, with the merge window open, and compare the CPU with the cursor focused and unfocused.

## 5. What the validation found

This was not decided anywhere. What is deliberate is in `phase-3-decisions.md`. In this task only points 4 and 5 were fixed; points 1 and 2 were fixed in the final review, and 8 was checked in it.

1. **The branch submenu of the log context menu did not offer Merge, Rebase, Checkout and Rebase or Pull into…. Fixed in the final review.** `operations` in `src/log/contextMenu.ts` now builds the same rows as `branchActions` (`src/branches/model.ts`), with their texts and rules, like `GitLogBranchOperationsActionGroup`, which reuses `LocalBranchActions` and `RemoteBranchActions`; they go to the same flow of `src/branches/merging.ts`. The test is now called `local_and_remote_branch_submenu_like_the_popup_without_phase_4_items`, and `the_branch_submenu_says_and_disables_the_same_as_the_popup` compares the two lists.
2. **The interactive rebase with the dialog's plan accepted a git todo longer than the generated one. Fixed in the final review:** it now requires the same number of lines (test `a_git_todo_with_an_extra_line_gives_invalid_todo`), and from the dialog only the order and the actions count (`the_written_todo_carries_gits_subject_and_not_the_dialogs`). What was found: In `Interactive::list` (`rewrite.rs`), `log.generated.len() <= entries.len()` lets through a todo with extra lines, and what is written is the whole plan: what git would have added is left over and those commits would not enter the list, so the rebase would leave them out of the branch. It only happens if the range changes between the generation and git's start, which `expected_head` almost prevents, but it is history that is lost. It is the finding that Task 14 marked as a priority of the final review: require `==`.
3. **The interface and Rust tests are slower than in phase 2.** `cargo test` takes 113 s in the library (phase 2 did not record it) and the interface suite 10 s; they passed the first time, with the machine at a load of 2 to 3.
4. **`NOTICE` did not list `src/editor/editor.ts` or `src/operation/ConfirmDialog.svelte`. Fixed.** Both reproduce texts and confirmations of `GitUnstructuredEditor`, `GitInteractiveRebaseEditorHandler` and `GitAbortOperationAction`. `src/merge/window.ts` only mounts the window and ports nothing.
5. **The matrix cited a test that no longer exists.** `menu_in_intellij_order_without_phase_4_items` is now called `menu_in_intellij_order_without_phase_4_items`. **Fixed**, and all the test names in the Result column were checked with `grep` against the code.
6. **The name of `the_git_menu_abort_merge_asks_for_confirmation_and_aborts` does not say what it does** (it cancels and does not abort; minor pending item of Task 2). That is why the matrix cites `abort_asks_for_the_intellij_confirmation` and `cancelling_the_confirmation_does_not_abort` for `Git.Merge.Abort`.
7. **A single test of the command layer.** As in phase 2, `mutating_without_saving_the_diff_is_not_allowed` is the only one that goes through the `vcs_commands.rs` layer, and the count was updated with each task. Nobody has gone through the complete path, not even with the 51 new commands (see section 4).
8. **That no command holds `RepoLocks` across `flush_diff_window`** was not checked in this task (Task 15). The final review checked it: no command does.

What the reviews deferred (126 minor findings) is in `phase-3-decisions.md`. Those that can most affect your own work:

- An old `SQUASH_MSG` would make `merge_commit` commit someone else's changes if you abort outside the app with the dialog open (Task 8).
- With two of the app's stashes at once, they can be returned in the opposite order; git refuses to overwrite (Task 9).
- After aborting a stopped rebase from outside, the card still has Continue and Abort and gives `Cannot continue rebase` (Task 9).
- The merge window's automatic resolution depends on time: after 1 s, the following blocks lose the automatic resolution even if they are trivial (Task 4).
- If `merge_window_done` fails, the merge window is left with dead buttons until the exit is forced (Task 6).
- A panic inside the editor's server would leave a cancel waiting for the 5 s grace (Task 1).
- Any regular file of git's directory can be edited through the editor dialog; it should be limited to the known editor names (Task 1).
- Even with the dialog's reword messages only in memory: if you close the app with the rebase stopped, git will ask for the message through the free editor (Task 14).

## 6. Known differences with IntelliJ

### The decided ones

They are at the beginning of `phase-3-decisions.md`.

### The ones that have not been compared

- `Show Details` and `Filter by conflicted file` of the merge window, the `Conflicts` window and the "Resolving merge conflicts is in progress" bar do not exist, on purpose (decisions 6 and 7). `Compare with…` and `Show Diff with Working Tree` still do not exist, until phase 4.
- The order of the log menu entries was compared with `Git.Log.ContextMenu` of `plugin.xml`, not with `Vcs.Log.ContextMenu`, whose XML is not at the tag (Task 10).
- The navigation rule between blocks of the merge window (`F7`) was not compared with `PrevNextDifferenceIterableBase`, which is not in IntelliJ's copy (Task 5).
- The gaps of the three panes of HEAD, staged and local (Task 15): no word highlighting, folding, unified view, combined view or chain of files; with `autocrlf` or filters it goes back to two panes; it only opens from the Staging view. The Stage, Unstage and Revert arrows save at once and cannot be undone with ⌘Z before saving.
- The gaps of the interactive rebase dialog (Task 14): no drag and drop, quick search or diff preview in the dialog's changes panel.
- With a deletion conflict during a normal merge, the label is `deleted by you` where IntelliJ says `deleted by them` (Task 3).
- The `Retry` button of a failed commit after conflicts was invented by Task 10; IntelliJ uses its commit dialog.
- `Cherry-pick was performed with conflicts` is not passed as the title of the Conflicts dialog (Task 10).
- IntelliJ commits in Commit and Rebase… only the changes of the default changelist; here the Commit view's inclusion is not touched (Task 13).

## Conclusion

The behavior that can be tested without a network, without a screen and without a keyboard is tested with real git, and matches what IntelliJ's code says, without live comparison, in the git commands it runs, except for the decided differences. The consumption of phase 3 has not been measured, and so it is not known whether the merge window repeats the 3 % idle CPU of the diff window. What decides whether the phase is finished has not been seen by anyone yet: a merge window on screen, a conflict from one of your repositories, an interactive rebase of a branch of your own and the physical keyboard. Phase 3 ends when you complete the list in section 4.
