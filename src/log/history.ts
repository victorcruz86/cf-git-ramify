import type { HistoryChange, HistorySide, RevisionPair, SideRef } from "../lib/api";

// `DvcsUtil.getShortHash`.
const SHORT_HASH = 8;
const short = (hash: string) => hash.slice(0, SHORT_HASH);

// `L:149-151`, `B:1110`, `L:165-168`.
export const HISTORY_ERROR = "Error calculating file history";
export const HISTORY_EMPTY = "File history";
export const RESET_FILTERS = "Reset filters";
export const REFRESH = "Refresh";
export const unsupportedFilterText = (filter: string) =>
  `Filtering history by "${filter}" is not supported`;
export const notFoundText = (hash: string, name: string, branches: boolean) =>
  branches
    ? `${short(hash)} does not exist in history for ${name} in selected branches`
    : `${short(hash)} does not exist in history for ${name}`;
export const SHOW_ALL_BRANCHES = "View and show all branches";
export const VIEW_IN_LOG = "View in Log";
// `L:117`.
export const affectedTitle = (hash: string) => `Paths Affected by Commit ${short(hash)}`;

export const fileName = (path: string) => path.slice(path.lastIndexOf("/") + 1);

// `FileHistoryTabsManager`: `<file name>`, or `<file name> (<short hash>)` for `History Up to Here`.
export const historyTabName = (path: string, revision: string | null) =>
  revision ? `${fileName(path)} (${short(revision)})` : fileName(path);

// `FileHistoryUi.matches`: the tab of that path and revision.
export const historyKey = (path: string, revision: string | null) => `${revision ?? ""}\0${path}`;

// The full hash in the standalone diff, the short one in the preview (G§0.5).
function sideRef(side: HistorySide | null, path: string, title: string, full: boolean): SideRef {
  if (!side) return { revision: null, path, title };
  return { revision: side.hash, path: side.path, title: full ? side.hash : short(side.hash) };
}

// `FileHistoryUtil.createChangeToParents` as a request of the diff window: one parent gives two
// sides; a merge, three (`MergedChangeDiffRequestProvider`), or two when one of them has no file.
export function historyPair(change: HistoryChange, full: boolean): RevisionPair {
  const { after, before, path, hash } = change;
  const right = sideRef(after, path, full ? hash : short(hash), full);
  if (before.length < 2) return { left: sideRef(before[0] ?? null, path, "", full), right };
  const [first, second] = before;
  const ref = (side: HistorySide) => sideRef(side, path, "", full);
  if (!first) return { left: right, right: ref(second!) };
  if (!second) return { left: ref(first), right };
  if (!after) return { left: ref(first), right: ref(second) };
  return { left: ref(first), center: right, right: ref(second) };
}
