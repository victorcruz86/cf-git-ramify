import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { get } from "svelte/store";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { BranchList } from "../lib/api";
import BranchesPanel from "./BranchesPanel.svelte";
import { whileFetching } from "../update/flow";
import { branchOptions, defaultBranchOptions, favorites } from "./state";
import { compareBranches } from "../compare/actions";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({ label: "main" }),
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}), emitTo: vi.fn() }));
vi.mock("../compare/actions", () => ({
  compareBranches: vi.fn(async () => {}),
  showBranchesDiff: vi.fn(async () => {}),
  showDiffWithWorkingTree: vi.fn(async () => {}),
}));

const LIST: BranchList = {
  local: [
    { name: "main", tracked: "origin/main" },
    { name: "side", tracked: null },
  ],
  remote: ["origin/main"],
  recent: ["main"],
  current: "main",
  previous: null,
};

let incoming: Promise<unknown>;
let fingerprint: string;
let changed: () => void;

const calls = (command: string) =>
  vi.mocked(invoke).mock.calls.filter(([name]) => name === command);

beforeEach(() => {
  incoming = Promise.resolve({});
  fingerprint = "one";
  changed = () => {};
  vi.mocked(listen).mockImplementation(async (event, handler) => {
    if (event === "repo-changed") changed = () => handler({} as never);
    return () => {};
  });
  favorites.set({ local: ["main"], remote: [] });
  branchOptions.set(defaultBranchOptions());
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "branch_list") return LIST;
    if (command === "branch_incoming_outgoing") return incoming;
    if (command === "branch_refs_fingerprint") return fingerprint;
    if (command === "favorites_toggle") return { local: [], remote: [] };
    return undefined;
  });
});

afterEach(cleanup);

async function panel(props: Record<string, unknown> = {}) {
  const handlers = { onfilter: vi.fn(), onnavigate: vi.fn(), onhide: vi.fn(), onvcs: vi.fn() };
  render(BranchesPanel, { active: true, ...handlers, ...props });
  await screen.findByRole("treeitem", { name: "Local" });
  return handlers;
}

const row = (name: string) => screen.getAllByRole("treeitem", { name })[0];
const rows = () => screen.getAllByRole("treeitem").map((item) => item.getAttribute("aria-label"));
const button = (name: string) => screen.getByRole("button", { name }) as HTMLButtonElement;

async function chooseSetting(name: string) {
  await fireEvent.click(button("Branches Pane Settings"));
  await fireEvent.click(screen.getByRole("menuitem", { name }));
}

test("shows_head_local_and_remote_without_recent", async () => {
  await panel();

  expect(rows().slice(0, 2)).toEqual(["HEAD (Current Branch)", "Local"]);
  expect(rows()).toContain("Remote");
  expect(rows()).not.toContain("Recent");
});

test("by_default_everything_open", async () => {
  await panel();

  expect(rows()).toEqual([
    "HEAD (Current Branch)",
    "Local",
    "main",
    "side",
    "Remote",
    "origin",
    "main",
  ]);
});

test("panel_double_click_changes_the_branch_filter", async () => {
  const { onfilter } = await panel();

  await fireEvent.click(row("side"));
  expect(onfilter).not.toHaveBeenCalled();

  await fireEvent.dblClick(row("side"));

  expect(onfilter).toHaveBeenCalledWith(["side"]);
});

test("enter_changes_the_filter_and_head_counts", async () => {
  const { onfilter } = await panel();
  await fireEvent.click(row("HEAD (Current Branch)"));

  await fireEvent.keyDown(screen.getByRole("tree"), { key: "Enter" });

  expect(onfilter).toHaveBeenCalledWith(["HEAD"]);
});

test("double_click_on_a_section_does_not_filter", async () => {
  const { onfilter } = await panel();

  await fireEvent.dblClick(row("Remote"));

  expect(onfilter).not.toHaveBeenCalled();
});

test("update_branch_filter_and_navigate_are_mutually_exclusive", async () => {
  const { onfilter, onnavigate } = await panel();

  await chooseSetting("Update Branch Filter");
  expect(get(branchOptions)).toMatchObject({ filterOnSelect: true, navigateOnSelect: false });
  await fireEvent.click(row("side"));
  expect(onfilter).toHaveBeenCalledWith(["side"]);

  await chooseSetting("Navigate Log to Branch Head");
  expect(get(branchOptions)).toMatchObject({ filterOnSelect: false, navigateOnSelect: true });
  onfilter.mockClear();
  await fireEvent.click(row("main"));
  expect(onnavigate).toHaveBeenCalledWith("main");
  expect(onfilter).not.toHaveBeenCalled();
});

test("cmd_click_selects_several_and_the_filter_carries_them", async () => {
  const { onfilter } = await panel();
  await fireEvent.click(row("side"));
  await fireEvent.click(row("main"), { metaKey: true });

  await fireEvent.keyDown(screen.getByRole("tree"), { key: "Enter" });

  expect(onfilter).toHaveBeenCalledWith(["main", "side"]);
});

test("arrows_traverse_and_collapse", async () => {
  await panel();
  await fireEvent.click(row("Local"));
  const tree = screen.getByRole("tree");

  await fireEvent.keyDown(tree, { key: "ArrowDown" });
  expect(screen.getAllByRole("treeitem").find((item) => item.ariaSelected === "true")).toBe(
    row("main"),
  );

  await fireEvent.keyDown(tree, { key: "ArrowLeft" });
  await fireEvent.keyDown(tree, { key: "ArrowLeft" });
  expect(rows()).toEqual(["HEAD (Current Branch)", "Local", "Remote", "origin", "main"]);
});

test("click_on_a_section_selects_it_without_collapsing_it", async () => {
  await panel();

  await fireEvent.click(row("Local"));

  expect(rows()).toContain("side");
  expect(row("Local").getAttribute("aria-selected")).toBe("true");
});

test("double_click_and_row_arrow_collapse", async () => {
  await panel();

  await fireEvent.dblClick(row("Local"));
  expect(rows()).not.toContain("side");

  await fireEvent.click(row("Local").querySelector(".chevron")!);
  expect(rows()).toContain("side");
  expect(row("Local").getAttribute("aria-selected")).toBe("false");
});

test("delete_is_disabled_on_a_protected_remote_and_on_the_current", async () => {
  await panel();
  const remote = screen.getAllByRole("treeitem", { name: "main" })[1];

  await fireEvent.click(remote);
  expect(button("Delete branch").disabled).toBe(true);

  await fireEvent.click(row("main"));
  expect(button("Delete branch").disabled).toBe(true);

  await fireEvent.click(row("side"));
  expect(button("Delete branch").disabled).toBe(false);
});

test("update_selected_only_with_a_local_branch_that_tracks_another", async () => {
  const { onvcs } = await panel();

  await fireEvent.click(row("side"));
  expect(button("Update Selected").disabled).toBe(true);

  await fireEvent.click(row("main"));
  expect(button("Update Selected").disabled).toBe(false);
  await fireEvent.click(button("Update Selected"));
  expect(onvcs).toHaveBeenCalledWith({
    kind: "ref",
    operation: "update",
    ref: { name: "main", kind: "local" },
  });
});

test("the_panel_fetch_does_not_ask_the_remotes", async () => {
  const onfetch = vi.fn(async () => {});
  await panel({ onfetch });

  await fireEvent.click(button("Fetch All Remotes"));

  await waitFor(() => expect(onfetch).toHaveBeenCalled());
  await new Promise((resolve) => setTimeout(resolve));
  expect(calls("branch_check_remotes")).toEqual([]);
});

// 241: `FetchAction : GitFetch()` is disabled while any fetch runs (an Update, a Pull, the menu Fetch).
test("the_panel_fetch_is_disabled_while_another_fetch_runs", async () => {
  const onfetch = vi.fn(async () => {});
  await panel({ onfetch });
  expect(button("Fetch All Remotes").disabled).toBe(false);

  let finish = () => {};
  const running = whileFetching(() => new Promise<void>((resolve) => (finish = resolve)));
  await waitFor(() => expect(button("Fetch All Remotes").disabled).toBe(true));

  finish();
  await running;
  await waitFor(() => expect(button("Fetch All Remotes").disabled).toBe(false));
});

test("the_list_comes_out_without_waiting_for_incoming_and_outgoing", async () => {
  incoming = new Promise(() => {});

  await panel();

  expect(rows()).toContain("side");
});

test("changes_without_references_do_not_read_again", async () => {
  await panel();
  await waitFor(() => expect(calls("branch_list")).toHaveLength(1));

  changed();
  await waitFor(() => expect(calls("branch_refs_fingerprint")).toHaveLength(2));
  await new Promise((resolve) => setTimeout(resolve));
  expect(calls("branch_list")).toHaveLength(1);

  fingerprint = "other";
  changed();
  await waitFor(() => expect(calls("branch_list")).toHaveLength(2));
});

test("navigate_requires_a_single_branch", async () => {
  const { onnavigate } = await panel();
  expect(button("Navigate Log to Selected Branch Head").disabled).toBe(true);

  await fireEvent.click(row("side"));
  await fireEvent.click(button("Navigate Log to Selected Branch Head"));

  expect(onnavigate).toHaveBeenCalledWith("side");
});

const tree = () => screen.getByRole("tree", { name: "Branches" });

test("f2_and_shift_f6_rename_the_chosen_local_branch", async () => {
  const { onvcs } = await panel();
  await fireEvent.click(row("side"));

  await fireEvent.keyDown(tree(), { key: "F2", code: "F2" });
  await fireEvent.keyDown(tree(), { key: "F6", code: "F6", shiftKey: true });

  const action = { kind: "ref", operation: "rename", ref: { name: "side", kind: "local" } };
  expect(onvcs.mock.calls).toEqual([[action], [action]]);
});

test("f2_does_not_rename_a_remote_nor_without_a_single_branch", async () => {
  const { onvcs } = await panel();

  await fireEvent.keyDown(tree(), { key: "F2", code: "F2" });
  await fireEvent.click(screen.getAllByRole("treeitem", { name: "main" })[1]);
  await fireEvent.keyDown(tree(), { key: "F2", code: "F2" });
  await fireEvent.click(row("main"));
  await fireEvent.click(row("side"), { metaKey: true });
  await fireEvent.keyDown(tree(), { key: "F2", code: "F2" });

  expect(onvcs).not.toHaveBeenCalled();
});

test("the_shortcut_does_not_reach_whoever_is_outside_the_panel", async () => {
  const outside = vi.fn();
  window.addEventListener("keydown", outside);
  await panel();
  await fireEvent.click(row("side"));

  await fireEvent.keyDown(tree(), { key: "˜", code: "KeyN", altKey: true, metaKey: true });
  window.removeEventListener("keydown", outside);

  expect(outside).not.toHaveBeenCalled();
});

test("option_f1_navigates_to_the_chosen_branch", async () => {
  const { onnavigate } = await panel();

  await fireEvent.keyDown(tree(), { key: "F1", code: "F1", altKey: true });
  expect(onnavigate).not.toHaveBeenCalled();

  await fireEvent.click(row("side"));
  await fireEvent.keyDown(tree(), { key: "F1", code: "F1", altKey: true });
  expect(onnavigate).toHaveBeenCalledWith("side");
});

test("cmd_option_n_opens_new_branch_from_the_chosen_branch", async () => {
  const { onvcs } = await panel();

  await fireEvent.keyDown(tree(), { key: "˜", code: "KeyN", altKey: true, metaKey: true });
  expect(onvcs).not.toHaveBeenCalled();

  await fireEvent.click(screen.getAllByRole("treeitem", { name: "main" })[1]);
  await fireEvent.keyDown(tree(), { key: "˜", code: "KeyN", altKey: true, metaKey: true });
  expect(onvcs).toHaveBeenCalledWith({
    kind: "ref",
    operation: "newBranch",
    ref: { name: "origin/main", kind: "remote" },
  });
});

test("panel_shortcuts_work_with_focus_in_the_search", async () => {
  const { onvcs } = await panel();
  await fireEvent.click(row("side"));

  await fireEvent.keyDown(screen.getByRole("textbox", { name: "Search" }), { key: "F2", code: "F2" });

  expect(onvcs).toHaveBeenCalledTimes(1);
});

test("hide_and_favorite_from_the_bar", async () => {
  const { onhide } = await panel();
  await fireEvent.click(button("Hide Git Branches"));
  expect(onhide).toHaveBeenCalled();

  await fireEvent.click(row("side"));
  await fireEvent.click(button("Mark/Unmark As Favorite"));

  await waitFor(() => expect(calls("favorites_toggle")).toHaveLength(1));
  expect(calls("favorites_toggle")[0][1]).toEqual({ name: "side", kind: "local" });
});

test("context_menu_with_the_branch_actions", async () => {
  const { onvcs } = await panel();

  await fireEvent.contextMenu(row("side"));
  await fireEvent.click(await screen.findByRole("menuitem", { name: "Delete" }));

  await waitFor(() =>
    expect(onvcs).toHaveBeenCalledWith({
      kind: "ref",
      operation: "delete",
      ref: { name: "side", kind: "local" },
    }),
  );
});

// `MultipleLocalBranchActions` (`BranchesDashboardActions.kt:68-71`): with several branches chosen, a
// right click inside the selection keeps it and shows their menu. Compare Branches and Show Files
// Not in IntelliJ 2024.1 (`docs/parity-matrix.md`): the same row as in the popup.
test("the_context_menu_copies_the_full_branch_name", async () => {
  await panel();
  const remote = screen.getAllByRole("treeitem", { name: "main" })[1];

  await fireEvent.contextMenu(remote);
  const menu = await screen.findByRole("menu", { name: "origin/main" });
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Copy Branch Name" }));

  await waitFor(() => expect(calls("clipboard_write")).toEqual([["clipboard_write", { text: "origin/main" }]]));
});

// Diff exist only with exactly two (`BranchesPairActionBase`), the first against the second in
// the order of the tree.
test("compare_branches_needs_two_selected", async () => {
  const { onvcs } = await panel();
  const remote = screen.getAllByRole("treeitem", { name: "main" })[1];
  await fireEvent.click(row("side"));
  await fireEvent.click(remote, { metaKey: true });

  await fireEvent.contextMenu(remote);
  let menu = await screen.findByRole("menu", { name: "Branches" });
  expect(within(menu).getAllByRole("menuitem").map((item) => item.getAttribute("aria-label"))).toEqual([
    "Compare Branches",
    "Show Files Diff",
    "Update Selected",
    "Delete branches",
  ]);
  expect(row("side").getAttribute("aria-selected")).toBe("true");
  expect(remote.getAttribute("aria-selected")).toBe("true");
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Compare Branches" }));
  expect(compareBranches).toHaveBeenCalledWith("side", "origin/main");

  await fireEvent.click(row("main"), { metaKey: true });
  await fireEvent.contextMenu(row("side"));
  menu = await screen.findByRole("menu", { name: "Branches" });
  expect(within(menu).getAllByRole("menuitem").map((item) => item.getAttribute("aria-label"))).toEqual([
    "Update Selected",
    "Delete branches",
  ]);
  // The current branch cannot be deleted; `main` tracks `origin/main`, so Update Selected is on.
  expect((within(menu).getByRole("menuitem", { name: "Delete branches" }) as HTMLButtonElement).disabled).toBe(true);
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Update Selected" }));
  // `updateBranches`: those that track a branch, at once.
  await waitFor(() => expect(onvcs).toHaveBeenCalledWith({ kind: "updateBranches", names: ["main"] }));

  // Outside the selection, a right click chooses only that row and shows its own menu.
  await fireEvent.click(row("side"));
  await fireEvent.contextMenu(remote);
  expect(await screen.findByRole("menuitem", { name: "Checkout" })).toBeTruthy();
});

// `DeleteBranchAction.delete` (`BranchesDashboardActions.kt:303-321`): the local branches one by one,
// then every remote one with a single question (`GitDeleteRemoteBranchOperation`).
test("deleting_two_remote_branches_asks_once", async () => {
  const list: BranchList = { ...LIST, remote: ["origin/a", "origin/b"] };
  const base = vi.mocked(invoke).getMockImplementation()!;
  vi.mocked(invoke).mockImplementation(async (command, args) =>
    command === "branch_list" ? list : base(command, args),
  );
  const { onvcs } = await panel();
  const first = row("a");
  const second = row("b");
  await fireEvent.click(row("side"));
  await fireEvent.click(first, { metaKey: true });
  await fireEvent.click(second, { metaKey: true });

  await fireEvent.contextMenu(second);
  const menu = await screen.findByRole("menu", { name: "Branches" });
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Delete branches" }));

  await waitFor(() =>
    expect(onvcs).toHaveBeenCalledWith({ kind: "deleteRemotes", names: ["origin/a", "origin/b"] }),
  );
  expect(vi.mocked(onvcs).mock.calls).toEqual([
    [{ kind: "ref", operation: "delete", ref: { name: "side", kind: "local" } }],
    [{ kind: "deleteRemotes", names: ["origin/a", "origin/b"] }],
  ]);
});

// `BranchActionsBuilder.build`: HEAD is not one of the chosen branches. With two more it is still
// the menu of several; with one more, the current branch's menu.
test("head_does_not_count_for_the_menu_of_several", async () => {
  await panel();
  await fireEvent.click(screen.getByRole("treeitem", { name: "HEAD (Current Branch)" }));
  await fireEvent.click(row("side"), { metaKey: true });

  await fireEvent.contextMenu(row("side"));
  let menu = await screen.findByRole("menu", { name: "main" });
  expect(within(menu).queryByRole("menuitem", { name: "Checkout" })).toBeNull();
  expect(within(menu).getByRole("menuitem", { name: "Rename…" })).toBeTruthy();
  await fireEvent.keyDown(menu, { key: "Escape" });

  await fireEvent.click(screen.getAllByRole("treeitem", { name: "main" })[1], { metaKey: true });
  await fireEvent.contextMenu(row("side"));
  menu = await screen.findByRole("menu", { name: "Branches" });
  expect(within(menu).getByRole("menuitem", { name: "Compare Branches" })).toBeTruthy();
});

// `CurrentBranchActions` (`BranchesDashboardActions.kt:165-167`) comes from the repository's current
// branch, not from a visible row: on HEAD alone, and with HEAD and one branch while its group is
// folded.
test("the_current_branch_menu_does_not_need_its_row", async () => {
  await panel();
  await fireEvent.contextMenu(screen.getByRole("treeitem", { name: "HEAD (Current Branch)" }));
  let menu = await screen.findByRole("menu", { name: "main" });
  expect(within(menu).getByRole("menuitem", { name: "Rename…" })).toBeTruthy();
  await fireEvent.keyDown(menu, { key: "Escape" });

  await fireEvent.click(row("side"));
  await fireEvent.click(screen.getByRole("treeitem", { name: "HEAD (Current Branch)" }), { metaKey: true });
  // Folding Local leaves no visible row of `main`.
  await fireEvent.dblClick(row("Local"));
  expect(screen.queryAllByRole("treeitem", { name: "main" })).toHaveLength(1);
  await fireEvent.contextMenu(screen.getByRole("treeitem", { name: "HEAD (Current Branch)" }));
  menu = await screen.findByRole("menu", { name: "main" });
  expect(within(menu).getByRole("menuitem", { name: "Push…" })).toBeTruthy();
});

// `disableActionIfAnyRepositoryIsFresh`: before the first commit HEAD has no hash, and a fetched
// remote branch's comparisons are off.
test("before_the_first_commit_the_comparisons_are_off", async () => {
  const base = vi.mocked(invoke).getMockImplementation()!;
  vi.mocked(invoke).mockImplementation(async (command, args) =>
    command === "repo_state"
      ? { operation: null, branch: "main", label: "main", head: "" }
      : base(command, args),
  );
  await panel();

  await fireEvent.contextMenu(screen.getAllByRole("treeitem", { name: "main" })[1]);
  const menu = await screen.findByRole("menu");
  for (const name of ["Compare with 'main'", "Show Diff with Working Tree"]) {
    expect((within(menu).getByRole("menuitem", { name }) as HTMLButtonElement).disabled, name).toBe(true);
  }
});

// `ShowBranchDiffAction` (`B:604`): the toolbar's diff button and ⌘D compare each chosen branch
// that is not the current one with the current one; with only the current one it is off.
test("compare_with_current_compares_the_chosen_branches_with_the_current_one", async () => {
  await panel();
  expect(button("Compare with Current").disabled).toBe(true);
  await fireEvent.click(row("main"));
  expect(button("Compare with Current").disabled).toBe(true);

  await fireEvent.click(row("side"), { metaKey: true });
  await fireEvent.click(button("Compare with Current"));
  expect(compareBranches).toHaveBeenLastCalledWith("side", "main");
  expect(compareBranches).toHaveBeenCalledTimes(1);

  vi.mocked(compareBranches).mockClear();
  await fireEvent.keyDown(screen.getByRole("tree"), { key: "d", metaKey: true });
  expect(compareBranches).toHaveBeenCalledWith("side", "main");
});

test("searching_filters_and_opens_everything", async () => {
  await panel();

  await fireEvent.input(screen.getByRole("textbox", { name: "Search" }), {
    target: { value: "orma" },
  });

  await waitFor(() => expect(rows()).toEqual(["Remote", "origin", "main"]));
});

test("expand_and_collapse_all", async () => {
  await panel();

  await fireEvent.click(button("Expand All"));
  expect(rows()).toContain("origin");

  await fireEvent.click(button("Collapse All"));
  expect(rows()).toEqual(["HEAD (Current Branch)", "Local", "Remote"]);
});

test("hidden_does_not_read_until_it_is_shown", async () => {
  const { rerender } = render(BranchesPanel, {
    active: false,
    onfilter: vi.fn(),
    onnavigate: vi.fn(),
    onhide: vi.fn(),
  });
  expect(calls("branch_list")).toEqual([]);

  await rerender({ active: true });

  await waitFor(() => expect(calls("branch_list")).toHaveLength(1));
});
