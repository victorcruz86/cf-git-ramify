import { fileNodes, withoutCheck, type TreeNode } from "../commit/tree";
import { LOCAL_REVISION, type ChangedFile, type FileChange, type FileState, type RevisionPair } from "../lib/api";
import { BASE_VERSION, YOUR_VERSION } from "./tab";

// A changes tree of two revisions, as the `Changes` tool window and a pull request's tab show it:
// the rows of `ChangesTree`, the files of the chosen rows, and the two sides of a file's diff.

// `git diff --name-status` letters as the states the changes tree colours.
const STATES: Record<string, FileState> = {
  A: "added",
  C: "copied",
  D: "deleted",
  R: "renamed",
};

export const stateOf = (file: ChangedFile): FileState => STATES[file.status[0]] ?? "modified";

export const asChange = (file: ChangedFile): FileChange => ({
  path: file.path,
  oldPath: file.oldPath,
  state: stateOf(file),
  staged: false,
  unstaged: false,
});

// The tree's rows, without check boxes.
export const changeRoots = (scope: string, files: ChangedFile[], byDirectory: boolean): TreeNode[] =>
  fileNodes(scope, new Set(), files.map(asChange), byDirectory).map(withoutCheck);

function nodesById(nodes: TreeNode[], into = new Map<string, TreeNode>()) {
  for (const node of nodes) {
    into.set(node.id, node);
    nodesById(node.children, into);
  }
  return into;
}

// `VcsTreeModelData.selected`: the files of the chosen rows, a folder giving all of its own, in
// the order of the tree.
export function selectedFiles<T extends ChangedFile>(
  roots: TreeNode[],
  selected: Set<string>,
  byPath: Map<string, T>,
): T[] {
  const nodes = nodesById(roots);
  const paths = new Set<string>();
  const visit = (node: TreeNode) => {
    if (node.change) paths.add(node.change.path);
    node.children.forEach(visit);
  };
  for (const id of selected) {
    const node = nodes.get(id);
    if (node) visit(node);
  }
  const files: T[] = [];
  const ordered = (nodes: TreeNode[]) => {
    for (const node of nodes) {
      const file = node.change && byPath.get(node.change.path);
      if (file && paths.has(file.path)) files.push(file);
      ordered(node.children);
    }
  };
  ordered(roots);
  return files;
}

// Every folder of the tree, for `Collapse All`.
export function folderIds(nodes: TreeNode[], into: string[] = []): string[] {
  for (const node of nodes) {
    if (node.children.length > 0) {
      into.push(node.id);
      folderIds(node.children, into);
    }
  }
  return into;
}

// `ChangeDiffRequestProducer.createSimpleRequest`: the revision number of each side, `Base
// version` where the old file is not and `Your version` for the local file or none.
// The local side of Show Diff with Working Tree is on the left: a `CurrentContentRevision`, whose
// empty number gives the left default, `Base version`.
export function pairOf(left: string, right: string, file: ChangedFile): RevisionPair {
  const kind = file.status[0];
  const leftTitle = left === LOCAL_REVISION ? BASE_VERSION : left;
  const rightTitle = right === LOCAL_REVISION ? YOUR_VERSION : right;
  return {
    left:
      kind === "A"
        ? { revision: null, path: file.path, title: BASE_VERSION }
        : { revision: left, path: file.oldPath ?? file.path, title: leftTitle },
    right:
      kind === "D"
        ? { revision: null, path: file.path, title: YOUR_VERSION }
        : { revision: right, path: file.path, title: rightTitle },
  };
}

export const fileName = (path: string) => path.slice(path.lastIndexOf("/") + 1);
