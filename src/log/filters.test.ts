import { expect, test } from "vitest";
import type { LogRef, Person } from "../lib/api";
import { speedMatches } from "../lib/speedSearch";
import {
  branchCompletionPrefix,
  branchMenu,
  branchValues,
  collectUsers,
  completionPrefix,
  completions,
  dateText,
  displayText,
  emptyFilters,
  lastDay,
  lastWeek,
  localizedUsers,
  ME,
  parseUsers,
  parseValues,
  pathsText,
  splitBranchValues,
  toLogFilter,
  userLabel,
} from "./filters";

const person = (name: string, email = "x@y.z"): Person => ({ name, email });

const refs = (names: [string, LogRef["kind"]][]): LogRef[] =>
  names.map(([name, kind]) => ({ name, kind }));

test("without_filters_requests_nothing", () => {
  expect(toLogFilter(emptyFilters())).toEqual({
    branches: [],
    users: [],
    since: null,
    until: null,
    paths: [],
    text: null,
    hashes: [],
    revisions: [],
    ranges: [],
  });
});

test("blank_text_does_not_filter", () => {
  const filter = toLogFilter({ ...emptyFilters(), text: "   ", regex: true });
  expect(filter.text).toBeNull();
});

test("text_carries_its_options", () => {
  const filter = toLogFilter({ ...emptyFilters(), text: "fo+", regex: true, matchCase: true });
  expect(filter.text).toEqual({ text: "fo+", regex: true, matchCase: true });
});

test("user_filter_me", () => {
  const filter = toLogFilter({ ...emptyFilters(), users: [ME, "Ana"] });

  expect(filter.users).toEqual(["*", "Ana"]);
  expect(userLabel(ME)).toBe("me");
  expect(userLabel("Ana")).toBe("Ana");
});

test("date_last_24_hours", () => {
  const now = new Date(2026, 2, 1, 15, 30, 0);
  expect(lastDay(now)).toBe(new Date(2026, 1, 28, 15, 30, 0).getTime() / 1000);
});

test("date_last_7_days", () => {
  const now = new Date(2026, 2, 3, 15, 30, 0);
  expect(lastWeek(now)).toBe(new Date(2026, 1, 24, 15, 30, 0).getTime() / 1000);
});

test("date_text", () => {
  const since = new Date(2026, 0, 2).getTime() / 1000;
  const until = new Date(2026, 0, 5).getTime() / 1000;
  const short = (value: number) =>
    new Intl.DateTimeFormat(undefined, { dateStyle: "short" }).format(value * 1000);

  expect(dateText(since, null)).toBe(`Since ${short(since)}`);
  expect(dateText(null, until)).toBe(`Until ${short(until)}`);
  expect(dateText(since, until)).toBe(`${short(since)}-${short(until)}`);
});

test("values_separated_by_bar_or_line_break", () => {
  expect(parseValues(" main | other\n\nlateral\n")).toEqual(["main", "other", "lateral"]);
  expect(parseValues("a|b\nc", ["\n"])).toEqual(["a|b", "c"]);
});

test("several_values_joined_and_shortened", () => {
  expect(displayText(["main"])).toBe("main");
  expect(displayText(["main", "other"])).toBe("main|other");
  expect(displayText(["a-branch-with-a-very-long-name"])).toBe("a-branch-with-a-ver…");
});

test("paths_text", () => {
  expect(pathsText(["src/b.txt"])).toBe("src/b.txt");
  expect(pathsText(["src/b.txt", "a.txt", "c"])).toBe("a.txt + 2");
});

test("users_with_capitals_sorted_and_without_repeats", () => {
  const users = [person("zoe mora"), person("Zoe Mora", "z@a.b"), person("ana"), person("", "luis@a.b")];

  expect(collectUsers(users)).toEqual(["ana", "luis", "Zoe Mora"].sort());
});

test("menu_favorites_are_the_repository_ones", () => {
  const available = refs([
    ["main", "local"],
    ["lateral", "local"],
    ["origin/main", "remote"],
    ["origin/lateral", "remote"],
  ]);

  const { favorites } = branchMenu(available, [], { local: ["lateral"], remote: ["origin/lateral"] });

  expect(favorites).toEqual(["lateral", "origin/lateral"]);
});

const FAVORITES = { local: ["main", "master"], remote: ["origin/main", "origin/master"] };

test("branch_menu_groups_like_the_filter", () => {
  const menu = branchMenu(
    refs([
      ["origin/main", "remote"],
      ["main", "local"],
      ["HEAD", "head"],
      ["zeta", "local"],
      ["upstream/x", "remote"],
      ["origin/other", "remote"],
      ["v1", "tag"],
    ]),
    [],
    FAVORITES,
  );

  expect(menu.favorites).toEqual(["HEAD", "main", "origin/main"]);
  expect(menu.groups).toEqual([
    { label: "Local", values: ["main", "zeta"] },
    { label: "origin", values: ["origin/main", "origin/other"] },
    { label: "upstream", values: ["upstream/x"] },
  ]);
});

test("recents_of_branches_that_no_longer_exist_do_not_appear", () => {
  const menu = branchMenu(
    refs([["main", "local"]]),
    [["main"], ["deleted"], ["deleted", "main"]],
    FAVORITES,
  );

  expect(menu.recent).toEqual([["main"], ["deleted", "main"]]);
});

test("recents_with_patterns_count_as_a_filter", () => {
  const available = refs([["main", "local"]]);

  const { recent } = branchMenu(
    available,
    [["ma.*"], ["-main"], ["-deleted"], ["ot("], ["(ok)x"]],
    FAVORITES,
  );

  expect(recent).toEqual([["ma.*"], ["-main"], ["(ok)x"]]);
});

test("branch_filter_range_and_hash", () => {
  const hash = "a".repeat(40);

  const parts = splitBranchValues(["main", hash, "v1..v2", "..x", "a..b..c", "-other"]);

  expect(parts).toEqual({
    branches: ["main", "..x", "a..b..c", "-other"],
    revisions: [hash],
    ranges: [["v1", "v2"]],
  });
  const filter = toLogFilter({ ...emptyFilters(), ...parts });
  expect(filter.revisions).toEqual([hash]);
  expect(filter.ranges).toEqual([["v1", "v2"]]);
  expect(filter.hashes).toEqual([]);
});

test("branch_filter_values_sorted_with_range_last", () => {
  const state = {
    ...emptyFilters(),
    branches: ["zeta", "main"],
    revisions: ["a".repeat(40)],
    ranges: [["v1", "v2"] as [string, string]],
  };

  expect(branchValues(state)).toEqual(["main", "zeta", "a".repeat(40), "v1..v2"]);
});

test("me_in_both_directions", () => {
  expect(localizedUsers([ME, "Ana"])).toEqual(["me", "Ana"]);
  expect(parseUsers(["me", "Ana"])).toEqual([ME, "Ana"]);
});

test("branch_completion_prefix", () => {
  expect(branchCompletionPrefix("main|-ot", 8)).toBe("ot");
  expect(branchCompletionPrefix("v1..ma", 6)).toBe("ma");
  expect(branchCompletionPrefix("one\nd", 5)).toBe("d");
  expect(branchCompletionPrefix("-ot", 3)).toBe("ot");
  expect(branchCompletionPrefix("ab|cd", 4)).toBe("c");
});

test("user_completion_prefix", () => {
  expect(completionPrefix("Ana|Lu", 6)).toBe("Lu");
  expect(completionPrefix("Ana\nLu", 6)).toBe("Lu");
  expect(completionPrefix("-Lu", 3)).toBe("-Lu");
});

test("complete_over_all_values", () => {
  const values = ["main", "origin/main", "lateral"];

  expect(completions(values, "ma")).toEqual(["main", "origin/main"]);
  expect(completions(values, "")).toEqual([]);
  expect(completions(values, "main")).toEqual(["origin/main"]);
});

test("speed_search", () => {
  expect(speedMatches("origin/feature-x", "FEAT")).toBe(true);
  expect(speedMatches("origin/feature-x", "ofx")).toBe(true);
  expect(speedMatches("origin/feature-x", "xf")).toBe(false);
  expect(speedMatches("main", "zz")).toBe(false);
});
