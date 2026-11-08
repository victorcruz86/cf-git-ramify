// The changes of a stash and the texts of their diffs (`GitStashProvider.StashObject`, `GitStashChange`,
// `ShowDiffWithLocalAction.getChangeWithLocal` and `GitStashOperations.viewStash`). The texts come from
// the repository: they are only shown as text.
import {
  stashChanges,
  stashFileSides,
  worktreeSides,
  type FileSides,
  type PathsSelection,
  type Side,
  type StashChanges,
  type StashEntry,
} from "../lib/api";
import { taskResult } from "../lib/tasks";
import type { DiffMode, SavedChange, SavedDiff } from "./savedPatches";

export interface StashChangesApi {
  changes(sha: string): Promise<StashChanges>;
  // `sha` is the stash's or that of one of its groups.
  sides(sha: string, path: string, oldPath: string | null): Promise<FileSides>;
  // The file on disk, as the diff of Local Changes reads it.
  local(path: string): Promise<Side>;
}

export const stashChangesBackend: StashChangesApi = {
  changes: async (sha) => taskResult<StashChanges>(await stashChanges(sha)),
  sides: async (sha, path, oldPath) => taskResult<FileSides>(await stashFileSides(sha, path, oldPath)),
  local: async (path) => (await worktreeSides(path)).new,
};

// `DiffBundle` `merge.version.title.base` and `.our`: the titles of a side without a revision number.
export const BASE_VERSION = "Base version";
export const YOUR_VERSION = "Your version";

// `toChangeObjects`: the stash against its first parent, then each other parent under its tag.
export function stashChangeList(entry: StashEntry, changes: StashChanges): SavedChange[] {
  return [
    ...changes.files.map((file) => ({ file, group: null, source: entry.sha })),
    ...changes.groups.flatMap((group) =>
      group.files.map((file) => ({ file, group: group.title, source: group.sha })),
    ),
  ];
}

// `stash.editor.diff.preview.id.change.title` with `data.stash.capitalize()`, or `.empty.title`.
export function stashDiffTitle(entry: StashEntry, change: SavedChange | null): string {
  if (!change) return "Stash";
  const name = change.file.path.slice(change.file.path.lastIndexOf("/") + 1);
  return `Stash@{${entry.index}}: ${name}`;
}

// The revision `Compare with Local` (the stash's version) or `Compare Before with Local` (the one it was
// made on) puts against the local file, and the local file to read: that of the revision's path, or
// of the other one when it is not on disk (`getChangeWithLocal`, renames).
const ABSENT: Side = { kind: "absent" };

function revisionMissing(change: SavedChange, mode: Exclude<DiffMode, "change">): boolean {
  const kind = change.file.status[0];
  return mode === "local" ? kind === "D" : kind === "A";
}

async function localFor(
  api: StashChangesApi,
  change: SavedChange,
  mode: Exclude<DiffMode, "change">,
): Promise<Side> {
  const { path, oldPath } = change.file;
  const own = mode === "local" ? path : (oldPath ?? path);
  const other = mode === "local" ? (oldPath ?? path) : path;
  const paths = revisionMissing(change, mode) ? [other] : [...new Set([own, other])];
  for (const candidate of paths) {
    const side = await api.local(candidate);
    if (side.kind !== "absent") return side;
  }
  return ABSENT;
}

// `getChangeWithLocal` gives nothing when neither the revision nor the local file exist: that change
// is left out of the chain.
export async function stashComparable(
  api: StashChangesApi,
  change: SavedChange,
  mode: DiffMode,
): Promise<boolean> {
  if (mode === "change" || !revisionMissing(change, mode)) return true;
  return (await localFor(api, change, mode)).kind !== "absent";
}

// The stash's change, or that change against the local file. The index and the stash share their
// parent, HEAD; the untracked files have none, and only add files.
export async function stashDiff(
  api: StashChangesApi,
  entry: StashEntry,
  change: SavedChange,
  mode: DiffMode,
): Promise<SavedDiff> {
  const { file, source } = change;
  const sides = await api.sides(source, file.path, file.oldPath);
  const before = sides.old.kind === "absent" ? BASE_VERSION : entry.parents[0];
  const after = sides.new.kind === "absent" ? YOUR_VERSION : source;
  if (mode === "change") return { left: sides.old, right: sides.new, titles: [before, after] };
  const local = await localFor(api, change, mode);
  if (mode === "local") {
    const stashed = sides.new.kind === "absent" ? BASE_VERSION : source;
    return { left: sides.new, right: local, titles: [stashed, YOUR_VERSION] };
  }
  return { left: sides.old, right: local, titles: [before, YOUR_VERSION] };
}

// `Cherry-Pick Selected Changes` (`Vcs.ApplySelectedChanges`): the change's commit against the base
// the tab shows it with, as the log's `apply_paths` takes it.
export function stashPathsSelection(entry: StashEntry, change: SavedChange): PathsSelection {
  const { file, source } = change;
  return {
    hash: source,
    // The untracked files' commit has no parent; the stash and its index have HEAD.
    base: source === entry.parents[2] ? null : entry.parents[0],
    paths: [file.path, ...(file.oldPath ? [file.oldPath] : [])],
  };
}

// `View` of the Unstash dialog (`getRevisionChanges(…, local = true)`): each file of the stash itself,
// the local one before it. The local side is a current revision, which has no number: it takes the
// default title.
export function stashViewChanges(entry: StashEntry, changes: StashChanges): SavedChange[] {
  return changes.files.map((file) => ({ file, group: null, source: entry.sha }));
}

export async function stashViewDiff(api: StashChangesApi, change: SavedChange): Promise<SavedDiff> {
  const { file, source } = change;
  // `case 'A'` has no before revision: nothing local on the left.
  const added = file.status[0] === "A";
  const [sides, local] = await Promise.all([
    api.sides(source, file.path, file.oldPath),
    added ? Promise.resolve(ABSENT) : api.local(file.oldPath ?? file.path),
  ]);
  const after = sides.new.kind === "absent" ? YOUR_VERSION : source;
  return { left: local, right: sides.new, titles: [BASE_VERSION, after] };
}
