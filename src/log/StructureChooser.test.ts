import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { FileChange, TreeEntry } from "../lib/api";
import { checkState, toggleChecked } from "../tree/model";
import { filterPath, pathFilterValues } from "./filters";
import StructureChooser from "./StructureChooser.svelte";

// `VcsStructureChooser.MAX_FOLDERS`.
const MAX_SELECTED = 100;
const toggled = (selected: ReadonlySet<string>, path: string) => toggleChecked(selected, path, MAX_SELECTED);

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

// jsdom has no ResizeObserver, which `bind:clientHeight` needs.
vi.stubGlobal(
  "ResizeObserver",
  class {
    observe() {}
    unobserve() {}
    disconnect() {}
  },
);

const file = (path: string): TreeEntry => ({ name: path.split("/").pop()!, path, dir: false });
const folder = (path: string): TreeEntry => ({ ...file(path), dir: true });

// What the disk has, by folder, as `tree_list` lists it (folders first, by name).
let disk: Record<string, TreeEntry[]>;
let status: FileChange[];

const change = (path: string, state: FileChange["state"]) => ({ path, state }) as FileChange;

beforeEach(() => {
  disk = {
    "": [folder("docs"), folder("out"), folder("src"), file("B.md"), file("a.txt"), file("new.txt")],
    src: [folder("src/lib"), file("src/main.ts")],
    "src/lib": [file("src/lib/util.ts")],
    docs: [file("docs/guide.md")],
    out: [file("out/app.js")],
  };
  status = [change("out/", "ignored"), change("new.txt", "untracked"), change("a.txt", "modified")];
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "tree_list") return disk[(args as { dir: string }).dir] ?? [];
    if (command === "repo_status") return status;
    throw new Error(`unexpected command: ${command}`);
  });
});

afterEach(cleanup);

async function chooser(paths: string[] = [], root = "/work/repo") {
  const onok = vi.fn<(values: string[]) => void>();
  const onclose = vi.fn();
  render(StructureChooser, { root, paths, onok, onclose });
  const dialog = await screen.findByRole("dialog", { name: "Select Paths to Filter by" });
  await within(dialog).findByRole("treeitem", { name: "src" });
  return { dialog, onok, onclose };
}

const names = () => screen.getAllByRole("treeitem").map((row) => row.getAttribute("aria-label"));
const box = (name: string) => screen.getByRole("checkbox", { name });
const state = (name: string) => [box(name).getAttribute("aria-checked"), box(name).getAttribute("aria-disabled")];
const ok = () => screen.getByRole("button", { name: "OK" }) as HTMLButtonElement;

async function expand(name: string) {
  const row = screen.getByRole("treeitem", { name });
  await fireEvent.dblClick(row);
  await waitFor(() => expect(row.getAttribute("aria-expanded")).toBe("true"));
  await new Promise((resolve) => setTimeout(resolve));
}

test("structure_chooser_selects_folders_and_files_up_to_100", () => {
  let selected: ReadonlySet<string> = new Set();
  for (let index = 0; index < MAX_SELECTED + 1; index++) selected = toggled(selected, `f${index}.txt`);

  // The 101st is not checked.
  expect(selected.size).toBe(MAX_SELECTED);
  expect(selected.has(`f${MAX_SELECTED}.txt`)).toBe(false);
  // Unchecking one makes room again.
  selected = toggled(selected, "f0.txt");
  expect(toggled(selected, "other.txt").has("other.txt")).toBe(true);
  // At the limit a folder with checked entries below still takes their place.
  const full = new Set([...Array(MAX_SELECTED - 1).keys()].map((index) => `a/${index}`).concat("b"));
  expect([...toggled(full, "a")].sort()).toEqual(["a", "b"]);
});

test("checking_follows_the_rules_of_the_selection_manager", () => {
  const selected = new Set(["src/lib", "src/main.ts"]);

  expect(checkState(selected, "src/lib")).toBe("selected");
  expect(checkState(selected, "src/lib/util.ts")).toBe("above");
  expect(checkState(selected, "src")).toBe("below");
  expect(checkState(selected, "")).toBe("below");
  expect(checkState(selected, "docs")).toBe("clear");
  // Under a checked folder nothing changes; checking a folder drops what is checked below it.
  expect(toggled(selected, "src/lib/util.ts")).toBe(selected);
  expect([...toggled(selected, "src")]).toEqual(["src"]);
  expect([...toggled(selected, "src/lib")]).toEqual(["src/main.ts"]);
  // The repository is a root like the others.
  expect([...toggled(selected, "")]).toEqual([""]);
});

test("the_filter_values_are_those_of_path2text", () => {
  const isDir = (path: string) => path === "" || path === "src";

  expect(pathFilterValues(["src", "a.txt"], isDir)).toEqual(["a.txt", "src/"]);
  expect(pathFilterValues([""], isDir)).toEqual(["./"]);
  expect(["src/", "src", "./", ".", "./src/lib"].map(filterPath)).toEqual(["src", "src", "", "", "src/lib"]);
});

test("folders_and_files_without_ignored_or_unversioned", async () => {
  await chooser();

  // The repository with its parent folder, then folders first and by path without case.
  expect(names()).toEqual(["repo", "docs", "src", "a.txt", "B.md"]);
  expect(screen.getByRole("treeitem", { name: "repo" }).textContent).toMatch(/repo\s*\(\/work\/repo\)/);
  await expand("src");
  expect(names()).toEqual(["repo", "docs", "src", "lib", "main.ts", "a.txt", "B.md"]);
});

test("a_root_right_under_the_top_folder_shows_its_path_too", async () => {
  await chooser([], "/repo");

  // `putParentPath`: only a root without a parent folder (`/`) has no grey path.
  expect(screen.getByRole("treeitem", { name: "repo" }).textContent).toMatch(/repo\s*\(\/repo\)/);
});

test("checkboxes_and_ok_disabled_while_nothing_is_checked", async () => {
  const { onok } = await chooser();
  await expand("src");
  expect(ok().disabled).toBe(true);
  expect(document.querySelector("p.selected")?.textContent?.trim()).toBe("");

  await fireEvent.click(box("main.ts"));
  expect(state("main.ts")).toEqual(["true", "false"]);
  expect(state("src")).toEqual(["true", "true"]);
  expect(state("repo")).toEqual(["true", "true"]);
  expect(ok().disabled).toBe(false);

  // The folder takes the place of the file below it, which shows checked and disabled.
  await fireEvent.click(box("src"));
  expect(state("src")).toEqual(["true", "false"]);
  expect(state("main.ts")).toEqual(["true", "true"]);
  await fireEvent.click(box("main.ts"));
  expect(state("main.ts")).toEqual(["true", "true"]);

  // Space toggles the selected rows.
  const tree = screen.getByRole("tree");
  await fireEvent.click(screen.getByRole("treeitem", { name: "a.txt" }));
  await fireEvent.keyDown(tree, { key: " " });
  expect(state("a.txt")).toEqual(["true", "false"]);

  await fireEvent.click(ok());
  expect(onok).toHaveBeenCalledWith(["a.txt", "src/"]);
});

test("the_existing_paths_of_the_filter_start_checked_and_the_limit_is_said_in_red", async () => {
  await chooser(["src/", "gone.txt", "docs/guide.md"]);
  await waitFor(() => expect(state("src")).toEqual(["true", "false"]));
  expect(ok().disabled).toBe(false);
  cleanup();

  disk[""] = [...Array(MAX_SELECTED).keys()].map((index) => file(`f${index}.txt`)).concat(folder("src"));
  const { onok } = await chooser([...Array(MAX_SELECTED).keys()].map((index) => `f${index}.txt`));

  const label = await screen.findByText(/No more is allowed/);
  expect(label.textContent).toBe(`(You have added ${MAX_SELECTED} elements. No more is allowed.)`);
  expect(label.parentElement?.textContent?.trim()).toBe(
    `Selected: (You have added ${MAX_SELECTED} elements. No more is allowed.)`,
  );
  await fireEvent.click(box("src"));
  expect(state("src")).toEqual(["false", "false"]);
  await fireEvent.click(ok());
  expect(onok.mock.calls[0][0]).toHaveLength(MAX_SELECTED);
});
