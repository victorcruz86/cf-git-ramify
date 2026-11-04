import { expect, test, vi } from "vitest";
import type { BranchList } from "../lib/api";
import type { MenuAction, MenuItem } from "../lib/menu";
import {
  autoExpanded,
  branchActions,
  buildTree,
  findBranch,
  nextLeaf,
  preferredId,
  visibleRows,
  type BranchNode,
  type TreeNode,
  type TreeOptions,
} from "./model";

const OPTIONS: TreeOptions = {
  favorites: { local: ["main", "master"], remote: ["origin/main", "origin/master"] },
  grouping: true,
  recent: true,
};

function branches(local: string[], remote: string[] = [], change: Partial<BranchList> = {}) {
  return {
    local: local.map((name) => ({ name, tracked: null })),
    remote,
    recent: [],
    current: null,
    previous: null,
    ...change,
  } satisfies BranchList;
}

// The tree as text: `>` opens a level and `<` closes it.
function shape(nodes: TreeNode[]): string[] {
  return nodes.flatMap((node) =>
    node.kind === "branch" ? [node.label] : [`${node.label}>`, ...shape(node.children), "<"],
  );
}

const section = (tree: TreeNode[], id: string) => {
  const found = tree.find((node) => node.id === id);
  if (found?.kind !== "section") throw new Error(`no section ${id}`);
  return found.children;
};

const labels = (nodes: TreeNode[]) => nodes.map((node) => node.label);
const shown = (items: MenuItem[]) =>
  items.map((item) => (item === null ? "—" : "label" in item ? item.label : item.header));
const row = (items: MenuItem[], label: string) =>
  items.find((item) => item !== null && "label" in item && item.label === label) as MenuAction;

test("order_current_favorites_no_slash_name", () => {
  const list = branches(["zeta", "alpha", "main", "feature/b", "feature/a", "beta", "my/current"], [], {
    current: "beta",
  });

  const local = section(buildTree(list, "", { ...OPTIONS, grouping: false }, ), "local");

  // Without grouping, the slash does not count.
  expect(labels(local)).toEqual([
    "beta",
    "main",
    "alpha",
    "feature/a",
    "feature/b",
    "my/current",
    "zeta",
  ]);
});

// `compareBy { !(isPrefixGrouping() && it.name.contains('/')) }`: `false` goes first, so with
// groups what contains a slash goes before what does not. The reference's summary says it the other way round.
test("with_groups_folders_come_before_loose_branches", () => {
  const list = branches(["zeta", "alpha", "main", "feature/a", "beta/x"], [], { current: "zeta" });

  const local = section(buildTree(list, "", OPTIONS), "local");

  expect(labels(local)).toEqual(["zeta", "main", "beta", "feature", "alpha"]);
});

test("inside_a_folder_favorites_then_folders_then_branches", () => {
  const list = branches(["f/z", "f/fav", "f/g/x", "f/a"]);
  const options = { ...OPTIONS, favorites: { local: ["f/fav"], remote: [] } };

  const [folder] = section(buildTree(list, "", options), "local");

  expect(folder.kind === "folder" && labels(folder.children)).toEqual(["fav", "g", "a", "z"]);
});

test("groups_by_slash", () => {
  const list = branches(["feature/a/one", "feature/b"], ["origin/x", "up/stream/y"]);

  expect(shape(buildTree(list, "", OPTIONS))).toEqual([
    "Local>",
    "feature>",
    "a>",
    "one",
    "<",
    "b",
    "<",
    "<",
    "Remote>",
    "origin>",
    "x",
    "<",
    "up>",
    "stream>",
    "y",
    "<",
    "<",
    "<",
  ]);
});

test("without_grouping_the_name_goes_whole", () => {
  const tree = buildTree(branches(["feature/a"]), "", { ...OPTIONS, grouping: false });

  expect(shape(tree)).toEqual(["Local>", "feature/a", "<"]);
});

test("recent_in_reflog_order_and_also_in_local", () => {
  const list = branches(["a", "b", "c"], [], { recent: ["c", "a"], current: "c" });

  const tree = buildTree(list, "", OPTIONS);

  expect(labels(section(tree, "recent"))).toEqual(["c", "a"]);
  expect(labels(section(tree, "local"))).toEqual(["c", "a", "b"]);
});

test("no_recent_if_the_setting_removes_them", () => {
  const list = branches(["a"], [], { recent: ["a"], current: "a" });

  const tree = buildTree(list, "", { ...OPTIONS, recent: false });

  expect(labels(tree)).toEqual(["Local"]);
});

test("local_open_remote_closed", () => {
  const list = branches(["main", "side"], ["origin/main"], { current: "main" });
  const tree = buildTree(list, "", OPTIONS);

  const open = autoExpanded(tree, list, "");

  expect(open.has("local")).toBe(true);
  expect(open.has("remote")).toBe(false);
  const rows = visibleRows(tree, open).map(({ node }) => node.label);
  expect(rows).toEqual(["Local", "main", "side", "Remote"]);
});

// `createTreePathFor` puts every local branch under Recent if that section exists: it is the one that opens.
test("with_recent_ones_recent_opens_and_local_stays_closed", () => {
  const list = branches(["main", "feature/x"], ["origin/main"], {
    current: "main",
    recent: ["main", "feature/x"],
  });
  const tree = buildTree(list, "", OPTIONS);

  const open = autoExpanded(tree, list, "");

  expect(open.has("recent")).toBe(true);
  expect(open.has("recent/feature"), "the folder of a recent one").toBe(true);
  expect(open.has("local")).toBe(false);
  expect(open.has("remote")).toBe(false);
});

test("opens_on_the_previous_branch", () => {
  const list = branches(["main", "side"], [], {
    current: "main",
    previous: "side",
    recent: ["main", "side"],
  });

  expect(preferredId(buildTree(list, "", OPTIONS), list, "")).toBe("recent:side");
});

test("without_a_previous_one_opens_on_the_current", () => {
  const list = branches(["main", "side"], [], { current: "main" });

  expect(preferredId(buildTree(list, "", OPTIONS), list, "")).toBe("local:main");
});

test("previous_outside_recent_is_looked_up_in_local", () => {
  const list = branches(["main", "side"], [], { current: "main", previous: "side" });

  expect(preferredId(buildTree(list, "", OPTIONS), list, "")).toBe("local:side");
});

test("fuzzy_search_includes_origin", () => {
  const list = branches(["main"], ["origin/feature/one", "upstream/other"]);

  const remote = section(buildTree(list, "orfe", OPTIONS), "remote");

  expect(shape(remote)).toEqual(["origin>", "feature>", "one", "<", "<"]);
});

test("the_whole_name_counts_with_the_slash", () => {
  const list = branches([], ["origin/main"]);

  expect(labels(buildTree(list, "origin/ma", OPTIONS))).toEqual(["Remote"]);
});

test("a_lone_slash_does_not_filter", () => {
  const list = branches(["main"], ["origin/main"]);

  expect(labels(buildTree(list, "/", OPTIONS))).toEqual(["Local", "Remote"]);
});

test("spaces_in_the_text_do_not_count", () => {
  const list = branches(["main", "side"]);

  expect(shape(buildTree(list, "  main ", OPTIONS))).toEqual(["Local>", "main", "<"]);
});

test("when_filtering_recent_ones_leave_local", () => {
  const list = branches(["main", "max"], [], { current: "main", recent: ["main"] });

  const tree = buildTree(list, "ma", OPTIONS);

  expect(labels(section(tree, "recent"))).toEqual(["main"]);
  expect(labels(section(tree, "local"))).toEqual(["max"]);
});

test("without_matches_no_section_remains", () => {
  expect(buildTree(branches(["main"], ["origin/main"]), "zzz", OPTIONS)).toEqual([]);
});

test("best_match_prefers_recent_then_local_then_remote", () => {
  const list = branches(["ma1", "ma2"], ["origin/ma3"], { recent: ["ma2"] });

  expect(preferredId(buildTree(list, "ma", OPTIONS), list, "ma")).toBe("recent:ma2");

  const without = { ...list, recent: [] };
  expect(preferredId(buildTree(without, "ma", OPTIONS), without, "ma")).toBe("local:ma1");

  const remote = branches([], ["origin/ma3"]);
  expect(preferredId(buildTree(remote, "ma", OPTIONS), remote, "ma")).toBe("remote:origin/ma3");
});

test("best_match_prefers_starting_earlier", () => {
  const list = branches(["ob-main", "main"]);

  expect(preferredId(buildTree(list, "main", OPTIONS), list, "main")).toBe("local:main");
});

test("filtering_opens_the_section_of_the_best_match", () => {
  const list = branches(["x"], ["origin/side"]);
  const tree = buildTree(list, "side", OPTIONS);

  expect(autoExpanded(tree, list, "side").has("remote/origin")).toBe(true);
});

test("when_filtering_arrows_jump_from_leaf_to_leaf_and_wrap_around", () => {
  const list = branches(["ma", "mb"], ["origin/mc"]);
  const tree = buildTree(list, "m", OPTIONS);

  expect(nextLeaf(tree, "local:ma", 1)?.id).toBe("local:mb");
  expect(nextLeaf(tree, "local:mb", 1)?.id).toBe("remote:origin/mc");
  expect(nextLeaf(tree, "remote:origin/mc", 1)?.id).toBe("local:ma");
  expect(nextLeaf(tree, "local:ma", -1)?.id).toBe("remote:origin/mc");
});

test("the_favorite_and_the_current_are_marked", () => {
  const list = branches(["main", "side"], ["origin/main"], { current: "side" });
  const tree = buildTree(list, "", OPTIONS);

  const main = findBranch(tree, "local:main") as BranchNode;
  const side = findBranch(tree, "local:side") as BranchNode;
  const remote = findBranch(tree, "remote:origin/main") as BranchNode;

  expect([main.favorite, main.current]).toEqual([true, false]);
  expect([side.favorite, side.current]).toEqual([false, true]);
  expect(remote.favorite).toBe(true);
});

test("the_branch_tracks_its_remote_one", () => {
  const list = branches(["main"]);
  list.local[0].tracked = "origin/main";

  const main = findBranch(buildTree(list, "", OPTIONS), "local:main") as BranchNode;

  expect(main.tracked).toBe("origin/main");
});

test("the_panel_sorts_current_favorites_folders_branches", () => {
  const list = branches(["zeta", "alpha", "main", "feature/a"], [], { current: "zeta" });

  const local = section(buildTree(list, "", { ...OPTIONS, panel: true }), "local");

  expect(labels(local)).toEqual(["zeta", "main", "feature", "alpha"]);
});

const NODE = (name: string, section: "local" | "remote"): BranchNode => ({
  kind: "branch",
  id: `${section}:${name}`,
  section,
  name,
  label: name,
  current: false,
  favorite: false,
  tracked: "origin/x",
  match: null,
});

const STATE = { current: "main", remotes: true };

test("submenu_in_order_without_phase_4_items", () => {
  expect(shown(branchActions(NODE("side", "local"), STATE))).toEqual([
    "Checkout",
    "New Branch from 'side'…",
    "Checkout and Rebase onto 'main'",
    "—",
    "Rebase 'main' onto 'side'",
    "Merge 'side' into 'main'",
    "—",
    "Update",
    "Push…",
    "—",
    "Rename…",
    "Delete",
  ]);
  expect(shown(branchActions(NODE("origin/x", "remote"), STATE))).toEqual([
    "Checkout",
    "New Branch from 'origin/x'…",
    "Checkout and Rebase onto 'main'",
    "—",
    "Rebase 'main' onto 'origin/x'",
    "Merge 'origin/x' into 'main'",
    "—",
    "Pull into 'main' Using Merge",
    "Pull into 'main' Using Rebase",
    "—",
    "Delete",
  ]);
});

test("current_branch_only_has_new_branch_update_push_rename", () => {
  expect(shown(branchActions(NODE("main", "local"), STATE))).toEqual([
    "New Branch from 'main'…",
    "—",
    "Update",
    "Push…",
    "—",
    "Rename…",
  ]);
});

test("update_is_hidden_without_remotes_and_disabled_without_tracking", () => {
  const lonely = branchActions(NODE("side", "local"), { ...STATE, remotes: false });
  expect(shown(lonely)).not.toContain("Update");

  const untracked = { ...NODE("side", "local"), tracked: null };
  expect(row(branchActions(untracked, STATE), "Update").disabled).toBe(true);
});

test("repository_patterns_decide_which_remote_cannot_be_deleted", () => {
  const onvcs = vi.fn();
  const state = { ...STATE, onvcs, protectedBranches: ["release.*"] };

  expect(row(branchActions(NODE("origin/release-1", "remote"), state), "Delete").disabled).toBe(true);
  expect(row(branchActions(NODE("origin/main", "remote"), state), "Delete").disabled).toBe(false);
});

test("without_a_handler_everything_is_disabled", () => {
  const items = branchActions(NODE("side", "local"), STATE);

  expect(row(items, "Checkout").disabled).toBe(true);
  expect(row(items, "Delete").disabled).toBe(true);
});

test("each_row_launches_its_operation", () => {
  const onvcs = vi.fn();
  const items = branchActions(NODE("origin/x", "remote"), { ...STATE, onvcs });

  row(items, "Checkout").run?.();

  expect(onvcs).toHaveBeenCalledWith({
    kind: "ref",
    operation: "checkout",
    ref: { name: "origin/x", kind: "remote" },
  });
});

test("delete_is_disabled_on_a_protected_remote", () => {
  const state = { ...STATE, onvcs: vi.fn(), protectedBranches: ["master", "main"] };
  const items = branchActions(NODE("origin/main", "remote"), state);

  expect(row(items, "Delete").disabled).toBe(true);
});

test("a_long_name_is_shortened", () => {
  const name = "x".repeat(50);

  const [first] = shown(branchActions(NODE(name, "remote"), STATE)).slice(1);

  expect(first).toBe(`New Branch from '${"x".repeat(39)}…'…`);
});
