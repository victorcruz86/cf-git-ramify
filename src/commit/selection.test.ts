import { expect, test } from "vitest";
import { reconcileSelection, rangeIds, sameIds, toggleId } from "./selection";

const rows = (...ids: string[]) => ids.map((id) => ({ node: { id } }));

test("range_between_the_reference_row_and_the_chosen_one", () => {
  expect(rangeIds(rows("a", "b", "c", "d"), "b", "d")).toEqual(["b", "c", "d"]);
  expect(rangeIds(rows("a", "b", "c", "d"), "c", "a")).toEqual(["a", "b", "c"]);
});

test("range_without_reference_is_the_row", () => {
  expect(rangeIds(rows("a", "b"), null, "b")).toEqual(["b"]);
});

test("toggle_a_row", () => {
  expect([...toggleId(new Set(["a"]), "b")]).toEqual(["a", "b"]);
  expect([...toggleId(new Set(["a"]), "a")]).toEqual([]);
});

test("what_remains_in_the_tree_keeps_the_selection", () => {
  const result = reconcileSelection(rows("a", "b"), new Set(["b"]), "b", 1);

  expect([...result.selected]).toEqual(["b"]);
  expect(result.lead).toBe("b");
});

test("if_the_row_disappears_the_reference_moves_to_the_one_that_takes_its_place", () => {
  const result = reconcileSelection(rows("a", "c", "d"), new Set(["b"]), "b", 1);

  expect([...result.selected]).toEqual(["c"]);
  expect(result.lead).toBe("c");
});

test("if_it_was_the_last_it_moves_to_the_new_last", () => {
  const result = reconcileSelection(rows("a"), new Set(["b"]), "b", 1);

  expect(result.lead).toBe("a");
});

test("without_rows_there_is_no_selection", () => {
  const result = reconcileSelection([], new Set(["a"]), "a", 0);

  expect(result.selected.size).toBe(0);
  expect(result.lead).toBeNull();
});

test("two_equal_selections_are_those_with_the_same_rows", () => {
  expect(sameIds(new Set(["a", "b"]), new Set(["b", "a"]))).toBe(true);
  expect(sameIds(new Set(["a"]), new Set(["b"]))).toBe(false);
  expect(sameIds(new Set(["a"]), new Set(["a", "b"]))).toBe(false);
});
