import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import type { ApplyPlan, Changelists, PlannedFile } from "../lib/api";
import ApplyPatchDialog from "./ApplyPatchDialog.svelte";
import { legendText } from "./applyModel";
import type { ApplyDialogRequest, ApplyServices } from "./flow";
import OverwriteDialog from "./OverwriteDialog.svelte";
import PartlyFailedDialog from "./PartlyFailedDialog.svelte";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));

// jsdom does no layout: CodeMirror measures with this.
Range.prototype.getClientRects = () => [] as unknown as DOMRectList;

afterEach(cleanup);

function planned(index: number, path: string, kind: PlannedFile["kind"], extra: Partial<PlannedFile> = {}): PlannedFile {
  return {
    index,
    path,
    before: kind === "added" ? null : path,
    after: kind === "deleted" ? null : path,
    original: path,
    strip: 0,
    maxStrip: path.split("/").length - 1,
    kind,
    binary: false,
    base: false,
    status: "applicable",
    error: null,
    hashes: {},
    ...extra,
  };
}

const FILES = [
  planned(0, "new.txt", "added"),
  planned(1, "src/a.txt", "modified"),
  planned(2, "old.txt", "deleted"),
  planned(3, "gone.txt", "modified", { status: "missing" }),
];

const plan = (files: PlannedFile[] = FILES, extra: Partial<ApplyPlan> = {}): ApplyPlan => ({
  subject: null,
  message: null,
  author: null,
  additionalInfoError: null,
  readError: null,
  files,
  ...extra,
});

const LISTS: Changelists = {
  lists: [
    { id: "default", name: "Changes", comment: "" },
    { id: "l2", name: "Feature", comment: "old comment" },
  ],
  active: "default",
  assignments: {},
  hunkAssignments: {},
};

function services(): ApplyServices & { [K in keyof ApplyServices]: ReturnType<typeof vi.fn> } {
  return {
    load: vi.fn(async () => plan()),
    preview: vi.fn(async () => ({ before: "one\n", after: "uno\n", titles: ["Base version", "Patched version"] })),
    chooseFile: vi.fn(async () => null),
    chooseFolder: vi.fn(async () => null),
    setOnTheFly: vi.fn(async () => {}),
    setRemoveApplied: vi.fn(async () => {}),
  } as never;
}

function open(request: Partial<ApplyDialogRequest> = {}) {
  const onanswer = vi.fn();
  const given = services();
  render(ApplyPatchDialog, {
    mode: "apply",
    repo: "/repo",
    source: { kind: "file", path: "/patches/fix_it.patch" },
    plan: plan(),
    lists: LISTS,
    onTheFly: false,
    services: given,
    onanswer,
    ...request,
  });
  return { onanswer, services: given };
}

const dialog = (name = "Apply Patch") => screen.getByRole("dialog", { name });
const tree = () => within(dialog()).getByRole("tree", { name: "Changes" });
const row = (name: string) => within(tree()).getByRole("treeitem", { name: new RegExp(`^${name}`) });
const check = (name: string) => within(row(name)).queryByRole("checkbox");
const legend = () => dialog().querySelector(".legend")?.textContent;
const ok = () => within(dialog()).getByRole("button", { name: "OK" }) as HTMLButtonElement;
const nameField = () => within(dialog()).getByRole("combobox", { name: "Name:" }) as HTMLInputElement;
const commentField = () => within(dialog()).getByRole("textbox", { name: "Comment:" }) as HTMLTextAreaElement;

// `ApplyPatchDifferentiatedDialog.createCenterPanel` and `MyChangeNodeDecorator`: the file field,
// the toolbar, the tree by folder with each file's status, a file whose target is missing with
// `Select missing base` and no checkbox, and the legend of the included files.
test("apply_patch_asks_select_patch_file_then_shows_the_tree_with_added_modified_deleted_and_missing_base", async () => {
  open();

  expect(within(dialog()).getByRole("heading", { name: "Apply Patch" })).toBeTruthy();
  expect((within(dialog()).getByRole("textbox", { name: "Patch file name:" }) as HTMLInputElement).value).toBe(
    "/patches/fix_it.patch",
  );
  for (const name of ["Show Difference", "Change Directory Paths", "Refresh"]) {
    expect(within(dialog()).getByRole("button", { name })).toBeTruthy();
  }
  expect(within(tree()).getAllByRole("treeitem").map((item) => item.getAttribute("aria-label"))).toEqual([
    "src",
    "a.txt   Modified",
    "gone.txt   Modified  Select missing base",
    "new.txt   Added",
    "old.txt   Deleted",
  ]);
  expect(check("new.txt")?.getAttribute("aria-checked")).toBe("true");
  expect(check("a.txt")?.getAttribute("aria-checked")).toBe("true");
  expect(check("gone.txt")).toBeNull();
  expect(within(row("gone.txt")).getByRole("button", { name: "Select missing base" })).toBeTruthy();
  expect(legend()).toBe("1 added   1 modified   1 deleted   1 missing base");

  await fireEvent.click(check("old.txt")!);
  expect(legend()).toBe("1 added   1 modified   1 missing base");
  expect(ok().disabled).toBe(false);
  expect(legendText([], new Set())).toBe("");
});

// `reloadPatchFromFile` (`:404-432`) with `ChangeListChooserPanel`: a message from the patch is the
// comment of the active list; without one, a list named like the file (`_` as a space) is chosen,
// with its own comment; `CREATE_CHANGELISTS_AUTOMATICALLY` is off, so no new name is suggested.
test("the_changelist_name_and_comment_come_from_the_patch_message_or_its_file_name", async () => {
  const first = open({ plan: plan(FILES, { subject: "Fix it", message: "Fix it\n\nThe body" }) });
  expect(nameField().value).toBe("Changes");
  expect(commentField().value).toBe("Fix it\n\nThe body");
  await fireEvent.click(ok());
  expect(first.onanswer.mock.calls[0][0].list).toEqual({
    name: "Changes",
    comment: "Fix it\n\nThe body",
    setActive: false,
  });
  cleanup();

  open({ source: { kind: "file", path: "/patches/Feature.patch" } });
  expect(nameField().value).toBe("Feature");
  expect(commentField().value).toBe("old comment");
  cleanup();

  const typed = open({ source: { kind: "file", path: "/patches/my_fix.patch" } });
  expect(nameField().value).toBe("Changes");
  // A new name is tagged `New`; an empty one is an error and OK goes off.
  await fireEvent.input(nameField(), { target: { value: "Brand new" } });
  expect(within(dialog()).getByText("New")).toBeTruthy();
  await fireEvent.input(nameField(), { target: { value: "  " } });
  expect(within(dialog()).getByText("Cannot create new changelist with empty name")).toBeTruthy();
  expect(ok().disabled).toBe(true);
  await fireEvent.input(nameField(), { target: { value: "Brand new" } });
  await fireEvent.click(within(dialog()).getByRole("checkbox", { name: "Set active" }));
  await fireEvent.click(ok());
  expect(typed.onanswer.mock.calls[0][0].list).toEqual({ name: "Brand new", comment: "", setActive: true });
  cleanup();

  // Without changelists (the staging area) there is no chooser.
  const staging = open({ lists: null });
  expect(within(dialog()).queryByRole("combobox", { name: "Name:" })).toBeNull();
  await fireEvent.click(ok());
  expect(staging.onanswer.mock.calls[0][0]).toMatchObject({ list: null, selected: [0, 1, 2] });
});

// `Change Directory Paths`: the strip actions on the selected files, and `Map Base Directory…`, which
// takes a folder of the repository. Each change plans the patch again with the new mapping.
test("strip_and_map_change_the_targets", async () => {
  const { services: given, onanswer } = open();
  const stripped = plan([FILES[0], planned(1, "a.txt", "modified", { original: "src/a.txt", strip: 1, maxStrip: 1 })]);
  given.load.mockResolvedValue(stripped);

  await fireEvent.click(row("a.txt"));
  await fireEvent.click(within(dialog()).getByRole("button", { name: "Change Directory Paths" }));
  const menu = screen.getByRole("menu");
  expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent?.trim())).toEqual([
    "Map Base Directory…",
    "Remove Leading Directory",
    "Remove All Leading Directories",
    "Restore Leading Directory",
    "Restore All Leading Directories",
  ]);
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Remove Leading Directory" }));

  await waitFor(() =>
    expect(given.load.mock.calls.at(-1)).toEqual([
      { kind: "file", path: "/patches/fix_it.patch" },
      { files: [{ strip: 0, base: "" }, { strip: 1, base: "" }] },
    ]),
  );
  await waitFor(() => expect(row("a.txt").getAttribute("aria-label")).toBe("a.txt stripped 1   Modified  New base detected"));

  given.chooseFolder.mockResolvedValue("/repo/lib");
  await fireEvent.click(row("a.txt"));
  await fireEvent.click(within(dialog()).getByRole("button", { name: "Change Directory Paths" }));
  await fireEvent.click(within(screen.getByRole("menu")).getByRole("menuitem", { name: "Map Base Directory…" }));
  await waitFor(() =>
    expect(given.load.mock.calls.at(-1)?.[1]).toEqual({ files: [{ strip: 0, base: "" }, { strip: 1, base: "lib" }] }),
  );
  expect(given.chooseFolder.mock.calls[0][0]).toEqual({ title: "Select Directory Base", defaultPath: "/repo" });

  // OK waits for the plan being read.
  await waitFor(() => expect(ok().disabled).toBe(false));
  await fireEvent.click(ok());
  expect(onanswer.mock.calls[0][0].mapping).toEqual({ files: [{ strip: 0, base: "" }, { strip: 1, base: "lib" }] });
});

// The patch file can be changed: typing reloads after 500 ms (`MergingUpdateQueue`), and so does
// `Refresh`. A patch that cannot be read says so above the tree.
test("editing_the_patch_file_name_or_refresh_reads_the_patch_again", async () => {
  vi.useFakeTimers();
  try {
    const { services: given } = open({ plan: plan([], { readError: "Unexpected line 3" }) });
    expect(within(dialog()).getByRole("alert").textContent).toBe("Cannot read patch fix_it.patch: Unexpected line 3");

    await fireEvent.input(within(dialog()).getByRole("textbox", { name: "Patch file name:" }), {
      target: { value: "/patches/other.patch" },
    });
    await vi.advanceTimersByTimeAsync(400);
    expect(given.load).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(200);
    expect(given.load.mock.calls).toEqual([[{ kind: "file", path: "/patches/other.patch" }, { files: [] }]]);

    await fireEvent.click(within(dialog()).getByRole("button", { name: "Refresh" }));
    expect(given.load).toHaveBeenCalledTimes(2);
  } finally {
    vi.useRealTimers();
  }
});

// `APPLY_PATCH_IN_MEMORY`: no file field and no Refresh; the on-the-fly checkbox on the left.
test("from_clipboard_the_dialog_has_the_on_the_fly_checkbox_and_no_file", async () => {
  const { services: given } = open({ mode: "clipboard", source: { kind: "text", text: "" }, plan: plan([]), onTheFly: false });

  expect(within(dialog()).queryByRole("textbox", { name: "Patch file name:" })).toBeNull();
  expect(within(dialog()).queryByRole("button", { name: "Refresh" })).toBeNull();
  expect(within(tree()).getByText("No changes detected")).toBeTruthy();
  expect(ok().disabled).toBe(true);
  const box = within(dialog()).getByRole("checkbox", { name: "Analyze and apply patch from clipboard on the fly" });
  expect((box as HTMLInputElement).checked).toBe(false);
  await fireEvent.click(box);
  expect(given.setOnTheFly.mock.calls).toEqual([[true]]);
});

// `Show Difference` (⌘D): the selected file as it is against the patched text; a missing target says
// it has no base.
test("show_difference_shows_the_file_against_the_patched_text", async () => {
  const { services: given } = open();

  await fireEvent.click(row("a.txt"));
  await fireEvent.keyDown(tree(), { key: "d", metaKey: true });

  const diff = await screen.findByRole("dialog", { name: "/repo/src/a.txt" });
  await waitFor(() => expect(diff.querySelectorAll(".cm-editor")).toHaveLength(2));
  expect(given.preview.mock.calls).toEqual([[{ kind: "file", path: "/patches/fix_it.patch" }, { files: [] }, 1]]);
  expect([...diff.querySelectorAll(".title")].map((title) => title.textContent)).toEqual([
    "Base version",
    "Patched version",
  ]);

  // The chain is every file by path: from src/a.txt, the first is gone.txt, which has no target.
  for (let step = 0; step < 3; step++) {
    await fireEvent.click(within(diff).getByRole("button", { name: "Previous File" }));
  }
  expect(await within(diff).findByText("Cannot find base for 'gone.txt'")).toBeTruthy();
  expect(given.preview).toHaveBeenCalledTimes(3);
});

// `PathsVerifier.DelayedPrecheckContext.doDelayed` with `selectFilePathsToProcess`: one file is a
// question with its path; several are a list to choose from.
test("overwrite_existing_files_for_one_file_and_for_several", async () => {
  const one = vi.fn();
  render(OverwriteDialog, { repo: "/repo", paths: ["a.txt"], onanswer: one });
  const single = screen.getByRole("dialog", { name: "Overwrite Existing Files" });
  expect(single.querySelector("p")?.textContent).toBe(
    "The following file should be created by patch, but it already exists.\nDo you want to overwrite it?\n/repo/a.txt",
  );
  await fireEvent.click(within(single).getByRole("button", { name: "Overwrite" }));
  expect(one.mock.calls).toEqual([[["a.txt"]]]);
  cleanup();

  const several = vi.fn();
  render(OverwriteDialog, { repo: "/repo", paths: ["a.txt", "b.txt"], onanswer: several });
  const list = screen.getByRole("dialog", { name: "Overwrite Existing Files" });
  expect(list.querySelector("p")?.textContent).toBe(
    "The following files should be created by patch, but they already exist.\nDo you want to overwrite them?",
  );
  await fireEvent.click(within(list).getByRole("checkbox", { name: "a.txt" }));
  await fireEvent.click(within(list).getByRole("button", { name: "Overwrite" }));
  expect(several.mock.calls).toEqual([[["b.txt"]]]);
  await fireEvent.click(within(list).getByRole("button", { name: "Cancel" }));
  expect(several.mock.calls[1]).toEqual([[]]);
});

// `UndoApplyPatchDialog`.
test("patch_applying_partly_failed_lists_the_files_and_offers_rollback", async () => {
  const onanswer = vi.fn();
  render(PartlyFailedDialog, { failed: ["src/a.txt", "b.txt"], binaries: true, onanswer });
  const shown = screen.getByRole("dialog", { name: "Patch Applying Partly Failed" });

  expect([...shown.querySelectorAll("p")].map((line) => line.textContent)).toEqual([
    "Failed to apply patch to files:",
    "Other changes from the patch have been applied successfully, would you like to rollback them?",
    "Rollback will not affect binaries",
  ]);
  expect(within(shown).getAllByRole("listitem").map((item) => item.textContent)).toEqual(["src/a.txt", "b.txt"]);
  await fireEvent.click(within(shown).getByRole("button", { name: "Rollback" }));
  expect(onanswer.mock.calls).toEqual([[true]]);
  cleanup();

  render(PartlyFailedDialog, { failed: ["a.txt"], binaries: false, onanswer });
  expect(screen.getByText("Failed to apply patch to file:")).toBeTruthy();
  expect(screen.queryByText("Rollback will not affect binaries")).toBeNull();
});

// Space, Enter and the arrows typed in a field belong to it: the tree's keys act only in the tree.
test("typed_keys_stay_in_their_fields", async () => {
  const { services: given } = open();
  await fireEvent.click(row("a.txt"));
  const fields = [
    within(dialog()).getByRole("textbox", { name: "Patch file name:" }),
    nameField(),
    commentField(),
  ];

  for (const field of fields) {
    for (const key of [" ", "Enter", "ArrowDown", "ArrowUp"]) {
      expect(await fireEvent.keyDown(field, { key }), `${key} in ${field.id}`).toBe(true);
    }
  }
  expect(check("a.txt")?.getAttribute("aria-checked")).toBe("true");
  expect(row("a.txt").getAttribute("aria-selected")).toBe("true");
  expect(screen.queryByRole("dialog", { name: "/repo/src/a.txt" })).toBeNull();
  expect(given.preview).not.toHaveBeenCalled();

  // In the tree they still move, toggle and show the difference.
  expect(await fireEvent.keyDown(tree(), { key: " " })).toBe(false);
  expect(check("a.txt")?.getAttribute("aria-checked")).toBe("false");
  await fireEvent.keyDown(tree(), { key: "ArrowDown" });
  expect(row("gone.txt").getAttribute("aria-selected")).toBe("true");
});

// `Patch file name:` reloads 500 ms after typing: until the new patch is read, OK would apply the old
// one, so it waits.
test("ok_waits_for_a_typed_patch_file_name", async () => {
  vi.useFakeTimers();
  try {
    const { services: given, onanswer } = open();
    expect(ok().disabled).toBe(false);

    await fireEvent.input(within(dialog()).getByRole("textbox", { name: "Patch file name:" }), {
      target: { value: "/patches/other.patch" },
    });
    expect(ok().disabled).toBe(true);
    await fireEvent.submit(ok().closest("form")!);
    expect(onanswer).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(600);
    expect(given.load).toHaveBeenCalledTimes(1);
    expect(ok().disabled).toBe(false);
  } finally {
    vi.useRealTimers();
  }
});

test("the_unshelve_dialog_has_its_title_ok_text_and_remove_checkbox", async () => {
  const A = { before: "src/a.txt", after: "src/a.txt", binary: false };
  const N = { before: null, after: "new.txt", binary: false };
  const shelved = plan([
    planned(0, "new.txt", "added", { shelf: N }),
    planned(1, "src/a.txt", "modified", { shelf: A }),
    planned(2, "x.bin", "added", { binary: true, shelf: { before: null, after: "x.bin", binary: true } }),
  ]);
  const { onanswer, services: given } = open({
    mode: "unshelve",
    source: { kind: "shelf", name: "Fix_it", files: null },
    plan: shelved,
    removeApplied: false,
    preselected: [A],
    listNames: ["Fix it", "Feature"],
  });

  const dialog = screen.getByRole("dialog", { name: "Unshelve Changes" });
  // No patch file and no Refresh, no Import to Shelf.
  expect(within(dialog).queryByLabelText("Patch file name:")).toBeNull();
  expect(within(dialog).queryByRole("button", { name: "Refresh" })).toBeNull();
  expect(within(dialog).queryByRole("button", { name: "Import to Shelf" })).toBeNull();
  // Only the preselected file is included, and the list named like the shelf is chosen.
  const checked = (name: string) => within(dialog).getByRole("checkbox", { name }).getAttribute("aria-checked");
  expect([checked("new.txt"), checked("a.txt"), checked("x.bin")]).toEqual(["false", "true", "false"]);
  expect((within(dialog).getByLabelText("Name:") as HTMLInputElement).value).toBe("Feature");

  const remove = within(dialog).getByRole("checkbox", { name: "Remove successfully applied files from the shelf" });
  expect((remove as HTMLInputElement).checked).toBe(false);
  await fireEvent.click(remove);
  expect(given.setRemoveApplied).toHaveBeenCalledWith(true);

  await fireEvent.click(within(dialog).getByRole("button", { name: "Unshelve Changes" }));
  expect(onanswer).toHaveBeenCalledWith(
    expect.objectContaining({ selected: [1], removeApplied: true, list: { name: "Feature", comment: "old comment", setActive: false } }),
  );
});

// Finding 6: a binary file that HEAD changed since it was shelved is not applicable, with its
// reason and Ramify's own `Overwrite`; chosen, the row is included and the answer says to overwrite it.
test("a_binary_changed_in_head_offers_overwrite_in_the_unshelve_dialog", async () => {
  const why = '"x.bin" changed in HEAD since it was shelved: it was not unshelved';
  const X = { before: "x.bin", after: "x.bin", binary: true };
  const shelved = plan([
    planned(0, "a.txt", "modified", { shelf: { before: "a.txt", after: "a.txt", binary: false } }),
    planned(1, "x.bin", "modified", { binary: true, status: "invalid", error: why, overwritable: true, shelf: X }),
    planned(2, "y.bin", "modified", { binary: true, status: "invalid", error: "other", shelf: X }),
  ]);
  const { onanswer } = open({
    mode: "unshelve",
    source: { kind: "shelf", name: "Fix_it", files: null },
    plan: shelved,
    lists: null,
  });
  const dialog = screen.getByRole("dialog", { name: "Unshelve Changes" });
  const row = (name: string) =>
    within(within(dialog).getByRole("tree", { name: "Changes" })).getByRole("treeitem", { name: new RegExp(`^${name}`) });
  expect(within(row("x.bin")).queryByRole("checkbox")).toBeNull();
  expect(within(row("x.bin")).getByText(why)).toBeTruthy();
  expect(within(row("y.bin")).queryByRole("button", { name: "Overwrite" })).toBeNull();

  await fireEvent.click(within(row("x.bin")).getByRole("button", { name: "Overwrite" }));

  expect(within(dialog).getByRole("checkbox", { name: "x.bin" }).getAttribute("aria-checked")).toBe("true");
  expect(within(row("x.bin")).queryByText(why)).toBeNull();
  await fireEvent.click(within(dialog).getByRole("button", { name: "Unshelve Changes" }));
  expect(onanswer).toHaveBeenCalledWith(expect.objectContaining({ selected: [0, 1], overwrite: ["x.bin"] }));
});

// Part 3's N1: a new plan (a strip or a base of another file) keeps the Overwrite of a binary, and
// with it its row stays ticked.
test("a_replanned_binary_with_overwrite_stays_ticked", async () => {
  const why = '"x.bin" changed in HEAD since it was shelved: it was not unshelved';
  const X = { before: "x.bin", after: "x.bin", binary: true };
  const files = (aPath: string, strip: number) => [
    planned(0, aPath, "modified", { original: "src/a.txt", strip, maxStrip: 1, shelf: { before: "src/a.txt", after: "src/a.txt", binary: false } }),
    planned(1, "x.bin", "modified", { binary: true, status: "invalid", error: why, overwritable: true, shelf: X }),
  ];
  const { services: given, onanswer } = open({
    mode: "unshelve",
    source: { kind: "shelf", name: "Fix_it", files: null },
    plan: plan(files("src/a.txt", 0)),
    lists: null,
  });
  const dialog = screen.getByRole("dialog", { name: "Unshelve Changes" });
  const changes = () => within(dialog).getByRole("tree", { name: "Changes" });
  const row = (name: string) => within(changes()).getByRole("treeitem", { name: new RegExp(`^${name}`) });
  await fireEvent.click(within(row("x.bin")).getByRole("button", { name: "Overwrite" }));
  expect(within(dialog).getByRole("checkbox", { name: "x.bin" }).getAttribute("aria-checked")).toBe("true");
  given.load.mockResolvedValue(plan(files("a.txt", 1)));

  await fireEvent.click(row("a.txt"));
  await fireEvent.click(within(dialog).getByRole("button", { name: "Change Directory Paths" }));
  await fireEvent.click(within(screen.getByRole("menu")).getByRole("menuitem", { name: "Remove Leading Directory" }));
  await waitFor(() => expect(given.load).toHaveBeenCalled());
  await waitFor(() => expect(row("a.txt").getAttribute("aria-label")).toMatch(/^a\.txt stripped 1/));

  expect(within(dialog).getByRole("checkbox", { name: "x.bin" }).getAttribute("aria-checked")).toBe("true");
  await fireEvent.click(within(dialog).getByRole("button", { name: "Unshelve Changes" }));
  expect(onanswer).toHaveBeenCalledWith(expect.objectContaining({ selected: [0, 1], overwrite: ["x.bin"] }));
});

test("import_to_shelf_is_a_button_of_the_apply_mode", async () => {
  const { onanswer } = open();
  const dialog = screen.getByRole("dialog", { name: "Apply Patch" });
  await fireEvent.click(within(dialog).getByRole("button", { name: "Import to Shelf" }));
  expect(onanswer).toHaveBeenCalledWith(expect.objectContaining({ selected: [0, 1, 2], importToShelf: true }));
  cleanup();

  open({ mode: "clipboard", source: { kind: "text", text: "" } });
  expect(screen.queryByRole("button", { name: "Import to Shelf" })).toBeNull();
});
