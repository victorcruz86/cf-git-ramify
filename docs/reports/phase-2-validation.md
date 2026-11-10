# Phase 2: validation report

Date: 2026-11-01. Raw data: `phase-2-consumption.json` (the main series), `phase-2-consumption-high-load.json` (another series, with the machine heavily loaded) and `phase-2-consumption-no-diff.json` (a run without the diff window). Commands: `phase-2-commands.md`. Decisions and pending items: `phase-2-decisions.md`.

## Summary

- The phase's feature is done in essence: log with its graph, filters, detail and context menu; branches popup and panel; branch, tag and reset operations; fetch, Update Project and Pull; push dialog with force push and protected branches; remotes, clone and init; credentials through askpass.
- Of the 80 rows of phase 2, 71 are `done`, 2 `partial` and 7 `pending`. 21 of them also ask for a check by hand.
- **`done` means tests, not use.** The Rust logic is tested with real git and local bare remotes, and the interface with the Tauri API simulated in jsdom. None of this has gone through the real IPC of the 78 new commands, nor through WebKit with a dialog on screen, nor over the network, nor with a physical keyboard. The phase cannot be accepted without the list in section 4.
- **The measurement could be done with a visible window for the first time**: in phase 1 the screen was locked. The median of 5 runs with `git/git` gives 219 MB and a scroll without jumps (0 of 997 frames over 50 ms), but startup is still at 2.5 s until the first paint (target: under 1 s) and idle CPU is 3.1 %, not close to 0. A run without a diff window and without modified files gives 161 MB and 0.0 %: that 3 % and about 58 MB of difference mix the two factors, and it has not been shown that they come from the diff window.
- **There is a regression with respect to phase 1:** showing the window goes from 1.45 to 2.26 s, the WebView from 86 to 165 MB and the memory from 206 to 219 MB. Phase 1 was measured with a load of 5 to 7 and without painting anything; this one, with a load of 9 to 14 and with the window visible.
- The machine was loaded by something else during the measurement (1-minute load from 9 to 14 in the main series, from 12 to 85 in the other). Startup times are pessimistic by an amount I cannot separate.
- The validation found things nobody had decided (section 5). The final batch fixed four: the `Git.Branches` shortcut that IntelliJ removes on Mac, `New Tag…` and `Reset HEAD…` in the Git menu, F2, ⇧F6, ⌥F1 and ⌘⌥N in the branches panel, and the text of the Apache 2.0 license. The measurements in section 1 are from before that batch.
- Phase 2 is not validated. It ends when you complete the list in section 4 with your real repositories.

## 1. Measurements

### What was measured and what was not

The screen was unlocked and the window visible: the 11 runs gave `window_visible: true` and a complete `visible_runs`. It is the path that in phase 1 was only covered by unit tests. `src/perf/run.ts` needed no changes to open `LogPanel`: since Task 7 it is `App.svelte` that hands it the panel's `perfTarget()` (the table's scroller, the `viewId` and `openCommit`), and the measurement already goes through it.

The measurement leaves the Commit view at the end, with the diff of `builtin/commit.c` in its window, the same as in phase 1. It is the same configuration as phase 0 with one difference: that diff was read-only and was in the main window, and this one is an editor in a window of its own.

| Measurement | Target | Phase 0 | Phase 1 | Phase 2, with the diff window and `builtin/commit.c` modified (median of 5) | Phase 2, without the diff window or modified files (1 run) | IntelliJ 2023.2.3 | Met? |
|---|---|---|---|---|---|---|---|
| Idle memory | About 300 MB | 224 MB, 102 MB of the WebView | 206 MB, 86 MB of the WebView, nothing painted | **219 MB**, 165 MB of the WebView (from 217 to 267 MB) | **161 MB**, 110 MB of the WebView | 2,918 MB | Yes, with both |
| Idle CPU | Close to 0 % | 0.03 % or less | 0.0 %, with the page suspended | **3.1 %** (from 2.8 to 3.2 %) | **0.0 %** | From 5 to 14 % | No with the diff window and the modified file; yes without both |
| Startup until the window is shown | Under 1 s | Not measured | 1.45 s (from 1.21 to 1.51 s) | **2.26 s** (from 1.25 to 3.00 s) | 2.21 s | Not measured | No |
| Startup until the first paint | Under 1 s | 4.6 s (until the first rows) | Not measured (1.3 s with `open -n` in an earlier version) | **2.53 s** (from 1.46 to 3.30 s) | 2.41 s | Not measured | **No** |
| Startup until the log is loaded | There is no target | 4.0 s | Not measured | 3.80 s (from 2.72 to 4.94 s) | 3.67 s | Not measured | Not applicable |
| Log scroll, frames over 50 ms | Under 1 % | 0.1 % (1 of 983), p99 of 19 ms | Not measured | **0 of 997** (in one run, 1 of 991), p50 10 ms, p95 12 ms, p99 12 ms | 0 of 994, p99 12 ms | Not measured | Yes |
| Opening the diff of a normal file | There is no target | Not measured | 0.29 s without painting | 0.37 s until the editors are created, 6 ms of painting | Not applicable | Not measured | Not applicable |

Medians of 5 runs in release with `git/git` (85,936 commits, 1,022 references), the log loaded, the Commit view and `builtin/commit.c` modified and open in the diff window. Startup times are counted from when the process is launched. After the measurement, `builtin/commit.c` was restored with `git checkout`.

**Machine load.** The 1-minute load at the start of each run of the main series was 10.4, 13.6, 9.0, 11.7 and 9.5. In phase 1 it was 5 to 7. Another series, made earlier with the machine much more loaded (85, 72, 54, 23 and 12), gave similar memory (median of 217 MB), similar CPU (3.0 %) and a worse startup (first paint from 2.2 to 5.0 s; median of 2.85 s). The cause of the load is external to the app (a virtual machine and system services). I do not know what part of the 2.5 s is the app's.

**Regression with respect to phase 1.** Showing the window goes from 1.45 to 2.26 s, the WebView from 86 to 165 MB and the memory from 206 to 219 MB. Two caveats: the load was 9 to 14 versus 5 to 7, and in phase 1 the page was suspended and painted nothing. With those differences I do not know what part is the app's; only a measurement at idle will tell (section 4).

### How to read each measurement

**Memory.** It is within the target with room to spare, with and without the diff window. With the diff window open and the file modified the WebView goes from 110 to 165 MB (58 MB in total). The run without it has no modified files, so the difference is from both things together. The first two runs of the main series gave 250 and 267 MB, and the next three 217 to 219 MB; I do not know what causes it. It does not include prolonged use: the measurement is 30 s of idle after opening.

**CPU.** With the diff window open and the file modified, the app and its WebKit processes add up to 3 %. Without both, 0.0 %. It has not been separated how much belongs to each factor, nor was a per-process breakdown saved. A sampling with `sample`, whose data was not saved, pointed to repainting of the window. **I have not identified what repaints it.** The diff editor's cursor is a possible cause and it is not verified; the experiment is in section 4. In the high-load series, one of the five runs with diff gave 0.07 %: I do not know why either. With the screen locked in phase 1 the CPU came out 0.0 % because the page was suspended, so that figure does not contradict this one.

**Startup.** It does not meet the target and has not changed in nature since phase 0. In the main series, the page reaches its first paint 0.97 s after starting (median), and shows the window at 0.73 s. The rest of the 2.26 and 2.53 s happens before the page exists, in the native startup of Tauri and the WebView. Loading the `git/git` log takes 1.15 s (from `log-open-start` to `log-open-end`); `phase-2-graph.md` measured 0.87 to 1.06 s in the process, without Tauri or drawing; I cannot separate how much of the difference is due to the machine's load. The app starts in the Commit view and the log is loaded when its tab is opened (section 4 of the design), so those 1.15 s do not enter into the normal startup, only into the measurement.

**Scroll.** It meets the target with room to spare: p99 of 12 ms versus 19 ms in phase 0. The measurement scrolls the log for 10 s with the full panel's table.

**Opening the diff.** 0.37 s until the editors are created (from 0.34 to 0.39 s), against 0.29 s without painting in phase 1; the painting is 6 ms. With a different load, they are not directly comparable.

### What changed in the binary

The executable goes from 5.5 MB to 7.5 MB, and the `.app` takes 7.2 MiB. The `.app` was rebuilt after the final batch: it is no longer the measured binary.

### What was not measured

- IntelliJ's startup and scroll, which were not measured in phases 0 and 1 either.
- A cold startup, after rebooting, and a startup from Finder: the measurements launch the binary from a script.
- The CPU with the Log tab in view at idle: the measurement ends in the Commit view.
- Memory after prolonged use, with several repositories or with network operations.
- The large file in the diff window (72,000 lines) of phase 1: it was not repeated.
- Any network operation. All the measurements are local.
- The scroll with filters, with Long Edges on or with the branches panel open: it was measured with the default configuration. `phase-2-graph.md` measures Long Edges in the process, without drawing.

To repeat the measurement with the machine at idle:

```bash
export PATH=$HOME/.nvm/versions/node/v22.23.4/bin:$PATH
echo "// measurement" >> ~/ramify-measure/git/builtin/commit.c
python3 scripts/measure.py --app src-tauri/target/release/ramify \
  --repo ~/ramify-measure/git --out docs/reports/phase-2-consumption.json --runs 5
git -C ~/ramify-measure/git checkout -- builtin/commit.c
```

### Measurement of the debt (2026-11-02)

Data: `validation-consumption-locked.json`. Median of 5 runs in release with `git/git` (82,668 commits, 1,022 references), `builtin/commit.c` modified and open in the diff window, load from 5.8 to 14.0. **The screen was locked**, as in phase 1, so the 5 runs gave `window_visible: false`. These figures are compared with phase 1 and not with the visible series of phase 2.

| Measurement | Phase 1 | Now, same conditions |
|---|---|---|
| Show the window | 1.45 s | **0.50 s** (from 0.49 to 0.64 s) |
| Memory | 206 MB, 86 of the WebView | **147 MB**, 76 of the WebView |
| Open the diff, without painting | 0.29 s | 0.36 s |
| Executable | 5.5 MB | 7.5 MB (7,507,424 bytes); `.app` of 7.2 MiB |

- **The startup regression does not appear.** Under the same condition, showing the window goes down from 1.45 to 0.50 s. The jump from 1.45 to 2.26 s compared a hidden window with a visible one. I do not know why it went down so much: the machine's state may have weighed, and phase 1 was measured in another session.
- With the window hidden the CPU comes out at 0.0 %, which proves nothing: WebKit suspends the page. The first paint and the CPU with a diff were measured afterwards with the screen unlocked, below.

### Measurement with the visible window (2026-11-02)

Data: `validation-consumption-visible.json`. Same method as the main series of phase 2 (`measure.py`, `git/git`, `builtin/commit.c` modified and open in the diff window), on the laptop's screen (Retina, 3456×2234) and without the external monitor. Each run waited until the Mac had gone 3 minutes without keyboard or mouse and with the load below 15. A run with input halfway, or in which Ramify was not in front at the start and at the end of the CPU sample, was thrown away; there was one. Two builds were alternated: that of `4881753`, which is the current one, and an uncommitted variant, with `drawSelection({ cursorBlinkRate: 0 })`. Four valid runs of each; the load, from 7.7 to 14.7.

| Measurement | Phase 2 | Current | Without CodeMirror blinking |
|---|---|---|---|
| First paint | 2.53 s | **0.76 s** (from 0.64 to 0.96 s) | 0.65 s |
| Show the window | 2.26 s | 0.70 s | 0.61 s |
| CPU at rest, diff in front | 3.1 % | **3.6 %** (from 2.8 to 3.9 %) | 2.8 % (from 2.7 to 2.9 %) |
| Memory | 219 MB | 362 MB, 302 of WebKit | 361 MB |
| Frames over 50 ms in the scroll | 0 | 0 (p95 of 18.5 ms) | 0 |

- **The first paint meets the goal (less than 1 s) in this series.** I do not know why in phase 2 it took 2.5 s. There the extra time passed before the page existed (section 1), and I have not been able to reproduce it. Nothing in the startup was changed.
- **CPU: the cause is the text cursor while the editor has focus.** A stack sample (`sample`) of the diff page, with the variant without blinking, puts the drawing time in `WebCore::OpacityCaretAnimator::updateAnimationProperties`, which is WebKit's native cursor. According to WebKit's code (`FrameSelection::updateAppearance`, `OpacityCaretAnimator.cpp`), WebKit animates that cursor whenever there is a cursor in an editable field with focus and the window active. It animates it even though CodeMirror has made it transparent: 10 opacity steps per second, and each one repaints and travels to the Ramify process and to the GPU. By process, with the variant: diff page 1.3 %, Ramify 1.3 %, GPU 0.6 %, main page 0 %. CodeMirror's own blinking adds about 0.8 points.
- **With the diff window open but not active, the CPU drops to between 0.1 and 0.2 %.** It was measured with the main one in front, in an earlier round without the input check; take it as an indication. The cost exists only while you look at the diff with the editor focused. IntelliJ, with the IDE at rest, used 5 to 14 % in phase 0 (`phase-0-intellij.json`).
- **How to stop the native cursor:** WebKit only stops it with the system accessibility setting that prefers a cursor without blinking, and that setting is not touched. Removing CodeMirror's blinking saves 0.8 points, but leaves a fixed cursor where IntelliJ makes it blink. It is pending a decision.
- **Memory:** it goes up because of WebKit (GPU 141 MB, the two pages 71 and 79 MB); the Ramify process stays at about 60 MB. It is probably the Retina screen: at scale 2, the drawing buffers have four times as many pixels. I do not know on which screen the phase 2 series was made, so it is not demonstrated.
- **Ruled out:** CodeMirror is not loaded at startup. `theme-*.js` is `src/diff/theme.ts` and only the diff window requests it. The main one loads `index`, `App` and `run`, about 338 KB.
- **Not done:** compare the row height with IntelliJ screenshots. It needs screenshots of yours.

## 2. Parity matrix

Row-by-row detail in the Result column of `docs/parity-matrix.md`. The legend above (what `done` is) is in the header of that file.

| Section | Phase 2 rows | `done` | `partial` | `pending` |
|---|---:|---:|---:|---:|
| 2. Interface parts | 7 | 6 | 1 | 0 |
| 3. Git settings | 8 | 7 | 1 | 0 |
| 4. IntelliJ actions | 65 | 58 | 0 | 7 |
| **Total** | **80** | **71** | **2** | **7** |

Twenty-one rows ask to confirm something by hand: 2 of section 2, 2 of section 3 and 17 of section 4. They are shortcuts with a physical keyboard, the clipboard, `open -t`, and everything that talks to a real network.

**The 2 `partial`:** the columns and panel sizes (the height of 24 and the sizes were not compared with screenshots, decision 12) and `Clean working tree using` (Stash fixed, decision 2).

**The 7 `pending`:**

- **6 with no effect in the app, because of decision 10:** `Vcs.Log.CheckFirstCommitsIndexData`, `CheckSelectedCommitsIndexData`, `InvalidateCaches`, `ResumeIndexing`, `ShowRootsColumnAction` and `Git.Log.Branches.GroupBy.Repository`.
- **1 that is really missing:** `Git.SelectInGitLog` (it belongs to a file's history, phase 4).

**Changes to rows of other phases:**

- `Git.Commit.And.Push.Executor`, of phase 1, becomes `done`: Commit and Push was done in Task 14.
- `Vcs.ShowDiffWithLocal` and `Vcs.ShowDiffWithLocal.Before`, also of phase 1 and dependent on the log, become `pending` in phase 4, with `Compare with…` and `Show Diff with Working Tree`.
- `Vcs.Log.ShowAllAffected` and `Vcs.Log.ShowOtherBranches` move to phase 4 (decision 11), and are moved to its block.
- Four new rows in phase 4: `Select in Tree…`, `Compare with…` and `Show Diff with Working Tree`, `Clean working tree using: Shelve` and GitHub's protected-branch rules.
- The row `Clean working tree…` becomes `Clean working tree using: Stash or Shelve`, and the protected-branches one is split into the list of patterns and GitHub's rules. The row of the update method stopped saying "or the branch's": the settings screen of 2023.2.3 only offers Merge and Rebase.

### Classifier

`scripts/extract_actions.py` took to phase 2 everything that starts with `Vcs.Log.`. Now it takes `ShowAllAffected`, `ShowOtherBranches` and the three rows that the matrix already had in phase 4 since phase 1 (`AnnotateRevisionAction`, `CompareRevisions`, `OpenRepositoryVersion`) to phase 4. The tests were written first and seen to fail: `test_show_all_affected_in_phase_4` and `test_the_history_log_actions_are_in_phase_4` failed with `('Log and branches', 2, 'include') != ('History and comparison', 4, 'include')`. After the change, the 50 tests of `scripts/` pass, and the classification of the 4 sections matches the matrix row by row. The table was not regenerated from IntelliJ's XML files (`VcsActions.xml`, the keymaps and the bundles are not in the reference folder): it was checked that `classify` gives, for each identifier of the matrix, the area, phase and status that appear in it.

## 3. Commands

`phase-2-commands.md` records, with `scripts/git-wrapper.sh` and a disposable repository with a local bare remote, what the app launches in checkout (clean, Smart, Force and of a remote branch), new branch, fetch, update (merge and rebase), pull (with and without `--rebase`), push, push of a new branch, rejected push, force push with and without lease, and the retry after a rejection. They match the plan and what IntelliJ's code says, without live comparison, in the write commands, their arguments and their order: the IntelliJ column comes from reading the code, and rows 4, 5, 8, 13 and 17 are left for the live comparison of section 4. The differences:

- **Decided:** no `-c credential.helper=` (decision 3); `--force-with-lease=refs/heads/<branch>:<sha>` and `+refs/heads/<branch>:refs/heads/<branch>` instead of `--force` (rulings of Task 14); `stash pop stash@{n}` of the entry the app saved; prior copy of what is only in the stage.
- **Extra reads**, which change nothing: about 170 of 267 lines are reads of the app's state.
- **The count of the result (row 4):** `rev-list --count <start>..<end>` instead of `git log <start>..<end>`.
- **Not read in IntelliJ:** the message of Update's `stash save`.

The trace does not go through the IPC or the interface, and does not cover deleting a branch, rename, tags, reset, background `ls-remote`, clone or the remotes commands.

## 4. List to check by hand

None of this could be checked automatically. Use first a test repository with a local bare remote (`git init --bare`) and then a real one. For the network, start with a repository of yours that you can break.

How to build and open the app:

```bash
export PATH=$HOME/.nvm/versions/node/v22.23.4/bin:$PATH
pnpm tauri build --bundles app
open -n src-tauri/target/release/bundle/macos/Ramify.app --args /path/to/a/repository
```

The `.app` is already built at `src-tauri/target/release/bundle/macos/Ramify.app` (7.2 MiB, ad hoc signature). With a double click in Finder it opens without a repository: type the path in the bar.

### Real IPC of the new commands

**Updated on 2026-11-01:** `scripts/smoke_ipc.py` runs the commands through the real IPC, in WebKit and in release, against a throwaway repository and with its own `HOME`: 125 of 127 commands in 136 steps, 0 IPC failures (`validation-ipc.json`). The 2 operation errors are expected: `branch_rebase_abort` with no rebase in progress and `init_repository` with a folder that does not exist. Not run, on purpose: `clipboard_write` and `open_in_new_window`. The pass is built separately with `VITE_RAMIFY_SMOKE`, and that code does not enter `Ramify.app`. What it does not cover: the arguments are written by the script, not by the dialogs; the real credential questions (`askpass_answer` was only called with an id that does not exist); and the network. That is why the boxes below are still open, although you should no longer find `command … not found` or argument errors.

What follows is the original text of the list. None of the 78 commands new since phase 1 was tested through Tauri's IPC; the tests call the Rust functions and simulate `invoke` in the interface. It is only known that `log_open` and `log_rows`, and those of phase 1 on the measurement's path, work through the real IPC, because the measurement uses them. A failure here would be one of registration in `lib.rs`, of argument names in camelCase, or of serialization. For each row, check that the action does something and that the console or a notification does not show `command … not found` or an argument error.

- [ ] **Log (16 new commands, plus `log_open` and `log_rows`, which already existed):** open the Log tab (`log_open`, `log_rows`, `log_view_open`, `log_refs`, `log_users`, `log_options_get`, `log_options_set`); change the order and collapse (`log_view_set`, `log_graph_click`, `log_graph_hover`); `Go To` and arrows (`log_row_of`, `log_find_rows`, `log_adjacent_rows`); the text filter and its history (`log_recent_filters`, `log_remember_filter`); reload when a branch changes from outside (`log_refs_changed`); the current branch (`log_current_branch`); close a tab (`log_view_close`).
- [ ] **Detail and changes (5):** select a commit and a merge (`commit_details`, `branches_containing`, `commit_changes_of`, `commit_changes_start`); copy the hash (`clipboard_write`).
- [ ] **Tasks, console and credentials (3):** cancel a task (`task_cancel`), the Console tab (`console_lines`) and answer the credentials dialog (`askpass_answer`).
- [ ] **Branches (25):** popup and panel (`branch_list`, `branch_current`, `branch_incoming_outgoing`, `branch_refs_fingerprint`, `branch_options_get`, `branch_options_set`, `favorites_get`, `favorites_toggle`); checkout (`branch_checkout`, `branch_checkout_remote`); `branch_create`, `branch_rename`, `branch_delete` and `branch_restore`; `branch_delete_remote` with `branch_trackers`; `branch_unmerged_commits` (View Commits of the deletion); `branch_rebase_onto` and `branch_rebase_abort`; reset (`branch_reset`, `reset_mode_get`, `reset_mode_set`); tags (`tag_create`, `tag_delete`, `tag_restore`).
- [ ] **Fetch, update and pull (10):** `fetch_all`, `fetch_remote` (reload branches of the Pull dialog), `update_project`, `update_branch`, `update_abort`, `update_restore`, `pull_info`, `git_pull`, `pull_options_get` and `pull_options_set`.
- [ ] **Push (10):** `push_info`, `push_branch_of`, `push_outgoing`, `git_push`, `git_push_update` (the retry after a rejection), `push_tags_get`, `push_tags_set`, `repo_settings_get`, `protected_branches_set` and `force_push_target_remember`.
- [ ] **Remotes, clone and init (9):** `remote_list`, `remote_add`, `remote_edit`, `remote_remove`, `remote_test_url`, `clone_repository`, `init_repository`, `open_exclude_file` and `open_in_new_window`.

### Log pass with synthetic events (2026-11-01)

Done by Claude in release `Ramify.app` on the throwaway lab, with System Events keys and CoreGraphics mouse. The log table is a canvas with no accessibility tree, so the selection is read in the detail panel. No screenshots. **The boxes below are still open:** it is not a physical keyboard or a real repository, and the graph has not been seen.

| Point | Result |
|---|---|
| Click on a row, `↓`, `→`, `←` with popup `Select Child to Navigate`, `Esc` | Good |
| `⌘F` (Go To) with branch and tag; `⌘L` toggles filter and table | Good, after fixing the focus |
| `F1` and `⌃J` (tooltip with references); `Esc` closes it | Good. `F1` was sent as the F1 key: it remains to see what it does on your function row |
| `⌥⌘⇧C` and `Copy Revision Number`: real clipboard | Good, after fixing the shortcut |
| `⌘D`: diff of the commit | Works, but inside the log. With `openInNewWindow = true` it has not been compared with IntelliJ |
| Text filter and branch filter | Good |
| `Branch`, `User`, `Date`, `Paths` popups, View Options and context menu | They open with their entries. `Select…` and the user, date and path filters were not used |
| Branch created from outside | It appears by itself in the panel |
| Branches panel: `F2`, `⇧F6`, `⌘⌥N`, `⌥F1` | Good |
| Branches popup: search and `Space` on a branch | Good |

User's visual review on 2026-11-01: accepted the graph on Retina, the label colors, the blue and gray selection of the table and `F1` on their physical keyboard. They leave no trace that can be checked; the user confirmed they saw them on the Mac itself. The `+` and `-` icons of the staging mode are still unconfirmed: `settings.json` did not change after turning the mode off, and seeing them requires turning it on.

### Native window and WebKit

- [ ] The Git menu of the menu bar: its 13 entries act, and with a dialog open they do nothing. It has no native accelerators: ⌘T, ⌘⇧K and the rest belong to the page's keyboard.
- [ ] The dialogs look right and close properly in WebKit: Push, Force Push, Push Rejected, Pull, Update Project, New Branch, Rename, Reset, Reset Head, Checkout Problem, Tag (both, the log's and the Git menu's), Remotes, Define Remote, Clone, Init and the credentials one (with focus, Enter and Escape).
- [ ] The menus and popups: log context menu, View Options, Go To, the branches popup with submenus, the menu of each branch in the panel and the filters one. The fixed-position submenus and the popup resizing have not been seen in WebKit.
- [ ] The log graph is drawn correctly on a Retina screen: nodes, diagonal edges, arrows, Long Edges, compact and normal labels, and the colors of HEAD, local branches, remote branches and tags.
- [ ] The selection is blue with the window active and gray without focus; the focus rings; the hand cursor over a graph arrow; the tooltips (`Jump to …`, branches, incoming and outgoing).
- [ ] Dragging the dividers of the log panel (table and detail; changes and detail; diff preview) and that the size is remembered on reopening. The divider is 1 px with no enlarged drag zone: it may be hard to hit.
- [ ] The progress bar of fetch, pull, push and clone advances (they carry `--progress`).
- [ ] The notification with buttons (`View Commits`, `Restore Changes`, `Abort`, `Force Push anyway`) and that the sticky ones are not lost.

### Physical keyboard

- [ ] `⌘T` Update Project, `⌘⇧K` Push, `⌥⌘K` Commit and Push, `⌘9` and `⌘0` between tabs.
- [ ] In the log: `⌘R`, `⌘F` (Go To), `⌘L` (text filter), `←` and `→` (child and parent), `F1` and `⌃J` (commit tooltip), `⌥⌘⇧C` (copy hash), `⌘D` (diff).
- [ ] `F1` on a Mac with the function row in media mode arrives as brightness except with `fn`: check what it does.
- [ ] With focus in the branches panel: `F2` and `⇧F6` (Rename, local branch only), `⌥F1` (navigate to the branch) and `⌘⌥N` (New Branch). In the Commit view, `F2` still renames the list.
- [ ] `⌘Q` with an Update or a push in progress: it waits up to 60 s and cancels. Close the window in the middle of an Update and check that what was saved comes back or is offered on reopening.
- [ ] Quitting with `Quit` in the Dock menu during an Update with saved changes: it must wait, or on reopening the card with what is pending must appear. The Dock does not go through `finish_writes`.
- [ ] `⌘Q` with unsaved text in the diff window while a Write task runs: it may be rejected at 10 s; check that the text is not lost.
- [ ] In the branches panel: Enter filters; arrows walk and collapse; Space toggles favorite in the popup.

### Real network, ssh and https

With real credentials. Use a branch of yours, not `main`.

- [ ] Fetch, Update Project and Pull over https with the system credential helper (`osxkeychain`) with the password already saved: no dialog must appear.
- [ ] The same with an unsaved password or token: the app's dialog appears, not a system dialog or anything hung. Accepting saves to the helper; canceling kills git.
- [ ] ssh with a key without a passphrase and with the agent (`ssh-add`), with a key with a passphrase without the agent (passphrase dialog) and with an unknown host (the trust question). Canceling the ssh dialog may retry up to 3 times (minor pending item of Task 2).
- [ ] An app opened from Finder brings a different environment than one opened from the terminal: `SSH_AUTH_SOCK` and `PATH`. Try ssh with the agent opened from Finder (it uses `env::var_os`, not `EnvironmentUtil`).
- [ ] Push over https and ssh: new branch (upstream), existing branch, with tags `All` and `Current branch`, and with a `pre-push` hook (`Run Git hooks` checkbox).
- [ ] Push rejected because of others' commits: merge, rebase and `Auto-update`, with the rejection dialog and with the `Remember` checkbox.
- [ ] Force push on a branch of yours, with lease and with the warning; and that a protected branch (`main`, `master`) ends up with `Force push to … is prohibited`. GitHub applies its own protection rules: the server may reject the push, and the app does not know them (decision 7). See how that rejection is shown.
- [ ] Clone over https and over ssh, with and without submodules; with a submodule that fails it must keep the clone and warn.
- [ ] The background `ls-remote` (incoming in the popup and the panel): only after a successful fetch, pull or push in that session. It must not bring up system dialogs. Try `Auto`, `Always` and `Never` in Settings.
- [ ] `Use credential helper` off: with a system helper it must no longer use it.
- [ ] A slow or down remote: the popup must open without waiting for it, and canceling kills the process (`task_cancel`).
- [ ] After a fetch over ssh with many branches, that the log and the branches panel update.

### Windows and startup

- [ ] `open -n …/Ramify.app --args /path`: opens a new window even if the app is already open, with that repository. It is what `Git > Clone…` does with an open repository, and `Create Git Repository…` from another folder.
- [ ] Double click on `Ramify.app` from Finder. It is an ad hoc signature: if macOS blocks it, "Open Anyway" (System Settings > Privacy & Security).
- [ ] A hook (`pre-commit`, `pre-push`) that uses `node` or `pnpm` works opened from Finder. Without the terminal's `PATH` it may not find them (already noted in phase 1).
- [ ] Two windows with different repositories at the same time: Update in one does not affect the other. The refreshes (`repo-changed`) are emitted to all windows (minor pending item of Task 3): check that they do not get in the way.
- [ ] `Open .git/info/exclude` opens it with the system text editor.

### Operations on a test repository

- [ ] Smart Checkout with overlapping changes, with staging area off and on; and a Force Checkout (it must leave a copy in the saved-copies dialog of the Commit view).
- [x] Delete of an unmerged branch (it forces itself and warns), `Restore` from the notification, and `View Commits` of what was discarded.
- [x] Update with merge and with rebase, with overlapping local changes; one that gives a conflict (the repository must be left as git leaves it, with `Abort` and `Restore Changes`).
- [x] While Update has your work saved, write to the file from another program (what Devin does): restoring must refuse, naming the file, without touching it, and the stash entry must remain.
- [x] Cancel an Update halfway: a merge comes back with `reset --merge`, a rebase with `rebase --abort`, and what was saved comes back.
- [ ] Reset with the four modes; `Hard` without a second confirmation and with a copy of what is modified and of the unversioned files it overwrites.
- [ ] Filter the log by branch, user, date, path and text with regex; the log of a large repository.

### Operations pass with synthetic events (2026-11-01)

Done by Claude in release `Ramify.app` on the throwaway lab, with its local bare remote, and checked with git after each step. **The boxes above are still open for what is missing.**

| Scenario | Result |
|---|---|
| Update Project with merge and local changes | Good: `1 file updated in 1 commit`, changes intact and no stash |
| Push with `⌘⇧K` | Good: the commit reaches the remote |
| Push rejected by someone else's commit → `Merge` | Good: it merges, retries and pushes; it warns about what was received |
| Smart Checkout with overlapping changes | Good: the changes remain, with no stash entry |
| Force Checkout | Good: it discards the 3 modified files, and the 3 stay in the copy with their content |
| Delete of an unmerged branch → `Restore` | Good: `Unmerged commits were discarded`, and the branch comes back |
| Reset Hard | Good: no second confirmation, and what was uncommitted stays in the copy |
| Reset Soft | Good: `Reset successful` |
| Update with conflict and local changes | Good: git is left with `UU`, yours in the stash, and a warning with `Restore Changes` and `Abort` |
| `Abort` → `Restore Changes` | Good: `Merge abort succeeded`, and afterwards what was saved comes back and the stash is emptied |

| Update with rebase and `View Commits` | Good: the local commit stays on top, the changes come back and the `Update Info` tab opens |
| File written from outside with yours saved | Good: `Restore Changes` refuses, names `b.txt`, does not touch Devin's work and the stash remains. After removing it, it restores |
| Cancel Update with rebase (in `pre-rebase`) | Good: it kills the hook, no half-done rebase is left, `Update canceled` |
| Cancel Update with merge (in `pre-merge-commit`) | **It failed; fixed and checked in the app** |
| Force Push on `feature` | Good: it asks for confirmation and the remote is left with the rewritten commit |
| Force Push on `main` | Good: disabled, it opens nothing and the remote does not change. Not seen: its tooltip |
| Reset Keep with overlapping changes | Good: `Git Reset Problem` with `Smart Reset`, which ends in a conflict and keeps yours in the stash |
| Remotes: add and remove with their confirmation | Good |
| Clone from the local remote | Good: it clones and opens another instance with that repository |
| Git menu: `New Branch…`, `New Tag…`, `Reset HEAD…`, `Manage Remotes…`, `Clone…` | Good, after fixing `New Branch…`. The other 8 entries were not clicked from the menu |

Not tested: Reset Mixed in the log, Smart Checkout with staging on, Update with `Don't show again`, `Create Git Repository…`, and what requires a real network or ssh. `scripts/smoke_ipc.py` was not repeated after these fixes: none of them changes a command.

### Live comparison with IntelliJ

Close IntelliJ and open it from a terminal so that it inherits the variables:

```bash
export RAMIFY_GIT_REAL=$(command -v git)
export RAMIFY_GIT_LOG=~/intellij-commands-phase-2.log
"/Applications/IntelliJ IDEA CE.app/Contents/MacOS/idea"
```

Then set `scripts/git-wrapper.sh` as the path of the Git executable in its settings and do the same as in `phase-2-commands.md`.

- [ ] Compare the log with the tables of `phase-2-commands.md`, especially rows 4, 5, 8, 13 and 17.
- [ ] The text of Update's `git stash save` message.
- [ ] The log's row height (24 px), the column widths, the panel sizes and the colors, side by side with screenshots (decision 12).
- [ ] Return IntelliJ's Git path to its previous value.

### Measurement

- [ ] Repeat the measurement of section 1 with the machine at idle and with your repositories, and check the CPU with the diff window open and closed. Done with `git/git` and the laptop's screen ("Measurement with the visible window"); your repositories and the external monitor are missing.
- [ ] The diff window CPU experiment: open the diff, click in the editor, take focus away from it and measure again. If it drops, what repaints is the cursor.

## 5. What the validation found

This was not decided anywhere. What was deliberate is in `phase-2-decisions.md`. The final batch fixed points 1, 2, 3 and 6.

1. **`Git.Branches` carried ⌃⇧`. Fixed.** In IntelliJ, `plugin.xml:150-153` declares it with `control shift BACK_QUOTE` in `$default` and removes it for `meta shift BACK_QUOTE` in `Mac OS X` and `Mac OS X 10.5+`. It was removed from `keymap.ts`: the popup is opened with the widget and the Git menu.
2. **`New Tag…` and `Reset HEAD…` were missing from the Git menu. Fixed.** IntelliJ's `Git.MainMenu` has them (`plugin.xml:156-157`), and the text of `Git.Tag` is `New Tag…` (`GitBundle.properties:557`). `New Tag…` opens the `Tag` variant of `TagDialog`, and `Reset HEAD…` a new `Reset Head` dialog, without `Keep`, like the old `GitResetDialog`.
3. **F2, ⇧F6, ⌥F1 and ⌘⌥N were not bound. Fixed in the branches panel.** In IntelliJ those actions read `GIT_BRANCHES` and `GIT_BRANCH_FILTERS`, which only `BranchesTreePanel` gives (`BranchesDashboardUi.kt:281-290`): they work with focus in the panel, and not in the popup or in the log table. F2 and ⇧F6 are still the list rename in the Commit view.
4. **With the diff window open and a modified file there are about 58 MB more and 3 % of idle CPU**, and the target is a CPU close to 0. It has not been identified what repaints, nor has the diff window been separated from having a modified file. It stays unfixed and as a manual experiment.
5. **Startup is still at 2.5 s until the first paint**, with 1.5 s before the page. It is the same problem as phase 0 and it has not been worked on.
6. **`NOTICE` did not include the license text or the TypeScript ports. Fixed.** Section 4(a) of Apache 2.0 asks to give a copy of the license to whoever receives a derivative work. The text is in `LICENSE-APACHE`, and `NOTICE` and the README also cite what was ported in `src/`.
7. **The adjustment of the sizes and of the row height was left undone.** Decision 12 said it would be adjusted by comparing screenshots, and there are no IntelliJ screenshots.
8. **The interface tests are fragile under load.** The first run of `pnpm vitest run` of this day, with the machine at a 1-minute load above 600, gave 19 failures: 17 for the 5 s limit and 2 for assertions about something that had not yet arrived. With normal load, the next two runs passed all 1,149. The same in Rust: `a_hung_remote_does_not_take_the_others_results_away` failed once on 2026-11-01 when compiling at the same time, with its 500 ms limit, and passed 5 of 5 alone and in the full suite.
9. **`done` without IPC (partly covered by `scripts/smoke_ipc.py`, see section 4).** There is only one test on the command layer, `mutating_without_saving_the_diff_is_not_allowed` (every command of `vcs_commands.rs` that mutates calls `flush_diff_window`). None goes through the complete path: registration in `lib.rs`, argument names of `api.ts`, serialization of the result.

What the reviews deferred (121 minor findings) is in `phase-2-decisions.md`. Those that can most affect your own work:

- Devin writes between the stash snapshot and its `reset --hard` (Task 11): known limit of the window, without a file-system lock.
- A `fast-forward` or clean merge killed halfway leaves half a tree, without losing work (Task 11).
- A damaged repository settings file goes back to the default patterns (`master`, `main`) instead of denying the force push (Task 14).
- A silent task with a hung ssh outlives quitting the app (Task 10), and read tasks are not cancelled on quitting (Task 1).
- `update::abort` uses `reset --merge` in any state without `MERGE_HEAD` and says `Pull abort succeeded` (Task 12).
- The `git branch -d` that `delete_remote` launches for the local branch goes as `TaskKind::Remote`, without `RepoLocks`, and `update_branch`, a fetch to a branch that is not the current one, does not take them either: narrow race with a checkout (Tasks 9 and 11). `branch_delete` does go as `TaskKind::Write`.

Found in the pass of 2026-11-01, and fixed with TDD and checked in the real app:

- **`⌘9` left the focus on the page**, and `⌘L`, `⌘F`, the arrows and `⌘D` did not respond until clicking on the table. It also happened when closing the preview. The tests sent the key directly to the table and did not see it.
- **`⌥⌘⇧C` did not copy with the Spanish - ISO keyboard**, where ⌥⇧C gives a space and not `Ç`.

- **`New Branch…`, `Rename` and `Checkout` with a requested name did nothing until the Log tab was opened.** They asked `log_refs` for the names, which fails without the log loaded, and the app starts in Commit. Now they come from `branch_list`. It was the minor finding of Task 9 `await names() outside attempt`.
- **Cancelling an Update merge left the tree halfway.** A `git merge` killed before writing `MERGE_HEAD` (for example in its hook) leaves no signal, `abort` did nothing and the restore was applied on top: what was merged in the stage and a conflict in the saved file. No work was lost, because the stash was still there. Now `reset --merge` always runs, with a prior copy, because Update starts with the index equal to HEAD. It was the minor finding of Task 11 `half a tree after killing a fast-forward or clean merge`.
- **WebKit applied macOS substitutions in the text fields:** `--` turned into a dash and the first letter into a capital, also in branch and remote names. Only the commit message and a few fields carried `spellcheck="false"`. Now every field is left without them on receiving focus (`src/lib/plainInputs.ts`).
- **`a_hung_remote_does_not_take_the_others_results_away` failed in 2 of 3 full suites.** Its 500 ms limit was short with the tests in parallel: it is now 4 s, and two suites in a row pass.

- **Cancelling a `Pull` halfway had the same window as Update.** Pull saves nothing, so it now depends on the stage: if it was empty before, it is undone with `reset --merge` and a prior copy; if there was something and the index did not change, it is not touched and there is no warning; if it changed, it is not touched and `Pull canceled` comes out with the warning. The first case was checked in the app; the other two, with real git tests. A limit that Pull and Update share: if git dies while writing files, before updating the index, those files remain as local changes. Nothing is lost, but `reset --merge` does not undo them.
- **`run_mutating_does_not_retry_other_errors` measured time and failed once with the suite in parallel.** It now counts the attempts with a hook; with a mutation that always retries, it detects 6 against 1.

Not fixed, to be decided:

- **`Directory` is not filled in Clone.** IntelliJ deduces it from the URL (`DvcsCloneDialogComponent.kt:68`) and remembers the parent folder; here it has to be typed.
- **`⌘D` in the log opens the diff inside the window,** with `openInNewWindow = true` in your settings. Not compared with IntelliJ.

Not fixed: `⌃Espacio` does not reach the app while macOS has it for switching the input source. With `⌘D` and `Show Diff Preview` on at the same time, turning the option off does not close the diff, and the `Close` button remains.

## 6. Known differences with IntelliJ

### The decided ones

They are at the beginning of `phase-2-decisions.md`.

### The ones not compared

- `Select in Tree…`, `Compare with…`, `Show Diff with Working Tree` and `Pull into…` do not exist in the menus, on purpose, until phases 3 and 4. The app has no editor, no shelf and no file history.
- The options of `git pull` come out in the command in the enum's order and not in the order in which you choose them (Task 12).
- With a rejected tag, the notification of a force push says `Force pushed` without mentioning it (Task 14).
- Chosen without comparing: the log's row height, 24 px (decision 12).

## Conclusion

The behavior that can be tested without a network, without a screen and without a keyboard is tested, and matches what IntelliJ's code says, without live comparison, in the git commands it runs, except for the decided differences. The measurement shows the scroll and the memory, and not the startup or the idle CPU with the diff window open. What decides whether the phase is finished nobody has been able to see yet: the real IPC, the network with credentials, the dialogs and the shortcuts in the real window. Until you complete the list in section 4, phase 2 is not validated.
