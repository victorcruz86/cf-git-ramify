import { expect, test } from "vitest";
import type { Changelists, FileChange, FileState } from "../lib/api";
import { canDrop, dragCount, dragLoad, dragText, dropPlan, dropRoot } from "./drag";
import { buildTree, CONFLICTS_ID, UNVERSIONED_ID, type TreeNode } from "./tree";

const change = (path: string, state: FileState = "modified") =>
  ({ path, oldPath: null, state, staged: false, unstaged: true }) satisfies FileChange;

const lists: Changelists = {
  lists: [
    { id: "default", name: "Changes", comment: "" },
    { id: "nocommit", name: "VCA NO COMMIT", comment: "" },
  ],
  active: "default",
  assignments: { "src/a.ts": "default", "src/b.ts": "default", "env.js": "nocommit" },
  hunkAssignments: {},
};

const changes = [
  change("src/a.ts"),
  change("src/b.ts"),
  change("env.js"),
  change("new/one.ts", "untracked"),
  change("new/two.ts", "untracked"),
  change("both.ts", "unmerged"),
];

const tree = (byDirectory = false) => buildTree(changes, lists, byDirectory);

function find(roots: TreeNode[], id: string): TreeNode {
  const visit = (nodes: TreeNode[]): TreeNode | undefined => {
    for (const node of nodes) {
      if (node.id === id) return node;
      const found = visit(node.children);
      if (found) return found;
    }
  };
  const node = visit(roots);
  if (!node) throw new Error(`no node ${id}`);
  return node;
}

test("drop_root_walks_a_file_up_to_its_list", () => {
  const roots = tree();
  expect(dropRoot(roots, "file:env.js")?.id).toBe("list:nocommit");
  expect(dropRoot(roots, "list:default")?.id).toBe("list:default");
});

test("drop_root_walks_a_folder_row_up_to_its_list", () => {
  const roots = tree(true);
  expect(dropRoot(roots, "dir:default:src")?.id).toBe("list:default");
  expect(dropRoot(roots, "file:src/a.ts")?.id).toBe("list:default");
});

test("drop_root_of_an_unknown_id_is_null", () => {
  expect(dropRoot(tree(), "file:missing.ts")).toBeNull();
});

test("the_load_of_a_list_row_is_every_file_in_it", () => {
  const load = dragLoad(tree(), new Set(["list:default"]));
  expect(load.versioned).toEqual([
    { path: "src/a.ts", list: "default" },
    { path: "src/b.ts", list: "default" },
  ]);
  expect(load.unversioned).toEqual([]);
});

test("a_file_selected_with_its_folder_is_carried_once", () => {
  const load = dragLoad(tree(true), new Set(["dir:default:src", "file:src/a.ts"]));
  expect(load.versioned.map((entry) => entry.path)).toEqual(["src/a.ts", "src/b.ts"]);
});

test("unversioned_files_are_carried_apart", () => {
  const load = dragLoad(tree(), new Set([UNVERSIONED_ID, "file:env.js"]));
  expect(load.versioned).toEqual([{ path: "env.js", list: "nocommit" }]);
  expect(load.unversioned).toEqual(["new/one.ts", "new/two.ts"]);
});

test("conflicted_files_are_versioned_with_no_list", () => {
  const load = dragLoad(tree(), new Set(["file:both.ts"]));
  expect(load.versioned).toEqual([{ path: "both.ts", list: "" }]);
});

test("count_does_not_count_a_row_inside_a_selected_row_twice", () => {
  const roots = tree();
  expect(dragCount(roots, new Set(["list:default", "file:src/a.ts"]))).toBe(2);
  expect(dragCount(roots, new Set(["file:src/a.ts", "file:env.js", UNVERSIONED_ID]))).toBe(4);
});

test("only_a_changelist_accepts", () => {
  const roots = tree();
  const load = dragLoad(roots, new Set(["file:env.js"]));
  expect(canDrop(find(roots, "list:default"), load)).toBe(true);
  expect(canDrop(find(roots, UNVERSIONED_ID), load)).toBe(false);
  expect(canDrop(find(roots, CONFLICTS_ID), load)).toBe(false);
  expect(canDrop(null, load)).toBe(false);
});

test("a_load_touching_the_target_list_is_refused_whole", () => {
  const roots = tree();
  const load = dragLoad(roots, new Set(["file:env.js", "file:src/a.ts"]));
  expect(canDrop(find(roots, "list:default"), load)).toBe(false);
  expect(canDrop(find(roots, "list:nocommit"), load)).toBe(false);
});

test("unversioned_only_is_accepted_by_any_list", () => {
  const roots = tree();
  const load = dragLoad(roots, new Set(["file:new/one.ts"]));
  expect(canDrop(find(roots, "list:default"), load)).toBe(true);
  expect(canDrop(find(roots, "list:nocommit"), load)).toBe(true);
});

test("an_empty_load_is_refused", () => {
  const roots = tree();
  expect(canDrop(find(roots, "list:default"), { versioned: [], unversioned: [] })).toBe(false);
});

test("drag_text_is_1_file_and_2_files", () => {
  expect(dragText(1)).toBe("1 file");
  expect(dragText(2)).toBe("2 files");
});

test("a_path_gone_before_the_drop_is_skipped", () => {
  const load = dragLoad(tree(), new Set(["file:src/a.ts", "file:src/b.ts", "file:new/one.ts"]));
  const now = changes.filter((entry) => entry.path !== "src/b.ts");
  expect(dropPlan(load, "nocommit", now, lists)).toEqual({
    list: "nocommit",
    versioned: ["src/a.ts"],
    hunks: [],
    unversioned: ["new/one.ts"],
  });
});

test("a_target_list_gone_before_the_drop_runs_nothing", () => {
  const load = dragLoad(tree(), new Set(["file:src/a.ts"]));
  const now = { ...lists, lists: lists.lists.filter((list) => list.id !== "nocommit") };
  expect(dropPlan(load, "nocommit", changes, now)).toBeNull();
});

test("a_drop_with_every_path_gone_runs_nothing", () => {
  const load = dragLoad(tree(), new Set(["file:src/a.ts"]));
  expect(dropPlan(load, "nocommit", [], lists)).toBeNull();
});

test("an_unversioned_file_added_meanwhile_is_only_moved", () => {
  const load = dragLoad(tree(), new Set(["file:new/one.ts"]));
  const now = changes.map((entry) =>
    entry.path === "new/one.ts" ? change("new/one.ts", "added") : entry,
  );
  expect(dropPlan(load, "nocommit", now, lists)).toEqual({
    list: "nocommit",
    versioned: ["new/one.ts"],
    hunks: [],
    unversioned: [],
  });
});

test("a_file_dragged_from_two_lists_is_moved_once", () => {
  const load = {
    versioned: [
      { path: "src/a.ts", list: "default" },
      { path: "src/a.ts", list: "other" },
    ],
    unversioned: [],
  };
  expect(dropPlan(load, "nocommit", changes, lists)?.versioned).toEqual(["src/a.ts"]);
});

// `moveChangesTo` with a `ChangeListChange`: a file split between lists moves only the hunks of the
// list it was dragged from; the other lists keep theirs.
const split: Changelists = {
  lists: [...lists.lists, { id: "feature", name: "Feature", comment: "" }],
  active: "default",
  assignments: { "src/a.ts": "default", "src/b.ts": "default", "env.js": "nocommit" },
  hunkAssignments: { "src/a.ts": { h1: "default", h2: "nocommit", h3: "default" } },
};

test("a_split_file_dragged_from_its_own_list_moves_only_that_lists_hunks", () => {
  const load = dragLoad(buildTree(changes, split, false), new Set(["list:default"]));
  expect(dropPlan(load, "feature", changes, split)).toEqual({
    list: "feature",
    versioned: ["src/b.ts"],
    hunks: [{ path: "src/a.ts", ids: ["h1", "h3"] }],
    unversioned: [],
  });
});

test("a_part_row_moves_only_the_hunks_of_its_list", () => {
  const load = dragLoad(buildTree(changes, split, false), new Set(["file:src/a.ts@nocommit"]));
  expect(load.versioned).toEqual([{ path: "src/a.ts", list: "nocommit" }]);
  expect(dropPlan(load, "feature", changes, split)).toEqual({
    list: "feature",
    versioned: [],
    hunks: [{ path: "src/a.ts", ids: ["h2"] }],
    unversioned: [],
  });
});

test("a_split_file_dragged_from_both_lists_moves_both_lists_hunks", () => {
  const load = dragLoad(buildTree(changes, split, false), new Set(["list:default", "list:nocommit"]));
  expect(dropPlan(load, "feature", changes, split)?.hunks).toEqual([
    { path: "src/a.ts", ids: ["h1", "h3"] },
    { path: "src/a.ts", ids: ["h2"] },
  ]);
});
