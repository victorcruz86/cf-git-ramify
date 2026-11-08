import { EditorView } from "@codemirror/view";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { get } from "svelte/store";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import type { ShelfChange, ShelfEntry, ShelfSettings, ShelfSides, ShelvedList } from "../lib/api";
import { prettyDateTime } from "../log/draw";
import { clearShelfEdit, requestShelfEdit } from "./flow";
import { shelfDialogsUi } from "./dialogs";
import {
  loadedShelfSettings,
  resetShelfSettings,
  saveShelfSetting,
  setShelfSettingsReader,
  setShelfSettingsWriter,
  shelfSettings,
  shelfSettingsGeneration,
} from "./settings";
import ShelfDialogs from "./ShelfDialogs.svelte";
import ShelfTab from "./ShelfTab.svelte";

// jsdom does no layout: CodeMirror measures with this.
Range.prototype.getClientRects = () => [] as unknown as DOMRectList;
Element.prototype.scrollIntoView = () => {};

const NOW = new Date(2026, 10, 8, 12, 0).getTime();
const MINUTE = 60_000;
const DAY = 24 * 60 * MINUTE;

function list(name: string, description: string, dateMs: number, flags: Partial<ShelvedList> = {}): ShelvedList {
  return {
    name,
    description,
    dateMs,
    recycled: false,
    toDelete: false,
    deleted: false,
    patch: `/data/shelf/${name}/shelved.patch`,
    binaries: [],
    ...flags,
  };
}

function change(path: string, status = "M", binary = false): ShelfChange {
  return {
    file: { before: status === "A" ? null : path, after: status === "D" ? null : path, binary },
    path,
    oldPath: null,
    status,
  };
}

const FIX: ShelfEntry = {
  list: list("Fix_it", "Fix it", NOW - 30 * MINUTE),
  changes: [change("src/a.txt"), change("b.txt", "A")],
  error: null,
};
const OLDER: ShelfEntry = {
  list: list("Older", "Older", NOW - 3 * DAY),
  changes: [change("c.txt", "D")],
  error: null,
};
const APPLIED: ShelfEntry = {
  list: list("Applied", "Applied", NOW - 10 * MINUTE, { recycled: true }),
  changes: [change("d.txt")],
  error: null,
};
const GONE: ShelfEntry = {
  list: list("Gone", "Gone", NOW - DAY, { deleted: true }),
  changes: [change("e.txt")],
  error: null,
};
const BROKEN: ShelfEntry = {
  list: list("Broken", "", NOW - 2 * DAY),
  changes: [],
  error: "Cannot load patch file: Unknown chunk start syntax",
};

const SETTINGS: ShelfSettings = { removeStrategy: false, showRecycled: false, groupByDirectory: false };

const text = (content: string) => ({ kind: "text" as const, content });

function show(entries: ShelfEntry[] = [OLDER, FIX, APPLIED, GONE], settings: ShelfSettings = SETTINGS) {
  const props = {
    entries,
    settings,
    active: true,
    now: () => NOW,
    api: {
      sides: vi.fn(
        async (): Promise<ShelfSides> => ({
          left: text("base\n"),
          right: text("shelved\n"),
          titles: ["Base version", "Shelved version"],
        }),
      ),
    },
    onunshelve: vi.fn(),
    ondelete: vi.fn(),
    onrestore: vi.fn(),
    onrename: vi.fn(),
    onimport: vi.fn(),
    onimportidea: vi.fn(),
    oncreatepatch: vi.fn(),
    oncopypatch: vi.fn(),
    onsettings: vi.fn(),
  };
  const result = render(ShelfTab, props);
  return { props, ...result, tree: screen.getByRole("tree", { name: "Shelf" }) };
}

const rows = (tree: HTMLElement) =>
  within(tree)
    .getAllByRole("treeitem")
    .map((item) => item.getAttribute("aria-label"));
const row = (name: string) => screen.getByRole("treeitem", { name: new RegExp(`^${name}`) });

const views = () =>
  [...document.querySelectorAll<HTMLElement>(".cm-editor")].map((dom) => EditorView.findFromDOM(dom) as EditorView);

beforeEach(() => localStorage.clear());
afterEach(() => {
  cleanup();
  clearShelfEdit();
});

test("the_shelf_tab_lists_newest_first_with_count_and_pretty_date", async () => {
  const { tree } = show([OLDER, FIX, APPLIED, GONE, BROKEN]);

  expect(rows(tree)).toEqual([
    `Fix it  2 files, ${prettyDateTime(FIX.list.dateMs, NOW)}`,
    `<empty name>  No files, ${prettyDateTime(BROKEN.list.dateMs, NOW)} Cannot load patch file: Unknown chunk start syntax`,
    `Older  1 file, ${prettyDateTime(OLDER.list.dateMs, NOW)}`,
    "Recently Deleted",
  ]);
  expect(prettyDateTime(FIX.list.dateMs, NOW)).toBe("30 minutes ago");
  // A recycled list is not there without `Already Unshelved`; the first row is selected.
  expect(row("Fix it").getAttribute("aria-selected")).toBe("true");

  // Opening a list shows its files by path, with their folders after the name.
  await fireEvent.keyDown(tree, { key: "ArrowRight" });
  expect(rows(tree).slice(1, 3)).toEqual(["b.txt", "a.txt src"]);
});

test("already_unshelved_shows_recycled_lists_grey", async () => {
  const { tree, props, rerender } = show();

  await fireEvent.click(screen.getByRole("button", { name: "View Options" }));
  const menu = screen.getByRole("menu", { name: "View Options" });
  const item = within(menu).getByRole("menuitem", { name: "Already Unshelved" });
  await fireEvent.click(item);
  expect(props.onsettings).toHaveBeenCalledWith({ ...SETTINGS, showRecycled: true });

  await rerender({ ...props, settings: { ...SETTINGS, showRecycled: true } });
  expect(rows(tree)[0]).toMatch(/^Applied/);
  expect(row("Applied").classList.contains("greyed")).toBe(true);
  expect(row("Fix it").classList.contains("greyed")).toBe(false);

  // The deleted lists are under `Recently Deleted`, grey too.
  await fireEvent.click(row("Recently Deleted"));
  await fireEvent.keyDown(tree, { key: "ArrowRight" });
  expect(row("Gone").classList.contains("greyed")).toBe(true);
});

test("rename_changes_the_description_only", async () => {
  const { tree, props } = show();

  await fireEvent.keyDown(tree, { key: "F2" });
  const field = screen.getByRole("textbox", { name: "Rename" }) as HTMLInputElement;
  expect(field.value).toBe("Fix it");
  await fireEvent.input(field, { target: { value: "Better name" } });
  await fireEvent.keyDown(field, { key: "Enter" });
  expect(props.onrename).toHaveBeenCalledWith("Fix_it", "Better name");
  expect(screen.queryByRole("textbox", { name: "Rename" })).toBeNull();

  // Escape gives up; ⇧F6 is the same action.
  await fireEvent.keyDown(tree, { key: "F6", shiftKey: true });
  const again = screen.getByRole("textbox", { name: "Rename" });
  await fireEvent.keyDown(again, { key: "Escape" });
  expect(props.onrename).toHaveBeenCalledTimes(1);
});

test("shelve_silently_starts_renaming_its_list_when_the_tab_is_in_front", async () => {
  show();

  requestShelfEdit("Older");
  const field = (await screen.findByRole("textbox", { name: "Rename" })) as HTMLInputElement;
  expect(field.value).toBe("Older");
});

test("delete_has_no_question_and_takes_the_selection", async () => {
  const { tree, props } = show();

  await fireEvent.keyDown(tree, { key: "ArrowRight" });
  await fireEvent.click(row("b.txt"));
  await fireEvent.click(row("Older"), { metaKey: true });
  await fireEvent.keyDown(tree, { key: "Backspace" });

  expect(props.ondelete).toHaveBeenCalledTimes(1);
  const selection = props.ondelete.mock.calls[0][0];
  expect(selection.exact.map((entry: ShelfEntry) => entry.list.name)).toEqual(["Older"]);
  expect(selection.files).toEqual([{ name: "Fix_it", files: [FIX.changes[1].file] }]);
});

test("the_toolbar_and_the_menu_follow_shelved_changes_toolbar_and_popup_menu", async () => {
  const { tree, props } = show();

  const toolbar = screen.getByRole("toolbar", { name: "Shelf" });
  expect(within(toolbar).getAllByRole("button").map((button) => button.getAttribute("aria-label"))).toEqual([
    "Unshelve Silently",
    "Show Diff",
    "View Options",
    "Expand All",
    "Collapse All",
  ]);
  expect(within(toolbar).getByRole("button", { name: "Unshelve Silently" }).title).toBe("Unshelve Silently (⌥⌘U)");

  await fireEvent.contextMenu(row("Fix it"));
  const menu = screen.getByRole("menu", { name: "Shelf" });
  const items = within(menu).getAllByRole("menuitem");
  expect(items.map((item) => item.getAttribute("aria-label"))).toEqual([
    "Unshelve...",
    "Restore",
    "Show Diff",
    "Show Diff in a New Tab or Window",
    "Compare with Local",
    "Create Patch...",
    "Copy as Patch to Clipboard",
    "Import Patches…",
    "Import from .idea",
    "Rename...",
    "Delete",
  ]);
  // Restore is for the deleted lists only.
  expect((within(menu).getByRole("menuitem", { name: "Restore" }) as HTMLButtonElement).disabled).toBe(true);
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Unshelve..." }));
  expect(props.onunshelve).toHaveBeenCalledWith(expect.objectContaining({ exact: [FIX] }), true);

  // ⌥⌘U is Unshelve Silently, ⇧⌘U Unshelve...
  await fireEvent.keyDown(tree, { key: "u", code: "KeyU", altKey: true, metaKey: true });
  expect(props.onunshelve).toHaveBeenLastCalledWith(expect.objectContaining({ exact: [FIX] }), false);
  await fireEvent.keyDown(tree, { key: "U", shiftKey: true, metaKey: true });
  expect(props.onunshelve).toHaveBeenLastCalledWith(expect.objectContaining({ exact: [FIX] }), true);
});

test("restore_brings_back_the_deleted_lists", async () => {
  const { tree, props } = show();

  await fireEvent.click(row("Recently Deleted"));
  await fireEvent.keyDown(tree, { key: "ArrowRight" });
  await fireEvent.contextMenu(row("Gone"));
  const menu = screen.getByRole("menu", { name: "Shelf" });
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Restore" }));
  expect(props.onrestore).toHaveBeenCalledWith(["Gone"]);
});

test("show_diff_of_a_file_is_its_base_against_the_shelved_version", async () => {
  const { tree, props } = show();

  await fireEvent.keyDown(tree, { key: "ArrowRight" });
  await fireEvent.dblClick(row("a.txt"));

  const dialog = await screen.findByRole("dialog", { name: "Shelved: a.txt" });
  expect(props.api.sides).toHaveBeenCalledWith("Fix_it", FIX.changes[0].file, false);
  await waitFor(() => expect(within(dialog).getByText("Shelved version")).toBeTruthy());
  // The chain is the whole list: the other file is next.
  await fireEvent.click(within(dialog).getByRole("button", { name: "Previous File" }));
  expect(await screen.findByRole("dialog", { name: "Shelved: b.txt" })).toBeTruthy();
  expect(views().length).toBeGreaterThan(0);
});

test("group_by_directory_puts_the_files_under_their_folders", async () => {
  const { props, rerender, tree } = show();

  await fireEvent.click(screen.getByRole("button", { name: "View Options" }));
  await fireEvent.click(screen.getByRole("menuitem", { name: "Group By" }));
  await fireEvent.click(screen.getByRole("menuitem", { name: "Directory" }));
  expect(props.onsettings).toHaveBeenCalledWith({ ...SETTINGS, groupByDirectory: true });

  await rerender({ ...props, settings: { ...SETTINGS, groupByDirectory: true } });
  await fireEvent.click(screen.getByRole("button", { name: "Expand All" }));
  expect(rows(tree).slice(0, 4)).toEqual([
    expect.stringMatching(/^Fix it/),
    "b.txt",
    "src",
    "a.txt",
  ]);
});

test("several_lists_ask_unshelve_changes_to_changelist", async () => {
  render(ShelfDialogs);
  const lists = {
    lists: [
      { id: "default", name: "Changes", comment: "" },
      { id: "l2", name: "Fix it", comment: "the fix" },
    ],
    active: "default",
    assignments: {},
    hunkAssignments: {},
  };
  const answer = shelfDialogsUi.chooseList({ lists, names: ["Fix it"], removeApplied: true });

  const dialog = await screen.findByRole("dialog", { name: "Unshelve Changes to Changelist" });
  const name = within(dialog).getByLabelText("Name:") as HTMLInputElement;
  expect(name.value).toBe("Fix it");
  expect((within(dialog).getByLabelText("Comment:") as HTMLTextAreaElement).value).toBe("the fix");
  const remove = within(dialog).getByRole("checkbox", { name: "Remove successfully applied files from the shelf" });
  expect((remove as HTMLInputElement).checked).toBe(true);
  await fireEvent.click(remove);
  await fireEvent.input(name, { target: { value: "" } });
  expect(within(dialog).getByText("Cannot create new changelist with empty name")).toBeTruthy();
  await fireEvent.input(name, { target: { value: "Brand new" } });
  expect(within(dialog).getByText("New")).toBeTruthy();
  await fireEvent.click(within(dialog).getByRole("button", { name: "OK" }));
  expect(await answer).toEqual({ list: { name: "Brand new", comment: "", setActive: false }, removeApplied: false });

  // Import Patches asks Yes or No.
  const confirmed = shelfDialogsUi.confirm("Import Patches", "Found 2 patch files.\nContinue with import?");
  const question = await screen.findByRole("dialog", { name: "Import Patches" });
  await fireEvent.click(within(question).getByRole("button", { name: "Yes" }));
  expect(await confirmed).toBe(true);
});

// `activateView(list)` for a list the tab has not read yet (Shelve Changes…, Import to Shelf,
// Import Patches…): it is selected once it arrives.
test("a_list_asked_for_before_it_arrives_is_selected_when_it_does", async () => {
  const { component, props, rerender } = show([OLDER]);

  component.select("Fix_it");
  expect(row("Older").getAttribute("aria-selected")).toBe("true");
  await rerender({ ...props, entries: [OLDER, FIX] });
  await waitFor(() => expect(row("Fix it").getAttribute("aria-selected")).toBe("true"));
  expect(row("Older").getAttribute("aria-selected")).toBe("false");
});

test("the_chooser_saves_remove_on_every_toggle", async () => {
  render(ShelfDialogs);
  const written: ShelfSettings[] = [];
  setShelfSettingsWriter(async (settings) => {
    written.push(settings);
  });
  loadedShelfSettings({ ...SETTINGS, groupByDirectory: true });
  const lists = {
    lists: [{ id: "default", name: "Changes", comment: "" }],
    active: "default",
    assignments: {},
    hunkAssignments: {},
  };
  const answer = shelfDialogsUi.chooseList({ lists, names: [], removeApplied: false });
  const dialog = await screen.findByRole("dialog", { name: "Unshelve Changes to Changelist" });
  const remove = within(dialog).getByRole("checkbox", { name: "Remove successfully applied files from the shelf" });
  await fireEvent.click(remove);
  await fireEvent.click(remove);
  await fireEvent.click(remove);
  // Kept even when the dialog is cancelled (`createRemoveFilesStrategyCheckbox`), and the other
  // options stay as they are.
  await fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
  expect(await answer).toBeNull();
  expect(written.map((settings) => settings.removeStrategy)).toEqual([true, false, true]);
  expect(written.every((settings) => settings.groupByDirectory)).toBe(true);
  setShelfSettingsWriter(null);
});

// One writer: a toggle of the tab after the dialog's keeps what the dialog saved.
test("the_shelf_settings_have_one_writer", async () => {
  const written: ShelfSettings[] = [];
  setShelfSettingsWriter(async (settings) => {
    written.push(settings);
  });
  loadedShelfSettings(SETTINGS);
  await saveShelfSetting({ removeStrategy: true });
  await saveShelfSetting({ showRecycled: true });
  expect(written).toEqual([
    { ...SETTINGS, removeStrategy: true },
    { ...SETTINGS, removeStrategy: true, showRecycled: true },
  ]);
  expect(get(shelfSettings)).toEqual({ ...SETTINGS, removeStrategy: true, showRecycled: true });
  setShelfSettingsWriter(null);
});

// Another repository: the store starts again. A save before its settings are read reads them first,
// and does not write when they cannot be read; a read started before a save does not undo it.
test("the_shelf_settings_follow_the_open_repository", async () => {
  const written: ShelfSettings[] = [];
  setShelfSettingsWriter(async (settings) => {
    written.push(settings);
  });
  loadedShelfSettings({ ...SETTINGS, groupByDirectory: true });

  resetShelfSettings();
  expect(get(shelfSettings)).toEqual(SETTINGS);
  setShelfSettingsReader(async () => ({ ...SETTINGS, showRecycled: true }));
  await saveShelfSetting({ removeStrategy: true });
  expect(written).toEqual([{ ...SETTINGS, showRecycled: true, removeStrategy: true }]);

  resetShelfSettings();
  setShelfSettingsReader(async () => {
    throw new Error("no settings");
  });
  await expect(saveShelfSetting({ groupByDirectory: true })).rejects.toThrow("no settings");
  expect(written).toHaveLength(1);

  // A read that started before the save comes back with the old values: it is dropped.
  const started = shelfSettingsGeneration();
  setShelfSettingsReader(async () => SETTINGS);
  await saveShelfSetting({ showRecycled: true });
  expect(loadedShelfSettings(SETTINGS, started)).toBe(false);
  expect(get(shelfSettings).showRecycled).toBe(true);
  expect(loadedShelfSettings(SETTINGS, shelfSettingsGeneration())).toBe(true);

  setShelfSettingsWriter(null);
  setShelfSettingsReader(null);
});
