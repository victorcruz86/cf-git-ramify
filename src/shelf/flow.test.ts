import { get } from "svelte/store";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import type {
  ApplyPlan,
  Changelists,
  FileChange,
  Hunk,
  MergeContents,
  PatchApplyResult,
  PlannedFile,
  ShelfEntry,
  ShelfFile,
  Shelved,
  ShelvedList,
} from "../lib/api";
import { applyFlow, type ApplyApi, type ApplyDialogRequest, type ApplyUi } from "../patch/flow";
import { matchShortcut, shortcutsOf } from "../lib/keymap";
import { dismiss, notifications } from "../lib/notifications";
import { onTabRequested } from "../lib/tabs";
import { dialogTexts, type ExecutorChanges } from "../commit/dialog";
import { partKey } from "../commit/tree";
import {
  IMPORT_PATCHES,
  NO_PATCHES,
  SHELF_DELETION,
  UNDO_SHELF_DELETION,
  cannotFullyRestore,
  deletedText,
  importPrompt,
  shelfTabFlow,
  unshelveListNames,
  type ShelfTabApi,
  type ShelfTabUi,
  type Unshelver,
  NOT_SHELVED,
  ONLY_DIRECTORIES,
  ROLLBACK_FAILED,
  SHELVED,
  SHELVE_TITLE,
  SHELVING_FAILED,
  shelfFlow,
  shelfTabRequest,
  shelveEntries,
  shelvingFailedText,
  showShelf,
  silentGroups,
  takeShelfTabRequest,
  viewSavedChanges,
  type ShelfApi,
  type ShelfUi,
} from "./flow";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const LIST: ShelvedList = {
  name: "Fix_it",
  description: "Fix it",
  dateMs: 1,
  recycled: false,
  toDelete: false,
  deleted: false,
  patch: "/data/shelf/Fix_it/shelved.patch",
  binaries: [],
};

const DONE: Shelved = { kind: "done", list: LIST, snapshot: "s1", rollbackError: null, skipped: [] };

function fakeApi(
  shelved: Shelved | Error = DONE,
): ShelfApi & { [K in keyof ShelfApi]: ReturnType<typeof vi.fn> } {
  return {
    shelve: vi.fn(async () => {
      if (shelved instanceof Error) throw { kind: "failed", message: shelved.message };
      return shelved;
    }),
    shelveSilently: vi.fn(async (groups: { name: string }[]) => ({
      lists: groups.map((group) => ({ ...LIST, name: group.name, description: group.name })),
      failed: [],
      snapshot: "s2",
      rollbackError: null,
      skipped: [],
    })),
  } as never;
}

function fakeUi(): ShelfUi & { [K in keyof ShelfUi]: ReturnType<typeof vi.fn> } {
  return { openShelf: vi.fn(), removeIfEmpty: vi.fn(async () => {}) } as never;
}

const CHANGES: ExecutorChanges = {
  repo: "/repo",
  list: "other",
  message: "Fix it",
  files: [
    { path: "a.txt", oldPath: null },
    { path: "new.txt", oldPath: "old.txt" },
  ],
  selections: [{ path: "a.txt", selection: [{ hunk: "h1", lines: [0, 2] }] }],
};

function context(here = true) {
  const order: string[] = [];
  return { order, close: vi.fn(() => order.push("close")), here: vi.fn(() => here) };
}

const shown = () => get(notifications).map(({ kind, title, body }) => ({ kind, title, body }));

beforeEach(() => {
  get(notifications).forEach((notice) => dismiss(notice.id));
  takeShelfTabRequest();
});

afterEach(() => vi.restoreAllMocks());

// `ShelveChangesAction` opens the modal dialog titled `Shelve Changes` with that button
// (V:448), and `ShelveChangesCommitSession.execute` shelves what it included with rollback: the
// whole files and the chosen lines of the others.
test("shelve_changes_opens_the_dialog_titled_shelve_changes_and_shelves_the_included_part", async () => {
  expect(dialogTexts("shelve")).toMatchObject({ title: SHELVE_TITLE, primary: SHELVE_TITLE });
  const api = fakeApi();
  const flow = shelfFlow(api, fakeUi());
  const ctx = context();
  api.shelve.mockImplementation(async () => {
    ctx.order.push("shelve");
    return DONE;
  });

  await flow.shelveChanges(CHANGES, ctx);

  expect(ctx.order).toEqual(["close", "shelve"]);
  expect(api.shelve).toHaveBeenCalledWith(
    [
      { path: "a.txt", oldPath: null, selection: [{ hunk: "h1", lines: [0, 2] }] },
      { path: "new.txt", oldPath: "old.txt", selection: null },
    ],
    "Fix it",
    true,
    "/repo",
  );
  expect(shown()).toEqual([]);
});

// V:489, under the action's name, as `Messages.showErrorDialog(…, shelve.changes.action)`.
test("only_directories_says_they_cannot_be_shelved", async () => {
  const ui = fakeUi();
  const flow = shelfFlow(fakeApi({ kind: "onlyDirectories" }), ui);

  await flow.shelveChanges(CHANGES, context());

  expect(shown()).toEqual([
    {
      kind: "error",
      title: "Shelve Changes",
      body: "The selected changes affect only directories and no files, so they cannot be shelved",
    },
  ]);
  expect(ONLY_DIRECTORIES).toBe(shown()[0].body);
  expect(ui.openShelf).not.toHaveBeenCalled();
  expect(ui.removeIfEmpty).not.toHaveBeenCalled();
});

// `scheduleAutomaticEmptyChangeListDeletion(changeList, true)`: the list the dialog shelved from
// is deleted if it was left empty, only after a shelve that went through and only while its
// repository is still open.
test("an_emptied_changelist_is_deleted", async () => {
  const ui = fakeUi();
  await shelfFlow(fakeApi(), ui).shelveChanges(CHANGES, context());
  expect(ui.removeIfEmpty).toHaveBeenCalledWith("other");

  const failed = fakeUi();
  await shelfFlow(fakeApi(new Error("disk full")), failed).shelveChanges(CHANGES, context());
  expect(failed.removeIfEmpty).not.toHaveBeenCalled();

  const elsewhere = fakeUi();
  await shelfFlow(fakeApi(), elsewhere).shelveChanges(CHANGES, context(false));
  expect(elsewhere.removeIfEmpty).not.toHaveBeenCalled();
  expect(elsewhere.openShelf).not.toHaveBeenCalled();
});

// `ShelvedChangesViewManager.activateView(list)`: the Shelf tab is asked for with the new list to
// select, a request the tab takes when it shows.
test("the_shelf_tab_opens_with_the_new_list_selected", async () => {
  const ui = fakeUi();
  await shelfFlow(fakeApi(), ui).shelveChanges(CHANGES, context());
  expect(ui.openShelf).toHaveBeenCalledWith("Fix_it");

  const tabs: string[] = [];
  const stop = onTabRequested((tab) => tabs.push(tab));
  showShelf("Fix_it");
  stop();
  expect(tabs).toEqual(["shelf"]);
  expect(get(shelfTabRequest)).toEqual({ select: "Fix_it" });
  expect(takeShelfTabRequest()).toEqual({ select: "Fix_it" });
  expect(get(shelfTabRequest)).toBeNull();
  // `Show Shelf` asks for the tab with nothing to select.
  showShelf();
  expect(takeShelfTabRequest()).toEqual({ select: null });
});

// V:343: a failure says `Error creating patch: …`, as IntelliJ's executor does, and the list is
// neither shown nor deleted. What could not be rolled back after shelving is said on its own.
test("a_failed_shelve_is_notified_and_rolls_nothing_back", async () => {
  const ui = fakeUi();
  await shelfFlow(fakeApi(new Error("No space left on device")), ui).shelveChanges(CHANGES, context());
  expect(shown()).toEqual([
    { kind: "error", title: "Error creating patch: No space left on device", body: undefined },
  ]);
  expect(ui.openShelf).not.toHaveBeenCalled();
});

// A rollback that fails after the shelf was written says so with its context: the changes are in
// the shelf, and the copy taken before the rollback is in Restore Snapshot….
test("a_rollback_error_after_shelving_says_the_shelf_was_saved_and_where_the_snapshot_is", async () => {
  const partly: Shelved = { ...DONE, rollbackError: "\"a.txt\" changed meanwhile" } as Shelved;
  await shelfFlow(fakeApi(partly), fakeUi()).shelveChanges(CHANGES, context());
  expect(shown()).toEqual([
    {
      kind: "error",
      title: ROLLBACK_FAILED,
      body:
        "The changes were saved to the shelf as \"Fix it\". A snapshot of the files was taken before " +
        "rolling them back: Restore Snapshot… in Local Changes brings it back.\n\"a.txt\" changed meanwhile",
    },
  ]);
  expect(ROLLBACK_FAILED).toBe("Shelved, but not rolled back");

  get(notifications).forEach((notice) => dismiss(notice.id));
  const api = fakeApi();
  api.shelveSilently.mockResolvedValueOnce({
    lists: [
      { ...LIST, description: "Changes" },
      { ...LIST, description: "Other" },
    ],
    failed: [],
    snapshot: "s2",
    rollbackError: "\"b.txt\" changed meanwhile",
    skipped: [],
  });
  await shelfFlow(api, fakeUi()).shelveSilently("/repo", [], true);
  expect(shown()[1]).toEqual({
    kind: "error",
    title: ROLLBACK_FAILED,
    body:
      "The changes were saved to the shelf as \"Changes\", \"Other\". A snapshot of the files was taken " +
      "before rolling them back: Restore Snapshot… in Local Changes brings it back.\n\"b.txt\" changed meanwhile",
  });
});

// Never lose work: what the shelf did not store (a submodule) was left as it is, and the user is
// told which.
test("what_the_shelf_did_not_store_is_reported", async () => {
  const skipping: Shelved = { ...DONE, skipped: ["sub", "lib/other"] } as Shelved;
  await shelfFlow(fakeApi(skipping), fakeUi()).shelveChanges(CHANGES, context());
  expect(shown()).toEqual([
    {
      kind: "warning",
      title: NOT_SHELVED,
      body: "\"sub\", \"lib/other\" could not be stored in the shelf and were left as they are.",
    },
  ]);

  get(notifications).forEach((notice) => dismiss(notice.id));
  const api = fakeApi();
  api.shelveSilently.mockResolvedValueOnce({
    lists: [LIST],
    failed: [],
    snapshot: "s2",
    rollbackError: null,
    skipped: ["sub"],
  });
  await shelfFlow(api, fakeUi()).shelveSilently("/repo", [], true);
  expect(shown()).toEqual([
    { kind: "info", title: SHELVED, body: undefined },
    { kind: "warning", title: NOT_SHELVED, body: "\"sub\" could not be stored in the shelf and was left as it is." },
  ]);
});

// Review Focus 5: the backend refuses when another repository is open; the refusal is notified
// and nothing else happens.
test("a_repository_switch_during_shelve_is_refused_before_writing", async () => {
  const ui = fakeUi();
  const refused = new Error("Another repository was opened before the changes were shelved");
  await shelfFlow(fakeApi(refused), ui).shelveChanges(CHANGES, context());

  expect(shown()[0].title).toBe(
    "Error creating patch: Another repository was opened before the changes were shelved",
  );
  expect(ui.openShelf).not.toHaveBeenCalled();
  expect(ui.removeIfEmpty).not.toHaveBeenCalled();
});

const change = (path: string, oldPath: string | null = null) =>
  ({ path, oldPath, state: oldPath ? "renamed" : "modified", staged: false, unstaged: true }) satisfies FileChange;

const LISTS: Changelists = {
  lists: [
    { id: "default", name: "Changes", comment: "" },
    { id: "other", name: "Other", comment: "" },
    { id: "third", name: "Third", comment: "" },
  ],
  active: "default",
  assignments: { "a.txt": "default", "b.txt": "other", "new.txt": "default" },
  hunkAssignments: { "a.txt": { h1: "default", h2: "other" } },
};

const HUNKS = new Map<string, Hunk[]>([
  [
    "a.txt",
    [
      { id: "h1", oldStart: 0, oldLines: 1, newStart: 0, newLines: 1 },
      { id: "h2", oldStart: 5, oldLines: 1, newStart: 5, newLines: 1 },
    ],
  ],
]);

// `shelveChangesInSeparatedLists`: one shelf per changelist that has selected changes, in the
// lists' order and named after them; a file split across lists goes with each list's hunks.
test("shelve_silently_makes_one_shelf_per_list_and_rolls_back", async () => {
  const changes = [change("a.txt"), change("b.txt"), change("new.txt", "old.txt")];
  const keys = ["b.txt", "a.txt", partKey("a.txt", "other"), "new.txt"];

  const built = silentGroups(keys, changes, LISTS, HUNKS);

  expect(built).toEqual({
    unreadable: [],
    groups: [
      {
        name: "Changes",
        entries: [
          { path: "a.txt", oldPath: null, selection: [{ hunk: "h1", lines: null }] },
          { path: "new.txt", oldPath: "old.txt", selection: null },
        ],
      },
      {
        name: "Other",
        entries: [
          { path: "b.txt", oldPath: null, selection: null },
          { path: "a.txt", oldPath: null, selection: [{ hunk: "h2", lines: null }] },
        ],
      },
    ],
  });
  // Without the hunks of a split file, nothing can be sent for it.
  expect(silentGroups(["a.txt"], changes, LISTS, new Map()).unreadable).toEqual(["a.txt"]);

  const api = fakeApi();
  await shelfFlow(api, fakeUi()).shelveSilently("/repo", built.groups, true);
  expect(api.shelveSilently).toHaveBeenCalledWith(built.groups, true, "/repo");
  expect(shown()).toEqual([{ kind: "info", title: SHELVED, body: undefined }]);
  expect(SHELVED).toBe("Changes shelved successfully");

  // V:476-477: the lists that failed, by name, after the success of the others, each with its
  // reason (IntelliJ only logs it).
  get(notifications).forEach((notice) => dismiss(notice.id));
  api.shelveSilently.mockResolvedValueOnce({
    lists: [],
    failed: [
      { name: "Other", reason: "No space left on device" },
      { name: "Third", reason: "\"x\" is binary" },
    ],
    snapshot: null,
    rollbackError: null,
    skipped: [],
  });
  await shelfFlow(api, fakeUi()).shelveSilently("/repo", built.groups, true);
  expect(shown()).toEqual([
    { kind: "info", title: SHELVED, body: undefined },
    {
      kind: "error",
      title: SHELVING_FAILED,
      body:
        "Shelving changes for changelists [Other,Third] failed\n" +
        "Other: No space left on device\nThird: \"x\" is binary",
    },
  ]);
  expect(shelvingFailedText(["Other"])).toBe("Shelving changes for changelist [Other] failed");
});

// `ChangesView.SaveToShelve` is only in the keymap, with no shortcut: it shelves the same way
// without rolling back. `Shelve Silently` is ⇧⌘H.
test("save_to_shelf_does_not_roll_back", async () => {
  const shortcut = new KeyboardEvent("keydown", { key: "h", metaKey: true, shiftKey: true });
  expect(matchShortcut(shortcut)).toBe("ChangesView.ShelveSilently");
  expect(shortcutsOf("ChangesView.ShelveSilently")).toEqual(["Meta+Shift+H"]);
  expect(shortcutsOf("ChangesView.SaveToShelve")).toEqual([]);

  const api = fakeApi();
  const groups = [{ name: "Changes", entries: shelveEntries([{ path: "b.txt", oldPath: null }], new Map()) }];
  await shelfFlow(api, fakeUi()).shelveSilently("/repo", groups, false);
  expect(api.shelveSilently).toHaveBeenCalledWith(groups, false, "/repo");
});

// A task that fails as a whole (another repository opened) says so under `Shelving Failed`.
test("a_failed_shelve_silently_is_notified", async () => {
  const api = fakeApi();
  api.shelveSilently.mockRejectedValueOnce({ kind: "failed", message: "Another repository was opened" });
  await shelfFlow(api, fakeUi()).shelveSilently("/repo", [], true);
  expect(shown()).toEqual([{ kind: "error", title: SHELVING_FAILED, body: "Another repository was opened" }]);
});

//
// The Shelf tab
//

const entryOf = (name: string, description: string, files: ShelfFile[], extra: Partial<ShelvedList> = {}): ShelfEntry => ({
  list: { ...LIST, name, description, ...extra },
  changes: files.map((file) => ({
    file,
    path: file.after ?? file.before ?? "",
    oldPath: null,
    status: file.before === null ? "A" : file.after === null ? "D" : "M",
  })),
  error: null,
});

const A: ShelfFile = { before: "a.txt", after: "a.txt", binary: false };
const B: ShelfFile = { before: "b.txt", after: "b.txt", binary: false };
const X: ShelfFile = { before: null, after: "x.bin", binary: true };
const ONE = entryOf("Fix_it", "Fix it", [A, B, X]);
const TWO = entryOf("Other", "Other", [A]);

const TAB_LISTS: Changelists = {
  lists: [
    { id: "default", name: "Changes", comment: "" },
    { id: "l2", name: "Fix it", comment: "" },
  ],
  active: "default",
  assignments: {},
  hunkAssignments: {},
};
const TAB_CONTEXT = { repo: "/repo", head: "h1", lists: TAB_LISTS, current: () => "/repo" };

function tabApi(removeStrategy = false): ShelfTabApi & { [K in keyof ShelfTabApi]: ReturnType<typeof vi.fn> } {
  return {
    delete: vi.fn(async () => ({
      restorable: [{ name: "Fix_it", dateMs: 1 }],
      files: 0,
      lists: 1,
      first: "Fix it",
      errors: [],
    })),
    undoDelete: vi.fn(async () => 0),
    restore: vi.fn(async () => {}),
    rename: vi.fn(async () => LIST),
    findPatches: vi.fn(async (paths: string[]) => paths),
    importPatches: vi.fn(async () => ({ lists: [{ ...LIST, name: "my_fix" }], errors: [] })),
    importIdea: vi.fn(async () => ({ added: 2, skipped: 1 })),
    chooseFiles: vi.fn(async () => ["/p/my_fix.patch"]),
    settings: vi.fn(async () => ({ removeStrategy, showRecycled: false, groupByDirectory: false })),
  } as never;
}

function tabUi(): ShelfTabUi & { [K in keyof ShelfTabUi]: ReturnType<typeof vi.fn> } {
  return {
    confirm: vi.fn(async () => true),
    chooseList: vi.fn(async () => ({ list: { name: "Fix it", comment: "", setActive: false }, removeApplied: true })),
    select: vi.fn(),
  } as never;
}

function unshelver(): Unshelver & { [K in keyof Unshelver]: ReturnType<typeof vi.fn> } {
  return { dialog: vi.fn(async () => {}), silently: vi.fn(async () => "success") } as never;
}

test("unshelve_with_one_list_opens_unshelve_changes", async () => {
  const runner = unshelver();
  const flow = shelfTabFlow(tabApi(true), tabUi(), runner);

  await flow.unshelve(TAB_CONTEXT, [ONE]);
  expect(runner.dialog).toHaveBeenCalledWith(TAB_CONTEXT, {
    name: "Fix_it",
    preselected: null,
    listNames: ["Fix it"],
    removeApplied: true,
  });

  // With some of its files chosen, they are the ones preselected.
  await flow.unshelve(TAB_CONTEXT, [ONE], [{ name: "Fix_it", files: [B] }], false);
  expect(runner.dialog).toHaveBeenLastCalledWith(TAB_CONTEXT, expect.objectContaining({ preselected: [B], removeApplied: false }));
  expect(runner.silently).not.toHaveBeenCalled();

  // A system list goes back to the changelist it came from.
  const system = { ...LIST, description: "Uncommitted changes before Update at 11/8/26 [Feature]", toDelete: true };
  expect(unshelveListNames(system)).toEqual([system.description, "Feature"]);
  expect(unshelveListNames(LIST)).toEqual(["Fix it"]);
});

test("unshelve_with_several_lists_asks_the_changelist_only_with_changelists_on", async () => {
  const runner = unshelver();
  const ui = tabUi();
  const flow = shelfTabFlow(tabApi(false), ui, runner);

  await flow.unshelve(TAB_CONTEXT, [ONE, TWO], [{ name: "Fix_it", files: [A] }]);
  expect(ui.chooseList).toHaveBeenCalledWith({ lists: TAB_LISTS, names: ["Fix it"], removeApplied: false });
  expect(runner.dialog).not.toHaveBeenCalled();
  const list = { name: "Fix it", comment: "", setActive: false };
  expect(runner.silently.mock.calls.map((call) => call[1])).toEqual([
    { name: "Fix_it", files: [A], list, removeApplied: true },
    { name: "Other", files: null, list, removeApplied: true },
  ]);

  // Cancel does nothing.
  ui.chooseList.mockResolvedValueOnce(null);
  await flow.unshelve(TAB_CONTEXT, [ONE, TWO]);
  expect(runner.silently).toHaveBeenCalledTimes(2);

  // Without changelists there is no question: the Remove setting decides.
  await flow.unshelve({ ...TAB_CONTEXT, lists: null }, [ONE, TWO]);
  expect(ui.chooseList).toHaveBeenCalledTimes(2);
  expect(runner.silently.mock.calls.slice(2).map((call) => call[1])).toEqual([
    { name: "Fix_it", files: null, list: null, removeApplied: false },
    { name: "Other", files: null, list: null, removeApplied: false },
  ]);

  // An aborted list stops the rest (`unshelveSilentlyAsynchronously`).
  runner.silently.mockResolvedValueOnce("abort");
  await flow.unshelveSilently(TAB_CONTEXT, [ONE, TWO], []);
  expect(runner.silently).toHaveBeenCalledTimes(5);
});

function shelfPlanned(index: number, file: ShelfFile, extra: Partial<PlannedFile> = {}): PlannedFile {
  const path = file.after ?? file.before ?? "";
  return {
    index,
    path,
    before: file.before,
    after: file.after,
    original: path,
    strip: 0,
    maxStrip: 0,
    kind: file.before === null ? "added" : file.after === null ? "deleted" : "modified",
    binary: file.binary,
    base: !file.binary,
    status: "applicable",
    error: null,
    hashes: { [path]: `hash-of-${path}` },
    shelf: file,
    ...extra,
  };
}

const C: ShelfFile = { before: "c.txt", after: "c.txt", binary: false };
const SHELF_PLAN: ApplyPlan = {
  subject: null,
  message: null,
  author: null,
  additionalInfoError: null,
  readError: null,
  files: [shelfPlanned(0, A), shelfPlanned(1, B), shelfPlanned(2, C), shelfPlanned(3, X)],
};

function unshelveApi(applied: Partial<PatchApplyResult> = {}): ApplyApi & { [K in keyof ApplyApi]: ReturnType<typeof vi.fn> } {
  return {
    plan: vi.fn(async () => SHELF_PLAN),
    apply: vi.fn(async () => ({ snapshot: "snap", status: "failure", failed: ["c.txt"], conflicts: [], messages: [], ...applied })),
    resolve: vi.fn(async () => ({ hash: "h", messages: [] })),
    rollback: vi.fn(async () => "before"),
    merge: vi.fn(async (contents: MergeContents) => ({ id: contents.id, result: "resolved", content: "merged\n" })),
    useList: vi.fn(async () => "l2"),
    moveToList: vi.fn(async () => {}),
    recycle: vi.fn(async () => {}),
    importToShelf: vi.fn(async () => LIST),
    setRemoveApplied: vi.fn(async () => {}),
  } as never;
}

function unshelveUi(rollback = false): ApplyUi & { [K in keyof ApplyUi]: ReturnType<typeof vi.fn> } {
  return {
    dialog: vi.fn(async (request: ApplyDialogRequest) => ({
      source: request.source,
      mapping: { files: [] },
      plan: request.plan,
      selected: [0, 1, 2, 3],
      list: { name: "Fix it", comment: "", setActive: false },
      removeApplied: true,
    })),
    overwrite: vi.fn(async (paths: string[]) => paths),
    errors: vi.fn(async () => {}),
    partlyFailed: vi.fn(async () => rollback),
  } as never;
}

const CONFLICT = {
  path: "b.txt",
  base: "base\n",
  local: "local\n",
  patched: "patched\n",
  localHash: "hb",
  movedTo: null,
  newMode: null,
};

test("remove_applied_recycles_or_splits_as_241", async () => {
  // Through the dialog: a.txt and x.bin applied, b.txt merged in the window, c.txt failed.
  const api = unshelveApi({ conflicts: [CONFLICT] });
  const ui = unshelveUi();
  const flow = applyFlow(ui, api);
  await flow.unshelveWithDialog(TAB_CONTEXT, { name: "Fix_it", preselected: [A], listNames: ["Fix it"], removeApplied: true });

  const request = ui.dialog.mock.calls[0][0] as ApplyDialogRequest;
  expect(request.mode).toBe("unshelve");
  expect(request.source).toEqual({ kind: "shelf", name: "Fix_it", files: null });
  expect(request.removeApplied).toBe(true);
  expect(request.preselected).toEqual([A]);
  expect(request.listNames).toEqual(["Fix it"]);
  expect(api.merge).toHaveBeenCalledTimes(1);
  // Only what went through leaves the shelf: the failed file stays.
  expect(api.recycle).toHaveBeenCalledWith("Fix_it", [A, B, X], "/repo");
  expect(api.moveToList).toHaveBeenCalledWith(["a.txt", "b.txt", "x.bin"], "l2");

  // Without Remove the shelf is not touched.
  const kept = unshelveApi();
  await applyFlow(unshelveUi(), kept).unshelveSilently(TAB_CONTEXT, { name: "Fix_it", files: null, list: null, removeApplied: false });
  expect(kept.apply).toHaveBeenCalledTimes(1);
  expect(kept.recycle).not.toHaveBeenCalled();

  // After a rollback nothing is recycled: the files are back as they were.
  const rolled = unshelveApi();
  await applyFlow(unshelveUi(true), rolled).unshelveSilently(TAB_CONTEXT, {
    name: "Fix_it",
    files: [A, C],
    list: null,
    removeApplied: true,
  });
  expect(rolled.plan).toHaveBeenCalledWith({ kind: "shelf", name: "Fix_it", files: [A, C] }, { files: [] });
  expect(rolled.rollback).toHaveBeenCalledWith("snap");
  expect(rolled.recycle).not.toHaveBeenCalled();
});

test("a_conflict_skipped_in_the_merge_window_stays_in_the_shelf", async () => {
  const api = unshelveApi({ status: "success", failed: [], conflicts: [CONFLICT] });
  api.merge.mockImplementation(async (contents: MergeContents) => ({ id: contents.id, result: "skip" }));
  await applyFlow(unshelveUi(), api).unshelveSilently(TAB_CONTEXT, { name: "Fix_it", files: null, list: null, removeApplied: true });
  expect(api.resolve).not.toHaveBeenCalled();
  expect(api.recycle).toHaveBeenCalledWith("Fix_it", [A, C, X], "/repo");
});

test("import_to_shelf_makes_a_list_and_shows_it", async () => {
  const api = unshelveApi();
  const ui = unshelveUi();
  ui.dialog.mockImplementationOnce(async (request: ApplyDialogRequest) => ({
    source: request.source,
    mapping: { files: [] },
    plan: request.plan,
    selected: [0, 2],
    list: null,
    importToShelf: true,
  }));
  api.plan.mockResolvedValue(SHELF_PLAN);
  const chooseFile = vi.fn(async () => "/p/fix.patch");
  const flow = applyFlow(ui, { ...api, chooseFile, appSettings: vi.fn(async () => ({ folder: null, detectPatchOnTheFly: false })), rememberFolder: vi.fn(async () => {}) } as never);
  const tabs: string[] = [];
  const stop = onTabRequested((tab) => tabs.push(tab));
  takeShelfTabRequest();

  await flow.applyPatch(TAB_CONTEXT);
  stop();
  expect(api.importToShelf).toHaveBeenCalledWith({ kind: "file", path: "/p/fix.patch" }, { files: [] }, ["a.txt", "c.txt"], "/repo");
  expect(api.apply).not.toHaveBeenCalled();
  expect(tabs).toEqual(["shelf"]);
  expect(takeShelfTabRequest()).toEqual({ select: "Fix_it" });
});

test("delete_then_undo_restores_the_list", async () => {
  const api = tabApi();
  const flow = shelfTabFlow(api, tabUi(), unshelver());

  await flow.remove("/repo", ["Fix_it"], [{ name: "Other", files: [A] }]);
  expect(api.delete).toHaveBeenCalledWith(["Fix_it"], [{ name: "Other", files: [A] }], "/repo");
  const shown = get(notifications).find((item) => item.title === SHELF_DELETION);
  expect(shown?.body).toBe("One changelist 'Fix it' deleted successfully");
  expect(shown?.kind).toBe("info");

  shown?.actions?.find((action) => action.label === "Undo")?.run();
  await vi.waitFor(() => expect(api.undoDelete).toHaveBeenCalledWith([{ name: "Fix_it", dateMs: 1 }], "/repo"));
  expect(get(notifications).some((item) => item.title === SHELF_DELETION)).toBe(false);

  // Lists erased meanwhile cannot come back, and Undo says so.
  api.undoDelete.mockResolvedValueOnce(2);
  await flow.remove("/repo", ["Fix_it"], []);
  get(notifications).find((item) => item.title === SHELF_DELETION)?.actions?.[0].run();
  await vi.waitFor(() =>
    expect(get(notifications).find((item) => item.title === UNDO_SHELF_DELETION)?.body).toBe(cannotFullyRestore(2)),
  );
  expect(cannotFullyRestore(1)).toBe("Cannot fully restore shelved changelist as some files were deleted permanently");

  expect(deletedText(1, 0, null)).toBe("One file deleted successfully");
  expect(deletedText(3, 2, "x")).toBe("3 files and 2 changelists deleted successfully");
  expect(deletedText(1, 1, "Fix")).toBe("One file and one changelist 'Fix' deleted successfully");
});

test("import_patches_finds_files_and_asks", async () => {
  const api = tabApi();
  const ui = tabUi();
  const flow = shelfTabFlow(api, ui, unshelver());

  // What was chosen is all patches: no question.
  await flow.importPatches("/repo");
  expect(api.chooseFiles).toHaveBeenCalledWith({ title: IMPORT_PATCHES, defaultPath: "/repo" });
  expect(ui.confirm).not.toHaveBeenCalled();
  expect(api.importPatches).toHaveBeenCalledWith(["/p/my_fix.patch"], "/repo");
  expect(ui.select).toHaveBeenCalledWith("my_fix");

  // Otherwise it asks, and No stops there.
  api.chooseFiles.mockResolvedValueOnce(["/p/a.patch", "/p/notes.txt"]);
  api.findPatches.mockResolvedValueOnce(["/p/a.patch"]);
  ui.confirm.mockResolvedValueOnce(false);
  await flow.importPatches("/repo");
  expect(ui.confirm).toHaveBeenCalledWith(IMPORT_PATCHES, "Found one patch file (/p/a.patch).\nContinue with import?");
  expect(api.importPatches).toHaveBeenCalledTimes(1);
  expect(importPrompt(["a", "b"])).toBe("Found 2 patch files.\nContinue with import?");

  // No patch among them: nothing at all.
  api.findPatches.mockResolvedValueOnce([]);
  await flow.importPatches("/repo");
  expect(ui.confirm).toHaveBeenCalledTimes(1);

  // Files with no change give no list: `No patches found`; a file that cannot be read is listed.
  api.importPatches.mockResolvedValueOnce({ lists: [], errors: [] });
  await flow.importPatches("/repo");
  expect(get(notifications).some((item) => item.title === NO_PATCHES && item.kind === "warning")).toBe(true);
  api.importPatches.mockResolvedValueOnce({ lists: [], errors: ["/p/x.patch: broken"] });
  await flow.importPatches("/repo");
  expect(get(notifications).find((item) => item.title === "Import patches to shelf")?.body).toBe("/p/x.patch: broken");
});

test("shelve_silently_of_one_list_asks_to_rename_it", async () => {
  const api = fakeApi();
  const startEditing = vi.fn();
  const flow = shelfFlow(api, { openShelf: vi.fn(), removeIfEmpty: vi.fn(), startEditing });
  await flow.shelveSilently("/repo", [{ name: "Changes", entries: [] }], true);
  expect(startEditing).toHaveBeenCalledWith("Changes");
  await flow.shelveSilently("/repo", [{ name: "One", entries: [] }, { name: "Two", entries: [] }], true);
  expect(startEditing).toHaveBeenCalledTimes(1);
});

test("a_new_shelf_deletion_hides_the_earlier_one_and_a_partial_failure_keeps_the_undo", async () => {
  const api = tabApi();
  const flow = shelfTabFlow(api, tabUi(), unshelver());
  await flow.remove("/repo", ["Fix_it"], []);
  await flow.remove("/repo", ["Fix_it"], []);
  expect(get(notifications).filter((item) => item.title === SHELF_DELETION)).toHaveLength(1);

  // A list that could not be deleted is said, and what was deleted keeps its Undo.
  api.delete.mockResolvedValueOnce({
    restorable: [{ name: "Fix_it", dateMs: 1 }],
    files: 0,
    lists: 2,
    first: "Fix it",
    errors: ["Gone: \"Gone\" is no longer in the shelf"],
  });
  await flow.remove("/repo", ["Fix_it", "Gone"], []);
  const shown = get(notifications).filter((item) => item.title === SHELF_DELETION);
  expect(shown).toHaveLength(1);
  expect(shown[0].actions?.map((action) => action.label)).toEqual(["Undo"]);
  expect(get(notifications).find((item) => item.kind === "error" && item.title === "Shelf Error")?.body).toBe(
    'Gone: "Gone" is no longer in the shelf',
  );
});

test("a_silent_unshelve_leaves_out_the_binaries_it_cannot_put_back_and_says_why", async () => {
  const api = unshelveApi({ status: "success", failed: [] });
  const why = '"x.bin" changed in HEAD since it was shelved: it was not unshelved';
  api.plan.mockResolvedValue({
    ...SHELF_PLAN,
    files: [shelfPlanned(0, A), shelfPlanned(3, X, { status: "invalid", error: why, hashes: {} })],
  });
  const ui = unshelveUi();
  await applyFlow(ui, api).unshelveSilently(TAB_CONTEXT, { name: "Fix_it", files: null, list: null, removeApplied: true });
  expect(api.apply.mock.calls[0][2].selected).toEqual(["a.txt"]);
  expect(ui.errors).toHaveBeenCalledWith([why]);
  expect(api.recycle).toHaveBeenCalledWith("Fix_it", [A], "/repo");
});

// View saved changes… after an Update that stopped: the system list is selected in the tab, Unshelve...
// opens on it, its conflict goes to the merge window and the merged file leaves the shelf.
test("view_saved_changes_then_unshelve_merges_the_conflict_and_recycles_it", async () => {
  const system = entryOf("Uncommitted_changes", "Uncommitted changes before Update at 11/8/26 [Feature]", [A, B], {
    toDelete: true,
  });
  takeShelfTabRequest();
  viewSavedChanges(
    { kind: "shelve", sha: "", message: "", lists: [{ name: system.list.name, description: system.list.description, changelist: null }] },
    () => {},
  );
  expect(takeShelfTabRequest()).toEqual({ select: "Uncommitted_changes" });

  const api = unshelveApi({ status: "success", failed: [], conflicts: [CONFLICT] });
  api.plan.mockResolvedValue({ ...SHELF_PLAN, files: [shelfPlanned(0, A), shelfPlanned(1, B)] });
  const ui = unshelveUi();
  ui.dialog.mockImplementationOnce(async (request: ApplyDialogRequest) => ({
    source: request.source,
    mapping: { files: [] },
    plan: request.plan,
    selected: [0, 1],
    list: { name: "Feature", comment: "", setActive: false },
    removeApplied: request.removeApplied,
  }));
  const applies = applyFlow(ui, api);
  const flow = shelfTabFlow(tabApi(true), tabUi(), {
    dialog: (context, request) => applies.unshelveWithDialog(context, request),
    silently: (context, request) => applies.unshelveSilently(context, request),
  });
  await flow.unshelve(TAB_CONTEXT, [system]);

  const request = ui.dialog.mock.calls[0][0] as ApplyDialogRequest;
  expect(request.listNames).toEqual([system.list.description, "Feature"]);
  expect(api.merge).toHaveBeenCalledWith(expect.objectContaining({ base: CONFLICT.base, left: CONFLICT.local, right: CONFLICT.patched }));
  expect(api.resolve).toHaveBeenCalledWith(CONFLICT, "merged\n");
  expect(api.recycle).toHaveBeenCalledWith("Uncommitted_changes", [A, B], "/repo");
});

test("the_deletion_text_counts_only_what_was_deleted", async () => {
  const api = tabApi();
  api.delete.mockResolvedValueOnce({
    restorable: [{ name: "Fix_it", dateMs: 1 }],
    files: 0,
    lists: 1,
    first: "Fix it",
    errors: ["Gone: no longer there"],
  });
  await shelfTabFlow(api, tabUi(), unshelver()).remove("/repo", ["Fix_it", "Gone"], []);
  expect(get(notifications).find((item) => item.title === SHELF_DELETION)?.body).toBe(
    "One changelist 'Fix it' deleted successfully",
  );
});
