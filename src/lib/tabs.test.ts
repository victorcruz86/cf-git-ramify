import { get } from "svelte/store";
import { expect, test, vi } from "vitest";
import {
  forgetToolWindows,
  registeredToolWindows,
  registerToolWindow,
  toolWindow,
  unregisterToolWindow,
} from "../shell/toolWindows";
import {
  contentTitle,
  keepSelected,
  orderContents,
  onTabRequested,
  requestTab,
  stripeWindows,
  toolWindows,
  type ContentId,
  type SelectedContents,
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

// Project, Changes and Repositories hold none of `ChangesViewContentManager`'s contents.
const OTHERS = { project: [], changes: null, repositories: null, pulls: null };

test("in modal mode Local Changes is in the Git window and there is no Commit window", () => {
  expect(toolWindows("modal", ["console", "log", "local-changes"])).toEqual({
    ...OTHERS,
    commit: null,
    git: ["local-changes", "log", "console"],
  });
});

test("in non-modal mode Local Changes, Shelf and Stash go to the Commit window", () => {
  expect(toolWindows("non-modal", ALL)).toEqual({
    ...OTHERS,
    commit: ["local-changes", "shelf", "stash"],
    git: ["log", "console"],
  });
});

// The user's `window.layouts.xml`: Project (order 0) and Commit (order 1) on the left, Git at
// the bottom. Changes and Repositories are not in it: they are registered on the left on first use.
test("the_stripe_shows_registered_windows_in_registry_order", () => {
  expect(stripeWindows(toolWindows("non-modal", ALL))).toEqual(["project", "commit", "git"]);
  expect(stripeWindows(toolWindows("modal", ALL))).toEqual(["project", "git"]);

  expect(stripeWindows(toolWindows("non-modal", ALL, ["repositories"]))).toEqual([
    "project",
    "commit",
    "repositories",
    "git",
  ]);
  const both = toolWindows("non-modal", ALL, ["repositories", "changes"]);
  expect(stripeWindows(both)).toEqual(["project", "commit", "changes", "repositories", "git"]);
  expect(both.changes).toEqual([]);
});

// `Pull Requests` (`GHPRToolWindowFactory`): on the left, while a remote points to GitHub.
test("the_pull_requests_window_is_on_the_left_once_registered", () => {
  const all = toolWindows("non-modal", ALL, ["pulls", "changes", "repositories"]);
  expect(stripeWindows(all)).toEqual(["project", "commit", "changes", "repositories", "pulls", "git"]);
  expect(all.pulls).toEqual([]);
  expect(toolWindow("pulls")).toMatchObject({ label: "Pull Requests", icon: "pullRequests", side: "left" });
});

test("a_window_registered_on_first_use_can_go_away", () => {
  forgetToolWindows();
  registerToolWindow("pulls");
  registerToolWindow("changes");
  unregisterToolWindow("pulls");
  expect(get(registeredToolWindows)).toEqual(["changes"]);
  unregisterToolWindow("pulls");
  expect(get(registeredToolWindows)).toEqual(["changes"]);
  forgetToolWindows();
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
  const none = { project: null, changes: null, repositories: null, pulls: null };
  const selected: SelectedContents = { ...none, commit: "local-changes", git: "stash" };

  expect(keepSelected({ ...OTHERS, commit: null, git: ["local-changes", "log", "console"] }, selected)).toEqual({
    ...none,
    commit: "local-changes",
    git: "local-changes",
  });
  expect(keepSelected({ ...OTHERS, commit: ["local-changes"], git: ["log", "console"] }, selected)).toEqual({
    ...none,
    commit: "local-changes",
    git: "log",
  });
  // Nothing vanished: the same record, so that nobody is told it changed.
  const kept: SelectedContents = { ...none, commit: "local-changes", git: "stash" };
  expect(keepSelected({ ...OTHERS, commit: null, git: ["local-changes", "stash", "log"] }, kept)).toBe(kept);
});

// `CommitTabTitleUpdater.updateTab` in the New UI: in the Commit window, Local Changes is `Commit`.
test("the_local_changes_tab_is_named_commit_in_the_commit_window", () => {
  expect(contentTitle("local-changes", "commit")).toBe("Commit");
  expect(contentTitle("local-changes", "git")).toBe("Local Changes");
  expect(contentTitle("stash", "commit")).toBe("Stash");
});
