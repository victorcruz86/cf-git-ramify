import { expect, test } from "vitest";
import type { FileChange, FileState } from "../lib/api";
import {
  comparisonOf,
  hasThreeVersions,
  groupStaging,
  hoverOperation,
  revertPaths,
  stagingTargets,
  stagingTree,
} from "./staging";
import { flattenTree, partKey, type TreeNode } from "./tree";

const change = (
  path: string,
  state: FileState,
  staged: boolean,
  unstaged: boolean,
  oldPath: string | null = null,
): FileChange => ({ path, oldPath, state, staged, unstaged });

const names = (files: FileChange[]) => files.map((file) => file.path);

test("file_in_both_groups", () => {
  const groups = groupStaging([change("a.txt", "modified", true, true)]);

  expect(names(groups.staged)).toEqual(["a.txt"]);
  expect(names(groups.unstaged)).toEqual(["a.txt"]);
  expect(groups.untracked).toEqual([]);
  expect(groups.conflicts).toEqual([]);
});

test("each state goes to its group", () => {
  const groups = groupStaging([
    change("staged-file", "added", true, false),
    change("unstaged-file", "modified", false, true),
    change("new", "untracked", false, true),
    change("conflict", "unmerged", false, true),
    change("ignored-file", "ignored", false, false),
  ]);

  expect(names(groups.staged)).toEqual(["staged-file"]);
  expect(names(groups.unstaged)).toEqual(["unstaged-file"]);
  expect(names(groups.untracked)).toEqual(["new"]);
  expect(names(groups.conflicts)).toEqual(["conflict"]);
});

test("the tree shows Staged and Unstaged even when empty, and the rest only if they have files", () => {
  const tree = stagingTree(groupStaging([]), false);

  expect(tree.map((node) => node.name)).toEqual(["Staged", "Unstaged"]);
});

test("the order of the groups is IntelliJ's", () => {
  const tree = stagingTree(
    groupStaging([
      change("a", "untracked", false, true),
      change("b", "modified", true, false),
      change("c", "unmerged", false, true),
    ]),
    false,
  );

  expect(tree.map((node) => node.name)).toEqual(["Unmerged", "Staged", "Unstaged", "Untracked"]);
});

test("a file in both groups has two distinct rows", () => {
  const tree = stagingTree(groupStaging([change("a.txt", "modified", true, true)]), false);

  const rows = flattenTree(tree, new Set()).filter((row) => row.node.kind === "file");

  expect(new Set(rows.map((row) => row.node.id)).size).toBe(2);
  expect(rows.flatMap((row) => row.node.paths)).toEqual([
    partKey("a.txt", "staged"),
    partKey("a.txt", "unstaged"),
  ]);
});

test("no node has a checkbox", () => {
  const tree = stagingTree(
    groupStaging([
      change("dir/a", "modified", true, true),
      change("dir/b", "untracked", false, true),
      change("dir/c", "unmerged", false, true),
    ]),
    true,
  );

  for (const { node } of flattenTree(tree, new Set())) expect(node.checkable).toBe(false);
});

test("the hover icon is Unstage in Staged and Stage in Unstaged and Untracked", () => {
  const tree = stagingTree(
    groupStaging([
      change("dir/a", "modified", true, true),
      change("n", "untracked", false, true),
      change("c", "unmerged", false, true),
    ]),
    true,
  );
  const icons = flattenTree(tree, new Set()).map(({ node }) => [
    node.name,
    hoverOperation(node)?.label ?? null,
    hoverOperation(node)?.icon ?? null,
  ]);

  expect(icons).toEqual([
    ["Unmerged", null, null],
    ["c", null, null],
    ["Staged", "Unstage", "remove"],
    ["dir", "Unstage", "remove"],
    ["a", "Unstage", "remove"],
    ["Unstaged", "Stage", "add"],
    ["dir", "Stage", "add"],
    ["a", "Stage", "add"],
    ["Untracked", "Stage", "add"],
    ["n", "Stage", "add"],
  ]);
});

test("an empty group has no icon", () => {
  const [staged] = stagingTree(groupStaging([]), false);

  expect(hoverOperation(staged)).toBeNull();
});

test("what is in Staged is taken out of the stage and the rest is put in", () => {
  const changes = [
    change("a.txt", "modified", true, true),
    change("b.txt", "modified", false, true),
    change("c.txt", "untracked", false, true),
  ];

  const targets = stagingTargets(
    [partKey("a.txt", "staged"), partKey("b.txt", "unstaged"), partKey("c.txt", "untracked")],
    changes,
  );

  expect(targets).toEqual({ stage: ["b.txt", "c.txt"], unstage: ["a.txt"] });
});

test("unstaging a rename also unstages the deletion of its old path", () => {
  const changes = [change("new.txt", "renamed", true, false, "old.txt")];

  const targets = stagingTargets([partKey("new.txt", "staged")], changes);

  expect(targets.unstage).toEqual(["new.txt", "old.txt"]);
});

test("what is in Staged is compared with HEAD and the rest with the stage", () => {
  const node = (scope: string) => ({ paths: [partKey("a.txt", scope)] }) as TreeNode;

  expect(comparisonOf(node("staged"))).toBe("staged");
  expect(comparisonOf(node("unstaged"))).toBe("local");
  expect(comparisonOf(node("untracked"))).toBe("local");
});

test("revert only looks at what is in Unstaged", () => {
  const keys = [
    partKey("a.txt", "staged"),
    partKey("b.txt", "unstaged"),
    partKey("c.txt", "untracked"),
    partKey("d.txt", "conflicts"),
  ];

  expect(revertPaths(keys)).toEqual(["b.txt"]);
});

// GitFileStatus.has(HEAD), has(STAGED) and has(LOCAL) based on what the state says.
test("three_versions_according_to_the_state", () => {
  const of = (state: FileState, staged: boolean, unstaged: boolean) =>
    hasThreeVersions({ path: "a.txt", oldPath: null, state, staged, unstaged });

  expect(of("modified", true, true)).toBe(true);
  expect(of("modified", false, true)).toBe(true);
  expect(of("modified", true, false)).toBe(true);
  expect(of("renamed", true, false)).toBe(true);
  expect(of("added", true, true)).toBe(false);
  expect(of("deleted", true, false)).toBe(false);
  expect(of("deleted", false, true)).toBe(false);
  expect(of("untracked", false, true)).toBe(false);
  expect(of("unmerged", false, true)).toBe(false);
});
