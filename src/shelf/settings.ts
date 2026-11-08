import { get, writable } from "svelte/store";
import { repoSettingsGet, shelfSettingsSet, type ShelfSettings } from "../lib/api";

// The Shelf tab's options of the open repository (`ShelveChangesManager.State`), with one writer:
// the tab's toggles and the Remove checkboxes of the unshelve dialogs change one field each over
// what is here, so none writes back a value another one changed.
//
// The store belongs to one repository: it starts again when another one opens. Until it has read
// that repository's settings, a save reads them first, and does not write if it cannot (it would
// put the previous repository's fields, or the defaults, over the real ones). Every change counts a
// generation: a read that started before it does not put the old values back.
export const NO_SHELF_SETTINGS: ShelfSettings = {
  removeStrategy: false,
  showRecycled: false,
  groupByDirectory: false,
};

const current = writable<ShelfSettings>(NO_SHELF_SETTINGS);
export const shelfSettings = { subscribe: current.subscribe };

let loaded = false;
let generation = 0;

export const shelfSettingsGeneration = () => generation;

// Another repository was opened: nothing of the previous one is kept.
export function resetShelfSettings(): void {
  generation += 1;
  loaded = false;
  current.set(NO_SHELF_SETTINGS);
}

// What the repository has, read when `started` was the generation (the current one when absent).
// `false`: something changed since the read began, and it is dropped.
export function loadedShelfSettings(settings: ShelfSettings, started = generation): boolean {
  if (started !== generation) return false;
  loaded = true;
  current.set(settings);
  return true;
}

export const currentShelfSettings = () => get(current);

let writer: ((settings: ShelfSettings) => Promise<void>) | null = null;
let reader: (() => Promise<ShelfSettings>) | null = null;

const readBackend = async () => (await repoSettingsGet()).shelf ?? NO_SHELF_SETTINGS;

// For the tests: where the settings are written and read instead of the backend (`null`: the
// backend).
export function setShelfSettingsWriter(write: ((settings: ShelfSettings) => Promise<void>) | null): void {
  writer = write;
}

export function setShelfSettingsReader(read: (() => Promise<ShelfSettings>) | null): void {
  reader = read;
}

export async function saveShelfSetting(change: Partial<ShelfSettings>): Promise<void> {
  if (!loaded) {
    const started = generation;
    const read = await (reader ?? readBackend)();
    if (started === generation) loadedShelfSettings(read, started);
    else if (!loaded) throw new Error("The shelf settings changed while they were read");
  }
  generation += 1;
  const next = { ...get(current), ...change };
  current.set(next);
  await (writer ?? shelfSettingsSet)(next);
}
