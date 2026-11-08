import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { get } from "svelte/store";
import { invoke } from "@tauri-apps/api/core";
import type { LogRef } from "../lib/api";
import { favorites } from "../branches/state";
import FilterBar from "./FilterBar.svelte";
import { emptyFilters, lastDay, ME, type FilterState } from "./filters";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const REFS: LogRef[] = [
  { name: "HEAD", kind: "head" },
  { name: "main", kind: "local" },
  { name: "lateral", kind: "local" },
  { name: "origin/main", kind: "remote" },
  { name: "origin/other", kind: "remote" },
  { name: "v1", kind: "tag" },
];

let recent: Record<string, string[][]> = {};

async function bar(props: Record<string, unknown> = {}) {
  const onchange = vi.fn<(filters: FilterState) => void>();
  render(FilterBar, { refs: REFS, users: [], onchange, ...props });
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("log_recent_filters"));
  await Promise.resolve();
  return onchange;
}

const items = () => screen.getAllByRole("menuitem").map((item) => item.getAttribute("aria-label"));

async function choose(name: string) {
  await fireEvent.click(screen.getByRole("menuitem", { name }));
}

async function openChip(name: string) {
  await fireEvent.click(screen.getByRole("button", { name }));
  return screen.getByRole("menu");
}

// `⌘⏎`, which closes the values popup accepting what was typed.
const finish = (field: HTMLElement) => fireEvent.keyDown(field, { key: "Enter", metaKey: true });

beforeEach(() => {
  recent = {};
  favorites.set({ local: ["main", "master"], remote: ["origin/main", "origin/master"] });
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "log_recent_filters") return recent;
    if (command === "favorites_toggle") {
      return { local: ["main", "lateral"], remote: ["origin/main"] };
    }
    if (command === "log_remember_filter") {
      const { filter, values } = args as { filter: string; values: string[] };
      recent = { ...recent, [filter]: [values, ...(recent[filter] ?? [])] };
      return recent;
    }
    throw new Error(`unexpected command: ${command}`);
  });
});

afterEach(cleanup);

test("bar_in_order", async () => {
  await bar();

  const names = screen.getAllByRole("button").map((button) => button.getAttribute("aria-label"));

  expect(names).toEqual([
    "Search History",
    "Text filter settings",
    "Branch",
    "User",
    "Date",
    "Paths",
    "Refresh",
    "View Options",
    "Go to Hash/Branch/Tag",
  ]);
  const field = screen.getByRole("searchbox", { name: "Filter by Commit Message or Hashes" }) as HTMLInputElement;
  expect(field.placeholder).toBe("Text or hash");
  expect(field.title).toBe("Filter by Commit Message or Hashes");
});

test("the_text_filter_shows_text_or_hash", async () => {
  await bar();
  const field = screen.getByRole("searchbox", {
    name: "Filter by Commit Message or Hashes",
  }) as HTMLInputElement;
  expect(field.placeholder).toBe("Text or hash");
  expect(field.title).toBe("Filter by Commit Message or Hashes");
});

test("branch_popup_shows_select_head_recent_favorites_local_and_remotes", async () => {
  recent = { Branch: [["lateral"]] };
  await bar();

  await openChip("Branch");

  expect(items()).toEqual([
    "Select…",
    "Recent",
    "Favorites",
    "HEAD",
    "main",
    "origin/main",
    "Local",
    "origin",
  ]);
});

test("choosing_a_branch_filters_and_is_remembered", async () => {
  const onchange = await bar();
  await openChip("Branch");

  await choose("main");

  await waitFor(() => expect(onchange).toHaveBeenCalledOnce());
  expect(onchange.mock.calls[0][0].branches).toEqual(["main"]);
  expect(screen.getByRole("button", { name: "Branch: main" })).toBeTruthy();
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("log_remember_filter", {
      filter: "Branch",
      values: ["main"],
    }),
  );
  expect(localStorage.length).toBe(0);
});

test("just_used_recent_appears_without_reloading", async () => {
  await bar();
  await openChip("Branch");
  await choose("main");
  await waitFor(() => expect(recent.Branch).toBeDefined());
  await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());

  await openChip("Branch: main");

  await waitFor(() => expect(items()).toContain("Recent"));
});

test("branch_group_is_a_submenu", async () => {
  const onchange = await bar();
  await openChip("Branch");

  await fireEvent.pointerEnter(screen.getByRole("menuitem", { name: "Local" }));
  const group = screen.getByRole("menu", { name: "Local" });
  expect(within(group).getAllByRole("menuitem").map((item) => item.textContent?.trim())).toEqual([
    "lateral",
    "main",
  ]);
  await fireEvent.click(within(group).getByRole("menuitem", { name: "lateral" }));

  await waitFor(() => expect(onchange).toHaveBeenCalledOnce());
  expect(onchange.mock.calls[0][0].branches).toEqual(["lateral"]);
});

test("typing_in_the_branch_popup_flattens_the_submenus", async () => {
  recent = { Branch: [["lateral", "main"]] };
  await bar();
  const menu = await openChip("Branch");

  await fireEvent.keyDown(menu, { key: "o" });
  await fireEvent.keyDown(menu, { key: "t" });

  expect(items()).toEqual(["origin/other"]);
  expect(screen.getByRole("separator", { name: "origin" })).toBeTruthy();
});

test("typing_in_the_branch_popup_removes_the_groups", async () => {
  await bar();
  const menu = await openChip("Branch");

  await fireEvent.keyDown(menu, { key: "l" });

  expect(items()).toEqual(["Select…", "lateral"]);
});

test("favorites_selects_all", async () => {
  const onchange = await bar();
  await openChip("Branch");

  await choose("Favorites");

  await waitFor(() => expect(onchange).toHaveBeenCalledOnce());
  expect(onchange.mock.calls[0][0].branches).toEqual(["HEAD", "main", "origin/main"]);
  expect(invoke).not.toHaveBeenCalledWith("log_remember_filter", expect.anything());
});

async function editor(name: string, chip = name) {
  await openChip(chip);
  await choose("Select…");
  return (await screen.findByRole("textbox", { name })) as HTMLTextAreaElement;
}

test("select_accepts_several_values", async () => {
  const onchange = await bar();
  const field = await editor("Branch");

  await fireEvent.input(field, { target: { value: "main|lateral" } });
  await finish(field);

  await waitFor(() => expect(onchange).toHaveBeenCalledOnce());
  expect(onchange.mock.calls[0][0].branches).toEqual(["main", "lateral"]);
  expect(screen.queryByRole("textbox", { name: "Branch" })).toBeNull();
});

test("branch_select_separates_range_and_hash", async () => {
  const hash = "b".repeat(40);
  const onchange = await bar();
  const field = await editor("Branch");

  await fireEvent.input(field, { target: { value: `main\n${hash}\nv1..v2` } });
  await finish(field);

  await waitFor(() => expect(onchange).toHaveBeenCalledOnce());
  expect(onchange.mock.calls[0][0]).toMatchObject({
    branches: ["main"],
    revisions: [hash],
    ranges: [["v1", "v2"]],
  });
  expect(screen.getByRole("button", { name: `Branch: main|${hash.slice(0, 14)}…` })).toBeTruthy();
});

test("chip_shows_an_already_set_range", async () => {
  const onchange = await bar({ filters: { ...emptyFilters(), ranges: [["v1", "v2"]] } });

  expect(screen.getByRole("button", { name: "Branch: v1..v2" })).toBeTruthy();
  await fireEvent.click(screen.getByRole("button", { name: "Remove Branch filter" }));

  expect(onchange.mock.calls[0][0].ranges).toEqual([]);
});

test("branch_select_shows_range_and_hash_again", async () => {
  const hash = "b".repeat(40);
  await bar({
    filters: { ...emptyFilters(), branches: ["main"], revisions: [hash], ranges: [["a", "b"]] },
  });

  const field = await editor("Branch", `Branch: main|${hash.slice(0, 14)}…`);

  expect(field.value).toBe(`main\n${hash}\na..b`);
});

test("select_is_a_popup_under_the_chip_without_dimming_what_is_below", async () => {
  await bar();

  await editor("Branch");

  expect(document.querySelector(".backdrop")).toBeNull();
  expect(screen.getByText(/Select one or more values separated with \| or new lines/)).toBeTruthy();
  expect(screen.queryByRole("button", { name: "OK" })).toBeNull();
});

test("click_outside_closes_the_popup_without_applying", async () => {
  const onchange = await bar();
  const field = await editor("Branch");
  await fireEvent.input(field, { target: { value: "main" } });

  await fireEvent.pointerDown(field);
  expect(screen.getByRole("textbox", { name: "Branch" })).toBeTruthy();

  await fireEvent.pointerDown(document.body);

  expect(screen.queryByRole("textbox", { name: "Branch" })).toBeNull();
  expect(onchange).not.toHaveBeenCalled();
});

test("escape_closes_the_popup_without_applying", async () => {
  const onchange = await bar();
  const field = await editor("Branch");

  await fireEvent.keyDown(field, { key: "Escape" });

  expect(screen.queryByRole("textbox", { name: "Branch" })).toBeNull();
  expect(onchange).not.toHaveBeenCalled();
});

test("completes_branches_without_the_leading_dash", async () => {
  await bar();
  const field = await editor("Branch");

  await fireEvent.input(field, { target: { value: "-la", selectionStart: 3 } });

  const list = screen.getByRole("listbox", { name: "Completions" });
  expect(within(list).getAllByRole("option").map((option) => option.textContent)).toEqual(["lateral"]);

  await fireEvent.keyDown(field, { key: "Enter" });

  expect(field.value).toBe("-lateral");
  expect(screen.queryByRole("listbox")).toBeNull();
});

test("arrows_walk_through_what_can_be_completed", async () => {
  await bar();
  const field = await editor("Branch");
  await fireEvent.input(field, { target: { value: "ma", selectionStart: 2 } });

  await fireEvent.keyDown(field, { key: "ArrowDown" });
  await fireEvent.keyDown(field, { key: "Tab" });

  expect(field.value).toBe("origin/main");
});

test("completes_users_and_me", async () => {
  await bar({ users: [{ name: "ana", email: "a@b.c" }] });
  const field = await editor("User");

  await fireEvent.input(field, { target: { value: "m", selectionStart: 1 } });
  expect(screen.getByRole("option", { name: "me" })).toBeTruthy();

  await fireEvent.input(field, { target: { value: "an", selectionStart: 2 } });
  expect(screen.getByRole("option", { name: "ana" })).toBeTruthy();
});

test("paths_do_not_complete", async () => {
  await bar();
  const field = await editor("Paths");

  await fireEvent.input(field, { target: { value: "ma", selectionStart: 2 } });

  expect(screen.queryByRole("listbox")).toBeNull();
});

test("user_popup_with_me_and_recents", async () => {
  recent = { User: [["Luis"]] };
  const onchange = await bar({ users: [{ name: "ana", email: "a@b.c" }] });
  await openChip("User");

  expect(items()).toEqual(["Select…", "me", "Luis"]);
  expect(screen.getByRole("separator", { name: "Recent" })).toBeTruthy();
  await choose("me");

  await waitFor(() => expect(onchange).toHaveBeenCalledOnce());
  expect(onchange.mock.calls[0][0].users).toEqual([ME]);
  expect(screen.getByRole("button", { name: "User: me" })).toBeTruthy();
});

test("users_only_appear_when_typing", async () => {
  recent = { User: [["Luis"], ["Luis", "Ana"]] };
  await bar({ users: [{ name: "ana", email: "a@b.c" }, { name: "Mar", email: "m@b.c" }] });
  const menu = await openChip("User");

  await fireEvent.keyDown(menu, { key: "a" });

  // Of the recents only those with several values remain: the others are already among the users.
  expect(items()).toEqual(["Luis|Ana", "Mar", "ana"]);
});

test("user_select_translates_me", async () => {
  const onchange = await bar({ filters: { ...emptyFilters(), users: [ME, "Ana"] } });

  const field = await editor("User", "User: me|Ana");
  expect(field.value).toBe("me\nAna");

  await fireEvent.input(field, { target: { value: "me|Luis" } });
  await finish(field);

  await waitFor(() => expect(onchange).toHaveBeenCalledOnce());
  expect(onchange.mock.calls[0][0].users).toEqual([ME, "Luis"]);
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("log_remember_filter", {
      filter: "User",
      values: [ME, "Luis"],
    }),
  );
});

test("empty_select_removes_the_filter_without_remembering", async () => {
  const onchange = await bar({ filters: { ...emptyFilters(), users: ["Ana"] } });
  const field = await editor("User", "User: Ana");

  await fireEvent.input(field, { target: { value: "  " } });
  await finish(field);

  await waitFor(() => expect(onchange).toHaveBeenCalledOnce());
  expect(onchange.mock.calls[0][0].users).toEqual([]);
  expect(invoke).not.toHaveBeenCalledWith("log_remember_filter", expect.anything());
});

test("date_popup", async () => {
  vi.useFakeTimers({ toFake: ["Date"], now: new Date(2026, 5, 10, 12, 0, 0) });
  const onchange = await bar();
  await openChip("Date");

  expect(items()).toEqual(["Select…", "Last 24 hours", "Last 7 days"]);
  await choose("Last 24 hours");

  await waitFor(() => expect(onchange).toHaveBeenCalledOnce());
  expect(onchange.mock.calls[0][0].since).toBe(lastDay(new Date()));
  vi.useRealTimers();
});

test("period_chosen_by_hand", async () => {
  const onchange = await bar();
  await openChip("Date");
  await choose("Select…");

  const after = await screen.findByLabelText("After:");
  await fireEvent.input(after, { target: { value: "2026-01-02" } });
  await fireEvent.click(screen.getByRole("button", { name: "OK" }));

  await waitFor(() => expect(onchange).toHaveBeenCalledOnce());
  expect(onchange.mock.calls[0][0].since).toBe(new Date(2026, 0, 2).getTime() / 1000);
  expect(onchange.mock.calls[0][0].until).toBeNull();
});

test("paths_popup", async () => {
  const onchange = await bar();
  await openChip("Paths");
  expect(items()).toEqual(["Select…"]);

  await choose("Select…");
  const field = await screen.findByRole("textbox", { name: "Paths" });
  expect(screen.getByText(/separated with new lines/)).toBeTruthy();
  await fireEvent.input(field, { target: { value: "src/a|b.txt\nc.txt" } });
  await finish(field);

  await waitFor(() => expect(onchange).toHaveBeenCalledOnce());
  expect(onchange.mock.calls[0][0].paths).toEqual(["src/a|b.txt", "c.txt"]);
});

test("closing_a_filter_removes_it", async () => {
  const onchange = await bar({ filters: { ...emptyFilters(), branches: ["main"] } });
  expect(screen.getByRole("button", { name: "Branch: main" })).toBeTruthy();

  await fireEvent.click(screen.getByRole("button", { name: "Remove Branch filter" }));

  expect(onchange.mock.calls[0][0].branches).toEqual([]);
  expect(screen.getByRole("button", { name: "Branch" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Remove Branch filter" })).toBeNull();
});

test("regex_and_match_case_in_the_gear", async () => {
  const onchange = await bar();
  const gear = screen.getByRole("button", { name: "Text filter settings" });

  await fireEvent.click(gear);
  expect(items()).toEqual(["Regex", "Match Case"]);
  await choose("Regex");
  await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());

  // Without text only the option changes: there is nothing to filter again.
  expect(onchange).not.toHaveBeenCalled();

  const field = screen.getByRole("searchbox");
  await fireEvent.input(field, { target: { value: "fo+" } });
  await fireEvent.keyDown(field, { key: "Enter" });
  expect(onchange.mock.calls[0][0]).toMatchObject({ text: "fo+", regex: true, matchCase: false });

  await fireEvent.click(gear);
  await choose("Match Case");

  await waitFor(() => expect(onchange).toHaveBeenCalledTimes(2));
  expect(onchange.mock.calls[1][0]).toMatchObject({ text: "fo+", regex: true, matchCase: true });
});

test("typing_must_wait_for_enter", async () => {
  const onchange = await bar();
  const field = screen.getByRole("searchbox");

  await fireEvent.input(field, { target: { value: "fix" } });
  expect(onchange).not.toHaveBeenCalled();

  await fireEvent.keyDown(field, { key: "Enter" });

  expect(onchange).toHaveBeenCalledOnce();
  expect(onchange.mock.calls[0][0].text).toBe("fix");
});

test("enter_saves_to_the_history", async () => {
  await bar();
  const field = screen.getByRole("searchbox");

  await fireEvent.input(field, { target: { value: "fix" } });
  await fireEvent.keyDown(field, { key: "Enter" });

  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("log_remember_filter", {
      filter: "Text",
      values: ["fix"],
    }),
  );
});

test("text_history_with_its_button", async () => {
  recent = { Text: [["fix"], ["another"]] };
  const onchange = await bar();

  await fireEvent.click(screen.getByRole("button", { name: "Search History" }));

  expect(items()).toEqual(["fix", "another"]);
  await choose("another");

  await waitFor(() => expect(onchange).toHaveBeenCalledOnce());
  expect(onchange.mock.calls[0][0].text).toBe("another");
  expect((screen.getByRole("searchbox") as HTMLInputElement).value).toBe("another");
});

test("arrow_down_opens_the_history", async () => {
  recent = { Text: [["fix"]] };
  await bar();

  await fireEvent.keyDown(screen.getByRole("searchbox"), { key: "ArrowDown" });

  expect(items()).toEqual(["fix"]);
});

test("without_history_there_is_no_menu", async () => {
  await bar();

  await fireEvent.click(screen.getByRole("button", { name: "Search History" }));

  expect(screen.queryByRole("menu")).toBeNull();
});

test("losing_focus_applies_what_was_typed", async () => {
  const onchange = await bar();
  const field = screen.getByRole("searchbox");

  await fireEvent.input(field, { target: { value: "fix" } });
  await fireEvent.blur(field);
  await fireEvent.blur(field);

  expect(onchange).toHaveBeenCalledOnce();
  expect(invoke).not.toHaveBeenCalledWith("log_remember_filter", expect.anything());
});

test("clearing_the_field_removes_the_filter", async () => {
  const onchange = await bar({ filters: { ...emptyFilters(), text: "fix" } });

  await fireEvent.click(screen.getByRole("button", { name: "Clear" }));

  expect(onchange.mock.calls[0][0].text).toBe("");
  expect((screen.getByRole("searchbox") as HTMLInputElement).value).toBe("");
});

test("indicator_while_the_result_arrives", async () => {
  const { rerender } = render(FilterBar, { refs: REFS, users: [], onchange: () => {}, busy: false });
  expect(screen.queryByRole("status")).toBeNull();

  await rerender({ busy: true });

  expect(screen.getByRole("status", { name: "Loading commits…" })).toBeTruthy();
});

test("buttons_on_the_right", async () => {
  const onrefresh = vi.fn();
  const onviewoptions = vi.fn();
  const ongoto = vi.fn();
  await bar({ onrefresh, onviewoptions, ongoto });

  await fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
  await fireEvent.click(screen.getByRole("button", { name: "View Options" }));
  await fireEvent.click(screen.getByRole("button", { name: "Go to Hash/Branch/Tag" }));

  expect(onrefresh).toHaveBeenCalledOnce();
  expect(onviewoptions).toHaveBeenCalledOnce();
  expect(ongoto).toHaveBeenCalledOnce();
});

test("repository_favorites_appear_on_top_in_the_filter", async () => {
  favorites.set({ local: ["lateral"], remote: [] });
  await bar();

  await openChip("Branch");

  expect(items()).toEqual(["Select…", "Favorites", "HEAD", "lateral", "Local", "origin"]);
});

test("the_filter_star_toggles_without_closing_the_popup", async () => {
  const onchange = await bar();
  const menu = await openChip("Branch");
  await fireEvent.pointerEnter(within(menu).getByRole("menuitem", { name: "Local" }));
  const row = await screen.findByRole("menuitem", { name: "lateral" });
  const star = row.querySelector("[data-star]")!;
  expect(star.getAttribute("data-star")).toBe("off");

  await fireEvent.click(star);

  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("favorites_toggle", { name: "lateral", kind: "local" }),
  );
  expect(onchange).not.toHaveBeenCalled();
  expect(screen.getByRole("menu", { name: "Branch" })).toBeTruthy();
  expect(star.getAttribute("data-star")).toBe("on");
  await waitFor(() => expect(get(favorites).local).toContain("lateral"));
});
