import { get, writable } from "svelte/store";
import {
  branchOptionsGet,
  branchOptionsSet,
  branchRefsFingerprint,
  favoritesGet,
  favoritesToggle,
  repoSettingsGet,
  type BranchKind,
  type FavoriteSet,
} from "../lib/api";

export interface PopupSize {
  width: number;
  height: number;
}

// What the popup and the panel remember, per repository.
export interface BranchOptions {
  grouping: boolean;
  showRecent: boolean;
  expanded: string[];
  popupSize: PopupSize | null;
  // `GitVcsSettings.filterByActionInPopup`: the popup search also looks at the actions.
  filterByAction: boolean;
  filterOnSelect: boolean;
  navigateOnSelect: boolean;
}

// `DvcsBranchSettings`, `GitVcsOptions` and the properties of `BranchesDashboardUi`.
export const defaultBranchOptions = (): BranchOptions => ({
  grouping: true,
  showRecent: true,
  expanded: [],
  popupSize: null,
  filterByAction: true,
  filterOnSelect: false,
  navigateOnSelect: false,
});

// `GitBranchManager.getDefaultBranchNames`, until the repository's own arrive.
export const favorites = writable<FavoriteSet>({
  local: ["main", "master"],
  remote: ["origin/main", "origin/master"],
});

export const branchOptions = writable<BranchOptions>(defaultBranchOptions());

// `GitSharedSettings`, until the repository's own arrive. They only turn rows off: the core reads them
// from disk every time.
const DEFAULT_PROTECTED = ["master", "main"];
export const protectedBranches = writable<string[]>(DEFAULT_PROTECTED);

// They are a convenience: if they cannot be read, the defaults apply.
export async function loadBranchState() {
  const [stored, set, settings] = await Promise.all([
    branchOptionsGet().catch(() => ({})),
    favoritesGet().catch(() => null),
    repoSettingsGet().catch(() => null),
  ]);
  branchOptions.set({ ...defaultBranchOptions(), ...stored });
  if (set) favorites.set(set);
  protectedBranches.set(settings?.protectedBranches ?? DEFAULT_PROTECTED);
}

export async function toggleFavorite(name: string, kind: BranchKind) {
  favorites.set(await favoritesToggle(name, kind));
}

export function setBranchOptions(change: Partial<BranchOptions>) {
  const next = { ...get(branchOptions), ...change };
  branchOptions.set(next);
  branchOptionsSet(next)?.catch(() => {});
}

// `repo-changed` fires for any file. What the popup and the panel show only changes with the
// references. If git fails now, it counts as a change: reading too much breaks nothing.
export function refsTracker() {
  let known: string | null = null;
  const current = () => branchRefsFingerprint().catch(() => null);
  return {
    async note() {
      known = await current();
    },
    async changed() {
      const next = await current();
      const differs = next === null || next !== known;
      known = next;
      return differs;
    },
  };
}
