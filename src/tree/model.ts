import type { FileChange, FileState, TreeEntry } from "../lib/api";
import type { FileTarget } from "../files/gitFileMenu";
import { speedMatch } from "../lib/speedSearch";

// The root of the tree is the repository itself: its path, relative to the repository, is "".
export const ROOT = "";

export interface TreeRow {
  entry: TreeEntry;
  depth: number;
}

// What `git status --ignored` said, as `ChangeListManagerImpl.getStatus` asks it: the files with a
// change by path, and the ignored folders (`dir/`), which colour everything under them.
export interface StatusIndex {
  states: Map<string, FileState>;
  ignoredFolders: string[];
}

export function statusIndex(changes: FileChange[]): StatusIndex {
  const states = new Map<string, FileState>();
  const ignoredFolders: string[] = [];
  for (const { path, state } of changes) {
    if (state === "ignored" && path.endsWith("/")) {
      ignoredFolders.push(path);
      continue;
    }
    // The unversioned holder is asked first: a file deleted from the index and still on disk
    // (`git rm --cached`) is unversioned.
    if (states.get(path) !== "untracked") states.set(path, state);
  }
  return { states, ignoredFolders };
}

// The status of an entry. A folder has none of its own, except an ignored one: folders are not
// coloured by their contents (`VcsConfiguration.SHOW_DIRTY_RECURSIVELY=false`).
export function stateOf(index: StatusIndex, path: string): FileState | null {
  const own = index.states.get(path);
  if (own) return own;
  if (path === ROOT) return null;
  const folder = `${path}/`;
  return index.ignoredFolders.some((ignored) => folder.startsWith(ignored)) ? "ignored" : null;
}

// FILESTATUS_* of the 241 dark New UI scheme (`expUI_darkScheme.xml:55-74`, G§1), as `theme.ts` names them.
const TREE_STATUS_TOKENS: Record<FileState, string> = {
  modified: "tree-status-modified",
  // Git reports renames as FileStatus.MODIFIED.
  renamed: "tree-status-modified",
  added: "tree-status-added",
  copied: "tree-status-added",
  deleted: "tree-status-deleted",
  unmerged: "tree-status-conflict",
  untracked: "tree-status-unknown",
  ignored: "tree-status-ignored",
};

// The colour of a name, or null for the default text.
export function entryColor(state: FileState | null): string | null {
  return state ? `var(--${TREE_STATUS_TOKENS[state]})` : null;
}

export function parentOf(path: string): string | null {
  if (path === ROOT) return null;
  const slash = path.lastIndexOf("/");
  return slash < 0 ? ROOT : path.slice(0, slash);
}

// The rows that show: the root, and below each expanded folder the entries it was listed with.
// An expanded folder whose listing has not arrived shows nothing below it yet.
export function visibleRows(
  rootName: string,
  listed: Map<string, TreeEntry[]>,
  expanded: Set<string>,
): TreeRow[] {
  const rows: TreeRow[] = [];
  const add = (entry: TreeEntry, depth: number) => {
    rows.push({ entry, depth });
    if (!entry.dir || !expanded.has(entry.path)) return;
    for (const child of listed.get(entry.path) ?? []) add(child, depth + 1);
  };
  add({ name: rootName, path: ROOT, dir: true }, 0);
  return rows;
}

// The folders whose listing shows now: what a change on disk lists again.
export function openFolders(listed: Map<string, TreeEntry[]>, expanded: Set<string>): string[] {
  const folders: string[] = [];
  const visit = (path: string) => {
    if (!expanded.has(path) || !listed.has(path)) return;
    folders.push(path);
    for (const child of listed.get(path) ?? []) if (child.dir) visit(child.path);
  };
  visit(ROOT);
  return folders;
}

// At most `max` tasks run at once; the others wait their turn in order.
export function limiter(max: number): <T>(task: () => Promise<T>) => Promise<T> {
  let running = 0;
  const queue: (() => void)[] = [];
  return async (task) => {
    if (running >= max) await new Promise<void>((resolve) => queue.push(resolve));
    running++;
    try {
      return await task();
    } finally {
      running--;
      queue.shift()?.();
    }
  };
}

// `DefaultTreeExpander.collapseAll` of the Project view (`TreeUtil.collapseAll(tree, false, 1)`):
// everything but the root, whose children stay in sight, and the selection goes up to the
// top-level entry it was under.
export function collapsedAll(lead: string | null): { expanded: Set<string>; lead: string | null } {
  return { expanded: new Set([ROOT]), lead: lead === null ? null : lead.split("/")[0] };
}

// `TreeSpeedSearch`: the rows whose name matches, the ones the arrows move between.
export function matchingRows(rows: TreeRow[], query: string): TreeRow[] {
  if (!query) return [];
  return rows.filter((row) => speedMatch(row.entry.name, query) !== null);
}

// The name in pieces, with what matches marked.
export function namePieces(name: string, query: string): { text: string; hit: boolean }[] {
  const match = query ? speedMatch(name, query) : null;
  if (!match) return [{ text: name, hit: false }];
  const pieces: { text: string; hit: boolean }[] = [];
  let position = 0;
  for (const [start, end] of match.fragments) {
    if (start > position) pieces.push({ text: name.slice(position, start), hit: false });
    pieces.push({ text: name.slice(start, end), hit: true });
    position = end;
  }
  if (position < name.length) pieces.push({ text: name.slice(position), hit: false });
  return pieces;
}

// The selected entries as the Git file actions see them: the root row is the repository's folder.
export function fileTargets(entries: TreeEntry[], index: StatusIndex): FileTarget[] {
  return entries.map(({ path, dir, binary }) => ({
    path,
    dir,
    state: stateOf(index, path),
    ...(binary ? { binary } : {}),
  }));
}

// `ChangeListManager.getAllChanges` is not empty: unversioned and ignored files are not changes.
export function hasLocalChanges(index: StatusIndex): boolean {
  return [...index.states.values()].some((state) => state !== "untracked" && state !== "ignored");
}

// The selection of the Project tree, as repository paths in row order ("" is the repository):
// what `Git > Current File` acts on when the tree has it. Outside a mounted tree there is none.
let selection: () => string[] = () => [];

export function treeSelection(): string[] {
  return selection();
}

export function provideTreeSelection(provider: () => string[]) {
  selection = provider;
  return () => {
    if (selection === provider) selection = () => [];
  };
}

// `TreeNodeState` of a tree with checkboxes (`SelectionManager`): checked by itself, under a
// checked folder, above a checked entry, or none.
export type CheckState = "selected" | "above" | "below" | "clear";

// `VfsUtilCore.isAncestor(ancestor, path, strict)` over repository paths.
function isAncestor(ancestor: string, path: string, strict: boolean): boolean {
  if (ancestor === path) return !strict;
  return ancestor === ROOT || path.startsWith(`${ancestor}/`);
}

// `SelectionManager.getState`.
export function checkState(checked: ReadonlySet<string>, path: string): CheckState {
  if (checked.has(path)) return "selected";
  for (let up = parentOf(path); up !== null; up = parentOf(up)) {
    if (checked.has(up)) return "above";
  }
  for (const other of checked) if (isAncestor(path, other, true)) return "below";
  return "clear";
}

// `SelectionManager.toggleSelection`: an entry under a checked folder does nothing, and with
// `limit` checked nothing more is. Checking one drops what was checked under it, so a folder with
// checked entries below takes their place.
export function toggleChecked(
  checked: ReadonlySet<string>,
  path: string,
  limit: number,
): ReadonlySet<string> {
  const state = checkState(checked, path);
  if (state === "above") return checked;
  if (state === "clear" && checked.size >= limit) return checked;
  if (state === "selected") return new Set([...checked].filter((other) => other !== path));
  return new Set([...[...checked].filter((other) => !isAncestor(path, other, true)), path]);
}
