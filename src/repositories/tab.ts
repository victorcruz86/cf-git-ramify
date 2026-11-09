import { get, writable } from "svelte/store";
import { activateWindow, registeredToolWindows, registerToolWindow } from "../shell/toolWindows";

// `Hash.toShortString` (`DvcsUtil.getShortHash`).
const SHORT_HASH = 8;

// What the `Repositories` tool window shows: the repository at `hash`, a full hash. `id` changes
// with each new tab, never when the same revision is shown again.
export interface RepositoryTab {
  id: number;
  hash: string;
}

const store = writable<RepositoryTab | null>(null);
export const repositoryTab = { subscribe: store.subscribe };
let lastId = 0;

// `tab.title.repo.root.name.at.revision` (`B:1367`): `<root name> at <short hash>`.
export function repositoryTabTitle(root: string, hash: string): string {
  const name = root.split("/").filter(Boolean).pop() ?? root;
  return `${name} at ${hash.slice(0, SHORT_HASH)}`;
}

// `GitBrowseRepoAtRevisionAction` and `RepositoryBrowser.showRepositoryBrowser`: the window is
// registered on first use, and comes to the front. A tab of the same root and revision is reused as
// it is; rama has one repository per window, so another revision replaces the tab (decision 5).
export function showRepositoryAtRevision(hash: string): void {
  if (get(store)?.hash !== hash) store.set({ id: ++lastId, hash });
  registerToolWindow("repositories");
  activateWindow("repositories");
}

// Opening another repository forgets the window, and its tab with it.
registeredToolWindows.subscribe((ids) => {
  if (!ids.includes("repositories") && get(store) !== null) store.set(null);
});
