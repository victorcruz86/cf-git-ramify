import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { get } from "svelte/store";
import type { FileTarget } from "../files/gitFileMenu";
import type { FileChange, FileState, TreeEntry } from "../lib/api";
import type { MenuAction } from "../lib/menu";
import { dismiss, notifications } from "../lib/notifications";
import { treeSelection } from "./model";
import ProjectTree from "./ProjectTree.svelte";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(), emit: vi.fn() }));

// jsdom has no ResizeObserver, which `bind:clientHeight` needs.
vi.stubGlobal(
  "ResizeObserver",
  class {
    observe() {}
    unobserve() {}
    disconnect() {}
  },
);

const handlers = new Map<string, (payload: unknown) => void>();
// What `tree_list` gives per folder, and what `repo_status` gives.
let disk: Record<string, TreeEntry[]> = {};
let changes: FileChange[] = [];
// With `hold`, each `tree_list` waits until `release` lets it answer; `inFlight` counts those waiting.
let hold = false;
// Folders whose `tree_list` fails.
let unreadable = new Set<string>();
let waiting: (() => void)[] = [];
let inFlight = 0;
let mostInFlight = 0;

const file = (path: string): TreeEntry => ({ name: path.split("/").pop()!, path, dir: false });
const folder = (path: string): TreeEntry => ({ name: path.split("/").pop()!, path, dir: true });
const change = (path: string, state: FileState): FileChange => ({
  path,
  oldPath: null,
  state,
  staged: false,
  unstaged: true,
});

function calls(command: string) {
  return vi.mocked(invoke).mock.calls.filter(([name]) => name === command);
}

const listedDirs = () => calls("tree_list").map(([, args]) => (args as { dir: string }).dir);
const opened = () => calls("viewer_open").map(([, args]) => (args as { targets: unknown }).targets);
const tree = () => screen.getByRole("tree", { name: "Project" });
const row = (name: string) => screen.getByRole("treeitem", { name });
const names = () => screen.queryAllByRole("treeitem").map((item) => item.getAttribute("aria-label"));
const colorOf = (name: string) =>
  (row(name).querySelector(".name") as HTMLElement).style.color || null;

async function key(key: string, init: KeyboardEventInit = {}) {
  await fireEvent.keyDown(tree(), { key, ...init });
}

async function select(name: string) {
  await fireEvent.click(row(name));
}

async function mount() {
  render(ProjectTree, { root: "/Users/me/rama" });
  await waitFor(() => expect(names()).toContain("src"));
  await waitFor(() => expect(handlers.has("repo-changed")).toBe(true));
}

// Lets every waiting `tree_list` answer, and the ones those start, until nothing waits.
async function release() {
  for (let round = 0; round < 50; round++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
    if (waiting.length === 0 && inFlight === 0) return;
    waiting.splice(0).forEach((resolve) => resolve());
  }
  throw new Error("tree_list calls still waiting");
}

beforeEach(() => {
  handlers.clear();
  hold = false;
  unreadable = new Set();
  waiting = [];
  inFlight = 0;
  mostInFlight = 0;
  disk = {
    "": [folder("[ab]"), folder("build"), folder("src"), file(".gitignore"), file("debug.log")],
    src: [folder("src/lib"), file("src/a.txt")],
    "src/lib": [file("src/lib/b.ts")],
    build: [file("build/out.o")],
    "[ab]": [file("[ab]/-x*?.txt"), file("[ab]/a.txt")],
  };
  changes = [change("build/", "ignored"), change("debug.log", "ignored"), change("src/a.txt", "modified")];
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    switch (command) {
      case "tree_list": {
        const { dir } = args as { dir: string };
        if (unreadable.has(dir)) throw { kind: "failed", message: `Permission denied: ${dir}` };
        const entries = disk[dir] ?? [];
        if (!hold) return entries;
        inFlight++;
        mostInFlight = Math.max(mostInFlight, inFlight);
        await new Promise<void>((resolve) => waiting.push(resolve));
        inFlight--;
        return entries;
      }
      case "repo_status":
        return changes;
      case "viewer_open":
        return undefined;
      default:
        throw new Error(`unexpected command: ${command}`);
    }
  });
  vi.mocked(listen).mockReset();
  vi.mocked(listen).mockImplementation(async (event, handler) => {
    handlers.set(event as string, (payload) => handler({ event: event as string, id: 0, payload } as never));
    return () => {};
  });
});

afterEach(() => {
  cleanup();
  get(notifications).forEach(({ id }) => dismiss(id));
});

test("the_root_opens_with_its_entries_coloured_from_git_status", async () => {
  await mount();

  expect(names()).toEqual(["rama", "[ab]", "build", "src", ".gitignore", "debug.log"]);
  expect(calls("repo_status")).toEqual([["repo_status", { includeIgnored: true }]]);
  await waitFor(() => expect(colorOf("debug.log")).toBe("var(--tree-status-ignored)"));
  expect(colorOf("build")).toBe("var(--tree-status-ignored)");
  expect(colorOf("src")).toBeNull();
  expect(colorOf(".gitignore")).toBeNull();
  expect(row("rama").getAttribute("aria-expanded")).toBe("true");
  expect(row("src").getAttribute("aria-expanded")).toBe("false");
});

test("double_click_opens_a_file_and_toggles_a_folder", async () => {
  await mount();

  await fireEvent.dblClick(row("src"));
  await waitFor(() => expect(names()).toContain("a.txt"));
  expect(names()).toEqual(["rama", "[ab]", "build", "src", "lib", "a.txt", ".gitignore", "debug.log"]);
  expect(row("src").getAttribute("aria-expanded")).toBe("true");
  expect(colorOf("a.txt")).toBe("var(--tree-status-modified)");
  expect(opened()).toEqual([]);

  await fireEvent.dblClick(row("a.txt"));
  expect(opened()).toEqual([[{ path: "src/a.txt", revision: null, line: null }]]);

  await fireEvent.dblClick(row("src"));
  expect(names()).toEqual(["rama", "[ab]", "build", "src", ".gitignore", "debug.log"]);
  // Opening it again shows what was read, and reads it again only on a change.
  await fireEvent.dblClick(row("src"));
  expect(names()).toContain("a.txt");
  expect(listedDirs()).toEqual(["", "src"]);
  expect(opened()).toHaveLength(1);
});

test("a_name_with_glob_characters_and_a_leading_dash_lists_and_opens", async () => {
  await mount();

  await select("[ab]");
  await key("Enter");
  await waitFor(() => expect(names()).toContain("-x*?.txt"));
  expect(listedDirs()).toContain("[ab]");

  await key("ArrowDown");
  await key("F4");
  await select("a.txt");
  await key("ArrowDown", { metaKey: true });
  expect(opened()).toEqual([
    [{ path: "[ab]/-x*?.txt", revision: null, line: null }],
    [{ path: "[ab]/a.txt", revision: null, line: null }],
  ]);
  expect(treeSelection()).toEqual(["[ab]/a.txt"]);
});

test("enter_on_a_file_opens_it_and_on_a_folder_toggles_it", async () => {
  await mount();

  await select("src");
  await key("Enter");
  await waitFor(() => expect(names()).toContain("a.txt"));
  await key("Enter");
  expect(names()).not.toContain("a.txt");
  await key("ArrowRight");
  await waitFor(() => expect(names()).toContain("a.txt"));
  await key("ArrowRight");
  expect(treeSelection()).toEqual(["src/lib"]);
  await key("ArrowLeft");
  expect(treeSelection()).toEqual(["src"]);
  await key("ArrowLeft");
  expect(names()).not.toContain("a.txt");

  await select("debug.log");
  await key("Enter");
  // F4 on a folder opens nothing.
  await select("build");
  await key("F4");
  expect(opened()).toEqual([[{ path: "debug.log", revision: null, line: null }]]);
});

test("a_change_on_disk_recolours_and_relists_open_folders", async () => {
  await mount();
  await fireEvent.dblClick(row("src"));
  await waitFor(() => expect(names()).toContain("a.txt"));
  // The root and `src` are open; `lib`, `build` and `[ab]` never were.
  expect(listedDirs()).toEqual(["", "src"]);

  disk = {
    ...disk,
    "": [...disk[""], file("new.txt")],
    src: [file("src/a.txt"), file("src/b.txt")],
  };
  changes = [change("src/a.txt", "added"), change("src/b.txt", "untracked"), change("new.txt", "untracked")];
  handlers.get("repo-changed")?.({ stash: false, shelf: false });

  await waitFor(() => expect(names()).toContain("b.txt"));
  expect(names()).toEqual(["rama", "[ab]", "build", "src", "a.txt", "b.txt", ".gitignore", "debug.log", "new.txt"]);
  await waitFor(() => expect(colorOf("a.txt")).toBe("var(--tree-status-added)"));
  expect(colorOf("b.txt")).toBe("var(--tree-status-unknown)");
  expect(colorOf("debug.log")).toBeNull();
  expect(colorOf("build")).toBeNull();
  // Only the folders that show are read again; `build` was never opened.
  expect(listedDirs()).toEqual(["", "src", "", "src"]);
  expect(calls("repo_status")).toHaveLength(2);
});

test("a_folder_read_then_collapsed_shows_what_changed_on_disk_when_expanded_again", async () => {
  await mount();
  await fireEvent.dblClick(row("src"));
  await waitFor(() => expect(names()).toContain("lib"));
  await fireEvent.dblClick(row("lib"));
  await waitFor(() => expect(names()).toContain("b.ts"));
  // `src` is collapsed with `lib` still expanded inside it, then both change on disk.
  await fireEvent.dblClick(row("src"));
  disk = {
    ...disk,
    src: [folder("src/lib"), file("src/c.txt")],
    "src/lib": [file("src/lib/d.ts")],
  };
  handlers.get("repo-changed")?.({ stash: false, shelf: false });
  await waitFor(() => expect(calls("repo_status")).toHaveLength(2));

  await fireEvent.dblClick(row("src"));
  await waitFor(() => expect(names()).toContain("d.ts"));
  expect(names()).toEqual(["rama", "[ab]", "build", "src", "lib", "d.ts", "c.txt", ".gitignore", "debug.log"]);
  expect(listedDirs()).toEqual(["", "src", "src/lib", "", "src", "src/lib"]);

  // Expand All reads again what changed while it was collapsed, too.
  await key("-", { metaKey: true });
  disk = { ...disk, "src/lib": [file("src/lib/e.ts")] };
  handlers.get("repo-changed")?.({ stash: false, shelf: false });
  await waitFor(() => expect(calls("repo_status")).toHaveLength(3));
  await key("=", { metaKey: true });
  await waitFor(() => expect(names()).toContain("e.ts"));
  expect(names()).not.toContain("d.ts");
});

test("a_refresh_reads_at_most_16_folders_at_once_and_changes_meanwhile_make_one_more_pass", async () => {
  const folders = Array.from({ length: 40 }, (_, index) => folder(`f${index}`));
  disk = { "": folders, ...Object.fromEntries(folders.map((entry) => [entry.path, [file(`${entry.path}/x`)]])) };
  render(ProjectTree, { root: "/Users/me/rama" });
  await waitFor(() => expect(names()).toContain("f0"));
  await waitFor(() => expect(handlers.has("repo-changed")).toBe(true));
  hold = true;
  await key("=", { metaKey: true });
  await release();
  expect(calls("tree_list")).toHaveLength(41);
  expect(mostInFlight).toBeLessThanOrEqual(16);

  // A build writing under the open folders: notices keep arriving while the folders are read.
  mostInFlight = 0;
  for (let notice = 0; notice < 5; notice++) handlers.get("repo-changed")?.({ stash: false, shelf: false });
  await new Promise((resolve) => setTimeout(resolve, 0));
  for (let notice = 0; notice < 5; notice++) handlers.get("repo-changed")?.({ stash: false, shelf: false });
  await release();

  expect(mostInFlight).toBe(16);
  // One pass for the first notice and one more for all those that came while it ran.
  expect(calls("tree_list")).toHaveLength(41 * 3);
  expect(calls("repo_status")).toHaveLength(3);
});

test("a_folder_that_cannot_be_read_is_reported_once_until_it_reads_again", async () => {
  await mount();
  await fireEvent.dblClick(row("src"));
  await waitFor(() => expect(names()).toContain("a.txt"));
  const changed = async (times: number) => {
    handlers.get("repo-changed")?.({ stash: false, shelf: false });
    await waitFor(() => expect(calls("repo_status")).toHaveLength(times));
    await new Promise((resolve) => setTimeout(resolve, 0));
  };
  const errors = () => get(notifications).map((shown) => shown.title);

  unreadable.add("src");
  await changed(2);
  await changed(3);
  expect(errors()).toEqual(["Permission denied: src"]);
  // What it showed before stays.
  expect(names()).toContain("a.txt");

  unreadable.clear();
  await changed(4);
  unreadable.add("src");
  await changed(5);
  expect(errors()).toEqual(["Permission denied: src", "Permission denied: src"]);
});

test("cmd_minus_collapses_all_and_cmd_equals_expands_all", async () => {
  await mount();
  await fireEvent.dblClick(row("src"));
  await waitFor(() => expect(names()).toContain("lib"));
  await select("lib");

  await key("-", { metaKey: true });
  expect(names()).toEqual(["rama", "[ab]", "build", "src", ".gitignore", "debug.log"]);
  expect(treeSelection()).toEqual(["src"]);

  await key("=", { metaKey: true });
  await waitFor(() => expect(names()).toContain("b.ts"));
  expect(names()).toEqual([
    "rama",
    "[ab]",
    "-x*?.txt",
    "a.txt",
    "build",
    "out.o",
    "src",
    "lib",
    "b.ts",
    "a.txt",
    ".gitignore",
    "debug.log",
  ]);
  expect(colorOf("out.o")).toBe("var(--tree-status-ignored)");
});

test("speed_search_jumps_between_matching_names", async () => {
  await mount();
  await select("rama");

  // `SpeedSearchBase.findElement`: the first match from the selected row on, any part of the name.
  await key("d");
  expect(treeSelection()).toEqual(["build"]);
  await key("e");
  expect(treeSelection()).toEqual(["debug.log"]);
  expect(screen.getByText("de", { selector: ".query" })).toBeTruthy();
  expect(row("debug.log").querySelector(".hit")?.textContent).toBe("de");
  await key("Backspace");
  expect(treeSelection()).toEqual(["debug.log"]);
  // The arrows go between the matches, round the end (`cycleScrolling`).
  await key("ArrowDown");
  expect(treeSelection()).toEqual(["build"]);
  await key("ArrowUp");
  expect(treeSelection()).toEqual(["debug.log"]);
  await key("Escape");
  expect(document.querySelector(".query")).toBeNull();
  await key("ArrowUp");
  expect(treeSelection()).toEqual([".gitignore"]);
});

test("the_context_menu_has_edit_source_and_the_git_file_menu", async () => {
  const asked: FileTarget[][] = [];
  const gitMenu = (files: FileTarget[]): MenuAction => {
    asked.push(files);
    return { label: "Git", submenu: [{ label: "Add" }] };
  };
  const reported: [FileTarget[], boolean][] = [];
  changes = [change("debug.log", "ignored"), change("src/a.ts", "modified")];
  render(ProjectTree, {
    root: "/Users/me/rama",
    gitMenu,
    onfiles: (files: FileTarget[], hasChanges: boolean) => reported.push([files, hasChanges]),
  });
  await waitFor(() => expect(names()).toContain("src"));
  // The root row is selected first: the repository's folder.
  await waitFor(() =>
    expect(reported.at(-1)).toEqual([[{ path: "", dir: true, state: null }], true]),
  );

  await fireEvent.contextMenu(row("debug.log"));
  const menu = screen.getByRole("menu", { name: "Project" });
  const items = [...menu.querySelectorAll('[role="menuitem"]')].map((item) => [
    item.getAttribute("aria-label"),
    (item as HTMLButtonElement).disabled,
  ]);
  expect(items).toEqual([
    ["Edit Source", false],
    ["Git", false],
  ]);
  expect(treeSelection()).toEqual(["debug.log"]);
  expect(asked.at(-1)).toEqual([{ path: "debug.log", dir: false, state: "ignored" }]);
  expect(reported.at(-1)).toEqual([[{ path: "debug.log", dir: false, state: "ignored" }], true]);

  await fireEvent.click(screen.getByRole("menuitem", { name: "Edit Source" }));
  expect(opened()).toEqual([[{ path: "debug.log", revision: null, line: null }]]);
  expect(screen.queryByRole("menu")).toBeNull();

  await fireEvent.contextMenu(row("src"));
  expect(screen.getByRole("menuitem", { name: "Edit Source" }).hasAttribute("disabled")).toBe(true);
});

test("shift_and_cmd_click_select_several_entries", async () => {
  await mount();

  await select("build");
  await fireEvent.click(row(".gitignore"), { shiftKey: true });
  expect(treeSelection()).toEqual(["build", "src", ".gitignore"]);
  await fireEvent.click(row("src"), { metaKey: true });
  expect(treeSelection()).toEqual(["build", ".gitignore"]);
  await fireEvent.click(row("debug.log"), { metaKey: true });
  await key("F4");
  expect(opened()).toEqual([
    [
      { path: ".gitignore", revision: null, line: null },
      { path: "debug.log", revision: null, line: null },
    ],
  ]);
});
