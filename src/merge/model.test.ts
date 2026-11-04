import { ChangeSet, Text } from "@codemirror/state";
import { expect, test } from "vitest";
import type { MergeBlock } from "../lib/api";
import {
  canResolveAutomatically,
  counts,
  MergeEdit,
  initialBlocks,
  mapStates,
  statusText,
  updateRangeOnModification,
  type BlockState,
} from "./model";

const conflict: MergeBlock = {
  left: [1, 2],
  base: [1, 2],
  right: [1, 2],
  kind: "conflict",
  leftChanged: true,
  rightChanged: true,
  auto: null,
};

const change: MergeBlock = { ...conflict, kind: "modified", leftChanged: false };

test("the_center_starts_at_the_ranges_of_the_base_with_nothing_resolved", () => {
  const states = initialBlocks([{ ...conflict, base: [3, 5] }]);

  expect(states).toEqual([
    {
      block: { ...conflict, base: [3, 5] },
      output: { from: 3, to: 5 },
      left: false,
      right: false,
      edited: false,
      oneside: false,
    },
  ]);
});

test("counter_and_status_text", () => {
  const resolved = (state: BlockState, left: boolean, right: boolean) => ({ ...state, left, right });
  const [c, m] = initialBlocks([conflict, change]);
  const states = [c, m, m, m, resolved(m, true, true), resolved(c, true, false), resolved(c, true, true)];

  expect(counts(states)).toEqual({ changes: 3, conflicts: 2 });
  expect(statusText({ changes: 3, conflicts: 1 })).toBe("3 changes. 1 conflict.");
  expect(statusText({ changes: 0, conflicts: 0 })).toBe("No changes. No conflicts.");
  expect(statusText({ changes: 1, conflicts: 0 })).toBe("1 change. No conflicts.");
  expect(statusText({ changes: 0, conflicts: 2 })).toBe("No changes. 2 conflicts.");
  expect(statusText({ changes: 1234, conflicts: 1 })).toBe("1,234 changes. 1 conflict.");
});

// DiffUtil.updateRangeOnModification, without `greedy`.
test("a_range_moves_grows_or_is_damaged_as_in_diff_util", () => {
  // Before, after, inside, completely inside, lower edge, upper edge.
  expect(updateRangeOnModification(1, 3, 5, 6, 2)).toEqual({ from: 1, to: 3, damaged: false });
  expect(updateRangeOnModification(5, 7, 1, 2, 2)).toEqual({ from: 7, to: 9, damaged: false });
  expect(updateRangeOnModification(1, 5, 2, 3, -1)).toEqual({ from: 1, to: 4, damaged: false });
  expect(updateRangeOnModification(2, 3, 1, 4, -2)).toEqual({ from: 2, to: 2, damaged: true });
  expect(updateRangeOnModification(1, 4, 3, 5, -1)).toEqual({ from: 1, to: 3, damaged: true });
  expect(updateRangeOnModification(2, 5, 1, 4, 0)).toEqual({ from: 4, to: 5, damaged: true });
});

test("writing_in_the_block_marks_it_edited_and_going_back_to_the_base_text_unmarks_it", () => {
  const base = Text.of(["x", "b", "y"]);
  const states = initialBlocks([{ ...conflict, auto: "z" }]);
  const typed = ChangeSet.of({ from: 2, to: 3, insert: "U" }, base.length);
  const after = typed.apply(base);

  const edited = mapStates(states, typed, base, after, base);
  expect(edited[0]).toMatchObject({ output: { from: 1, to: 2 }, edited: true });
  expect(canResolveAutomatically(edited[0], "base")).toBe(false);

  const back = ChangeSet.of({ from: 2, to: 3, insert: "b" }, after.length);
  const restored = mapStates(edited, back, after, back.apply(after), base);
  expect(restored[0].edited).toBe(false);
  expect(canResolveAutomatically(restored[0], "base")).toBe(true);
});

// DocumentImpl.replaceString trims what does not change: replacing the text with another almost equal one
// only touches what differs.
test("a_replacement_with_equal_beginning_and_end_only_counts_what_changes", () => {
  const base = Text.of(["a", "X", "B", "c"]);
  const states = initialBlocks([
    { ...conflict, left: [0, 1], base: [0, 1], right: [0, 1] },
    { ...conflict, left: [2, 3], base: [2, 3], right: [2, 3] },
  ]);
  const whole = ChangeSet.of({ from: 0, to: base.length, insert: "a\nX\nb\nc" }, base.length);

  const mapped = mapStates(states, whole, base, whole.apply(base), base);

  expect(mapped.map((state) => state.output)).toEqual([
    { from: 0, to: 1 },
    { from: 2, to: 3 },
  ]);
});

// Adjacent blocks do not come out of merge3, but moveChangesAfterInsertion leaves them like that: deleting the one
// below cannot empty the one above, neither in the middle nor on the last line.
test("deleting_a_block_does_not_empty_the_adjacent_one_above", () => {
  const above = (from: number): MergeBlock => ({
    left: [from, from + 1],
    base: [from, from + 1],
    right: [from, from + 1],
    kind: "modified",
    leftChanged: true,
    rightChanged: false,
    auto: null,
  });
  const deleted = (from: number): MergeBlock => ({
    left: [from, from + 1],
    base: [from, from + 1],
    right: [from, from],
    kind: "deleted",
    leftChanged: false,
    rightChanged: true,
    auto: null,
  });
  for (const [lines, first] of [
    [["x", "a", "b"], 0],
    [["K", "b", "a"], 1],
  ] as const) {
    const base = Text.of([...lines]);
    const texts = { left: base, base, right: Text.of(lines.filter((line) => line !== "a")) };
    const edit = new MergeEdit(base, initialBlocks([above(first), deleted(first + 1)]), texts);

    edit.replace(1, "right", false);

    expect(edit.doc.toString()).toBe(lines.filter((line) => line !== "a").join("\n"));
    expect(edit.states.map((state) => state.output)).toEqual([
      { from: first, to: first + 1 },
      { from: first + 1, to: first + 1 },
    ]);
    expect(edit.states[0].edited).toBe(false);
  }
});

test("a_deletion_that_empties_its_block_marks_it_resolved", () => {
  const base = Text.of(["X", "1", "Ins", "2", "Y"]);
  const deleted: MergeBlock = {
    left: [2, 2],
    base: [2, 3],
    right: [2, 2],
    kind: "deleted",
    leftChanged: true,
    rightChanged: true,
    auto: null,
  };
  const removal = ChangeSet.of({ from: 2, to: 10, insert: "" }, base.length);

  const [state] = mapStates(initialBlocks([deleted]), removal, base, removal.apply(base), base);

  expect(state).toMatchObject({ output: { from: 2, to: 2 }, left: true, right: true });
});
