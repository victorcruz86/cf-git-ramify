import { expect, test, vi } from "vitest";
import {
  contentTitle,
  keepSelected,
  orderContents,
  onTabRequested,
  requestTab,
  toolWindows,
  type ContentId,
} from "./tabs";

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

test("a_vanished_selected_tab_falls_back_to_the_first", () => {
  const selected = { commit: "local-changes", git: "stash" } as const;

  expect(keepSelected({ commit: null, git: ["local-changes", "log", "console"] }, selected)).toEqual({
    commit: "local-changes",
    git: "local-changes",
  });
  expect(keepSelected({ commit: ["local-changes"], git: ["log", "console"] }, selected)).toEqual({
    commit: "local-changes",
    git: "log",
  });
  // Nothing vanished: the same record, so that nobody is told it changed.
  const kept = { commit: "local-changes", git: "stash" } as const;
  expect(keepSelected({ commit: null, git: ["local-changes", "stash", "log"] }, kept)).toBe(kept);
});

// `CommitTabTitleUpdater.updateTab` in the New UI: in the Commit window, Local Changes is `Commit`.
test("the_local_changes_tab_is_named_commit_in_the_commit_window", () => {
  expect(contentTitle("local-changes", "commit")).toBe("Commit");
  expect(contentTitle("local-changes", "git")).toBe("Local Changes");
  expect(contentTitle("stash", "commit")).toBe("Stash");
});
