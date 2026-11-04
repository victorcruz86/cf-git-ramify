import type {
  Changelists,
  CommitOptions,
  CommitRequest,
  FileChange,
  FileSelection,
  Hunk,
  LineSelection,
  SelectionLost,
} from "../lib/api";
import { partKey, partLists, pathOfKey } from "./tree";

export type Selections = Map<string, LineSelection[]>;

export function buildCommit(
  changes: FileChange[],
  included: Set<string>,
  lineSelections: Selections,
  message: string,
  options: CommitOptions,
): { request: CommitRequest; selections: FileSelection[] } {
  const added: string[] = [];
  const removed: string[] = [];
  const selections: FileSelection[] = [];
  for (const change of changes) {
    if (!included.has(change.path)) continue;
    if (change.state === "deleted") {
      removed.push(change.path);
      continue;
    }
    if (change.state === "renamed" && change.oldPath) removed.push(change.oldPath);
    const selection = lineSelections.get(change.path);
    if (selection && selection.length > 0) selections.push({ path: change.path, selection });
    else added.push(change.path);
  }
  return { request: { added, removed, partial: [], message, options }, selections };
}

// Only the hunks that exist: list changes for a hunk that is no longer there mean nothing.
export function liveLists(lists: Changelists, hunks: Map<string, Hunk[]>): Changelists {
  const assignments = Object.entries(lists.hunkAssignments).map(([path, known]) => {
    const live = hunks.get(path);
    if (!live) return [path, known] as const;
    const ids = new Set(live.map((hunk) => hunk.id));
    return [
      path,
      Object.fromEntries(Object.entries(known).filter(([id]) => ids.has(id))),
    ] as const;
  });
  return { ...lists, hunkAssignments: Object.fromEntries(assignments) };
}

// What goes into the commit according to the checkboxes of each list. A file split across lists is
// committed by hunks: those of the checked rows, with the line selection on those of its own list.
export function listSelections(
  included: Set<string>,
  lists: Changelists,
  hunks: Map<string, Hunk[]>,
  lineSelections: Selections,
): { included: Set<string>; selections: Selections; unreadable: string[] } {
  const paths = new Set<string>();
  const unreadable: string[] = [];
  const selections: Selections = new Map(lineSelections);
  for (const key of included) paths.add(pathOfKey(key));
  for (const path of [...paths]) {
    const parts = partLists(lists, path);
    const known = hunks.get(path);
    if (parts.length === 0) continue;
    if (!known) {
      unreadable.push(path);
      continue;
    }
    const own = lists.assignments[path] ?? lists.active;
    const chosen = new Set(parts.filter((list) => included.has(partKey(path, list))));
    if (included.has(path)) chosen.add(own);
    const listOf = (hunk: Hunk) => lists.hunkAssignments[path]?.[hunk.id] ?? own;
    const explicit = lineSelections.get(path);
    const entries = known
      .filter((hunk) => chosen.has(listOf(hunk)))
      .flatMap((hunk) => {
        if (!explicit || listOf(hunk) !== own) return [{ hunk: hunk.id, lines: null }];
        return explicit.filter((entry) => entry.hunk === hunk.id);
      });
    if (entries.length === known.length && !explicit) selections.delete(path);
    else if (entries.length > 0) selections.set(path, entries);
    else paths.delete(path);
  }
  return { included: paths, selections, unreadable };
}

export function dropLostHunks(selections: Selections, lost: SelectionLost): Selections {
  const next = new Map(selections);
  const left = (next.get(lost.path) ?? []).filter((item) => !lost.hunks.includes(item.hunk));
  if (left.length > 0) next.set(lost.path, left);
  else next.delete(lost.path);
  return next;
}

// What rollback shows: the lists of the chosen files, with only these checked. With no choice, the
// whole active list. Unversioned files never get in.
export function rollbackTargets(
  changes: FileChange[],
  lists: Changelists,
  chosen: string[],
): { files: FileChange[]; checked: string[] } {
  const versioned = changes.filter(
    (change) => change.state !== "untracked" && change.state !== "ignored",
  );
  const listOf = (change: FileChange) => lists.assignments[change.path] ?? lists.active;
  const wanted = new Set(chosen);
  const selected = versioned.filter((change) => wanted.has(change.path));
  if (selected.length === 0) {
    const files = versioned.filter((change) => listOf(change) === lists.active);
    return { files, checked: files.map((file) => file.path) };
  }
  const affected = new Set(selected.map(listOf));
  return {
    files: versioned.filter((change) => affected.has(listOf(change))),
    checked: selected.map((file) => file.path),
  };
}

export type LegendKey = "added" | "modified" | "deleted";

export function legend(
  changes: FileChange[],
  included: Set<string>,
): { key: LegendKey; text: string }[] {
  let added = 0;
  let unversioned = 0;
  let modified = 0;
  let deleted = 0;
  for (const change of changes) {
    if (!included.has(change.path)) continue;
    if (change.state === "added" || change.state === "copied") added++;
    else if (change.state === "untracked") unversioned++;
    else if (change.state === "deleted") deleted++;
    else modified++;
  }
  const items: { key: LegendKey; text: string }[] = [];
  if (added > 0 || unversioned > 0) {
    const value = added > 0 && unversioned > 0 ? `${added}+${unversioned}` : added + unversioned;
    items.push({ key: "added", text: `${value} added` });
  }
  if (modified > 0) items.push({ key: "modified", text: `${modified} modified` });
  if (deleted > 0) items.push({ key: "deleted", text: `${deleted} deleted` });
  return items;
}
