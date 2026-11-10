import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { afterAll, afterEach, beforeAll, beforeEach, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { get } from "svelte/store";
import type { HistoryChange, LogRow, TaskEvent } from "../lib/api";
import { dismiss, notifications } from "../lib/notifications";
import { trackTasks } from "../lib/tasks";
import { showFolderHistory, showHistory, showHistoryOf } from "../compare/actions";
import { ROW_HEIGHT } from "./draw";
import { emptyFilters } from "./filters";
import LogPanel from "./LogPanel.svelte";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
  emit: vi.fn(async () => {}),
}));

const HASH_A = "a".repeat(40);
const HASH_B = "b".repeat(40);
const HASH_C = "c".repeat(40);
const HASH_D = "d".repeat(40);
const MAIN_VIEW = 1;
const X = 300;

function row(hash: string, subject: string, parentCount = 1): LogRow {
  return {
    hash,
    subject,
    author: "Ana",
    time: 100,
    commitTime: 100,
    samePerson: true,
    parentCount,
    refs: [],
    labels: { compact: [], normal: [] },
    highlight: { bold: false, muted: false, branch: false },
    print: { nodeX: 0, nodeColor: 1, width: 2, edges: [], arrows: [] },
  };
}

const MAIN_ROWS = [row(HASH_A, "one"), row(HASH_B, "two"), row(HASH_C, "three")];
// The history of `src/a.txt`: a merge of `b` and `c`.
const HISTORY_ROWS = [row(HASH_D, "merge", 2), row(HASH_B, "two"), row(HASH_C, "three")];

type Handler = (args: Record<string, unknown>) => unknown;

let commands: Record<string, Handler>;
let handlers: Map<string, ((event: { payload: unknown }) => void)[]>;
let nextTask = 100;
let nextView = 2;
// Per history view, the request it was opened for.
let historyViews: Map<number, Record<string, unknown>>;

function deliver(event: Partial<TaskEvent>): number {
  const id = nextTask++;
  const full: TaskEvent = {
    id,
    title: "",
    text: "",
    fraction: null,
    state: "done",
    kind: null,
    value: null,
    ...event,
  };
  handlers.get("task-changed")?.forEach((handler) => handler({ payload: full }));
  return id;
}

const calls = (command: string) =>
  vi.mocked(invoke).mock.calls.filter(([name]) => name === command);

const lastArgs = (command: string) => calls(command).at(-1)?.[1] as Record<string, unknown>;

function mock(overrides: Record<string, Handler>) {
  commands = { ...commands, ...overrides };
}

const MERGE: HistoryChange = {
  hash: HASH_D,
  path: "src/a.txt",
  after: { hash: HASH_D, path: "src/a.txt" },
  before: [
    { hash: HASH_B, path: "src/a.txt" },
    { hash: HASH_C, path: "src/old.txt" },
  ],
};

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
  historyViews = new Map();
  nextView = 2;
  vi.mocked(listen).mockImplementation(async (event, handler) => {
    handlers.set(event as string, [...(handlers.get(event as string) ?? []), handler as never]);
    return () => {};
  });
  commands = {
    log_options_get: () => ({}),
    log_options_set: () => undefined,
    log_open: () => ({ viewId: MAIN_VIEW, count: 3, collapsible: true }),
    log_rows: ({ viewId, start }) =>
      start !== 0 ? [] : historyViews.has(viewId as number) ? HISTORY_ROWS : MAIN_ROWS,
    log_refs: () => [
      { name: "HEAD", kind: "head" },
      { name: "main", kind: "local" },
    ],
    log_users: () => [],
    log_current_branch: () => "main",
    log_graph_hover: () => null,
    log_recent_filters: () => ({}),
    log_refs_changed: () => false,
    branch_list: () => ({ local: [], remote: [], recent: [], current: "main", previous: null }),
    branch_incoming_outgoing: () => ({}),
    log_view_open: () => {
      const viewId = nextView++;
      return deliver({ value: { viewId, summary: { viewId, count: 3, collapsible: false } } });
    },
    log_history_open: ({ request }) => {
      const viewId = nextView++;
      historyViews.set(viewId, request as Record<string, unknown>);
      return deliver({ value: { viewId, summary: { viewId, count: 3, collapsible: false } } });
    },
    log_view_close: () => undefined,
    log_find_rows: ({ viewId, hashes }) => {
      const rows = historyViews.has(viewId as number) ? HISTORY_ROWS : MAIN_ROWS;
      return (hashes as string[]).map((hash) => {
        const index = rows.findIndex((item) => item.hash === hash);
        return index < 0 ? null : index;
      });
    },
    log_history_entries: ({ hashes }) =>
      (hashes as string[]).map(() => ({ path: "src/a.txt", deleted: false })),
    log_history_change: () => MERGE,
    compare_sides: () => ({
      old: { kind: "text", content: "a\n" },
      new: { kind: "text", content: "b\n" },
      hunks: [],
      editable: false,
      note: null,
      hash: null,
      conflicted: false,
      oldPreview: false,
      newPreview: false,
      identical: null,
    }),
    merge_blocks: () => [],
    compare_local_state: () => ({ root: "/repo", modified: false, current: null, headLabel: "main" }),
    diff_window_open_request: () => undefined,
    commit_details: () => [],
    branches_containing: () => [],
    commit_changes_of: () => [],
    task_cancel: () => undefined,
    log_signatures: () => nextTask++,
    // No GitHub repository answers.
    github_links: () => [],
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

const historyRequests = () => calls("log_history_open").map(([, args]) => (args as { request: unknown }).request);

async function contextMenuOnRow(index: number) {
  const grid = screen.getByRole("grid", { name: "Commits" });
  // jsdom has no height: revealing the first row scrolled the table by one.
  grid.scrollTop = 0;
  await fireEvent.contextMenu(grid, {
    clientX: X,
    clientY: index * ROW_HEIGHT + 2,
  });
  return screen.findByRole("menu", { name: "File History" });
}

const menuItem = (menu: HTMLElement, name: string) =>
  within(menu).getByRole("menuitem", { name: new RegExp(`^${name}`) }) as HTMLButtonElement;

test("the_branch_filter_defaults_to_head_and_empty_means_all_refs", async () => {
  await panel();

  await showHistory("src/a.txt");

  expect(historyRequests()).toEqual([{ path: "src/a.txt", revision: null, branches: ["HEAD"] }]);
  const toolbar = screen.getByRole("toolbar", { name: "File History Toolbar" });
  await fireEvent.click(within(toolbar).getByRole("button", { name: "Remove Branch filter" }));
  await waitFor(() => expect(historyRequests()).toHaveLength(2));
  expect(historyRequests()[1]).toEqual({ path: "src/a.txt", revision: null, branches: [] });
  expect(within(toolbar).getByRole("button", { name: "Branch" })).toBeTruthy();
});

// `GitLogHistoryHandler.getSupportedFilters` is empty with a revision: no Branch filter, and the
// tab is named after the short hash and jumps to it.
test("history_up_to_here_has_no_branch_filter", async () => {
  await panel();

  await showHistory("src/a.txt", HASH_B);

  expect(historyRequests()).toEqual([{ path: "src/a.txt", revision: HASH_B, branches: [] }]);
  const toolbar = screen.getByRole("toolbar", { name: "File History Toolbar" });
  expect(within(toolbar).queryByRole("button", { name: /^Branch/ })).toBeNull();
  expect(tabNames()).toEqual(["Log", "History: a.txt (bbbbbbbb)"]);
  await waitFor(() =>
    expect(calls("log_find_rows").some(([, args]) => (args as { hashes: string[] }).hashes[0] === HASH_B)).toBe(true),
  );
});

test("history_tabs_are_reused_and_reopened_after_the_log_reloads", async () => {
  const view = await panel();

  await showHistory("src/a.txt");
  await showHistory("src/a.txt");
  await showHistory("src/a.txt", HASH_B);

  expect(tabNames()).toEqual(["Log", "History: a.txt", "History: a.txt (bbbbbbbb)"]);
  expect(historyRequests()).toHaveLength(2);
  const saved = lastArgs("log_options_set").options as Record<string, { tab?: unknown }>;
  const remembered = Object.values(saved).map((options) => options.tab).filter(Boolean);
  expect(remembered).toEqual([
    { kind: "history", key: "\0src/a.txt", name: "a.txt", history: { path: "src/a.txt", revision: null } },
    {
      kind: "history",
      key: `${HASH_B}\0src/a.txt`,
      name: "a.txt (bbbbbbbb)",
      history: { path: "src/a.txt", revision: HASH_B },
    },
  ]);

  // The repository opened again: the Log reads the saved tabs and computes their histories.
  view.unmount();
  mock({ log_options_get: () => saved });
  vi.mocked(invoke).mockClear();
  await panel();

  expect(tabNames()).toEqual(["Log", "History: a.txt", "History: a.txt (bbbbbbbb)"]);
  await waitFor(() => expect(historyRequests()).toHaveLength(2));
  await showHistory("src/a.txt");
  expect(tabNames()).toHaveLength(3);
});

test("closing_a_history_tab_closes_its_view_and_the_walk_with_it", async () => {
  await panel();
  await showHistory("src/a.txt");

  await fireEvent.click(screen.getByRole("button", { name: "Close Log Tab" }));

  await waitFor(() => expect(calls("log_view_close")).toHaveLength(1));
  expect(tabNames()).toEqual(["Log"]);
});

// `VcsLogDirectoryHistoryProvider`: a Log tab with the paths and `HEAD`, Show Only Affected
// Changes on, reused while it has those filters.
test("a_folder_opens_a_log_tab_with_the_path_filter", async () => {
  await panel();

  await showHistoryOf([{ path: "src", dir: true }]);
  await showFolderHistory(["src"]);

  expect(calls("log_history_open")).toHaveLength(0);
  expect(calls("log_view_open")).toHaveLength(1);
  const spec = lastArgs("log_view_open").spec as { filter: { paths: string[]; branches: string[] } };
  expect(spec.filter.paths).toEqual(["src"]);
  expect(spec.filter.branches).toEqual(["HEAD"]);
  expect(tabNames()).toEqual(["Log", "Log: HEAD|src"]);
  const saved = lastArgs("log_options_set").options as Record<string, { showOnlyAffected: boolean }>;
  expect(Object.values(saved).map((options) => options.showOnlyAffected)).toEqual([false, true]);

  await showFolderHistory([""]);
  expect((lastArgs("log_view_open").spec as { filter: { paths: string[] } }).filter.paths).toEqual([]);

  // The match is on the filters a tab has now: once its Branch filter is gone it is another tab.
  await showFolderHistory(["src"]);
  expect(tabNames()).toEqual(["Log", "Log: HEAD|src", "Log: HEAD"]);
  await fireEvent.click(screen.getByRole("tab", { name: "Log: HEAD|src" }));
  await fireEvent.click(await screen.findByRole("button", { name: "Remove Branch filter" }));
  await waitFor(() => expect(tabNames()).toContain("Log: src"));
  await showFolderHistory(["src"]);
  expect(tabNames()).toEqual(["Log", "Log: src", "Log: HEAD", "Log: HEAD|src"]);
});

// Review Focus 5: the Log reloads when the references move, and the history is computed again.
test("a_commit_from_the_terminal_refreshes_an_open_history_tab", async () => {
  await panel();
  await showHistory("src/a.txt");
  expect(calls("log_open")).toHaveLength(1);

  mock({ log_refs_changed: () => true });
  repoChanged();

  await waitFor(() => expect(calls("log_open")).toHaveLength(2));
  await waitFor(() => expect(historyRequests()).toHaveLength(2));
});

test("show_in_git_log_selects_the_commit_in_the_main_log", async () => {
  await panel();
  await showHistory("src/a.txt");

  const menu = await contextMenuOnRow(1);
  await fireEvent.click(menuItem(menu, "Show in Git Log"));

  await waitFor(() =>
    expect(screen.getByRole("tab", { name: "Log" }).getAttribute("aria-selected")).toBe("true"),
  );
  await waitFor(() =>
    expect(
      calls("log_find_rows").some(([, args]) => {
        const { viewId, hashes } = args as { viewId: number; hashes: string[] };
        return viewId === MAIN_VIEW && hashes.includes(HASH_B);
      }),
    ).toBe(true),
  );
});

// `MergedChangeDiffRequestProvider` (decision 8): the merge in the center, its parents around it,
// full hashes in the diff window.
test("a_merge_row_opens_three_sides", async () => {
  await panel();
  await showHistory("src/a.txt");
  await waitFor(() => expect(screen.getByRole("region", { name: "Diff Preview" })).toBeTruthy());

  await fireEvent.click(screen.getByRole("button", { name: "Show Diff" }));

  await waitFor(() => expect(calls("diff_window_open_request")).toHaveLength(1));
  expect(lastArgs("diff_window_open_request").request).toEqual({
    kind: "revisions",
    title: "a.txt (/repo/src)",
    files: [
      {
        left: { revision: HASH_B, path: "src/a.txt", title: HASH_B },
        center: { revision: HASH_D, path: "src/a.txt", title: HASH_D },
        right: { revision: HASH_C, path: "src/old.txt", title: HASH_C },
      },
    ],
  });
  // The preview reads the three versions with short titles.
  await waitFor(() => expect(calls("merge_blocks")).toHaveLength(1));
  expect(screen.getByText("dddddddd")).toBeTruthy();
});

test("the_history_says_241s_texts_when_it_is_empty_or_fails", async () => {
  mock({
    log_history_open: () => deliver({ state: "failed", kind: "failed", text: "fatal: bad revision" }),
  });
  await panel();

  await showHistory("src/a.txt");

  const alert = await screen.findByRole("alert");
  expect(alert.textContent).toContain("Error calculating file history");
  expect(alert.textContent).toContain("fatal: bad revision");
  mock({
    log_history_open: () => {
      const viewId = nextView++;
      historyViews.set(viewId, {});
      return deliver({ value: { viewId, summary: { viewId, count: 0, collapsible: false } } });
    },
  });
  await fireEvent.click(within(alert).getByRole("button", { name: "Refresh" }));
  expect(await screen.findByText("File history")).toBeTruthy();
});

// `GitLogHistoryHandler.throwIfFiltersNotSupported`: a range is no Branch filter of the history;
// the tab says so (`B:1110`) with `Reset filters` (`L:151`), which brings back `HEAD`, and git is
// never asked for every reference instead.
test("a_range_in_the_branch_filter_is_not_supported", async () => {
  await panel();
  await showHistory("src/a.txt");
  const toolbar = screen.getByRole("toolbar", { name: "File History Toolbar" });

  await fireEvent.click(within(toolbar).getByRole("button", { name: "Branch: HEAD" }));
  await fireEvent.click(await screen.findByRole("menuitem", { name: "Select…" }));
  const field = await screen.findByRole("textbox", { name: "Branch" });
  await fireEvent.input(field, { target: { value: "main..other" } });
  await fireEvent.keyDown(field, { key: "Enter", metaKey: true });

  const alert = await screen.findByRole("alert");
  expect(alert.textContent).toContain("Error calculating file history");
  expect(alert.textContent).toContain('Filtering history by "main..other" is not supported');
  expect(historyRequests()).toHaveLength(1);
  await fireEvent.click(within(alert).getByRole("button", { name: "Reset filters" }));
  await waitFor(() => expect(historyRequests()).toHaveLength(2));
  expect(historyRequests()[1]).toEqual({ path: "src/a.txt", revision: null, branches: ["HEAD"] });
});

// `FileHistoryUi.handleCommitNotFound` (`L:165-168`): the revision asked for is not in the history
// of the selected branches; `View and show all branches` clears the filter, `View in Log` shows it in
// the main log.
test("a_commit_not_in_the_history_says_so_with_both_links", async () => {
  await panel();
  await showHistory("src/a.txt");
  // A file's history that is already open, asked to show a commit it does not have.
  const { openLogTab } = await import("./tabKinds");
  await openLogTab("history", "\0src/a.txt", () => ({ name: "", filters: emptyFilters() }), {
    select: HASH_A,
  });

  await waitFor(() => expect(get(notifications)).toHaveLength(1));
  const [warning] = get(notifications);
  expect(warning.kind).toBe("warning");
  expect(warning.title).toBe("aaaaaaaa does not exist in history for a.txt in selected branches");
  expect(warning.actions?.map((action) => action.label)).toEqual([
    "View and show all branches",
    "View in Log",
  ]);
  warning.actions?.[0].run();
  await waitFor(() => expect(historyRequests()).toHaveLength(2));
  expect(historyRequests()[1]).toEqual({ path: "src/a.txt", revision: null, branches: [] });

  warning.actions?.[1].run();
  await waitFor(() =>
    expect(screen.getByRole("tab", { name: "Log" }).getAttribute("aria-selected")).toBe("true"),
  );
  await waitFor(() =>
    expect(
      calls("log_find_rows").some(([, args]) => {
        const { viewId, hashes } = args as { viewId: number; hashes: string[] };
        return viewId === MAIN_VIEW && hashes.includes(HASH_A);
      }),
    ).toBe(true),
  );
});

// `Author, Date, Commit`: the graph is drawn in the Commit column, so the left edge of a row is the
// author's cell and a click there selects the row instead of hitting the graph.
test("the_history_table_puts_author_and_date_before_the_commit_column", async () => {
  await panel();
  await showHistory("src/a.txt");
  const grid = screen.getByRole("grid", { name: "Commits" });
  grid.scrollTop = 0;

  await fireEvent.click(grid, { clientX: 8, clientY: 2 * ROW_HEIGHT + 12 });

  await waitFor(() =>
    expect(
      calls("log_history_change").some(([, args]) =>
        (args as { hashes: string[] }).hashes.includes(HASH_C),
      ),
    ).toBe(true),
  );
});

// `Open on GitHub` is the last item of the menu and the last button of the toolbar
// (`VcsHistoryActionsGroup`, `VcsHistoryActionsGroup.Toolbar`), for the commit of the row; with no
// GitHub repository neither is there.
test("each_menu_has_open_on_github_at_241s_place_file_history", async () => {
  const url = `https://github.com/o/r/commit/${HASH_B}`;
  mock({
    github_links: () => [{ name: "https://github.com/o/r", url }],
    open_url: () => undefined,
  });
  await panel();
  await showHistory("src/a.txt");

  const menu = await contextMenuOnRow(1);
  const labels = [...menu.querySelectorAll('[role="menuitem"]')].map((item) =>
    item.getAttribute("aria-label"),
  );
  expect(labels.at(-1)).toBe("Open on GitHub");
  expect(lastArgs("github_links")).toEqual({ target: { kind: "commit", hash: HASH_B } });
  await fireEvent.click(menuItem(menu, "Open on GitHub"));
  await waitFor(() => expect(lastArgs("open_url")).toEqual({ url }));

  const toolbar = screen.getByRole("toolbar", { name: "File History Toolbar" });
  const buttons = within(toolbar).getAllByRole("button");
  expect(buttons.at(-1)?.getAttribute("title")).toBe("Open on GitHub");
});

test("no_github_remote_hides_the_history_items", async () => {
  await panel();
  await showHistory("src/a.txt");

  const menu = await contextMenuOnRow(1);

  expect(within(menu).queryByRole("menuitem", { name: /^Open on GitHub/ })).toBeNull();
  const toolbar = screen.getByRole("toolbar", { name: "File History Toolbar" });
  expect(within(toolbar).queryByRole("button", { name: "Open on GitHub" })).toBeNull();
});

// `Vcs.FileHistory.PresentationSettings` has the same `Columns` group: `GPG Signature`, hidden by
// default, after the others. The Log's own menu is `ViewOptions.test.ts`'s.
test("the_column_is_hidden_by_default_and_toggles_in_both_columns_menus", async () => {
  await panel();
  await showHistory("src/a.txt");
  const toolbar = await screen.findByRole("toolbar", { name: "File History Toolbar" });

  const columns = async () => {
    await fireEvent.click(within(toolbar).getByRole("button", { name: "View Options" }));
    const menu = screen.getByRole("menu", { name: "View Options" });
    await fireEvent.click(within(menu).getByRole("menuitem", { name: "Columns" }));
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

  await waitFor(() => expect(screen.queryByRole("menu", { name: "Columns" })).toBeNull());
  expect(items(await columns()).at(-1)).toEqual(["GPG Signature", true]);
  const saved = JSON.stringify(lastArgs("log_options_set").options);
  expect(saved).toContain('"columns":["author","date","subject","signature"]');
});
