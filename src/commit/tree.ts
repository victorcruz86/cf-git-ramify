import type { ChangeList, Changelists, FileChange, FileState } from "../lib/api";

export const UNVERSIONED_ID = "unversioned";
const UNVERSIONED_TITLE = "Unversioned Files";

export const CONFLICTS_ID = "conflicts";
const CONFLICTS_TITLE = "Merge Conflicts";

export type NodeKind = "changelist" | "unversioned" | "conflicts" | "directory" | "file";
export type CheckState = "on" | "off" | "mixed";

export interface TreeNode {
  // Stable across refreshes, and across lists in the case of a file: the selection and the
  // collapsed state are stored by this id.
  id: string;
  kind: NodeKind;
  name: string;
  // The file's folder, in gray next to the name, when the tree does not group by directory.
  location: string;
  paths: string[];
  children: TreeNode[];
  change: FileChange | null;
  list: ChangeList | null;
  active: boolean;
  // No checkbox, even if it has files.
  checkable?: boolean;
}

export interface TreeRow {
  node: TreeNode;
  depth: number;
}

const collator = new Intl.Collator("en", { numeric: true, sensitivity: "base" });

function compareNames(left: string, right: string): number {
  return collator.compare(left, right) || (left < right ? -1 : left > right ? 1 : 0);
}

// A file with hunks in another list also shows up in it, with its own checkbox. That row is
// checked with this key, not with the path: each checkbox decides the hunks of its list.
const PART_SEPARATOR = "\0";

export function partKey(path: string, list: string): string {
  return `${path}${PART_SEPARATOR}${list}`;
}

export function pathOfKey(key: string): string {
  return key.split(PART_SEPARATOR, 1)[0];
}

export function scopeOfKey(key: string): string {
  return key.split(PART_SEPARATOR)[1] ?? "";
}

// The lists, other than the file's own, that have hunks of it.
export function partLists(lists: Changelists, path: string): string[] {
  const own = lists.assignments[path] ?? lists.active;
  const assigned = Object.values(lists.hunkAssignments[path] ?? {});
  return [...new Set(assigned)].filter(
    (id) => id !== own && lists.lists.some((list) => list.id === id),
  );
}

// No checkboxes at any level: what is not chosen for a commit.
export function withoutCheck(node: TreeNode): TreeNode {
  return { ...node, checkable: false, children: node.children.map(withoutCheck) };
}

export function group(
  id: string,
  kind: NodeKind,
  name: string,
  children: TreeNode[],
  list: ChangeList | null = null,
  active = false,
): TreeNode {
  const paths = children.flatMap((child) => child.paths);
  return { id, kind, name, location: "", paths, children, change: null, list, active };
}

function fileNode(change: FileChange, part: string | null = null): TreeNode {
  const slash = change.path.lastIndexOf("/");
  return {
    id: part ? `file:${change.path}@${part}` : `file:${change.path}`,
    kind: "file",
    name: change.path.slice(slash + 1),
    location: slash < 0 ? "" : change.path.slice(0, slash),
    paths: [part ? partKey(change.path, part) : change.path],
    children: [],
    change,
    list: null,
    active: false,
  };
}

interface Folder {
  folders: Map<string, Folder>;
  files: FileChange[];
}

function folderTree(changes: FileChange[]): Folder {
  const root: Folder = { folders: new Map(), files: [] };
  for (const change of changes) {
    const parts = change.path.split("/");
    const name = parts.pop();
    if (name === undefined) continue;
    let folder = root;
    for (const part of parts) {
      let next = folder.folders.get(part);
      if (!next) {
        next = { folders: new Map(), files: [] };
        folder.folders.set(part, next);
      }
      folder = next;
    }
    folder.files.push(change);
  }
  return root;
}

// A folder with a single subfolder and no files is merged with it into one entry.
function folderNodes(
  scope: string,
  parts: Set<string>,
  prefix: string,
  folder: Folder,
): TreeNode[] {
  const directories: TreeNode[] = [];
  for (const [name, child] of folder.folders) {
    let label = name;
    let current = child;
    while (current.files.length === 0 && current.folders.size === 1) {
      const [[nextName, next]] = current.folders;
      label = `${label}/${nextName}`;
      current = next;
    }
    const path = prefix + label;
    directories.push(
      group(
        `dir:${scope}:${path}`,
        "directory",
        label,
        folderNodes(scope, parts, `${path}/`, current),
      ),
    );
  }
  directories.sort((left, right) => compareNames(left.name, right.name));
  const files = folder.files
    .map((change) => nodeIn(scope, parts, change))
    .map((node) => ({ ...node, location: "" }));
  return [...directories, ...sortFiles(files)];
}

function sortFiles(nodes: TreeNode[]): TreeNode[] {
  return nodes.sort(
    (left, right) =>
      compareNames(left.name, right.name) || compareNames(left.location, right.location),
  );
}

function nodeIn(scope: string, parts: Set<string>, change: FileChange): TreeNode {
  return fileNode(change, parts.has(change.path) ? scope : null);
}

export function fileNodes(
  scope: string,
  parts: Set<string>,
  changes: FileChange[],
  byDirectory: boolean,
): TreeNode[] {
  return byDirectory
    ? folderNodes(scope, parts, "", folderTree(changes))
    : sortFiles(changes.map((change) => nodeIn(scope, parts, change)));
}

interface Classified {
  // Per list, its files and the ones that only have hunks in it.
  versioned: Map<string, FileChange[]>;
  parts: Map<string, Set<string>>;
  unversioned: FileChange[];
  conflicted: FileChange[];
}

function classify(changes: FileChange[], lists: Changelists): Classified {
  const versioned = new Map<string, FileChange[]>();
  const parts = new Map<string, Set<string>>();
  const unversioned: FileChange[] = [];
  const conflicted: FileChange[] = [];
  const add = (id: string, change: FileChange) => {
    const members = versioned.get(id);
    if (members) members.push(change);
    else versioned.set(id, [change]);
  };
  for (const change of changes) {
    if (change.state === "ignored") continue;
    if (change.state === "untracked") {
      unversioned.push(change);
      continue;
    }
    if (change.state === "unmerged") {
      conflicted.push(change);
      continue;
    }
    const assigned = lists.assignments[change.path];
    const known = lists.lists.some((list) => list.id === assigned);
    add(known ? assigned : lists.active, change);
    for (const id of partLists(lists, change.path)) {
      add(id, change);
      parts.set(id, (parts.get(id) ?? new Set()).add(change.path));
    }
  }
  return { versioned, parts, unversioned, conflicted };
}

// `ChangesBrowserConflictsNode`: before the lists, and its files do not go into a commit.
function conflictsNode(conflicted: FileChange[], groupByDirectory: boolean): TreeNode[] {
  if (conflicted.length === 0) return [];
  const files = fileNodes(CONFLICTS_ID, new Set(), conflicted, groupByDirectory);
  return [withoutCheck(group(CONFLICTS_ID, "conflicts", CONFLICTS_TITLE, files))];
}

function unversionedNode(unversioned: FileChange[], groupByDirectory: boolean): TreeNode[] {
  if (unversioned.length === 0) return [];
  const files = fileNodes(UNVERSIONED_ID, new Set(), unversioned, groupByDirectory);
  return [group(UNVERSIONED_ID, "unversioned", UNVERSIONED_TITLE, files)];
}

// The active list first and then the others by name; `Unversioned Files` last and only if it has
// files, as in the Commit window.
export function buildTree(
  changes: FileChange[],
  lists: Changelists,
  groupByDirectory: boolean,
): TreeNode[] {
  const { versioned, parts, unversioned, conflicted } = classify(changes, lists);
  const ordered = [...lists.lists].sort(
    (left, right) =>
      Number(right.id === lists.active) - Number(left.id === lists.active) ||
      compareNames(left.name, right.name),
  );
  const roots = ordered.map((list) =>
    group(
      `list:${list.id}`,
      "changelist",
      list.name,
      fileNodes(list.id, parts.get(list.id) ?? new Set(), versioned.get(list.id) ?? [], groupByDirectory),
      list,
      list.id === lists.active,
    ),
  );
  return [
    ...conflictsNode(conflicted, groupByDirectory),
    ...roots,
    ...unversionedNode(unversioned, groupByDirectory),
  ];
}

// `MultipleLocalChangeListsBrowser`: the files of one list straight under the root, without the
// list's node, and then `Unversioned Files` if the dialog shows them. The conflicts stay apart, as
// in Local Changes.
export function listTree(
  changes: FileChange[],
  lists: Changelists,
  list: string,
  groupByDirectory: boolean,
  withUnversioned: boolean,
): TreeNode[] {
  const { versioned, parts, unversioned, conflicted } = classify(changes, lists);
  const files = versioned.get(list) ?? [];
  return [
    ...conflictsNode(conflicted, groupByDirectory),
    ...fileNodes(list, parts.get(list) ?? new Set(), files, groupByDirectory),
    ...(withUnversioned ? unversionedNode(unversioned, groupByDirectory) : []),
  ];
}

// The paths each list shows, its own files and those with hunks in it, without the unversioned ones.
export function listFiles(changes: FileChange[], lists: Changelists): Map<string, string[]> {
  const { versioned } = classify(changes, lists);
  return new Map(
    lists.lists.map((list) => [
      list.id,
      (versioned.get(list.id) ?? []).map((change) => change.path),
    ]),
  );
}

// The checkbox key of a path in a list's tree: the part key if only some of its hunks are there.
export function listKey(lists: Changelists, list: string, path: string): string {
  return partLists(lists, path).includes(list) ? partKey(path, list) : path;
}

// `partial`: the files of which only a part goes in, chosen in the diff window.
export function checkState(
  node: TreeNode,
  included: Set<string>,
  partial: Set<string> = new Set(),
): CheckState {
  let count = 0;
  let whole = true;
  for (const path of node.paths) {
    if (included.has(path)) count++;
    if (partial.has(path)) whole = false;
  }
  if (count === 0) return "off";
  return count === node.paths.length && whole ? "on" : "mixed";
}

export function flattenTree(roots: TreeNode[], collapsed: Set<string>): TreeRow[] {
  const rows: TreeRow[] = [];
  const visit = (node: TreeNode, depth: number) => {
    rows.push({ node, depth });
    if (collapsed.has(node.id)) return;
    for (const child of node.children) visit(child, depth + 1);
  };
  for (const root of roots) visit(root, 0);
  return rows;
}

export function collectFiles(nodes: TreeNode[]): FileChange[] {
  const files = new Map<string, FileChange>();
  const visit = (node: TreeNode) => {
    if (node.change) files.set(node.change.path, node.change);
    node.children.forEach(visit);
  };
  nodes.forEach(visit);
  return [...files.values()];
}

export function countText(node: TreeNode): string {
  const count = node.paths.length;
  if (count === 0) return "";
  return count === 1 ? "1 file" : `${count} files`;
}

const STATUS_TOKENS: Record<FileState, string> = {
  modified: "status-modified",
  // Git reports renames as FileStatus.MODIFIED.
  renamed: "status-modified",
  added: "status-added",
  copied: "status-added",
  deleted: "status-deleted",
  unmerged: "status-conflict",
  untracked: "status-unversioned",
  ignored: "status-ignored",
};

export function statusColor(state: FileState): string {
  return `var(--${STATUS_TOKENS[state]})`;
}

// "- renamed from x" or "- moved from dir/x", like the origin IntelliJ puts on the row.
export function originText(change: FileChange): string {
  if (!change.oldPath) return "";
  const parent = (path: string) => path.slice(0, path.lastIndexOf("/") + 1);
  const moved = parent(change.oldPath) !== parent(change.path);
  const verb = change.state === "copied" ? "copied" : moved ? "moved" : "renamed";
  const from = moved ? change.oldPath : change.oldPath.slice(change.oldPath.lastIndexOf("/") + 1);
  return `- ${verb} from ${from}`;
}
