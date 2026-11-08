import { get, writable } from "svelte/store";
import { requestTab } from "../lib/tabs";

// `ShelvedChangesViewManager.activateView(list)` and `Show Shelf`: the Shelf tab is asked for, with
// the list to select. The request stays until the tab takes it, since the tab only exists once the
// shelf has a list.
export interface ShelfTabRequest {
  select: string | null;
}

const tabRequest = writable<ShelfTabRequest | null>(null);

export const shelfTabRequest = { subscribe: tabRequest.subscribe };

export function showShelf(select: string | null = null): void {
  tabRequest.set({ select });
  requestTab("shelf");
}

// What the tab calls when it shows: the pending request, which is then gone.
export function takeShelfTabRequest(): ShelfTabRequest | null {
  const request = get(tabRequest);
  tabRequest.set(null);
  return request;
}

