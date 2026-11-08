// The tree of the Shelf tab (`ShelvedChangesViewManager.MyShelvedTreeModelBuilder`, `ShelvedListNode`,
// `ShelvedChangeNode`): the lists newest first, each with its files, then `Recently Deleted` with the
// deleted ones. Everything shown comes from the store and is text.
import type { ListFiles, ShelfChange, ShelfEntry, ShelfFile } from "../lib/api";
import { prettyDateTime } from "../log/draw";

// `changes.nodetitle.empty.changelist.name` (V:257), `shelve.recently.deleted.node` (V:466) and
// `changes.nodetitle.changecount` (V:259) after `spaceAndThinSpace`.
export const EMPTY_NAME = "<empty name>";
export const RECENTLY_DELETED = "Recently Deleted";
export const countText = (count: number) =>
  `  ${count === 0 ? "No files" : count === 1 ? "1 file" : `${count} files`}`;

export type ShelfRow =
  | { kind: "tag"; id: string; text: string; depth: number }
  | { kind: "list"; id: string; entry: ShelfEntry; depth: number }
  | { kind: "folder"; id: string; list: string; name: string; depth: number }
  | { kind: "file"; id: string; list: string; change: ShelfChange; depth: number };

export const TAG_ID = "tag:deleted";
export const listId = (name: string) => `list:${name}`;
export const fileKey = (file: ShelfFile) => `${file.binary ? "b" : "t"}\0${file.before ?? ""}\0${file.after ?? ""}`;
export const fileId = (list: string, file: ShelfFile) => `file:${list}\0${fileKey(file)}`;
const folderId = (list: string, name: string) => `folder:${list}\0${name}`;

export const sameFile = (a: ShelfFile, b: ShelfFile) => fileKey(a) === fileKey(b);

// The node's text: the description (or `<empty name>`), the count and the pretty date.
export function listText(entry: ShelfEntry, now: number): { name: string; count: string; date: string } {
  const description = entry.list.description.trim() === "" ? EMPTY_NAME : entry.list.description;
  return {
    name: description,
    count: countText(entry.changes.length),
    date: `, ${prettyDateTime(entry.list.dateMs, now)}`,
  };
}

// Shown greyed: applied (`recycled`) or deleted.
export const greyed = (entry: ShelfEntry) => entry.list.recycled || entry.list.deleted;

const directory = (path: string) => path.slice(0, Math.max(0, path.lastIndexOf("/")));
export const fileName = (path: string) => path.slice(path.lastIndexOf("/") + 1);

// `CHANGE_COMPARATOR`: by path.
const byPath = (a: ShelfChange, b: ShelfChange) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0);

export interface TreeOptions {
  showRecycled: boolean;
  groupByDirectory: boolean;
  // The lists and folders that are open, by id.
  expanded: Set<string>;
}

// The lists that the tree shows, in its order: not deleted (and not recycled unless
// `Already Unshelved`), then the deleted ones.
export function shownEntries(entries: ShelfEntry[], showRecycled: boolean): {
  lists: ShelfEntry[];
  deleted: ShelfEntry[];
} {
  const sorted = [...entries].sort((a, b) => b.list.dateMs - a.list.dateMs);
  return {
    lists: sorted.filter((entry) => !entry.list.deleted && (showRecycled || !entry.list.recycled)),
    deleted: sorted.filter((entry) => entry.list.deleted),
  };
}

function listRows(entry: ShelfEntry, depth: number, options: TreeOptions): ShelfRow[] {
  const name = entry.list.name;
  const rows: ShelfRow[] = [{ kind: "list", id: listId(name), entry, depth }];
  if (!options.expanded.has(listId(name))) return rows;
  const changes = [...entry.changes].sort(byPath);
  if (!options.groupByDirectory) {
    return [...rows, ...changes.map((change) => fileRow(name, change, depth + 1))];
  }
  const folders = new Map<string, ShelfChange[]>();
  for (const change of changes) {
    const folder = directory(change.path);
    folders.set(folder, [...(folders.get(folder) ?? []), change]);
  }
  for (const [folder, inside] of folders) {
    if (folder === "") {
      rows.push(...inside.map((change) => fileRow(name, change, depth + 1)));
      continue;
    }
    const id = folderId(name, folder);
    rows.push({ kind: "folder", id, list: name, name: folder, depth: depth + 1 });
    if (options.expanded.has(id)) rows.push(...inside.map((change) => fileRow(name, change, depth + 2)));
  }
  return rows;
}

const fileRow = (list: string, change: ShelfChange, depth: number): ShelfRow => ({
  kind: "file",
  id: fileId(list, change.file),
  list,
  change,
  depth,
});

export function treeRows(entries: ShelfEntry[], options: TreeOptions): ShelfRow[] {
  const { lists, deleted } = shownEntries(entries, options.showRecycled);
  const rows = lists.flatMap((entry) => listRows(entry, 0, options));
  if (deleted.length > 0) {
    rows.push({ kind: "tag", id: TAG_ID, text: RECENTLY_DELETED, depth: 0 });
    if (options.expanded.has(TAG_ID)) rows.push(...deleted.flatMap((entry) => listRows(entry, 1, options)));
  }
  return rows;
}

// Every id that can be opened: Expand All.
export function expandableIds(entries: ShelfEntry[]): Set<string> {
  const ids = new Set<string>([TAG_ID]);
  for (const entry of entries) {
    ids.add(listId(entry.list.name));
    for (const change of entry.changes) {
      const folder = directory(change.path);
      if (folder) ids.add(folderId(entry.list.name, folder));
    }
  }
  return ids;
}

// What the selection means for the actions (`getShelvedLists`, `getShelveChanges`,
// `getExactlySelectedLists`): every list a selected node is in, the lists selected themselves, and the
// files selected in each list (those of a selected folder count).
export interface ShelfSelection {
  lists: ShelfEntry[];
  exact: ShelfEntry[];
  files: ListFiles[];
}

export function selectionOf(entries: ShelfEntry[], rows: ShelfRow[], selected: Set<string>): ShelfSelection {
  const byName = new Map(entries.map((entry) => [entry.list.name, entry]));
  const lists = new Map<string, ShelfEntry>();
  const exact: ShelfEntry[] = [];
  const files = new Map<string, ShelfFile[]>();
  const addFile = (list: string, file: ShelfFile) => {
    const known = files.get(list) ?? [];
    if (!known.some((other) => sameFile(other, file))) files.set(list, [...known, file]);
  };
  for (const row of rows) {
    if (!selected.has(row.id)) continue;
    if (row.kind === "list") {
      lists.set(row.entry.list.name, row.entry);
      exact.push(row.entry);
    } else if (row.kind === "folder" || row.kind === "file") {
      const entry = byName.get(row.list);
      if (!entry) continue;
      lists.set(row.list, entry);
      if (row.kind === "file") addFile(row.list, row.change.file);
      else {
        entry.changes
          .filter((change) => directory(change.path) === row.name)
          .forEach((change) => addFile(row.list, change.file));
      }
    }
  }
  return {
    lists: [...lists.values()],
    exact,
    files: [...files].map(([name, chosen]) => ({ name, files: chosen })),
  };
}

// The files chosen in `list`, or `null` when none of its files is selected (the whole list).
export function filesOf(selection: ShelfSelection, list: string): ShelfFile[] | null {
  return selection.files.find((files) => files.name === list)?.files ?? null;
}

// `getSelectedChangesOrAll`: one selected file shows the chain of its whole list from it; otherwise
// the selected files, or every file of the selected lists.
export function diffChain(selection: ShelfSelection): { list: string; change: ShelfChange }[] {
  const of = (entry: ShelfEntry) =>
    [...entry.changes].sort(byPath).map((change) => ({ list: entry.list.name, change }));
  const chosen = selection.files.flatMap((files) => {
    const entry = selection.lists.find((item) => item.list.name === files.name);
    return entry ? of(entry).filter(({ change }) => files.files.some((file) => sameFile(file, change.file))) : [];
  });
  if (chosen.length === 1) {
    const entry = selection.lists.find((item) => item.list.name === chosen[0].list);
    if (entry) return of(entry);
  }
  return chosen.length > 0 ? chosen : selection.exact.flatMap(of);
}
