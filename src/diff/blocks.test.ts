import { describe, expect, test } from "vitest";
import type { Hunk } from "../lib/api";
import { changeType, foldRuns, mapLine, textLines, visibleHunks, wholeFileHunk } from "./blocks";

const hunk = (oldStart: number, oldLines: number, newStart: number, newLines: number): Hunk => ({
  id: `${oldStart}`,
  oldStart,
  oldLines,
  newStart,
  newLines,
});

test("change_type", () => {
  expect(changeType(hunk(1, 0, 1, 2))).toBe("inserted");
  expect(changeType(hunk(1, 2, 1, 0))).toBe("deleted");
  expect(changeType(hunk(1, 1, 1, 2))).toBe("modified");
});

test("lines_of_a_text_as_git_counts_them", () => {
  expect(textLines("")).toBe(0);
  expect(textLines("a")).toBe(1);
  expect(textLines("a\n")).toBe(1);
  expect(textLines("a\nb")).toBe(2);
});

test("file_without_hunks_that_changed_whole", () => {
  expect(wholeFileHunk("a\nb\n", "")).toEqual({
    id: "",
    oldStart: 0,
    oldLines: 2,
    newStart: 0,
    newLines: 0,
  });
  expect(wholeFileHunk("a\n", "a\n")).toBeNull();
});

describe("folding", () => {
  test("leaves_context_around_each_change", () => {
    expect(foldRuns([hunk(20, 1, 20, 1)], 40, 40, 4)).toEqual([
      { old: 0, new: 0, lines: 16 },
      { old: 25, new: 25, lines: 15 },
    ]);
  });

  test("uses_the_lines_of_each_side", () => {
    expect(foldRuns([hunk(5, 0, 5, 3)], 20, 23, 2).at(-1)).toEqual({
      old: 7,
      new: 10,
      lines: 13,
    });
  });

  test("without_changes_does_not_fold", () => {
    expect(foldRuns([], 100, 100, 4)).toEqual([]);
  });

  test("a_short_stretch_is_not_folded", () => {
    const hunks = [hunk(5, 1, 5, 1), hunk(10, 1, 10, 1)];

    expect(foldRuns(hunks, 11, 11, 2)).toEqual([{ old: 0, new: 0, lines: 3 }]);
    expect(foldRuns([hunks[0], hunk(12, 1, 12, 1)], 13, 13, 2)).toEqual([
      { old: 0, new: 0, lines: 3 },
      { old: 8, new: 8, lines: 2 },
    ]);
  });
});

describe("line_mapping", () => {
  const hunks = [hunk(5, 2, 5, 4), hunk(20, 3, 22, 0)];

  test("outside_the_changes_it_shifts", () => {
    expect(mapLine(hunks, 2, "old")).toBe(2);
    expect(mapLine(hunks, 10, "old")).toBe(12);
    expect(mapLine(hunks, 12, "new")).toBe(10);
    expect(mapLine(hunks, 30, "old")).toBe(29);
  });

  test("inside_a_change_it_interpolates", () => {
    expect(mapLine(hunks, 6, "old")).toBe(7);
    expect(mapLine(hunks, 7, "new")).toBe(6);
    expect(mapLine(hunks, 21, "old")).toBe(22);
  });
});

describe("whitespace", () => {
  const OLD = ["a", "  b", "c", "", "d"];
  const NEW = ["a", "b  ", "c", "d"];
  const onlySpaces = hunk(1, 1, 1, 1);
  const blank = hunk(3, 1, 3, 0);

  test("without_ignoring_all_are_visible", () => {
    expect(visibleHunks([onlySpaces, blank], OLD, NEW, "none")).toHaveLength(2);
  });

  test("trim_removes_the_changes_at_the_ends", () => {
    expect(visibleHunks([onlySpaces, blank], OLD, NEW, "trim")).toEqual([blank]);
  });

  test("ignore_whitespace_only_removes_what_is_equal_without_it", () => {
    const inner = hunk(0, 1, 0, 1);
    expect(visibleHunks([inner], ["a b"], ["ab"], "trim")).toEqual([inner]);
    expect(visibleHunks([inner], ["a b"], ["ab"], "whitespaces")).toEqual([]);
    expect(visibleHunks([onlySpaces, blank], OLD, NEW, "whitespaces")).toEqual([blank]);
  });

  test("ignore_empty_lines", () => {
    expect(visibleHunks([onlySpaces, blank], OLD, NEW, "emptyLines")).toEqual([]);
  });
});
