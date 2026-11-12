import { expect, test } from "vitest";
import { lineAtLane, MIN_MARK_HEIGHT, stripeBlocks, stripeMarks } from "./stripe";

test("a_block_takes_its_share_of_the_lane", () => {
  // Lines 25-50 of 100 in a 400-point lane: from 0.25 to 0.5.
  expect(stripeMarks([{ start: 25, end: 50, kind: "inserted" }], 100, 400)).toEqual([
    { top: 0.25, height: 0.25, kind: "inserted", line: 25 },
  ]);
});

test("a_one_line_change_in_a_long_file_is_at_least_three_points", () => {
  const [mark] = stripeMarks([{ start: 500, end: 501, kind: "modified" }], 10_000, 300);
  expect(mark.height * 300).toBeCloseTo(MIN_MARK_HEIGHT);
  expect(MIN_MARK_HEIGHT).toBe(3);
  expect(mark.top).toBeCloseTo(0.05);
});

test("a_mark_at_the_end_stays_inside_the_lane", () => {
  const [mark] = stripeMarks([{ start: 9_999, end: 10_000, kind: "deleted" }], 10_000, 300);
  expect(mark.top + mark.height).toBeLessThanOrEqual(1);
  expect(mark.height * 300).toBeCloseTo(MIN_MARK_HEIGHT);
});

test("an_empty_block_has_no_mark", () => {
  expect(stripeMarks([{ start: 10, end: 10, kind: "inserted" }], 100, 400)).toEqual([]);
});

test("an_empty_document_has_no_marks", () => {
  expect(stripeMarks([{ start: 0, end: 1, kind: "inserted" }], 0, 400)).toEqual([]);
  expect(stripeMarks([{ start: 0, end: 1, kind: "inserted" }], 10, 0)).toEqual([]);
});

test("the_kinds_keep_their_colour_name", () => {
  const kinds = stripeMarks(
    [
      { start: 0, end: 1, kind: "inserted" },
      { start: 2, end: 3, kind: "deleted" },
      { start: 4, end: 5, kind: "modified" },
      { start: 6, end: 7, kind: "conflict" },
    ],
    10,
    100,
  ).map((mark) => mark.kind);
  expect(kinds).toEqual(["inserted", "deleted", "modified", "conflict"]);
});

test("a_click_maps_back_to_a_line", () => {
  expect(lineAtLane(100, 400, 100)).toBe(25);
  expect(lineAtLane(0, 400, 100)).toBe(0);
  expect(lineAtLane(400, 400, 100)).toBe(99);
  expect(lineAtLane(-5, 400, 100)).toBe(0);
  expect(lineAtLane(50, 0, 100)).toBe(0);
});

// Ramify's hunks are 0-based. `changeType`: no old lines is inserted, no new lines is deleted.
const hunk = (oldStart: number, oldLines: number, newStart: number, newLines: number) => ({
  id: "",
  oldStart,
  oldLines,
  newStart,
  newLines,
});

test("each_side_marks_only_the_lines_it_has", () => {
  const hunks = [hunk(2, 3, 2, 3), hunk(10, 0, 10, 2), hunk(20, 4, 22, 0)];
  expect(stripeBlocks(hunks, "old", false, 30)).toEqual([
    { start: 2, end: 5, kind: "modified" },
    { start: 20, end: 24, kind: "deleted" },
  ]);
  expect(stripeBlocks(hunks, "new", false, 30)).toEqual([
    { start: 2, end: 5, kind: "modified" },
    { start: 10, end: 12, kind: "inserted" },
  ]);
});

test("the_unified_view_marks_every_change_even_one_with_no_lines_in_its_document", () => {
  const hunks = [hunk(10, 0, 10, 2), hunk(20, 4, 22, 0)];
  expect(stripeBlocks(hunks, "new", true, 30)).toEqual([
    { start: 10, end: 12, kind: "inserted" },
    { start: 22, end: 23, kind: "deleted" },
  ]);
});

test("a_deletion_at_the_end_of_the_unified_document_stays_on_its_last_line", () => {
  expect(stripeBlocks([hunk(5, 2, 5, 0)], "new", true, 5)).toEqual([
    { start: 4, end: 5, kind: "deleted" },
  ]);
  expect(stripeBlocks([hunk(0, 2, 0, 0)], "new", true, 0)).toEqual([]);
});
