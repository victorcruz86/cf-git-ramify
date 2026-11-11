import { EditorView } from "@codemirror/view";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { get } from "svelte/store";
import type { Changelists, FileChange, FileState, RepoInfo, WorktreeSides } from "../lib/api";
import { dismiss, notifications } from "../lib/notifications";
import { trackTasks } from "../lib/tasks";
import { checkinFilesText } from "./checkin";
import LocalChangesView from "./LocalChangesView.svelte";

// What `EditorView.lineWrapping` puts on the content; `view.lineWrapping` only follows it after a
// measure, which jsdom does not do.
const wrapped = (view: EditorView) => view.contentDOM.classList.contains("cm-lineWrapping");

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({ label: "main" }),
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const REPO: RepoInfo = {
  path: "/repo",
  head: "abc",
  branch: "main",
  recovery: null,
  pending: [],
  state: { operation: null, branch: "main", label: "main", head: "abcdef1234567890" },
};

const change = (path: string, state: FileState = "modified") =>
  ({ path, oldPath: null, state, staged: false, unstaged: true }) satisfies FileChange;

const LISTS: Changelists = {
  lists: [
    { id: "default", name: "Changes", comment: "" },
    { id: "other", name: "Other", comment: "" },
  ],
  active: "default",
  assignments: { "a.txt": "default", "b.txt": "default", "c.txt": "other" },
  hunkAssignments: {},
};

const SIDES: WorktreeSides = {
  old: { kind: "text", content: "a\nb\n" },
  new: { kind: "text", content: "a\nB\n" },
  hunks: [{ id: "h1", oldStart: 1, oldLines: 1, newStart: 1, newLines: 1 }],
  editable: true,
  note: null,
  hash: "hash",
  conflicted: false,
  submodule: null,
};

// jsdom has no ResizeObserver, which `bind:clientHeight` needs, and does no layout for CodeMirror.
vi.stubGlobal(
  "ResizeObserver",
  class {
    observe() {}
    unobserve() {}
    disconnect() {}
  },
);
Range.prototype.getClientRects = () => [] as unknown as DOMRectList;

function mockCommands(commands: Record<string, (args: never) => unknown> = {}) {
  const all: Record<string, (args: never) => unknown> = {
    repo_status: () => [change("a.txt"), change("b.txt"), change("c.txt")],
    changelists_get: () => LISTS,
    worktree_sides: () => SIDES,
    ...commands,
  };
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    const handler = all[command];
    if (!handler) throw new Error(`unexpected command: ${command}`);
    return handler(args as never);
  });
}

function calls(command: string) {
  return vi.mocked(invoke).mock.calls.filter(([name]) => name === command);
}

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(listen).mockReset();
  vi.mocked(listen).mockImplementation(async () => () => {});
  localStorage.clear();
  get(notifications).forEach((shown) => dismiss(shown.id));
});

afterEach(cleanup);

async function open(props: Record<string, unknown> = {}) {
  const view = render(LocalChangesView, { repo: REPO, active: true, oncheckin: vi.fn(), ...props });
  await screen.findByRole("treeitem", { name: /^Changes/ });
  return view;
}

const row = (name: string | RegExp) => screen.getByRole("treeitem", { name });

async function contextMenu(name: string | RegExp) {
  await fireEvent.contextMenu(row(name));
  return screen.findByRole("menu");
}

const labels = (menu: HTMLElement) =>
  within(menu)
    .getAllByRole("menuitem")
    .map((item) => item.getAttribute("aria-label"));

test("Local Changes shows the changelists without checkboxes", async () => {
  mockCommands();
  await open();

  expect(row(/^Changes/).textContent).toContain("2 files");
  expect(row(/^Other/).textContent).toContain("1 file");
  expect(row("a.txt")).toBeTruthy();
  expect(within(screen.getByRole("tree")).queryAllByRole("checkbox")).toEqual([]);
  expect(screen.queryByRole("textbox", { name: "Commit Message" })).toBeNull();
});

test("the toolbar follows the order of ChangesViewToolbar", async () => {
  mockCommands();
  await open();

  const toolbar = screen.getByRole("toolbar", { name: "Changes" });

  expect(toolbar.getAttribute("aria-orientation")).toBe("vertical");
  expect(
    within(toolbar)
      .getAllByRole("button")
      .map((button) => button.getAttribute("aria-label")),
  ).toEqual([
    "Refresh",
    "Commit…",
    "Rollback…",
    "Show Diff",
    "Changelists",
    "Shelve Silently",
    "View Options",
    "Expand All",
    "Collapse All",
    "Preview Diff",
  ]);
});

test("the popup follows the order of ChangesViewPopupMenu", async () => {
  mockCommands();
  await open();

  const listMenu = await contextMenu(/^Other/);
  expect(labels(listMenu)).toEqual([
    "Commit File…",
    "Rollback…",
    "Move to Another Changelist...",
    "Show Diff",
    "New Changelist...",
    "Delete Changelist",
    "Set Active Changelist",
    "Edit Changelist...",
    "Create Patch from Local Changes...",
    "Copy as Patch to Clipboard",
    "Shelve Changes...",
    "Import Changelists from .idea",
    "Restore Snapshot…",
    "Refresh",
  ]);
  await fireEvent.keyDown(listMenu, { key: "Escape" });

  await fireEvent.click(row("a.txt"));
  await fireEvent.click(row("b.txt"), { metaKey: true });
  const filesMenu = await contextMenu("b.txt");
  expect(labels(filesMenu)[0]).toBe("Commit Files…");

  // Ramify lists every file, as `--untracked-files=all`, and folders are helper nodes as in
  // `DirectoryChangesGroupingPolicy`: a directory comes first only from a directory root.
  expect(checkinFilesText([{ path: "src", directory: true }])).toBe("Commit Directory…");
  expect(
    checkinFilesText([
      { path: "src", directory: true },
      { path: "a.txt", directory: false },
    ]),
  ).toBe("Commit Directories…");
});

test("Preview Diff shows the selected file's diff without line checkboxes and is shown by default", async () => {
  mockCommands();
  await open();
  const toggle = screen.getByRole("button", { name: "Preview Diff" });
  expect(toggle.getAttribute("aria-pressed")).toBe("true");
  expect(calls("worktree_sides")).toHaveLength(0);

  await fireEvent.click(row("a.txt"));

  const preview = await screen.findByRole("region", { name: "Diff Preview" });
  await waitFor(() => expect(preview.textContent).toContain("B"));
  expect(calls("worktree_sides")[0][1]).toEqual({ path: "a.txt" });
  expect(preview.querySelectorAll(".rm-check")).toHaveLength(0);
  expect(within(preview).queryByRole("checkbox")).toBeNull();
  const editors = [...preview.querySelectorAll(".cm-content")];
  expect(editors.length).toBeGreaterThan(0);
  expect(editors.every((editor) => editor.getAttribute("aria-readonly") === "true")).toBe(true);

  await fireEvent.click(toggle);

  expect(toggle.getAttribute("aria-pressed")).toBe("false");
  expect(screen.queryByRole("region", { name: "Diff Preview" })).toBeNull();
  const reads = calls("worktree_sides").length;
  await fireEvent.click(row("b.txt"));
  expect(calls("worktree_sides")).toHaveLength(reads);
});

// `SHOW_DIFF_ON_DOUBLE_CLICK` is false by default: a double click and Enter open the source of the
// selection in the viewer.
test("double click opens the source in the viewer", async () => {
  mockCommands({ viewer_open: () => undefined });
  const onshowdiff = vi.fn();
  await open({ onshowdiff });

  await fireEvent.click(row("a.txt"));
  await fireEvent.dblClick(row("a.txt"));

  expect(calls("viewer_open").map(([, args]) => args)).toEqual([
    { targets: [{ path: "a.txt", revision: null, line: null }] },
  ]);
  await fireEvent.click(row("b.txt"), { metaKey: true });
  await fireEvent.keyDown(screen.getByRole("tree"), { key: "Enter" });
  expect(calls("viewer_open").at(-1)?.[1]).toEqual({
    targets: [
      { path: "a.txt", revision: null, line: null },
      { path: "b.txt", revision: null, line: null },
    ],
  });
  expect(onshowdiff).not.toHaveBeenCalled();
});

// `Show on Double-Click` ▸ `Diff` on: the diff instead.
test("double click opens the diff with Show on Double-Click set to Diff", async () => {
  localStorage.setItem("VcsApplicationSettings.SHOW_DIFF_ON_DOUBLE_CLICK", "true");
  mockCommands();
  const onshowdiff = vi.fn();
  await open({ onshowdiff });

  await fireEvent.dblClick(row("a.txt"));

  expect(onshowdiff).toHaveBeenCalledTimes(1);
  expect(onshowdiff.mock.calls[0][0].path).toBe("a.txt");
  expect(calls("viewer_open")).toEqual([]);
});

test("Commit… with nothing selected asks for the whole list and Commit File… for the selected files", async () => {
  mockCommands();
  const oncheckin = vi.fn();
  await open({ oncheckin });

  await fireEvent.click(screen.getByRole("button", { name: "Commit…" }));
  expect(oncheckin).toHaveBeenLastCalledWith({ kind: "project", paths: [], lists: [] });

  await fireEvent.click(row(/^Other/));
  await fireEvent.keyDown(window, { key: "k", code: "KeyK", metaKey: true });
  expect(oncheckin).toHaveBeenLastCalledWith({ kind: "project", paths: ["c.txt"], lists: ["other"] });

  await fireEvent.click(row("b.txt"));
  const menu = await contextMenu("b.txt");
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Commit File…" }));
  expect(oncheckin).toHaveBeenLastCalledWith({ kind: "files", paths: ["b.txt"], lists: [] });
  expect(oncheckin).toHaveBeenCalledTimes(3);
});

// A task whose end arrives through `task-changed`.
let deliverTask: (id: number, failure?: string) => void = () => {};
let nextTask = 700;

function task(failure?: string) {
  const id = nextTask++;
  setTimeout(() => deliverTask(id, failure));
  return id;
}

async function listenToTasks() {
  vi.mocked(listen).mockImplementation(async (event, handler) => {
    if (event === "task-changed") {
      deliverTask = (id, failure) =>
        handler({
          event: "task-changed",
          id: 0,
          payload: {
            id,
            title: "Adding Files to VCS…",
            text: failure ?? "",
            fraction: null,
            state: failure ? "failed" : "done",
            kind: failure ? "failed" : null,
            value: null,
          },
        } as never);
    }
    return () => {};
  });
  await trackTasks();
}

const WITH_NEW = [change("a.txt"), change("b.txt"), change("c.txt"), change("new.txt", "untracked")];

// The status changes to `added` once `changes_add` ran.
function addingCommands(failure?: string) {
  let status = WITH_NEW;
  mockCommands({
    repo_status: () => status,
    changes_add: () => {
      if (!failure) status = [...WITH_NEW.slice(0, 3), change("new.txt", "added")];
      return task(failure);
    },
    changelists_move: () => LISTS,
  });
}

test("Add to VCS is offered only for unversioned files, after Show Diff and before the changelists", async () => {
  await listenToTasks();
  addingCommands();
  await open();

  const versioned = await contextMenu("a.txt");
  expect(labels(versioned)).not.toContain("Add to VCS");
  await fireEvent.keyDown(versioned, { key: "Escape" });

  await fireEvent.click(row(/^Unversioned Files/));
  const menu = await contextMenu(/^Unversioned Files/);
  expect(labels(menu)).toEqual([
    "Commit File…",
    "Rollback…",
    "Show Diff",
    "Add to VCS",
    "New Changelist...",
    "Create Patch from Local Changes...",
    "Copy as Patch to Clipboard",
    "Shelve Changes...",
    "Import Changelists from .idea",
    "Restore Snapshot…",
    "Refresh",
  ]);
  const item = within(menu).getByRole("menuitem", { name: "Add to VCS" });
  expect(item.textContent).toContain("⌥⌘A");
  // The group of `$Delete`, `ChangesView.AddUnversioned` and `ChangesView.RemoveDeleted`, between two
  // separators: Ramify has neither Delete nor Remove from VCS, so the group is only this item.
  expect(item.previousElementSibling?.getAttribute("role")).toBe("separator");
  expect(item.nextElementSibling?.getAttribute("role")).toBe("separator");
});

test("Add to VCS adds the selected unversioned files and they show in the active list", async () => {
  await listenToTasks();
  addingCommands();
  await open();
  expect(row(/^Changes/).textContent).toContain("2 files");

  await fireEvent.click(row(/^Unversioned Files/));
  const menu = await contextMenu(/^Unversioned Files/);
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Add to VCS" }));

  await waitFor(() => expect(row(/^Changes/).textContent).toContain("3 files"));
  expect(calls("changes_add").map(([, args]) => args)).toEqual([
    { repo: "/repo", paths: ["new.txt"] },
  ]);
  expect(calls("changelists_move").map(([, args]) => args)).toEqual([
    { paths: ["new.txt"], id: "default" },
  ]);
  expect(screen.queryByRole("treeitem", { name: /^Unversioned Files/ })).toBeNull();
});

test("⌥⌘A adds the unversioned files of the selection and does nothing without them", async () => {
  await listenToTasks();
  addingCommands();
  await open();
  const press = () => fireEvent.keyDown(window, { key: "å", code: "KeyA", metaKey: true, altKey: true });

  await fireEvent.click(row("a.txt"));
  await press();
  expect(calls("changes_add")).toHaveLength(0);

  await fireEvent.click(row(/^Unversioned Files/));
  await press();

  await waitFor(() => expect(row(/^Changes/).textContent).toContain("3 files"));
  expect(calls("changes_add").map(([, args]) => args)).toEqual([
    { repo: "/repo", paths: ["new.txt"] },
  ]);
});

test("a failed Add to VCS is notified as Failed to add files", async () => {
  await listenToTasks();
  addingCommands("fatal: pathspec did not match");
  await open();

  await fireEvent.click(row(/^Unversioned Files/));
  const menu = await contextMenu(/^Unversioned Files/);
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Add to VCS" }));

  await waitFor(() => expect(get(notifications)).toHaveLength(1));
  expect(get(notifications)[0]).toMatchObject({
    kind: "error",
    title: "Failed to add files",
    body: "The following problems have occurred when adding the files:\nfatal: pathspec did not match",
  });
});

test("right_click_on_the_gutter_offers_soft_wrap", async () => {
  mockCommands();
  await open();
  await fireEvent.click(row("a.txt"));
  const preview = await screen.findByRole("region", { name: "Diff Preview" });
  await waitFor(() => expect(preview.querySelectorAll(".cm-gutters")).toHaveLength(2));
  const views = () =>
    [...preview.querySelectorAll<HTMLElement>(".cm-editor")].map(
      (dom) => EditorView.findFromDOM(dom) as EditorView,
    );
  const before = views();
  // The place `ChangesView` starts off, as in IntelliJ.
  expect(before.map(wrapped)).toEqual([false, false]);

  await fireEvent.contextMenu(preview.querySelector(".cm-gutters .cm-gutterElement") as HTMLElement);
  const menu = await screen.findByRole("menu");
  expect(labels(menu)).toEqual(["Soft-Wrap"]);
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Soft-Wrap" }));

  expect(views()).toEqual(before);
  expect(before.map(wrapped)).toEqual([true, true]);
  expect(localStorage.getItem("diff.wrap.ChangesView")).toBe("true");
  expect(localStorage.getItem("diff.wrap.CommitDialog")).toBeNull();

  // The text keeps the menu it had.
  await fireEvent.contextMenu(preview.querySelector(".cm-line") as HTMLElement);
  expect(labels(await screen.findByRole("menu"))).toEqual([
    "Rollback Lines",
    "Move Lines to Another Changelist...",
    "Show Diff for Lines",
  ]);
});

// `ChangesView.CreatePatchToClipboard`: no dialog, the chosen files and the comment of the chosen
// list, and `Patch copied to clipboard`.
test("copy_as_patch_puts_the_selected_changes_on_the_clipboard_and_says_so", async () => {
  await listenToTasks();
  const commented: Changelists = {
    ...LISTS,
    lists: [{ id: "default", name: "Changes", comment: "Work in progress" }, LISTS.lists[1]],
  };
  mockCommands({
    repo_status: () => WITH_NEW,
    changelists_get: () => commented,
    patch_create: () => task(),
  });
  await open();

  const menu = await contextMenu(/^Changes/);
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Copy as Patch to Clipboard" }));

  await waitFor(() => expect(calls("patch_create")).toHaveLength(1));
  expect(calls("patch_create")[0][1]).toEqual({
    entries: [
      { path: "a.txt", oldPath: null, before: { kind: "head" }, after: { kind: "disk" } },
      { path: "b.txt", oldPath: null, before: { kind: "head" }, after: { kind: "disk" } },
    ],
    options: { basePath: "/repo", reverse: false, message: "Work in progress" },
    to: { kind: "clipboard" },
    remember: false,
    expectedRepo: "/repo",
  });
  await waitFor(() =>
    expect(get(notifications).map((shown) => shown.title)).toContain("Patch copied to clipboard"),
  );

  // An unversioned file goes in as a new one. Only unversioned files: no changes, so no Create Patch.
  await fireEvent.click(row(/^Unversioned Files/));
  const files = await contextMenu(/^Unversioned Files/);
  const create = within(files).getByRole("menuitem", { name: "Create Patch from Local Changes..." });
  expect((create as HTMLButtonElement).disabled).toBe(true);
  await fireEvent.click(within(files).getByRole("menuitem", { name: "Copy as Patch to Clipboard" }));
  await waitFor(() => expect(calls("patch_create")).toHaveLength(2));
  expect(calls("patch_create")[1][1]).toMatchObject({
    entries: [{ path: "new.txt", oldPath: null, before: null, after: { kind: "disk" } }],
    options: { message: null },
  });
});

// A file split across lists: from one list, only the hunks of that list.
test("copy_as_patch_of_a_partial_changelist_takes_only_its_hunks", async () => {
  await listenToTasks();
  const split: Changelists = { ...LISTS, hunkAssignments: { "a.txt": { h1: "default", h2: "other" } } };
  mockCommands({
    changelists_get: () => split,
    file_hunks: () => [
      { id: "h1", oldStart: 0, oldLines: 1, newStart: 0, newLines: 1 },
      { id: "h2", oldStart: 5, oldLines: 1, newStart: 5, newLines: 1 },
    ],
    patch_create: () => task(),
  });
  await open();

  const menu = await contextMenu(/^Other/);
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Copy as Patch to Clipboard" }));

  await waitFor(() => expect(calls("patch_create")).toHaveLength(1));
  const sent = calls("patch_create")[0][1] as { entries: unknown[] };
  expect(sent.entries).toEqual([
    {
      path: "a.txt",
      oldPath: null,
      before: { kind: "head" },
      after: { kind: "selection", selection: [{ hunk: "h2", lines: null }] },
    },
    { path: "c.txt", oldPath: null, before: { kind: "head" }, after: { kind: "disk" } },
  ]);
});

// `CreatePatchAction`: the Create Patch dialog with the chosen changes; two lists turn both off.
test("create_patch_from_local_changes_opens_the_create_patch_dialog", async () => {
  mockCommands();
  const { commitDialog, closeCommitDialog } = await import("./dialogStore");
  await open();

  await fireEvent.click(row("a.txt"));
  const files = await contextMenu("a.txt");
  await fireEvent.click(within(files).getByRole("menuitem", { name: "Create Patch from Local Changes..." }));

  expect(get(commitDialog)).toMatchObject({
    executor: "patch",
    request: { kind: "project", paths: ["a.txt"], lists: [] },
  });
  closeCommitDialog();

  await fireEvent.click(row(/^Changes/));
  await fireEvent.click(row(/^Other/), { metaKey: true });
  const two = await contextMenu(/^Other/);
  for (const name of ["Create Patch from Local Changes...", "Copy as Patch to Clipboard"]) {
    expect((within(two).getByRole("menuitem", { name }) as HTMLButtonElement).disabled, name).toBe(true);
  }
});

// `ChangesView.Shelve`: the Shelve Changes dialog with the chosen changes, enabled as Create Patch.
test("shelve_changes_opens_the_dialog_titled_shelve_changes_and_shelves_the_included_part", async () => {
  mockCommands();
  const { commitDialog, closeCommitDialog } = await import("./dialogStore");
  await open();

  await fireEvent.click(row("a.txt"));
  const files = await contextMenu("a.txt");
  await fireEvent.click(within(files).getByRole("menuitem", { name: "Shelve Changes..." }));

  expect(get(commitDialog)).toMatchObject({
    executor: "shelve",
    request: { kind: "project", paths: ["a.txt"], lists: [] },
  });
  closeCommitDialog();

  await fireEvent.click(row(/^Changes/));
  await fireEvent.click(row(/^Other/), { metaKey: true });
  const two = await contextMenu(/^Other/);
  const shelve = within(two).getByRole("menuitem", { name: "Shelve Changes..." });
  expect((shelve as HTMLButtonElement).disabled).toBe(true);
});

// `ChangesView.ShelveSilently`, on the toolbar and with ⇧⌘H: one shelf per changelist of the
// chosen changes, with rollback. Unversioned files are not changes: alone, they leave it off.
test("shelve_silently_makes_one_shelf_per_list_and_rolls_back", async () => {
  mockCommands({ repo_status: () => WITH_NEW, shelf_shelve_silently: () => 7 });
  await open();
  const toolbar = screen.getByRole("toolbar", { name: "Changes" });
  const button = within(toolbar).getByRole("button", { name: "Shelve Silently" }) as HTMLButtonElement;
  expect(button.disabled).toBe(true);

  await fireEvent.click(row(/^Changes/));
  await fireEvent.click(row("c.txt"), { metaKey: true });
  await fireEvent.click(row(/^Unversioned Files/), { metaKey: true });
  await fireEvent.click(button);

  const groups = [
    {
      name: "Changes",
      entries: [
        { path: "a.txt", oldPath: null, selection: null },
        { path: "b.txt", oldPath: null, selection: null },
      ],
    },
    { name: "Other", entries: [{ path: "c.txt", oldPath: null, selection: null }] },
  ];
  await waitFor(() => expect(calls("shelf_shelve_silently")).toHaveLength(1));
  expect(calls("shelf_shelve_silently")[0][1]).toEqual({
    lists: groups,
    rollback: true,
    expectedRepo: "/repo",
  });

  await fireEvent.keyDown(row("c.txt"), { key: "H", metaKey: true, shiftKey: true });
  await waitFor(() => expect(calls("shelf_shelve_silently")).toHaveLength(2));
  expect(calls("shelf_shelve_silently")[1][1]).toEqual(calls("shelf_shelve_silently")[0][1]);

  await fireEvent.click(row(/^Unversioned Files/));
  expect(button.disabled).toBe(true);
});

// `scheduleAutomaticEmptyChangeListDeletion(list, true)`: after Shelve Changes the list is read
// again; left empty and not active, it is deleted without asking. The active one and one that still
// has changes stay.
test("an_emptied_changelist_is_deleted", async () => {
  let status = [change("a.txt"), change("b.txt"), change("c.txt")];
  mockCommands({ repo_status: () => status, changelists_remove: () => LISTS });
  const view = await open();

  await view.component.removeIfEmpty("other");
  expect(calls("changelists_remove")).toEqual([]);

  status = [];
  await view.component.removeIfEmpty("default");
  expect(calls("changelists_remove")).toEqual([]);
  await view.component.removeIfEmpty("other");
  expect(calls("changelists_remove")).toEqual([["changelists_remove", { id: "other" }]]);
});

// Drag and drop between changelists (`ChangesViewDnDSupport`). Rows sit at `top`, 24 px high.
const yOf = (element: HTMLElement) => parseFloat(element.style.top) + 12;
const dragLabel = () => document.querySelector<HTMLElement>(".drag-label");

async function startDrag(from: string | RegExp) {
  const source = row(from);
  await fireEvent.pointerDown(source, { button: 0, clientX: 40, clientY: yOf(source) });
  await fireEvent.pointerMove(window, { buttons: 1, clientX: 40, clientY: yOf(source) + 10 });
}

async function moveOver(to: string | RegExp) {
  await fireEvent.pointerMove(window, { buttons: 1, clientX: 40, clientY: yOf(row(to)) });
}

async function dropOn(to: string | RegExp) {
  await moveOver(to);
  await fireEvent.pointerUp(window, { clientX: 40, clientY: yOf(row(to)) });
}

const movingCommands = () => mockCommands({ changelists_move: () => LISTS });

test("dragging_a_file_onto_another_list_highlights_it_and_drops_there", async () => {
  movingCommands();
  await open();

  await startDrag("a.txt");
  await moveOver(/^Other/);
  expect(row(/^Other/).hasAttribute("data-drop-target")).toBe(true);
  await fireEvent.pointerUp(window, { clientX: 40, clientY: yOf(row(/^Other/)) });

  await waitFor(() =>
    expect(calls("changelists_move").map(([, args]) => args)).toEqual([
      { paths: ["a.txt"], id: "other" },
    ]),
  );
  expect(row(/^Other/).hasAttribute("data-drop-target")).toBe(false);
});

test("a_drop_on_a_file_row_lands_on_its_list", async () => {
  movingCommands();
  await open();

  await startDrag("a.txt");
  expect(row("c.txt").hasAttribute("data-drop-target")).toBe(false);
  await moveOver("c.txt");
  expect(row(/^Other/).hasAttribute("data-drop-target")).toBe(true);
  await dropOn("c.txt");

  await waitFor(() =>
    expect(calls("changelists_move").map(([, args]) => args)).toEqual([
      { paths: ["a.txt"], id: "other" },
    ]),
  );
});

test("a_drop_on_the_source_list_runs_nothing_and_shows_no_highlight", async () => {
  movingCommands();
  await open();

  await startDrag("a.txt");
  await moveOver(/^Changes/);
  expect(row(/^Changes/).hasAttribute("data-drop-target")).toBe(false);
  expect(screen.getByRole("tree").classList.contains("no-drop")).toBe(true);
  await dropOn("b.txt");

  expect(calls("changelists_move")).toHaveLength(0);
});

test("a_move_under_5_points_is_a_click", async () => {
  movingCommands();
  await open();

  const source = row("a.txt");
  await fireEvent.pointerDown(source, { button: 0, clientX: 40, clientY: yOf(source) });
  await fireEvent.pointerMove(window, { buttons: 1, clientX: 43, clientY: yOf(source) + 3 });
  expect(dragLabel()).toBeNull();
  await fireEvent.pointerUp(window, { clientX: 43, clientY: yOf(source) + 3 });
  await fireEvent.click(source);

  expect(row("a.txt").getAttribute("aria-selected")).toBe("true");
  expect(calls("changelists_move")).toHaveLength(0);
});

test("dragging_an_unselected_row_selects_it_alone", async () => {
  movingCommands();
  await open();
  await fireEvent.click(row("b.txt"));

  await startDrag("a.txt");
  expect(row("a.txt").getAttribute("aria-selected")).toBe("true");
  expect(row("b.txt").getAttribute("aria-selected")).toBe("false");
  await dropOn(/^Other/);

  await waitFor(() =>
    expect(calls("changelists_move").map(([, args]) => args)).toEqual([
      { paths: ["a.txt"], id: "other" },
    ]),
  );
});

test("dragging_a_selected_row_keeps_the_whole_selection", async () => {
  movingCommands();
  await open();
  await fireEvent.click(row("a.txt"));
  await fireEvent.click(row("b.txt"), { metaKey: true });

  await startDrag("a.txt");
  expect(dragLabel()?.textContent?.trim()).toBe("2 files");
  await dropOn(/^Other/);

  await waitFor(() =>
    expect(calls("changelists_move").map(([, args]) => args)).toEqual([
      { paths: ["a.txt", "b.txt"], id: "other" },
    ]),
  );
});

test("dragging_a_list_header_moves_every_file_and_keeps_the_list", async () => {
  movingCommands();
  await open();

  await startDrag(/^Changes/);
  expect(dragLabel()?.textContent?.trim()).toBe("2 files");
  await dropOn(/^Other/);

  await waitFor(() =>
    expect(calls("changelists_move").map(([, args]) => args)).toEqual([
      { paths: ["a.txt", "b.txt"], id: "other" },
    ]),
  );
  expect(calls("changelists_remove")).toHaveLength(0);
});

test("the_label_says_1_file_next_to_the_pointer", async () => {
  movingCommands();
  await open();

  await startDrag("a.txt");
  await fireEvent.pointerMove(window, { buttons: 1, clientX: 120, clientY: 200 });

  const label = dragLabel();
  expect(label?.textContent?.trim()).toBe("1 file");
  expect(label?.style.left).toBe("120px");
  expect(label?.style.top).toBe("200px");
});

test("escape_cancels_and_releasing_runs_nothing", async () => {
  movingCommands();
  await open();

  await startDrag("a.txt");
  await moveOver(/^Other/);
  await fireEvent.keyDown(window, { key: "Escape" });
  expect(dragLabel()).toBeNull();
  expect(row(/^Other/).hasAttribute("data-drop-target")).toBe(false);
  await fireEvent.pointerUp(window, { clientX: 40, clientY: yOf(row(/^Other/)) });

  expect(calls("changelists_move")).toHaveLength(0);
});

test("no_click_follows_a_drop", async () => {
  movingCommands();
  await open();

  await startDrag("a.txt");
  await dropOn("c.txt");
  await fireEvent.click(row("c.txt"));

  expect(row("a.txt").getAttribute("aria-selected")).toBe("true");
  expect(row("c.txt").getAttribute("aria-selected")).toBe("false");
});

test("the_tree_scrolls_when_the_pointer_nears_an_edge", async () => {
  movingCommands();
  await open();
  const tree = screen.getByRole("tree");
  tree.getBoundingClientRect = () =>
    ({ top: 0, bottom: 100, height: 100, left: 0, right: 300, width: 300 }) as DOMRect;

  await startDrag("a.txt");
  await fireEvent.pointerMove(window, { buttons: 1, clientX: 40, clientY: 95 });

  await waitFor(() => expect(tree.scrollTop).toBeGreaterThan(0));
  await fireEvent.keyDown(window, { key: "Escape" });
});

test("a_drop_of_unversioned_files_adds_them_then_moves_them_to_the_target_not_the_active_list", async () => {
  await listenToTasks();
  addingCommands();
  await open();

  await startDrag(/^Unversioned Files/);
  await dropOn(/^Other/);

  await waitFor(() =>
    expect(calls("changelists_move").map(([, args]) => args)).toEqual([
      { paths: ["new.txt"], id: "other" },
    ]),
  );
  expect(calls("changes_add").map(([, args]) => args)).toEqual([
    { repo: "/repo", paths: ["new.txt"] },
  ]);
});

test("a_mixed_drop_moves_first_then_adds", async () => {
  await listenToTasks();
  addingCommands();
  await open();
  await fireEvent.click(row("a.txt"));
  await fireEvent.click(row(/^Unversioned Files/), { metaKey: true });

  await startDrag("a.txt");
  await dropOn(/^Other/);

  await waitFor(() => expect(calls("changelists_move")).toHaveLength(2));
  const order = vi
    .mocked(invoke)
    .mock.calls.filter(([name]) => name === "changelists_move" || name === "changes_add")
    .map(([name, args]) => [name, (args as { paths: string[] }).paths]);
  expect(order).toEqual([
    ["changelists_move", ["a.txt"]],
    ["changes_add", ["new.txt"]],
    ["changelists_move", ["new.txt"]],
  ]);
});

test("a_failed_add_after_a_drop_shows_failed_to_add_files", async () => {
  await listenToTasks();
  addingCommands("fatal: pathspec did not match");
  await open();

  await startDrag(/^Unversioned Files/);
  await dropOn(/^Other/);

  await waitFor(() => expect(get(notifications)).toHaveLength(1));
  expect(get(notifications)[0]).toMatchObject({ kind: "error", title: "Failed to add files" });
  expect(calls("changelists_move")).toHaveLength(0);
});

test("releasing_outside_the_tree_runs_nothing_even_at_a_lists_height", async () => {
  movingCommands();
  await open();
  const tree = screen.getByRole("tree");
  tree.getBoundingClientRect = () =>
    ({ top: 0, bottom: 400, height: 400, left: 0, right: 300, width: 300 }) as DOMRect;

  await startDrag("a.txt");
  await fireEvent.pointerMove(window, { buttons: 1, clientX: 500, clientY: yOf(row(/^Other/)) });
  expect(row(/^Other/).hasAttribute("data-drop-target")).toBe(false);
  await fireEvent.pointerUp(window, { clientX: 500, clientY: yOf(row(/^Other/)) });
  await fireEvent.pointerUp(window, { clientX: 40, clientY: 1000 });

  expect(calls("changelists_move")).toHaveLength(0);
});

test("below_the_tree_it_neither_scrolls_nor_drops", async () => {
  movingCommands();
  await open();
  const tree = screen.getByRole("tree");
  tree.getBoundingClientRect = () =>
    ({ top: 0, bottom: 100, height: 100, left: 0, right: 300, width: 300 }) as DOMRect;

  await startDrag("a.txt");
  await fireEvent.pointerMove(window, { buttons: 1, clientX: 40, clientY: 300 });
  await new Promise((resolve) => setTimeout(resolve, 120));
  expect(tree.scrollTop).toBe(0);
  await fireEvent.pointerUp(window, { clientX: 40, clientY: 300 });

  expect(calls("changelists_move")).toHaveLength(0);
});

test("a_move_without_the_button_held_ends_the_drag", async () => {
  movingCommands();
  await open();

  await startDrag("a.txt");
  await fireEvent.pointerMove(window, { clientX: 40, clientY: yOf(row(/^Other/)), buttons: 0 });
  expect(dragLabel()).toBeNull();
  await fireEvent.pointerUp(window, { clientX: 40, clientY: yOf(row(/^Other/)) });

  expect(calls("changelists_move")).toHaveLength(0);
});

test("losing_the_window_focus_ends_the_drag", async () => {
  movingCommands();
  await open();

  await startDrag("a.txt");
  await fireEvent.blur(window);
  expect(dragLabel()).toBeNull();
});

test("an_empty_list_does_not_start_a_drag", async () => {
  mockCommands({
    changelists_get: () => ({ ...LISTS, assignments: { "a.txt": "default", "b.txt": "default", "c.txt": "default" } }),
    changelists_move: () => LISTS,
  });
  await open();

  await startDrag(/^Other/);

  expect(dragLabel()).toBeNull();
});

test("a_split_file_dropped_moves_only_the_hunks_of_its_list", async () => {
  const split: Changelists = {
    ...LISTS,
    lists: [...LISTS.lists, { id: "feature", name: "Feature", comment: "" }],
    hunkAssignments: { "a.txt": { h1: "default", h2: "other" } },
  };
  mockCommands({
    changelists_get: () => split,
    changelists_move: () => split,
    changelists_move_hunks: () => split,
  });
  await open();

  await startDrag(/^Changes/);
  await dropOn(/^Feature/);

  await waitFor(() =>
    expect(calls("changelists_move_hunks").map(([, args]) => args)).toEqual([
      { path: "a.txt", hunks: ["h1"], id: "feature" },
    ]),
  );
  expect(calls("changelists_move").map(([, args]) => args)).toEqual([
    { paths: ["b.txt"], id: "feature" },
  ]);
});

test("a_double_click_and_modified_clicks_still_work_with_drag_on", async () => {
  movingCommands();
  await open();

  const source = row("a.txt");
  await fireEvent.pointerDown(source, { button: 0, clientX: 40, clientY: yOf(source) });
  await fireEvent.pointerUp(window, { clientX: 40, clientY: yOf(source) });
  await fireEvent.click(source);
  await fireEvent.pointerDown(row("c.txt"), { button: 0, metaKey: true, clientX: 40, clientY: yOf(row("c.txt")) });
  await fireEvent.pointerMove(window, { buttons: 1, clientX: 40, clientY: yOf(row("c.txt")) + 30 });
  expect(dragLabel()).toBeNull();
  await fireEvent.click(row("c.txt"), { metaKey: true });
  expect(row("a.txt").getAttribute("aria-selected")).toBe("true");
  expect(row("c.txt").getAttribute("aria-selected")).toBe("true");

  await fireEvent.pointerDown(row(/^Changes/), { button: 0, clientX: 40, clientY: yOf(row(/^Changes/)) });
  await fireEvent.pointerUp(window, { clientX: 40, clientY: yOf(row(/^Changes/)) });
  await fireEvent.dblClick(row(/^Changes/));
  expect(screen.queryByRole("treeitem", { name: "a.txt" })).toBeNull();
});
