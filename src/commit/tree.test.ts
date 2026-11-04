import { expect, test } from "vitest";
import type { Changelists, FileChange, FileState } from "../lib/api";
import {
  buildTree,
  checkState,
  collectFiles,
  CONFLICTS_ID,
  countText,
  flattenTree,
  partKey,
  statusColor,
  UNVERSIONED_ID,
  type TreeNode,
} from "./tree";

const change = (path: string, state: FileState = "modified", oldPath: string | null = null) =>
  ({ path, oldPath, state, staged: false, unstaged: true }) satisfies FileChange;

const lists = (assignments: Record<string, string> = {}): Changelists => ({
  lists: [
    { id: "default", name: "Changes", comment: "" },
    { id: "other", name: "Other", comment: "" },
  ],
  active: "default",
  assignments,
  hunkAssignments: {},
});

const names = (nodes: TreeNode[]) => nodes.map((node) => node.name);

test("groups_by_changelist", () => {
  const roots = buildTree(
    [change("a.txt"), change("b.txt"), change("c.txt")],
    lists({ "a.txt": "default", "b.txt": "other", "c.txt": "other" }),
    false,
  );

  expect(names(roots)).toEqual(["Changes", "Other"]);
  expect(roots[0].paths).toEqual(["a.txt"]);
  expect(roots[1].paths).toEqual(["b.txt", "c.txt"]);
  expect(countText(roots[1])).toBe("2 files");
  expect(countText(roots[0])).toBe("1 file");
});

test("the_active_list_goes_first_and_the_others_by_name", () => {
  const data = lists();
  data.lists = [
    { id: "z", name: "Zeta", comment: "" },
    { id: "a", name: "alpha", comment: "" },
    { id: "m", name: "Middle", comment: "" },
  ];
  data.active = "m";

  const roots = buildTree([], data, false);

  expect(names(roots)).toEqual(["Middle", "alpha", "Zeta"]);
  expect(roots[0].active).toBe(true);
  expect(roots[1].active).toBe(false);
});

test("an_empty_list_stays_in_the_tree_without_a_counter", () => {
  const roots = buildTree([], lists(), false);

  expect(names(roots)).toEqual(["Changes", "Other"]);
  expect(countText(roots[0])).toBe("");
});

test("unversioned_in_its_own_section", () => {
  const roots = buildTree(
    [change("a.txt"), change("new.txt", "untracked")],
    lists({ "a.txt": "default", "new.txt": "default" }),
    false,
  );

  expect(names(roots)).toEqual(["Changes", "Other", "Unversioned Files"]);
  expect(roots[0].paths).toEqual(["a.txt"]);
  expect(roots[2].id).toBe(UNVERSIONED_ID);
  expect(roots[2].paths).toEqual(["new.txt"]);
});

test("without_unversioned_files_there_is_no_section", () => {
  expect(names(buildTree([change("a.txt")], lists({ "a.txt": "default" }), false))).toEqual([
    "Changes",
    "Other",
  ]);
});

test("ignored_files_do_not_show_up", () => {
  const roots = buildTree([change("x.log", "ignored")], lists({ "x.log": "default" }), false);

  expect(roots[0].paths).toEqual([]);
  expect(names(roots)).toEqual(["Changes", "Other"]);
});

test("an_unassigned_file_goes_to_the_active_list", () => {
  expect(buildTree([change("a.txt")], lists(), false)[0].paths).toEqual(["a.txt"]);
});

test("groups_by_directory", () => {
  const [root] = buildTree([change("src/a/b.txt")], lists({ "src/a/b.txt": "default" }), true);

  expect(names(root.children)).toEqual(["src/a"]);
  expect(root.children[0].kind).toBe("directory");
  expect(names(root.children[0].children)).toEqual(["b.txt"]);
});

test("groups_by_directory_with_branching", () => {
  const paths = ["src/a/b.txt", "src/a/c.txt", "src/d.txt", "top.txt"];
  const [root] = buildTree(paths.map((path) => change(path)), lists(), true);

  expect(names(root.children)).toEqual(["src", "top.txt"]);
  const [src] = root.children;
  expect(names(src.children)).toEqual(["a", "d.txt"]);
  expect(names(src.children[0].children)).toEqual(["b.txt", "c.txt"]);
  expect(src.paths).toEqual(["src/a/b.txt", "src/a/c.txt", "src/d.txt"]);
  expect(countText(src)).toBe("3 files");
});

test("without_grouping_the_folder_goes_in_gray_next_to_the_name", () => {
  const [root] = buildTree([change("src/a/b.txt")], lists(), false);

  expect(root.children[0].name).toBe("b.txt");
  expect(root.children[0].location).toBe("src/a");
});

test("mixed_checkbox", () => {
  const [root] = buildTree([change("a.txt"), change("b.txt")], lists(), false);

  expect(checkState(root, new Set(["a.txt"]))).toBe("mixed");
  expect(checkState(root, new Set(["a.txt", "b.txt"]))).toBe("on");
  expect(checkState(root, new Set())).toBe("off");
  expect(checkState(root.children[0], new Set(["a.txt"]))).toBe("on");
  // A part of the file chosen: neither the row nor what contains it is whole.
  const partial = new Set(["a.txt"]);
  expect(checkState(root.children[0], new Set(["a.txt"]), partial)).toBe("mixed");
  expect(checkState(root, new Set(["a.txt", "b.txt"]), partial)).toBe("mixed");
  expect(checkState(root.children[1], new Set(["a.txt", "b.txt"]), partial)).toBe("on");
});

test("stable_order", () => {
  const first = buildTree([change("b.txt"), change("a.txt"), change("c/d.txt")], lists(), true);
  const second = buildTree([change("c/d.txt"), change("a.txt"), change("b.txt")], lists(), true);

  expect(second).toEqual(first);
});

test("ids_do_not_change_when_moving_to_another_list", () => {
  const before = buildTree([change("a.txt")], lists({ "a.txt": "default" }), false);
  const after = buildTree([change("a.txt")], lists({ "a.txt": "other" }), false);

  expect(after[1].children[0].id).toBe(before[0].children[0].id);
});

test("natural_order_of_the_names", () => {
  const [root] = buildTree([change("a10.txt"), change("a2.txt"), change("B.txt")], lists(), false);

  expect(names(root.children)).toEqual(["a2.txt", "a10.txt", "B.txt"]);
});

test("flattens_only_what_is_expanded", () => {
  const roots = buildTree([change("src/a.txt"), change("src/b.txt")], lists(), true);
  const dir = roots[0].children[0];

  const open = flattenTree(roots, new Set());
  expect(open.map((row) => row.node.name)).toEqual(["Changes", "src", "a.txt", "b.txt", "Other"]);
  expect(open.map((row) => row.depth)).toEqual([0, 1, 2, 2, 0]);

  const closed = flattenTree(roots, new Set([dir.id]));
  expect(closed.map((row) => row.node.name)).toEqual(["Changes", "src", "Other"]);
});

test("collects_the_files_of_the_selection_without_repeating", () => {
  const roots = buildTree([change("a.txt"), change("b.txt")], lists(), false);
  const [root] = roots;

  const files = collectFiles([root, root.children[0]]);

  expect(files.map((file) => file.path)).toEqual(["a.txt", "b.txt"]);
});

test("status_colors", () => {
  expect(statusColor("modified")).toBe("var(--status-modified)");
  expect(statusColor("renamed")).toBe("var(--status-modified)");
  expect(statusColor("added")).toBe("var(--status-added)");
  expect(statusColor("copied")).toBe("var(--status-added)");
  expect(statusColor("deleted")).toBe("var(--status-deleted)");
  expect(statusColor("untracked")).toBe("var(--status-unversioned)");
  expect(statusColor("unmerged")).toBe("var(--status-conflict)");
});

test("a_file_with_hunks_in_another_list_shows_up_in_both", () => {
  const lists: Changelists = {
    lists: [
      { id: "default", name: "Changes", comment: "" },
      { id: "other", name: "Other", comment: "" },
    ],
    active: "default",
    assignments: { "a.txt": "default" },
    hunkAssignments: { "a.txt": { h1: "default", h2: "other" } },
  };
  const a: FileChange = {
    path: "a.txt",
    oldPath: null,
    state: "modified",
    staged: false,
    unstaged: true,
  };

  const [own, other] = buildTree([a], lists, false);

  expect(own.paths).toEqual(["a.txt"]);
  expect(other.paths).toEqual([partKey("a.txt", "other")]);
  expect(other.children[0].change).toBe(a);
  expect(other.children[0].id).not.toBe(own.children[0].id);
});

test("without_hunks_in_another_list_the_file_shows_up_once", () => {
  const lists: Changelists = {
    lists: [
      { id: "default", name: "Changes", comment: "" },
      { id: "other", name: "Other", comment: "" },
    ],
    active: "default",
    assignments: { "a.txt": "default" },
    hunkAssignments: { "a.txt": { h1: "default" } },
  };
  const a: FileChange = {
    path: "a.txt",
    oldPath: null,
    state: "modified",
    staged: false,
    unstaged: true,
  };

  const [, other] = buildTree([a], lists, false);

  expect(other.paths).toEqual([]);
});

test("conflicts_go_to_their_own_node_first_and_not_to_the_lists", () => {
  const roots = buildTree(
    [change("a.txt"), change("c.txt", "unmerged"), change("d/e.txt", "unmerged")],
    lists({ "a.txt": "default", "c.txt": "default" }),
    false,
  );

  expect(names(roots)).toEqual(["Merge Conflicts", "Changes", "Other"]);
  const [conflicts, active] = roots;
  expect(conflicts).toMatchObject({ id: CONFLICTS_ID, kind: "conflicts", checkable: false });
  expect(conflicts.paths).toEqual(["c.txt", "d/e.txt"]);
  expect(names(conflicts.children)).toEqual(["c.txt", "e.txt"]);
  expect(conflicts.children[1].location).toBe("d");
  expect(active.paths).toEqual(["a.txt"]);
  expect(conflicts.children.every((child) => child.checkable === false)).toBe(true);
});

test("the_conflicts_node_groups_by_directory_and_without_conflicts_it_does_not_show", () => {
  const [conflicts] = buildTree([change("d/e.txt", "unmerged")], lists(), true);

  expect(conflicts.kind).toBe("conflicts");
  expect(names(conflicts.children)).toEqual(["d"]);
  expect(names(buildTree([change("a.txt")], lists(), false))).toEqual(["Changes", "Other"]);
});
