import { get, writable } from "svelte/store";
import {
  compareChangesBetween,
  compareChangesFromWorkingTree,
  compareChangesWith,
  LOCAL_REVISION,
  repoState,
  type ChangedFile,
} from "../lib/api";
import { activateWindow, registeredToolWindows, registerToolWindow } from "../shell/toolWindows";

// `ChangeDiffRequestProducer.getRevisionTitle`: what a side without a revision number is called.
export const BASE_VERSION = "Base version";
export const YOUR_VERSION = "Your version";

// Show Diff with Working Tree (`CompareBranchesDiffPanel`): the branch, the current branch (or the
// short hash) as its label names them, and the branch's full hash.
export interface BranchComparison {
  branch: string;
  current: string;
  hash: string;
}

// What the `Changes` tool window shows: the files between two sides, `left` a full hash and `right`
// a full hash or `"local"`. `paths` are the ones the list was read for (none: the whole
// repository), which `Refresh` reads again; `message` is the commit message of Show All Affected
// Files, shown under the tree, and `select` the file chosen when the tab opens. With a local side
// on the right, `head` is the HEAD read before the list (else it is read when the tab opens).
// `branch`: Show Diff with Working Tree, whose working tree is on the left unless the sides are
// swapped; it has neither Refresh nor this window's Get, but Get from Branch.
export interface ChangesRequest {
  title: string;
  left: string;
  right: string;
  files: ChangedFile[];
  message?: string;
  paths?: string[];
  head?: string | null;
  select?: string;
  branch?: BranchComparison;
}

// The one tab of the window. `id` changes with each new tab, not with a refresh; `head` is the
// HEAD seen when the list with a local side was read, which `Get` must still find.
export interface ChangesTab extends ChangesRequest {
  id: number;
  head: string | null;
}

const store = writable<ChangesTab | null>(null);
export const changesTab = { subscribe: store.subscribe };
let lastId = 0;

export const hasLocalSide = (tab: ChangesRequest) => tab.right === LOCAL_REVISION;

// `DvcsCompareSettings.shouldSwapSidesInCompareBranches`, which the application remembers.
const SWAP_KEY = "rama.compareBranches.swapSides";

export function swapSidesSetting(): boolean {
  try {
    return localStorage.getItem(SWAP_KEY) === "true";
  } catch {
    return false;
  }
}

function storeSwapSides(swap: boolean) {
  try {
    localStorage.setItem(SWAP_KEY, String(swap));
  } catch {
    // It lasts until the window closes.
  }
}

// `DvcsBranchUtil.swapRevisions`: each change the other way round.
export function swappedFiles(files: ChangedFile[]): ChangedFile[] {
  return files.map((file) => {
    const kind = file.status[0];
    if (kind === "A") return { ...file, status: "D" };
    if (kind === "D") return { ...file, status: "A" };
    return file.oldPath ? { ...file, path: file.oldPath, oldPath: file.path } : file;
  });
}

// The working tree's list against the branch, `git diff -R` (working tree on the left), turned
// round when the sides are swapped.
export async function branchChanges(
  hash: string,
  swap: boolean,
): Promise<Pick<ChangesRequest, "left" | "right" | "files">> {
  const files = await compareChangesFromWorkingTree(hash);
  return swap
    ? { left: hash, right: LOCAL_REVISION, files: swappedFiles(files) }
    : { left: LOCAL_REVISION, right: hash, files };
}

// The `Swap branches` link (`CompareBranchesDiffPanel.swapSides`): the setting flips and the same
// list is shown the other way round.
export function swapBranchSides(): void {
  const tab = get(store);
  if (!tab?.branch) return;
  storeSwapSides(!(tab.right === LOCAL_REVISION));
  store.update((current) =>
    current?.id === tab.id
      ? { ...current, left: current.right, right: current.left, files: swappedFiles(current.files) }
      : current,
  );
}

// After Get from Branch, `reloadTotalDiff`: the list read again, with the sides as they are.
export async function reloadBranchChanges(): Promise<void> {
  const tab = get(store);
  if (!tab?.branch) return;
  const head = await headNow();
  const read = await branchChanges(tab.branch.hash, tab.right === LOCAL_REVISION);
  store.update((current) => (current?.id === tab.id ? { ...current, ...read, head } : current));
}

const headNow = () =>
  repoState().then(
    (state) => state.head,
    () => null,
  );

// `ChangesBrowserToolWindow.showTab`: the window is registered on first use, `removeAllContents`
// leaves only the new tab, and the window comes to the front.
export async function showChanges(request: ChangesRequest): Promise<void> {
  const head =
    request.head !== undefined
      ? request.head
      : hasLocalSide(request)
        ? await headNow()
        : null;
  store.set({ ...request, id: ++lastId, head });
  registerToolWindow("changes");
  activateWindow("changes");
}

// `MyRefreshAction`: the same comparison read again, in the same tab. HEAD is read before the list,
// so a list never looks newer than the HEAD it goes with.
export async function refreshChanges(): Promise<void> {
  const tab = get(store);
  if (!tab) return;
  const head = hasLocalSide(tab) ? await headNow() : null;
  const files = hasLocalSide(tab)
    ? await compareChangesWith(tab.left, tab.paths ?? [])
    : await compareChangesBetween(tab.left, tab.right, tab.paths ?? []);
  store.update((current) => (current?.id === tab.id ? { ...current, files, head } : current));
}

// The list of a failed refresh is empty, as `LoadingChangesPanel` leaves it.
export function emptyChanges(): void {
  store.update((current) => (current ? { ...current, files: [] } : current));
}

// Opening another repository forgets the window, and its tab with it.
registeredToolWindows.subscribe((ids) => {
  if (!ids.includes("changes") && get(store) !== null) store.set(null);
});
