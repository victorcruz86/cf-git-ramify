# Resizable Log Columns Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** In the commit table of the Log, the user drags the border between two columns to change a column's width, and a double click on the border goes back to the automatic width, as in IntelliJ IDEA CE 2024.1. Each log tab remembers its widths.

**Architecture:**
- Pure functions in `src/log/draw.ts` do the geometry:
  - the widths in use (saved or automatic);
  - which border lies under a point;
  - the new widths while a border is dragged.
- `LogView.svelte` runs the gesture on its canvas:
  - the resize cursor, and the press and drag;
  - the double click that resets a column.
  - It reports the saved widths through a new optional callback.
- The tab keeps the widths in its `LogOptions`, which `LogPanel` already saves per tab with `logOptionsSet`.

No Rust changes.

**Tech Stack:** Svelte 5, TypeScript, Vitest with jsdom.

**Spec:** none written. The binding reference is 241's source, tag `idea/241.14494.240`, under `.superpowers/ref-idea-241/ic/platform/vcs-log/impl/src/com/intellij/vcs/log/`:
- `ui/table/VcsLogGraphTable.java`:
  - `:146`, the `InvisibleResizableHeader`, under which every column but Root can be resized;
  - `:374-397` `updateDynamicColumnsWidth`, where a saved width wins unless it is ≤ 0 or wider than the table, and Commit takes the rest;
  - `:795` `BORDER_THICKNESS = 3`;
  - `:800-828`, where a double click on a border resets the width; a column left of Commit uses its right border, one right of Commit its left border;
  - `:840-847` `isOnLeftBorder` / `isOnRightBorder`;
  - `:995-1012`, where a width is saved while a column is being resized.
- `ui/table/column/VcsLogColumnProperties.kt:20`: the property `Table.<id>.ColumnIdWidth`.
- `impl/VcsLogUiPropertiesImpl.kt:128` and `history/FileHistoryUiProperties.kt:142`: `COLUMN_ID_WIDTH` belongs to each log UI's state, and file history has its own.
- `ui/table/column/VcsLogDefaultColumn.kt`: Root is fixed (`isResizable = false`), and so is the signature icon column (`VcsLogExternalStatusColumn.kt:13`).

## What 241 does

1. Commit (Ramify's `subject`) takes whatever width the other columns leave. Author, Date and Hash are "dynamic": automatic from their content, or the width the user set.
2. The table has no visible header. A border between columns is dragged from the table itself, within 3 points of it.
3. A width set by dragging is saved in that log's state and used again, unless it is wider than the whole table.
4. A double click on a column's border clears its saved width, and it goes back to automatic. A column left of Commit uses its right border, one right of Commit its left border.
5. Root and the signature column cannot be resized.

## Global Constraints

- Everything written to the repository is in English.
- No new confirmation, and no new text in the interface: 241 shows none.
- Saved widths are Ramify's own data, stored with the tab's options. Nothing is written to `.idea/` or to IntelliJ's settings.
- Load rules: `nice -n 15`, `vitest --maxWorkers=2`, `CARGO_BUILD_JOBS=6`, `--test-threads=6`, one building or testing agent at a time.

## Decisions (Ramify's calls, each with its cost if wrong)

1. **Only the column next to the border changes.** A border between Subject and a column changes that column, and Subject takes the difference. A border between two columns, such as Author|Date, makes the left one wider and the right one narrower by the same amount. Swing's default spreads the difference over every column to the right. Cost: a slightly different feel when the Date column comes after Author.
2. **Minimum width 15 points** (Swing's `TableColumn` default). Subject never gets narrower than 15 points through a drag.
3. **The width is saved when the drag ends,** in one write. 241 saves on every change during the drag. Cost: none visible.
4. **A press within 3 points of a border starts a resize and does not change the selection.** Hovering there shows the `col-resize` cursor on any row. Cost: a click right on a border no longer selects that row.
5. **Each tab keeps its own widths, file history tabs included.** Ramify already stores every option per tab. 241 shares one width set among all file history tabs. Cost: a width set in one file history tab does not carry to the next one.
6. **Out of scope:**
   - Reordering columns by dragging. The class that would do it in 241 (`InvisibleResizableHeader`) is not in our copy of the source, so it is not verified.
   - The tab of Compare Branches keeps widths only while it is open: its options are not saved today.

## Review Focus

1. A saved width wider than the table, for example after making the window narrower, is ignored: the column goes back to automatic and Subject is not squeezed to 0.
2. Hiding a column and showing it again: its saved width comes back. Showing the signature column adds no border to drag.
3. A double click on a border resets only that column. A double click on a row away from a border still does what it did.
4. While dragging, the graph and the labels follow Subject's new left edge (`subjectX`) with no stale frame.
5. Old saved options without widths load as automatic. Bad values (a string, a negative number, NaN) are ignored.

---

### Task 1: The geometry, as pure functions

**Files:**
- Modify: `src/log/draw.ts`
- Test: `src/log/draw.test.ts`

**Interfaces:**
- Produces:
  - `export type SavedWidths = Partial<Record<"author" | "date" | "hash", number>>`.
  - `export const RESIZABLE: ReadonlySet<LogColumn>` = author, date, hash.
  - `effectiveWidths(auto: ColumnWidths, saved: SavedWidths | undefined, tableWidth: number): ColumnWidths`: a saved width is used when it is a finite number > 0 and ≤ `tableWidth`; otherwise the automatic one.
  - `interface Border { column: "author" | "date" | "hash"; x: number; neighbour: LogColumn }`: the column the border resizes, its x, and the column on the other side.
  - `borderAt(layout: TableLayout, columns: LogColumn[], x: number): Border | null`: the border within `BORDER_THICKNESS = 3` of `x`. A column left of Subject resizes by its right border, one right of Subject by its left border. Borders of `signature`, and the outer edges, give null.
  - `dragWidths(start: ColumnWidths, border: Border, delta: number, tableWidth: number, columns: LogColumn[]): SavedWidths`: the widths after moving the border by `delta`, following decisions 1 and 2. Only the columns that changed are returned.
  - `readSavedWidths(stored: unknown): SavedWidths`: keeps only the three keys, each a finite number > 0.

- [ ] **Step 1: Write the failing tests** in `src/log/draw.test.ts`:
  - `a_saved_width_wins_over_the_automatic_one`;
  - `a_saved_width_wider_than_the_table_is_ignored`;
  - `zero_negative_and_nan_are_ignored`;
  - `the_border_right_of_subject_resizes_the_column_after_it`, for Subject, Author, Date;
  - `a_column_left_of_subject_resizes_by_its_right_border`, for the history order Author, Date, Subject;
  - `three_points_from_a_border_is_on_it_and_four_is_not`;
  - `the_signature_column_has_no_border`;
  - `dragging_the_subject_border_changes_only_that_column`;
  - `dragging_between_author_and_date_moves_width_from_one_to_the_other`;
  - `a_column_does_not_go_under_15_and_subject_keeps_15`;
  - `read_saved_widths_keeps_only_known_positive_numbers`.
- [ ] **Step 2:** Run `nice -n 15 pnpm exec vitest run --maxWorkers=2 src/log/draw.test.ts`. Expected: the new tests FAIL.
- [ ] **Step 3:** Implement the functions in `draw.ts`. `tableLayout` itself does not change.
- [ ] **Step 4:** Run the same command. Expected: PASS.
- [ ] **Step 5:** Commit: `feat: the geometry of resizable log columns`.

### Task 2: The gesture in the log table

**Files:**
- Modify: `src/log/LogView.svelte`
- Test: `src/log/LogView.test.ts`

**Interfaces:**
- Consumes: Task 1.
- Produces: new optional props on `LogView`. A host that does not pass `onwidths` still gets resizing, but nothing is saved.
  - `savedWidths?: SavedWidths`;
  - `onwidths?: (widths: SavedWidths) => void`.

- [ ] **Step 1: Write the failing tests**, driving the mouse over the scroller with the canvas measured as the suite already does:
  - `hovering_a_border_shows_the_resize_cursor`;
  - `dragging_the_author_border_widens_author_and_reports_it_once_on_release`;
  - `a_press_on_a_border_does_not_select_the_row`;
  - `a_double_click_on_a_border_resets_that_column`, so that `onwidths` comes without that key;
  - `a_double_click_away_from_a_border_still_selects_as_before`;
  - `saved_widths_are_drawn_and_move_the_graph_with_subject`, checking `subjectX` through the graph click or the draw spy the suite uses;
  - `a_saved_width_wider_than_the_table_draws_the_automatic_one`.
- [ ] **Step 2:** Run `nice -n 15 pnpm exec vitest run --maxWorkers=2 src/log/LogView.test.ts`. Expected: the new tests FAIL.
- [ ] **Step 3: Implement it in `LogView.svelte`:**
  - Every place that calls `tableLayout` with `widths ?? INITIAL_WIDTHS` uses `effectiveWidths(...)` instead, with the saved widths, or the live ones during a drag.
  - `mousemove` asks `borderAt` first. On a border it sets `col-resize` and skips the hover work.
  - `onmousedown` on a border starts the drag, with window listeners for `mousemove` and `mouseup`. It sets a flag so that the `click` that follows does not select.
  - `mouseup` calls `onwidths` once with `{ ...savedWidths, ...dragged }`.
  - `ondblclick` on a border calls `onwidths` without that column.
- [ ] **Step 4:** Run the same command. Expected: PASS, the old tests included.
- [ ] **Step 5:** Commit: `feat: drag a log column's border to resize it, double click to reset`.

### Task 3: Each tab remembers its widths

**Files:**
- Modify: `src/log/options.ts`, which gets `columnWidths?: SavedWidths` in `LogOptions`; `readOptions` passes it through `readSavedWidths`.
- Modify: `src/log/LogPanel.svelte`, `src/log/HistoryTab.svelte` and `src/log/CompareBranchesTab.svelte`, which pass `savedWidths={options.columnWidths}` and `onwidths={(columnWidths) => onoptions({ columnWidths })}`, or `setOptions` in `LogPanel`.
- Test: `src/log/LogPanel.test.ts`, plus a case in `src/log/options.test.ts` if that file exists, otherwise in `LogPanel.test.ts`.

- [ ] **Step 1: Write the failing tests:**
  - `a_resized_column_is_saved_with_the_tabs_options`: `log_options_set` receives `columnWidths` for the main tab;
  - `saved_widths_come_back_when_the_repository_opens_again`;
  - `old_options_without_widths_load_as_automatic`;
  - `bad_stored_widths_are_dropped`.
- [ ] **Step 2:** Run `nice -n 15 pnpm exec vitest run --maxWorkers=2 src/log/LogPanel.test.ts`. Expected: the new tests FAIL.
- [ ] **Step 3:** Implement it.
- [ ] **Step 4:** Run the same command. Expected: PASS.
- [ ] **Step 5:** Commit: `feat: each log tab remembers its column widths`.

### Task 4: Docs, verification, review and build

- [ ] **Step 1:** In `docs/parity-matrix.md`, update the row "Columns Subject, Author and Date…" (Log and branches, phase 2): resizing is done, with the tests, and reordering is not copied (decision 6).
- [ ] **Step 2:** In `docs/reports/phase-4-validation.md`, add a "Resizable log columns" part with:
  - what was built, and decisions 1-6 with their costs;
  - a manual checklist:
    1. drag the Author and Date borders;
    2. double-click a border to reset;
    3. restart the app and check the widths came back;
    4. make the window narrow and check that a saved width wider than the table is ignored;
    5. check that a file history tab keeps its own widths;
  - what could not be checked.
- [ ] **Step 3: Full verification:**
  - `pnpm exec tsc --noEmit -p .`
  - `nice -n 15 pnpm vitest run --maxWorkers=2`
  - `CARGO_BUILD_JOBS=6 nice -n 15 cargo clippy --all-targets -- -D warnings`
  - `CARGO_BUILD_JOBS=6 nice -n 15 cargo test -- --test-threads=6`
  - `python3 -I -m unittest discover -s scripts`

  Expected: all green, with the counts in the report.
- [ ] **Step 4:** One independent final review on the most capable model, then one fix pass with tests that fail first.
- [ ] **Step 5:** Commit the docs, and build `Ramify.app` with `CARGO_BUILD_JOBS=6 nice -n 15 pnpm tauri build --bundles app`.
