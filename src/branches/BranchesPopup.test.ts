import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { get } from "svelte/store";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { BranchList } from "../lib/api";
import BranchesPopup from "./BranchesPopup.svelte";
import { whileFetching } from "../update/flow";
import { ongoingActions } from "../operation/operation";
import { branchOptions, defaultBranchOptions, favorites } from "./state";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({ label: "main" }),
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}), emitTo: vi.fn() }));

const LIST: BranchList = {
  local: [
    { name: "main", tracked: "origin/main" },
    { name: "side", tracked: null },
    { name: "feature/one", tracked: null },
  ],
  remote: ["origin/main", "origin/other"],
  recent: [],
  current: "main",
  previous: "side",
};

let list: BranchList;
let sync: Record<string, { incoming: boolean; outgoing: boolean }>;
let incoming: Promise<unknown> | null;
let fingerprint: string;
let changed: () => void;
let synced: () => void;

const calls = (command: string) =>
  vi.mocked(invoke).mock.calls.filter(([name]) => name === command);

beforeEach(() => {
  list = LIST;
  sync = {};
  incoming = null;
  fingerprint = "one";
  changed = () => {};
  synced = () => {};
  vi.mocked(listen).mockImplementation(async (event, handler) => {
    if (event === "repo-changed") changed = () => handler({} as never);
    if (event === "branch-sync-changed") synced = () => handler({} as never);
    return () => {};
  });
  favorites.set({ local: ["main", "master"], remote: ["origin/main", "origin/master"] });
  branchOptions.set(defaultBranchOptions());
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "branch_list") return list;
    if (command === "branch_incoming_outgoing") return incoming ?? sync;
    if (command === "branch_refs_fingerprint") return fingerprint;
    if (command === "favorites_toggle") {
      return { local: [(args as { name: string }).name], remote: [] };
    }
    return undefined;
  });
});

afterEach(cleanup);

async function popup(props: Record<string, unknown> = {}) {
  const onclose = vi.fn();
  render(BranchesPopup, { onclose, ...props });
  await screen.findByRole("treeitem", { name: "Local" });
  await waitFor(() => expect(selectedRow()).toBeTruthy());
  return { onclose };
}

const rows = () => screen.getAllByRole("treeitem").map((row) => row.getAttribute("aria-label"));
const selectedRow = () =>
  screen
    .getAllByRole("treeitem")
    .find((row) => row.getAttribute("aria-selected") === "true")
    ?.getAttribute("aria-label");
const field = () => screen.getByRole("textbox", { name: "Search" });
const press = (key: string) => fireEvent.keyDown(field(), { key });
const row = (name: string) => screen.getByRole("treeitem", { name });

async function type(text: string) {
  await fireEvent.input(field(), { target: { value: text } });
}

test("top_rows_update_commit_push_new_branch_checkout_tag", async () => {
  await popup();

  expect(rows().slice(0, 5)).toEqual([
    "Update Project",
    "Commit…",
    "Push…",
    "New Branch…",
    "Checkout Tag or Revision…",
  ]);
  expect(screen.getAllByRole("separator")).toHaveLength(2);
  expect(within(row("Commit…")).getByText("⌘K")).toBeTruthy();
  expect(within(row("Push…")).getByText("⇧⌘K")).toBeTruthy();
  expect(within(row("Update Project")).getByText("⌘T")).toBeTruthy();
});

test("update_project_closes_the_popup_and_says_whether_shift_was_held", async () => {
  const onupdateproject = vi.fn();
  await popup({ onupdateproject });

  await fireEvent.click(row("Update Project"), { shiftKey: true });

  await waitFor(() => expect(onupdateproject).toHaveBeenCalledWith({ shift: true }));
});

test("update_project_without_shift", async () => {
  const onupdateproject = vi.fn();
  await popup({ onupdateproject });

  await fireEvent.click(row("Update Project"));

  await waitFor(() => expect(onupdateproject).toHaveBeenCalledWith({ shift: false }));
});

test("cmd_t_inside_the_popup_launches_update_project_like_cmd_k_does_commit", async () => {
  const onupdateproject = vi.fn();
  const { onclose } = await popup({ onupdateproject });

  await fireEvent.keyDown(field(), { key: "t", code: "KeyT", metaKey: true });

  await waitFor(() => expect(onupdateproject).toHaveBeenCalledWith({ shift: false }));
  expect(onclose).toHaveBeenCalled();
});

test("the_dots_say_update_project_is_going_to_ask", async () => {
  await popup({ onupdateproject: vi.fn(), updatedialog: true });

  expect(row("Update Project…")).toBeTruthy();
});

test("cmd_shift_k_in_the_popup_is_push", async () => {
  const onpush = vi.fn();
  const { onclose } = await popup({ onpush });

  await fireEvent.keyDown(field(), { key: "k", code: "KeyK", metaKey: true, shiftKey: true });

  expect(onclose).toHaveBeenCalled();
  await waitFor(() => expect(onpush).toHaveBeenCalled());
});

test("without_a_handler_the_top_rows_are_disabled", async () => {
  const oncommit = vi.fn();
  const { onclose } = await popup({ oncommit });

  expect(row("Update Project").getAttribute("aria-disabled")).toBe("true");
  expect(row("Push…").getAttribute("aria-disabled")).toBe("true");
  expect(row("Commit…").getAttribute("aria-disabled")).toBe("false");
  await fireEvent.click(row("Update Project"));
  expect(onclose).not.toHaveBeenCalled();

  await fireEvent.click(row("Commit…"));

  expect(onclose).toHaveBeenCalled();
  await waitFor(() => expect(oncommit).toHaveBeenCalled());
});

test("the_commit_shortcut_works_inside_the_popup", async () => {
  const oncommit = vi.fn();
  const { onclose } = await popup({ oncommit });

  await fireEvent.keyDown(field(), { key: "k", code: "KeyK", metaKey: true });

  expect(onclose).toHaveBeenCalled();
  await waitFor(() => expect(oncommit).toHaveBeenCalled());
});

test("if_the_selected_branch_disappears_the_selection_goes_back_to_the_preferred_one", async () => {
  await popup();
  expect(selectedRow()).toBe("side");

  list = { ...LIST, local: LIST.local.slice(0, 1), previous: null };
  fingerprint = "other";
  changed();

  await waitFor(() => expect(selectedRow()).toBe("main"));
});

test("fetch_is_disabled_without_a_handler", async () => {
  await popup();

  const fetch = screen.getByRole("button", { name: "Fetch" }) as HTMLButtonElement;
  expect(fetch.disabled).toBe(true);
});

// `onFetchFinished` is `forceUpdateBranches`, with `updateBranchesWithIncoming(false)`: it looks at
// the local data again and asks no one.
test("after_a_popup_fetch_the_local_data_is_read_again_without_asking_the_remotes", async () => {
  const onfetch = vi.fn(async () => {});
  await popup({ onfetch });
  const before = calls("branch_incoming_outgoing").length;

  await fireEvent.click(screen.getByRole("button", { name: "Fetch" }));

  await waitFor(() => expect(calls("branch_incoming_outgoing").length).toBeGreaterThan(before));
  expect(onfetch).toHaveBeenCalledTimes(1);
  expect(calls("branch_check_remotes")).toEqual([]);
});

// Decision 5: the button reads the flag shared by the whole app, not its own.
test("the_popup_fetch_is_disabled_while_another_fetch_runs", async () => {
  const onfetch = vi.fn(async () => {});
  await popup({ onfetch });
  const button = () => screen.getByRole("button", { name: /^Fetch/ }) as HTMLButtonElement;
  expect(button().disabled).toBe(false);

  let finish = () => {};
  const running = whileFetching(() => new Promise<void>((resolve) => (finish = resolve)));
  await waitFor(() => expect(button().disabled).toBe(true));
  await fireEvent.click(button());
  expect(onfetch).not.toHaveBeenCalled();

  finish();
  await running;
  await waitFor(() => expect(button().disabled).toBe(false));
});

test("opens_without_waiting_for_incoming_and_outgoing_and_fills_them_in_afterwards", async () => {
  let finish = (_: unknown) => {};
  incoming = new Promise((resolve) => (finish = resolve));
  await popup();

  expect(rows()).toContain("side");
  expect(row("side").title).toBe("");

  finish({ side: { incoming: true, outgoing: false } });

  await waitFor(() => expect(row("side").title).toBe("There are incoming commits"));
});

test("changes_without_references_do_not_read_again", async () => {
  await popup();
  expect(calls("branch_list")).toHaveLength(1);

  changed();
  await waitFor(() => expect(calls("branch_refs_fingerprint")).toHaveLength(2));
  await new Promise((resolve) => setTimeout(resolve));
  expect(calls("branch_list")).toHaveLength(1);

  fingerprint = "other";
  changed();
  await waitFor(() => expect(calls("branch_list")).toHaveLength(2));
});

test("the_remotes_notice_only_rereads_incoming_and_outgoing", async () => {
  await popup();
  const before = calls("branch_incoming_outgoing").length;

  synced();

  await waitFor(() => expect(calls("branch_incoming_outgoing").length).toBeGreaterThan(before));
  expect(calls("branch_list")).toHaveLength(1);
});

test("opens_on_the_previous_one_and_the_selection_is_visible", async () => {
  await popup();

  expect(selectedRow()).toBe("side");
  expect(rows()).toContain("Local");
  expect(rows()).toContain("Remote");
  expect(rows()).not.toContain("origin");
});

test("enter_opens_the_submenu_and_does_not_checkout", async () => {
  const onvcs = vi.fn();
  const { onclose } = await popup({ onvcs });

  await press("Enter");

  const menu = await screen.findByRole("menu", { name: "side" });
  expect(within(menu).getByRole("menuitem", { name: "Checkout" })).toBeTruthy();
  expect(onvcs).not.toHaveBeenCalled();
  expect(onclose).not.toHaveBeenCalled();
});

test("right_arrow_and_click_also_open_the_submenu", async () => {
  await popup();

  await press("ArrowRight");
  expect(await screen.findByRole("menu", { name: "side" })).toBeTruthy();

  await fireEvent.keyDown(screen.getByRole("menu", { name: "side" }), { key: "ArrowLeft" });
  expect(screen.queryByRole("menu", { name: "side" })).toBeNull();

  await fireEvent.click(row("main"));
  expect(await screen.findByRole("menu", { name: "main" })).toBeTruthy();
});

test("choosing_in_the_submenu_closes_and_launches_the_operation", async () => {
  const onvcs = vi.fn();
  const { onclose } = await popup({ onvcs });
  await press("Enter");

  await fireEvent.click(await screen.findByRole("menuitem", { name: "Checkout" }));

  expect(onclose).toHaveBeenCalled();
  await waitFor(() =>
    expect(onvcs).toHaveBeenCalledWith({
      kind: "ref",
      operation: "checkout",
      ref: { name: "side", kind: "local" },
    }),
  );
});

test("space_toggles_favorite", async () => {
  await popup();

  const event = new KeyboardEvent("keydown", { key: " ", bubbles: true, cancelable: true });
  field().dispatchEvent(event);

  expect(event.defaultPrevented, "the space is not typed").toBe(true);
  await waitFor(() => expect(calls("favorites_toggle")).toHaveLength(1));
  expect(calls("favorites_toggle")[0][1]).toEqual({ name: "side", kind: "local" });
  await waitFor(() => expect(get(favorites).local).toEqual(["side"]));
});

test("clicking_the_star_toggles_and_does_not_open_the_submenu", async () => {
  await popup();

  await fireEvent.click(within(row("main")).getByRole("button", { name: "Mark/Unmark As Favorite" }));

  await waitFor(() => expect(calls("favorites_toggle")).toHaveLength(1));
  expect(screen.queryByRole("menu")).toBeNull();
});

test("icon_and_tooltip_of_incoming_and_outgoing", async () => {
  sync = {
    main: { incoming: true, outgoing: true },
    side: { incoming: true, outgoing: false },
    "feature/one": { incoming: false, outgoing: true },
  };
  await popup({});
  await press("ArrowLeft");
  await fireEvent.click(row("feature"));

  await waitFor(() => expect(row("main").title).toBe("There are incoming and outgoing commits"));
  expect(row("side").title).toBe("There are incoming commits");
  expect(row("one").title).toBe("There are outgoing commits");
  expect(row("main").querySelectorAll(".sync .icon")).toHaveLength(2);
  expect(row("side").querySelectorAll(".sync .icon")).toHaveLength(1);
});

test("the_tracked_branch_shows_on_the_right", async () => {
  await popup();

  expect(within(row("main")).getByText("origin/main")).toBeTruthy();
});

test("without_results_says_not_found", async () => {
  await popup();

  await type("zzz");

  expect(await screen.findByText("'zzz' not found")).toBeTruthy();
  expect(screen.queryAllByRole("treeitem")).toEqual([]);
});

test("without_results_and_the_toggle_off_says_branch_not_found", async () => {
  branchOptions.set({ ...defaultBranchOptions(), filterByAction: false });
  await popup();

  await type("zzz");

  expect(await screen.findByText("Branch 'zzz' not found")).toBeTruthy();
});

test("the_placeholder_says_branches_and_actions", async () => {
  await popup();
  expect(field().getAttribute("placeholder")).toBe("Search for branches and actions");

  cleanup();
  branchOptions.set({ ...defaultBranchOptions(), filterByAction: false });
  await popup();
  expect(field().getAttribute("placeholder")).toBe("Search for branches");
});

test("the_gear_starts_with_show_actions_in_search_results", async () => {
  await popup();
  await fireEvent.click(screen.getByRole("button", { name: "Settings" }));

  const items = within(screen.getByRole("menu", { name: "Settings" })).getAllByRole("menuitem");
  expect(items.map((item) => item.getAttribute("aria-label") ?? item.textContent)).toEqual([
    "Show Actions in Search Results",
    "Group by Prefix",
    "Show Recent Branches",
    "Restore Popup Size",
  ]);
  const toggle = screen.getByRole("menuitem", { name: "Show Actions in Search Results" });
  expect(within(toggle).getByText("✓")).toBeTruthy();
  expect(
    within(screen.getByRole("menu", { name: "Settings" })).getAllByRole("separator"),
  ).toHaveLength(1);

  await fireEvent.click(toggle);

  expect(get(branchOptions).filterByAction).toBe(false);
  expect(calls("branch_options_set")).toHaveLength(1);
  expect((calls("branch_options_set")[0][1] as { options: { filterByAction: boolean } }).options)
    .toMatchObject({ filterByAction: false });
  await waitFor(() => expect(field().getAttribute("placeholder")).toBe("Search for branches"));
});

test("typing_filters_the_actions_too", async () => {
  list = { ...LIST, local: [...LIST.local, { name: "pushy", tracked: null }] };
  await popup();

  await type("pus");

  await waitFor(() => expect(rows()).toEqual(["Push…", "Local", "pushy"]));
  const hits = [...row("Push…").querySelectorAll(".hit")].map((hit) => hit.textContent);
  expect(hits).toEqual(["Pus"]);
});

test("a_separator_sits_between_actions_and_branches_only_when_both_exist", async () => {
  list = { ...LIST, local: [...LIST.local, { name: "pushy", tracked: null }] };
  await popup();

  await type("pus");
  await waitFor(() => expect(rows()).toEqual(["Push…", "Local", "pushy"]));
  expect(screen.getAllByRole("separator")).toHaveLength(1);

  await type("Checkout");
  await waitFor(() => expect(rows()).toEqual(["Checkout Tag or Revision…"]));
  expect(screen.queryAllByRole("separator")).toHaveLength(0);
  expect(screen.queryByText(/not found/)).toBeNull();
});

test("a_matching_action_is_selected_before_a_matching_branch", async () => {
  list = { ...LIST, local: [...LIST.local, { name: "push-fix", tracked: null }] };
  const onpush = vi.fn();
  const { onclose } = await popup({ onpush });

  await type("push");

  await waitFor(() => expect(selectedRow()).toBe("Push…"));
  await press("ArrowDown");
  expect(selectedRow()).toBe("push-fix");
  await press("ArrowDown");
  expect(selectedRow()).toBe("Push…");
  await press("ArrowUp");
  expect(selectedRow()).toBe("push-fix");
  await press("Home");
  expect(selectedRow()).toBe("Push…");
  await press("End");
  expect(selectedRow()).toBe("push-fix");
  await press("Home");
  await press("Enter");
  expect(onclose).toHaveBeenCalled();
  await waitFor(() => expect(onpush).toHaveBeenCalled());
});

test("with_the_toggle_off_actions_stay_and_are_skipped", async () => {
  branchOptions.set({ ...defaultBranchOptions(), filterByAction: false });
  list = { ...LIST, local: [...LIST.local, { name: "push-fix", tracked: null }] };
  await popup();

  await type("push");

  await waitFor(() => expect(selectedRow()).toBe("push-fix"));
  expect(rows().slice(0, 5)).toEqual([
    "Update Project",
    "Commit…",
    "Push…",
    "New Branch…",
    "Checkout Tag or Revision…",
  ]);
  expect(row("Push…").querySelector(".hit")).toBeNull();
  await press("ArrowDown");
  expect(selectedRow()).toBe("push-fix");
});

test("the_minimum_width_is_375", async () => {
  await popup();

  expect((document.querySelector(".popup") as HTMLElement).style.minWidth).toBe("375px");
});

test("filtering_selects_the_best_and_enter_opens_its_submenu", async () => {
  await popup();

  await type("orot");

  await waitFor(() => expect(selectedRow()).toBe("other"));
  await press("Enter");
  expect(await screen.findByRole("menu", { name: "origin/other" })).toBeTruthy();
});

test("escape_with_text_clears_it_and_without_text_closes", async () => {
  const { onclose } = await popup();
  await type("ma");

  await press("Escape");

  expect((field() as HTMLInputElement).value).toBe("");
  expect(onclose).not.toHaveBeenCalled();

  await press("Escape");
  expect(onclose).toHaveBeenCalled();
});

test("arrows_traverse_the_rows_and_folders_open", async () => {
  await popup();
  // Selected: side. Going down ends on Remote.
  await press("ArrowDown");
  expect(selectedRow()).toBe("Remote");

  await press("ArrowRight");
  expect(rows()).toContain("origin");

  await press("ArrowLeft");
  expect(rows()).not.toContain("origin");
});

test("click_on_a_section_collapses_it", async () => {
  await popup();

  await fireEvent.click(row("Local"));

  expect(rows()).not.toContain("main");
  await fireEvent.click(row("Local"));
  expect(rows()).toContain("main");
});

test("group_and_recent_show_up_in_the_gear", async () => {
  list = { ...LIST, recent: ["main"] };
  await popup();
  await fireEvent.click(screen.getByRole("button", { name: "Settings" }));

  expect(screen.getByRole("menuitem", { name: "Restore Popup Size" })).toBeTruthy();
  await fireEvent.click(screen.getByRole("menuitem", { name: "Show Recent Branches" }));

  await waitFor(() => expect(rows()).not.toContain("Recent"));
  expect(get(branchOptions).showRecent).toBe(false);
  expect(calls("branch_options_set")).toHaveLength(1);
});

test("on_close_it_remembers_what_is_open", async () => {
  const { onclose } = await popup();
  await fireEvent.click(row("Remote"));

  await press("Escape");

  expect(onclose).toHaveBeenCalled();
  cleanup();
  expect(get(branchOptions).expanded).toContain("remote");
});

test("what_was_remembered_opens_again", async () => {
  branchOptions.set({ ...defaultBranchOptions(), expanded: ["remote"] });

  await popup();

  expect(rows()).toContain("origin");
});

test("click_outside_closes", async () => {
  const { onclose } = await popup();

  await fireEvent.pointerDown(document.body);

  expect(onclose).toHaveBeenCalled();
});

// `GitBranchPopupActions.createActions`: what continues a half-done operation goes at the very top.
const rebasing = {
  operation: "rebasing",
  branch: "main",
  label: "Rebasing main",
  head: "abc",
} as const;
const handlers = () => ({ abort: vi.fn(), continue: vi.fn(), skip: vi.fn() });

test("popup_puts_the_ongoing_actions_on_top", async () => {
  const calls = handlers();
  await popup({ ongoing: ongoingActions(rebasing, calls) });

  const labels = rows();
  expect(labels.slice(0, 4)).toEqual([
    "Abort Rebase",
    "Continue Rebase",
    "Skip Commit",
    "Update Project",
  ]);
  expect(labels.indexOf("Skip Commit")).toBeLessThan(labels.indexOf("Commit…"));
  expect(labels.indexOf("Skip Commit")).toBeLessThan(labels.indexOf("New Branch…"));
  await fireEvent.click(row("Continue Rebase"));
  expect(calls.continue).toHaveBeenCalledTimes(1);
});

test("without_an_operation_none_appears", async () => {
  await popup({ ongoing: ongoingActions({ ...rebasing, operation: null }, handlers()) });

  expect(rows()[0]).toBe("Update Project");
  expect(rows().some((label) => /^(Abort|Continue|Skip)/.test(label ?? ""))).toBe(false);
});

test("a_merge_only_offers_abort_merge", async () => {
  await popup({ ongoing: ongoingActions({ ...rebasing, operation: "merging" }, handlers()) });

  expect(rows().slice(0, 2)).toEqual(["Abort Merge", "Update Project"]);
});
