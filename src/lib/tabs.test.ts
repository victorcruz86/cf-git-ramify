import { expect, test, vi } from "vitest";
import { orderContents, onTabRequested, requestTab, toolWindows, type ContentId } from "./tabs";

const ALL: ContentId[] = ["console", "log", "stash", "local-changes", "shelf"];

test("tabs are ordered by IntelliJ's weights", () => {
  expect(orderContents(ALL)).toEqual(["local-changes", "shelf", "stash", "log", "console"]);
});

test("a Stash tab goes after Shelf and before Log", () => {
  expect(orderContents(["log", "stash", "local-changes", "shelf"])).toEqual([
    "local-changes",
    "shelf",
    "stash",
    "log",
  ]);
  expect(orderContents(["console", "log", "stash"])).toEqual(["stash", "log", "console"]);
});

test("in modal mode Local Changes is in the Git window and there is no Commit window", () => {
  expect(toolWindows("modal", ["console", "log", "local-changes"])).toEqual({
    commit: null,
    git: ["local-changes", "log", "console"],
  });
});

test("in non-modal mode Local Changes, Shelf and Stash go to the Commit window", () => {
  expect(toolWindows("non-modal", ALL)).toEqual({
    commit: ["local-changes", "shelf", "stash"],
    git: ["log", "console"],
  });
});

test("whoever_requests_a_tab_gets_it_from_whoever_listens", () => {
  const shown = vi.fn();
  const stop = onTabRequested(shown);

  requestTab("console");
  requestTab("commit");
  stop();
  requestTab("log");

  expect(shown.mock.calls).toEqual([["console"], ["commit"]]);
});
