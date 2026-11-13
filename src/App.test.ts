import { tick } from "svelte";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { get } from "svelte/store";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { emitTo, listen } from "@tauri-apps/api/event";
import { open as chooseNative } from "@tauri-apps/plugin-dialog";
import App from "./App.svelte";
import { closeCommitDialog, openCommitDialog, requestCommit } from "./commit/dialogStore";
import { dismiss, notifications } from "./lib/notifications";
import { requestTab } from "./lib/tabs";
import { activateWindow, registerToolWindow } from "./shell/toolWindows";
import { showRepositoryAtRevision } from "./repositories/tab";
import { answerPatchDialog, applyInProgress } from "./patch/flow";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
  emitTo: vi.fn(async () => {}),
}));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({ label: "main" }),
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

// A command's handler gets its arguments.
type Command = (args?: Record<string, unknown>) => unknown;

function mockCommands(commands: Record<string, Command>) {
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    const handler = commands[command];
    if (!handler) throw new Error(`unexpected command: ${command}`);
    return handler(args as Record<string, unknown> | undefined);
  });
}

beforeEach(() => {
  vi.mocked(invoke).mockReset();
});

// Notifications live in the module: those of one test must not be seen in the next.
afterEach(() => {
  cleanup();
  get(notifications).forEach(({ id }) => dismiss(id));
  // A patch dialog left open by a failed test would be the next one's.
  answerPatchDialog(null);
});

// `Ramify <folder>`: the repository alone, as before files could be opened.
const folder = (repo: string) => ({ repo, file: null, line: null });

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
  githubConnectionTimeoutMs: 5000,
  openProjectIn: "ask",
  reopenProjects: true,
  commitMessageTicket: true,
  notifyExternalChanges: true,
  copyBranchName: true,
};

const NON_MODAL = { ...SETTINGS, nonModalCommit: true };
// `Open project in: Current window`: another repository replaces this one without asking.
const CURRENT_WINDOW = { ...SETTINGS, openProjectIn: "current" };

function mockApp(commands: Record<string, Command> = {}) {
  mockCommands({
    app_version: () => "0.0.1",
    initial_target: () => null,
    // The open flow: each target is its own root, and no other window has it open.
    project_resolve: (args) => args?.target,
    project_focus_open: () => false,
    perf_enabled: () => false,
    show_window: () => undefined,
    repo_status: () => [],
    changelists_get: () => LISTS,
    changelists_lists: () => LISTS,
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
    stash_list: finishing(STASH_LIST_TASK, []),
    // Nothing of an earlier session to reopen.
    open_projects_reopen: () => ({ missing: [] }),
    commit_draft_saved: () => undefined,
    branch_incoming_outgoing: () => ({}),
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
    initial_target: () => folder("/no/repo"),
    repo_open: () => {
      throw { kind: "not_a_repository", message: "/no/repo is not a Git repository" };
    },
  });
  render(App);
  expect(await screen.findByText("/no/repo is not a Git repository")).toBeTruthy();
});

test("a_log_error_comes_out_as_a_notification_and_does_not_stay_in_commit", async () => {
  mockApp({
    initial_target: () => folder("/repo/a"),
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
    initial_target: () => folder("/repo/a"),
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
    initial_target: () => folder("/repo/a"),
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
  mockApp({ initial_target: () => folder("/repo/a"), repo_open: () => INFO });
  render(App);

  const tab = await screen.findByRole("tab", { name: "Local Changes" });

  expect(tab.getAttribute("aria-selected")).toBe("true");
  expect(tabState("Git")).toBe("true");
  expect(screen.queryByRole("tab", { name: "Commit" })).toBeNull();
  expect(await screen.findByText("main")).toBeTruthy();
  expect(calls("log_open")).toHaveLength(0);
});

const stripeLabels = () =>
  within(screen.getByRole("tablist", { name: "Tool windows" }))
    .getAllByRole("tab")
    .map((button) => button.getAttribute("aria-label"));

test("the stripe has Project and Git in modal mode", async () => {
  mockApp({ initial_target: () => folder("/repo/a"), repo_open: () => INFO });
  render(App);
  await screen.findByRole("tab", { name: "Local Changes" });

  expect(stripeLabels()).toEqual(["Project", "Git"]);
});

// The user's `window.layouts.xml`: Project (⌘1) before Commit on the left, Git at the bottom.
// Changes and Repositories exist once something registers them, and another repository forgets them.
test("the_stripe_shows_registered_windows_in_registry_order", async () => {
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    settings_get: () => ({ ...NON_MODAL, openProjectIn: "current" }),
  });
  render(App);
  await screen.findByRole("textbox", { name: "Commit Message" });
  expect(stripeLabels()).toEqual(["Project", "Commit", "Git"]);
  expect(screen.getByRole("tab", { name: "Project" }).getAttribute("title")).toBe("Project (⌘1)");

  registerToolWindow("repositories");
  registerToolWindow("changes");
  await waitFor(() => expect(stripeLabels()).toEqual(["Project", "Commit", "Changes", "Repositories", "Git"]));
  // A request brings a window to the front and never hides it.
  activateWindow("changes");
  await waitFor(() => expect(tabState("Changes")).toBe("true"));
  activateWindow("changes");
  await tick();
  expect(tabState("Changes")).toBe("true");

  // Another repository replaces this one.
  const input = screen.getByLabelText("Repository path");
  await fireEvent.input(input, { target: { value: "/repo/b" } });
  await fireEvent.submit(input.closest("form")!);
  await waitFor(() => expect(stripeLabels()).toEqual(["Project", "Commit", "Git"]));
  expect(tabState("Commit")).toBe("true");
});

// `GHPRToolWindowViewModel.selectorVm` is lazy: with one GitHub repository and one account, nothing
// is sent to GitHub until the Pull Requests window is first shown.
test("an_automatic_pair_connects_only_once_the_window_is_shown", async () => {
  const known = {
    mappings: [
      { remote: "origin", url: "https://github.com/o/r.git", display: "o/r", server: "github.com", repository: "github.com/o/r" },
    ],
    accounts: [
      { id: "github.com/octocat", server: "github.com", login: "octocat", name: null, error: null, needsLogin: false, isDefault: false },
    ],
    saved: null,
  };
  const connection = {
    remote: "origin",
    url: "https://github.com/o/r.git",
    account: "github.com/octocat",
    coordinates: { server: { useHttp: null, host: "github.com", port: null, suffix: null }, owner: "o", name: "r" },
    viewerLogin: "octocat",
    repository: {
      id: "R_1",
      owner: "o",
      nameWithOwner: "o/r",
      viewerPermission: "WRITE",
      mergeCommitAllowed: true,
      squashMergeAllowed: true,
      rebaseMergeAllowed: true,
      defaultBranch: "main",
      isFork: false,
      url: "https://github.com/o/r",
      sshUrl: "git@github.com:o/r.git",
    },
    title: "r",
  };
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    settings_get: () => NON_MODAL,
    repo_settings_get: () => ({}),
    github_known: () => known,
    github_connect: finishing(41, connection),
  });
  render(App);
  await screen.findByRole("textbox", { name: "Commit Message" });
  await waitFor(() => expect(stripeLabels()).toEqual(["Project", "Commit", "Pull Requests", "Git"]));
  await tick();
  expect(calls("github_connect")).toHaveLength(0);

  await fireEvent.click(screen.getByRole("tab", { name: "Pull Requests" }));
  await waitFor(() => expect(calls("github_connect")).toHaveLength(1));
  const pane = screen.getByRole("region", { name: "Pull Requests" });
  expect(await within(pane).findByRole("tab", { name: "r" })).toBeTruthy();
});

// `Github.View.Pull.Request` (`GithubViewPullRequestsAction`, in `GitHub.MainMenu` of the Git menu):
// there only while the Pull Requests window is available, that is while a remote points to a
// GitHub repository, and it brings the window to the front.
test("view_pull_requests_is_in_the_git_menu_only_with_the_window_and_brings_it_to_the_front", async () => {
  const github = {
    mappings: [
      { remote: "origin", url: "https://github.com/o/r.git", display: "o/r", server: "github.com", repository: "github.com/o/r" },
    ],
    accounts: [],
    saved: null,
  };
  let known = github;
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    settings_get: () => NON_MODAL,
    repo_settings_get: () => ({}),
    github_known: () => known,
  });
  render(App);
  await screen.findByRole("textbox", { name: "Commit Message" });
  const lastIds = () => (calls("git_menu_enable").at(-1)?.[1] as { ids: string[] }).ids;
  await waitFor(() => expect(stripeLabels()).toEqual(["Project", "Commit", "Pull Requests", "Git"]));
  await waitFor(() => expect(lastIds()).toContain("Github.View.Pull.Request"));
  expect(tabState("Pull Requests")).toBe("false");

  deliver("git-action", "Github.View.Pull.Request");

  await waitFor(() => expect(tabState("Pull Requests")).toBe("true"));
  const pane = screen.getByRole("region", { name: "Pull Requests" });
  expect(pane.closest(".hidden")).toBeNull();
  // With no account, the selector offers the login; nothing was sent to GitHub.
  expect(await within(pane).findByRole("button", { name: "Log In with Token…" })).toBeTruthy();
  expect(calls("github_connect")).toHaveLength(0);

  // The last GitHub remote goes: the entry goes with the window, and the action does nothing.
  await fireEvent.click(screen.getByRole("tab", { name: "Commit" }));
  known = { ...github, mappings: [] };
  deliver("repo-changed");
  await waitFor(() => expect(stripeLabels()).toEqual(["Project", "Commit", "Git"]));
  await waitFor(() => expect(lastIds()).not.toContain("Github.View.Pull.Request"));
  deliver("git-action", "Github.View.Pull.Request");
  await tick();
  expect(tabState("Commit")).toBe("true");
});

// `Show Repository at Revision`: the Repositories window appears on the left and comes to the front
// with the revision's tree.
test("show_repository_at_revision_brings_the_repositories_window", async () => {
  const hash = "a1b2c3d4".repeat(5);
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    settings_get: () => NON_MODAL,
    tree_list_at: () => [{ name: "README.md", path: "README.md", dir: false }],
  });
  render(App);
  await screen.findByRole("textbox", { name: "Commit Message" });

  showRepositoryAtRevision(hash);

  await waitFor(() => expect(tabState("Repositories")).toBe("true"));
  expect(stripeLabels()).toEqual(["Project", "Commit", "Repositories", "Git"]);
  const pane = screen.getByRole("region", { name: "Repositories" });
  expect(await within(pane).findByRole("treeitem", { name: "README.md" })).toBeTruthy();
  expect(within(pane).getByRole("tab").textContent).toBe("a at a1b2c3d4");
});

// `ActivateToolWindowAction`: ⌘1 brings the Project window to the front, and hides it when it is the
// active one; what was in front before comes back. Its stripe button does the same.
test("meta_1_activates_the_project_window_and_hides_it_when_active", async () => {
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    settings_get: () => NON_MODAL,
    log_open: () => ({ viewId: 1, count: 0 }),
    tree_list: () => [{ name: "src", path: "src", dir: true }],
  });
  render(App);
  await screen.findByRole("textbox", { name: "Commit Message" });
  const project = () => screen.getByRole("region", { name: "Project" });
  expect(project().closest(".hidden")).not.toBeNull();
  // The tree is built when its window first comes to the front.
  expect(calls("tree_list")).toEqual([]);

  await fireEvent.keyDown(window, { key: "1", code: "Digit1", metaKey: true });
  expect(tabState("Project")).toBe("true");
  expect(tabState("Commit")).toBe("false");
  expect(project().closest(".hidden")).toBeNull();
  expect(await within(project()).findByRole("treeitem", { name: "src" })).toBeTruthy();
  expect(within(project()).getByRole("treeitem", { name: "a" }).getAttribute("aria-expanded")).toBe("true");
  expect(calls("tree_list")).toEqual([["tree_list", { dir: "" }]]);
  expect(screen.getByRole("textbox", { name: "Commit Message" }).closest(".hidden")).not.toBeNull();

  await fireEvent.keyDown(window, { key: "1", code: "Digit1", metaKey: true });
  expect(tabState("Commit")).toBe("true");
  expect(tabState("Project")).toBe("false");
  expect(project().closest(".hidden")).not.toBeNull();

  // From the Git window, and with the stripe button.
  await fireEvent.keyDown(window, { key: "9", code: "Digit9", metaKey: true });
  await fireEvent.click(screen.getByRole("tab", { name: "Project" }));
  expect(tabState("Project")).toBe("true");
  await fireEvent.click(screen.getByRole("tab", { name: "Project" }));
  expect(tabState("Git")).toBe("true");
  expect(tabState("Log")).toBe("true");
});

test("cmd 0 does nothing in modal mode", async () => {
  mockApp({
    initial_target: () => folder("/repo/a"),
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
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    log_open: () => ({ viewId: 1, count: 0 }),
  });
  render(App);
  await screen.findByRole("tab", { name: "Local Changes" });

  await fireEvent.keyDown(window, { key: "9", metaKey: true });

  const tabs = within(screen.getByRole("tablist", { name: "Git" })).getAllByRole("tab");
  expect(tabs.map((tab) => tab.textContent?.trim())).toEqual(["Local Changes", "Log", "Console", "Terminal"]);
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
  expect(tabs.map((tab) => tab.textContent?.trim())).toEqual(["Log", "Console", "Terminal"]);

  deliver("settings-changed", SETTINGS);

  await screen.findByRole("tab", { name: "Local Changes" });
  expect(tabState("Local Changes")).toBe("true");
  expect(screen.queryByRole("tab", { name: "Commit" })).toBeNull();
  expect(screen.queryByRole("textbox", { name: "Commit Message" })).toBeNull();
  expect(screen.getByRole("button", { name: "Commit…" })).toBeTruthy();
});

test("a draft and a line selection of the Commit view survive switching to modal and back", async () => {
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    settings_get: () => NON_MODAL,
    repo_status: () => [CHANGE],
    file_hunks: () => [{ id: "h1", oldStart: 1, oldLines: 1, newStart: 1, newLines: 1 }],
    diff_window_open: () => undefined,
  });
  vi.mocked(emitTo).mockClear();
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
    expect(vi.mocked(emitTo)).toHaveBeenCalledWith(
      "main",
      "selection-state",
      expect.objectContaining({ selections: { "a.txt": [{ hunk: "h1", lines: [0] }] } }),
    ),
  );
});

test("arrow keys in the tab strip of the Git window keep the focus on the strip", async () => {
  mockApp({
    initial_target: () => folder("/repo/a"),
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
  mockApp({ initial_target: () => folder("/repo/a"), repo_open: () => INFO, repo_status: () => [CHANGE] });
  render(App);

  await screen.findByRole("treeitem", { name: /a\.txt/ });

  expect(within(screen.getByRole("tree")).queryAllByRole("checkbox")).toEqual([]);
  expect(screen.queryByRole("textbox", { name: "Commit Message" })).toBeNull();
  expect(screen.getByRole("button", { name: "Commit…" })).toBeTruthy();
});

test("in modal mode Commit… and cmd K open the Commit Changes dialog, which keeps the keys while open", async () => {
  mockApp({
    initial_target: () => folder("/repo/a"),
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
    initial_target: () => folder("/repo/a"),
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
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    settings_get: () => CURRENT_WINDOW,
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
    initial_target: () => folder("/repo/a"),
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
    initial_target: () => folder("/repo/a"),
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
    initial_target: () => folder("/repo/a"),
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
    initial_target: () => folder("/repo/a"),
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
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    log_open: () => ({ viewId: 1, count: 0, collapsible: true }),
  });
  render(App);
  await screen.findByRole("tab", { name: "Local Changes" });

  await fireEvent.keyDown(window, { key: "r", code: "KeyR", metaKey: true });

  expect(calls("log_open")).toHaveLength(0);
});

test("shows_the_window_when_mounting_finishes", async () => {
  mockApp({ initial_target: () => folder("/repo/a"), repo_open: () => INFO });
  render(App);

  await vi.waitFor(() => expect(calls("show_window")).toHaveLength(1));

  const order = vi.mocked(invoke).mock.calls.map(([name]) => name);
  expect(order.indexOf("repo_open")).toBeLessThan(order.indexOf("show_window"));
});

// Decision 1 of part 4: `Ramify <file>:<line>` opens the file's repository, then the viewer there.
test("a_file_argument_opens_its_repository_and_the_viewer_at_the_line", async () => {
  mockApp({
    initial_target: () => ({ repo: "/repo/a", file: "src/main file.rs", line: 12 }),
    repo_open: () => INFO,
    viewer_open: () => undefined,
  });
  render(App);

  await vi.waitFor(() => expect(calls("viewer_open")).toHaveLength(1));
  expect(calls("repo_open")[0][1]).toEqual({ repo: "/repo/a" });
  expect(calls("viewer_open")[0][1]).toEqual({
    targets: [{ path: "src/main file.rs", revision: null, line: 12 }],
  });
  const order = vi.mocked(invoke).mock.calls.map(([name]) => name);
  expect(order.indexOf("repo_open")).toBeLessThan(order.indexOf("viewer_open"));
  await vi.waitFor(() => expect(calls("show_window")).toHaveLength(1));
});

test("a_folder_argument_still_opens_the_repository", async () => {
  mockApp({ initial_target: () => folder("/repo/a/src"), repo_open: () => INFO });
  render(App);

  await vi.waitFor(() => expect(calls("show_window")).toHaveLength(1));
  expect(calls("repo_open")[0][1]).toEqual({ repo: "/repo/a/src" });
  expect(calls("viewer_open")).toEqual([]);
});

test("a_file_outside_a_repository_says_so", async () => {
  mockApp({
    initial_target: () => {
      throw {
        kind: "not_a_repository",
        message: "/tmp/notes.txt is not inside a Git repository",
      };
    },
  });
  render(App);

  expect(await screen.findByText("/tmp/notes.txt is not inside a Git repository")).toBeTruthy();
  await vi.waitFor(() => expect(calls("show_window")).toHaveLength(1));
  expect(calls("repo_open")).toEqual([]);
});

// The repository did not open: there is nothing to show the file from.
test("a_file_whose_repository_does_not_open_opens_no_viewer", async () => {
  mockApp({
    initial_target: () => ({ repo: "/repo/a", file: "a.txt", line: null }),
    repo_open: () => {
      throw { kind: "failed", message: "git rev-parse failed" };
    },
  });
  render(App);

  expect(await screen.findByText("git rev-parse failed")).toBeTruthy();
  expect(calls("viewer_open")).toEqual([]);
});

test("shows_the_window_even_if_startup_fails", async () => {
  mockApp({
    initial_target: () => {
      throw new Error("no arguments");
    },
  });
  render(App);

  await vi.waitFor(() => expect(calls("show_window")).toHaveLength(1));
});

test("measure_mode_opens_the_log", async () => {
  mockApp({
    initial_target: () => folder("/repo/a"),
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
      initial_target: () => folder("/repo/a"),
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
      initial_target: () => folder("/repo/a"),
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
      initial_target: () => folder("/repo/a"),
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
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    settings_get: () => NON_MODAL,
    repo_status: () => [CHANGE],
    diff_window_open: () => undefined,
  });
  vi.mocked(emitTo).mockClear();
  render(App);

  await fireEvent.dblClick(await screen.findByRole("treeitem", { name: /a\.txt/ }));

  await vi.waitFor(() => expect(calls("diff_window_open")).toHaveLength(1));
  expect(calls("diff_window_open")[0][1]).toEqual({ path: "a.txt" });
  await fireEvent.click(screen.getByRole("treeitem", { name: /a\.txt/ }).querySelector("[role=checkbox]")!);
  await vi.waitFor(() =>
    expect(vi.mocked(emitTo)).toHaveBeenCalledWith(
      "main",
      "selection-state",
      expect.objectContaining({ selections: {} }),
    ),
  );
});

// `Show on Double-Click` is in the gear of the Git window while Local Changes is shown
// (`LocalChangesView.GearActions`): `Source` by default, so a double click opens the viewer.
test("in modal mode a double click in Local Changes opens the source, or the diff with Show on Double-Click set to Diff", async () => {
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    repo_status: () => [CHANGE],
    viewer_open: () => undefined,
    diff_window_open: () => undefined,
  });
  render(App);
  await fireEvent.dblClick(await screen.findByRole("treeitem", { name: /a\.txt/ }));
  await vi.waitFor(() => expect(calls("viewer_open")).toHaveLength(1));
  expect(calls("viewer_open")[0][1]).toEqual({
    targets: [{ path: "a.txt", revision: null, line: null }],
  });

  const strip = screen.getByRole("tablist", { name: "Git" });
  await fireEvent.click(within(strip).getByRole("button", { name: "Options" }));
  const gear = await screen.findByRole("menu", { name: "Options" });
  const item = within(gear).getByRole("menuitem", { name: "Show on Double-Click" });
  expect(within(gear).getAllByRole("menuitem").map((row) => row.getAttribute("aria-label"))).toEqual([
    "Show on Double-Click",
  ]);
  await fireEvent.pointerEnter(item);
  const options = await screen.findByRole("menu", { name: "Show on Double-Click" });
  const choices = within(options).getAllByRole("menuitem");
  expect(choices.map((row) => row.textContent?.replace(/\s+/g, " ").trim())).toEqual([
    "Diff",
    "✓ Source",
  ]);
  await fireEvent.click(choices[0]);
  expect(localStorage.getItem("VcsApplicationSettings.SHOW_DIFF_ON_DOUBLE_CLICK")).toBe("true");

  await fireEvent.dblClick(screen.getByRole("treeitem", { name: /a\.txt/ }));
  await vi.waitFor(() => expect(calls("diff_window_open")).toHaveLength(1));
  expect(calls("viewer_open")).toHaveLength(1);
  localStorage.clear();
});

test("in modal mode a double click in Local Changes opens the diff window without line checkboxes", async () => {
  localStorage.setItem("VcsApplicationSettings.SHOW_DIFF_ON_DOUBLE_CLICK", "true");
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    repo_status: () => [CHANGE],
    diff_window_open: () => undefined,
  });
  vi.mocked(emitTo).mockClear();
  render(App);

  await fireEvent.dblClick(await screen.findByRole("treeitem", { name: /a\.txt/ }));
  localStorage.clear();

  await vi.waitFor(() => expect(calls("diff_window_open")).toHaveLength(1));
  expect(calls("diff_window_open")[0][1]).toEqual({ path: "a.txt" });
  deliver("diff-ready");
  await vi.waitFor(() =>
    expect(vi.mocked(emitTo)).toHaveBeenCalledWith("main", "selection-state", {
      owner: "none",
      included: [],
      selections: {},
      lists: null,
    }),
  );
});

test("switching to modal mode takes the line checkboxes away from an open diff window", async () => {
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    settings_get: () => NON_MODAL,
    repo_status: () => [CHANGE],
    diff_window_open: () => undefined,
  });
  vi.mocked(emitTo).mockClear();
  render(App);
  await fireEvent.dblClick(await screen.findByRole("treeitem", { name: /a\.txt/ }));
  await vi.waitFor(() => expect(calls("diff_window_open")).toHaveLength(1));
  deliver("diff-ready");
  await vi.waitFor(() =>
    expect(vi.mocked(emitTo)).toHaveBeenCalledWith(
      "main",
      "selection-state",
      expect.objectContaining({ owner: "commit-view", included: ["a.txt"] }),
    ),
  );
  vi.mocked(emitTo).mockClear();

  deliver("settings-changed", SETTINGS);

  await vi.waitFor(() =>
    expect(vi.mocked(emitTo)).toHaveBeenCalledWith(
      "main",
      "selection-state",
      expect.objectContaining({ owner: "none", included: [], selections: {} }),
    ),
  );
  // The window asks again when it is reopened: what the Commit view kept is not sent.
  vi.mocked(emitTo).mockClear();
  deliver("diff-ready");
  await vi.waitFor(() =>
    expect(vi.mocked(emitTo)).toHaveBeenCalledWith(
      "main",
      "selection-state",
      expect.objectContaining({ owner: "none" }),
    ),
  );
  expect(vi.mocked(emitTo)).not.toHaveBeenCalledWith(
    "main",
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
  expect(vi.mocked(emitTo)).not.toHaveBeenCalledWith(
    "main",
    "selection-state",
    expect.objectContaining({ selections: { "a.txt": expect.anything() } }),
  );
});

test("a diff window opened from the Commit Changes dialog follows the dialog and is cleared when it closes", async () => {
  mockApp({
    initial_target: () => folder("/repo/a"),
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
  vi.mocked(emitTo).mockClear();
  render(App);
  await screen.findByRole("treeitem", { name: /a\.txt/ });
  await fireEvent.click(screen.getByRole("button", { name: "Commit…" }));
  const dialog = await screen.findByRole("dialog", { name: "Commit Changes" });

  await fireEvent.dblClick(await within(dialog).findByRole("treeitem", { name: "a.txt" }));

  await vi.waitFor(() => expect(calls("diff_window_open")).toHaveLength(1));
  deliver("diff-ready");
  await vi.waitFor(() =>
    expect(vi.mocked(emitTo)).toHaveBeenCalledWith(
      "main",
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
    expect(vi.mocked(emitTo)).toHaveBeenCalledWith(
      "main",
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

  vi.mocked(emitTo).mockClear();
  await fireEvent.keyDown(within(dialog).getByRole("textbox", { name: "Commit Message" }), {
    key: "Escape",
  });

  await vi.waitFor(() =>
    expect(vi.mocked(emitTo)).toHaveBeenCalledWith("main", "selection-state", {
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
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    settings_get: () => NON_MODAL,
    repo_status: () => [CHANGE],
    ...DIALOG_COMMANDS,
  });
  vi.mocked(emitTo).mockClear();
  render(App);
  await fireEvent.dblClick(await screen.findByRole("treeitem", { name: /a\.txt/ }));
  await vi.waitFor(() => expect(calls("diff_window_open")).toHaveLength(1));
  deliver("diff-ready");
  deliver("selection-changed", LINE_CHANGE);
  const chosen = { a: [{ hunk: "h1", lines: [0] }] };
  await vi.waitFor(() =>
    expect(vi.mocked(emitTo)).toHaveBeenCalledWith(
      "main",
      "selection-state",
      expect.objectContaining({ owner: "commit-view", selections: { "a.txt": chosen.a } }),
    ),
  );

  // The dialog is not reachable from the non-modal entry points: it opens as the executors of part 3 do.
  openCommitDialog({ request: { kind: "project", paths: [], lists: [] }, executor: "commit" });
  const dialog = await screen.findByRole("dialog", { name: "Commit Changes" });
  await vi.waitFor(() =>
    expect(vi.mocked(emitTo)).toHaveBeenCalledWith(
      "main",
      "selection-state",
      expect.objectContaining({ owner: "dialog", selections: {} }),
    ),
  );
  vi.mocked(emitTo).mockClear();
  await fireEvent.keyDown(within(dialog).getByRole("textbox", { name: "Commit Message" }), {
    key: "Escape",
  });

  await vi.waitFor(() =>
    expect(vi.mocked(emitTo)).toHaveBeenCalledWith(
      "main",
      "selection-state",
      expect.objectContaining({ owner: "commit-view", selections: { "a.txt": chosen.a } }),
    ),
  );
});

test("the commit of the dialog runs after the dialog has closed", async () => {
  mockApp({
    initial_target: () => folder("/repo/a"),
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
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    repo_status: () => [CHANGE],
    ...DIALOG_COMMANDS,
    changelists_set_comment: () => LISTS,
    commit_changes_selected: () => {
      throw { kind: "selection_lost", message: "x", details: { path: "a.txt", hunks: ["h1"] } };
    },
  });
  vi.mocked(emitTo).mockClear();
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
    expect(vi.mocked(emitTo)).toHaveBeenCalledWith(
      "main",
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
    initial_target: () => folder("/repo/a"),
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
      initial_target: () => folder("/repo/a"),
      repo_open: () => (++opened === 1 ? INFO : { ...INFO, path: "/repo/b" }),
      settings_get: () => CURRENT_WINDOW,
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
  expect(screen.queryByRole("dialog", { name: "Push Commits to ramify" })).toBeNull();
  expect(calls("push_info")).toHaveLength(0);
});

test("cmd K opens the dialog from Local Changes and from the Log in modal mode", async () => {
  mockApp({
    initial_target: () => folder("/repo/a"),
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
    initial_target: () => folder("/repo/a"),
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
    initial_target: () => folder("/repo/a"),
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
    initial_target: () => folder("/repo/a"),
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
    initial_target: () => folder("/repo/a"),
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
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    settings_get: () => NON_MODAL,
    repo_status: () => [CHANGE],
    diff_window_open: () => undefined,
  });
  vi.mocked(emitTo).mockClear();
  render(App);
  await fireEvent.dblClick(await screen.findByRole("treeitem", { name: /a\.txt/ }));
  await vi.waitFor(() => expect(calls("diff_window_open")).toHaveLength(1));

  deliver("diff-ready");

  await vi.waitFor(() =>
    expect(vi.mocked(emitTo)).toHaveBeenCalledWith(
      "main",
      "selection-state",
      expect.objectContaining({ owner: "commit-view" }),
    ),
  );
});

test("without_a_diff_window_nothing_is_emitted", async () => {
  mockApp({ initial_target: () => folder("/repo/a"), repo_open: () => INFO, repo_status: () => [CHANGE] });
  vi.mocked(emitTo).mockClear();
  render(App);

  await screen.findByRole("treeitem", { name: /a\.txt/ });

  expect(emitTo).not.toHaveBeenCalled();
});

test("the_selection_of_the_diff_window_reaches_the_commit_view", async () => {
  const handlers = new Map<string, (event: { payload: unknown }) => void>();
  vi.mocked(listen).mockImplementation(async (event, handler) => {
    handlers.set(event as string, handler as never);
    return () => {};
  });
  mockApp({
    initial_target: () => folder("/repo/a"),
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
    initial_target: () => folder("/repo/a"),
    repo_open: () => {
      if (opened++ === 0) return INFO;
      throw { kind: "failed", message: "There are unsaved changes" };
    },
    settings_get: () => ({ ...NON_MODAL, openProjectIn: "current" }),
    repo_status: () => [CHANGE],
  });
  render(App);
  const message = await screen.findByRole("textbox", { name: "Commit Message" });
  await fireEvent.input(message, { target: { value: "half done" } });

  const input = screen.getByLabelText("Repository path");
  await fireEvent.input(input, { target: { value: "/repo/b" } });
  await fireEvent.submit(input.closest("form")!);

  expect(await screen.findByText("There are unsaved changes")).toBeTruthy();
  expect(
    (screen.getByRole("textbox", { name: "Commit Message" }) as HTMLTextAreaElement).value,
  ).toBe("half done");
});

// Final review: opening the repository this window already has brings the window to the front
// and opens nothing again (`findAndFocusExistingProjectForPath` matches the calling frame): what
// was typed stays.
test("opening_the_repository_this_window_has_only_focuses_it", async () => {
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    settings_get: () => ({ ...NON_MODAL, openProjectIn: "current" }),
    repo_status: () => [CHANGE],
  });
  render(App);
  const message = await screen.findByRole("textbox", { name: "Commit Message" });
  await fireEvent.input(message, { target: { value: "half done" } });
  // The start shows the window once.
  await waitFor(() => expect(calls("show_window")).toHaveLength(1));
  const shown = calls("show_window").length;

  const input = screen.getByLabelText("Repository path");
  await fireEvent.input(input, { target: { value: "/repo/a" } });
  await fireEvent.submit(input.closest("form")!);

  await waitFor(() => expect(calls("show_window")).toHaveLength(shown + 1));
  expect(calls("repo_open")).toHaveLength(1);
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

test("events of another window are not heard", async () => {
  mockApp();
  render(App);
  await screen.findByText("0.0.1");

  const options = (event: string) =>
    vi.mocked(listen).mock.calls.filter(([name]) => name === event).map(([, , target]) => target);
  for (const event of [
    "repo-changed",
    "open-recent",
    "open-project-chooser",
    "open-path",
    "open-settings",
    "leave-refused",
    "commit-draft-save",
    "app-activated",
    "menu-owner-gained",
    "git-action",
  ]) {
    expect(options(event), event).not.toHaveLength(0);
    for (const target of options(event)) expect(target, event).toEqual({ target: "main" });
  }
  expect(options("settings-changed")).not.toHaveLength(0);
  for (const target of options("settings-changed")) expect(target).toBeUndefined();
});

// What the backend notifies the windows about.
function deliver(event: string, payload?: unknown) {
  for (const [name, handler] of vi.mocked(listen).mock.calls) {
    if (name === event) handler({ event: name, id: 0, payload } as never);
  }
}

async function openRepo() {
  mockApp({ repo_open: () => INFO, initial_target: () => folder("/repo/a") });
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

test("Open chooses a folder with the macOS chooser and opens it", async () => {
  vi.mocked(chooseNative).mockReset();
  vi.mocked(chooseNative).mockResolvedValueOnce("/repo/a");
  mockApp({ repo_open: () => INFO });
  render(App);
  await screen.findByText("0.0.1");

  await fireEvent.click(screen.getByRole("button", { name: "Open" }));

  await waitFor(() => expect(calls("repo_open")).toHaveLength(1));
  expect(calls("repo_open")[0][1]).toEqual({ repo: "/repo/a" });
  expect(chooseNative).toHaveBeenCalledWith(
    expect.objectContaining({ title: "Open File or Project", directory: true, multiple: false }),
  );
  expect((screen.getByLabelText("Repository path") as HTMLInputElement).value).toBe("/repo/a");
});

test("a cancelled chooser opens nothing and it starts in the typed folder", async () => {
  vi.mocked(chooseNative).mockReset();
  vi.mocked(chooseNative).mockResolvedValueOnce(null);
  mockApp({ repo_open: () => INFO });
  render(App);
  await screen.findByText("0.0.1");
  const input = screen.getByLabelText("Repository path");
  await fireEvent.input(input, { target: { value: "/repo/b" } });

  await fireEvent.click(screen.getByRole("button", { name: "Open" }));

  await waitFor(() => expect(chooseNative).toHaveBeenCalledTimes(1));
  expect(chooseNative).toHaveBeenCalledWith(expect.objectContaining({ defaultPath: "/repo/b" }));
  await tick();
  expect(calls("repo_open")).toHaveLength(0);
  expect((input as HTMLInputElement).value).toBe("/repo/b");
});

test("Enter in the path field opens the typed path without the chooser", async () => {
  vi.mocked(chooseNative).mockReset();
  mockApp({ repo_open: () => INFO });
  render(App);
  await screen.findByText("0.0.1");
  const input = screen.getByLabelText("Repository path");
  await fireEvent.input(input, { target: { value: "/repo/a" } });

  await fireEvent.submit(input.closest("form")!);

  await waitFor(() => expect(calls("repo_open")).toHaveLength(1));
  expect(calls("repo_open")[0][1]).toEqual({ repo: "/repo/a" });
  expect(chooseNative).not.toHaveBeenCalled();
});

test("the File Open menu opens the chooser", async () => {
  vi.mocked(chooseNative).mockReset();
  vi.mocked(chooseNative).mockResolvedValueOnce("/repo/a");
  mockApp({ repo_open: () => INFO });
  render(App);
  await screen.findByText("0.0.1");

  deliver("open-project-chooser");

  await waitFor(() => expect(calls("repo_open")).toHaveLength(1));
  expect(calls("repo_open")[0][1]).toEqual({ repo: "/repo/a" });
});

test("a recent project from the menu opens like a typed one", async () => {
  mockApp({ repo_open: () => INFO });
  render(App);
  await screen.findByText("0.0.1");

  deliver("open-recent", { path: "/repo/a", exists: true });

  await waitFor(() => expect(calls("repo_open")).toHaveLength(1));
  expect(calls("repo_open")[0][1]).toEqual({ repo: "/repo/a" });
});

test("a project open in another window is focused there and opens nothing here", async () => {
  vi.mocked(chooseNative).mockReset();
  vi.mocked(chooseNative).mockResolvedValueOnce("/repo/b/sub");
  mockApp({
    repo_open: () => INFO,
    project_resolve: () => "/repo/b",
    project_focus_open: () => true,
  });
  render(App);
  await screen.findByText("0.0.1");

  await fireEvent.click(screen.getByRole("button", { name: "Open" }));

  await waitFor(() =>
    expect(calls("project_focus_open")).toEqual([["project_focus_open", { root: "/repo/b" }]]),
  );
  expect(calls("project_resolve")).toEqual([["project_resolve", { target: "/repo/b/sub" }]]);
  await tick();
  expect(calls("repo_open")).toHaveLength(0);
  expect(screen.queryByRole("dialog", { name: "Open Project" })).toBeNull();
});

test("with a repository open, Ask shows Open Project, and This Window opens here", async () => {
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    settings_save: (args) => args?.settings,
  });
  render(App);
  await screen.findByRole("treeitem", { name: /^Changes/ });
  const input = screen.getByLabelText("Repository path");
  await fireEvent.input(input, { target: { value: "/repo/b" } });

  await fireEvent.submit(input.closest("form")!);

  const dialog = await screen.findByRole("dialog", { name: "Open Project" });
  expect(dialog.textContent).toContain("Where would you like to open the project 'b'?");
  expect(calls("repo_open")).toHaveLength(1);
  await fireEvent.click(within(dialog).getByRole("checkbox", { name: "Don't ask again" }));
  await fireEvent.click(within(dialog).getByRole("button", { name: "This Window" }));

  await waitFor(() => expect(calls("repo_open")).toHaveLength(2));
  expect(calls("repo_open")[1][1]).toEqual({ repo: "/repo/b" });
  expect(calls("settings_save")).toEqual([
    ["settings_save", { settings: { ...SETTINGS, openProjectIn: "current" } }],
  ]);
  expect(screen.queryByRole("dialog", { name: "Open Project" })).toBeNull();
});

test("New Window in Open Project opens a new window, and Cancel nothing", async () => {
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    project_new_window: () => ({ label: "main-2", created: true }),
  });
  render(App);
  await screen.findByRole("treeitem", { name: /^Changes/ });

  deliver("open-recent", { path: "/repo/b", exists: true });
  let dialog = await screen.findByRole("dialog", { name: "Open Project" });
  await fireEvent.click(within(dialog).getByRole("button", { name: "New Window" }));
  await waitFor(() =>
    expect(calls("project_new_window")).toEqual([["project_new_window", { root: "/repo/b" }]]),
  );

  deliver("open-recent", { path: "/repo/c", exists: true });
  dialog = await screen.findByRole("dialog", { name: "Open Project" });
  await fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
  await waitFor(() => expect(screen.queryByRole("dialog", { name: "Open Project" })).toBeNull());
  expect(calls("project_new_window")).toHaveLength(1);
  expect(calls("repo_open")).toHaveLength(1);
  expect(calls("settings_save")).toHaveLength(0);
});

test("a root another window started opening meanwhile shows no error here", async () => {
  mockApp({
    repo_open: () => {
      throw { kind: "open_elsewhere", message: "The project is already open in another window" };
    },
  });
  render(App);
  await screen.findByText("0.0.1");
  const input = screen.getByLabelText("Repository path");
  await fireEvent.input(input, { target: { value: "/repo/b" } });

  await fireEvent.submit(input.closest("form")!);

  await waitFor(() => expect(calls("repo_open")).toHaveLength(1));
  await new Promise((resolve) => setTimeout(resolve, 20));
  expect(screen.queryByText("The project is already open in another window")).toBeNull();
});

test("a folder handed to the app goes through the open flow", async () => {
  mockApp({ repo_open: () => INFO });
  render(App);
  await screen.findByText("0.0.1");

  deliver("open-path", "/repo/a/sub");

  await waitFor(() => expect(calls("repo_open")).toHaveLength(1));
  expect(calls("project_resolve")).toEqual([["project_resolve", { target: "/repo/a/sub" }]]);
  expect(calls("repo_open")[0][1]).toEqual({ repo: "/repo/a/sub" });
});

test("a recent project that no longer exists can be removed from the list", async () => {
  mockApp({ repo_open: () => INFO, recent_projects_remove: () => undefined });
  render(App);
  await screen.findByText("0.0.1");

  deliver("open-recent", { path: "/gone/app", exists: false });

  const dialog = await screen.findByRole("dialog", { name: "Reopen Project" });
  expect(dialog.textContent).toContain("The path /gone/app does not exist.");
  expect(dialog.textContent).toContain(
    "If it is on a removable or network drive, please make sure that the drive is connected.",
  );
  await fireEvent.click(within(dialog).getByRole("button", { name: "Remove From List" }));

  await waitFor(() => expect(calls("recent_projects_remove")).toHaveLength(1));
  expect(calls("recent_projects_remove")[0][1]).toEqual({ path: "/gone/app" });
  expect(screen.queryByRole("dialog", { name: "Reopen Project" })).toBeNull();
  expect(calls("repo_open")).toHaveLength(0);
});

test("a missing recent project chosen in the project widget asks to reopen or remove it", async () => {
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    projects_list: () => ({
      open: [],
      recent: [{ path: "/gone/app", name: "app", homePath: "/gone/app", exists: false }],
    }),
    recent_projects_remove: () => undefined,
  });
  render(App);
  await screen.findByText("0.0.1");

  await fireEvent.click(await screen.findByTitle("/repo/a"));
  await fireEvent.click(await screen.findByText("app"));

  const dialog = await screen.findByRole("dialog", { name: "Reopen Project" });
  expect(dialog.textContent).toContain("The path /gone/app does not exist.");
  await fireEvent.click(within(dialog).getByRole("button", { name: "Remove From List" }));
  await waitFor(() => expect(calls("recent_projects_remove")).toEqual([["recent_projects_remove", { path: "/gone/app" }]]));
  expect(calls("project_resolve")).toHaveLength(0);
  // Only the start opened a repository.
  expect(calls("repo_open")).toHaveLength(1);
});

test("with a modal dialog open, File > Open… and Recent Projects do nothing", async () => {
  vi.mocked(chooseNative).mockReset();
  mockApp({ repo_open: () => INFO });
  render(App);
  await screen.findByText("0.0.1");
  deliver("open-settings");
  await screen.findByRole("dialog", { name: "Settings" });

  deliver("open-recent", { path: "/repo/a", exists: true });
  deliver("open-project-chooser");
  await tick();

  expect(calls("repo_open")).toHaveLength(0);
  expect(chooseNative).not.toHaveBeenCalled();
});

test("OK keeps a recent project that no longer exists in the list", async () => {
  mockApp({ repo_open: () => INFO, recent_projects_remove: () => undefined });
  render(App);
  await screen.findByText("0.0.1");

  deliver("open-recent", { path: "/gone/app", exists: false });

  const dialog = await screen.findByRole("dialog", { name: "Reopen Project" });
  await fireEvent.click(within(dialog).getByRole("button", { name: "OK" }));

  await waitFor(() => expect(screen.queryByRole("dialog", { name: "Reopen Project" })).toBeNull());
  expect(calls("recent_projects_remove")).toHaveLength(0);
  expect(calls("repo_open")).toHaveLength(0);
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
    initial_target: () => folder("/repo/a"),
    settings_get: () => ({ ...SETTINGS, nonModalCommit: true, stagingArea: true }),
  });
  render(App);

  await screen.findByRole("treeitem", { name: /^Staged/ });

  expect(calls("changelists_get")).toHaveLength(0);
});

test("changing the git path refreshes what is shown", async () => {
  await openRepo();

  deliver("settings-changed", { ...SETTINGS, gitPath: "/other/git" });

  await waitFor(() => expect(emitTo).toHaveBeenCalledWith("main", "repo-changed", undefined));
});

test("the_toolbar_branch_follows_the_repository_without_remounting_the_views", async () => {
  mockApp({
    initial_target: () => folder("/repo/a"),
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
    initial_target: () => folder("/repo/a"),
    repo_open: () => ({ ...INFO, state: REBASING }),
  });
  render(App);

  expect(await screen.findByRole("button", { name: "Git Branch: Rebasing main" })).toBeTruthy();
});

test("the_widget_goes_from_one_operation_to_another_with_repo_changed", async () => {
  let state: unknown = REBASING;
  mockApp({
    initial_target: () => folder("/repo/a"),
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

// The menu bar is the app's: the window that gains the focus puts its own state in it again.
test("gaining focus syncs the Git menu", async () => {
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
  });
  render(App);
  await screen.findByRole("button", { name: "Git Branch: main" });
  await waitFor(() => expect(calls("git_menu_enable").length).toBeGreaterThan(0));
  const before = calls("git_menu_enable").length;

  window.dispatchEvent(new Event("focus"));

  await waitFor(() => expect(calls("git_menu_enable").length).toBe(before + 1));
  expect(lastIdsOf()).toContain("Git.Merge");
});

// Rust hands the menu to this window (its own focus, or its viewer's): it says its state again.
test("becoming the menu owner re-sends the Git menu state", async () => {
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
  });
  render(App);
  await screen.findByRole("button", { name: "Git Branch: main" });
  await waitFor(() => expect(calls("git_menu_enable").length).toBeGreaterThan(0));
  const before = calls("git_menu_enable").length;

  deliver("menu-owner-gained");

  await waitFor(() => expect(calls("git_menu_enable").length).toBe(before + 1));
  expect(lastIdsOf()).toContain("Git.Merge");
});

// Final review: the menu goes to this window while one of its windows other than the viewer has
// the focus (its diff window, say), or before its own focus arrives. Current File is then this
// window's, with nothing to act on, not the previous owner's. The viewer says its own.
test("becoming the menu owner without the focus clears Current File", async () => {
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    git_menu_current_file: () => undefined,
  });
  render(App);
  await screen.findByRole("button", { name: "Git Branch: main" });
  await fireEvent.blur(window);
  const before = calls("git_menu_current_file").length;

  deliver("menu-owner-gained", "diff");

  await waitFor(() => expect(calls("git_menu_current_file").length).toBe(before + 1));
  const sent = calls("git_menu_current_file").at(-1)?.[1] as { title: string; items: { enabled: boolean }[] };
  expect(sent.title).toBe("Current File");
  expect(sent.items.every((item) => !item.enabled)).toBe(true);

  deliver("menu-owner-gained", "viewer");
  await new Promise((resolve) => setTimeout(resolve, 20));
  expect(calls("git_menu_current_file").length).toBe(before + 1);
});

function lastIdsOf(): string[] {
  return (calls("git_menu_enable").at(-1)?.[1] as { ids: string[] }).ids;
}

test("opening_a_half_done_repository_shows_the_state", async () => {
  mockApp({
    initial_target: () => folder("/repo/a"),
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
    initial_target: () => folder("/repo/a"),
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
    initial_target: () => folder("/repo/a"),
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
    initial_target: () => folder("/repo/a"),
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
    initial_target: () => folder("/repo/a"),
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
    repository: "ramify",
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

  expect(await screen.findByRole("dialog", { name: "Push Commits to ramify" })).toBeTruthy();
  expect(calls("commit_changes")).toHaveLength(1);
});

test("cmd_shift_k_opens_the_dialog", async () => {
  openWithBranches(PUSH_COMMANDS);
  render(App);
  await screen.findByRole("button", { name: "Git Branch: main" });

  await fireEvent.keyDown(window, { key: "K", code: "KeyK", metaKey: true, shiftKey: true });

  expect(await screen.findByRole("dialog", { name: "Push Commits to ramify" })).toBeTruthy();
  expect(calls("push_info")).toEqual([["push_info", { branch: null }]]);
});

test("push_from_the_popup_opens_the_dialog", async () => {
  openWithBranches(PUSH_COMMANDS);
  render(App);
  await fireEvent.click(await screen.findByRole("button", { name: "Git Branch: main" }));

  await fireEvent.click(await screen.findByRole("treeitem", { name: "Push…" }));

  expect(await screen.findByRole("dialog", { name: "Push Commits to ramify" })).toBeTruthy();
});

test("the_git_menu_also_opens_push", async () => {
  openWithBranches(PUSH_COMMANDS);
  render(App);
  await screen.findByRole("button", { name: "Git Branch: main" });

  deliver("git-action", "Vcs.Push");

  expect(await screen.findByRole("dialog", { name: "Push Commits to ramify" })).toBeTruthy();
});

test("on_open_what_was_saved_comes_back_and_changing_repository_withdraws_it", async () => {
  const pending = [{ sha: "abc", title: "Update", message: "Uncommitted changes before Update" }];
  const merging = { ...INFO.state, operation: "merging", label: "Merging main" };
  const opens = [{ ...INFO, pending, state: merging }, INFO];
  mockApp({ repo_open: () => opens.shift(), settings_get: () => CURRENT_WINDOW });
  render(App);
  await screen.findByText("0.0.1");
  const input = screen.getByLabelText("Repository path");
  await fireEvent.input(input, { target: { value: "/repo/a" } });
  await fireEvent.submit(input.closest("form")!);

  expect(await screen.findByRole("button", { name: "Restore Changes" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Abort" })).toBeTruthy();
  // The first open has finished: another one is not ignored.
  await waitFor(() =>
    expect((screen.getByRole("button", { name: "Open" }) as HTMLButtonElement).disabled).toBe(false),
  );

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
    initial_target: () => folder("/repo/a"),
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
    initial_target: () => folder("/repo/a"),
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
  expect(calls("project_new_window")).toHaveLength(0);
});

test("clone_with_an_open_repository_goes_where_open_project_in_says", async () => {
  openWithBranches({
    clone_repository: finishing(6, { path: "/work/copy" }),
    settings_get: () => ({ ...SETTINGS, openProjectIn: "new" }),
    project_new_window: () => ({ label: "main-2", created: true }),
  });
  render(App);
  await screen.findByRole("button", { name: "Git Branch: main" });

  deliver("git-action", "Git.Clone");
  await fillClone("/srv/a.git", "/work/copy");

  await waitFor(() =>
    expect(calls("project_new_window")).toEqual([["project_new_window", { root: "/work/copy" }]]),
  );
  expect(calls("project_resolve")).toEqual([["project_resolve", { target: "/work/copy" }]]);
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
  const push = await screen.findByRole("dialog", { name: "Push Commits to ramify" });

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
    initial_target: () => folder("/repo/a"),
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
  mockApp({ initial_target: () => folder("/repo/a"), repo_open: () => INFO });
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
    initial_target: () => folder("/repo/a"),
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
    initial_target: () => folder("/repo/a"),
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
    initial_target: () => folder("/repo/a"),
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
    initial_target: () => folder("/repo/a"),
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
  mockApp({ initial_target: () => folder("/repo/a"), repo_open: () => INFO });
  render(App);
  await screen.findByRole("button", { name: "Git Branch: main" });

  expect(screen.queryByRole("button", { name: "Continue Rebase" })).toBeNull();
  expect(screen.queryByRole("button", { name: "Abort Merge" })).toBeNull();
});

// `stash_list` is a task: its result arrives through `task-changed`.
const STASH_LIST_TASK = 4242;
const STASH = {
  sha: "5".repeat(40),
  parents: ["h".repeat(40)],
  time: 1_700_000_000_000,
  index: 0,
  subject: "my work",
  branch: "main",
  prefix: "On main",
  message: "my work",
};

// The watcher's payload when `refs/stash` or its reflog changed in that quiet period.
const STASH_TOUCHED = { stash: true, shelf: false };
// What `repo-changed` says after a command wrote the open repository's shelf.
const SHELF_TOUCHED = { stash: false, shelf: true };

const stashCommands = (list: () => unknown[]) => ({
  stash_list: () => {
    deliver("task-changed", taskDone(STASH_LIST_TASK, list()));
    return STASH_LIST_TASK;
  },
  stash_changes: finishing(4243, { files: [], groups: [] }),
});

test("the_stash_tab_appears_with_the_first_stash_and_leaves_with_the_last", async () => {
  let stashes: unknown[] = [];
  mockApp({ initial_target: () => folder("/repo/a"), repo_open: () => INFO, ...stashCommands(() => stashes) });
  render(App);
  const strip = await screen.findByRole("tablist", { name: "Git" });
  await waitFor(() => expect(calls("stash_list").length).toBeGreaterThan(0));
  expect(within(strip).queryByRole("tab", { name: "Stash" })).toBeNull();

  stashes = [STASH];
  deliver("repo-changed", STASH_TOUCHED);

  const tab = await within(strip).findByRole("tab", { name: "Stash" });
  expect(within(strip).getAllByRole("tab").map((item) => item.textContent?.trim())).toEqual([
    "Local Changes",
    "Stash",
    "Log",
    "Console",
    "Terminal",
  ]);
  expect(tab.getAttribute("aria-selected")).toBe("false");
  await fireEvent.click(tab);
  expect(tab.getAttribute("aria-selected")).toBe("true");
  expect(await screen.findByRole("treeitem", { name: "my work" })).toBeTruthy();

  stashes = [];
  deliver("repo-changed", STASH_TOUCHED);

  await waitFor(() => expect(within(strip).queryByRole("tab", { name: "Stash" })).toBeNull());
  expect(within(strip).getByRole("tab", { name: "Local Changes" }).getAttribute("aria-selected")).toBe(
    "true",
  );
  // It comes back unselected: the selection does not wait for it.
  stashes = [STASH];
  deliver("repo-changed", STASH_TOUCHED);
  const back = await within(strip).findByRole("tab", { name: "Stash" });
  expect(back.getAttribute("aria-selected")).toBe("false");
});

test("the_window_label_follows_the_window", async () => {
  let stashes: unknown[] = [];
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    settings_get: () => NON_MODAL,
    log_open: () => ({ viewId: 1, count: 0 }),
    ...stashCommands(() => stashes),
  });
  render(App);
  await screen.findByRole("textbox", { name: "Commit Message" });
  // A single content: the Commit window shows no strip.
  expect(screen.queryByRole("tablist", { name: "Commit" })).toBeNull();

  stashes = [STASH];
  deliver("repo-changed", STASH_TOUCHED);

  const commit = await screen.findByRole("tablist", { name: "Commit" });
  expect(within(commit).getAllByRole("tab").map((item) => item.textContent?.trim())).toEqual([
    "Commit",
    "Stash",
  ]);
  expect(screen.queryByRole("tablist", { name: "Git" })).toBeNull();
  const stripe = screen.getByRole("tablist", { name: "Tool windows" });
  await fireEvent.click(within(stripe).getByRole("tab", { name: "Git" }));
  const git = screen.getByRole("tablist", { name: "Git" });
  expect(within(git).getAllByRole("tab").map((item) => item.textContent?.trim())).toEqual([
    "Log",
    "Console",
    "Terminal",
  ]);
  expect(screen.queryByRole("tablist", { name: "Commit" })).toBeNull();
});

test("unstash_changes_shows_the_stash_tab_and_opens_the_dialog_only_without_stashes", async () => {
  let stashes: unknown[] = [STASH];
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    branch_list: () => ({ local: [], remote: [], recent: [], current: "main", previous: null }),
    ...stashCommands(() => stashes),
  });
  render(App);
  const strip = await screen.findByRole("tablist", { name: "Git" });
  const tab = await within(strip).findByRole("tab", { name: "Stash" });

  deliver("git-action", "Git.Unstash");

  await waitFor(() => expect(tab.getAttribute("aria-selected")).toBe("true"));
  expect(screen.queryByRole("dialog", { name: "Unstash Changes" })).toBeNull();

  stashes = [];
  deliver("repo-changed", STASH_TOUCHED);
  await waitFor(() => expect(within(strip).queryByRole("tab", { name: "Stash" })).toBeNull());
  deliver("git-action", "Git.Unstash");

  expect(await screen.findByRole("dialog", { name: "Unstash Changes" })).toBeTruthy();
});

// `GitStashTracker`: the list is read again only when the stash ref changed, not on every save.
test("a_worktree_save_does_not_read_the_stash_list_and_a_stash_push_does", async () => {
  mockApp({ initial_target: () => folder("/repo/a"), repo_open: () => INFO, ...stashCommands(() => [STASH]) });
  render(App);
  const strip = await screen.findByRole("tablist", { name: "Git" });
  await within(strip).findByRole("tab", { name: "Stash" });
  expect(calls("stash_list")).toHaveLength(1);
  const states = calls("repo_state").length;

  deliver("repo-changed", { stash: false, shelf: false });
  await waitFor(() => expect(calls("repo_state").length).toBeGreaterThan(states));
  await new Promise((resolve) => setTimeout(resolve, 20));
  expect(calls("stash_list")).toHaveLength(1);

  deliver("repo-changed", STASH_TOUCHED);
  await waitFor(() => expect(calls("stash_list")).toHaveLength(2));
});

// `SHELF_TOPIC`: the lists are read again only when a command wrote the store, not on every save
// of a file in the repository.
test("a_worktree_save_does_not_read_the_shelf_and_a_change_to_the_store_does", async () => {
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    repo_settings_get: () => ({ shelf: { removeStrategy: false, showRecycled: false, groupByDirectory: false } }),
    shelf_lists: () => {
      deliver("task-changed", taskDone(4320, [SHELF_LIST]));
      return 4320;
    },
  });
  render(App);
  const strip = await screen.findByRole("tablist", { name: "Git" });
  await within(strip).findByRole("tab", { name: "Shelf" });
  expect(calls("shelf_lists")).toHaveLength(1);
  const states = calls("repo_state").length;

  deliver("repo-changed", { stash: false, shelf: false });
  deliver("repo-changed", null);
  await waitFor(() => expect(calls("repo_state").length).toBeGreaterThan(states + 1));
  await new Promise((resolve) => setTimeout(resolve, 20));
  expect(calls("shelf_lists")).toHaveLength(1);

  deliver("repo-changed", SHELF_TOUCHED);
  await waitFor(() => expect(calls("shelf_lists")).toHaveLength(2));
});

// A read of the lists that failed is tried again on the next `repo-changed`, whatever it says;
// once read, only a change to the store reads them again.
test("a_failed_shelf_read_is_retried_on_the_next_change", async () => {
  let fails = true;
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    repo_settings_get: () => ({ shelf: { removeStrategy: false, showRecycled: false, groupByDirectory: false } }),
    shelf_lists: () => {
      if (fails) throw { kind: "io", message: "the store cannot be read" };
      deliver("task-changed", taskDone(4321, [SHELF_LIST]));
      return 4321;
    },
  });
  render(App);
  await screen.findByRole("tab", { name: "Local Changes" });
  await waitFor(() => expect(calls("shelf_lists")).toHaveLength(1));
  expect(screen.queryByRole("tab", { name: "Shelf" })).toBeNull();
  fails = false;

  deliver("repo-changed", { stash: false, shelf: false });

  const strip = screen.getByRole("tablist", { name: "Git" });
  expect(await within(strip).findByRole("tab", { name: "Shelf" })).toBeTruthy();
  expect(calls("shelf_lists")).toHaveLength(2);
  const states = calls("repo_state").length;
  deliver("repo-changed", { stash: false, shelf: false });
  await waitFor(() => expect(calls("repo_state").length).toBeGreaterThan(states));
  await new Promise((resolve) => setTimeout(resolve, 20));
  expect(calls("shelf_lists")).toHaveLength(2);
});

// `GitStashTracker.Stashes.Error` keeps the tab, with the error as its only node.
test("a_stash_list_that_fails_shows_an_error_node", async () => {
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    stash_list: () => {
      throw { kind: "git", message: "fatal: bad stash" };
    },
  });
  render(App);
  const strip = await screen.findByRole("tablist", { name: "Git" });

  await fireEvent.click(await within(strip).findByRole("tab", { name: "Stash" }));

  const list = screen.getByRole("tree", { name: "Stashes" });
  expect(within(list).getByText("fatal: bad stash")).toBeTruthy();
  expect((screen.getByRole("button", { name: "Apply" }) as HTMLButtonElement).disabled).toBe(true);
});

// `VcsShowToolWindowTabAction` at the level of the app: active and selected does nothing (Ramify
// has no hidden tool window), active elsewhere selects, inactive activates the tab.
test("show_git_stash_selects_activates_or_leaves_the_tab", async () => {
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    log_open: () => ({ viewId: 1, count: 0 }),
    ...stashCommands(() => [STASH]),
  });
  render(App);
  const strip = await screen.findByRole("tablist", { name: "Git" });
  const tab = await within(strip).findByRole("tab", { name: "Stash" });
  await fireEvent.click(tab);
  const list = await screen.findByRole("tree", { name: "Stashes" });
  await waitFor(() => expect(document.activeElement).toBe(list));

  deliver("git-action", "Git.Show.Stash");
  await new Promise((resolve) => setTimeout(resolve, 20));
  expect(tab.getAttribute("aria-selected")).toBe("true");
  expect(document.activeElement).toBe(list);

  const log = within(strip).getByRole("tab", { name: "Log" });
  await fireEvent.click(log);
  log.focus();
  deliver("git-action", "Git.Show.Stash");
  await waitFor(() => expect(tab.getAttribute("aria-selected")).toBe("true"));

  const path = screen.getByLabelText("Repository path");
  path.focus();
  deliver("git-action", "Git.Show.Stash");
  await waitFor(() => expect(document.activeElement).toBe(list));
});

// `ChangesView.CreatePatch` of the Git menu: the Create Patch dialog with the active list, whose
// `Create Patch…` opens Patch File Settings on top of it. With the staging area it does nothing.
test("the_git_menu_create_patch_opens_create_patch_then_patch_file_settings", async () => {
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    repo_status: () => [CHANGE],
    ...DIALOG_COMMANDS,
    messages_push: () => undefined,
    changelists_set_comment: () => LISTS,
    patch_settings_get: () => ({ folder: null, toClipboard: false, showInFinder: null }),
    patch_paths: () => ({ file: "/repo/a/m.patch", fileExists: false, base: "/repo/a", baseExists: true }),
  });
  render(App);
  await screen.findByRole("treeitem", { name: /a\.txt/ });

  deliver("git-action", "ChangesView.CreatePatch");

  const dialog = await screen.findByRole("dialog", { name: "Create Patch" });
  expect(await within(dialog).findByRole("treeitem", { name: "a.txt" })).toBeTruthy();
  await fireEvent.input(within(dialog).getByRole("textbox", { name: "Commit Message" }), {
    target: { value: "m" },
  });
  await fireEvent.click(within(dialog).getByRole("button", { name: "Create Patch…" }));

  const settings = await screen.findByRole("dialog", { name: "Patch File Settings" });
  expect((within(settings).getByRole("textbox", { name: "To file:" }) as HTMLInputElement).value).toBe(
    "/repo/a/m.patch",
  );
  // On top: it comes after the Create Patch dialog in the document.
  expect(dialog.compareDocumentPosition(settings) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  await fireEvent.click(within(settings).getByRole("button", { name: "Cancel" }));
  expect(screen.getByRole("dialog", { name: "Create Patch" })).toBeTruthy();
});

test("the_git_menu_create_patch_does_nothing_with_the_staging_area", async () => {
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    settings_get: () => ({ ...SETTINGS, nonModalCommit: true, stagingArea: true }),
    branch_list: () => ({ local: [], remote: [], recent: [], current: null, previous: null }),
  });
  render(App);
  await screen.findByText("0.0.1");
  await waitFor(() => expect(calls("repo_open")).toHaveLength(1));

  deliver("git-action", "ChangesView.CreatePatch");
  // Waiting for the New Branch dialog, which comes after a round trip to the backend, gives Create
  // Patch the same time to show its dialog: it did not.
  deliver("git-action", "Git.CreateNewBranch");
  await screen.findByRole("dialog", { name: "Create New Branch" });

  expect(screen.queryByRole("dialog", { name: "Create Patch" })).toBeNull();
});

// `ChangesView.Shelve` of the Git menu: the Shelve Changes dialog, which shelves what it includes
// with rollback, from the repository it was opened in. Not with the staging area.
test("the_git_menu_shelve_changes_opens_shelve_changes_and_shelves_the_included_part", async () => {
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    repo_status: () => [CHANGE],
    ...DIALOG_COMMANDS,
    messages_push: () => undefined,
    changelists_set_comment: () => LISTS,
    shelf_shelve: () => 9,
  });
  render(App);
  await screen.findByRole("treeitem", { name: /a\.txt/ });

  deliver("git-action", "ChangesView.Shelve");

  const dialog = await screen.findByRole("dialog", { name: "Shelve Changes" });
  expect(await within(dialog).findByRole("treeitem", { name: "a.txt" })).toBeTruthy();
  await fireEvent.input(within(dialog).getByRole("textbox", { name: "Commit Message" }), {
    target: { value: "Later" },
  });
  await fireEvent.click(within(dialog).getByRole("button", { name: "Shelve Changes" }));

  await waitFor(() => expect(calls("shelf_shelve")).toHaveLength(1));
  expect(calls("shelf_shelve")[0][1]).toEqual({
    entries: [{ path: "a.txt", oldPath: null, selection: null }],
    description: "Later",
    rollback: true,
    expectedRepo: "/repo/a",
  });
  expect(screen.queryByRole("dialog", { name: "Shelve Changes" })).toBeNull();
});

// P26: Ramify's own `Import Shelf from .idea…` of Git › Uncommitted Changes runs the Shelf tab's
// import, also while the shelf is empty and the tab is not there; the lists it adds bring the tab.
test("the_git_menu_import_shelf_from_idea_imports_while_the_shelf_is_empty", async () => {
  let lists: unknown[] = [];
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    repo_settings_get: () => ({ shelf: { removeStrategy: false, showRecycled: false, groupByDirectory: false } }),
    shelf_lists: () => {
      deliver("task-changed", taskDone(4330, lists));
      return 4330;
    },
    shelf_import_idea: () => {
      lists = [SHELF_LIST];
      deliver("task-changed", taskDone(4331, { added: 1, skipped: 0 }));
      deliver("repo-changed", SHELF_TOUCHED);
      return 4331;
    },
  });
  render(App);
  const strip = await screen.findByRole("tablist", { name: "Git" });
  await waitFor(() => expect(calls("shelf_lists")).toHaveLength(1));
  expect(within(strip).queryByRole("tab", { name: "Shelf" })).toBeNull();

  deliver("git-action", "Shelf.ImportFromIdea");

  await waitFor(() => expect(calls("shelf_import_idea")).toHaveLength(1));
  expect(calls("shelf_import_idea")[0][1]).toEqual({ expectedRepo: "/repo/a" });
  expect(await within(strip).findByRole("tab", { name: "Shelf" })).toBeTruthy();
});

// Finding 8: `ChangesView.Revert` of Git › Uncommitted Changes is Local Changes' Rollback.
test("the_git_menu_rollback_opens_the_rollback_of_local_changes", async () => {
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    repo_status: () => [CHANGE],
  });
  render(App);
  await screen.findByRole("treeitem", { name: /a\.txt/ });

  deliver("git-action", "ChangesView.Revert");

  const dialog = await screen.findByRole("dialog", { name: "Rollback Changes" });
  expect(within(dialog).getByRole("checkbox", { name: "a.txt" })).toHaveProperty("checked", true);
});

test("the_git_menu_shelve_changes_does_nothing_with_the_staging_area", async () => {
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    settings_get: () => ({ ...SETTINGS, nonModalCommit: true, stagingArea: true }),
    branch_list: () => ({ local: [], remote: [], recent: [], current: null, previous: null }),
  });
  render(App);
  await screen.findByText("0.0.1");
  await waitFor(() => expect(calls("repo_open")).toHaveLength(1));

  deliver("git-action", "ChangesView.Shelve");
  deliver("git-action", "Git.CreateNewBranch");
  await screen.findByRole("dialog", { name: "Create New Branch" });

  expect(screen.queryByRole("dialog", { name: "Shelve Changes" })).toBeNull();
});

// `Vcs.Show.Shelf` (`VcsShowToolWindowTabAction`): nothing without the tab; otherwise it shows the
// tab, and the step that would hide it leaves it as it is.
const SHELF_LIST = {
  list: {
    name: "Fix_it",
    description: "Fix it",
    dateMs: 1_700_000_000_000,
    recycled: false,
    toDelete: false,
    deleted: false,
    patch: "/data/shelf/Fix_it/shelved.patch",
    binaries: [],
  },
  changes: [{ file: { before: "a.txt", after: "a.txt", binary: false }, path: "a.txt", oldPath: null, status: "M" }],
  error: null,
};

test("show_shelf_toggles", async () => {
  const { takeShelfTabRequest } = await import("./shelf/flow");
  takeShelfTabRequest();
  let lists: unknown[] = [];
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    repo_status: () => [CHANGE],
    repo_settings_get: () => ({ shelf: { removeStrategy: false, showRecycled: false, groupByDirectory: false } }),
    shelf_lists: () => {
      deliver("task-changed", taskDone(4300, lists));
      return 4300;
    },
    shelf_sides: finishing(4301, { left: { kind: "text", content: "" }, right: { kind: "text", content: "" }, titles: ["", ""] }),
  });
  render(App);
  await screen.findByRole("treeitem", { name: /a\.txt/ });
  const strip = await screen.findByRole("tablist", { name: "Git" });

  // No list, no tab: the action does nothing and leaves nothing behind.
  deliver("git-action", "Vcs.Show.Shelf");
  expect(within(strip).queryByRole("tab", { name: "Shelf" })).toBeNull();
  expect(takeShelfTabRequest()).toBeNull();

  lists = [SHELF_LIST];
  deliver("repo-changed", SHELF_TOUCHED);
  const tab = await within(strip).findByRole("tab", { name: "Shelf" });
  expect(within(strip).getAllByRole("tab").map((item) => item.textContent?.trim())).toEqual([
    "Local Changes",
    "Shelf",
    "Log",
    "Console",
    "Terminal",
  ]);
  expect(tab.getAttribute("aria-selected")).toBe("false");

  deliver("git-action", "Vcs.Show.Shelf");
  await waitFor(() => expect(tab.getAttribute("aria-selected")).toBe("true"));
  const tree = screen.getByRole("tree", { name: "Shelf" });
  await waitFor(() => expect(document.activeElement).toBe(tree));
  // In front with the focus inside: the hiding step, which Ramify has not, changes nothing.
  deliver("git-action", "Vcs.Show.Shelf");
  expect(tab.getAttribute("aria-selected")).toBe("true");

  // A request with a list (after Shelve Changes) selects it.
  await fireEvent.click(within(strip).getByRole("tab", { name: "Local Changes" }));
  const { showShelf } = await import("./shelf/flow");
  showShelf("Fix_it");
  await waitFor(() => expect(tab.getAttribute("aria-selected")).toBe("true"));
  expect(screen.getByRole("treeitem", { name: /^Fix it/ }).getAttribute("aria-selected")).toBe("true");
});

// Shelve Changes…, Import to Shelf and Import Patches… ask for their new list with `showShelf(name)`
// while the tab already exists and has not read that list yet: it is selected once read.
test("a_new_list_asked_for_while_the_shelf_tab_exists_is_selected_when_it_arrives", async () => {
  const { takeShelfTabRequest, showShelf } = await import("./shelf/flow");
  takeShelfTabRequest();
  const NEW = { ...SHELF_LIST, list: { ...SHELF_LIST.list, name: "New_one", description: "New one", dateMs: 1_800_000_000_000 } };
  let lists: unknown[] = [SHELF_LIST];
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    repo_status: () => [CHANGE],
    repo_settings_get: () => ({ shelf: { removeStrategy: false, showRecycled: false, groupByDirectory: false } }),
    shelf_lists: () => {
      deliver("task-changed", taskDone(4310, lists));
      return 4310;
    },
    shelf_sides: finishing(4311, { left: { kind: "text", content: "" }, right: { kind: "text", content: "" }, titles: ["", ""] }),
  });
  render(App);
  const strip = await screen.findByRole("tablist", { name: "Git" });
  const tab = await within(strip).findByRole("tab", { name: "Shelf" });

  showShelf("New_one");
  await waitFor(() => expect(tab.getAttribute("aria-selected")).toBe("true"));
  expect(screen.queryByRole("treeitem", { name: /^New one/ })).toBeNull();

  lists = [NEW, SHELF_LIST];
  deliver("repo-changed", SHELF_TOUCHED);
  await waitFor(() =>
    expect(screen.getByRole("treeitem", { name: /^New one/ }).getAttribute("aria-selected")).toBe("true"),
  );
  expect(screen.getByRole("treeitem", { name: /^Fix it/ }).getAttribute("aria-selected")).toBe("false");
});

const APPLY_PLAN = {
  subject: null,
  message: null,
  author: null,
  additionalInfoError: null,
  readError: null,
  files: [
    {
      index: 0,
      path: "a.txt",
      before: "a.txt",
      after: "a.txt",
      original: "a.txt",
      strip: 0,
      maxStrip: 0,
      kind: "modified",
      binary: false,
      base: false,
      status: "applicable",
      error: null,
      hashes: { "a.txt": "h" },
    },
  ],
};

// `ChangesView.ApplyPatch` of the Git menu's `Patch` submenu: `Select Patch File`, then the dialog
// over that file with the repository's lists; OK applies it with the HEAD it was opened on.
test("the_git_menu_apply_patch_opens_select_patch_file_then_the_dialog", async () => {
  vi.mocked(chooseNative).mockResolvedValue("/patches/fix.patch");
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    patch_app_settings_get: () => ({ folder: "/patches", detectPatchOnTheFly: false }),
    patch_storage_set: () => undefined,
    patch_plan: finishing(61, APPLY_PLAN),
    patch_apply: finishing(62, { snapshot: "s", status: "success", failed: [], conflicts: [], messages: [] }),
  });
  render(App);
  await screen.findByRole("treeitem", { name: /^Changes/ });

  deliver("git-action", "ChangesView.ApplyPatch");

  const dialog = await screen.findByRole("dialog", { name: "Apply Patch" });
  expect(within(dialog).getByRole("treeitem", { name: /^a\.txt +Modified$/ })).toBeTruthy();
  expect(calls("patch_storage_set")[0][1]).toEqual({ folder: "/patches" });
  await fireEvent.click(within(dialog).getByRole("button", { name: "OK" }));
  await waitFor(() => expect(calls("patch_apply")).toHaveLength(1));
  expect(calls("patch_apply")[0][1]).toMatchObject({
    source: { kind: "file", path: "/patches/fix.patch" },
    expectedHead: INFO.head,
    choice: { selected: ["a.txt"], overwrite: [], expected: { "a.txt": "h" } },
  });
  expect(await screen.findByText("Patch successfully applied")).toBeTruthy();
});

// Reading the lists with no paths told the backend no file had changes, and it forgot every file's
// list: after an Apply Patch, even a cancelled one, all the changes fell into the active list.
test("apply_patch_reads_the_lists_without_forgetting_which_list_each_file_is_in", async () => {
  vi.mocked(chooseNative).mockResolvedValue("/patches/fix.patch");
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    repo_status: () => [CHANGE],
    patch_app_settings_get: () => ({ folder: "/patches", detectPatchOnTheFly: false }),
    patch_storage_set: () => undefined,
    patch_plan: finishing(66, APPLY_PLAN),
    patch_apply: finishing(67, { snapshot: "s", status: "success", failed: [], conflicts: [], messages: [] }),
  });
  render(App);
  await screen.findByRole("treeitem", { name: /a\.txt/ });

  deliver("git-action", "ChangesView.ApplyPatch");
  const dialog = await screen.findByRole("dialog", { name: "Apply Patch" });
  await fireEvent.click(within(dialog).getByRole("button", { name: "OK" }));
  await waitFor(() => expect(calls("patch_apply")).toHaveLength(1));
  expect(await screen.findByText("Patch successfully applied")).toBeTruthy();

  const forgetting = calls("changelists_get").filter(([, args]) => (args as { paths: string[] }).paths.length === 0);
  expect(forgetting).toEqual([]);
  expect(calls("changelists_lists").length).toBeGreaterThan(0);
});

// P22: from OK to its end an apply is modal, as IntelliJ's merge window for its conflicts is: the
// main window is inert meanwhile, and live again after.
test("the_main_window_is_inert_while_an_apply_runs", async () => {
  vi.mocked(chooseNative).mockResolvedValue("/patches/fix.patch");
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    patch_app_settings_get: () => ({ folder: "/patches", detectPatchOnTheFly: false }),
    patch_storage_set: () => undefined,
    patch_plan: finishing(64, APPLY_PLAN),
    patch_apply: () => 65,
  });
  render(App);
  await screen.findByRole("treeitem", { name: /^Changes/ });
  // jsdom has no `inert`: Svelte sets the property, a browser reflects it as the attribute.
  const inert = (selector: string) => {
    const element = document.querySelector(selector) as HTMLElement;
    return element.inert === true || element.hasAttribute("inert");
  };
  expect(inert(".workspace")).toBe(false);

  deliver("git-action", "ChangesView.ApplyPatch");
  const dialog = await screen.findByRole("dialog", { name: "Apply Patch" });
  await fireEvent.click(within(dialog).getByRole("button", { name: "OK" }));
  await waitFor(() => expect(calls("patch_apply")).toHaveLength(1));

  await waitFor(() => expect(inert(".workspace")).toBe(true));
  expect(inert(".main-toolbar")).toBe(true);
  deliver("task-changed", taskDone(65, { snapshot: "s", status: "success", failed: [], conflicts: [], messages: [] }));
  expect(await screen.findByText("Patch successfully applied")).toBeTruthy();
  await waitFor(() => expect(inert(".workspace")).toBe(false));
  expect(inert(".main-toolbar")).toBe(false);
});

// `PatchClipboardListener`: with `Analyze and apply patch from clipboard on the fly` on, the app
// becoming active (the backend's `app-activated`, not a move between Ramify's own windows) offers
// the dialog for a patch on the clipboard, once for that text.
test("on_the_fly_offers_the_dialog_once_per_clipboard_text", async () => {
  const text = "--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-a\n+b\n";
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    patch_app_settings_get: () => ({ folder: null, detectPatchOnTheFly: true }),
    patch_clipboard: () => ({ length: text.length, hash: "t1", patch: text }),
    patch_plan: finishing(63, APPLY_PLAN),
  });
  render(App);
  await screen.findByRole("treeitem", { name: /^Changes/ });

  // Coming back from the merge or diff window is not activating the app.
  window.dispatchEvent(new Event("focus"));
  await new Promise((resolve) => setTimeout(resolve, 30));
  expect(calls("patch_clipboard")).toHaveLength(0);

  deliver("app-activated", "main");
  const dialog = await screen.findByRole("dialog", { name: "Apply Patch" });
  expect(within(dialog).queryByRole("textbox", { name: "Patch file name:" })).toBeNull();
  expect(calls("patch_plan")[0][1]).toEqual({ source: { kind: "text", text }, mapping: { files: [] } });
  await fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));

  deliver("app-deactivated");
  deliver("app-activated", "main");
  await new Promise((resolve) => setTimeout(resolve, 30));
  expect(screen.queryByRole("dialog", { name: "Apply Patch" })).toBeNull();
  expect(calls("patch_plan")).toHaveLength(1);
});

// Final review: every main window hears `app-activated`, with the project window that got the
// focus. Only that one looks at the clipboard: IntelliJ offers the patch in the activated frame.
test("on_the_fly_asks_only_in_the_activated_window", async () => {
  const text = "--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-a\n+b\n";
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    patch_app_settings_get: () => ({ folder: null, detectPatchOnTheFly: true }),
    patch_clipboard: () => ({ length: text.length, hash: "t1", patch: text }),
    patch_plan: finishing(66, APPLY_PLAN),
  });
  render(App);
  await screen.findByRole("treeitem", { name: /^Changes/ });

  // Another project window, or one of its auxiliary windows, was activated.
  deliver("app-activated", "main-2");
  await new Promise((resolve) => setTimeout(resolve, 30));
  expect(calls("patch_clipboard")).toHaveLength(0);
  expect(screen.queryByRole("dialog", { name: "Apply Patch" })).toBeNull();

  // This window was activated.
  deliver("app-activated", "main");
  expect(await screen.findByRole("dialog", { name: "Apply Patch" })).toBeTruthy();
});

// While a patch conflict is in the merge window the apply is modal: the Git menu does nothing.
test("the_git_menu_waits_while_a_patch_conflict_is_in_the_merge_window", async () => {
  const conflict = { path: "a.txt", base: "b\n", local: "l\n", patched: "p\n", localHash: "h", movedTo: null, newMode: null };
  vi.mocked(chooseNative).mockResolvedValue("/patches/fix.patch");
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    patch_app_settings_get: () => ({ folder: null, detectPatchOnTheFly: false }),
    patch_storage_set: () => undefined,
    patch_plan: finishing(64, APPLY_PLAN),
    patch_apply: finishing(65, { snapshot: "s", status: "skip", failed: [], conflicts: [conflict], messages: [] }),
    merge_window_open_contents: () => undefined,
    // The window still shows the contents it was opened with, however long the test takes.
    merge_window_contents: () => ({
      id: (calls("merge_window_open_contents")[0]?.[1] as { input: { id: string } } | undefined)?.input.id,
    }),
  });
  render(App);
  await screen.findByRole("treeitem", { name: /^Changes/ });
  deliver("git-action", "ChangesView.ApplyPatch");
  const dialog = await screen.findByRole("dialog", { name: "Apply Patch" });
  await fireEvent.click(within(dialog).getByRole("button", { name: "OK" }));
  await waitFor(() => expect(calls("merge_window_open_contents")).toHaveLength(1));

  deliver("git-action", "Git.CreateNewBranch");
  deliver("git-action", "ChangesView.ApplyPatchFromClipboard");
  await new Promise((resolve) => setTimeout(resolve, 30));

  expect(screen.queryByRole("dialog", { name: "Create New Branch" })).toBeNull();
  expect(calls("patch_clipboard")).toHaveLength(0);

  // `Skip` ends the apply, and the menu acts again.
  const { id } = (calls("merge_window_open_contents")[0][1] as { input: { id: string } }).input;
  deliver("merge-done", { id, result: "skip" });
  await waitFor(() => expect(applyInProgress()).toBe(false));
});

// `Git > Current File` of the main window: the Project tree's selection while it is the active
// tool window (`GitFileActionGroup`); its entries run on that selection.
test("current_file_follows_the_project_tree_and_runs_on_its_selection", async () => {
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    repo_status: () => [CHANGE],
    tree_list: () => [{ name: "a.txt", path: "a.txt", dir: false }],
    git_menu_current_file: () => undefined,
    branch_list: () => ({
      local: [{ name: "main", tracked: null }, { name: "other", tracked: null }],
      remote: [],
      recent: [],
      current: "main",
      previous: null,
    }),
    compare_local_state: () => {
      throw new Error("stop here");
    },
  });
  render(App);
  await screen.findByRole("treeitem", { name: /a\.txt/ });
  // Only the focused window says what Current File is.
  await fireEvent.focus(window);
  const lastMenu = () =>
    calls("git_menu_current_file").at(-1)?.[1] as { title: string; items: { id: string; enabled: boolean }[] };
  await waitFor(() => expect(lastMenu()?.title).toBe("Current File"));
  expect(lastMenu().items.every((item) => !item.enabled)).toBe(true);

  await fireEvent.keyDown(window, { key: "1", code: "Digit1", metaKey: true });
  const project = screen.getByRole("region", { name: "Project" });
  await fireEvent.click(await within(project).findByRole("treeitem", { name: "a.txt" }));
  await waitFor(() => expect(lastMenu().title).toBe("Selected File"));
  const enabled = lastMenu().items.filter((item) => item.enabled).map((item) => item.id);
  expect(enabled).toEqual([
    "CheckinFiles",
    "Git.Add",
    "Annotate",
    "Compare.SameVersion",
    "Compare.Selected",
    "Git.CompareWithBranch",
    "Vcs.ShowTabbedFileHistory",
  ]);

  // Without the focus (another of Ramify's windows has it) Current File is not this window's.
  const focus = vi.spyOn(document, "hasFocus").mockReturnValue(false);
  deliver("git-action", "Compare.SameVersion");
  await new Promise((resolve) => setTimeout(resolve, 20));
  focus.mockRestore();
  expect(calls("compare_local_state")).toEqual([]);

  // The text of a comparison that cannot be made is shown.
  deliver("git-action", "Compare.SameVersion");
  await waitFor(() => expect(calls("compare_local_state")).toEqual([["compare_local_state", { path: "a.txt" }]]));
  expect(await screen.findByText("stop here")).toBeTruthy();

  deliver("git-action", "CheckinFiles");
  const dialog = await screen.findByRole("dialog", { name: "Commit Changes" });
  expect(await within(dialog).findByRole("treeitem", { name: "a.txt" })).toBeTruthy();
});

test("show_git_log_and_the_viewers_file_actions_come_to_the_main_window", async () => {
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    repo_status: () => [CHANGE],
    log_open: () => ({ viewId: 1, count: 0 }),
  });
  render(App);
  await screen.findByRole("treeitem", { name: /a\.txt/ });

  deliver("git-action", "Vcs.Show.Log");
  await waitFor(() => expect(tabState("Log")).toBe("true"));

  deliver("git-file-action", { id: "CheckinFiles", paths: ["a.txt"] });
  const dialog = await screen.findByRole("dialog", { name: "Commit Changes" });
  expect(await within(dialog).findByRole("treeitem", { name: "a.txt" })).toBeTruthy();
});

// `GitAnnotationAspect.showAffectedPaths`: a click on an annotated line in the viewer selects its
// commit in the main window's Log.
test("an_annotated_line_selects_its_commit_in_the_main_log", async () => {
  const hash = "2222222222222222222222222222222222222222";
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    repo_status: () => [CHANGE],
    log_open: () => ({ viewId: 1, count: 3 }),
    log_rows: () => [],
    log_find_rows: () => [null],
  });
  render(App);
  await screen.findByRole("treeitem", { name: /a\.txt/ });

  deliver("annotation-show-in-log", hash);

  await waitFor(() => expect(tabState("Log")).toBe("true"));
  await waitFor(() => expect(calls("log_find_rows").at(-1)?.[1]).toEqual({ viewId: 1, hashes: [hash] }));
});

// `branch_list` is read again on each `repo-changed`: an older answer that arrives late is dropped.
test("compare_with_branch_follows_the_last_branch_list_read", async () => {
  const answers: ((list: unknown) => void)[] = [];
  const list = (...names: string[]) => ({
    local: names.map((name) => ({ name, tracked: null })),
    remote: [],
    recent: [],
    current: "main",
    previous: null,
  });
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    repo_status: () => [CHANGE],
    tree_list: () => [{ name: "a.txt", path: "a.txt", dir: false }],
    git_menu_current_file: () => undefined,
    branch_list: () => new Promise((resolve) => answers.push(resolve)),
  });
  render(App);
  await screen.findByRole("treeitem", { name: /a\.txt/ });
  await fireEvent.focus(window);
  await fireEvent.keyDown(window, { key: "1", code: "Digit1", metaKey: true });
  const project = screen.getByRole("region", { name: "Project" });
  await fireEvent.click(await within(project).findByRole("treeitem", { name: "a.txt" }));
  await waitFor(() => expect(answers).toHaveLength(1));
  deliver("repo-changed", { stash: false, shelf: false });
  await waitFor(() => expect(answers).toHaveLength(2));

  answers[1](list("main"));
  answers[0](list("main", "other"));
  await new Promise((resolve) => setTimeout(resolve, 20));

  const last = calls("git_menu_current_file").at(-1)?.[1] as {
    items: { id: string; enabled: boolean }[];
  };
  expect(last.items.find((item) => item.id === "Git.CompareWithBranch")?.enabled).toBe(false);
});

// §5.8 step 2: before a window closes (or the app quits) its commit message draft goes to the
// active changelist's comment, the store the Commit Changes dialog keeps its draft in, and only
// then does the window answer.
test("the commit draft is saved to the changelist comment when the window closes", async () => {
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    settings_get: () => NON_MODAL,
    repo_status: () => [CHANGE],
    changelists_set_comment: () => LISTS,
  });
  render(App);
  await screen.findByRole("treeitem", { name: /a\.txt/ });
  const message = screen.getByRole("textbox", { name: "Commit Message" });
  await fireEvent.input(message, { target: { value: "half done" } });

  deliver("commit-draft-save");

  await vi.waitFor(() => expect(calls("commit_draft_saved")).toHaveLength(1));
  expect(calls("changelists_set_comment")).toEqual([
    ["changelists_set_comment", { id: "default", comment: "half done", repo: "/repo/a" }],
  ]);
  const order = vi.mocked(invoke).mock.calls.map(([name]) => name);
  expect(order.lastIndexOf("changelists_set_comment")).toBeLessThan(
    order.lastIndexOf("commit_draft_saved"),
  );
});

test("the draft of an open Commit Changes dialog is kept as its Cancel keeps it", async () => {
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    repo_status: () => [CHANGE],
    changelists_set_comment: () => LISTS,
    ...DIALOG_COMMANDS,
  });
  render(App);
  await screen.findByRole("treeitem", { name: /a\.txt/ });
  await fireEvent.click(screen.getByRole("button", { name: "Commit…" }));
  const dialog = await screen.findByRole("dialog", { name: "Commit Changes" });
  await within(dialog).findByRole("treeitem", { name: "a.txt" });
  await fireEvent.input(within(dialog).getByRole("textbox", { name: "Commit Message" }), {
    target: { value: "from the dialog" },
  });

  deliver("commit-draft-save");

  await vi.waitFor(() => expect(calls("commit_draft_saved")).toHaveLength(1));
  expect(calls("changelists_set_comment")).toEqual([
    ["changelists_set_comment", { id: "default", comment: "from the dialog", repo: "/repo/a" }],
  ]);
});

test("a window with no draft answers at once, without writing", async () => {
  mockApp();
  render(App);
  await screen.findByText("0.0.1");

  deliver("commit-draft-save");

  await vi.waitFor(() => expect(calls("commit_draft_saved")).toHaveLength(1));
  expect(calls("changelists_set_comment")).toHaveLength(0);
});

// §5.8 Starting: `main` opens the open list's first project as its initial target, the backend
// opens the others in new windows, and the paths that no longer exist are reported once.
test("at start the open projects reopen, the missing ones are reported", async () => {
  mockApp({
    initial_target: () => folder("/work/a"),
    repo_open: () => INFO,
    open_projects_reopen: () => ({ missing: ["/gone/first", "/gone/second"] }),
  });
  render(App);

  await vi.waitFor(() => expect(calls("open_projects_reopen")).toHaveLength(1));
  await vi.waitFor(() => expect(calls("repo_open")).toHaveLength(1));
  expect(calls("repo_open")[0][1]).toEqual({ repo: "/work/a" });
  const order = vi.mocked(invoke).mock.calls.map(([name]) => name);
  expect(order.indexOf("initial_target")).toBeLessThan(order.indexOf("open_projects_reopen"));
  await vi.waitFor(() =>
    expect(get(notifications).map(({ kind, title }) => ({ kind, title }))).toEqual([
      {
        kind: "warning",
        title:
          "Some projects could not be reopened because their folders no longer exist: first, second",
      },
    ]),
  );
});

test("a start with nothing missing reports nothing", async () => {
  mockApp({ initial_target: () => folder("/work/a"), repo_open: () => INFO });
  render(App);

  await vi.waitFor(() => expect(calls("show_window")).toHaveLength(1));
  expect(calls("open_projects_reopen")).toHaveLength(1);
  expect(get(notifications)).toEqual([]);
});

// Fix round 1: the draft a closed window saved comes back in the Commit tool window when the
// repository is opened again; a message emptied since (committed, say) clears the comment.
test("a saved draft shows in the Commit Message box after reopening", async () => {
  let lists = LISTS;
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    settings_get: () => NON_MODAL,
    repo_status: () => [CHANGE],
    changelists_get: () => lists,
    changelists_set_comment: (args) => {
      lists = {
        ...lists,
        lists: lists.lists.map((list) =>
          list.id === args?.id ? { ...list, comment: args?.comment as string } : list,
        ),
      };
      return lists;
    },
  });
  render(App);
  await screen.findByRole("treeitem", { name: /a\.txt/ });
  await fireEvent.input(screen.getByRole("textbox", { name: "Commit Message" }), {
    target: { value: "half done" },
  });
  deliver("commit-draft-save");
  await vi.waitFor(() => expect(calls("commit_draft_saved")).toHaveLength(1));
  cleanup();
  // The new window's listeners only.
  vi.mocked(listen).mockClear();

  render(App);
  await screen.findByRole("treeitem", { name: /a\.txt/ });
  const message = screen.getByRole("textbox", { name: "Commit Message" }) as HTMLTextAreaElement;
  await vi.waitFor(() => expect(message.value).toBe("half done"));

  await fireEvent.input(message, { target: { value: "" } });
  deliver("commit-draft-save");
  await vi.waitFor(() => expect(calls("commit_draft_saved")).toHaveLength(2));
  expect(lists.lists[0].comment).toBe("");
});


// `GitToolbarWidgetAction.updateCustomComponent`: the branch widget's right icons are Incoming and
// Outgoing for the current branch, read through `branch_incoming_outgoing`.
const widget = () => screen.getByRole("button", { name: /^Git Branch:/ });
const syncIcons = () =>
  [...widget().querySelectorAll<HTMLElement>("[data-sync]")].map((icon) => icon.dataset.sync);

async function openWithSync(sync: () => unknown, state = INFO.state) {
  mockApp({
    repo_open: () => ({ ...INFO, state }),
    repo_state: () => state,
    initial_target: () => folder("/repo/a"),
    branch_incoming_outgoing: sync,
  });
  render(App);
  await screen.findByRole("treeitem", { name: /^Changes/ });
}

test("the_branch_widget_shows_incoming_when_the_current_branch_has_commits_to_pull", async () => {
  await openWithSync(() => ({ main: { incoming: true, outgoing: false }, other: { incoming: false, outgoing: true } }));
  await waitFor(() => expect(syncIcons()).toEqual(["incoming"]));
});

test("it_shows_outgoing_too_and_both_together", async () => {
  await openWithSync(() => ({ main: { incoming: true, outgoing: true } }));
  await waitFor(() => expect(syncIcons()).toEqual(["incoming", "outgoing"]));
});

test("it_shows_neither_when_synced_or_detached", async () => {
  await openWithSync(() => ({ main: { incoming: false, outgoing: false } }));
  await waitFor(() => expect(calls("branch_incoming_outgoing").length).toBeGreaterThan(0));
  expect(syncIcons()).toEqual([]);
  cleanup();

  const detached = { operation: null, branch: null, label: "abcdef12", head: "abcdef1234567890" };
  await openWithSync(() => ({ main: { incoming: true, outgoing: true } }), detached as never);
  await new Promise((resolve) => setTimeout(resolve, 20));
  expect(syncIcons()).toEqual([]);
});

test("branch_sync_changed_and_repo_changed_refresh_the_icons", async () => {
  let sync = { main: { incoming: false, outgoing: false } };
  await openWithSync(() => sync);
  await waitFor(() => expect(calls("branch_incoming_outgoing").length).toBeGreaterThan(0));
  expect(syncIcons()).toEqual([]);

  sync = { main: { incoming: true, outgoing: false } };
  deliver("branch-sync-changed");
  await waitFor(() => expect(syncIcons()).toEqual(["incoming"]));

  sync = { main: { incoming: false, outgoing: true } };
  deliver("repo-changed");
  await waitFor(() => expect(syncIcons()).toEqual(["outgoing"]));
});

test("a_failed_read_shows_no_icons", async () => {
  await openWithSync(() => {
    throw new Error("git failed");
  });
  await waitFor(() => expect(calls("branch_incoming_outgoing").length).toBeGreaterThan(0));
  await new Promise((resolve) => setTimeout(resolve, 20));
  expect(syncIcons()).toEqual([]);
  expect(get(notifications)).toEqual([]);
});

test("the_widget_label_and_tooltip_do_not_change", async () => {
  await openWithSync(() => ({ main: { incoming: true, outgoing: true } }));
  await waitFor(() => expect(syncIcons()).toEqual(["incoming", "outgoing"]));
  expect(widget().getAttribute("aria-label")).toBe("Git Branch: main");
  expect(widget().textContent?.trim()).toBe("main");
});

// `refsTracker`: `repo-changed` fires for any file under the work tree; the icons are read again
// only when the refs changed (`branch_refs_fingerprint`), on open and on `branch-sync-changed`.
test("repo_changed_reads_the_icons_again_only_when_the_refs_changed", async () => {
  let refs = "r1";
  mockApp({
    repo_open: () => INFO,
    initial_target: () => folder("/repo/a"),
    branch_incoming_outgoing: () => ({ main: { incoming: true, outgoing: false } }),
    branch_refs_fingerprint: () => refs,
  });
  render(App);
  await screen.findByRole("treeitem", { name: /^Changes/ });
  await waitFor(() => expect(syncIcons()).toEqual(["incoming"]));
  await new Promise((resolve) => setTimeout(resolve, 20));
  const before = calls("branch_incoming_outgoing").length;
  const fingerprints = calls("branch_refs_fingerprint").length;

  deliver("repo-changed");
  await waitFor(() => expect(calls("branch_refs_fingerprint").length).toBeGreaterThan(fingerprints));
  await new Promise((resolve) => setTimeout(resolve, 20));
  expect(calls("branch_incoming_outgoing")).toHaveLength(before);

  refs = "r2";
  deliver("repo-changed");
  await waitFor(() => expect(calls("branch_incoming_outgoing").length).toBe(before + 1));
});

test("after_a_checkout_the_old_branchs_icons_do_not_show_next_to_the_new_name", async () => {
  let state = INFO.state;
  let answer: (value: unknown) => void = () => {};
  let first = true;
  mockApp({
    repo_open: () => INFO,
    repo_state: () => state,
    initial_target: () => folder("/repo/a"),
    branch_incoming_outgoing: () => {
      if (first) {
        first = false;
        return { main: { incoming: true, outgoing: true } };
      }
      return new Promise((resolve) => (answer = resolve));
    },
  });
  render(App);
  await screen.findByRole("treeitem", { name: /^Changes/ });
  await waitFor(() => expect(syncIcons()).toEqual(["incoming", "outgoing"]));

  state = { ...INFO.state, branch: "dev", label: "dev" };
  deliver("repo-changed");
  await waitFor(() => expect(widget().textContent?.trim()).toBe("dev"));
  expect(syncIcons()).toEqual([]);

  await waitFor(() => expect(calls("branch_incoming_outgoing")).toHaveLength(2));
  answer({ dev: { incoming: false, outgoing: true } });
  await waitFor(() => expect(syncIcons()).toEqual(["outgoing"]));
});

// Ramify's own notice of Git changes made outside it: the event from the watcher becomes a notice
// that stays, and `Show in Log` selects the commit HEAD moved to.
test("changes_made_outside_ramify_show_a_notice_that_opens_the_log_at_the_new_head", async () => {
  const hash = "3333333333333333333333333333333333333333";
  mockApp({
    initial_target: () => folder("/repo/a"),
    repo_open: () => INFO,
    repo_status: () => [CHANGE],
    log_open: () => ({ viewId: 1, count: 3 }),
    log_rows: () => [],
    log_find_rows: () => [null],
  });
  render(App);
  await screen.findByRole("treeitem", { name: /a\.txt/ });

  deliver("external-changes", [{ kind: "headMoved", from: "11111111", to: hash }]);

  expect(await screen.findByText("Changes made outside Ramify")).toBeTruthy();
  expect(screen.getByText("HEAD moved from 11111111 to 33333333 (reset, rebase or amend)")).toBeTruthy();
  await fireEvent.click(screen.getByRole("button", { name: "Show in Log" }));
  await waitFor(() => expect(tabState("Log")).toBe("true"));
  await waitFor(() => expect(calls("log_find_rows").at(-1)?.[1]).toEqual({ viewId: 1, hashes: [hash] }));
});
