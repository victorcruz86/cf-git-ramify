import type { Comparison, FileChange } from "../lib/api";
import { fileNodes, group, pathOfKey, scopeOfKey, withoutCheck, type TreeNode } from "./tree";

const STAGED = "staged";
const UNSTAGED = "unstaged";
const UNTRACKED = "untracked";
const CONFLICTS = "conflicts";

export interface StagingGroups {
  staged: FileChange[];
  unstaged: FileChange[];
  untracked: FileChange[];
  conflicts: FileChange[];
}

// A file with changes in the index and on disk is in both groups.
export function groupStaging(changes: FileChange[]): StagingGroups {
  const groups: StagingGroups = { staged: [], unstaged: [], untracked: [], conflicts: [] };
  for (const change of changes) {
    if (change.state === "ignored") continue;
    if (change.state === "unmerged") groups.conflicts.push(change);
    else if (change.state === "untracked") groups.untracked.push(change);
    else {
      if (change.staged) groups.staged.push(change);
      if (change.unstaged) groups.unstaged.push(change);
    }
  }
  return groups;
}

// No checkboxes: in `GitStageTree` you act with an icon shown when hovering the row.
function kindNode(scope: string, title: string, changes: FileChange[], byDirectory: boolean) {
  const parts = new Set(changes.map((change) => change.path));
  const children = fileNodes(scope, parts, changes, byDirectory);
  // `Unmerged` carries the `Resolve` link, like the conflicts node of the changelists view.
  const kind = scope === CONFLICTS ? "conflicts" : "changelist";
  return withoutCheck(group(`stage:${scope}`, kind, title, children));
}

// The order of `NodeKind.sortOrder`. Staged and Unstaged are always there, even when empty.
export function stagingTree(groups: StagingGroups, byDirectory: boolean): TreeNode[] {
  const roots = [
    kindNode(STAGED, "Staged", groups.staged, byDirectory),
    kindNode(UNSTAGED, "Unstaged", groups.unstaged, byDirectory),
  ];
  if (groups.conflicts.length > 0) {
    roots.unshift(kindNode(CONFLICTS, "Unmerged", groups.conflicts, byDirectory));
  }
  if (groups.untracked.length > 0) {
    roots.push(kindNode(UNTRACKED, "Untracked", groups.untracked, byDirectory));
  }
  return roots;
}

// The icon of `GitAddOperation` or `GitResetOperation`, depending on what is under the node. An
// empty group or a conflict has none.
export function hoverOperation(node: TreeNode): { icon: string; label: string } | null {
  const scope = scopeOfKey(node.paths[0] ?? "");
  if (scope === STAGED) return { icon: "remove", label: "Unstage" };
  if (scope === UNSTAGED || scope === UNTRACKED) return { icon: "add", label: "Stage" };
  return null;
}

// What is in Staged leaves the index and everything else enters it. Unstaging a rename must also
// return its old path, or its deletion would stay in the index.
export function stagingTargets(
  keys: string[],
  changes: FileChange[],
): { stage: string[]; unstage: string[] } {
  const stage = new Set<string>();
  const unstage = new Set<string>();
  for (const key of keys) {
    const path = pathOfKey(key);
    if (scopeOfKey(key) === CONFLICTS) continue;
    if (scopeOfKey(key) !== STAGED) {
      stage.add(path);
      continue;
    }
    unstage.add(path);
    const oldPath = changes.find((change) => change.path === path)?.oldPath;
    if (oldPath) unstage.add(oldPath);
  }
  return { stage: [...stage], unstage: [...unstage] };
}

// Everything that is still left to stage.
export function stageAllTargets(groups: StagingGroups): { stage: string[]; unstage: string[] } {
  const paths = [...groups.unstaged, ...groups.untracked].map((change) => change.path);
  return { stage: [...new Set(paths)], unstage: [] };
}

// `GitRevertOperation` only applies to what is in Unstaged: unversioned files have nothing to go
// back to.
export function revertPaths(keys: string[]): string[] {
  const paths = keys.filter((key) => scopeOfKey(key) === UNSTAGED).map(pathOfKey);
  return [...new Set(paths)];
}

export function comparisonOf(node: TreeNode): Comparison {
  return scopeOfKey(node.paths[0] ?? "") === STAGED ? "staged" : "local";
}

// GitFileStatus.has for HEAD, the stage and the disk, based on what the state says. A file changed
// in the stage and deleted from the disk is not told apart here: the diff window compares it two
// at a time.
export function hasThreeVersions(change: FileChange): boolean {
  return change.state === "modified" || change.state === "renamed";
}
