import { describe, expect, test } from "vitest";
import type { Hunk } from "../lib/api";
import {
  effectiveSelection,
  fileCheck,
  hunkCheck,
  hunkPlace,
  hunksAt,
  lineChecked,
  linesSelection,
  marginFor,
  normalizeSelection,
  toggleHunk,
  toggleLine,
} from "./gutter";

// Replacement of 2 lines by 3: old numbering 0-1, new 2-4.
const HUNK: Hunk = { id: "h1", oldStart: 10, oldLines: 2, newStart: 12, newLines: 3 };
const OTHER: Hunk = { id: "h2", oldStart: 30, oldLines: 0, newStart: 33, newLines: 2 };
const GONE: Hunk = { id: "h3", oldStart: 40, oldLines: 2, newStart: 45, newLines: 0 };

test("check_a_line_of_a_whole_hunk", () => {
  const next = toggleLine([{ hunk: "h1", lines: null }], HUNK, 3);

  expect(next).toEqual([{ hunk: "h1", lines: [0, 1, 2, 4] }]);
});

test("check_the_last", () => {
  const next = toggleLine([{ hunk: "h1", lines: [0, 1, 2, 4] }], HUNK, 3);

  expect(next).toEqual([{ hunk: "h1", lines: null }]);
});

test("hunk_without_lines", () => {
  const next = toggleLine([{ hunk: "h1", lines: [3] }], HUNK, 3);

  expect(next).toEqual([]);
});

test("check_a_line_of_an_unchecked_hunk", () => {
  expect(toggleLine([], HUNK, 2)).toEqual([{ hunk: "h1", lines: [2] }]);
});

test("a_single_checked_line_of_a_one_line_hunk_is_the_whole_hunk", () => {
  const single: Hunk = { id: "s", oldStart: 0, oldLines: 0, newStart: 0, newLines: 1 };

  expect(toggleLine([], single, 0)).toEqual([{ hunk: "s", lines: null }]);
});

test("the_lines_end_up_sorted_and_the_rest_of_the_selection_intact", () => {
  const selection = [
    { hunk: "h2", lines: null },
    { hunk: "h1", lines: [4, 0] },
  ];

  expect(toggleLine(selection, HUNK, 2)).toEqual([
    { hunk: "h2", lines: null },
    { hunk: "h1", lines: [0, 2, 4] },
  ]);
});

test("state_of_a_hunk_and_its_lines", () => {
  expect(hunkCheck([], HUNK)).toBe("off");
  expect(hunkCheck([{ hunk: "h1", lines: null }], HUNK)).toBe("on");
  expect(hunkCheck([{ hunk: "h1", lines: [0, 1, 2, 3, 4] }], HUNK)).toBe("on");
  expect(hunkCheck([{ hunk: "h1", lines: [1] }], HUNK)).toBe("partial");
  expect(lineChecked([{ hunk: "h1", lines: [1] }], HUNK, 1)).toBe(true);
  expect(lineChecked([{ hunk: "h1", lines: [1] }], HUNK, 2)).toBe(false);
  expect(lineChecked([{ hunk: "h1", lines: null }], HUNK, 2)).toBe(true);
});

test("toggle_a_hunk", () => {
  expect(toggleHunk([], HUNK)).toEqual([{ hunk: "h1", lines: null }]);
  expect(toggleHunk([{ hunk: "h1", lines: null }], HUNK)).toEqual([]);
  expect(toggleHunk([{ hunk: "h1", lines: [1] }], HUNK)).toEqual([{ hunk: "h1", lines: null }]);
});

describe("file", () => {
  const hunks = [HUNK, OTHER];

  test("included_without_selection_means_all_its_hunks", () => {
    expect(effectiveSelection(hunks, [], true)).toEqual([
      { hunk: "h1", lines: null },
      { hunk: "h2", lines: null },
    ]);
    expect(effectiveSelection(hunks, [], false)).toEqual([]);
    const partial = [{ hunk: "h1", lines: [0] }];
    expect(effectiveSelection(hunks, partial, true)).toEqual(partial);
    expect(effectiveSelection(hunks, partial, false)).toEqual([]);
  });

  test("everything_checked_is_the_whole_file_again", () => {
    const all = effectiveSelection(hunks, [], true);

    expect(normalizeSelection(hunks, all)).toEqual({ selection: [], included: true });
    expect(normalizeSelection(hunks, [])).toEqual({ selection: [], included: false });
    expect(normalizeSelection(hunks, [all[0]])).toEqual({ selection: [all[0]], included: true });
  });

  test("file_checkbox", () => {
    expect(fileCheck(hunks, [])).toBe("off");
    expect(fileCheck(hunks, effectiveSelection(hunks, [], true))).toBe("on");
    expect(fileCheck(hunks, [{ hunk: "h1", lines: null }])).toBe("partial");
    expect(fileCheck(hunks, [{ hunk: "h1", lines: [0] }])).toBe("partial");
  });
});

describe("margin", () => {
  test("whole_hunk_with_one_checkbox_on_the_local_side", () => {
    const right = marginFor([HUNK], [{ hunk: "h1", lines: null }], "new", null);
    const left = marginFor([HUNK], [{ hunk: "h1", lines: null }], "old", null);

    expect([...right]).toEqual([[12, { hunk: HUNK, line: null, checked: true }]]);
    expect(left.size).toBe(0);
  });

  test("unchecked_hunk_has_the_empty_checkbox", () => {
    const right = marginFor([HUNK], [], "new", null);

    expect(right.get(12)?.checked).toBe(false);
  });

  test("deletion_hunk_puts_the_checkbox_where_it_was", () => {
    expect(marginFor([GONE], [], "new", null).get(45)?.line).toBe(null);
  });

  test("partial_hunk_has_one_checkbox_per_line_on_its_side", () => {
    const selection = [{ hunk: "h1", lines: [1, 3] }];

    const left = marginFor([HUNK], selection, "old", null);
    const right = marginFor([HUNK], selection, "new", null);

    expect([...left.keys()]).toEqual([10, 11]);
    expect(left.get(10)).toEqual({ hunk: HUNK, line: 0, checked: false });
    expect(left.get(11)).toEqual({ hunk: HUNK, line: 1, checked: true });
    expect([...right.keys()]).toEqual([12, 13, 14]);
    expect(right.get(12)).toEqual({ hunk: HUNK, line: 2, checked: false });
    expect(right.get(13)).toEqual({ hunk: HUNK, line: 3, checked: true });
  });

  test("hovering_a_line_shows_its_checkbox", () => {
    const selection = [{ hunk: "h1", lines: null }];

    const left = marginFor([HUNK], selection, "old", { side: "old", line: 11 });
    const right = marginFor([HUNK], selection, "new", { side: "new", line: 13 });

    expect(left.get(11)).toEqual({ hunk: HUNK, line: 1, checked: true });
    expect(right.get(13)).toEqual({ hunk: HUNK, line: 3, checked: true });
    expect(right.get(12)?.line).toBe(null);
  });

  test("hovering_the_line_of_the_hunk_checkbox_does_not_cover_it", () => {
    const right = marginFor([HUNK], [], "new", { side: "new", line: 12 });

    expect(right.get(12)?.line).toBe(null);
  });

  test("hovering_outside_a_change_shows_nothing", () => {
    expect(marginFor([HUNK], [], "new", { side: "new", line: 20 }).has(20)).toBe(false);
  });
});

describe("lines_chosen_in_the_editor", () => {
  test("hunks_that_touch_the_lines", () => {
    expect(hunksAt([HUNK, OTHER], "new", 13, 34)).toEqual([HUNK, OTHER]);
    expect(hunksAt([HUNK, OTHER], "new", 15, 20)).toEqual([]);
    expect(hunksAt([HUNK, OTHER], "old", 11, 11)).toEqual([HUNK]);
  });

  test("with_the_cursor_it_is_the_whole_hunk_as_when_undoing_in_the_editor", () => {
    expect(linesSelection([HUNK], "new", 13, 13, false)).toEqual([{ hunk: "h1", lines: null }]);
  });

  test("a_selection_chooses_those_lines_and_their_pair_in_the_same_position", () => {
    expect(linesSelection([HUNK], "new", 13, 13, true)).toEqual([{ hunk: "h1", lines: [1, 3] }]);
    expect(linesSelection([HUNK], "new", 14, 14, true)).toEqual([{ hunk: "h1", lines: [4] }]);
    expect(linesSelection([HUNK], "old", 10, 10, true)).toEqual([{ hunk: "h1", lines: [0, 2] }]);
  });

  test("the_whole_selection_of_a_hunk_is_the_whole_hunk", () => {
    expect(linesSelection([HUNK], "new", 12, 14, true)).toEqual([{ hunk: "h1", lines: null }]);
  });

  test("where_a_change_was", () => {
    expect(hunkPlace(HUNK)).toBe("lines 13–15");
    expect(hunkPlace({ ...HUNK, newLines: 1 })).toBe("line 13");
    expect(hunkPlace(GONE)).toBe("deleted at line 46");
  });
});
