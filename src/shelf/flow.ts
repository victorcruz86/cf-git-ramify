import { writable } from "svelte/store";
import { showShelf } from "./request";
import { NO_SHELF_SETTINGS, currentShelfSettings } from "./settings";
import {
  shelfDelete,
  shelfFindPatches,
  shelfImportIdea,
  shelfImportPatches,
  shelfRename,
  shelfRestore,
  shelfShelve,
  shelfShelveSilently,
  shelfUndoDelete,
  type ApiError,
  type ApplyStatus,
  type ImportedPatches,
  type ListFiles,
  type Restorable,
  type ShelfDeleted,
  type ShelfEntry,
  type ShelfFile,
  type ShelfImported,
  type ShelfSettings,
  type ShelvedList,
  type Changelists,
  type FileChange,
  type Hunk,
  type LineSelection,
  type SavedChanges,
  type Shelved,
  type ShelvedSilently,
  type ShelveEntry,
  type ShelveGroup,
} from "../lib/api";
import { chooseFiles, type ChooserOptions } from "../lib/choosers";
import { errorText } from "../lib/errors";
import { dismiss, notify } from "../lib/notifications";
import { taskResult } from "../lib/tasks";
import type { ExecutorChanges, ExecutorContext } from "../commit/dialog";
import { listSelections } from "../commit/request";
import { pathOfKey, scopeOfKey } from "../commit/tree";
import { creationError, type ApplyContext, type ListChoice } from "../patch/flow";

// Shelve Changes... (`ShelveChangesCommitExecutor`), Shelve Silently and Save To Shelf
// (`ShelveSilentlyActionBase`, `ShelveChangesManager.shelveSilentlyUnderProgress`). The texts are
// VcsBundle's (V:n is its line), without mnemonics.
export const SHELVE_TITLE = "Shelve Changes"; // V:448 `shelve.changes.action`
// V:489 `shelve.changes.only.directories`.
export const ONLY_DIRECTORIES =
  "The selected changes affect only directories and no files, so they cannot be shelved";
export const SHELVED = "Changes shelved successfully"; // V:478 `shelve.successful.message`
export const SHELVING_FAILED = "Shelving Failed"; // V:476 `shelve.failed.title`
// V:477 `shelve.failed.message`.
export const shelvingFailedText = (names: string[]) =>
  `Shelving changes for ${names.length === 1 ? "changelist" : "changelists"} [${names.join(",")}] failed`;

const quoted = (items: string[]) => items.map((item) => `"${item}"`).join(", ");

// Ramify's own: what the shelf did not store (a folder, a submodule), which was left as it is.
export const NOT_SHELVED = "Some changes were not shelved";
export const notShelvedText = (paths: string[]) =>
  paths.length === 1
    ? `${quoted(paths)} could not be stored in the shelf and was left as it is.`
    : `${quoted(paths)} could not be stored in the shelf and were left as they are.`;

// Ramify's own: the shelf was written but the rollback after it failed.
export const ROLLBACK_FAILED = "Shelved, but not rolled back";
export const rollbackFailedText = (lists: string[], error: string) =>
  `The changes were saved to the shelf as ${quoted(lists)}. A snapshot of the files was taken ` +
  `before rolling them back: Restore Snapshot… in Local Changes brings it back.\n${error}`;

// V:477 and, one per line, why each list failed.
export function shelvingFailedBody(failed: { name: string; reason: string }[]): string {
  const names = failed.map((failure) => failure.name);
  const reasons = failed.map((failure) => `${failure.name}: ${failure.reason}`);
  return [shelvingFailedText(names), ...reasons].join("\n");
}

// Ramify's own: a file split across lists whose hunks the last refresh could not read.
export function unreadableShelfText(paths: string[]): string {
  const files = paths.map((path) => `"${path}"`).join(", ");
  return `Could not read the changes of ${files}, split across lists: nothing was shelved`;
}

//
// The Shelf tab
//

export { shelfTabRequest, showShelf, takeShelfTabRequest, type ShelfTabRequest } from "./request";

// `String.CASE_INSENSITIVE_ORDER`: unit by unit, ignoring case.
const caseInsensitive = (a: string, b: string) => {
  const [left, right] = [a.toUpperCase().toLowerCase(), b.toUpperCase().toLowerCase()];
  return left < right ? -1 : left > right ? 1 : 0;
};

// `GitChangesSaver.showSavedChanges`: the Shelf tab with the first list by name selected
// (`GitShelveChangesSaver`), or, for a stash, `showStashes`: the Stash tab while it is there,
// otherwise the Unstash dialog (`GitStashChangesSaver`).
export function viewSavedChanges(saved: SavedChanges, showStashes: () => void): void {
  if (saved.kind !== "shelve") {
    showStashes();
    return;
  }
  const [first] = [...saved.lists].sort((a, b) => caseInsensitive(a.description, b.description));
  showShelf(first?.name ?? null);
}

//
// Changes
//

// HEAD against what the list has of each file: the whole file, or only its chosen hunks and lines.
export function shelveEntries(
  files: { path: string; oldPath: string | null }[],
  selections: Map<string, LineSelection[]>,
): ShelveEntry[] {
  return files.map((file) => ({
    path: file.path,
    oldPath: file.oldPath,
    selection: selections.get(file.path) ?? null,
  }));
}

// `shelveChangesInSeparatedLists`: the chosen changes of each changelist, in the lists' order and
// named after the list. A row is a whole file of its own list or the part of a file split across
// lists (`ChangeListChange`), which goes with that list's hunks.
export function silentGroups(
  keys: string[],
  changes: FileChange[],
  lists: Changelists,
  hunks: Map<string, Hunk[]>,
): { groups: ShelveGroup[]; unreadable: string[] } {
  const listOf = (key: string) => {
    const path = pathOfKey(key);
    return scopeOfKey(key) || (lists.assignments[path] ?? lists.active);
  };
  const byPath = new Map(changes.map((change) => [change.path, change]));
  const groups: ShelveGroup[] = [];
  const unreadable = new Set<string>();
  for (const list of lists.lists) {
    const own = keys.filter((key) => listOf(key) === list.id);
    if (own.length === 0) continue;
    const chosen = listSelections(new Set(own), lists, hunks, new Map());
    chosen.unreadable.forEach((path) => unreadable.add(path));
    const files = [...chosen.included].map((path) => ({
      path,
      oldPath: byPath.get(path)?.oldPath ?? null,
    }));
    if (files.length > 0) {
      groups.push({ name: list.name, entries: shelveEntries(files, chosen.selections) });
    }
  }
  return { groups, unreadable: [...unreadable] };
}

//
// Flow
//

export interface ShelfApi {
  shelve(
    entries: ShelveEntry[],
    description: string,
    rollback: boolean,
    repo: string,
  ): Promise<Shelved>;
  shelveSilently(groups: ShelveGroup[], rollback: boolean, repo: string): Promise<ShelvedSilently>;
}

export const defaultShelfApi: ShelfApi = {
  shelve: async (entries, description, rollback, repo) =>
    taskResult<Shelved>(await shelfShelve(entries, description, rollback, repo)),
  shelveSilently: async (groups, rollback, repo) =>
    taskResult<ShelvedSilently>(await shelfShelveSilently(groups, rollback, repo)),
};

// What the application does after a Shelve Changes that went through: the Shelf tab with the list
// selected, and the list the dialog shelved from deleted if it was left empty.
export interface ShelfUi {
  openShelf(name: string): void;
  removeIfEmpty(list: string): Promise<void> | void;
  // After Shelve Silently made one list: its name is edited in place while the Shelf tab is in
  // front (`startEditing`).
  startEditing?(name: string): void;
}

export const defaultShelfUi: ShelfUi = {
  openShelf: showShelf,
  removeIfEmpty: () => {},
  startEditing: (name) => requestShelfEdit(name),
};

export function shelfFlow(api: ShelfApi = defaultShelfApi, ui: ShelfUi = defaultShelfUi) {
  // `Shelve Changes` of the dialog (`ShelveChangesCommitSession.execute`): the dialog closes, the
  // included part is shelved with rollback, then `activateView(list)` and the emptied list's
  // deletion, while the repository is still the open one.
  async function shelveChanges(changes: ExecutorChanges, context: ExecutorContext): Promise<void> {
    context.close();
    const selections = new Map(changes.selections.map((file) => [file.path, file.selection]));
    const entries = shelveEntries(changes.files, selections);
    let shelved: Shelved;
    try {
      shelved = await api.shelve(entries, changes.message, true, changes.repo);
    } catch (failure) {
      notify({ kind: "error", title: creationError(errorText(failure)) });
      return;
    }
    if (shelved.kind === "onlyDirectories") {
      notify({ kind: "error", title: SHELVE_TITLE, body: ONLY_DIRECTORIES });
      return;
    }
    if (shelved.skipped.length > 0) {
      notify({ kind: "warning", title: NOT_SHELVED, body: notShelvedText(shelved.skipped) });
    }
    if (shelved.rollbackError) {
      const body = rollbackFailedText([shelved.list.description], shelved.rollbackError);
      notify({ kind: "error", title: ROLLBACK_FAILED, body });
    }
    if (!context.here()) return;
    ui.openShelf(shelved.list.name);
    await ui.removeIfEmpty(changes.list);
  }

  // `Shelve Silently` (`rollback`) and `Save To Shelf`: `Changes shelved successfully`, then the
  // lists that failed with their reasons, what no list stored, and a failed rollback.
  async function shelveSilently(repo: string, groups: ShelveGroup[], rollback: boolean): Promise<void> {
    let shelved: ShelvedSilently;
    try {
      shelved = await api.shelveSilently(groups, rollback, repo);
    } catch (failure) {
      notify({ kind: "error", title: SHELVING_FAILED, body: errorText(failure) });
      return;
    }
    notify({ kind: "info", title: SHELVED });
    if (shelved.lists.length === 1) ui.startEditing?.(shelved.lists[0].name);
    if (shelved.failed.length > 0) {
      notify({ kind: "error", title: SHELVING_FAILED, body: shelvingFailedBody(shelved.failed) });
    }
    if (shelved.skipped.length > 0) {
      notify({ kind: "warning", title: NOT_SHELVED, body: notShelvedText(shelved.skipped) });
    }
    if (shelved.rollbackError) {
      const lists = shelved.lists.map((list) => list.description);
      notify({ kind: "error", title: ROLLBACK_FAILED, body: rollbackFailedText(lists, shelved.rollbackError) });
    }
  }

  return { shelveChanges, shelveSilently };
}

// The application's: Shelve Silently and Save To Shelf of Local Changes.
export const shelves = shelfFlow();

//
// The Shelf tab
//

// `shelve.deletion.title` (V:451), `shelve.undo.deletion` (V:465), `shelve.changes.restore.error`
// (V:487, with its missing space), `import.patches` (V:855), `shelve.import.*.prompt` (V:457-458),
// `patch.import.to.shelf.tab` (V:444), `patch.import.no.patches.found.warning` (V:445),
// `unshelve.changelist.chooser.title` (V:536) and `action.import.to.shelf` (V:447).
export const SHELF_DELETION = "Shelf Deletion";
export const SHELF_ERROR = "Shelf Error"; // V:471 `shelve.error.title`
export const UNDO_SHELF_DELETION = "Undo Shelf Deletion";
export const cannotFullyRestore = (count: number) =>
  `Cannot fully restore ${count === 1 ? "shelved changelist" : `${count}shelved changelists`} as some files were deleted permanently`;
export const IMPORT_PATCHES = "Import Patches";
export const importPrompt = (found: string[]) =>
  found.length === 1
    ? `Found one patch file (${found[0]}).\nContinue with import?`
    : `Found ${found.length} patch files.\nContinue with import?`;
export const IMPORT_ERRORS = "Import patches to shelf";
export const NO_PATCHES = "No patches found";
export const UNSHELVE_TO_CHANGELIST = "Unshelve Changes to Changelist";
export const IMPORT_TO_SHELF = "Import to Shelf";
// Ramify's own: `Import from .idea` (decision 5 of the overview).
export const IMPORT_IDEA = "Import from .idea";
export const importedIdeaText = (imported: ShelfImported) =>
  `${imported.added === 1 ? "1 shelved changelist" : `${imported.added} shelved changelists`} imported, ` +
  `${imported.skipped} skipped`;

// `constructDeleteSuccessfullyMessage`: `One file and one changelist 'Fix' deleted successfully`.
export function deletedText(files: number, lists: number, first: string | null): string {
  const filesText = files === 0 ? "" : files === 1 ? "one file" : `${files} files`;
  const description = lists === 1 && first !== null ? first : "";
  const listsText =
    lists === 0
      ? ""
      : `${lists === 1 ? "one changelist" : `${lists} changelists`}${description ? ` '${description}'` : ""}`;
  const text = `${filesText}${files > 0 && lists > 0 ? " and " : ""}${listsText} deleted successfully`;
  return text.charAt(0).toUpperCase() + text.slice(1);
}

// `ChangeListUtil.getChangeListNameForUnshelve` and `getPredefinedChangeList`: a system list is named
// `<message> [<list>]`, and goes back to `<list>`.
const SYSTEM_NAME = /^.*\s\[(.*)\]$/s;
export function unshelveListNames(list: ShelvedList): string[] {
  const original = list.toDelete ? (SYSTEM_NAME.exec(list.description)?.[1] ?? list.description) : null;
  return original !== null && original !== list.description ? [list.description, original] : [list.description];
}

// What unshelving needs of the apply flow (`src/patch/flow.ts`).
export interface Unshelver {
  // `MyUnshelveDialog`: the dialog over one list, with the files chosen in it.
  dialog(context: ApplyContext, request: UnshelveDialogRequest): Promise<void>;
  // `unshelveChangeList` with no dialog, into `list` (the active list when `null`).
  silently(context: ApplyContext, request: SilentUnshelve): Promise<ApplyStatus | null>;
}

export interface UnshelveDialogRequest {
  name: string;
  preselected: ShelfFile[] | null;
  listNames: string[];
  removeApplied: boolean;
}

export interface SilentUnshelve {
  name: string;
  files: ShelfFile[] | null;
  list: ListChoice | null;
  removeApplied: boolean;
}

export interface ShelfTabApi {
  delete(lists: string[], files: ListFiles[], repo: string): Promise<ShelfDeleted>;
  undoDelete(lists: Restorable[], repo: string): Promise<number>;
  restore(names: string[], repo: string): Promise<void>;
  rename(name: string, description: string, repo: string): Promise<ShelvedList>;
  findPatches(paths: string[]): Promise<string[]>;
  importPatches(files: string[], repo: string): Promise<ImportedPatches>;
  importIdea(repo: string): Promise<ShelfImported>;
  chooseFiles(options: ChooserOptions): Promise<string[]>;
  settings(): Promise<ShelfSettings>;
}

export const defaultShelfTabApi: ShelfTabApi = {
  delete: async (lists, files, repo) => taskResult<ShelfDeleted>(await shelfDelete(lists, files, repo)),
  undoDelete: async (lists, repo) => taskResult<number>(await shelfUndoDelete(lists, repo)),
  restore: async (names, repo) => {
    await taskResult<null>(await shelfRestore(names, repo));
  },
  rename: async (name, description, repo) =>
    taskResult<ShelvedList>(await shelfRename(name, description, repo)),
  findPatches: async (paths) => taskResult<string[]>(await shelfFindPatches(paths)),
  importPatches: async (files, repo) => taskResult<ImportedPatches>(await shelfImportPatches(files, repo)),
  importIdea: async (repo) => taskResult<ShelfImported>(await shelfImportIdea(repo)),
  chooseFiles,
  settings: async () => currentShelfSettings(),
};

// The questions of the tab.
export interface ShelfTabUi {
  // `Messages.showYesNoDialog`.
  confirm(title: string, message: string): Promise<boolean>;
  // `ChangeListChooser` titled `Unshelve Changes to Changelist`, with the Remove checkbox.
  chooseList(request: ListChooserRequest): Promise<{ list: ListChoice; removeApplied: boolean } | null>;
  // `activateView(list)`.
  select(name: string): void;
}

export interface ListChooserRequest {
  lists: Changelists;
  names: string[];
  removeApplied: boolean;
}

const cancelled = (failure: unknown) => (failure as ApiError | undefined)?.kind === "cancelled";

export function shelfTabFlow(api: ShelfTabApi, ui: ShelfTabUi, unshelver: Unshelver) {
  // `showNotificationAndHideExisting(…, ShelfDeleteNotification.class)`: one at a time.
  let deletion: number | null = null;

  const failed = (title: string) => (failure: unknown) => {
    if (!cancelled(failure)) notify({ kind: "error", title, body: errorText(failure) });
  };

  // `UnshelveWithDialogAction`: one list opens `Unshelve Changes` with the files chosen in it (all
  // of them when it has none, or all are chosen); several are unshelved silently into the list the
  // chooser gives, which is only asked with changelists on.
  async function unshelve(
    context: ApplyContext,
    lists: ShelfEntry[],
    files: ListFiles[] = [],
    removeApplied: boolean | null = null,
  ): Promise<void> {
    if (lists.length === 0) return;
    let remove = removeApplied;
    if (remove === null) remove = (await api.settings().catch(() => NO_SHELF_SETTINGS)).removeStrategy;
    const chosenIn = (entry: ShelfEntry) => files.find((list) => list.name === entry.list.name)?.files ?? null;
    if (lists.length === 1) {
      const [entry] = lists;
      const chosen = chosenIn(entry);
      const all = chosen === null || chosen.length === entry.changes.length;
      await unshelver.dialog(context, {
        name: entry.list.name,
        preselected: all ? null : chosen,
        listNames: unshelveListNames(entry.list),
        removeApplied: remove,
      });
      return;
    }
    let target: ListChoice | null = null;
    if (context.lists) {
      const answer = await ui.chooseList({
        lists: context.lists,
        names: [lists[0].list.description],
        removeApplied: remove,
      });
      if (!answer) return;
      target = answer.list;
      remove = answer.removeApplied;
    }
    await silently(context, lists, files, target, remove);
  }

  // `unshelveSilentlyAsynchronously`: each list in turn, only its chosen files when it has some,
  // until one is aborted.
  async function silently(
    context: ApplyContext,
    lists: ShelfEntry[],
    files: ListFiles[],
    list: ListChoice | null,
    removeApplied: boolean,
  ): Promise<void> {
    for (const entry of lists) {
      const chosen = files.find((item) => item.name === entry.list.name)?.files ?? null;
      const status = await unshelver.silently(context, {
        name: entry.list.name,
        files: chosen && chosen.length > 0 ? chosen : null,
        list,
        removeApplied,
      });
      if (status === "abort") break;
    }
  }

  // `UnshelveSilentlyAction`: into the active list, with the Remove setting.
  async function unshelveSilently(context: ApplyContext, lists: ShelfEntry[], files: ListFiles[]): Promise<void> {
    if (lists.length === 0) return;
    const remove = (await api.settings().catch(() => NO_SHELF_SETTINGS)).removeStrategy;
    await silently(context, lists, files, null, remove);
  }

  // `Delete`: no question; then `Shelf Deletion` with `Undo` (`showUndoDeleteNotification`).
  async function remove(repo: string, lists: string[], files: ListFiles[]): Promise<void> {
    if (lists.length === 0 && files.length === 0) return;
    let deleted: ShelfDeleted;
    try {
      deleted = await api.delete(lists, files, repo);
    } catch (failure) {
      failed(SHELF_DELETION)(failure);
      return;
    }
    if (deleted.errors.length > 0) {
      notify({ kind: "error", title: SHELF_ERROR, body: deleted.errors.join("\n") });
    }
    if (deleted.restorable.length === 0) return;
    if (deletion !== null) dismiss(deletion);
    const id = notify({
      kind: "info",
      title: SHELF_DELETION,
      body: deletedText(deleted.files, deleted.lists, deleted.first),
      actions: [{ label: "Undo", run: () => void undo(repo, deleted.restorable, id) }],
    });
    deletion = id;
  }

  async function undo(repo: string, lists: Restorable[], id: number) {
    dismiss(id);
    try {
      const gone = await api.undoDelete(lists, repo);
      if (gone > 0) notify({ kind: "warning", title: UNDO_SHELF_DELETION, body: cannotFullyRestore(gone) });
    } catch (failure) {
      failed(UNDO_SHELF_DELETION)(failure);
    }
  }

  async function restore(repo: string, names: string[]): Promise<void> {
    if (names.length === 0) return;
    await api.restore(names, repo).catch(failed("Restore"));
  }

  // The inline `Rename...`: the description only (`renameChangeList`).
  async function rename(repo: string, name: string, description: string): Promise<void> {
    await api.rename(name, description, repo).catch(failed("Rename"));
  }

  // `ImportIntoShelfAction`: the chosen files, the patches among them (and inside folders), a
  // question when that is not exactly what was chosen, then one list per patch file.
  async function importPatches(repo: string): Promise<void> {
    let chosen: string[];
    let found: string[];
    try {
      chosen = await api.chooseFiles({ title: IMPORT_PATCHES, defaultPath: repo });
      if (chosen.length === 0) return;
      found = await api.findPatches(chosen);
    } catch (failure) {
      failed(IMPORT_ERRORS)(failure);
      return;
    }
    if (found.length === 0) return;
    const same = found.length === chosen.length && found.every((path, index) => path === chosen[index]);
    if (!same && !(await ui.confirm(IMPORT_PATCHES, importPrompt(found)))) return;
    let imported: ImportedPatches;
    try {
      imported = await api.importPatches(found, repo);
    } catch (failure) {
      failed(IMPORT_ERRORS)(failure);
      return;
    }
    const last = imported.lists[imported.lists.length - 1];
    if (last) ui.select(last.name);
    if (imported.errors.length > 0) {
      notify({ kind: "error", title: IMPORT_ERRORS, body: imported.errors.join("\n") });
    }
    if (imported.lists.length === 0 && imported.errors.length === 0) {
      notify({ kind: "warning", title: NO_PATCHES });
    }
  }

  async function importIdea(repo: string): Promise<void> {
    try {
      const imported = await api.importIdea(repo);
      notify({ kind: "info", title: IMPORT_IDEA, body: importedIdeaText(imported) });
    } catch (failure) {
      failed(IMPORT_IDEA)(failure);
    }
  }

  return { unshelve, unshelveSilently, remove, restore, rename, importPatches, importIdea };
}

// A request of Shelve Silently to rename its one new list in place (`startEditing`), which the tab
// takes only while it is the one in front.
const editRequest = writable<string | null>(null);
export const shelfEditRequest = { subscribe: editRequest.subscribe };
export const requestShelfEdit = (name: string) => editRequest.set(name);
export const clearShelfEdit = () => editRequest.set(null);
