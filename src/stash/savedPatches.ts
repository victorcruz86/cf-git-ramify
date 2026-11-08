// What the Stash tab and the Shelf tab share (`SavedPatchesUi`, `SavedPatchesProvider`): a change of a
// saved patch, how it is grouped, and the two texts its diff shows.
import type { ChangedFile, Side } from "../lib/api";
import { sectionOf, type ChangeSection } from "../log/changes";
import { speedMatch } from "../lib/speedSearch";

// `SavedPatchesProvider.ChangeObject`. `group` is its tag (`Index on main`), `null` for the patch's own
// changes. `source` says to the provider where its texts are (for a stash, the commit).
export interface SavedChange {
  file: ChangedFile;
  group: string | null;
  source: string;
}

// The two sides of a diff and their titles (`ChangeDiffRequestProducer.getRevisionTitle`).
export interface SavedDiff {
  left: Side;
  right: Side;
  titles: [string, string];
}

// `Show Diff`, `Compare with Local` and `Compare Before with Local`.
export type DiffMode = "change" | "local" | "beforeLocal";

// `VcsBundle` `saved.patch.*` and `ActionsBundle`.
export const SAVED_TEXTS = {
  empty: "No stashes or shelves added.",
  changesEmpty: "Select stash or shelf to view files",
  loading: "Loading...",
  apply: "Apply",
  pop: "Pop",
  // `saved.patch.apply.pop.help.tooltip`, with its `<br/>` as line breaks.
  help: "Apply — apply and keep the stash\nin the list.\nPop — apply and remove the stash\nfrom the list.",
  showDiff: "Show Diff",
  showStandaloneDiff: "Show Diff in a New Tab or Window",
  compareWithLocal: "Compare with Local",
  compareBeforeWithLocal: "Compare Before with Local",
} as const;

export const changeKey = (change: SavedChange) =>
  `${change.group ?? ""}\0${change.source}\0${change.file.path}`;

// `SavedPatchesChangesBrowser.buildTreeModelSync`: the changes grouped by tag in the order they come,
// those without a tag at the root; inside each, by folder.
export function changeSections(changes: SavedChange[]): ChangeSection<SavedChange>[] {
  const groups = new Map<string | null, SavedChange[]>();
  for (const change of changes) {
    groups.set(change.group, [...(groups.get(change.group) ?? []), change]);
  }
  return [...groups].map(([group, grouped]) => sectionOf(group, grouped));
}

// The changes in the order the tree shows them, which is the order of `Show Diff`'s chain.
export const shownChanges = (changes: SavedChange[]) =>
  changeSections(changes).flatMap((section) => section.folders.flatMap((folder) => folder.changes));

export const fileName = (path: string) => path.slice(path.lastIndexOf("/") + 1);

// `SavedPatchesTree.MySpeedSearch.isMatchingElement`: the row's text, or the name of one of its
// changes already loaded.
export function patchMatches(text: string, changes: SavedChange[] | undefined, query: string) {
  if (speedMatch(text, query)) return true;
  return (changes ?? []).some((change) => speedMatch(fileName(change.file.path), query) !== null);
}
