# Diff Change Stripe and Branch Sync Icons Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Two things IntelliJ IDEA CE 2024.1 shows and Ramify does not:
1. Along the scrollbar edge of each diff editor, a mark for every changed block, so the user sees at a glance where a file changed and can click to go there.
2. Next to the branch name in the title bar, the incoming arrow when the current branch has commits to pull, and the outgoing arrow when it has commits to push.

**Architecture:**
- **Stripe.** A pure function in `src/diff/stripe.ts` turns the blocks and the editor's geometry into marks. A small component, `src/diff/ChangeStripe.svelte`, draws them in a thin lane at the editor's scrollbar edge and scrolls the editor on a click. `FilePane.svelte` places one lane per editor:
  - the right editor on its right edge;
  - the left editor on its left edge, as 241 puts its scrollbar there;
  - the only editor of the unified view on its right edge.
- **Icons.** `App.svelte` reads `branchIncomingOutgoing()` for the current branch, on open, on `repo-changed` and on the existing `branch-sync-changed` event. It then draws the `incoming` and `outgoing` icons, which Ramify already has, after the branch name.

No Rust changes.

**Tech Stack:** Svelte 5, TypeScript, CodeMirror 6, Vitest with jsdom.

**Spec:** none written. The binding reference is 241's source, tag `idea/241.14494.240`, under `.superpowers/ref-idea-241/ic/`:
- `platform/diff-impl/src/com/intellij/diff/util/DiffDrawUtil.java`:
  - `:597-633`: every changed block that has a background also gets a stripe highlighter over its lines (`STRIPE_LAYER`);
  - `:874-887` `DiffStripeMarkerTextAttributes`: its colour is the diff type's marker colour.
- `platform/diff-impl/src/com/intellij/diff/tools/util/side/TwosideTextDiffViewer.java:114` and `ThreesideTextDiffViewer.java:116`: the left editor's vertical scrollbar, which carries its stripe, is on the left.
- `plugins/git4idea/src/git4idea/ui/toolbar/GitToolbarWidgetAction.kt:75-99`: the widget's right icons are `DvcsImplIcons.Incoming` when `syncStatus.incoming` and `DvcsImplIcons.Outgoing` when `syncStatus.outgoing`. Both are shown while the registry key `vcs.new.ui.main.toolbar.actions` is off, its default (`registry.properties:883`).
- `plugins/git4idea/src/git4idea/branch/GitBranchSyncStatus.kt:16-24`: the status is that of the current branch, from `GitBranchIncomingOutgoingManager`.
- `plugins/git4idea/src/git4idea/ui/branch/GitCurrentBranchPresenter.kt:28-58`: the tooltip says only the branch, `Git Branch: <name>`; the icons add no text.

## What 241 does

1. **The stripe.** In a text diff, each block of changed lines has a mark on its editor's error stripe, at the height that its lines take in the whole document, in the block's colour: inserted, deleted, modified, or conflict in a three-side diff. A side with no lines for a change, such as the old side of an insertion, still gets a mark at its place: the empty range has no background, but its stripe highlighter is added anyway (corrected after the final review). The centre pane of a three-side diff has no marks (`ThreesideDiffChangeBase`: `withHideStripeMarkers(side == BASE)`). A click on the stripe scrolls the editor to that height.
2. **Where.** In side-by-side, the left editor's stripe is on its left edge and the right editor's on its right edge. In the unified view, the single editor has it on its right edge.
3. **The widget.** After the current branch's name in the title bar's Git widget: the incoming icon if the branch has commits on its upstream that it lacks, the outgoing icon if it has commits its upstream lacks. Neither shows on a detached HEAD or with nothing to report.

## Global Constraints

- Everything written to the repository is in English. No new user text: the icons have none, and the widget's tooltip stays as it is.
- Colours: the existing tokens `--diff-inserted-stripe`, `--diff-deleted-stripe`, `--diff-modified-stripe` and `--diff-conflict-stripe` (DIFF_* error stripe colours of the Darcula scheme, `src/lib/theme.ts:114-120`).
- The widget runs no new git command: `branch_incoming_outgoing` already exists and honours the `Check incoming commits on remotes` setting. Reading it is not a write: no `flush_diff_window`, no lock.
- Load rules: `nice -n 15`, `vitest --maxWorkers=2`, `CARGO_BUILD_JOBS=6`, `--test-threads=6`, one building or testing agent at a time.

## Decisions (Ramify's calls, each with its cost if wrong)

1. **The lane is 8 points wide and sits over the scrollbar edge,** on top of the editor. 241's thin stripe is part of its own scrollbar, and Ramify uses the system scrollbar, which macOS hides when idle. Cost: the lane covers 8 points of the editor's edge.
2. **A mark is at least 3 points tall,** so a one-line change in a long file stays visible. The minimum height is Ramify's.
3. **The heights follow the document's lines, not their pixel heights.** A block's mark goes from `start / lineCount` to `end / lineCount` of the lane. With folded unchanged lines or wrapped lines, 241 maps visual heights instead. Cost: with folding on, marks sit where the lines are in the file, not where they show on screen.
4. **A click scrolls so that the clicked line is at the top of the editor;** in side-by-side the other editor follows through the existing scroll sync. 241 centres it. Cost: a slightly different landing place.
5. **Scope of the stripe:** the two-side and unified views of `FilePane` (the diff window and every diff preview) and the three-side view of `ThreeVersionsPane`. Left out: the merge window and the `Compare with Clipboard` texts diff (`TextsDiff`, `RevisionsPreview`). Cost: no stripe there yet.
6. **The widget's icons refresh** on open, on `repo-changed` and on `branch-sync-changed`. A failed read shows no icons and says nothing, as the popup does.

## Review Focus

1. A file with one change at the very end, or with thousands of lines: the mark is visible (decision 2) and inside the lane.
2. Changing the ignore policy or the highlight mode, or switching side-by-side ↔ unified: the marks follow the new blocks and the new layout, with no stale lane.
3. An empty side (an added or deleted file): no division by zero, and no mark on the empty side.
4. Switching repository in the window, or a detached HEAD: the icons of the previous branch do not stay.
5. `Check incoming commits on remotes` set to Never: only the outgoing icon can show, as `branch_incoming_outgoing` already decides.

---

### Task 1: The marks, as a pure function

**Files:** Create `src/diff/stripe.ts`, test `src/diff/stripe.test.ts`.

**Interfaces:**
- Produces:
  - `type StripeKind = "inserted" | "deleted" | "modified" | "conflict"`;
  - `interface StripeMark { top: number; height: number; kind: StripeKind; line: number }`, where `top` and `height` are fractions of the lane, from 0 to 1, and `line` is the block's first line;
  - `stripeMarks(blocks: { start: number; end: number; kind: StripeKind }[], lineCount: number, laneHeight: number): StripeMark[]`, with blocks given in that side's 0-based lines, end exclusive;
  - `lineAtLane(y: number, laneHeight: number, lineCount: number): number`.

- [ ] **Step 1: Write the failing tests:**
  - `a_block_takes_its_share_of_the_lane`;
  - `a_one_line_change_in_a_long_file_is_at_least_three_points`;
  - `a_mark_at_the_end_stays_inside_the_lane`;
  - `an_empty_block_has_no_mark`;
  - `an_empty_document_has_no_marks`;
  - `the_kinds_keep_their_colour_name`;
  - `a_click_maps_back_to_a_line`.
- [ ] **Step 2:** Run `nice -n 15 pnpm exec vitest run --maxWorkers=2 src/diff/stripe.test.ts`. Expected: FAIL.
- [ ] **Step 3:** Implement it.
- [ ] **Step 4:** Run the same command. Expected: PASS.
- [ ] **Step 5:** Commit: `feat: the marks of the diff change stripe`.

### Task 2: The lane in the diff views

**Files:**
- Create: `src/diff/ChangeStripe.svelte`, with props `marks: StripeMark[]`, `edge: "left" | "right"` and `onjump: (line: number) => void`.
- Modify: `src/diff/FilePane.svelte`, and `src/diff/editor.ts` if a getter is needed for the blocks of each side.
- Modify: `src/diff/ThreeVersionsPane.svelte`.
- Test: `src/diff/DiffView.test.ts` or the suite that renders `FilePane`, and `src/diff/ThreeVersionsPane.test.ts`.

- [ ] **Step 1: Write the failing tests:**
  - `side_by_side_has_a_lane_on_each_outer_edge_with_one_mark_per_block_side`;
  - `an_insertion_has_no_mark_on_the_old_side`;
  - `the_unified_view_has_one_lane_on_the_right`;
  - `clicking_a_mark_scrolls_that_editor_to_its_line`;
  - `changing_the_ignore_policy_redraws_the_marks`;
  - `the_three_side_view_has_marks_with_the_conflict_colour`.
- [ ] **Step 2:** Run the two suites. Expected: the new tests FAIL.
- [ ] **Step 3:** Implement it. The lane is `position: absolute` over the editor's edge, at the full height of its scroll area, `role="presentation"`. Each mark is a `div` with `background: var(--diff-<kind>-stripe)`. The marks are recomputed whenever the blocks or the document change.
- [ ] **Step 4:** Run the two suites. Expected: PASS, the old tests included.
- [ ] **Step 5:** Commit: `feat: the diff editors show their changes on the scrollbar edge`.

### Task 3: Incoming and outgoing icons on the branch widget

**Files:** Modify `src/App.svelte`. Test `src/App.test.ts`.

- [ ] **Step 1: Write the failing tests:**
  - `the_branch_widget_shows_incoming_when_the_current_branch_has_commits_to_pull`;
  - `it_shows_outgoing_too_and_both_together`;
  - `it_shows_neither_when_synced_or_detached`;
  - `branch_sync_changed_and_repo_changed_refresh_the_icons`;
  - `a_failed_read_shows_no_icons`;
  - `the_widget_label_and_tooltip_do_not_change`.
- [ ] **Step 2:** Run `nice -n 15 pnpm exec vitest run --maxWorkers=2 src/App.test.ts`. Expected: FAIL.
- [ ] **Step 3:** Implement it. A `$state` `sync` holds the current branch's `BranchSync`, read through `branchIncomingOutgoing()` keyed by `repoNow.branch`. A read started for an earlier repository or branch is dropped. The widget renders `<Icon name="incoming" />` and `<Icon name="outgoing" />` after the label, with the same blue as in the branches popup.
- [ ] **Step 4:** Run the same command. Expected: PASS.
- [ ] **Step 5:** Commit: `feat: the branch widget shows incoming and outgoing commits`.

### Task 4: Docs, verification, review and build

- [ ] **Step 1:** Add two rows to `docs/parity-matrix.md`:
  - "Change marks on the diff editors' scrollbar edge (`DiffDrawUtil` stripe)", in the Commit and diff group;
  - "Incoming and outgoing icons on the title bar's Git widget (`GitToolbarWidgetAction`)", in the Log and branches group.

  Each row lists its tests and the decisions.
- [ ] **Step 2:** Add a part to `docs/reports/phase-4-validation.md` with:
  - what was built;
  - decisions 1-6 with their costs;
  - a manual checklist:
    1. a long file with a change at the end;
    2. side-by-side and unified;
    3. a click on a mark;
    4. a branch behind its upstream after a fetch;
    5. a branch ahead after a commit;
    6. a detached HEAD;
  - what could not be checked.
- [ ] **Step 3: Full verification:**
  - `pnpm exec tsc --noEmit -p .`
  - `nice -n 15 pnpm vitest run --maxWorkers=2`
  - `CARGO_BUILD_JOBS=6 nice -n 15 cargo clippy --all-targets -- -D warnings`
  - `CARGO_BUILD_JOBS=6 nice -n 15 cargo test -- --test-threads=6`
  - `python3 -I -m unittest discover -s scripts`

  Expected: all green, counts in the report.
- [ ] **Step 4:** One independent final review on the most capable model, then one fix pass with tests that fail first.
- [ ] **Step 5:** Commit the docs, and build `Ramify.app` with `CARGO_BUILD_JOBS=6 nice -n 15 pnpm tauri build --bundles app`.
