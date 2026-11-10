import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { CommitWarning, FileChange, FileState, RepoInfo, Settings } from "../lib/api";
import { trackTasks } from "../lib/tasks";
import CommitView from "./CommitView.svelte";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const REPO: RepoInfo = {
  path: "/repo",
  head: "abc",
  branch: "main",
  recovery: null,
  pending: [],
  state: { operation: null, branch: "main", label: "main", head: "abcdef1234567890" },
};

const change = (path: string, state: FileState, staged: boolean, unstaged: boolean) =>
  ({ path, oldPath: null, state, staged, unstaged }) satisfies FileChange;

const SETTINGS: Settings = {
  gitPath: null,
  stagingArea: true,
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
  nonModalCommit: true,
  clearInitialCommitMessage: false,
  showDropCommitDialog: true,
  githubConnectionTimeoutMs: 5000,
};

const LISTS = {
  lists: [{ id: "default", name: "Changes", comment: "" }],
  active: "default",
  assignments: {},
  hunkAssignments: {},
};

vi.stubGlobal(
  "ResizeObserver",
  class {
    observe() {}
    unobserve() {}
    disconnect() {}
  },
);

function mockCommands(commands: Record<string, (args: never) => unknown> = {}) {
  const all: Record<string, (args: never) => unknown> = {
    repo_status: () => [
      change("a.txt", "modified", true, true),
      change("b.txt", "modified", false, true),
      change("c.txt", "untracked", false, true),
    ],
    changelists_get: () => LISTS,
    commit_warnings_staged: () => [],
    commit_staged: () => "hash",
    staging_stage: () => null,
    staging_unstage: () => null,
    settings_get: () => SETTINGS,
    settings_save: (args) => (args as unknown as { settings: Settings }).settings,
    commit_fix_autocrlf: () => undefined,
    ...commands,
  };
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    const handler = all[command];
    if (!handler) throw new Error(`unexpected command: ${command}`);
    return handler(args as never);
  });
}

function calls(command: string) {
  return vi.mocked(invoke).mock.calls.filter(([name]) => name === command);
}

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(listen).mockReset();
  vi.mocked(listen).mockImplementation(async () => () => {});
  localStorage.clear();
});

afterEach(cleanup);

async function open(props: Record<string, unknown> = {}) {
  const view = render(CommitView, { repo: REPO, active: true, stagingArea: true, ...props });
  await screen.findAllByRole("treeitem", { name: /\.txt$/ });
  return view;
}

// `a.txt` shows up twice: first in Staged and then in Unstaged.
const rows = (name: string) => screen.getAllByRole("treeitem", { name });
const hover = (row: HTMLElement, name: string) => within(row).getByRole("button", { name });
const messageField = () => screen.getByRole("textbox", { name: "Commit Message" });
const commitButton = () => screen.getByRole("button", { name: "Commit" });

async function commitWithMessage() {
  await fireEvent.input(messageField(), { target: { value: "message" } });
  await fireEvent.click(commitButton());
}

test("shows Staged and Unstaged without changelists", async () => {
  mockCommands();
  await open();

  expect(screen.getByRole("treeitem", { name: /^Staged 1 file/ })).toBeTruthy();
  expect(screen.getByRole("treeitem", { name: /^Unstaged 2 files/ })).toBeTruthy();
  expect(rows("a.txt")).toHaveLength(2);
  expect(calls("changelists_get")).toHaveLength(0);
  expect(screen.queryByRole("button", { name: "Changelists" })).toBeNull();
});

test("there are no checkboxes but icons when hovering the row", async () => {
  mockCommands();
  await open();

  const [inStaged, inUnstaged] = rows("a.txt");

  expect(within(screen.getByRole("tree")).queryByRole("checkbox")).toBeNull();
  expect(hover(inStaged, "Unstage")).toBeTruthy();
  expect(hover(inUnstaged, "Stage")).toBeTruthy();
  expect(hover(rows("c.txt")[0], "Stage")).toBeTruthy();
});

test("the + icon moves the file to the stage", async () => {
  mockCommands();
  await open();

  await fireEvent.click(hover(rows("b.txt")[0], "Stage"));

  await waitFor(() => expect(calls("staging_stage")).toEqual([["staging_stage", { paths: ["b.txt"] }]]));
  await waitFor(() => expect(calls("repo_status").length).toBeGreaterThan(1));
});

test("the - icon takes the file out of the stage", async () => {
  mockCommands();
  await open();

  await fireEvent.click(hover(rows("a.txt")[0], "Unstage"));

  await waitFor(() =>
    expect(calls("staging_unstage")).toEqual([["staging_unstage", { paths: ["a.txt"] }]]),
  );
});

test("the icon of a group acts on the whole group", async () => {
  mockCommands();
  await open();

  await fireEvent.click(hover(screen.getByRole("treeitem", { name: /^Unstaged/ }), "Stage"));

  await waitFor(() =>
    expect(calls("staging_stage")).toEqual([["staging_stage", { paths: ["a.txt", "b.txt"] }]]),
  );
});

test("the icon of a folder acts on what it contains", async () => {
  mockCommands({
    repo_status: () => [
      change("dir/a.txt", "modified", false, true),
      change("dir/b.txt", "modified", false, true),
      change("c.txt", "modified", false, true),
    ],
  });
  await open();

  await fireEvent.click(hover(screen.getByRole("treeitem", { name: /^dir/ }), "Stage"));

  await waitFor(() =>
    expect(calls("staging_stage")).toEqual([
      ["staging_stage", { paths: ["dir/a.txt", "dir/b.txt"] }],
    ]),
  );
});

test("clicking the icon does not open the diff", async () => {
  mockCommands();
  const onshowdiff = vi.fn();
  await open({ onshowdiff });

  await fireEvent.dblClick(hover(rows("b.txt")[0], "Stage"));

  expect(onshowdiff).not.toHaveBeenCalled();
});

test("Stage All moves what is modified and what is new to the stage", async () => {
  mockCommands();
  await open();

  await fireEvent.click(screen.getByRole("button", { name: "Stage All" }));

  await waitFor(() =>
    expect(calls("staging_stage")).toEqual([
      ["staging_stage", { paths: ["a.txt", "b.txt", "c.txt"] }],
    ]),
  );
});

test("conflicts show up without an icon", async () => {
  mockCommands({ repo_status: () => [change("c.txt", "unmerged", false, true)] });
  await open();

  expect(within(rows("c.txt")[0]).queryByRole("button")).toBeNull();
  expect(screen.getByRole("treeitem", { name: /^Unmerged/ })).toBeTruthy();
});

test("if a snapshot of the stage is saved it can be restored", async () => {
  mockCommands({ staging_stage: () => "snapshot", snapshots_restore: () => "previous" });
  await open();

  await fireEvent.click(hover(rows("b.txt")[0], "Stage"));
  await fireEvent.click(await screen.findByRole("button", { name: "Restore" }));

  await waitFor(() =>
    expect(calls("snapshots_restore")).toEqual([["snapshots_restore", { id: "snapshot", expectedRepo: "/repo" }]]),
  );
});

test("an error when touching the stage is reported", async () => {
  mockCommands({
    staging_stage: () => {
      throw { kind: "failed", message: "could not" };
    },
  });
  await open();

  await fireEvent.click(hover(rows("b.txt")[0], "Stage"));

  expect((await screen.findByRole("alert")).textContent).toContain("could not");
});

test("the commit uses the index as it is", async () => {
  mockCommands();
  await open();

  await commitWithMessage();

  await waitFor(() => expect(calls("commit_staged")).toHaveLength(1));
  const [, args] = calls("commit_staged")[0] as [string, { message: string }];
  expect(args.message).toBe("message");
  expect(calls("commit_changes")).toHaveLength(0);
  await waitFor(() => expect((messageField() as HTMLTextAreaElement).value).toBe(""));
  expect((await screen.findByRole("status")).textContent).toContain("1 file committed: message");
});

test("with nothing in the stage it asks to stage files", async () => {
  mockCommands({ repo_status: () => [change("b.txt", "modified", false, true)] });
  await open();

  await fireEvent.click(commitButton());
  expect(screen.getByText("Stage files to commit and specify commit message")).toBeTruthy();

  await commitWithMessage();
  expect(screen.getByText("Stage files to commit")).toBeTruthy();
  expect(calls("commit_staged")).toHaveLength(0);
});

test("with conflicts it asks to resolve them", async () => {
  mockCommands({
    repo_status: () => [
      change("a.txt", "modified", true, false),
      change("c.txt", "unmerged", false, true),
    ],
  });
  await open();

  await commitWithMessage();

  expect(screen.getByText("Resolve conflicts in order to commit")).toBeTruthy();
  expect(calls("commit_staged")).toHaveLength(0);
});

test("the legend counts what is in the stage", async () => {
  mockCommands();
  await open();

  expect(screen.getByText("1 modified")).toBeTruthy();
});

function warned(warning: CommitWarning) {
  return { commit_warnings_staged: () => [warning] };
}

test("warns about a detached HEAD and lets the user continue", async () => {
  mockCommands(warned({ kind: "detachedHead" }));
  await open();

  await commitWithMessage();
  const dialog = await screen.findByRole("dialog", { name: "Commit in Detached HEAD" });
  expect(dialog.textContent).toContain("/repo");
  expect(dialog.textContent).toMatch(/be sure to checkout a\s+branch/);
  expect(calls("commit_staged")).toHaveLength(0);

  await fireEvent.click(within(dialog).getByRole("button", { name: "Commit" }));

  await waitFor(() => expect(calls("commit_staged")).toHaveLength(1));
  expect(calls("settings_save")).toHaveLength(0);
});

test("cancelling the warning does not commit and keeps the message", async () => {
  mockCommands(warned({ kind: "rebasing" }));
  await open();

  await commitWithMessage();
  const dialog = await screen.findByRole("dialog", { name: "Unfinished Rebase Process" });
  await fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));

  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(calls("commit_staged")).toHaveLength(0);
  expect((messageField() as HTMLTextAreaElement).value).toBe("message");
});

test("Don't warn again turns the warning off when continuing", async () => {
  mockCommands(warned({ kind: "detachedHead" }));
  await open();

  await commitWithMessage();
  const dialog = await screen.findByRole("dialog");
  await fireEvent.click(within(dialog).getByRole("checkbox", { name: "Don't warn again" }));
  await fireEvent.click(within(dialog).getByRole("button", { name: "Commit" }));

  await waitFor(() => expect(calls("commit_staged")).toHaveLength(1));
  expect(calls("settings_save")[0][1]).toEqual({
    settings: { ...SETTINGS, warnDetached: false },
  });
});

test("Don't warn again is not saved on cancel", async () => {
  mockCommands(warned({ kind: "detachedHead" }));
  await open();

  await commitWithMessage();
  const dialog = await screen.findByRole("dialog");
  await fireEvent.click(within(dialog).getByRole("checkbox", { name: "Don't warn again" }));
  await fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));

  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(calls("settings_save")).toHaveLength(0);
});

test("CRLF warning with its three outcomes", async () => {
  mockCommands(warned({ kind: "crlf", paths: ["a.txt"] }));
  await open();

  await commitWithMessage();
  const dialog = await screen.findByRole("dialog", { name: "Line Separators Warning" });
  expect(dialog.textContent).toContain("git config --global core.autocrlf input");
  expect(dialog.textContent).toContain("a.txt");
  await fireEvent.click(within(dialog).getByRole("button", { name: "Fix and Commit" }));

  await waitFor(() => expect(calls("commit_staged")).toHaveLength(1));
  expect(calls("commit_fix_autocrlf")).toHaveLength(1);
});

test("Commit As Is leaves the setting alone and can turn the warning off", async () => {
  mockCommands(warned({ kind: "crlf", paths: ["a.txt"] }));
  await open();

  await commitWithMessage();
  const dialog = await screen.findByRole("dialog");
  await fireEvent.click(within(dialog).getByRole("checkbox", { name: "Don't warn again" }));
  await fireEvent.click(within(dialog).getByRole("button", { name: "Commit As Is" }));

  await waitFor(() => expect(calls("commit_staged")).toHaveLength(1));
  expect(calls("commit_fix_autocrlf")).toHaveLength(0);
  expect(calls("settings_save")[0][1]).toEqual({ settings: { ...SETTINGS, warnCrlf: false } });
});

test("several warnings are asked one by one", async () => {
  mockCommands({
    commit_warnings_staged: () => [{ kind: "crlf", paths: ["a.txt"] }, { kind: "detachedHead" }],
  });
  await open();

  await commitWithMessage();
  const first = await screen.findByRole("dialog", { name: "Line Separators Warning" });
  await fireEvent.click(within(first).getByRole("button", { name: "Commit As Is" }));
  const second = await screen.findByRole("dialog", { name: "Commit in Detached HEAD" });
  expect(calls("commit_staged")).toHaveLength(0);
  await fireEvent.click(within(second).getByRole("button", { name: "Commit" }));

  await waitFor(() => expect(calls("commit_staged")).toHaveLength(1));
});

test("opening the diff chooses the comparison by the group", async () => {
  mockCommands();
  const onshowdiff = vi.fn();
  await open({ onshowdiff });
  const [inStaged, inUnstaged] = rows("a.txt");

  await fireEvent.dblClick(inStaged);
  await fireEvent.dblClick(inUnstaged);

  expect(onshowdiff.mock.calls.map(([file, comparison]) => [file.path, comparison])).toEqual([
    ["a.txt", "staged"],
    ["a.txt", "local"],
  ]);
});

const rollbackKey = { key: "z", code: "KeyZ", metaKey: true, altKey: true };
const stageKey = { key: "å", code: "KeyA", metaKey: true, altKey: true };

test("⌥⌘Z in Unstaged goes back to what is in the stage without a dialog and can be restored", async () => {
  mockCommands({ staging_revert: () => "snapshot", snapshots_restore: () => "previous" });
  await open();

  await fireEvent.click(rows("a.txt")[1]);
  await fireEvent.keyDown(window, rollbackKey);

  await waitFor(() =>
    expect(calls("staging_revert")).toEqual([["staging_revert", { paths: ["a.txt"] }]]),
  );
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(calls("rollback_files")).toHaveLength(0);
  await fireEvent.click(await screen.findByRole("button", { name: "Restore" }));
  await waitFor(() =>
    expect(calls("snapshots_restore")).toEqual([["snapshots_restore", { id: "snapshot", expectedRepo: "/repo" }]]),
  );
});

test("⌥⌘Z in Staged takes it out of the stage", async () => {
  mockCommands();
  await open();

  await fireEvent.click(rows("a.txt")[0]);
  await fireEvent.keyDown(window, rollbackKey);

  await waitFor(() =>
    expect(calls("staging_unstage")).toEqual([["staging_unstage", { paths: ["a.txt"] }]]),
  );
  expect(calls("staging_revert")).toHaveLength(0);
});

test("⌥⌘Z in Untracked does nothing", async () => {
  mockCommands();
  await open();

  await fireEvent.click(rows("c.txt")[0]);
  await fireEvent.keyDown(window, rollbackKey);

  await waitFor(() => expect(calls("repo_status")).toHaveLength(1));
  expect(calls("staging_revert")).toHaveLength(0);
  expect(calls("staging_unstage")).toHaveLength(0);
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("⌥⌘A moves what is in Unstaged and Untracked to the stage", async () => {
  mockCommands();
  await open();

  await fireEvent.click(rows("b.txt")[0]);
  await fireEvent.click(rows("c.txt")[0], { metaKey: true });
  await fireEvent.keyDown(window, stageKey);

  await waitFor(() =>
    expect(calls("staging_stage")).toEqual([["staging_stage", { paths: ["b.txt", "c.txt"] }]]),
  );
});

test("⌥⌘A in Staged does nothing", async () => {
  mockCommands();
  await open();

  await fireEvent.click(rows("a.txt")[0]);
  await fireEvent.keyDown(window, stageKey);

  await new Promise((resolve) => setTimeout(resolve, 20));
  expect(calls("staging_stage")).toHaveLength(0);
});

test("the menu shows the macOS shortcuts", async () => {
  mockCommands();
  await open();

  await fireEvent.contextMenu(rows("b.txt")[0]);

  const stage = await screen.findByRole("menuitem", { name: "Stage" });
  expect(stage.textContent).toContain("⌥⌘A");
  expect(screen.getByRole("menuitem", { name: "Rollback" }).textContent).toContain("⌥⌘Z");
  expect(screen.getByRole("menuitem", { name: "Unstage" }).textContent).toContain("⌥⌘Z");
});

test("the Rollback button does the same as the shortcut", async () => {
  mockCommands({ staging_revert: () => "snapshot" });
  await open();
  expect((screen.getByRole("button", { name: "Rollback" }) as HTMLButtonElement).disabled).toBe(true);

  await fireEvent.click(rows("b.txt")[0]);
  await fireEvent.click(screen.getByRole("button", { name: "Rollback" }));

  await waitFor(() =>
    expect(calls("staging_revert")).toEqual([["staging_revert", { paths: ["b.txt"] }]]),
  );
});

test("the menu has Rollback only for what is in Unstaged", async () => {
  mockCommands({ staging_revert: () => "snapshot" });
  await open();

  await fireEvent.contextMenu(rows("a.txt")[0]);
  const item = await screen.findByRole("menuitem", { name: /^Rollback/ });
  expect((item as HTMLButtonElement).disabled).toBe(true);
});

test("a failure when reverting is reported", async () => {
  mockCommands({
    staging_revert: () => {
      throw { kind: "failed", message: "could not revert" };
    },
  });
  await open();

  await fireEvent.click(rows("b.txt")[0]);
  await fireEvent.keyDown(window, rollbackKey);

  expect((await screen.findByRole("alert")).textContent).toContain("could not revert");
});

test("⌘⇧M does not open the changelists dialog even if lists have been read", async () => {
  mockCommands();
  const view = render(CommitView, { repo: REPO, active: true, stagingArea: false });
  await screen.findByRole("treeitem", { name: /^Changes/ });
  await view.rerender({ stagingArea: true });
  await screen.findByRole("treeitem", { name: /^Unstaged/ });

  await fireEvent.click(rows("b.txt")[0]);
  await fireEvent.keyDown(window, { key: "M", code: "KeyM", metaKey: true, shiftKey: true });

  await new Promise((resolve) => setTimeout(resolve, 20));
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("the menu of a row has Stage and Unstage", async () => {
  mockCommands();
  await open();

  await fireEvent.contextMenu(rows("b.txt")[0]);
  await fireEvent.click(await screen.findByRole("menuitem", { name: "Stage" }));

  await waitFor(() => expect(calls("staging_stage")).toHaveLength(1));
});

const THREE = "Compare HEAD, Staged and Local Versions";

test("the_menu_compares_head_staged_and_local", async () => {
  mockCommands();
  const onshowdiff = vi.fn();
  await open({ onshowdiff });

  await fireEvent.contextMenu(rows("a.txt")[1]);
  await fireEvent.click(await screen.findByRole("menuitem", { name: THREE }));

  expect(onshowdiff.mock.calls.map(([file, comparison]) => [file.path, comparison])).toEqual([
    ["a.txt", "three"],
  ]);
});

test("only_with_the_three_versions", async () => {
  mockCommands({
    repo_status: () => [
      change("a.txt", "modified", true, true),
      change("c.txt", "untracked", false, true),
      change("d.txt", "added", true, false),
      change("e.txt", "deleted", false, true),
    ],
  });
  await open();

  for (const path of ["c.txt", "d.txt", "e.txt"]) {
    await fireEvent.contextMenu(rows(path)[0]);
    const item = await screen.findByRole("menuitem", { name: THREE });
    expect((item as HTMLButtonElement).disabled, path).toBe(true);
    await fireEvent.keyDown(item, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("menuitem", { name: THREE })).toBeNull());
  }
});

test("turning the mode off brings the changelists back", async () => {
  mockCommands();
  const view = await open();

  await view.rerender({ stagingArea: false });

  await screen.findByRole("treeitem", { name: /^Changes/ });
  expect(calls("changelists_get").length).toBeGreaterThan(0);
  expect(screen.queryByRole("treeitem", { name: /^Staged/ })).toBeNull();
});

test("commit_and_push_with_staging_uses_the_index_and_then_pushes", async () => {
  const onpush = vi.fn(async () => {});
  mockCommands();
  await open({ onpreparepush: () => onpush });

  await fireEvent.input(screen.getByRole("textbox", { name: "Commit Message" }), {
    target: { value: "message" },
  });
  await fireEvent.click(screen.getByRole("button", { name: "Commit and Push…" }));

  await waitFor(() => expect(onpush).toHaveBeenCalledTimes(1));
  expect(calls("commit_staged")).toHaveLength(1);
});

test("with_staging_a_failing_commit_does_not_push", async () => {
  const onpush = vi.fn(async () => {});
  mockCommands({
    commit_staged: () => {
      throw { kind: "failed", message: "no" };
    },
  });
  await open({ onpreparepush: () => onpush });

  await fireEvent.input(screen.getByRole("textbox", { name: "Commit Message" }), {
    target: { value: "message" },
  });
  await fireEvent.click(screen.getByRole("button", { name: "Commit and Push…" }));

  expect(await screen.findByRole("alert")).toBeTruthy();
  expect(onpush).not.toHaveBeenCalled();
});

// A task whose result arrives through `task-changed`.
let deliverTask: (id: number) => void = () => {};
let nextTask = 900;

function task() {
  const id = nextTask++;
  setTimeout(() => deliverTask(id));
  return id;
}

async function listenToTasks() {
  vi.mocked(listen).mockImplementation(async (event, handler) => {
    if (event === "task-changed") {
      deliverTask = (id) =>
        handler({
          event: "task-changed",
          id: 0,
          payload: { id, title: "", text: "", fraction: null, state: "done", kind: null, value: null },
        } as never);
    }
    return () => {};
  });
  await trackTasks();
}

const UNMERGED = {
  repo_status: () => [
    change("a.txt", "modified", true, false),
    change("c.txt", "unmerged", false, true),
  ],
};

test("staging_unmerged_offers_the_same_actions", async () => {
  await listenToTasks();
  mockCommands({ ...UNMERGED, conflicts_accept: task, merge_window_open: () => undefined });
  const onresolveconflicts = vi.fn();
  render(CommitView, { repo: REPO, active: true, stagingArea: true, onresolveconflicts });
  await screen.findAllByRole("treeitem", { name: "c.txt" });

  await fireEvent.contextMenu(rows("c.txt")[0]);
  const menu = await screen.findByRole("menu");
  const items = within(menu).getAllByRole("menuitem");
  expect(items.slice(0, 3).map((item) => item.textContent?.trim())).toEqual([
    "Merge…",
    "Accept Theirs",
    "Accept Yours",
  ]);

  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Accept Yours" }));
  await waitFor(() => expect(calls("conflicts_accept")).toHaveLength(1));
  expect(calls("conflicts_accept")[0][1]).toEqual({ paths: ["c.txt"], side: "yours" });

  await fireEvent.contextMenu(rows("c.txt")[0]);
  await fireEvent.click(
    within(await screen.findByRole("menu")).getByRole("menuitem", { name: "Merge…" }),
  );
  await waitFor(() => expect(calls("merge_window_open")).toHaveLength(1));
  expect(calls("merge_window_open")[0][1]).toEqual({ path: "c.txt" });

  const unmerged = screen.getByRole("treeitem", { name: /^Unmerged/ });
  await fireEvent.click(within(unmerged).getByRole("button", { name: "Resolve" }));
  expect(onresolveconflicts).toHaveBeenCalledTimes(1);
});

test("staging_a_normal_file_does_not_offer_the_conflict_actions", async () => {
  mockCommands(UNMERGED);
  await open();

  await fireEvent.contextMenu(rows("a.txt")[0]);
  const menu = await screen.findByRole("menu");

  expect(within(menu).queryByRole("menuitem", { name: "Merge…" })).toBeNull();
  expect(within(menu).getAllByRole("menuitem")[0].textContent).toContain("Stage");
});

// `Git.Stage.Stash.Files`, after Unstage and before Rollback, with the chosen paths.
test("stash_files_of_the_tree_stashes_the_chosen_paths", async () => {
  await listenToTasks();
  mockCommands({ stash_push_paths: task });
  await open();

  await fireEvent.contextMenu(rows("c.txt")[0]);
  const menu = await screen.findByRole("menu");
  const labels = within(menu)
    .getAllByRole("menuitem")
    .map((item) => item.textContent?.replace(/⌥⌘./, "").trim());
  expect(labels.slice(labels.indexOf("Unstage"), labels.indexOf("Rollback") + 1)).toEqual([
    "Unstage",
    "Stash Files",
    "Rollback",
  ]);
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Stash Files" }));

  await waitFor(() => expect(calls("stash_push_paths")).toHaveLength(1));
  expect(calls("stash_push_paths")[0][1]).toEqual({ paths: ["c.txt"] });
});

// `Git.Stash.Silently` of the toolbar: `stash save` without a message.
test("stash_silently_saves_without_a_message", async () => {
  await listenToTasks();
  mockCommands({ stash_save: task });
  await open();

  await fireEvent.click(screen.getByRole("button", { name: "Stash Silently" }));

  await waitFor(() => expect(calls("stash_save")).toHaveLength(1));
  expect(calls("stash_save")[0][1]).toEqual({ message: "", keepIndex: false });
});

// `GitStageCreatePatchActionProvider`, after Compare HEAD, Staged and Local Versions: HEAD→index for
// a Staged node, index→disk for an Unstaged or Untracked one, HEAD→disk for both. `Create Patch from
// Local Changes...` does not exist with the staging area.
test("staging_copy_as_patch_follows_the_head_index_disk_rule", async () => {
  await listenToTasks();
  mockCommands({ patch_create: task });
  await open();

  await fireEvent.contextMenu(rows("a.txt")[0]);
  let menu = await screen.findByRole("menu");
  const labels = within(menu)
    .getAllByRole("menuitem")
    .map((item) => item.textContent?.replace(/[⌥⌘]+./, "").trim());
  expect(labels.slice(labels.indexOf(THREE), labels.indexOf(THREE) + 3)).toEqual([
    THREE,
    "Create Patch...",
    "Copy as Patch to Clipboard",
  ]);
  expect(labels).not.toContain("Create Patch from Local Changes...");
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Copy as Patch to Clipboard" }));
  await waitFor(() => expect(calls("patch_create")).toHaveLength(1));
  expect((calls("patch_create")[0][1] as { entries: unknown[] }).entries).toEqual([
    { path: "a.txt", oldPath: null, before: { kind: "head" }, after: { kind: "index" } },
  ]);

  // The Unstaged node of `a.txt`, then both of its nodes, then a new file.
  await fireEvent.click(rows("a.txt")[1]);
  await fireEvent.contextMenu(rows("a.txt")[1]);
  menu = await screen.findByRole("menu");
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Copy as Patch to Clipboard" }));
  await waitFor(() => expect(calls("patch_create")).toHaveLength(2));
  expect((calls("patch_create")[1][1] as { entries: unknown[] }).entries).toEqual([
    { path: "a.txt", oldPath: null, before: { kind: "index" }, after: { kind: "disk" } },
  ]);

  await fireEvent.click(rows("a.txt")[0], { metaKey: true });
  await fireEvent.click(rows("c.txt")[0], { metaKey: true });
  await fireEvent.contextMenu(rows("c.txt")[0]);
  menu = await screen.findByRole("menu");
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Copy as Patch to Clipboard" }));
  await waitFor(() => expect(calls("patch_create")).toHaveLength(3));
  expect(calls("patch_create")[2][1]).toEqual({
    entries: [
      { path: "a.txt", oldPath: null, before: { kind: "head" }, after: { kind: "disk" } },
      { path: "c.txt", oldPath: null, before: { kind: "index" }, after: { kind: "disk" } },
    ],
    options: { basePath: "/repo", reverse: false, message: null },
    to: { kind: "clipboard" },
    remember: false,
    expectedRepo: "/repo",
  });
});

// isVisible = isEnabled: a conflict alone gives neither item.
test("a_conflict_alone_offers_no_patch_items", async () => {
  mockCommands({
    repo_status: () => [change("u.txt", "unmerged", false, true)],
  });
  await open();

  await fireEvent.contextMenu(screen.getAllByRole("treeitem", { name: "u.txt" })[0]);
  const menu = await screen.findByRole("menu");

  expect(within(menu).queryByRole("menuitem", { name: /Patch/ })).toBeNull();
});

// `Git ▸ Rollback...` of the Project tree or the viewer (`RollbackAction` with files): the Rollback
// Changes dialog with the changes at or under its paths checked, staged and unstaged both going
// back to HEAD; never the selection of the staging tree.
test("rollback_from_the_git_file_menu_uses_its_paths_in_staging_mode", async () => {
  mockCommands({ rollback_files: () => "snapshot" });
  const view = await open();
  await fireEvent.click(rows("b.txt")[0]);

  view.component.rollback(["a.txt"]);

  const dialog = await screen.findByRole("dialog", { name: "Rollback Changes" });
  const checked = within(dialog)
    .getAllByRole("checkbox")
    .filter((box) => (box as HTMLInputElement).checked)
    .map((box) => box.closest('[role="treeitem"]')?.getAttribute("aria-label") ?? box.getAttribute("aria-label"));
  expect(checked.join(" ")).toContain("a.txt");
  expect(checked.join(" ")).not.toContain("b.txt");
  expect(calls("staging_revert")).toHaveLength(0);
});
