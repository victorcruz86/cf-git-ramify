import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { afterAll, afterEach, beforeAll, beforeEach, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { get } from "svelte/store";
import type { LogRow, TaskEvent, ViewSpec } from "../lib/api";
import { dismiss, notifications } from "../lib/notifications";
import { trackTasks } from "../lib/tasks";
import { compareBranches } from "../compare/actions";
import LogPanel from "./LogPanel.svelte";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
  emit: vi.fn(async () => {}),
}));

const MAIN_VIEW = 1;
const TOP = "Commits that exist in side but don't exist in main";
const BOTTOM = "Commits that exist in main but don't exist in side";

function row(hash: string, subject: string): LogRow {
  return {
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
  };
}

const ROWS = [row("a".repeat(40), "one"), row("b".repeat(40), "two")];

type Handler = (args: Record<string, unknown>) => unknown;

let commands: Record<string, Handler>;
let handlers: Map<string, ((event: { payload: unknown }) => void)[]>;
let nextTask = 100;
let nextView = 2;
// How many commits a view over each range has: `main` already has every commit of `side`.
let counts: Record<string, number>;

function deliver(value: unknown): number {
  const id = nextTask++;
  const event: TaskEvent = {
    id,
    title: "",
    text: "",
    fraction: null,
    state: "done",
    kind: null,
    value,
  };
  handlers.get("task-changed")?.forEach((handler) => handler({ payload: event }));
  return id;
}

const calls = (command: string) =>
  vi.mocked(invoke).mock.calls.filter(([name]) => name === command);

const rangeOf = (spec: ViewSpec) => spec.filter.ranges.map(([a, b]) => `${a}..${b}`).join(" ");

// The ranges of the views opened, in order.
const opened = () =>
  calls("log_view_open")
    .map(([, args]) => (args as { spec: ViewSpec }).spec)
    .filter((spec) => spec.filter.ranges.length > 0)
    .map(rangeOf);

function mock(overrides: Record<string, Handler>) {
  commands = { ...commands, ...overrides };
}

function repoChanged() {
  handlers.get("repo-changed")?.forEach((handler) => handler({ payload: undefined }));
}

// jsdom does no layout: CodeMirror measures with this.
Range.prototype.getClientRects = () => [] as unknown as DOMRectList;

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
  HTMLCanvasElement.prototype.getContext = (() => null) as never;
});

afterAll(() => {
  vi.unstubAllGlobals();
});

beforeEach(async () => {
  handlers = new Map();
  nextView = 2;
  counts = { "main..side": 2, "side..main": 0 };
  vi.mocked(listen).mockImplementation(async (event, handler) => {
    handlers.set(event as string, [...(handlers.get(event as string) ?? []), handler as never]);
    return () => {};
  });
  commands = {
    log_options_get: () => ({}),
    log_options_set: () => undefined,
    log_open: () => ({ viewId: MAIN_VIEW, count: 2, collapsible: true }),
    log_rows: ({ start }) => (start !== 0 ? [] : ROWS),
    log_refs: () => [
      { name: "HEAD", kind: "head" },
      { name: "main", kind: "local" },
      { name: "side", kind: "local" },
    ],
    log_users: () => [],
    log_current_branch: () => "main",
    log_graph_hover: () => null,
    log_recent_filters: () => ({}),
    log_remember_filter: () => undefined,
    log_refs_changed: () => false,
    branch_list: () => ({ local: [], remote: [], recent: [], current: "main", previous: null }),
    branch_incoming_outgoing: () => ({}),
    log_view_open: ({ spec }) => {
      const viewId = nextView++;
      const range = rangeOf(spec as ViewSpec);
      const filtered = (spec as ViewSpec).filter.text !== null;
      const count = filtered ? 0 : (counts[range] ?? 2);
      return deliver({ viewId, summary: { viewId, count, collapsible: false } });
    },
    log_view_close: () => undefined,
    log_find_rows: ({ hashes }) => (hashes as string[]).map(() => null),
    commit_details: () => [],
    branches_containing: () => [],
    commit_changes_of: () => [
      {
        parent: null,
        subject: null,
        base: "f".repeat(40),
        merge: false,
        files: [{ path: "src/a.txt", oldPath: null, status: "M" }],
        size: 1,
      },
    ],
    viewer_openable: () => [],
    file_sides: () => ({
      old: { kind: "text", content: "a\n" },
      new: { kind: "text", content: "b\n" },
    }),
    task_cancel: () => undefined,
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

async function panel() {
  const view = render(LogPanel, { active: true });
  await screen.findByRole("grid", { name: "Commits" });
  await waitFor(() => expect(calls("log_rows").length).toBeGreaterThan(0));
  return view;
}

const tabNames = () =>
  within(screen.getByRole("tablist", { name: "Log tabs" }))
    .getAllByRole("tab")
    .map((tab) => tab.textContent?.trim());

const half = (name: string) => screen.getByRole("region", { name });

// `GitCompareBranchesUi`: `current..branch` above, reversed below, each with its banner and the
// filters minus Branch; `{0} contains all commits from {1}` until another filter is set.
test("compare_tab_shows_both_ranges_and_the_empty_text", async () => {
  await panel();

  await compareBranches("side", "main");

  expect(tabNames()).toEqual(["Log", "Compare: side and main"]);
  expect(opened()).toEqual(["main..side", "side..main"]);
  const top = await screen.findByRole("region", { name: TOP });
  const bottom = half(BOTTOM);
  // `HtmlChunk.text(name).bold()` inside `code`.
  expect([...top.querySelectorAll("code b")].map((name) => name.textContent)).toEqual([
    "side",
    "main",
  ]);
  expect(within(top).queryByRole("button", { name: /^Branch/ })).toBeNull();
  expect(within(top).getByRole("button", { name: "User" })).toBeTruthy();
  await within(bottom).findByText("side contains all commits from main");
  await within(top).findByRole("grid", { name: "Commits" });
  expect(within(top).queryByText("main contains all commits from side")).toBeNull();

  const text = within(bottom).getByRole("searchbox", { name: "Filter by Commit Message or Hashes" });
  await fireEvent.input(text, { target: { value: "fix" } });
  await fireEvent.keyDown(text, { key: "Enter" });
  await within(bottom).findByText("No commits matching filters");
  expect(opened()).toEqual(["main..side", "side..main", "side..main"]);
  await fireEvent.click(within(bottom).getByRole("button", { name: "Reset filters" }));
  await within(bottom).findByText("side contains all commits from main");

  // `GitCompareBranchesFilesManager.openFile` goes back to the tab of those ranges.
  await compareBranches("side", "main");
  expect(tabNames()).toEqual(["Log", "Compare: side and main"]);
  await compareBranches("main", "side");
  expect(tabNames()).toEqual(["Log", "Compare: side and main", "Compare: main and side"]);
});

// Review Focus 5: a checkout moves the references, the Log reloads, and both logs are read again.
test("compare_tab_refreshes_with_the_log_after_a_checkout", async () => {
  await panel();
  await compareBranches("side", "main");
  await screen.findByRole("region", { name: TOP });
  expect(opened()).toHaveLength(2);

  counts["side..main"] = 1;
  mock({ log_refs_changed: () => true });
  repoChanged();

  await waitFor(() => expect(calls("log_open")).toHaveLength(2));
  await waitFor(() => expect(opened()).toEqual(["main..side", "side..main", "main..side", "side..main"]));
  await waitFor(() =>
    expect(within(half(BOTTOM)).queryByText("side contains all commits from main")).toBeNull(),
  );
});

test("closing_the_compare_tab_closes_both_views", async () => {
  await panel();
  await compareBranches("side", "main");
  await screen.findByRole("region", { name: TOP });

  await fireEvent.click(screen.getByRole("button", { name: "Close Log Tab" }));

  await waitFor(() => expect(calls("log_view_close")).toHaveLength(2));
  expect(tabNames()).toEqual(["Log"]);
});

// `GitCompareBranchesUi.create`: each half is a `VcsLogPanel` with the changes and details of its
// selection and its diff preview; ⌘D opens the preview of the half that has the focus, and
// `Vcs.Log.FocusTextFilter` (⌘L) goes to its text filter.
test("each_half_has_its_changes_details_and_diff_preview", async () => {
  await panel();
  await compareBranches("side", "main");
  const top = await screen.findByRole("region", { name: TOP });
  const bottom = half(BOTTOM);
  const grid = await within(top).findByRole("grid", { name: "Commits" });
  expect(within(top).getByText("Select commit to view changes")).toBeTruthy();
  expect(within(bottom).getByText("Select commit to view changes")).toBeTruthy();

  await fireEvent.click(grid, { clientX: 300, clientY: 2 });
  await within(top).findByRole("treeitem", { name: "a.txt" });
  expect(calls("commit_changes_of").at(-1)?.[1]).toEqual({ hash: "a".repeat(40), mode: "merged" });
  expect(calls("commit_details").length).toBeGreaterThan(0);
  expect(within(bottom).getByText("Select commit to view changes")).toBeTruthy();

  await fireEvent.keyDown(grid, { key: "l", metaKey: true });
  expect(document.activeElement).toBe(
    within(top).getByRole("searchbox", { name: "Filter by Commit Message or Hashes" }),
  );
  expect(within(top).queryByRole("region", { name: "Diff Preview" })).toBeNull();
  await fireEvent.keyDown(grid, { key: "d", metaKey: true });
  await within(top).findByRole("region", { name: "Diff Preview" });
  expect(within(bottom).queryByRole("region", { name: "Diff Preview" })).toBeNull();

});
