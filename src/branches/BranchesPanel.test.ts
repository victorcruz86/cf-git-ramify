import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { get } from "svelte/store";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { BranchList } from "../lib/api";
import BranchesPanel from "./BranchesPanel.svelte";
import { whileFetching } from "../update/flow";
import { branchOptions, defaultBranchOptions, favorites } from "./state";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn() }));

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
