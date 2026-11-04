import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Changelists, FileChange, FileState, RepoInfo, WorktreeSides } from "../lib/api";
import { checkinFilesText } from "./checkin";
import LocalChangesView from "./LocalChangesView.svelte";

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
