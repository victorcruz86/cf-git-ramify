import { expect, test } from "vitest";
import { alignSpacers, withoutSpacers, type Boundary, type Spacers } from "./spacers";

// The tops a column shows once its spacers are in: a spacer sits before the text of its line, so
// it is above every later line, and the end of the document is below all of them.
const ownOf = (spacers: Spacers, side: "left" | "right") =>
  spacers.flatMap((spacer) => (spacer?.side === side ? [spacer] : []));

function laidOut(
  lines: number[],
  text: number[],
  spacers: Spacers,
  side: "left" | "right",
  docLines: number,
): number[] {
  const own = ownOf(spacers, side);
  return lines.map((line, index) => {
    const above = own.filter((spacer) => line >= docLines || spacer.line < line);
    return text[index] + above.reduce((sum, spacer) => sum + spacer.height, 0);
  });
}

// What each column measures at each boundary after its own spacers, the level the two must share.
function levels(text: number[], spacers: Spacers, side: "left" | "right"): number[] {
  let offset = 0;
  return text.map((top, index) => {
    const spacer = spacers[index];
    if (spacer?.side === side) offset += spacer.height;
    return top + offset;
  });
}

test("equal_heights_need_no_spacers", () => {
  const boundaries: Boundary[] = [
    { left: 2, right: 2 },
    { left: 3, right: 3 },
    { left: 5, right: 5 },
  ];

  expect(alignSpacers(boundaries, [40, 60, 100], [40, 60, 100])).toEqual([null, null, null]);
});

test("a_hunk_taller_on_the_left_puts_its_spacer_on_the_right_after_the_hunk", () => {
  // Two lines on the left against one on the right, the second left line wrapped in two rows.
  const boundaries: Boundary[] = [
    { left: 1, right: 1 },
    { left: 3, right: 2 },
    { left: 4, right: 3 },
  ];

  const spacers = alignSpacers(boundaries, [20, 80, 100], [20, 40, 60]);

  expect(spacers).toEqual([null, { side: "right", line: 2, height: 40 }, null]);
});

test("an_unchanged_line_wrapping_only_on_one_side_gets_a_top_spacer_before_the_next_hunk", () => {
  // The first line is unchanged and takes two rows on the right only.
  const boundaries: Boundary[] = [
    { left: 2, right: 2 },
    { left: 3, right: 3 },
    { left: 4, right: 4 },
  ];
  const left = [40, 60, 80];
  const right = [60, 80, 100];

  const spacers = alignSpacers(boundaries, left, right);

  expect(spacers).toEqual([{ side: "left", line: 2, height: 20 }, null, null]);
  expect(levels(left, spacers, "left")).toEqual(levels(right, spacers, "right"));
});

test("an_insertion_at_the_end_levels_the_end", () => {
  // "a\nb\n" against "a\nb\nlong\n", the inserted line in two rows: three and four document lines.
  const boundaries: Boundary[] = [
    { left: 2, right: 2 },
    { left: 2, right: 3 },
    { left: 3, right: 4 },
  ];
  const left = [40, 40, 60];
  const right = [40, 80, 100];

  const spacers = alignSpacers(boundaries, left, right);

  expect(spacers).toEqual([null, { side: "left", line: 2, height: 40 }, null]);
  expect(levels(left, spacers, "left").at(-1)).toBe(levels(right, spacers, "right").at(-1));
});

test("the_end_gets_its_own_spacer_when_the_last_unchanged_lines_wrap_differently", () => {
  const boundaries: Boundary[] = [
    { left: 0, right: 0 },
    { left: 1, right: 1 },
    { left: 3, right: 3 },
  ];

  const spacers = alignSpacers(boundaries, [0, 20, 40], [0, 20, 60]);

  expect(spacers).toEqual([null, null, { side: "left", line: 3, height: 20 }]);
});

test("differences_within_the_tolerance_are_ignored", () => {
  const boundaries: Boundary[] = [
    { left: 1, right: 1 },
    { left: 2, right: 2 },
  ];

  expect(alignSpacers(boundaries, [20, 41], [20, 40])).toEqual([null, null]);
  expect(alignSpacers(boundaries, [20.5, 40], [20, 40])).toEqual([null, null]);
  expect(alignSpacers(boundaries, [20, 42], [20, 40])).toEqual([
    null,
    { side: "right", line: 2, height: 2 },
  ]);
  expect(alignSpacers(boundaries, [20, 41], [20, 40], 0)).toEqual([
    null,
    { side: "right", line: 2, height: 1 },
  ]);
});

test("feeding_its_output_back_changes_nothing", () => {
  // Wrapped lines everywhere: before the first hunk, inside both hunks, after the last one.
  const boundaries: Boundary[] = [
    { left: 2, right: 2 },
    { left: 4, right: 3 },
    { left: 6, right: 5 },
    { left: 6, right: 7 },
    { left: 9, right: 10 },
  ];
  const left = [60, 140, 180, 180, 260];
  const right = [40, 60, 100, 160, 200];
  const spacers = alignSpacers(boundaries, left, right);
  expect(levels(left, spacers, "left")).toEqual(levels(right, spacers, "right"));

  const leftLines = boundaries.map((boundary) => boundary.left);
  const rightLines = boundaries.map((boundary) => boundary.right);
  const shownLeft = laidOut(leftLines, left, spacers, "left", 9);
  const shownRight = laidOut(rightLines, right, spacers, "right", 10);
  const again = alignSpacers(
    boundaries,
    withoutSpacers(leftLines, shownLeft, ownOf(spacers, "left"), 9),
    withoutSpacers(rightLines, shownRight, ownOf(spacers, "right"), 10),
  );

  expect(again).toEqual(spacers);
});
