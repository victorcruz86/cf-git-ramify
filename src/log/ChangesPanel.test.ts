import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { ChangeGroup } from "../lib/api";
import ChangesPanel from "./ChangesPanel.svelte";
import { defaultOptions } from "./options";
import { compareChangesWithLocal, showHistory } from "../compare/actions";
import { getRevision } from "../changes/get";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("../compare/actions", () => ({
  showHistory: vi.fn(async () => {}),
  compareChangesWithLocal: vi.fn(async () => {}),
}));
vi.mock("../changes/get", () => ({ getRevision: vi.fn(async () => null) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn() }));

const HASH = "a".repeat(40);

const file = (path: string, status = "M", oldPath: string | null = null) => ({
  path,
  oldPath,
  status,
});

const GROUP: ChangeGroup = {
  parent: null,
  subject: null,
  base: "f".repeat(40),
  merge: false,
  files: [file("src/a.txt"), file("gone.txt", "D"), file("src/new.txt", "R", "src/old.txt")],
  size: 3,
};

// The local files on disk: `viewer_openable` keeps the targets among them, as `viewer_open` does.
let onDisk: Set<string>;

beforeEach(() => {
  onDisk = new Set(["src/a.txt", "src/new.txt", "src/old.txt"]);
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "commit_changes_of") return [GROUP];
    if (command === "viewer_open") return undefined;
    if (command === "viewer_openable") {
      const { targets } = args as { targets: { path: string; revision: string | null }[] };
      return targets.filter(({ path, revision }) => revision !== null || onDisk.has(path));
    }
    throw new Error(`unexpected command: ${command}`);
  });
});

afterEach(cleanup);

function viewerOpened() {
  return vi
    .mocked(invoke)
    .mock.calls.filter(([name]) => name === "viewer_open")
    .map(([, args]) => (args as { targets: unknown }).targets);
}

async function panel(modal = false) {
  render(ChangesPanel, {
    hashes: [HASH],
    paths: [],
    options: defaultOptions(),
    onoptions: () => {},
    onselect: () => {},
    onopen: () => {},
    modal,
  });
  return screen.findByRole("treeitem", { name: "a.txt" });
}

async function menuOn(name: string) {
  await fireEvent.contextMenu(screen.getByRole("treeitem", { name }));
  return screen.findByRole("menu");
}

const item = (menu: HTMLElement, name: string) =>
  within(menu).getByRole("menuitem", { name: new RegExp(`^${name}`) }) as HTMLButtonElement;

// `Vcs.RepositoryChangesBrowserMenu`: `Edit Source` opens the local file at the change's path
// (G§0.7) and `Open Repository Version` its after-revision, read-only.
test("edit_source_opens_the_local_file_and_open_repository_version_the_revision", async () => {
  await panel();

  let menu = await menuOn("a.txt");
  await fireEvent.click(item(menu, "Edit Source"));
  expect(viewerOpened()).toEqual([[{ path: "src/a.txt", revision: null, line: null }]]);

  menu = await menuOn("new.txt");
  await fireEvent.click(item(menu, "Open Repository Version"));
  expect(viewerOpened()[1]).toEqual([{ path: "src/new.txt", revision: HASH, line: null }]);
});

// `ChangesUtil.iterateFiles`: the before path, then the after path of each change.
test("edit_source_of_a_rename_opens_the_before_and_the_after_path", async () => {
  await panel();

  const menu = await menuOn("new.txt");
  await fireEvent.click(item(menu, "Edit Source"));

  expect(viewerOpened()).toEqual([
    [
      { path: "src/old.txt", revision: null, line: null },
      { path: "src/new.txt", revision: null, line: null },
    ],
  ]);
});

// `EditSourceAction`: without a file to open, it is disabled.
test("edit_source_is_disabled_when_no_chosen_file_is_on_disk", async () => {
  await panel();

  let menu = await menuOn("gone.txt");
  await waitFor(() => expect(item(menu, "Edit Source").disabled).toBe(true));
  await fireEvent.keyDown(menu, { key: "Escape" });

  await fireEvent.click(screen.getByRole("treeitem", { name: "a.txt" }), { metaKey: true });
  menu = await menuOn("gone.txt");
  expect(item(menu, "Edit Source").disabled).toBe(false);
});

test("open_repository_version_is_disabled_when_every_change_is_a_deletion", async () => {
  onDisk.add("gone.txt");
  await panel();

  let menu = await menuOn("gone.txt");
  expect(item(menu, "Open Repository Version").disabled).toBe(true);
  expect(item(menu, "Edit Source").disabled).toBe(false);
  await fireEvent.keyDown(menu, { key: "Escape" });

  await fireEvent.click(screen.getByRole("treeitem", { name: "a.txt" }), { metaKey: true });
  menu = await menuOn("gone.txt");
  await fireEvent.click(item(menu, "Open Repository Version"));

  expect(viewerOpened()).toEqual([[{ path: "src/a.txt", revision: HASH, line: null }]]);
});

test("f4_and_meta_down_edit_the_source_of_the_chosen_files", async () => {
  const row = await panel();

  await fireEvent.click(row);
  await fireEvent.keyDown(row, { key: "F4" });
  await fireEvent.keyDown(row, { key: "ArrowDown", metaKey: true });

  expect(viewerOpened()).toEqual([
    [{ path: "src/a.txt", revision: null, line: null }],
    [{ path: "src/a.txt", revision: null, line: null }],
  ]);
});

// `IS_MODAL_CONTEXT`: in the push and interactive rebase dialogs both stay disabled.
test("in_a_modal_dialog_neither_opens_anything", async () => {
  const row = await panel(true);

  const menu = await menuOn("a.txt");
  expect(item(menu, "Edit Source").disabled).toBe(true);
  expect(item(menu, "Open Repository Version").disabled).toBe(true);
  await fireEvent.keyDown(row, { key: "F4" });

  await waitFor(() => expect(viewerOpened()).toEqual([]));
});

// `Vcs.RepositoryChangesBrowserMenu` (`VA:330-340`); `Get` is `GetVersionFromRepositoryActionProvider`,
// which names itself `Get from Revision` (`V:169`). The toolbar is `Vcs.RepositoryChangesBrowserToolbar`.
test("changes_panel_menu_in_241_order", async () => {
  const onvcs = vi.fn();
  render(ChangesPanel, {
    hashes: [HASH],
    paths: [],
    options: defaultOptions(),
    onoptions: () => {},
    onselect: () => {},
    onopen: () => {},
    onvcs,
  });
  await screen.findByRole("treeitem", { name: "a.txt" });

  const menu = await menuOn("new.txt");
  expect(within(menu).getAllByRole("menuitem").map((entry) => entry.textContent?.trim())).toEqual([
    "Compare with Local",
    "Compare Before with Local",
    "Edit Source ⌘↓",
    "Open Repository Version",
    "Revert Selected Changes",
    "Cherry-Pick Selected Changes",
    "Create Patch...",
    "Get from Revision",
    "History Up to Here",
  ]);
  const change = { file: file("src/new.txt", "R", "src/old.txt"), hash: HASH, base: GROUP.base };
  await fireEvent.click(item(menu, "Compare with Local"));
  expect(compareChangesWithLocal).toHaveBeenLastCalledWith(
    [change],
    false,
    new Set(["src/new.txt", "src/old.txt"]),
    change,
  );
  await fireEvent.click(item(await menuOn("new.txt"), "Compare Before with Local"));
  expect(compareChangesWithLocal).toHaveBeenLastCalledWith(
    [change],
    true,
    new Set(["src/new.txt", "src/old.txt"]),
    change,
  );
  await fireEvent.click(item(await menuOn("gone.txt"), "Get from Revision"));
  expect(getRevision).toHaveBeenLastCalledWith([{ path: "gone.txt", revision: HASH }]);

  const toolbar = screen.getByRole("toolbar", { name: "Changes" });
  const revert = within(toolbar).getByRole("button", { name: "Revert Selected Changes" });
  const history = within(toolbar).getByRole("button", { name: "History Up to Here" });
  expect(within(toolbar).getAllByRole("button").map((entry) => entry.getAttribute("aria-label"))).toEqual([
    "Revert Selected Changes",
    "History Up to Here",
    "Changes options",
  ]);
  await fireEvent.click(revert);
  expect(onvcs).toHaveBeenLastCalledWith({
    kind: "applyPaths",
    reverse: true,
    selection: [{ hash: HASH, base: GROUP.base, paths: ["gone.txt"] }],
    hashes: [HASH],
  });
  await fireEvent.click(history);
  expect(vi.mocked(showHistory)).toHaveBeenLastCalledWith("gone.txt", HASH);
});

// `TabbedShowHistoryForRevisionAction` (`A:31`): one change, its file's history up to the commit;
// Task 8 places it in 241's order and on the toolbar.
test("history_up_to_here_opens_the_file_history_at_that_commit", async () => {
  await panel();

  const menu = await menuOn("new.txt");
  await fireEvent.click(item(menu, "History Up to Here"));

  expect(vi.mocked(showHistory)).toHaveBeenCalledWith("src/new.txt", HASH);
});
