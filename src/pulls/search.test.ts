import { afterEach, beforeEach, expect, test, vi } from "vitest";
import type { SavedSearch, SearchValue } from "../lib/api";
import {
  clearFiltersLabel,
  defaultSearch,
  EMPTY_SEARCH,
  FILTERS,
  filterCount,
  HISTORY_DELAY,
  initialSearch,
  quickFilters,
  REVIEWS,
  sameSearch,
  searchHistory,
  shortText,
  STATES,
} from "./search";

const search = (value: Partial<SearchValue>): SearchValue => ({ ...EMPTY_SEARCH, ...value });

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

test("the_default_search_is_assigned_to_you_unless_one_was_saved", () => {
  expect(defaultSearch("octocat")).toEqual(search({ state: "OPEN", assignee: "octocat" }));
  expect(initialSearch(null, "octocat")).toEqual(defaultSearch("octocat"));
  expect(initialSearch({ last: null, history: [] }, "octocat")).toEqual(defaultSearch("octocat"));
  const saved: SavedSearch = { last: search({ author: "bob" }), history: [] };
  expect(initialSearch(saved, "octocat")).toEqual(search({ author: "bob" }));
  // The empty search was saved: it stays empty.
  expect(initialSearch({ last: EMPTY_SEARCH, history: [] }, "octocat")).toEqual(EMPTY_SEARCH);
});

test("the_quick_filters_are_241s", () => {
  expect(quickFilters("octocat")).toEqual([
    { title: "Open", value: search({ state: "OPEN" }) },
    { title: "Your pull requests", value: search({ state: "OPEN", author: "octocat" }) },
    { title: "Assigned to you", value: search({ state: "OPEN", assignee: "octocat" }) },
    { title: "Review requests", value: search({ state: "OPEN", review: "AWAITING_REVIEW" }) },
  ]);
});

test("clear_filters_sets_the_empty_search", () => {
  // `Clear {0} Filters` counts each value that is set, the text too.
  expect(filterCount(EMPTY_SEARCH)).toBe(0);
  const every = search({
    text: "crash",
    state: "CLOSED",
    assignee: "a",
    review: "APPROVED",
    author: "b",
    label: "bug",
  });
  expect(filterCount(every)).toBe(6);
  expect(clearFiltersLabel(2)).toBe("Clear 2 Filters");
  expect(sameSearch(search({ state: "OPEN" }), search({ state: "OPEN" }))).toBe(true);
  expect(sameSearch(search({ state: "OPEN" }), search({ state: "OPEN", label: "x" }))).toBe(false);
});

test("the_filter_dropdowns_are_in_241s_order_with_their_values", () => {
  expect(FILTERS.map((filter) => filter.name)).toEqual(["State", "Author", "Label", "Assignee", "Review"]);
  expect(STATES.map((state) => state.label)).toEqual(["Open", "Closed", "Merged"]);
  expect(REVIEWS.map(({ value, short, full }) => [value, short, full])).toEqual([
    ["NO_REVIEW", "No reviews", "No reviews"],
    ["REQUIRED", "Review required", "Review required"],
    ["APPROVED", "Approved review", "Approved review"],
    ["CHANGES_REQUESTED", "Changes requested", "Changes requested"],
    ["REVIEWED_BY_ME", "Reviewed", "Reviewed by you"],
    ["NOT_REVIEWED_BY_ME", "Not reviewed", "Not reviewed by you"],
    ["AWAITING_REVIEW", "Awaiting review", "Awaiting review from you"],
  ]);
});

test("the_short_text_is_241s_with_its_trailing_space", () => {
  expect(shortText(EMPTY_SEARCH)).toBe("");
  expect(
    shortText(
      search({
        text: "crash",
        state: "OPEN",
        assignee: "a",
        review: "REVIEWED_BY_ME",
        author: "b",
        label: "good first",
      }),
    ),
  ).toBe('"crash" state:"Open" label:good first assignee:a reviewState:"Reviewed" author:b ');
});

test("a_search_is_remembered_at_once_when_its_text_changed_and_after_ten_seconds_otherwise", async () => {
  const save = vi.fn(async (value: SearchValue, remember: boolean): Promise<SavedSearch> => ({
    last: value,
    history: remember ? [value] : [],
  }));
  const saved = vi.fn();
  const history = searchHistory(save, "octocat", saved);

  // The text changed: the last search and the history at once.
  history.changed(EMPTY_SEARCH, search({ text: "crash" }));
  expect(save.mock.calls).toEqual([[search({ text: "crash" }), true]]);
  await vi.runAllTimersAsync();
  expect(saved).toHaveBeenCalledTimes(1);

  // Only the filters changed: the last search now, the history 10 s later.
  save.mockClear();
  history.changed(search({ text: "crash" }), search({ text: "crash", label: "bug" }));
  expect(save.mock.calls).toEqual([[search({ text: "crash", label: "bug" }), false]]);
  await vi.advanceTimersByTimeAsync(HISTORY_DELAY - 1);
  expect(save).toHaveBeenCalledTimes(1);
  await vi.advanceTimersByTimeAsync(1);
  expect(save.mock.calls[1]).toEqual([search({ text: "crash", label: "bug" }), true]);

  // A change within the 10 s drops the one waiting.
  save.mockClear();
  history.changed(search({ label: "bug" }), search({ label: "ui" }));
  history.changed(search({ label: "ui" }), search({ label: "docs" }));
  await vi.runAllTimersAsync();
  expect(save.mock.calls).toEqual([
    [search({ label: "ui" }), false],
    [search({ label: "docs" }), false],
    [search({ label: "docs" }), true],
  ]);

  // No filters, or the default: saved as the last search, never in the history.
  save.mockClear();
  history.changed(search({ text: "x" }), EMPTY_SEARCH);
  history.changed(EMPTY_SEARCH, defaultSearch("octocat"));
  await vi.runAllTimersAsync();
  expect(save.mock.calls).toEqual([
    [EMPTY_SEARCH, false],
    [defaultSearch("octocat"), false],
  ]);

  // Disposed: what was waiting is dropped.
  save.mockClear();
  history.changed(search({ label: "a" }), search({ label: "b" }));
  history.dispose();
  await vi.runAllTimersAsync();
  expect(save.mock.calls).toEqual([[search({ label: "b" }), false]]);
});
