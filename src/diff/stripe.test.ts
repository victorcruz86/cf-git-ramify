import { expect, test } from "vitest";
import { lineAtLane, MIN_MARK_HEIGHT, stripeMarks } from "./stripe";

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
