import { describe, expect, test } from "vitest";
import type {
  Changelists,
  CommitOptions,
  FileChange,
  FileState,
  Hunk,
  LineSelection,
} from "../lib/api";
import {
  buildCommit,
  dropLostHunks,
  legend,
  listSelections,
  liveLists,
  rollbackTargets,
} from "./request";
import { partKey } from "./tree";

const change = (path: string, state: FileState = "modified", oldPath: string | null = null) =>
  ({ path, oldPath, state, staged: false, unstaged: true }) satisfies FileChange;

const options: CommitOptions = {
  amend: false,
  signOff: false,
  skipHooks: false,
  author: null,
  cleanup: false,
};

const none = new Map<string, LineSelection[]>();

test("modified_and_unversioned_go_to_added", () => {
  const { request, selections } = buildCommit(
    [change("a.txt"), change("new.txt", "untracked"), change("outside.txt")],
    new Set(["a.txt", "new.txt"]),
    none,
    "message",
    options,
  );

  expect(request.added).toEqual(["a.txt", "new.txt"]);
  expect(request.removed).toEqual([]);
  expect(request.partial).toEqual([]);
  expect(request.message).toBe("message");
  expect(request.options).toBe(options);
  expect(selections).toEqual([]);
});

test("deleted_goes_to_removed", () => {
  const { request } = buildCommit(
    [change("b.txt", "deleted")],
    new Set(["b.txt"]),
    none,
    "m",
    options,
  );

  expect(request.added).toEqual([]);
  expect(request.removed).toEqual(["b.txt"]);
});

test("rename_removes_the_old_and_adds_the_new", () => {
  const { request } = buildCommit(
    [change("new.txt", "renamed", "old.txt")],
    new Set(["new.txt"]),
    none,
    "m",
    options,
  );

  expect(request.added).toEqual(["new.txt"]);
  expect(request.removed).toEqual(["old.txt"]);
});

test("a_file_with_selection_goes_by_lines_and_not_whole", () => {
  const selection: LineSelection[] = [{ hunk: "h1", lines: null }];
  const { request, selections } = buildCommit(
    [change("a.txt"), change("b.txt")],
    new Set(["a.txt", "b.txt"]),
    new Map([["a.txt", selection]]),
    "m",
    options,
  );

  expect(request.added).toEqual(["b.txt"]);
  expect(selections).toEqual([{ path: "a.txt", selection }]);
});

test("an_empty_selection_or_one_of_an_unchecked_file_does_not_count", () => {
  const { request, selections } = buildCommit(
    [change("a.txt"), change("b.txt")],
    new Set(["a.txt"]),
    new Map([
      ["a.txt", []],
      ["b.txt", [{ hunk: "h1", lines: null }]],
    ]),
    "m",
    options,
  );

  expect(request.added).toEqual(["a.txt"]);
  expect(selections).toEqual([]);
});

test("rollback_of_the_selection_with_the_rest_of_its_lists_unchecked", () => {
  const lists: Changelists = {
    lists: [
      { id: "default", name: "Changes", comment: "" },
      { id: "other", name: "Other", comment: "" },
    ],
    active: "default",
    assignments: { "a.txt": "default", "b.txt": "default", "c.txt": "other" },
    hunkAssignments: {},
  };
  const changes = [change("a.txt"), change("b.txt"), change("c.txt"), change("n", "untracked")];

  const { files, checked } = rollbackTargets(changes, lists, ["a.txt", "n"]);

  expect(files.map((file) => file.path)).toEqual(["a.txt", "b.txt"]);
  expect(checked).toEqual(["a.txt"]);
});

test("rollback_without_selection_takes_the_active_list", () => {
  const lists: Changelists = {
    lists: [{ id: "default", name: "Changes", comment: "" }],
    active: "default",
    assignments: { "a.txt": "default" },
    hunkAssignments: {},
  };

  const { files, checked } = rollbackTargets([change("a.txt")], lists, []);

  expect(files.map((file) => file.path)).toEqual(["a.txt"]);
  expect(checked).toEqual(["a.txt"]);
});

test("legend_like_the_commit_one", () => {
  const changes = [
    change("a", "added"),
    change("b", "modified"),
    change("c", "renamed", "d"),
    change("e", "deleted"),
    change("f", "untracked"),
    change("g", "modified"),
  ];

  expect(legend(changes, new Set(["a", "b", "c", "e", "f"]))).toEqual([
    { key: "added", text: "1+1 added" },
    { key: "modified", text: "2 modified" },
    { key: "deleted", text: "1 deleted" },
  ]);
  expect(legend(changes, new Set(["f"]))).toEqual([{ key: "added", text: "1 added" }]);
  expect(legend(changes, new Set())).toEqual([]);
});

test("a_lost_selection_leaves_the_others", () => {
  const selections = new Map<string, LineSelection[]>([
    [
      "a.txt",
      [
        { hunk: "h1", lines: null },
        { hunk: "h2", lines: null },
      ],
    ],
    ["b.txt", [{ hunk: "h3", lines: null }]],
  ]);

  const left = dropLostHunks(selections, { path: "a.txt", hunks: ["h1"] });

  expect(left.get("a.txt")).toEqual([{ hunk: "h2", lines: null }]);
  expect(left.get("b.txt")).toEqual([{ hunk: "h3", lines: null }]);
  expect(dropLostHunks(left, { path: "b.txt", hunks: ["h3"] }).has("b.txt")).toBe(false);
});

describe("hunks_of_each_list", () => {
  const h1: Hunk = { id: "h1", oldStart: 1, oldLines: 1, newStart: 1, newLines: 1 };
  const h2: Hunk = { id: "h2", oldStart: 9, oldLines: 1, newStart: 9, newLines: 1 };
  const h3: Hunk = { id: "h3", oldStart: 20, oldLines: 0, newStart: 20, newLines: 1 };
  const lists: Changelists = {
    lists: [
      { id: "default", name: "Changes", comment: "" },
      { id: "other", name: "Other", comment: "" },
    ],
    active: "default",
    assignments: { "a.txt": "default", "b.txt": "default" },
    hunkAssignments: { "a.txt": { h1: "default", h2: "other" } },
  };
  const hunks = new Map([["a.txt", [h1, h2, h3]]]);
  const whole = (hunk: Hunk) => ({ hunk: hunk.id, lines: null });

  test("the_commit_of_a_list_carries_only_its_hunks", () => {
    const { included, selections } = listSelections(
      new Set(["a.txt", "b.txt"]),
      lists,
      hunks,
      new Map(),
    );

    expect(included).toEqual(new Set(["a.txt", "b.txt"]));
    // A hunk that was never moved follows the file's list.
    expect(selections.get("a.txt")).toEqual([whole(h1), whole(h3)]);
    expect(selections.has("b.txt")).toBe(false);
  });

  test("the_commit_of_the_other_list_carries_its_own", () => {
    const { included, selections } = listSelections(
      new Set([partKey("a.txt", "other")]),
      lists,
      hunks,
      new Map(),
    );

    expect(included).toEqual(new Set(["a.txt"]));
    expect(selections.get("a.txt")).toEqual([whole(h2)]);
  });

  test("with_both_lists_the_whole_file_goes", () => {
    const { selections } = listSelections(
      new Set(["a.txt", partKey("a.txt", "other")]),
      lists,
      hunks,
      new Map(),
    );

    expect(selections.has("a.txt")).toBe(false);
  });

  test("a_line_selection_is_respected_within_its_list", () => {
    const chosen = [{ hunk: "h1", lines: [0] }];

    const { selections } = listSelections(
      new Set(["a.txt"]),
      lists,
      hunks,
      new Map([["a.txt", chosen]]),
    );

    expect(selections.get("a.txt")).toEqual(chosen);
  });

  test("without_knowing_its_hunks_a_split_file_is_not_committed", () => {
    const { unreadable } = listSelections(new Set(["a.txt", "b.txt"]), lists, new Map(), new Map());

    expect(unreadable).toEqual(["a.txt"]);
  });

  test("hunks_that_no_longer_exist_do_not_count", () => {
    const live = liveLists(lists, new Map([["a.txt", [h1]]]));

    expect(live.hunkAssignments).toEqual({ "a.txt": { h1: "default" } });
  });
});
