import { expect, test } from "vitest";
import type { FileChange, FileState, TreeEntry } from "../lib/api";
import { TOKENS } from "../lib/theme";
import {
  ROOT,
  collapsedAll,
  entryColor,
  fileTargets,
  hasLocalChanges,
  limiter,
  matchingRows,
  namePieces,
  openFolders,
  parentOf,
  provideTreeSelection,
  stateOf,
  statusIndex,
  treeSelection,
  visibleRows,
} from "./model";

const change = (path: string, state: FileState): FileChange => ({
  path,
  oldPath: null,
  state,
  staged: false,
  unstaged: state !== "ignored",
});
const file = (path: string): TreeEntry => ({ name: path.split("/").pop()!, path, dir: false });
const folder = (path: string): TreeEntry => ({ name: path.split("/").pop()!, path, dir: true });

const listed = new Map<string, TreeEntry[]>([
  [ROOT, [folder("build"), folder("src"), file(".gitignore"), file("debug.log")]],
  ["src", [folder("src/lib"), file("src/a.txt")]],
  ["src/lib", [file("src/lib/b.ts")]],
  ["build", [file("build/out.o")]],
]);

test("ds_store_and_dot_git_are_hidden_but_ignored_files_are_shown_coloured", () => {
  const index = statusIndex([
    change("build/", "ignored"),
    change("debug.log", "ignored"),
    change("src/a.txt", "modified"),
    change("src/new.txt", "untracked"),
    // `git rm --cached`: deleted in the index and unversioned on disk, which IntelliJ shows first.
    change("src/kept.txt", "deleted"),
    change("src/kept.txt", "untracked"),
    change("src/added.txt", "added"),
    change("src/both.txt", "unmerged"),
    change("src/moved.txt", "renamed"),
  ]);

  expect(stateOf(index, "debug.log")).toBe("ignored");
  // An ignored folder colours itself and everything under it (`hasAncestor`).
  expect(stateOf(index, "build")).toBe("ignored");
  expect(stateOf(index, "build/out.o")).toBe("ignored");
  expect(stateOf(index, "build/deep/x")).toBe("ignored");
  expect(stateOf(index, "buildx")).toBeNull();
  // Folders are not coloured by their contents (`SHOW_DIRTY_RECURSIVELY=false`).
  expect(stateOf(index, "src")).toBeNull();
  expect(stateOf(index, ROOT)).toBeNull();
  expect(stateOf(index, "src/kept.txt")).toBe("untracked");

  // The dark New UI scheme of 241 (`expUI_darkScheme.xml:55-74`).
  const colors = Object.fromEntries(
    ["src/a.txt", "src/moved.txt", "src/added.txt", "src/new.txt", "debug.log", "src/both.txt"].map(
      (path) => [path, entryColor(stateOf(index, path))],
    ),
  );
  expect(colors).toEqual({
    "src/a.txt": "var(--tree-status-modified)",
    "src/moved.txt": "var(--tree-status-modified)",
    "src/added.txt": "var(--tree-status-added)",
    "src/new.txt": "var(--tree-status-unknown)",
    "debug.log": "var(--tree-status-ignored)",
    "src/both.txt": "var(--tree-status-conflict)",
  });
  expect(entryColor(null)).toBeNull();
  expect(TOKENS).toMatchObject({
    "tree-status-added": "#73BD79",
    "tree-status-modified": "#70AEFF",
    "tree-status-deleted": "#6F737A",
    "tree-status-unknown": "#E88F89",
    "tree-status-ignored": "#D69A6B",
    "tree-status-conflict": "#DE6A66",
  });
});

test("rows_follow_the_expanded_and_loaded_folders", () => {
  const rows = visibleRows("rama", listed, new Set([ROOT, "src", "src/lib", "missing"]));

  expect(rows.map((row) => [row.entry.path, row.depth])).toEqual([
    [ROOT, 0],
    ["build", 1],
    ["src", 1],
    ["src/lib", 2],
    ["src/lib/b.ts", 3],
    ["src/a.txt", 2],
    [".gitignore", 1],
    ["debug.log", 1],
  ]);
  expect(rows[0].entry).toEqual({ name: "rama", path: ROOT, dir: true });
  // An expanded folder that is not loaded yet shows nothing below it.
  expect(visibleRows("rama", new Map(), new Set([ROOT])).map((row) => row.entry.path)).toEqual([
    ROOT,
  ]);
  expect(visibleRows("rama", listed, new Set()).map((row) => row.entry.path)).toEqual([ROOT]);
});

test("a_change_on_disk_recolours_and_relists_open_folders", () => {
  // What `repo-changed` reads again: the loaded folders that show, not those under a collapsed one.
  expect(openFolders(listed, new Set([ROOT, "src", "src/lib", "build/none"]))).toEqual([
    ROOT,
    "src",
    "src/lib",
  ]);
  expect(openFolders(listed, new Set([ROOT, "src/lib"]))).toEqual([ROOT]);
});

test("collapse_all_keeps_the_root_open_and_moves_the_lead_to_its_top_level_folder", () => {
  expect(collapsedAll("src/lib/b.ts")).toEqual({ expanded: new Set([ROOT]), lead: "src" });
  expect(collapsedAll("debug.log")).toEqual({ expanded: new Set([ROOT]), lead: "debug.log" });
  expect(collapsedAll(ROOT)).toEqual({ expanded: new Set([ROOT]), lead: ROOT });
  expect(collapsedAll(null)).toEqual({ expanded: new Set([ROOT]), lead: null });
  expect(parentOf("src/lib/b.ts")).toBe("src/lib");
  expect(parentOf("src")).toBe(ROOT);
  expect(parentOf(ROOT)).toBeNull();
});

test("speed_search_marks_the_matching_names", () => {
  const rows = visibleRows("rama", listed, new Set([ROOT, "src", "src/lib"]));

  expect(matchingRows(rows, "a").map((row) => row.entry.path)).toEqual([ROOT, "src/a.txt"]);
  expect(matchingRows(rows, "lib").map((row) => row.entry.path)).toEqual(["src/lib"]);
  expect(matchingRows(rows, "")).toEqual([]);
  expect(namePieces("debug.log", "log")).toEqual([
    { text: "debug.", hit: false },
    { text: "log", hit: true },
  ]);
  expect(namePieces("debug.log", "zz")).toEqual([{ text: "debug.log", hit: false }]);
});

test("the_selection_is_given_as_repository_paths_and_as_file_targets", () => {
  expect(treeSelection()).toEqual([]);
  const stop = provideTreeSelection(() => ["[ab]/-x*?.txt", "src"]);
  expect(treeSelection()).toEqual(["[ab]/-x*?.txt", "src"]);
  stop();
  expect(treeSelection()).toEqual([]);

  const index = statusIndex([
    { path: "src/a.txt", oldPath: null, state: "modified", staged: false, unstaged: true },
    { path: "build/", oldPath: null, state: "ignored", staged: false, unstaged: false },
  ]);
  const entries: TreeEntry[] = [
    { name: "rama", path: ROOT, dir: true },
    { name: "a.txt", path: "src/a.txt", dir: false, binary: false },
    { name: "out", path: "build/out", dir: true, binary: false },
    // Rust says which types are binary: the Git file menu does not annotate it.
    { name: "logo.png", path: "logo.png", dir: false, binary: true },
  ];
  expect(fileTargets(entries, index)).toEqual([
    { path: "", dir: true, state: null },
    { path: "src/a.txt", dir: false, state: "modified" },
    { path: "build/out", dir: true, state: "ignored" },
    { path: "logo.png", dir: false, state: null, binary: true },
  ]);
  expect(hasLocalChanges(index)).toBe(true);
  const untracked = statusIndex([
    { path: "new.txt", oldPath: null, state: "untracked", staged: false, unstaged: true },
  ]);
  expect(hasLocalChanges(untracked)).toBe(false);
});

test("the_limiter_runs_at_most_that_many_tasks_at_once_in_order", async () => {
  const limit = limiter(2);
  const started: number[] = [];
  const finish: (() => void)[] = [];
  let running = 0;
  let most = 0;
  const done = [0, 1, 2, 3, 4].map((index) =>
    limit(async () => {
      started.push(index);
      running++;
      most = Math.max(most, running);
      await new Promise<void>((resolve) => finish.push(resolve));
      running--;
      return index;
    }),
  );
  while (started.length < 5 || finish.length > 0) {
    await new Promise((resolve) => setTimeout(resolve, 0));
    finish.splice(0).forEach((resolve) => resolve());
  }

  expect(await Promise.all(done)).toEqual([0, 1, 2, 3, 4]);
  expect(started).toEqual([0, 1, 2, 3, 4]);
  expect(most).toBe(2);
});
