import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { get } from "svelte/store";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import App from "./App.svelte";
import { closeCommitDialog, openCommitDialog, requestCommit } from "./commit/dialogStore";
import { dismiss, notifications } from "./lib/notifications";
import { requestTab } from "./lib/tabs";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
  emit: vi.fn(async () => {}),
}));

// jsdom has no ResizeObserver, which the changes tree needs.
vi.stubGlobal(
  "ResizeObserver",
  class {
    observe() {}
    unobserve() {}
    disconnect() {}
  },
);

// The diff panel of the dialog measures text with CodeMirror, and jsdom does no layout.
Range.prototype.getClientRects = () => [] as unknown as DOMRectList;
Element.prototype.scrollIntoView = () => {};

function mockCommands(commands: Record<string, () => unknown>) {
  vi.mocked(invoke).mockImplementation(async (command) => {
    const handler = commands[command];
    if (!handler) throw new Error(`unexpected command: ${command}`);
    return handler();
  });
}

beforeEach(() => {
  vi.mocked(invoke).mockReset();
});

// Notifications live in the module: those of one test must not be seen in the next.
afterEach(() => {
  cleanup();
  get(notifications).forEach(({ id }) => dismiss(id));
});

const INFO = {
  path: "/repo/a",
  head: "abcdef1234567890",
  branch: "main",
  recovery: null,
  pending: [],
  state: { operation: null, branch: "main", label: "main", head: "abcdef1234567890" },
};
const LISTS = {
  lists: [{ id: "default", name: "Changes", comment: "" }],
  active: "default",
  assignments: {},
  hunkAssignments: {},
};

const SETTINGS = {
  gitPath: null,
  stagingArea: false,
  warnCrlf: true,
  warnDetached: true,
  warnLargeFiles: true,
  largeFilesLimitMb: 50,
  warnBadFileNames: true,
  signCommits: false,
  signingKey: null,
  useCredentialHelper: true,
  updateMethod: "merge",
  showUpdateDialog: true,
  updatePaths: [],
  incomingCheck: "auto",
  pushAutoUpdate: false,
  previewPushOnCommit: true,
  previewPushProtectedOnly: false,
  mergeAutoApply: false,
  mergeWhitespace: "default",
  cherryPickSuffix: true,
  nonModalCommit: false,
  clearInitialCommitMessage: false,
  showDropCommitDialog: true,
};

const NON_MODAL = { ...SETTINGS, nonModalCommit: true };

function mockApp(commands: Record<string, () => unknown> = {}) {
  mockCommands({
    app_version: () => "0.0.1",
    initial_repo: () => null,
    perf_enabled: () => false,
    show_window: () => undefined,
    repo_status: () => [],
    changelists_get: () => LISTS,
    settings_get: () => SETTINGS,
    git_version: () => "2.38.0",
    commit_warnings: () => [],
    console_lines: () => [],
    log_options_get: () => ({}),
    log_options_set: () => undefined,
    log_recent_filters: () => ({}),
    log_users: () => [],
    log_refs: () => [],
    log_refs_changed: () => false,
    repo_state: () => INFO.state,
    git_menu_enable: () => undefined,
    ...commands,
  });
}

function calls(command: string) {
  return vi.mocked(invoke).mock.calls.filter(([name]) => name === command);
}

async function openLogTab() {
  await fireEvent.click(await screen.findByRole("tab", { name: "Log" }));
}

test("shows the version", async () => {
  mockApp();
  render(App);
  expect(await screen.findByText("0.0.1")).toBeTruthy();
});

test("second_open_while_loading_is_ignored", async () => {
  const pending = new Promise(() => {});
  mockApp({ repo_open: () => pending });
  render(App);
  await screen.findByText("0.0.1");
  const input = screen.getByLabelText("Repository path");
  await fireEvent.input(input, { target: { value: "/repo/a" } });
  await fireEvent.submit(input.closest("form")!);
  await fireEvent.input(input, { target: { value: "/repo/b" } });
  await fireEvent.submit(input.closest("form")!);

  expect(calls("repo_open")).toHaveLength(1);
});

test("shows_repository_error", async () => {
  mockApp({
    initial_repo: () => "/no/repo",
    repo_open: () => {
      throw { kind: "not_a_repository", message: "/no/repo is not a Git repository" };
    },
  });
  render(App);
  expect(await screen.findByText("/no/repo is not a Git repository")).toBeTruthy();
});

test("a_log_error_comes_out_as_a_notification_and_does_not_stay_in_commit", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    log_open: () => {
      throw { kind: "failed", message: "git log failed: corrupted" };
    },
  });
  render(App);
  await openLogTab();

  const notifications = await screen.findByRole("region", { name: "Notifications" });
  expect(within(notifications).getByText("git log failed: corrupted")).toBeTruthy();
  await fireEvent.click(screen.getByRole("tab", { name: "Local Changes" }));
  expect(within(notifications).getByText("git log failed: corrupted")).toBeTruthy();
  expect(document.querySelector("p.error")).toBeNull();
  await fireEvent.click(within(notifications).getByRole("button", { name: "Close" }));
  expect(screen.queryByText("git log failed: corrupted")).toBeNull();
});

test("console_tab_with_show_console_tab", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    console_lines: () => [
      { time: "10:42:07.123", command: true, text: "[a] git fetch origin" },
    ],
  });
  render(App);
  const tab = await screen.findByRole("tab", { name: "Console" });
  expect(tab.getAttribute("aria-selected")).toBe("false");

  requestTab("console");

  await vi.waitFor(() => expect(tab.getAttribute("aria-selected")).toBe("true"));
  expect(screen.getByRole("tab", { name: "Local Changes" }).getAttribute("aria-selected")).toBe("false");
  expect(await screen.findByText("10:42:07.123: [a] git fetch origin")).toBeTruthy();
  await fireEvent.click(screen.getByRole("tab", { name: "Local Changes" }));
  expect(tab.getAttribute("aria-selected")).toBe("false");
  await fireEvent.click(tab);
  expect(tab.getAttribute("aria-selected")).toBe("true");
});

const tabState = (name: string) =>
  screen.getByRole("tab", { name }).getAttribute("aria-selected");

test("in non-modal mode cmd 9 and cmd 0 switch between the Git and Commit windows", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    settings_get: () => NON_MODAL,
    log_open: () => ({ viewId: 1, count: 0 }),
  });
  render(App);
  await screen.findByRole("textbox", { name: "Commit Message" });
  expect(tabState("Commit")).toBe("true");
  expect(tabState("Git")).toBe("false");
  expect(calls("log_open")).toHaveLength(0);

  await fireEvent.keyDown(window, { key: "9", metaKey: true });
  expect(tabState("Git")).toBe("true");
  expect(tabState("Commit")).toBe("false");
  expect(tabState("Log")).toBe("true");
  await fireEvent.click(screen.getByRole("tab", { name: "Console" }));

  await fireEvent.keyDown(window, { key: "0", metaKey: true });
  expect(tabState("Commit")).toBe("true");
  expect(tabState("Git")).toBe("false");
  expect(screen.queryByRole("tab", { name: "Console" })).toBeNull();

  // The window comes back on the tab it had.
  await fireEvent.keyDown(window, { key: "9", metaKey: true });
  expect(tabState("Console")).toBe("true");
});

test("in modal mode the app opens on Local Changes without loading the log", async () => {
  mockApp({ initial_repo: () => "/repo/a", repo_open: () => INFO });
  render(App);

  const tab = await screen.findByRole("tab", { name: "Local Changes" });

  expect(tab.getAttribute("aria-selected")).toBe("true");
  expect(tabState("Git")).toBe("true");
  expect(screen.queryByRole("tab", { name: "Commit" })).toBeNull();
  expect(await screen.findByText("main")).toBeTruthy();
  expect(calls("log_open")).toHaveLength(0);
});

test("the stripe has a single Git button in modal mode", async () => {
  mockApp({ initial_repo: () => "/repo/a", repo_open: () => INFO });
  render(App);
  await screen.findByRole("tab", { name: "Local Changes" });

  const stripe = screen.getByRole("tablist", { name: "Tool windows" });

  expect(within(stripe).getAllByRole("tab").map((button) => button.getAttribute("aria-label"))).toEqual(["Git"]);
});

test("cmd 0 does nothing in modal mode", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    log_open: () => ({ viewId: 1, count: 0 }),
  });
  render(App);
  await openLogTab();
  expect(tabState("Log")).toBe("true");

  await fireEvent.keyDown(window, { key: "0", metaKey: true });

  expect(tabState("Log")).toBe("true");
  expect(tabState("Local Changes")).toBe("false");
});

test("cmd 9 shows the Git window with Local Changes, Log and Console in that order", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    log_open: () => ({ viewId: 1, count: 0 }),
  });
  render(App);
  await screen.findByRole("tab", { name: "Local Changes" });

  await fireEvent.keyDown(window, { key: "9", metaKey: true });

  const tabs = within(screen.getByRole("tablist", { name: "Git" })).getAllByRole("tab");
  expect(tabs.map((tab) => tab.textContent?.trim())).toEqual(["Local Changes", "Log", "Console"]);
  expect(tabState("Local Changes")).toBe("true");
  // Pressed again with the window active, it does nothing.
  await openLogTab();
  await fireEvent.keyDown(window, { key: "9", metaKey: true });
  expect(tabState("Log")).toBe("true");
  expect(tabState("Local Changes")).toBe("false");
});

test("switching the setting moves Local Changes between the Commit and Git windows", async () => {
  await openRepo();
  expect(within(screen.getByRole("tablist", { name: "Git" })).getByRole("tab", { name: "Local Changes" })).toBeTruthy();

  deliver("settings-changed", NON_MODAL);

  await screen.findByRole("tab", { name: "Commit" });
  expect(tabState("Commit")).toBe("true");
  expect(screen.queryByRole("tab", { name: "Local Changes" })).toBeNull();
  await fireEvent.click(screen.getByRole("tab", { name: "Git" }));
  const tabs = within(screen.getByRole("tablist", { name: "Git" })).getAllByRole("tab");
  expect(tabs.map((tab) => tab.textContent?.trim())).toEqual(["Log", "Console"]);

  deliver("settings-changed", SETTINGS);

  await screen.findByRole("tab", { name: "Local Changes" });
  expect(tabState("Local Changes")).toBe("true");
  expect(screen.queryByRole("tab", { name: "Commit" })).toBeNull();
  expect(screen.queryByRole("textbox", { name: "Commit Message" })).toBeNull();
  expect(screen.getByRole("button", { name: "Commit…" })).toBeTruthy();
});

test("a draft and a line selection of the Commit view survive switching to modal and back", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    settings_get: () => NON_MODAL,
    repo_status: () => [CHANGE],
    file_hunks: () => [{ id: "h1", oldStart: 1, oldLines: 1, newStart: 1, newLines: 1 }],
    diff_window_open: () => undefined,
  });
  vi.mocked(emit).mockClear();
  render(App);
  const check = () =>
    screen.getByRole("treeitem", { name: /a\.txt/ }).querySelector("[role=checkbox]");
  await fireEvent.input(await screen.findByRole("textbox", { name: "Commit Message" }), {
    target: { value: "draft" },
  });
  await screen.findByRole("treeitem", { name: /a\.txt/ });
  deliver("selection-changed", {
    path: "a.txt",
    selection: [{ hunk: "h1", lines: [0] }],
    included: true,
    places: {},
  });
  await vi.waitFor(() => expect(check()?.getAttribute("aria-checked")).toBe("mixed"));

  deliver("settings-changed", SETTINGS);
  await screen.findByRole("tab", { name: "Local Changes" });
  expect(screen.queryByRole("textbox", { name: "Commit Message" })).toBeNull();
  deliver("settings-changed", NON_MODAL);

  const field = (await screen.findByRole("textbox", {
    name: "Commit Message",
  })) as HTMLTextAreaElement;
  expect(field.value).toBe("draft");
  await vi.waitFor(() => expect(check()?.getAttribute("aria-checked")).toBe("mixed"));
  await fireEvent.dblClick(screen.getByRole("treeitem", { name: /a\.txt/ }));
  deliver("diff-ready");
  await vi.waitFor(() =>
    expect(vi.mocked(emit)).toHaveBeenCalledWith(
      "selection-state",
      expect.objectContaining({ selections: { "a.txt": [{ hunk: "h1", lines: [0] }] } }),
    ),
  );
});

test("arrow keys in the tab strip of the Git window keep the focus on the strip", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    log_open: () => ({ viewId: 1, count: 0 }),
  });
  render(App);
  await openLogTab();
  const log = screen.getByRole("tab", { name: "Log" });
  log.focus();

  await fireEvent.keyDown(log, { key: "ArrowLeft" });
  await new Promise((resolve) => setTimeout(resolve, 20));

  expect(tabState("Local Changes")).toBe("true");
  expect(document.activeElement).toBe(screen.getByRole("tab", { name: "Local Changes" }));
});

test("in modal mode Local Changes has no checkboxes and no commit message", async () => {
  mockApp({ initial_repo: () => "/repo/a", repo_open: () => INFO, repo_status: () => [CHANGE] });
  render(App);

  await screen.findByRole("treeitem", { name: /a\.txt/ });

  expect(within(screen.getByRole("tree")).queryAllByRole("checkbox")).toEqual([]);
  expect(screen.queryByRole("textbox", { name: "Commit Message" })).toBeNull();
  expect(screen.getByRole("button", { name: "Commit…" })).toBeTruthy();
});

test("in modal mode Commit… and cmd K open the Commit Changes dialog, which keeps the keys while open", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    repo_status: () => [CHANGE],
    merge_message: () => null,
    messages_last: () => null,
    repo_has_commit_hooks: () => false,
    worktree_sides: () => ({
      old: { kind: "text", content: "a\n" },
      new: { kind: "text", content: "b\n" },
      hunks: [],
      editable: true,
      note: null,
      hash: "hash",
      conflicted: false,
    }),
    log_open: () => ({ viewId: 1, count: 0 }),
  });
  render(App);
  await screen.findByRole("treeitem", { name: /a\.txt/ });

  await fireEvent.click(screen.getByRole("button", { name: "Commit…" }));

  const dialog = await screen.findByRole("dialog", { name: "Commit Changes" });
  expect(await within(dialog).findByRole("treeitem", { name: "a.txt" })).toBeTruthy();
  await fireEvent.keyDown(window, { key: "9", code: "Digit9", metaKey: true });
  expect(screen.getAllByRole("dialog", { name: "Commit Changes" })).toHaveLength(1);

  await fireEvent.keyDown(within(dialog).getByRole("textbox", { name: "Commit Message" }), {
    key: "Escape",
  });
  expect(screen.queryByRole("dialog", { name: "Commit Changes" })).toBeNull();

  await openLogTab();
  await fireEvent.keyDown(window, { key: "k", code: "KeyK", metaKey: true });
  expect(await screen.findByRole("dialog", { name: "Commit Changes" })).toBeTruthy();
});

test("log_tab_loads_once", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    log_open: () => ({ viewId: 1, count: 0 }),
  });
  render(App);

  await openLogTab();
  await screen.findByText("This repository has no commits yet");
  await fireEvent.click(screen.getByRole("tab", { name: "Local Changes" }));
  await openLogTab();

  expect(calls("log_open")).toHaveLength(1);
});

test("changing_repository_reloads_the_log", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    log_open: () => ({ viewId: 1, count: 0 }),
  });
  render(App);
  await openLogTab();
  await screen.findByText("This repository has no commits yet");

  const input = screen.getByLabelText("Repository path");
  await fireEvent.input(input, { target: { value: "/repo/b" } });
  await fireEvent.submit(input.closest("form")!);
  await openLogTab();
  await screen.findByText("This repository has no commits yet");

  expect(calls("log_open")).toHaveLength(2);
});

test("in non-modal mode cmd_k_from_the_log_goes_back_to_commit_and_to_the_message", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    settings_get: () => NON_MODAL,
    log_open: () => ({ viewId: 1, count: 0 }),
  });
  render(App);
  await fireEvent.click(await screen.findByRole("tab", { name: "Git" }));
  await openLogTab();
  await screen.findByText("This repository has no commits yet");

  await fireEvent.keyDown(window, { key: "k", code: "KeyK", metaKey: true });

  expect(tabState("Commit")).toBe("true");
  await vi.waitFor(() =>
    expect(document.activeElement).toBe(screen.getByRole("textbox", { name: "Commit Message" })),
  );
});

test("the commit view stays alive when switching tabs", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    settings_get: () => NON_MODAL,
    log_open: () => ({ viewId: 1, count: 0 }),
  });
  render(App);
  const field = (await screen.findByRole("textbox", {
    name: "Commit Message",
  })) as HTMLTextAreaElement;
  await fireEvent.input(field, { target: { value: "draft" } });

  await fireEvent.click(screen.getByRole("tab", { name: "Git" }));
  await fireEvent.click(screen.getByRole("tab", { name: "Commit" }));

  expect(field.isConnected).toBe(true);
  expect(field.value).toBe("draft");
});

test("after_a_commit_the_log_is_reloaded", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    settings_get: () => NON_MODAL,
    log_open: () => ({ viewId: 1, count: 0 }),
    repo_status: () => [
      { path: "a.txt", oldPath: null, state: "modified", staged: false, unstaged: true },
    ],
    commit_changes: () => "hash",
  });
  render(App);
  await fireEvent.click(await screen.findByRole("tab", { name: "Git" }));
  await openLogTab();
  await screen.findByText("This repository has no commits yet");
  await fireEvent.click(screen.getByRole("tab", { name: "Commit" }));

  await fireEvent.input(screen.getByRole("textbox", { name: "Commit Message" }), {
    target: { value: "message" },
  });
  await fireEvent.click(await screen.findByRole("button", { name: "Commit" }));
  await vi.waitFor(() => expect(calls("commit_changes")).toHaveLength(1));
  await screen.findByText("1 file committed: message");

  await fireEvent.click(screen.getByRole("tab", { name: "Git" }));
  await vi.waitFor(() => expect(calls("log_open")).toHaveLength(2));
});

test("cmd_r_rereads_the_log", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    log_open: () => ({ viewId: 1, count: 0, collapsible: true }),
  });
  render(App);
  await openLogTab();
  await screen.findByText("This repository has no commits yet");

  await fireEvent.keyDown(window, { key: "r", code: "KeyR", metaKey: true });

  await vi.waitFor(() => expect(calls("log_open")).toHaveLength(2));
});

test("cmd_r_outside_the_log_does_not_read_it", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    log_open: () => ({ viewId: 1, count: 0, collapsible: true }),
  });
  render(App);
  await screen.findByRole("tab", { name: "Local Changes" });

  await fireEvent.keyDown(window, { key: "r", code: "KeyR", metaKey: true });

  expect(calls("log_open")).toHaveLength(0);
});

test("shows_the_window_when_mounting_finishes", async () => {
  mockApp({ initial_repo: () => "/repo/a", repo_open: () => INFO });
  render(App);

  await vi.waitFor(() => expect(calls("show_window")).toHaveLength(1));

  const order = vi.mocked(invoke).mock.calls.map(([name]) => name);
  expect(order.indexOf("repo_open")).toBeLessThan(order.indexOf("show_window"));
});

test("shows_the_window_even_if_startup_fails", async () => {
  mockApp({
    initial_repo: () => {
      throw new Error("no arguments");
    },
  });
  render(App);

  await vi.waitFor(() => expect(calls("show_window")).toHaveLength(1));
});

test("measure_mode_opens_the_log", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    perf_enabled: () => true,
    log_open: () => ({ viewId: 1, count: 0 }),
  });
  render(App);

  await vi.waitFor(() => expect(calls("log_open")).toHaveLength(1));
});

test("measure_mode_without_frames_reports_anyway", async () => {
  vi.useFakeTimers();
  const frames = vi.spyOn(window, "requestAnimationFrame").mockReturnValue(0);
  try {
    mockApp({
      initial_repo: () => "/repo/a",
      repo_open: () => INFO,
      perf_enabled: () => true,
      log_open: () => ({ viewId: 1, count: 1 }),
      log_rows: () => [],
      perf_report: () => undefined,
    });
    render(App);

    await vi.advanceTimersByTimeAsync(30_000);

    expect(calls("perf_report")).toHaveLength(1);
    expect(JSON.parse((calls("perf_report")[0][1] as { json: string }).json)).toMatchObject({
      windowVisible: false,
    });
  } finally {
    frames.mockRestore();
    vi.useRealTimers();
  }
});

test("measure_mode_carries_a_failure_to_open_the_diff_to_the_report", async () => {
  vi.useFakeTimers();
  const frames = vi.spyOn(window, "requestAnimationFrame").mockReturnValue(0);
  try {
    mockApp({
      initial_repo: () => "/repo/a",
      repo_open: () => INFO,
      perf_enabled: () => true,
      log_open: () => ({ viewId: 1, count: 1 }),
      log_rows: () => [],
      repo_status: () => [{ ...CHANGE }],
      diff_window_open: () => {
        throw { kind: "failed", message: "could not be opened" };
      },
      perf_report: () => undefined,
    });
    render(App);

    await vi.advanceTimersByTimeAsync(30_000);

    expect(JSON.parse((calls("perf_report")[0][1] as { json: string }).json).diff).toEqual({
      path: "a.txt",
      error: "could not be opened",
    });
  } finally {
    frames.mockRestore();
    vi.useRealTimers();
  }
});

test("measure_mode_with_a_late_first_frame_does_not_switch_to_measuring_without_a_window", async () => {
  vi.useFakeTimers();
  const frames = vi.spyOn(window, "requestAnimationFrame").mockImplementation((run) => {
    setTimeout(() => run(0), 4_000);
    return 0;
  });
  const visible = vi.spyOn(document, "visibilityState", "get").mockReturnValue("visible");
  try {
    mockApp({
      initial_repo: () => "/repo/a",
      repo_open: () => INFO,
      perf_enabled: () => true,
      log_open: () => ({ viewId: 1, count: 1 }),
      log_rows: () => [],
      perf_report: () => undefined,
    });
    render(App);

    await vi.advanceTimersByTimeAsync(30_000);

    expect(calls("perf_report")).toHaveLength(0);
  } finally {
    visible.mockRestore();
    frames.mockRestore();
    vi.useRealTimers();
  }
});

const CHANGE = { path: "a.txt", oldPath: null, state: "modified", staged: false, unstaged: true };
// How a pending backend call ends.
type Settle = { resolve: (value: unknown) => void; reject: (failure: unknown) => void };

test("show_diff_opens_the_diff_window_and_tells_it_what_is_chosen", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    settings_get: () => NON_MODAL,
    repo_status: () => [CHANGE],
    diff_window_open: () => undefined,
  });
  vi.mocked(emit).mockClear();
  render(App);

  await fireEvent.dblClick(await screen.findByRole("treeitem", { name: /a\.txt/ }));

  await vi.waitFor(() => expect(calls("diff_window_open")).toHaveLength(1));
  expect(calls("diff_window_open")[0][1]).toEqual({ path: "a.txt" });
  await fireEvent.click(screen.getByRole("treeitem", { name: /a\.txt/ }).querySelector("[role=checkbox]")!);
  await vi.waitFor(() =>
    expect(vi.mocked(emit)).toHaveBeenCalledWith(
      "selection-state",
      expect.objectContaining({ selections: {} }),
    ),
  );
});

test("in modal mode a double click in Local Changes opens the diff window without line checkboxes", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    repo_status: () => [CHANGE],
    diff_window_open: () => undefined,
  });
  vi.mocked(emit).mockClear();
  render(App);

  await fireEvent.dblClick(await screen.findByRole("treeitem", { name: /a\.txt/ }));

  await vi.waitFor(() => expect(calls("diff_window_open")).toHaveLength(1));
  expect(calls("diff_window_open")[0][1]).toEqual({ path: "a.txt" });
  deliver("diff-ready");
  await vi.waitFor(() =>
    expect(vi.mocked(emit)).toHaveBeenCalledWith("selection-state", {
      owner: "none",
      included: [],
      selections: {},
      lists: null,
    }),
  );
});

test("switching to modal mode takes the line checkboxes away from an open diff window", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    settings_get: () => NON_MODAL,
    repo_status: () => [CHANGE],
    diff_window_open: () => undefined,
  });
  vi.mocked(emit).mockClear();
  render(App);
  await fireEvent.dblClick(await screen.findByRole("treeitem", { name: /a\.txt/ }));
  await vi.waitFor(() => expect(calls("diff_window_open")).toHaveLength(1));
  deliver("diff-ready");
  await vi.waitFor(() =>
    expect(vi.mocked(emit)).toHaveBeenCalledWith(
      "selection-state",
      expect.objectContaining({ owner: "commit-view", included: ["a.txt"] }),
    ),
  );
  vi.mocked(emit).mockClear();

  deliver("settings-changed", SETTINGS);

  await vi.waitFor(() =>
    expect(vi.mocked(emit)).toHaveBeenCalledWith(
      "selection-state",
      expect.objectContaining({ owner: "none", included: [], selections: {} }),
    ),
  );
  // The window asks again when it is reopened: what the Commit view kept is not sent.
  vi.mocked(emit).mockClear();
  deliver("diff-ready");
  await vi.waitFor(() =>
    expect(vi.mocked(emit)).toHaveBeenCalledWith(
      "selection-state",
      expect.objectContaining({ owner: "none" }),
    ),
  );
  expect(vi.mocked(emit)).not.toHaveBeenCalledWith(
    "selection-state",
    expect.objectContaining({ owner: "commit-view" }),
  );
  // Nobody listens: a change that still arrives from the window does not come back.
  deliver("selection-changed", {
    path: "a.txt",
    selection: [{ hunk: "h1", lines: [0] }],
    included: true,
    places: {},
  });
  await new Promise((resolve) => setTimeout(resolve, 20));
  expect(vi.mocked(emit)).not.toHaveBeenCalledWith(
    "selection-state",
    expect.objectContaining({ selections: { "a.txt": expect.anything() } }),
  );
});

test("a diff window opened from the Commit Changes dialog follows the dialog and is cleared when it closes", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    repo_status: () => [CHANGE],
    merge_message: () => null,
    messages_last: () => null,
    repo_has_commit_hooks: () => false,
    diff_window_open: () => undefined,
    file_hunks: () => [{ id: "h1", oldStart: 1, oldLines: 1, newStart: 1, newLines: 1 }],
    worktree_sides: () => ({
      old: { kind: "text", content: "a\n" },
      new: { kind: "text", content: "b\n" },
      hunks: [],
      editable: true,
      note: null,
      hash: "hash",
      conflicted: false,
    }),
  });
  vi.mocked(emit).mockClear();
  render(App);
  await screen.findByRole("treeitem", { name: /a\.txt/ });
  await fireEvent.click(screen.getByRole("button", { name: "Commit…" }));
  const dialog = await screen.findByRole("dialog", { name: "Commit Changes" });

  await fireEvent.dblClick(await within(dialog).findByRole("treeitem", { name: "a.txt" }));

  await vi.waitFor(() => expect(calls("diff_window_open")).toHaveLength(1));
  deliver("diff-ready");
  await vi.waitFor(() =>
    expect(vi.mocked(emit)).toHaveBeenCalledWith(
      "selection-state",
      expect.objectContaining({ owner: "dialog", included: ["a.txt"], selections: {} }),
    ),
  );

  // A line checkbox of the window reaches the dialog, which publishes it back.
  deliver("selection-changed", {
    path: "a.txt",
    selection: [{ hunk: "h1", lines: [0] }],
    included: true,
    places: {},
  });
  await vi.waitFor(() =>
    expect(vi.mocked(emit)).toHaveBeenCalledWith(
      "selection-state",
      expect.objectContaining({
        owner: "dialog",
        selections: { "a.txt": [{ hunk: "h1", lines: [0] }] },
      }),
    ),
  );
  await vi.waitFor(() =>
    expect(
      within(dialog).getByRole("treeitem", { name: "a.txt" }).querySelector("[role=checkbox]")?.getAttribute("aria-checked"),
    ).toBe("mixed"),
  );

  vi.mocked(emit).mockClear();
  await fireEvent.keyDown(within(dialog).getByRole("textbox", { name: "Commit Message" }), {
    key: "Escape",
  });

  await vi.waitFor(() =>
    expect(vi.mocked(emit)).toHaveBeenCalledWith("selection-state", {
      owner: "none",
      included: [],
      selections: {},
      lists: null,
    }),
  );
});

const DIALOG_COMMANDS = {
  merge_message: () => null,
  messages_last: () => null,
  repo_has_commit_hooks: () => false,
  diff_window_open: () => undefined,
  file_hunks: () => [{ id: "h1", oldStart: 1, oldLines: 1, newStart: 1, newLines: 1 }],
  worktree_sides: () => ({
    old: { kind: "text", content: "a\n" },
    new: { kind: "text", content: "b\n" },
    hunks: [],
    editable: true,
    note: null,
    hash: "hash",
    conflicted: false,
  }),
};
const LINE_CHANGE = {
  path: "a.txt",
  selection: [{ hunk: "h1", lines: [0] }],
  included: true,
  places: {},
};

test("a line chosen in the diff window comes back to the Commit view when the dialog opened over it closes", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    settings_get: () => NON_MODAL,
    repo_status: () => [CHANGE],
    ...DIALOG_COMMANDS,
  });
  vi.mocked(emit).mockClear();
  render(App);
  await fireEvent.dblClick(await screen.findByRole("treeitem", { name: /a\.txt/ }));
  await vi.waitFor(() => expect(calls("diff_window_open")).toHaveLength(1));
  deliver("diff-ready");
  deliver("selection-changed", LINE_CHANGE);
  const chosen = { a: [{ hunk: "h1", lines: [0] }] };
  await vi.waitFor(() =>
    expect(vi.mocked(emit)).toHaveBeenCalledWith(
      "selection-state",
      expect.objectContaining({ owner: "commit-view", selections: { "a.txt": chosen.a } }),
    ),
  );

  // The dialog is not reachable from the non-modal entry points: it opens as the executors of part 3 do.
  openCommitDialog({ request: { kind: "project", paths: [], lists: [] }, executor: "commit" });
  const dialog = await screen.findByRole("dialog", { name: "Commit Changes" });
  await vi.waitFor(() =>
    expect(vi.mocked(emit)).toHaveBeenCalledWith(
      "selection-state",
      expect.objectContaining({ owner: "dialog", selections: {} }),
    ),
  );
  vi.mocked(emit).mockClear();
  await fireEvent.keyDown(within(dialog).getByRole("textbox", { name: "Commit Message" }), {
    key: "Escape",
  });

  await vi.waitFor(() =>
    expect(vi.mocked(emit)).toHaveBeenCalledWith(
      "selection-state",
      expect.objectContaining({ owner: "commit-view", selections: { "a.txt": chosen.a } }),
    ),
  );
});

test("the commit of the dialog runs after the dialog has closed", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    repo_status: () => [CHANGE],
    changelists_set_comment: () => LISTS,
    commit_changes: () => "hash",
    ...DIALOG_COMMANDS,
  });
  render(App);
  await screen.findByRole("treeitem", { name: /a\.txt/ });
  await fireEvent.click(screen.getByRole("button", { name: "Commit…" }));
  const dialog = await screen.findByRole("dialog", { name: "Commit Changes" });
  await within(dialog).findByRole("treeitem", { name: "a.txt" });
  const message = within(dialog).getByRole("textbox", { name: "Commit Message" });
  await fireEvent.input(message, { target: { value: "Message" } });

  await fireEvent.keyDown(message, { key: "Enter", metaKey: true });

  await vi.waitFor(() => expect(calls("commit_changes")).toHaveLength(1));
  expect(screen.queryByRole("dialog", { name: "Commit Changes" })).toBeNull();
  await vi.waitFor(() =>
    expect(get(notifications).map(({ title }) => title)).toEqual(["1 file committed: Message"]),
  );
});

test("a line lost by a commit that failed after the dialog closed reaches the diff window", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    repo_status: () => [CHANGE],
    ...DIALOG_COMMANDS,
    changelists_set_comment: () => LISTS,
    commit_changes_selected: () => {
      throw { kind: "selection_lost", message: "x", details: { path: "a.txt", hunks: ["h1"] } };
    },
  });
  vi.mocked(emit).mockClear();
  render(App);
  await screen.findByRole("treeitem", { name: /a\.txt/ });
  await fireEvent.click(screen.getByRole("button", { name: "Commit…" }));
  const dialog = await screen.findByRole("dialog", { name: "Commit Changes" });
  await fireEvent.dblClick(await within(dialog).findByRole("treeitem", { name: "a.txt" }));
  await vi.waitFor(() => expect(calls("diff_window_open")).toHaveLength(1));
  deliver("diff-ready");
  deliver("selection-changed", LINE_CHANGE);
  await vi.waitFor(() =>
    expect(
      within(dialog).getByRole("treeitem", { name: "a.txt" }).querySelector("[role=checkbox]")?.getAttribute("aria-checked"),
    ).toBe("mixed"),
  );
  const message = within(dialog).getByRole("textbox", { name: "Commit Message" });
  await fireEvent.input(message, { target: { value: "Message" } });

  await fireEvent.keyDown(message, { key: "Enter", metaKey: true });

  await vi.waitFor(() =>
    expect(vi.mocked(emit)).toHaveBeenCalledWith(
      "selection-lost",
      expect.objectContaining({ path: "a.txt", hunks: ["h1"] }),
    ),
  );
  expect(screen.queryByRole("dialog", { name: "Commit Changes" })).toBeNull();
});

const FIXUP_REQUEST = {
  request: { kind: "project" as const, paths: ["a.txt"], lists: ["default"] },
  prefilled: "fixup! one",
  rebaseFrom: "abc~",
  head: INFO.head,
};

// Opens the dialog of a fixup and picks `Commit and Rebase…`.
async function commitAndRebase() {
  requestCommit(FIXUP_REQUEST);
  const dialog = await screen.findByRole("dialog", { name: "Commit Changes" });
  await within(dialog).findByRole("treeitem", { name: "a.txt" });
  await vi.waitFor(() =>
    expect((within(dialog).getByRole("textbox", { name: "Commit Message" }) as HTMLTextAreaElement).value).toBe(
      "fixup! one",
    ),
  );
  await fireEvent.click(within(dialog).getByRole("button", { name: "Other Commit Actions" }));
  await fireEvent.click(screen.getByRole("menuitem", { name: /Commit and Rebase…/ }));
}

test("Commit and Rebase… of the dialog rebases from the base once the commit after closing is made", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    repo_status: () => [CHANGE],
    changelists_set_comment: () => LISTS,
    commit_changes: () => "hash",
    rewrite_autosquash: () => 7,
    ...DIALOG_COMMANDS,
  });
  render(App);
  await screen.findByRole("treeitem", { name: /a\.txt/ });

  await commitAndRebase();

  await vi.waitFor(() => expect(calls("rewrite_autosquash")).toHaveLength(1));
  expect(screen.queryByRole("dialog", { name: "Commit Changes" })).toBeNull();
  expect(calls("commit_changes")).toHaveLength(1);
  expect(calls("rewrite_autosquash")[0][1]).toEqual({ base: "abc~" });
});

test.each([
  ["made", (finish: Settle) => finish.resolve("hash"), "1 file committed: fixup! one"],
  ["failed", (finish: Settle) => finish.reject({ kind: "git", message: "hook failed" }), "hook failed"],
])(
  "a dialog commit %s after another repository was opened leaves that repository alone",
  async (_, settle, notice) => {
    let finish: Settle = { resolve: () => {}, reject: () => {} };
    let opened = 0;
    mockApp({
      initial_repo: () => "/repo/a",
      repo_open: () => (++opened === 1 ? INFO : { ...INFO, path: "/repo/b" }),
      repo_status: () => [CHANGE],
      changelists_set_comment: () => LISTS,
      commit_changes: () => new Promise((resolve, reject) => (finish = { resolve, reject })),
      rewrite_autosquash: () => 7,
      ...DIALOG_COMMANDS,
    });
    render(App);
    await screen.findByRole("treeitem", { name: /a\.txt/ });
    await commitAndRebase();
    await vi.waitFor(() => expect(calls("commit_changes")).toHaveLength(1));

    const input = screen.getByLabelText("Repository path");
    await fireEvent.input(input, { target: { value: "/repo/b" } });
    await fireEvent.submit(input.closest("form")!);
    await vi.waitFor(() => expect(calls("repo_open")).toHaveLength(2));
    await new Promise((resolve) => setTimeout(resolve, 20));
    settle(finish);

    // The outcome is still notified; the draft of repository B, its lists and its HEAD are not touched.
    await vi.waitFor(() => expect(get(notifications).map(({ title }) => title)).toContain(notice));
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(calls("changelists_set_comment")).toHaveLength(0);
    expect(calls("rewrite_autosquash")).toHaveLength(0);
  },
);

test("a commit of the dialog that fails after closing does not push", async () => {
  openWithBranches({
    ...PUSH_COMMANDS,
    ...DIALOG_COMMANDS,
    repo_status: () => [CHANGE],
    changelists_set_comment: () => LISTS,
    commit_changes: () => {
      throw { kind: "git", message: "hook failed" };
    },
  });
  render(App);
  await screen.findByRole("treeitem", { name: /a\.txt/ });
  await fireEvent.click(screen.getByRole("button", { name: "Commit…" }));
  const dialog = await screen.findByRole("dialog", { name: "Commit Changes" });
  await within(dialog).findByRole("treeitem", { name: "a.txt" });
  await fireEvent.input(within(dialog).getByRole("textbox", { name: "Commit Message" }), {
    target: { value: "Message" },
  });

  await fireEvent.click(within(dialog).getByRole("button", { name: "Other Commit Actions" }));
  await fireEvent.click(screen.getByRole("menuitem", { name: /Commit and Push…/ }));

  await vi.waitFor(() => expect(get(notifications).map(({ title }) => title)).toContain("hook failed"));
  await new Promise((resolve) => setTimeout(resolve, 20));
  expect(calls("commit_changes")).toHaveLength(1);
  expect(screen.queryByRole("dialog", { name: "Push Commits to rama" })).toBeNull();
  expect(calls("push_info")).toHaveLength(0);
});

test("cmd K opens the dialog from Local Changes and from the Log in modal mode", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    repo_status: () => [CHANGE],
    log_open: () => ({ viewId: 1, count: 0 }),
    ...DIALOG_COMMANDS,
  });
  render(App);
  await fireEvent.click(await screen.findByRole("tree"));
  await screen.findByRole("treeitem", { name: /a\.txt/ });

  await fireEvent.keyDown(screen.getByRole("tree"), { key: "k", code: "KeyK", metaKey: true });
  const first = await screen.findByRole("dialog", { name: "Commit Changes" });
  await fireEvent.keyDown(within(first).getByRole("textbox", { name: "Commit Message" }), {
    key: "Escape",
  });
  expect(screen.queryByRole("dialog", { name: "Commit Changes" })).toBeNull();

  await openLogTab();
  await fireEvent.keyDown(window, { key: "k", code: "KeyK", metaKey: true });

  expect(await screen.findByRole("dialog", { name: "Commit Changes" })).toBeTruthy();
});

test("in non-modal mode cmd K still focuses the Commit tool window message", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    settings_get: () => NON_MODAL,
    log_open: () => ({ viewId: 1, count: 0 }),
  });
  render(App);
  await fireEvent.click(await screen.findByRole("tab", { name: "Git" }));
  await openLogTab();
  await screen.findByText("This repository has no commits yet");

  await fireEvent.keyDown(window, { key: "k", code: "KeyK", metaKey: true });

  expect(screen.queryByRole("dialog", { name: "Commit Changes" })).toBeNull();
  expect(tabState("Commit")).toBe("true");
  await vi.waitFor(() =>
    expect(document.activeElement).toBe(screen.getByRole("textbox", { name: "Commit Message" })),
  );
});

test("cmd alt K in modal mode opens the dialog", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    repo_status: () => [CHANGE],
    log_open: () => ({ viewId: 1, count: 0 }),
    ...DIALOG_COMMANDS,
  });
  render(App);
  await screen.findByRole("treeitem", { name: /a\.txt/ });
  await openLogTab();

  await fireEvent.keyDown(window, { key: "˚", code: "KeyK", metaKey: true, altKey: true });

  expect(await screen.findByRole("dialog", { name: "Commit Changes" })).toBeTruthy();
});

test("a fixup request in modal mode opens the dialog with the message and Commit and Rebase…", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    repo_status: () => [CHANGE],
    ...DIALOG_COMMANDS,
  });
  render(App);
  await screen.findByRole("treeitem", { name: /a\.txt/ });

  requestCommit({
    request: { kind: "project", paths: ["a.txt"], lists: ["default"] },
    prefilled: "fixup! one",
    rebaseFrom: "abc~",
    head: INFO.head,
  });

  const dialog = await screen.findByRole("dialog", { name: "Commit Changes" });
  await vi.waitFor(() =>
    expect((within(dialog).getByRole("textbox", { name: "Commit Message" }) as HTMLTextAreaElement).value).toBe(
      "fixup! one",
    ),
  );
  await fireEvent.click(within(dialog).getByRole("button", { name: "Other Commit Actions" }));
  expect(screen.getByRole("menuitem", { name: /Commit and Rebase…/ })).toBeTruthy();
  closeCommitDialog();
});

test("a fixup request in non-modal mode goes to the Commit view", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    settings_get: () => NON_MODAL,
    repo_status: () => [CHANGE],
  });
  render(App);
  await screen.findByRole("treeitem", { name: /a\.txt/ });

  requestCommit({
    request: { kind: "project", paths: ["a.txt"], lists: ["default"] },
    prefilled: "squash! one",
    rebaseFrom: "abc~",
    head: INFO.state.head,
  });

  await screen.findByRole("button", { name: "Commit and Rebase…" });
  expect(screen.queryByRole("dialog", { name: "Commit Changes" })).toBeNull();
  expect((screen.getByRole("textbox", { name: "Commit Message" }) as HTMLTextAreaElement).value).toBe(
    "squash! one",
  );
});

test("in non-modal mode the Commit view still owns the line checkboxes of the diff window", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    settings_get: () => NON_MODAL,
    repo_status: () => [CHANGE],
    diff_window_open: () => undefined,
  });
  vi.mocked(emit).mockClear();
  render(App);
  await fireEvent.dblClick(await screen.findByRole("treeitem", { name: /a\.txt/ }));
  await vi.waitFor(() => expect(calls("diff_window_open")).toHaveLength(1));

  deliver("diff-ready");

  await vi.waitFor(() =>
    expect(vi.mocked(emit)).toHaveBeenCalledWith(
      "selection-state",
      expect.objectContaining({ owner: "commit-view" }),
    ),
  );
});

test("without_a_diff_window_nothing_is_emitted", async () => {
  mockApp({ initial_repo: () => "/repo/a", repo_open: () => INFO, repo_status: () => [CHANGE] });
  vi.mocked(emit).mockClear();
  render(App);

  await screen.findByRole("treeitem", { name: /a\.txt/ });

  expect(emit).not.toHaveBeenCalled();
});

test("the_selection_of_the_diff_window_reaches_the_commit_view", async () => {
  const handlers = new Map<string, (event: { payload: unknown }) => void>();
  vi.mocked(listen).mockImplementation(async (event, handler) => {
    handlers.set(event as string, handler as never);
    return () => {};
  });
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    settings_get: () => NON_MODAL,
    repo_status: () => [CHANGE],
    commit_changes_selected: () => "hash",
  });
  render(App);
  const row = await screen.findByRole("treeitem", { name: /a\.txt/ });

  handlers.get("selection-changed")?.({
    payload: {
      path: "a.txt",
      selection: [{ hunk: "h1", lines: [0] }],
      included: true,
      places: {},
    },
  });
  await vi.waitFor(() =>
    expect(row.querySelector("[role=checkbox]")?.getAttribute("aria-checked")).toBe("true"),
  );
  await fireEvent.input(screen.getByRole("textbox", { name: "Commit Message" }), {
    target: { value: "partial" },
  });
  await fireEvent.click(screen.getByRole("button", { name: "Commit" }));

  await vi.waitFor(() => expect(calls("commit_changes_selected")).toHaveLength(1));
});

test("if_another_repository_cannot_be_opened_the_current_one_stays", async () => {
  let opened = 0;
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => {
      if (opened++ === 0) return INFO;
      throw { kind: "failed", message: "There are unsaved changes" };
    },
    settings_get: () => NON_MODAL,
    repo_status: () => [CHANGE],
  });
  render(App);
  const message = await screen.findByRole("textbox", { name: "Commit Message" });
  await fireEvent.input(message, { target: { value: "half done" } });

  await fireEvent.submit(screen.getByLabelText("Repository path").closest("form")!);

  expect(await screen.findByText("There are unsaved changes")).toBeTruthy();
  expect(
    (screen.getByRole("textbox", { name: "Commit Message" }) as HTMLTextAreaElement).value,
  ).toBe("half done");
});

test("if_closing_is_not_possible_the_main_window_says_so", async () => {
  const handlers = new Map<string, (event: { payload: unknown }) => void>();
  vi.mocked(listen).mockImplementation(async (event, handler) => {
    handlers.set(event as string, handler as never);
    return () => {};
  });
  mockApp();
  render(App);
  await screen.findByText("0.0.1");

  handlers.get("leave-refused")?.({ payload: "There are unsaved changes" });

  expect(await screen.findByText("There are unsaved changes")).toBeTruthy();
});

// What the backend notifies the windows about.
function deliver(event: string, payload?: unknown) {
  for (const [name, handler] of vi.mocked(listen).mock.calls) {
    if (name === event) handler({ event: name, id: 0, payload } as never);
  }
}

async function openRepo() {
  mockApp({ repo_open: () => INFO, initial_repo: () => "/repo/a" });
  render(App);
  await screen.findByRole("treeitem", { name: /^Changes/ });
}

test("⌘, opens the settings even without a repository", async () => {
  mockApp();
  render(App);
  await screen.findByText("0.0.1");

  await fireEvent.keyDown(window, { key: ",", metaKey: true });

  expect(await screen.findByRole("dialog", { name: "Settings" })).toBeTruthy();
});

test("the Settings menu opens the settings", async () => {
  mockApp();
  render(App);
  await screen.findByText("0.0.1");

  deliver("open-settings");

  expect(await screen.findByRole("dialog", { name: "Settings" })).toBeTruthy();
});

test("saved settings apply without restarting", async () => {
  await openRepo();

  deliver("settings-changed", { ...SETTINGS, nonModalCommit: true, stagingArea: true });

  expect(await screen.findByRole("treeitem", { name: /^Staged/ })).toBeTruthy();
  expect(screen.queryByRole("treeitem", { name: /^Changes/ })).toBeNull();
});

test("staging area saved while the commit interface is modal is not in effect", async () => {
  await openRepo();

  deliver("settings-changed", { ...SETTINGS, nonModalCommit: false, stagingArea: true });

  expect(await screen.findByRole("treeitem", { name: /^Changes/ })).toBeTruthy();
  expect(screen.queryByRole("treeitem", { name: /^Staged/ })).toBeNull();
});

test("with staging area saved it starts without changelists", async () => {
  mockApp({
    repo_open: () => INFO,
    initial_repo: () => "/repo/a",
    settings_get: () => ({ ...SETTINGS, nonModalCommit: true, stagingArea: true }),
  });
  render(App);

  await screen.findByRole("treeitem", { name: /^Staged/ });

  expect(calls("changelists_get")).toHaveLength(0);
});

test("changing the git path refreshes what is shown", async () => {
  await openRepo();

  deliver("settings-changed", { ...SETTINGS, gitPath: "/other/git" });

  await waitFor(() => expect(emit).toHaveBeenCalledWith("repo-changed"));
});

test("the_toolbar_branch_follows_the_repository_without_remounting_the_views", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    repo_state: () => ({ ...INFO.state, branch: "another", label: "another" }),
  });
  render(App);
  await screen.findByRole("tab", { name: "Log" });
  expect(screen.getByText("main")).toBeTruthy();

  deliver("repo-changed");

  expect(await screen.findByText("another")).toBeTruthy();
  expect(calls("repo_open")).toHaveLength(1);
});

const REBASING = {
  operation: "rebasing",
  branch: "main",
  label: "Rebasing main",
  head: "abcdef1234567890",
};

test("widget_shows_rebasing_main", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => ({ ...INFO, state: REBASING }),
  });
  render(App);

  expect(await screen.findByRole("button", { name: "Git Branch: Rebasing main" })).toBeTruthy();
});

test("the_widget_goes_from_one_operation_to_another_with_repo_changed", async () => {
  let state: unknown = REBASING;
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => ({ ...INFO, state: REBASING }),
    repo_state: () => state,
  });
  render(App);
  await screen.findByRole("button", { name: "Git Branch: Rebasing main" });

  state = INFO.state;
  deliver("repo-changed");

  expect(await screen.findByRole("button", { name: "Git Branch: main" })).toBeTruthy();
  expect(calls("git_menu_enable").map(([, args]) => args)).toEqual([
    {
      ids: ["Git.MainMenu.RebaseActions", "Git.Rebase.Abort", "Git.Rebase.Continue", "Git.Rebase.Skip", "Git.Fetch"],
    },
    { ids: ["Git.Merge", "Git.Rebase", "Git.Fetch"] },
  ]);
});

test("opening_a_half_done_repository_shows_the_state", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => ({ ...INFO, state: { ...REBASING, label: "Rebasing another", branch: "another" } }),
  });
  render(App);

  expect(await screen.findByRole("button", { name: "Git Branch: Rebasing another" })).toBeTruthy();
  // And the usual card, with Abort, even though nothing is saved.
  expect(await screen.findByText("Cannot complete update")).toBeTruthy();
  expect(screen.getByRole("button", { name: "Abort" })).toBeTruthy();
  expect(calls("git_menu_enable")[0][1]).toEqual({
    ids: ["Git.MainMenu.RebaseActions", "Git.Rebase.Abort", "Git.Rebase.Continue", "Git.Rebase.Skip", "Git.Fetch"],
  });
});

test("the_git_menu_abort_merge_asks_for_confirmation_and_aborts", async () => {
  const merging = { ...INFO.state, operation: "merging", label: "Merging main" };
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => ({ ...INFO, state: merging }),
    repo_state: () => merging,
    flush: () => undefined,
  });
  render(App);
  await screen.findByRole("button", { name: "Git Branch: Merging main" });

  deliver("git-action", "Git.Merge.Abort");

  const dialog = await screen.findByRole("dialog", { name: "Abort Merge" });
  expect(within(dialog).getByText("Abort merge?")).toBeTruthy();
  await fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
  expect(calls("operation_abort")).toHaveLength(0);
});

test("new_branch_during_a_rebase_does_not_suggest_the_rebased_branch", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => ({ ...INFO, state: REBASING }),
    branch_list: () => ({ local: [], remote: [], recent: [], current: null, previous: null }),
  });
  render(App);
  await screen.findByRole("button", { name: "Git Branch: Rebasing main" });

  deliver("git-action", "Git.CreateNewBranch");

  const dialog = await screen.findByRole("dialog", { name: "Create New Branch" });
  expect((within(dialog).getAllByRole("textbox")[0] as HTMLInputElement).value).toBe("");
});

test("new_branch_without_an_operation_still_suggests_the_current_branch", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    branch_list: () => ({ local: [], remote: [], recent: [], current: null, previous: null }),
  });
  render(App);
  await screen.findByRole("button", { name: "Git Branch: main" });

  deliver("git-action", "Git.CreateNewBranch");

  const dialog = await screen.findByRole("dialog", { name: "Create New Branch" });
  expect((within(dialog).getAllByRole("textbox")[0] as HTMLInputElement).value).toBe("main");
});

const openWithBranches = (commands: Record<string, () => unknown> = {}) =>
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    branch_list: () => ({
      local: [{ name: "main", tracked: null }],
      remote: [],
      recent: [],
      current: "main",
      previous: null,
    }),
    branch_incoming_outgoing: () => ({}),
    ...commands,
  });

test("the_branch_widget_opens_and_closes_the_popup", async () => {
  openWithBranches();
  render(App);
  const widget = await screen.findByRole("button", { name: "Git Branch: main" });

  await fireEvent.click(widget);
  expect(await screen.findByRole("tree", { name: "Branches Tree" })).toBeTruthy();

  await fireEvent.click(widget);
  expect(screen.queryByRole("tree", { name: "Branches Tree" })).toBeNull();
});

test("on_mac_git_branches_has_no_shortcut", async () => {
  openWithBranches();
  render(App);
  await screen.findByRole("button", { name: "Git Branch: main" });

  await fireEvent.keyDown(window, { key: "~", code: "Backquote", ctrlKey: true, shiftKey: true });
  await new Promise((resolve) => setTimeout(resolve, 20));

  expect(screen.queryByRole("tree", { name: "Branches Tree" })).toBeNull();
});

test("commit_from_the_popup_goes_to_the_commit_view", async () => {
  openWithBranches();
  render(App);
  await fireEvent.click(await screen.findByRole("button", { name: "Git Branch: main" }));

  const row = await screen.findByRole("treeitem", { name: "Commit…" });
  expect(row.getAttribute("aria-disabled")).toBe("false");
});

const PUSH_COMMANDS = {
  push_info: () => ({
    repository: "rama",
    branch: "main",
    head: "a".repeat(40),
    remotes: ["origin"],
    remoteBranches: ["origin/main"],
    target: { remote: "origin", branch: "main", isNew: false },
    prePushHook: false,
    protectedBranches: ["master", "main"],
    forcePushTargets: [],
  }),
  push_tags_get: () => null,
  push_outgoing: () => 1,
};

test("cmd_alt_k_commits_and_opens_the_push_dialog", async () => {
  openWithBranches({
    ...PUSH_COMMANDS,
    settings_get: () => NON_MODAL,
    repo_status: () => [{ path: "a.txt", state: "modified", staged: false, unstaged: true }],
    commit_changes: () => "hash",
  });
  render(App);
  await screen.findByRole("treeitem", { name: "a.txt" });
  const field = screen.getByRole("textbox", { name: "Commit Message" });

  await fireEvent.input(field, { target: { value: "message" } });
  await fireEvent.keyDown(field, { key: "˚", code: "KeyK", metaKey: true, altKey: true });

  expect(await screen.findByRole("dialog", { name: "Push Commits to rama" })).toBeTruthy();
  expect(calls("commit_changes")).toHaveLength(1);
});

test("cmd_shift_k_opens_the_dialog", async () => {
  openWithBranches(PUSH_COMMANDS);
  render(App);
  await screen.findByRole("button", { name: "Git Branch: main" });

  await fireEvent.keyDown(window, { key: "K", code: "KeyK", metaKey: true, shiftKey: true });

  expect(await screen.findByRole("dialog", { name: "Push Commits to rama" })).toBeTruthy();
  expect(calls("push_info")).toEqual([["push_info", { branch: null }]]);
});

test("push_from_the_popup_opens_the_dialog", async () => {
  openWithBranches(PUSH_COMMANDS);
  render(App);
  await fireEvent.click(await screen.findByRole("button", { name: "Git Branch: main" }));

  await fireEvent.click(await screen.findByRole("treeitem", { name: "Push…" }));

  expect(await screen.findByRole("dialog", { name: "Push Commits to rama" })).toBeTruthy();
});

test("the_git_menu_also_opens_push", async () => {
  openWithBranches(PUSH_COMMANDS);
  render(App);
  await screen.findByRole("button", { name: "Git Branch: main" });

  deliver("git-action", "Vcs.Push");

  expect(await screen.findByRole("dialog", { name: "Push Commits to rama" })).toBeTruthy();
});

test("on_open_what_was_saved_comes_back_and_changing_repository_withdraws_it", async () => {
  const pending = [{ sha: "abc", title: "Update", message: "Uncommitted changes before Update" }];
  const merging = { ...INFO.state, operation: "merging", label: "Merging main" };
  const opens = [{ ...INFO, pending, state: merging }, INFO];
  mockApp({ repo_open: () => opens.shift() });
  render(App);
  await screen.findByText("0.0.1");
  const input = screen.getByLabelText("Repository path");
  await fireEvent.input(input, { target: { value: "/repo/a" } });
  await fireEvent.submit(input.closest("form")!);

  expect(await screen.findByRole("button", { name: "Restore Changes" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Abort" })).toBeTruthy();

  await fireEvent.input(input, { target: { value: "/repo/b" } });
  await fireEvent.submit(input.closest("form")!);

  await waitFor(() => expect(screen.queryByRole("button", { name: "Abort" })).toBeNull());
});

test("cmd_t_opens_update_project_from_any_view", async () => {
  openWithBranches();
  render(App);
  await screen.findByRole("button", { name: "Git Branch: main" });

  await fireEvent.keyDown(window, { key: "t", code: "KeyT", metaKey: true });

  expect(await screen.findByRole("dialog", { name: "Update Project" })).toBeTruthy();
});

test("the_row_and_the_fetch_of_the_popup_now_have_someone_to_run_them", async () => {
  openWithBranches();
  render(App);
  await fireEvent.click(await screen.findByRole("button", { name: "Git Branch: main" }));

  const row = await screen.findByRole("treeitem", { name: "Update Project…" });
  expect(row.getAttribute("aria-disabled")).toBe("false");
  const fetch = screen.getByRole("button", { name: "Fetch" }) as HTMLButtonElement;
  expect(fetch.disabled).toBe(false);

  await fireEvent.click(row);

  expect(await screen.findByRole("dialog", { name: "Update Project" })).toBeTruthy();
});

const PULL_INFO = {
  branch: "main",
  remotes: ["origin"],
  remoteBranches: ["origin/main"],
  tracked: ["origin", "main"],
};

test("git_pull_is_reached_with_the_git_action_event", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    pull_info: () => PULL_INFO,
    pull_options_get: () => [],
    pull_options_set: () => undefined,
  });
  render(App);
  await screen.findByRole("button", { name: "Git Branch: main" });

  deliver("git-action", "Git.Pull");

  expect(await screen.findByRole("dialog", { name: "Pull to main" })).toBeTruthy();
});

test("without_an_open_repository_git_action_does_nothing", async () => {
  mockApp({ pull_info: () => PULL_INFO });
  render(App);
  await screen.findByText("0.0.1");

  deliver("git-action", "Git.Pull");

  await new Promise((resolve) => setTimeout(resolve, 20));
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(calls("pull_info")).toHaveLength(0);
});

test("view_commits_of_a_pull_opens_the_update_info_tab_with_the_path_filter", async () => {
  const range = ["a".repeat(40), "b".repeat(40)];
  const done = (value: unknown) => ({
    id: 7,
    title: "Pull",
    text: "",
    fraction: null,
    state: "done",
    kind: null,
    value,
  });
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    settings_get: () => ({ ...SETTINGS, updatePaths: ["src"] }),
    pull_info: () => PULL_INFO,
    pull_options_get: () => [],
    pull_options_set: () => undefined,
    git_pull: () => {
      deliver(
        "task-changed",
        done({ outcome: { kind: "success" }, files: 2, commits: 1, range, matching: 1 }),
      );
      return 7;
    },
    log_open: () => ({ viewId: 1, count: 1, collapsible: true }),
    log_rows: () => [],
    log_current_branch: () => "main",
    log_view_open: () => {
      deliver("task-changed", {
        ...done({ viewId: 2, summary: { viewId: 2, count: 1, collapsible: true } }),
        id: 8,
      });
      return 8;
    },
  });
  render(App);
  await screen.findByRole("button", { name: "Git Branch: main" });
  deliver("git-action", "Git.Pull");
  await fireEvent.click(await screen.findByRole("button", { name: "Pull" }));

  const notification = await screen.findByText("2 files updated in 1 commit");
  expect(within(notification.closest("div")!.parentElement!).getByText("1 commit matching filters")).toBeTruthy();
  await fireEvent.click(screen.getByRole("button", { name: "View Commits" }));

  await waitFor(() => expect(calls("log_view_open")).toHaveLength(1));
  const filter = (calls("log_view_open")[0][1] as { spec: { filter: unknown } }).spec.filter;
  expect(filter).toMatchObject({ ranges: [range], paths: ["src"] });
  const stripe = screen.getByRole("tablist", { name: "Git" });
  expect(within(stripe).getByRole("tab", { name: "Log" }).getAttribute("aria-selected")).toBe("true");
  const tab = await screen.findByRole("tab", { name: /^Update Info: / });
  expect(tab.getAttribute("aria-selected")).toBe("true");
});

const taskDone = (id: number, value: unknown) => ({
  id,
  title: "task",
  text: "",
  fraction: null,
  state: "done",
  kind: null,
  value,
});

// The task finishes before the command returns its id, as in the other tests.
const finishing = (id: number, value: unknown) => () => {
  deliver("task-changed", taskDone(id, value));
  return id;
};

async function fillClone(url: string, directory: string) {
  const dialog = await screen.findByRole("dialog", { name: "Clone Repository" });
  await fireEvent.input(within(dialog).getByLabelText("URL:"), { target: { value: url } });
  await fireEvent.input(within(dialog).getByLabelText("Directory:"), {
    target: { value: directory },
  });
  await fireEvent.click(within(dialog).getByRole("button", { name: "Clone" }));
}

test("clone_without_an_open_repository_opens_it_in_this_window", async () => {
  mockApp({
    clone_repository: finishing(5, { path: "/work/copy" }),
    repo_open: () => ({ ...INFO, path: "/work/copy" }),
    branch_list: () => ({ local: [], remote: [], recent: [], current: null, previous: null }),
    branch_incoming_outgoing: () => ({}),
  });
  render(App);
  await screen.findByText("0.0.1");

  deliver("git-action", "Git.Clone");
  await fillClone("/srv/a.git", "/work/copy");

  await waitFor(() => expect(calls("repo_open")).toEqual([["repo_open", { repo: "/work/copy" }]]));
  expect(calls("clone_repository")).toEqual([
    ["clone_repository", { url: "/srv/a.git", directory: "/work/copy" }],
  ]);
  expect(calls("open_in_new_window")).toHaveLength(0);
});

test("clone_with_an_open_repository_asks_the_system_for_another_window", async () => {
  openWithBranches({
    clone_repository: finishing(6, { path: "/work/copy" }),
    open_in_new_window: () => undefined,
  });
  render(App);
  await screen.findByRole("button", { name: "Git Branch: main" });

  deliver("git-action", "Git.Clone");
  await fillClone("/srv/a.git", "/work/copy");

  await waitFor(() =>
    expect(calls("open_in_new_window")).toEqual([["open_in_new_window", { path: "/work/copy" }]]),
  );
  expect(calls("repo_open")).toHaveLength(1);
});

test("create_git_repository_works_without_a_repository_and_opens_what_was_created", async () => {
  mockApp({
    init_repository: finishing(7, { kind: "created", path: "/work/new" }),
    repo_open: () => ({ ...INFO, path: "/work/new" }),
    branch_list: () => ({ local: [], remote: [], recent: [], current: null, previous: null }),
    branch_incoming_outgoing: () => ({}),
  });
  render(App);
  await screen.findByText("0.0.1");

  deliver("git-action", "Git.Init");
  const dialog = await screen.findByRole("dialog", { name: "Create Git Repository" });
  await fireEvent.input(within(dialog).getByLabelText("Directory:"), {
    target: { value: "/work/new" },
  });
  await fireEvent.click(within(dialog).getByRole("button", { name: "Create" }));

  await waitFor(() => expect(calls("repo_open")).toEqual([["repo_open", { repo: "/work/new" }]]));
  expect(calls("init_repository")).toEqual([
    ["init_repository", { path: "/work/new", confirmed: false }],
  ]);
});

test("manage_remotes_and_the_rest_of_the_git_menu_need_a_repository", async () => {
  mockApp({ remote_list: () => [] });
  render(App);
  await screen.findByText("0.0.1");

  for (const action of [
    "Git.Configure.Remotes",
    "Git.CreateNewBranch",
    "Git.Branches",
    "Git.Tag",
    "Git.Reset",
  ]) {
    deliver("git-action", action);
  }

  await new Promise((resolve) => setTimeout(resolve, 20));
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(calls("remote_list")).toHaveLength(0);
});

test("manage_remotes_opens_the_list_with_the_remotes", async () => {
  openWithBranches({
    remote_list: () => [{ name: "origin", url: "/srv/a.git", pushUrl: null }],
  });
  render(App);
  await screen.findByRole("button", { name: "Git Branch: main" });

  deliver("git-action", "Git.Configure.Remotes");

  const dialog = await screen.findByRole("dialog", { name: "Git Remotes" });
  expect(await within(dialog).findByText("/srv/a.git")).toBeTruthy();
});

test("the_git_menu_actions_open_branches_new_branch_and_commit", async () => {
  openWithBranches();
  render(App);
  await screen.findByRole("button", { name: "Git Branch: main" });

  deliver("git-action", "Git.Branches");
  expect(await screen.findByRole("tree", { name: "Branches Tree" })).toBeTruthy();
  await fireEvent.click(screen.getByRole("button", { name: "Git Branch: main" }));

  deliver("git-action", "Git.CreateNewBranch");
  expect(await screen.findByRole("dialog", { name: "Create New Branch" })).toBeTruthy();
});

test("new_branch_from_the_git_menu_does_not_need_the_log_loaded", async () => {
  openWithBranches({
    log_refs: () => Promise.reject({ kind: "failed", message: "The log is not loaded" }),
  });
  render(App);
  await screen.findByRole("button", { name: "Git Branch: main" });

  deliver("git-action", "Git.CreateNewBranch");

  expect(await screen.findByRole("dialog", { name: "Create New Branch" })).toBeTruthy();
});

test("new_tag_from_the_git_menu_opens_the_tag_dialog", async () => {
  openWithBranches();
  render(App);
  await screen.findByRole("button", { name: "Git Branch: main" });

  deliver("git-action", "Git.Tag");

  const dialog = await screen.findByRole("dialog", { name: "Tag" });
  expect(within(dialog).getByLabelText("Commit:")).toBeTruthy();
  expect(within(dialog).getByRole("button", { name: "Create Tag" })).toBeTruthy();
});

test("reset_head_from_the_git_menu_opens_the_old_dialog", async () => {
  openWithBranches({ branch_current: () => "main" });
  render(App);
  await screen.findByRole("button", { name: "Git Branch: main" });

  deliver("git-action", "Git.Reset");

  const dialog = await screen.findByRole("dialog", { name: "Reset Head" });
  expect(within(dialog).getByLabelText("To Commit:")).toBeTruthy();
});

test("with_a_modal_dialog_open_the_git_menu_does_nothing", async () => {
  openWithBranches({ remote_list: () => [] });
  render(App);
  await screen.findByRole("button", { name: "Git Branch: main" });
  deliver("git-action", "Git.Configure.Remotes");
  await screen.findByRole("dialog", { name: "Git Remotes" });

  deliver("git-action", "Git.Clone");
  deliver("git-action", "Git.CreateNewBranch");

  await new Promise((resolve) => setTimeout(resolve, 20));
  expect(screen.getAllByRole("dialog")).toHaveLength(1);
});

test("define_remote_from_the_push_dialog_opens_define_remote_and_reloads_the_information", async () => {
  const pushInfo = (remotes: string[]) => () => ({
    ...PUSH_COMMANDS.push_info(),
    remotes,
    remoteBranches: remotes.map((remote) => `${remote}/main`),
    target: { remote: remotes[0] ?? null, branch: "main", isNew: remotes.length === 0 },
  });
  const infos = [pushInfo([])(), pushInfo(["origin"])()];
  openWithBranches({
    ...PUSH_COMMANDS,
    push_info: () => infos.shift() ?? pushInfo(["origin"])(),
    remote_list: () => [],
    remote_test_url: finishing(8, null),
    remote_add: finishing(9, null),
  });
  render(App);
  await screen.findByRole("button", { name: "Git Branch: main" });
  deliver("git-action", "Vcs.Push");
  const push = await screen.findByRole("dialog", { name: "Push Commits to rama" });

  await fireEvent.click(within(push).getByRole("button", { name: "Define remote" }));
  const define = await screen.findByRole("dialog", { name: "Define Remote" });
  expect((within(define).getByLabelText("Name:") as HTMLInputElement).value).toBe("origin");
  await fireEvent.input(within(define).getByLabelText("URL:"), { target: { value: "/srv/a.git" } });
  await fireEvent.click(within(define).getByRole("button", { name: "OK" }));

  await waitFor(() =>
    expect(calls("remote_add")).toEqual([["remote_add", { name: "origin", url: "/srv/a.git" }]]),
  );
  expect(calls("remote_test_url")).toEqual([
    ["remote_test_url", { url: "/srv/a.git", remote: "origin" }],
  ]);
  await waitFor(() => expect(calls("push_info")).toHaveLength(2));
});

// The menu no longer raises the window on its own: the interface does it only if it is going to act.
test("a_git_menu_action_that_is_handled_raises_the_window", async () => {
  openWithBranches();
  render(App);
  await screen.findByRole("button", { name: "Git Branch: main" });
  await waitFor(() => expect(calls("show_window")).toHaveLength(1));
  const before = calls("show_window").length;

  deliver("git-action", "Git.Branches");

  await screen.findByRole("tree", { name: "Branches Tree" });
  expect(calls("show_window")).toHaveLength(before + 1);
});

test("a_git_menu_action_that_is_ignored_does_not_raise_the_window", async () => {
  openWithBranches({ remote_list: () => [] });
  render(App);
  await screen.findByRole("button", { name: "Git Branch: main" });
  await waitFor(() => expect(calls("show_window")).toHaveLength(1));
  deliver("git-action", "Git.Configure.Remotes");
  await screen.findByRole("dialog", { name: "Git Remotes" });
  const before = calls("show_window").length;

  deliver("git-action", "Git.Clone");
  deliver("git-action", "Git.Branches");

  await new Promise((resolve) => setTimeout(resolve, 20));
  expect(calls("show_window")).toHaveLength(before);
});

test("without_a_repository_an_action_that_needs_one_does_not_raise_the_window_and_clone_does", async () => {
  mockApp();
  render(App);
  await screen.findByText("0.0.1");
  await waitFor(() => expect(calls("show_window")).toHaveLength(1));

  deliver("git-action", "Git.Pull");
  await new Promise((resolve) => setTimeout(resolve, 20));
  expect(calls("show_window")).toHaveLength(1);

  deliver("git-action", "Git.Clone");
  await screen.findByRole("dialog", { name: "Clone Repository" });
  expect(calls("show_window")).toHaveLength(2);
});

test("a_clone_with_a_failed_submodule_warns_and_opens_the_repository", async () => {
  mockApp({
    clone_repository: finishing(11, {
      path: "/work/copy",
      warning: "fatal: clone of '/no/such/sub' into submodule path 'sub' failed",
    }),
    repo_open: () => ({ ...INFO, path: "/work/copy" }),
    branch_list: () => ({ local: [], remote: [], recent: [], current: null, previous: null }),
    branch_incoming_outgoing: () => ({}),
  });
  render(App);
  await screen.findByText("0.0.1");

  deliver("git-action", "Git.Clone");
  await fillClone("~/code/copy", "~/code/copy");

  const warning = await screen.findByText("Cloned, but a submodule failed");
  expect(warning).toBeTruthy();
  expect(screen.getByText(/into submodule path 'sub' failed/)).toBeTruthy();
  await waitFor(() => expect(calls("repo_open")).toEqual([["repo_open", { repo: "/work/copy" }]]));
});

// `conflicts_session` is a task: the result arrives through `task-changed`.
function conflictsApp(commands: Record<string, () => unknown> = {}) {
  let nextTask = 4000;
  const session = {
    conflicts: [{ path: "c.txt", yours: "modified", theirs: "modified", kind: "both modified" }],
    reversed: false,
    yoursColumn: "Yours (main)",
    theirsColumn: "Theirs (another)",
    description: "Merging branch another into branch main",
  };
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    repo_status: () => [
      { path: "c.txt", oldPath: null, state: "unmerged", staged: false, unstaged: true },
    ],
    conflicts_session: () => {
      const id = nextTask++;
      setTimeout(() =>
        deliver("task-changed", {
          id,
          title: "",
          text: "",
          fraction: null,
          state: "done",
          kind: null,
          value: session,
        }),
      );
      return id;
    },
    ...commands,
  });
}

test("resolve_conflicts_from_the_git_menu_turns_on_with_conflicts_and_opens_the_dialog", async () => {
  conflictsApp();
  render(App);
  await screen.findByRole("treeitem", { name: "Merge Conflicts" });

  await waitFor(() =>
    expect(calls("git_menu_enable").map(([, args]) => args)).toContainEqual({
      ids: ["Git.Merge", "Git.Rebase", "Git.ResolveConflicts", "Git.Fetch"],
    }),
  );
  deliver("git-action", "Git.ResolveConflicts");

  const dialog = await screen.findByRole("dialog", { name: "Conflicts" });
  expect(within(dialog).getByText("Merging branch another into branch main")).toBeTruthy();
});

test("the_resolve_link_of_the_node_opens_the_same_dialog", async () => {
  conflictsApp();
  render(App);
  const node = await screen.findByRole("treeitem", { name: "Merge Conflicts" });

  await fireEvent.click(within(node).getByRole("button", { name: "Resolve" }));

  expect(await screen.findByRole("dialog", { name: "Conflicts" })).toBeTruthy();
});

test("without_conflicts_resolve_conflicts_does_not_turn_on", async () => {
  mockApp({ initial_repo: () => "/repo/a", repo_open: () => INFO });
  render(App);
  await screen.findByRole("treeitem", { name: /^Changes/ });

  expect(
    calls("git_menu_enable").every(([, args]) => !(args as { ids: string[] }).ids.includes("Git.ResolveConflicts")),
  ).toBe(true);
});

test("when_everything_is_resolved_resolve_conflicts_turns_off", async () => {
  let unmerged = true;
  conflictsApp({
    repo_status: () =>
      unmerged
        ? [{ path: "c.txt", oldPath: null, state: "unmerged", staged: false, unstaged: true }]
        : [],
  });
  render(App);
  await screen.findByRole("treeitem", { name: "Merge Conflicts" });
  await waitFor(() =>
    expect(calls("git_menu_enable").map(([, args]) => args)).toContainEqual({
      ids: ["Git.Merge", "Git.Rebase", "Git.ResolveConflicts", "Git.Fetch"],
    }),
  );

  unmerged = false;
  deliver("repo-changed");

  await waitFor(() =>
    expect(calls("git_menu_enable").map(([, args]) => args).at(-1)).toEqual({
      ids: ["Git.Merge", "Git.Rebase", "Git.Fetch"],
    }),
  );
});

// Decision 5: `Git.Fetch` is off while a fetch runs, and comes back when it ends.
test("the_git_menu_fetch_is_disabled_while_a_fetch_runs", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    fetch_all: () => 9,
  });
  render(App);
  await screen.findByRole("button", { name: "Git Branch: main" });
  const lastIds = () => (calls("git_menu_enable").at(-1)?.[1] as { ids: string[] }).ids;
  await waitFor(() => expect(lastIds()).toContain("Git.Fetch"));

  deliver("git-action", "Git.Fetch");

  await waitFor(() => expect(lastIds()).not.toContain("Git.Fetch"));
  expect(lastIds()).toEqual(["Git.Merge", "Git.Rebase"]);
  deliver("task-changed", taskDone(9, { pruned: [], failed: [] }));
  await waitFor(() => expect(lastIds()).toContain("Git.Fetch"));
});

// Merge… and Rebase… of the Git menu: they open their dialog, launch the command with the HEAD it saw and notify.
function mergeApp(commands: Record<string, () => unknown> = {}) {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => INFO,
    branch_list: () => ({
      local: [
        { name: "main", tracked: null },
        { name: "side", tracked: null },
      ],
      remote: [],
      recent: [],
      current: "main",
      previous: null,
    }),
    branch_incoming_outgoing: () => ({}),
    repo_settings_get: () => ({
      protectedBranches: [],
      forcePushTargets: [],
      merge: { branch: "side", options: [] },
      rebase: { options: [], newBase: null },
    }),
    unmerged_branches: finishing(6001, ["side"]),
    merge_settings_set: () => undefined,
    ...commands,
  });
}

test("merge_from_the_git_menu_opens_the_dialog_and_launches_the_merge_with_the_head_it_saw", async () => {
  mergeApp({
    git_merge: finishing(6002, {
      kind: "merged",
      reason: { files: 1, commits: 1, range: null, matching: null },
    }),
  });
  render(App);
  await screen.findByRole("button", { name: "Git Branch: main" });

  deliver("git-action", "Git.Merge");

  const dialog = await screen.findByRole("dialog", { name: "Merge into main" });
  expect((within(dialog).getByRole("combobox", { name: "Branch" }) as HTMLInputElement).value).toBe(
    "side",
  );
  await fireEvent.click(within(dialog).getByRole("button", { name: "Merge" }));

  expect(await screen.findByText("1 file updated in 1 commit")).toBeTruthy();
  expect(calls("git_merge")[0][1]).toEqual({
    branch: "side",
    options: [],
    message: null,
    expectedHead: INFO.state.head,
  });
  expect(calls("merge_settings_set")[0][1]).toEqual({ branch: "side", options: [] });
});

test("rebase_from_the_git_menu_opens_the_dialog_and_launches_the_rebase", async () => {
  mergeApp({
    git_rebase: finishing(6003, { outcome: { kind: "done" }, title: null, saved: [] }),
    rebase_published: finishing(6004, null),
    rebase_settings_set: () => undefined,
  });
  render(App);
  await screen.findByRole("button", { name: "Git Branch: main" });

  deliver("git-action", "Git.Rebase");

  const dialog = await screen.findByRole("dialog", { name: "Rebase" });
  await fireEvent.input(within(dialog).getByRole("combobox", { name: "Upstream" }), {
    target: { value: "side" },
  });
  await fireEvent.click(within(dialog).getByRole("button", { name: "Rebase" }));

  expect(await screen.findByText("Rebase successful")).toBeTruthy();
  expect(calls("git_rebase")[0][1]).toMatchObject({
    params: { upstream: "side", onto: null, branch: null, root: false },
    expectedHead: INFO.state.head,
  });
  expect(calls("rebase_settings_set")[0][1]).toEqual({ options: [], newBase: "side" });
});

test("with_a_half_done_merge_merge_and_rebase_from_the_menu_open_nothing", async () => {
  const merging = { ...INFO.state, operation: "merging", label: "Merging main" };
  mergeApp({ repo_open: () => ({ ...INFO, state: merging }), repo_state: () => merging });
  render(App);
  await screen.findByRole("button", { name: "Git Branch: Merging main" });

  deliver("git-action", "Git.Merge");
  deliver("git-action", "Git.Rebase");
  await new Promise((resolve) => setTimeout(resolve, 20));

  expect(screen.queryByRole("dialog", { name: "Merge into main" })).toBeNull();
  expect(screen.queryByRole("dialog", { name: "Rebase" })).toBeNull();
  expect(calls("git_merge")).toHaveLength(0);
});

test("the_merge_and_rebase_widget_sits_after_the_branch_widget", async () => {
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => ({ ...INFO, state: REBASING }),
  });
  render(App);
  const branch = await screen.findByRole("button", { name: "Git Branch: Rebasing main" });

  const cont = screen.getByRole("button", { name: "Continue Rebase" });
  expect(branch.compareDocumentPosition(cont) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  expect(branch.nextElementSibling?.contains(cont)).toBe(true);
});

test("the_widget_buttons_run_the_git_menu_actions", async () => {
  const merging = { ...INFO.state, operation: "merging", label: "Merging main" };
  mockApp({
    initial_repo: () => "/repo/a",
    repo_open: () => ({ ...INFO, state: merging }),
    repo_state: () => merging,
    flush: () => undefined,
  });
  render(App);

  await fireEvent.click(await screen.findByRole("button", { name: "Abort Merge" }));

  const dialog = await screen.findByRole("dialog", { name: "Abort Merge" });
  expect(within(dialog).getByText("Abort merge?")).toBeTruthy();
});

test("without_an_operation_the_widget_is_not_in_the_toolbar", async () => {
  mockApp({ initial_repo: () => "/repo/a", repo_open: () => INFO });
  render(App);
  await screen.findByRole("button", { name: "Git Branch: main" });

  expect(screen.queryByRole("button", { name: "Continue Rebase" })).toBeNull();
  expect(screen.queryByRole("button", { name: "Abort Merge" })).toBeNull();
});
