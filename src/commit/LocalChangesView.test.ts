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
    "View Options",
    "Expand All",
    "Collapse All",
    "Preview Diff",
  ]);
  expect(within(toolbar).queryByRole("button", { name: /Shelve/ })).toBeNull();
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
    "Import Changelists from .idea",
    "Restore Snapshot…",
    "Refresh",
  ]);
  expect(within(listMenu).queryByRole("menuitem", { name: /Shelve|Patch/ })).toBeNull();
  await fireEvent.keyDown(listMenu, { key: "Escape" });

  await fireEvent.click(row("a.txt"));
  await fireEvent.click(row("b.txt"), { metaKey: true });
  const filesMenu = await contextMenu("b.txt");
  expect(labels(filesMenu)[0]).toBe("Commit Files…");

  // rama lists every file, as `--untracked-files=all`, and folders are helper nodes as in
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

test("double click opens the diff", async () => {
  mockCommands();
  const onshowdiff = vi.fn();
  await open({ onshowdiff });

  await fireEvent.dblClick(row("a.txt"));

  expect(onshowdiff).toHaveBeenCalledTimes(1);
  expect(onshowdiff.mock.calls[0][0].path).toBe("a.txt");
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
    "Import Changelists from .idea",
    "Restore Snapshot…",
    "Refresh",
  ]);
  const item = within(menu).getByRole("menuitem", { name: "Add to VCS" });
  expect(item.textContent).toContain("⌥⌘A");
  // The group of `$Delete`, `ChangesView.AddUnversioned` and `ChangesView.RemoveDeleted`, between two
  // separators: rama has neither Delete nor Remove from VCS, so the group is only this item.
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
