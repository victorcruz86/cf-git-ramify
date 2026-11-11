// The rules of a drag in the Local Changes tree, from 241's `ChangesViewDnDSupport`,
// `ChangesTreeDnDSupport` and `ChangesBrowserChangeListNode.canAcceptDrop`.
import type { Changelists, FileChange } from "../lib/api";
import type { TreeNode } from "./tree";

export interface DragLoad {
  // `list` is the top-level changelist the row sits in, or "" outside any list.
  versioned: { path: string; list: string }[];
  unversioned: string[];
}

// `getDropRootNode`: the row under the pointer, walked up to the row under the root.
export function dropRoot(roots: TreeNode[], id: string): TreeNode | null {
  const contains = (node: TreeNode): boolean =>
    node.id === id || node.children.some(contains);
  return roots.find(contains) ?? null;
}

// Every file under the selected rows, once, and the list each one is dragged from.
export function dragLoad(roots: TreeNode[], selected: Set<string>): DragLoad {
  const load: DragLoad = { versioned: [], unversioned: [] };
  const seen = new Set<string>();
  const visit = (node: TreeNode, root: TreeNode, inside: boolean) => {
    const taken = inside || selected.has(node.id);
    if (taken && node.change) {
      const list = root.kind === "changelist" && root.list ? root.list.id : "";
      const key = `${node.change.path}\0${list}`;
      if (!seen.has(key)) {
        seen.add(key);
        if (node.change.state === "untracked") load.unversioned.push(node.change.path);
        else if (node.change.state !== "ignored") load.versioned.push({ path: node.change.path, list });
      }
    }
    for (const child of node.children) visit(child, root, taken);
  };
  for (const root of roots) visit(root, root, false);
  return load;
}

// `getSelectionCount`: the files of each selected row that is not inside another selected row.
export function dragCount(roots: TreeNode[], selected: Set<string>): number {
  let count = 0;
  const visit = (node: TreeNode) => {
    if (selected.has(node.id)) {
      count += node.paths.length;
      return;
    }
    node.children.forEach(visit);
  };
  roots.forEach(visit);
  return count;
}

// Only a changelist accepts, and not when any dragged change is already in it.
export function canDrop(target: TreeNode | null, load: DragLoad): boolean {
  if (!target || target.kind !== "changelist" || !target.list) return false;
  if (load.versioned.length === 0 && load.unversioned.length === 0) return false;
  const id = target.list.id;
  return load.versioned.every((entry) => entry.list !== id);
}

// `vcs.dnd.image.text.n.files`.
export function dragText(count: number): string {
  return count === 1 ? "1 file" : `${count} files`;
}

export interface DropPlan {
  list: string;
  // Whole files, with all their hunks.
  versioned: string[];
  // Files split between lists: only the hunks of the list each was dragged from.
  hunks: { path: string; ids: string[] }[];
  unversioned: string[];
}

// What a drop runs, read against the changes and lists of the moment of the drop: files may be
// written or removed while the drag goes on. Null when the list is gone or nothing is left.
// `moveChangesTo` gets a `ChangeListChange` for a file split between lists, so only the ranges of
// the dragged list move and the other lists keep theirs.
export function dropPlan(
  load: DragLoad,
  list: string,
  changes: FileChange[],
  lists: Changelists,
): DropPlan | null {
  if (!lists.lists.some((entry) => entry.id === list)) return null;
  const known = (id: string) => lists.lists.some((entry) => entry.id === id);
  const states = new Map(changes.map((change) => [change.path, change.state]));
  const versioned = new Set<string>();
  const hunks: DropPlan["hunks"] = [];
  const unversioned = new Set<string>();
  for (const { path, list: from } of load.versioned) {
    const state = states.get(path);
    if (state === undefined || state === "ignored") continue;
    if (state === "untracked") {
      unversioned.add(path);
      continue;
    }
    const assigned = Object.entries(lists.hunkAssignments[path] ?? {});
    const split = from !== "" && assigned.some(([, id]) => id !== from && known(id));
    if (!split) {
      versioned.add(path);
      continue;
    }
    const ids = assigned.filter(([, id]) => id === from).map(([hunk]) => hunk);
    if (ids.length > 0) hunks.push({ path, ids });
  }
  for (const path of load.unversioned) {
    const state = states.get(path);
    if (state === undefined || state === "ignored") continue;
    if (state === "untracked") unversioned.add(path);
    else versioned.add(path);
  }
  if (versioned.size === 0 && hunks.length === 0 && unversioned.size === 0) return null;
  return { list, versioned: [...versioned], hunks, unversioned: [...unversioned] };
}
