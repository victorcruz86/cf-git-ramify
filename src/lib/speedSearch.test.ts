import { expect, test } from "vitest";
import { speedMatch, speedMatches } from "./speedSearch";

test("contiguous_text_prefers_a_word_start", () => {
  expect(speedMatch("format/main", "ma")?.fragments).toEqual([[7, 9]]);
  expect(speedMatch("format", "ma")?.fragments).toEqual([[3, 5]]);
});

test("loose_letters_at_word_starts", () => {
  expect(speedMatch("origin/feature/one", "orfe")?.fragments).toEqual([
    [0, 2],
    [7, 9],
  ]);
  expect(speedMatches("origin/feature", "oe")).toBe(false);
});

test("without_a_match_there_is_no_result", () => {
  expect(speedMatch("main", "zzz")).toBeNull();
});

test("starting_earlier_and_at_a_word_is_worth_more", () => {
  const degree = (text: string) => speedMatch(text, "main")?.degree ?? 0;

  expect(degree("main")).toBeGreaterThan(degree("ob-main"));
  expect(degree("ob-main")).toBeGreaterThan(degree("domain"));
});

test("empty_text_matches_everything", () => {
  expect(speedMatches("main", "")).toBe(true);
});
