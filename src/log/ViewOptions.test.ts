import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { afterAll, afterEach, beforeAll, beforeEach, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { get } from "svelte/store";
import type { LogFilter, LogRow, TaskEvent } from "../lib/api";
import { dismiss, notifications } from "../lib/notifications";
import { trackTasks } from "../lib/tasks";
import { favorites } from "../branches/state";
import { drawRows } from "./draw";
import LogPanel from "./LogPanel.svelte";
import { defaultOptions, historyOptions } from "./options";

// `Git.Log.DeepCompare` (B:557).
const NOT_PICKED = "Not Cherry-Picked Commits";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
  emit: vi.fn(async () => {}),
}));
// What the table paints with: jsdom has no canvas, and the painting itself is `draw.test.ts`'s.
vi.mock("./draw", async (original) => ({
  ...(await original<typeof import("./draw")>()),
  drawRows: vi.fn(),
  columnWidths: () => ({ author: 150, date: 100, hash: 72, signature: 24 }),
}));

const HASH_A = "a".repeat(40);
const HASH_B = "b".repeat(40);

const row = (hash: string, subject: string): LogRow => ({
  hash,
  subject,
  author: "Ana",
  time: 100,
  commitTime: 100,
  samePerson: true,
  parentCount: 1,
  refs: [],
  labels: { compact: [], normal: [] },
  highlight: { bold: false, muted: false, branch: false },
  print: { nodeX: 0, nodeColor: 1, width: 2, edges: [], arrows: [] },
});

type Handler = (args: Record<string, unknown>) => unknown;

let commands: Record<string, Handler>;
let handlers: Map<string, ((event: { payload: unknown }) => void)[]>;
let nextTask = 100;
let nextView = 2;
// Tasks whose result the test sends when it wants.
let held: Map<number, string>;

function deliver(id: number, value: unknown, failure?: string) {
  const event: TaskEvent = {
    id,
    title: "",
    text: failure ?? "",
    fraction: null,
    state: failure ? "failed" : "done",
    kind: failure ? "failed" : null,
    value,
  };
  handlers.get("task-changed")?.forEach((handler) => handler({ payload: event }));
}

function startTask(value: unknown): number {
  const id = nextTask++;
  setTimeout(() => deliver(id, value));
  return id;
}

function calls(command: string) {
  return vi.mocked(invoke).mock.calls.filter(([name]) => name === command);
}

async function panel() {
  render(LogPanel, { active: true });
  await screen.findByRole("grid", { name: "Commits" });
  await waitFor(() => expect(calls("log_rows").length).toBeGreaterThan(0));
  await new Promise((resolve) => setTimeout(resolve));
}

async function viewOptions() {
  await fireEvent.click(screen.getByRole("button", { name: "View Options" }));
  return screen.getByRole("menu", { name: "View Options" });
}

const toggle = (menu: HTMLElement) => within(menu).getByRole("menuitem", { name: NOT_PICKED });

async function clickToggle() {
  await fireEvent.click(toggle(await viewOptions()));
}

async function checked() {
  const menu = await viewOptions();
  const value = toggle(menu).textContent?.includes("✓");
  await fireEvent.keyDown(menu, { key: "Escape" });
  return value;
}

// The Branch filter with a branch of the `Local` group.
async function filterByBranch(name: string) {
  await fireEvent.click(screen.getByRole("button", { name: "Branch" }));
  await fireEvent.click(await screen.findByRole("menuitem", { name: "Local" }));
  await fireEvent.click(await screen.findByRole("menuitem", { name }));
  await waitFor(() => expect(screen.getByRole("button", { name: `Branch: ${name}` })).toBeTruthy());
}

// The branch of the filters as Rust sees it: the only branch, or nothing.
const comparedOf = ({ filter }: Record<string, unknown>) => {
  const { branches } = filter as LogFilter;
  return branches.length === 1 ? branches[0] : null;
};

beforeAll(() => {
  vi.stubGlobal(
    "ResizeObserver",
    class {
      constructor(private run: () => void) {}
      observe() {
        this.run();
      }
      unobserve() {}
      disconnect() {}
    },
  );
  HTMLCanvasElement.prototype.getContext = (() => ({ scale() {} })) as never;
});

afterAll(() => {
  vi.unstubAllGlobals();
});

beforeEach(async () => {
  vi.mocked(drawRows).mockClear();
  handlers = new Map();
  held = new Map();
  favorites.set({ local: ["main"], remote: [] });
  vi.mocked(listen).mockImplementation(async (event, handler) => {
    handlers.set(event as string, [...(handlers.get(event as string) ?? []), handler as never]);
    return () => {};
  });
  commands = {
    log_options_get: () => ({}),
    log_options_set: () => undefined,
    log_open: () => ({ viewId: 1, count: 2, collapsible: true }),
    log_rows: ({ start }) => (start === 0 ? [row(HASH_A, "one"), row(HASH_B, "two")] : []),
    log_refs: () => [
      { name: "HEAD", kind: "head" },
      { name: "main", kind: "local" },
      { name: "lateral", kind: "local" },
      { name: "origin/main", kind: "remote" },
    ],
    log_users: () => [],
    log_current_branch: () => "main",
    log_graph_hover: () => null,
    log_recent_filters: () => ({}),
    log_refs_changed: () => false,
    branch_list: () => ({
      local: [{ name: "main", tracked: null }, { name: "lateral", tracked: null }],
      remote: [],
      recent: [],
      current: "main",
      previous: null,
    }),
    branch_incoming_outgoing: () => ({}),
    log_find_rows: ({ hashes }) => (hashes as string[]).map(() => null),
    log_view_open: () => {
      const viewId = nextView++;
      return startTask({ viewId, summary: { viewId, count: 2, collapsible: true } });
    },
    log_compared_branch: comparedOf,
    log_not_picked: ({ branch }) => {
      const id = nextTask++;
      held.set(id, branch as string);
      return id;
    },
    task_cancel: () => undefined,
    log_signatures: () => nextTask++,
  };
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    const handler = commands[command];
    if (!handler) throw new Error(`unexpected command: ${command}`);
    return handler((args ?? {}) as Record<string, unknown>);
  });
  await trackTasks();
});

afterEach(() => {
  cleanup();
  get(notifications).forEach(({ id }) => dismiss(id));
});

// The comparison that was asked for, answered.
async function answer(commits: string[], failure?: string) {
  await waitFor(() => expect(held.size).toBe(1));
  const [[id]] = [...held];
  held.clear();
  deliver(id, failure ? null : { current: "main", commits }, failure);
  await new Promise((resolve) => setTimeout(resolve));
}

// The commits the table keeps in their style when it last painted, or null with no highlight.
async function highlighted() {
  await new Promise((resolve) => requestAnimationFrame(resolve));
  const painted = vi.mocked(drawRows).mock.calls.at(-1)?.[0].notPicked ?? null;
  return painted === null ? null : [...painted];
}

test("the_toggle_follows_the_highlight_section", async () => {
  await panel();

  const menu = await viewOptions();

  const shown = [...menu.querySelectorAll("[role=menuitem], [role=separator]")].map(
    (item) => item.getAttribute("aria-label") ?? "—",
  );
  const at = shown.indexOf(NOT_PICKED);
  expect(shown.slice(at - 4, at + 2)).toEqual([
    "Highlight",
    "My Commits",
    "Merge Commits",
    "Current Branch",
    NOT_PICKED,
    "Branches",
  ]);
  expect(toggle(menu).textContent).not.toContain("✓");
});

test("toggle_with_one_branch_compares_at_once", async () => {
  await panel();
  await filterByBranch("lateral");
  await waitFor(() => expect(calls("log_view_open")).toHaveLength(1));

  await clickToggle();
  await answer([HASH_A]);

  expect(calls("log_not_picked").map(([, args]) => args)).toEqual([{ branch: "lateral" }]);
  expect(screen.queryByRole("menu", { name: "Select Source Branch" })).toBeNull();
  expect(await checked()).toBe(true);

  // Unchecking stops it.
  await clickToggle();
  expect(await checked()).toBe(false);
});

test("toggle_without_one_branch_asks_select_source_branch", async () => {
  await panel();

  // Chosen with the mouse: the popup opens where it was pressed, with its title.
  const item = toggle(await viewOptions());
  await fireEvent.pointerDown(item, { clientX: 300, clientY: 200 });
  await fireEvent.click(item);

  const popup = await screen.findByRole("menu", { name: "Select Source Branch" });
  expect([popup.style.left, popup.style.top]).toEqual(["300px", "200px"]);
  expect(popup.firstElementChild?.textContent).toBe("Select Source Branch");
  expect(calls("log_not_picked")).toHaveLength(0);
  expect(
    within(popup)
      .getAllByRole("menuitem")
      .map((item) => item.getAttribute("aria-label")),
  ).toEqual(["HEAD", "main", "Local", "origin"]);
  await fireEvent.click(within(popup).getByRole("menuitem", { name: "Local" }));
  await fireEvent.click(await screen.findByRole("menuitem", { name: "lateral" }));

  // The branch replaces the Branch filter and is compared.
  await waitFor(() => expect(calls("log_view_open")).toHaveLength(1));
  const [, opened] = calls("log_view_open")[0] as [string, { spec: { filter: LogFilter } }];
  expect(opened.spec.filter).toMatchObject({ branches: ["lateral"], revisions: [], ranges: [] });
  expect(screen.getByRole("button", { name: "Branch: lateral" })).toBeTruthy();
  await answer([HASH_A]);
  expect(calls("log_not_picked").map(([, args]) => args)).toEqual([{ branch: "lateral" }]);
  expect(await checked()).toBe(true);
});

test("a_tag_or_a_hash_is_not_compared", async () => {
  commands.log_not_picked = () => null;
  await panel();
  await filterByBranch("lateral");

  await clickToggle();

  await waitFor(() => expect(calls("log_not_picked")).toHaveLength(1));
  expect(await checked()).toBe(false);
});

test("changing_the_branch_filter_cancels_with_the_balloon", async () => {
  await panel();
  await filterByBranch("lateral");
  await clickToggle();
  await answer([HASH_A]);
  expect(await checked()).toBe(true);

  // The User filter does not change the branch: it goes on.
  await fireEvent.click(screen.getByRole("button", { name: "User" }));
  await fireEvent.click(await screen.findByRole("menuitem", { name: "me" }));
  await waitFor(() => expect(calls("log_compared_branch").length).toBe(2));
  expect(screen.queryByRole("status", { name: /cancelled/ })).toBeNull();
  expect(await checked()).toBe(true);

  await fireEvent.click(screen.getByRole("button", { name: "Remove Branch filter" }));

  const balloon = await screen.findByText(
    "Highlighting of non-picked commits in 'lateral' has been cancelled, as branch filter was changed.",
  );
  expect(balloon.getAttribute("role")).toBe("status");
  expect(await checked()).toBe(false);
});

test("a_refresh_compares_again_unless_the_current_branch_changed", async () => {
  await panel();
  await filterByBranch("lateral");
  await clickToggle();
  await answer([HASH_A]);

  await fireEvent.click(screen.getByRole("button", { name: "Refresh" }));

  await waitFor(() => expect(calls("log_not_picked")).toHaveLength(2));
  // What was highlighted stays until the new result.
  expect(await highlighted()).toEqual([HASH_A]);
  await answer([HASH_B]);
  expect(await highlighted()).toEqual([HASH_B]);
  expect(await checked()).toBe(true);

  // Another current branch: it stops, without a balloon.
  commands.log_current_branch = () => "lateral";
  await fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
  await waitFor(() => expect(calls("log_open")).toHaveLength(3));
  await waitFor(async () => expect(await checked()).toBe(false));
  expect(calls("log_not_picked")).toHaveLength(2);
  expect(screen.queryByText(/has been cancelled/)).toBeNull();
});

test("a_failure_says_could_not_compare_and_stays_checked", async () => {
  await panel();
  await filterByBranch("lateral");
  await clickToggle();

  await answer([], "fatal: bad revision");

  const shown = get(notifications);
  expect(shown.map(({ kind, title, body }) => ({ kind, title, body }))).toEqual([
    { kind: "error", title: "Could not compare with branch lateral", body: "fatal: bad revision" },
  ]);
  expect(await checked()).toBe(true);
});

test("a_failed_recomputation_clears_the_highlight", async () => {
  await panel();
  await filterByBranch("lateral");
  await clickToggle();
  await answer([HASH_A]);
  expect(await highlighted()).toEqual([HASH_A]);

  await fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
  await waitFor(() => expect(calls("log_not_picked")).toHaveLength(2));
  await answer([], "fatal: bad revision");

  expect(await highlighted()).toBeNull();
  expect(await checked()).toBe(true);
  expect(get(notifications).map(({ title }) => title)).toEqual(["Could not compare with branch lateral"]);
});

test("a_refresh_while_it_computes_cancels_that_task_and_drops_its_answer", async () => {
  await panel();
  await filterByBranch("lateral");
  await clickToggle();
  await waitFor(() => expect(held.size).toBe(1));
  const [[first]] = [...held];
  held.clear();

  await fireEvent.click(screen.getByRole("button", { name: "Refresh" }));

  await waitFor(() => expect(calls("log_not_picked")).toHaveLength(2));
  expect(calls("task_cancel").map(([, args]) => args)).toContainEqual({ id: first });
  // The old task's late answer, a failure or not, is no longer this tab's.
  deliver(first, null, "cancelled");
  await new Promise((resolve) => setTimeout(resolve));
  expect(get(notifications)).toEqual([]);
  expect(await highlighted()).toBeNull();
  await answer([HASH_B]);
  expect(await highlighted()).toEqual([HASH_B]);
  expect(await checked()).toBe(true);
});

test("a_toggle_while_the_comparison_is_asked_for_stops_it", async () => {
  let reply: (id: number) => void = () => {};
  commands.log_not_picked = () => new Promise<number>((resolve) => (reply = resolve));
  await panel();
  await filterByBranch("lateral");
  await clickToggle();
  await waitFor(() => expect(calls("log_not_picked")).toHaveLength(1));

  // Checked at once, before git is asked; unchecking now stops it.
  expect(await checked()).toBe(true);
  await clickToggle();
  reply(555);
  await new Promise((resolve) => setTimeout(resolve));

  expect(await checked()).toBe(false);
  expect(calls("task_cancel").map(([, args]) => args)).toContainEqual({ id: 555 });
});

test("a_refresh_whose_filters_name_another_branch_stops_with_the_balloon", async () => {
  await panel();
  await filterByBranch("lateral");
  await clickToggle();
  await answer([HASH_A]);
  // After a fetch, the filter matches another branch.
  commands.log_compared_branch = () => null;

  await fireEvent.click(screen.getByRole("button", { name: "Refresh" }));

  await screen.findByText(/non-picked commits in 'lateral' has been cancelled/);
  expect(await checked()).toBe(false);
  expect(calls("log_not_picked")).toHaveLength(1);
});

// `log_compared_branch` answers when the test says: each call waits in order.
function holdComparedBranch() {
  const waiting: ((branch: string | null) => void)[] = [];
  commands.log_compared_branch = () => new Promise<string | null>((resolve) => waiting.push(resolve));
  return waiting;
}

const CANCELLED = "Highlighting of non-picked commits in 'lateral' has been cancelled, as branch filter was changed.";

test("a_filter_change_while_a_refresh_checks_the_branch_stops_with_the_balloon", async () => {
  await panel();
  await filterByBranch("lateral");
  await clickToggle();
  await answer([HASH_A]);
  const waiting = holdComparedBranch();

  // The refresh asks which branch its filters name, and the filter changes before the answer.
  await fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
  await waitFor(() => expect(waiting).toHaveLength(1));
  await fireEvent.click(screen.getByRole("button", { name: "Remove Branch filter" }));
  await waitFor(() => expect(waiting).toHaveLength(2));
  // The refresh's answer is for the filters as they were; then the new filters' answer.
  waiting[0]("lateral");
  await new Promise((resolve) => setTimeout(resolve));
  waiting[1](null);

  await screen.findByText(CANCELLED);
  expect(await checked()).toBe(false);
});

test("a_filter_change_while_the_toggle_checks_the_branch_is_seen", async () => {
  await panel();
  await filterByBranch("lateral");
  const waiting = holdComparedBranch();

  await clickToggle();
  await waitFor(() => expect(waiting).toHaveLength(1));
  await fireEvent.click(screen.getByRole("button", { name: "Remove Branch filter" }));
  waiting[0]("lateral");
  // The filters changed while it waited: they are asked again.
  await waitFor(() => expect(waiting).toHaveLength(2));
  waiting[1](null);

  await screen.findByText(CANCELLED);
  expect(calls("log_not_picked")).toHaveLength(0);
  expect(await checked()).toBe(false);
});

// `GitCommitSignatureStatusProvider`: `GPG Signature` (`GitB:1084`) is hidden by default, and comes
// after the other columns in `Columns` (`ToggleLogColumnsActionGroup`). The file history's own menu
// is `HistoryTab.test.ts`'s.
test("the_column_is_hidden_by_default_and_toggles_in_both_columns_menus", async () => {
  expect(defaultOptions().columns).not.toContain("signature");
  expect(defaultOptions(true).columns).not.toContain("signature");
  expect(historyOptions().columns).not.toContain("signature");
  await panel();

  const columns = async () => {
    await fireEvent.click(within(await viewOptions()).getByRole("menuitem", { name: "Columns" }));
    return screen.getByRole("menu", { name: "Columns" });
  };
  const items = (menu: HTMLElement) =>
    within(menu)
      .getAllByRole("menuitem")
      .map((item) => [item.getAttribute("aria-label"), item.textContent?.includes("✓")]);

  expect(items(await columns())).toEqual([
    ["Author", true],
    ["Date", true],
    ["Hash", false],
    ["GPG Signature", false],
  ]);
  await fireEvent.click(within(screen.getByRole("menu", { name: "Columns" })).getByRole("menuitem", { name: "GPG Signature" }));

  await waitFor(() => expect(calls("log_options_set").length).toBeGreaterThan(0));
  const saved = JSON.stringify(calls("log_options_set").at(-1)?.[1]);
  expect(saved).toContain('"columns":["subject","author","date","signature"]');
  await new Promise((resolve) => requestAnimationFrame(resolve));
  expect(vi.mocked(drawRows).mock.calls.at(-1)?.[0].options.columns).toContain("signature");
  expect(items(await columns()).at(-1)).toEqual(["GPG Signature", true]);
});
