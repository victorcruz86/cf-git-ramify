# Drag and Drop Between Changelists Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** In the Local Changes tree, the user drags files, folders or whole changelists onto another changelist, as in IntelliJ IDEA CE 2024.1, instead of using `Move to Another Changelist...` (⇧⌘M).

**Architecture:** A pure module `src/commit/drag.ts` decides what is dragged, where it lands and whether the drop is allowed. `ChangesTree.svelte` runs the gesture with pointer events: the threshold, the label next to the pointer, the highlighted target row, scrolling at the edges and Escape. `ChangesPanel.svelte` turns a drop into the commands Ramify already has: `changelistsMove` for versioned files, and the `Add to VCS` path for unversioned ones. No Rust changes.

**Tech Stack:** Svelte 5, TypeScript, Vitest with jsdom.

**Spec:** none written. The binding reference is 241's source, tag `idea/241.14494.240`, under `.superpowers/ref-idea-241/ic/platform/vcs-impl/src/com/intellij/openapi/vcs/changes/`:
- `ui/ChangesViewDnDSupport.java`: what a drag carries and what a drop does;
- `ui/ChangesTreeDnDSupport.java`: the drop node, the highlight and the drag image;
- `ui/ChangesBrowserChangeListNode.java:109-135`: when a changelist accepts a drop and what it does;
- `ChangesViewManager.java:488`: support is installed only on the Local Changes tree.
- The drag text is `vcs.dnd.image.text.n.files` in `platform/vcs-api/vcs-api-core/resources/messages/VcsBundle.properties:1188`: `{0} {0, choice, 1#file|2#files}`.

## What 241 does

1. **Where.** Only the Local Changes tree, which is also the tree of the non-modal Commit tool window. The Commit dialog's browser has no drag support, and neither does the staging area's tree in this plan (decision 5).
2. **What is dragged.** A move gesture (not copy) on the selection. It carries the changes under the selection, the unversioned files under the `Unversioned Files` node, and the ignored files. Ramify shows no ignored files, so it carries the first two. Selecting a folder or a changelist row carries every file under it.
3. **Where it lands.** The row under the pointer is walked up to its top-level row. A file inside a list lands on that list. A drop below the last row has no target.
4. **When it is allowed.** The drag must start in the same tree. Only a changelist row accepts. It refuses when any dragged change already belongs to it, so a selection that touches the target list is refused as a whole. `Unversioned Files` and `Merge Conflicts` accept nothing.
5. **What it does.** First, it moves the changes to the list (`moveChangesTo`). Then it adds the unversioned files to Git in that list (`addUnversionedFiles`, which is `Add to VCS`). Nothing asks first.
6. **How it looks.** A label `N file(s)` follows the pointer, above and to the left of it. N counts the files of each selected row, without counting twice a row inside another selected row. The target row gets a rectangle; where a drop is refused there is no rectangle and the pointer shows no-drop.

## Global Constraints

- Everything written to the repository is in English. User texts are 241's: `1 file`, `N files`.
- No new confirmation: 241 asks nothing on a drop.
- A drop runs the same commands as `Move to Another Changelist...` and `Add to VCS`. `Add to VCS` already calls `flush_diff_window`, takes `RepoLocks` and keeps a recoverable copy of the index. The drop adds no new path to git.
- The dragged paths are read again from the current tree when the drop happens. Devin writes files during a drag, so a file that is gone by then is skipped, and a target list that is gone means nothing runs.
- Load rules: `nice -n 15`, `vitest --maxWorkers=2`, `CARGO_BUILD_JOBS=6`, `--test-threads=6`, one building or testing agent at a time.

## Decisions (Ramify's calls, each with its cost if wrong)

1. **Pointer events, not HTML5 drag and drop.** Tauri's window-level file drop (`dragDropEnabled`, on by default) and WebKit's drag image make HTML5 drag and drop unreliable inside the webview, and jsdom cannot test it. Pointer events are testable and match the other drags Ramify has (`CommitDialog.svelte:754`). Cost if wrong: none for the user; it is internal.
2. **Threshold of 5 points** before a press becomes a drag, the AWT default (`awt.dnd.drag.threshold`). Cost: a drag that starts a little earlier or later than in IntelliJ.
3. **Press on a row that is not selected, then drag:** that row alone becomes the selection when the drag starts, as Swing selects on press. Pressing on a row that is already selected keeps the whole selection. The click that follows a drag is ignored, so a drop does not reselect. Cost: none known.
4. **Scrolling at the edges.** While dragging, the tree scrolls when the pointer is within one row of the top or bottom edge. The lists sit above 1236 unversioned files, so without it the target can be out of view. 241 gets this from its drag manager; the speed is Ramify's. Cost: a different scroll speed.
5. **Out of scope, each with its cost:**
   - The staging area's drag between Staged and Unstaged (`GitStageTree.kt`): you use changelists; Stage and Unstage stay on their buttons and shortcuts.
   - Dragging onto the Shelf, and from the Shelf onto a changelist: parity-matrix row "Drag and drop between Local Changes and the Shelf" stays `out`.
   - Dragging to the `Ignored Files` node: Ramify does not show it.
   - A copy gesture (⌥ while dragging): in 241 it only matters for shelves.
6. **Escape cancels** a drag in progress and nothing runs. Releasing outside the tree also runs nothing.

## Review Focus

1. A selection mixing a file of the target list with files of other lists: the whole drop is refused, nothing moves.
2. A drop while a refresh rebuilt the tree (Devin wrote a file mid-drag): only paths still present move; a list deleted meanwhile means nothing runs.
3. Unversioned files dropped on a list that is not the active one: they are added to Git and end up in that list, not in the active one.
4. Dragging the header of a changelist onto another: all of its files move; the empty list stays (no removal offer, as 241's `moveChangesTo`).
5. A plain click, a double click and a ⌘/⇧ click still select and open as before; a press that moves less than 5 points is a click.

---

### Task 1: The drag rules, as pure functions

**Files:**
- Create: `src/commit/drag.ts`
- Test: `src/commit/drag.test.ts`

**Interfaces:**
- Consumes: `TreeNode`, `NodeKind`, `UNVERSIONED_ID`, `CONFLICTS_ID` from `src/commit/tree.ts`.
- Produces:
  - `dropRoot(roots: TreeNode[], id: string): TreeNode | null`: the top-level node that contains the node `id` (itself if top-level), or null.
  - `interface DragLoad { versioned: { path: string; list: string }[]; unversioned: string[] }`.
  - `dragLoad(roots: TreeNode[], selected: Set<string>): DragLoad`: every file under the selected nodes, once. `list` is the id of the top-level changelist the row sits in. Files under `Unversioned Files` go to `unversioned`. Files under `Merge Conflicts` count as versioned, with `list` set to `""`.
  - `dragCount(roots: TreeNode[], selected: Set<string>): number`: 241's `getSelectionCount`, i.e. the files of each selected node not inside another selected node.
  - `canDrop(target: TreeNode | null, load: DragLoad): boolean`: true only for a `changelist` node, when no `versioned` entry has `list === target.list.id` and the load is not empty.
  - `dragText(count: number): string`: `1 file` / `N files`.

- [ ] **Step 1: Write the failing tests** in `src/commit/drag.test.ts`, built with `buildTree` from `tree.ts` on two lists `Changes` (active) and `VCA NO COMMIT`, plus unversioned files:
  - `drop_root_walks_a_file_up_to_its_list`, and the same for a folder row when grouping by directory;
  - `drop_root_of_an_unknown_id_is_null`;
  - `the_load_of_a_list_row_is_every_file_in_it`;
  - `a_file_selected_with_its_folder_is_carried_once`;
  - `unversioned_files_are_carried_apart`;
  - `count_does_not_count_a_row_inside_a_selected_row_twice`;
  - `only_a_changelist_accepts`, which checks that `Unversioned Files`, `Merge Conflicts` and null refuse;
  - `a_load_touching_the_target_list_is_refused_whole`;
  - `unversioned_only_is_accepted_by_any_list`;
  - `an_empty_load_is_refused`;
  - `drag_text_is_1_file_and_2_files`.
- [ ] **Step 2:** Run `nice -n 15 pnpm exec vitest run --maxWorkers=2 src/commit/drag.test.ts`. Expected: FAIL, because the module does not exist.
- [ ] **Step 3:** Implement `src/commit/drag.ts`. Every function walks `roots` recursively; none keeps state.
- [ ] **Step 4:** Run the same command. Expected: PASS.
- [ ] **Step 5:** Commit: `feat: the rules of a drag between changelists`.

### Task 2: The gesture in the tree

**Files:**
- Modify: `src/commit/ChangesTree.svelte`
- Test: `src/commit/LocalChangesView.test.ts`, the suite that renders the Local Changes tree.

**Interfaces:**
- Consumes: `dropRoot`, `dragLoad`, `dragCount`, `canDrop`, `dragText` from Task 1.
- Produces: new optional props on `ChangesTree`. Without `ondrop`, the tree behaves exactly as today.
  - `ondrop?: (target: TreeNode, load: DragLoad) => void`.

- [ ] **Step 1: Write the failing tests**, driving `pointerdown`, `pointermove` and `pointerup` on rows:
  - `dragging_a_file_onto_another_list_highlights_it_and_drops_there`: the target list row gets `data-drop-target`, and `ondrop` receives that list and the file;
  - `a_drop_on_a_file_row_lands_on_its_list`;
  - `a_drop_on_the_source_list_runs_nothing_and_shows_no_highlight`;
  - `a_move_under_5_points_is_a_click`: the selection changes as a click does, and `ondrop` is not called;
  - `dragging_an_unselected_row_selects_it_alone`;
  - `dragging_a_selected_row_keeps_the_whole_selection`;
  - `the_label_says_3_files_next_to_the_pointer`;
  - `escape_cancels_and_releasing_runs_nothing`;
  - `no_click_follows_a_drop`;
  - `the_tree_scrolls_when_the_pointer_nears_an_edge`;
  - `without_ondrop_a_press_and_move_does_not_drag`.
- [ ] **Step 2:** Run `nice -n 15 pnpm exec vitest run --maxWorkers=2 src/commit/LocalChangesView.test.ts`. Expected: the new tests FAIL.
- [ ] **Step 3: Implement it in `ChangesTree.svelte`:**
  - `onpointerdown` on a row records the start point, but not on the chevron, the checkbox or the hover icon. A move past 5 points starts the drag: `setPointerCapture`, and decision 3 for the selection.
  - While dragging, `document.elementFromPoint` finds the row under the pointer, and `dropRoot` gives its top-level node. `canDrop` sets `data-drop-target` on that row, or the no-drop cursor.
  - A fixed label shows `dragText(dragCount(...))`, placed above and to the left of the pointer.
  - Scrolling at the edges follows decision 4.
  - `pointerup` calls `ondrop` only when `canDrop` holds. Escape, or `pointercancel`, ends the drag without calling it.
  - The row style for `data-drop-target` is a 1px rectangle in the theme's focus color, around the row.
- [ ] **Step 4:** Run the same command. Expected: PASS, the old tests included.
- [ ] **Step 5:** Commit: `feat: drag files between changelists in the Local Changes tree`.

### Task 3: The drop runs Move and Add to VCS

**Files:**
- Modify: `src/commit/ChangesPanel.svelte`; `addToVcs` at line 680 is refactored to take the paths and the target list.
- Test: `src/commit/LocalChangesView.test.ts`

**Interfaces:**
- Consumes: `ondrop` from Task 2, and `changelistsMove` and `changesAdd` from `src/lib/api`, which already exist.
- Produces: nothing new for other files.

- [ ] **Step 1: Write the failing tests**, with the API mocked as the suite already does:
  - `a_drop_moves_versioned_files_with_changelists_move_to_the_target`;
  - `a_drop_of_unversioned_files_adds_them_then_moves_them_to_the_target_not_the_active_list`;
  - `a_mixed_drop_moves_first_then_adds`, following 241's order;
  - `a_path_gone_before_the_drop_is_skipped`;
  - `a_target_list_gone_before_the_drop_runs_nothing`;
  - `a_failed_add_shows_add_failed_as_add_to_vcs_does`;
  - `the_commit_dialog_browser_and_the_staging_area_do_not_drag`, which checks that no `ondrop` is passed there.
- [ ] **Step 2:** Run `nice -n 15 pnpm exec vitest run --maxWorkers=2 src/commit/LocalChangesView.test.ts`. Expected: the new tests FAIL.
- [ ] **Step 3: Implement it in `ChangesPanel.svelte`.** Pass `ondrop` only when `!browser && !stagingArea && lists`. On a drop:
  1. Filter the load's paths against the current `changes`, and look up the target list by id in the current `lists`. If the list is gone, return.
  2. If there are versioned paths, `await run(() => changelistsMove(versioned, target.id))`.
  3. If there are unversioned paths, call `addToVcs(unversioned, target.id)`, the refactored function the context menu also uses with the selection and the active list.
- [ ] **Step 4:** Run the same command. Expected: PASS.
- [ ] **Step 5:** Commit: `feat: a drop moves changes and adds unversioned files to the list`.

### Task 4: Docs and full verification

**Files:**
- Modify: `docs/parity-matrix.md`:
  - row 96 keeps `out` for the Shelf, with its text now saying that changelist drag is done and the Shelf is not;
  - a new row in the Commit and diff group: "Drag and drop between changelists in Local Changes", `done`, with the tests of Tasks 1-3 and "Confirm by hand: the drag on a real repository".
- Modify: `docs/reports/phase-4-validation.md`: a short "Drag between changelists" section with decisions 1-6 and their costs, and a manual checklist:
  1. drag a file from `Changes` to `VCA NO COMMIT`;
  2. drag a list's header onto another list;
  3. drag unversioned files onto a list that is not the active one;
  4. try a drop on the source list, and check it is refused;
  5. press Escape during a drag;
  6. drag up to a list that is out of view, through a long `Unversioned Files`.

- [ ] **Step 1:** Write both docs.
- [ ] **Step 2: Full verification:**
  - `pnpm exec tsc --noEmit -p .`
  - `nice -n 15 pnpm vitest run --maxWorkers=2`
  - `CARGO_BUILD_JOBS=6 nice -n 15 cargo clippy --all-targets -- -D warnings`
  - `CARGO_BUILD_JOBS=6 nice -n 15 cargo test -- --test-threads=6`
  - `python3 -I -m unittest discover -s scripts`

  Expected: all green, with the counts reported.
- [ ] **Step 3:** Commit: `docs: drag between changelists in the parity matrix and the validation report`.
- [ ] **Step 4:** Build `Ramify.app` with `CARGO_BUILD_JOBS=6 nice -n 15 pnpm tauri build --bundles app`, so you can try it on your repositories.
