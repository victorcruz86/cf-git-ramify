import { expect, test } from "vitest";
import {
  activateInclusion,
  forceInclusion,
  noteInclusion,
  retainInclusion,
  toggleInclusion,
} from "./inclusion";

const state = (included: string[], known: string[] = []) => ({
  included: new Set(included),
  known: new Set(known),
});
const sorted = (paths: Set<string>) => [...paths].sort();

test("refreshing_keeps_the_checkboxes_of_what_remains_and_forgets_the_rest", () => {
  const result = retainInclusion(state(["a", "b"], ["a", "b"]), ["a", "c"], ["a", "c"]);

  expect(sorted(result.included)).toEqual(["a"]);
  expect(sorted(result.known)).toEqual(["a"]);
});

test("a_new_file_is_not_checked_on_refresh", () => {
  expect(sorted(retainInclusion(state(["a"], ["a"]), ["a", "b"], ["a", "b"]).included)).toEqual([
    "a",
  ]);
});

test("commit_includes_what_is_new_in_the_active_list", () => {
  const result = activateInclusion(state(["a"], ["a"]), ["a", "b"], []);

  expect(sorted(result.included)).toEqual(["a", "b"]);
  expect(sorted(result.known)).toEqual(["a", "b"]);
});

test("commit_respects_what_the_user_unchecked", () => {
  const result = activateInclusion(state(["a"], ["a", "b"]), ["a", "b"], []);

  expect(sorted(result.included)).toEqual(["a"]);
});

test("commit_with_nothing_checked_includes_the_whole_active_list", () => {
  const result = activateInclusion(state([], ["a", "b"]), ["a", "b"], []);

  expect(sorted(result.included)).toEqual(["a", "b"]);
});

test("commit_touches_nothing_if_files_of_another_list_are_checked", () => {
  const before = state(["other"], []);

  expect(activateInclusion(before, ["a"], [])).toBe(before);
});

test("checked_unversioned_files_do_not_count_as_another_list", () => {
  const result = activateInclusion(state(["new"]), ["a"], ["new"]);

  expect(sorted(result.included)).toEqual(["a", "new"]);
});

test("commit_file_leaves_only_the_chosen_files_checked", () => {
  const result = forceInclusion(["b"], ["a", "b"]);

  expect(sorted(result.included)).toEqual(["b"]);
  expect(sorted(result.known)).toEqual(["b"]);
});

test("checking_remembers_the_active_ones_so_they_are_not_included_again", () => {
  const result = noteInclusion(state(["a"], []), ["a", "b"]);

  expect(sorted(result.known)).toEqual(["a"]);
});

test("toggle_checks_everything_if_any_is_missing_and_otherwise_unchecks", () => {
  expect(sorted(toggleInclusion(new Set(["a"]), ["a", "b"]))).toEqual(["a", "b"]);
  expect(sorted(toggleInclusion(new Set(["a", "b", "c"]), ["a", "b"]))).toEqual(["c"]);
});
