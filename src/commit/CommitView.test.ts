import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Changelists, Conflict, ConflictSession, FileChange, FileState, RepoInfo } from "../lib/api";
import { trackTasks } from "../lib/tasks";
import CommitView from "./CommitView.svelte";
import { signOffKey } from "./options";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({ label: "main" }),
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const REPO: RepoInfo = {
  path: "/repo",
  head: "abc",
  branch: "main",
  recovery: null,
  pending: [],
  state: { operation: null, branch: "main", label: "main", head: "abcdef1234567890" },
};

const change = (path: string, state: FileState = "modified", oldPath: string | null = null) =>
  ({ path, oldPath, state, staged: false, unstaged: true }) satisfies FileChange;

const LISTS: Changelists = {
  lists: [
    { id: "default", name: "Changes", comment: "" },
    { id: "other", name: "Other", comment: "" },
  ],
  active: "default",
  assignments: { "a.txt": "default", "b.txt": "default", "c.txt": "other" },
  hunkAssignments: {},
};

let repoChanged: () => void = () => {};
let sent: unknown[] = [];
const listeners = new Map<string, (payload: unknown) => void>();

// jsdom has no ResizeObserver, which `bind:clientHeight` needs.
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
    repo_status: () => [change("a.txt"), change("b.txt"), change("c.txt")],
    changelists_get: () => LISTS,
    messages_list: () => [],
    commit_warnings: () => [],
    repo_state: () => REPO.state,
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
  listeners.clear();
  vi.mocked(listen).mockImplementation(async (event, handler) => {
    listeners.set(event as string, (payload) =>
      handler({ event: event as string, id: 0, payload } as never),
    );
    if (event === "repo-changed") {
      repoChanged = () => handler({ event: "repo-changed", id: 0, payload: undefined });
    }
    return () => {};
  });
  sent = [];
  localStorage.clear();
});

afterEach(cleanup);

async function open(props: Record<string, unknown> = {}) {
  const view = render(CommitView, { repo: REPO, active: true, ...props });
  await screen.findByRole("treeitem", { name: /Changes/ });
  return view;
}

const row = (name: string | RegExp) => screen.getByRole("treeitem", { name });
const checkOf = (name: string | RegExp) => within(row(name)).getByRole("checkbox");
const messageField = () => screen.getByRole("textbox", { name: "Commit Message" });
const commitButton = () => screen.getByRole("button", { name: "Commit" });

test("shows_each_list_with_its_files", async () => {
  mockCommands();
  await open();

  expect(row(/^Changes/).textContent).toContain("2 files");
  expect(row(/^Other/).textContent).toContain("1 file");
  expect(row("a.txt")).toBeTruthy();
  expect(calls("repo_status")[0][1]).toEqual({ includeIgnored: false });
  expect(calls("changelists_get")[0][1]).toEqual({ paths: ["a.txt", "b.txt", "c.txt"] });
});

test("on_open_the_active_list_is_checked", async () => {
  mockCommands();
  await open();

  expect(checkOf("a.txt").getAttribute("aria-checked")).toBe("true");
  expect(checkOf(/^Changes/).getAttribute("aria-checked")).toBe("true");
  expect(checkOf("c.txt").getAttribute("aria-checked")).toBe("false");
});

test("unversioned_collapsed_by_default", async () => {
  mockCommands({ repo_status: () => [change("a.txt"), change("new.txt", "untracked")] });
  await open();

  expect(row(/^Unversioned Files/).textContent).toContain("1 file");
  expect(screen.queryByRole("treeitem", { name: "new.txt" })).toBeNull();

  await fireEvent.click(within(row(/^Unversioned Files/)).getByTestId("chevron"));

  expect(row("new.txt")).toBeTruthy();
});

test("repo_changed_reloads_without_losing_checkboxes_or_selection", async () => {
  mockCommands();
  await open();
  await fireEvent.click(checkOf("b.txt"));
  await fireEvent.click(row("a.txt"));

  mockCommands({
    repo_status: () => [change("a.txt"), change("b.txt"), change("c.txt"), change("d.txt")],
    changelists_get: () => ({
      ...LISTS,
      assignments: { ...LISTS.assignments, "d.txt": "default" },
    }),
  });
  repoChanged();

  await screen.findByRole("treeitem", { name: "d.txt" });
  expect(checkOf("a.txt").getAttribute("aria-checked")).toBe("true");
  expect(checkOf("b.txt").getAttribute("aria-checked")).toBe("false");
  expect(checkOf("d.txt").getAttribute("aria-checked")).toBe("false");
  expect(row("a.txt").getAttribute("aria-selected")).toBe("true");
});

test("commit_sends_what_is_checked", async () => {
  mockCommands({
    repo_status: () => [
      change("a.txt"),
      change("deleted.txt", "deleted"),
      change("new.txt", "renamed", "old.txt"),
      change("c.txt"),
    ],
    changelists_get: () => ({
      ...LISTS,
      assignments: {
        "a.txt": "default",
        "deleted.txt": "default",
        "new.txt": "default",
        "c.txt": "other",
      },
    }),
    commit_changes: () => "hash",
  });
  await open();

  await fireEvent.input(messageField(), { target: { value: "message" } });
  await fireEvent.click(commitButton());

  await waitFor(() => expect(calls("commit_changes")).toHaveLength(1));
  expect(calls("commit_changes")[0][1]).toEqual({
    request: {
      added: ["a.txt", "new.txt"],
      removed: ["deleted.txt", "old.txt"],
      partial: [],
      message: "message",
      options: { amend: false, signOff: false, skipHooks: false, author: null, cleanup: false },
    },
  });
  expect(await screen.findByText("3 files committed: message")).toBeTruthy();
  expect((messageField() as HTMLTextAreaElement).value).toBe("");
});

test("an_inactive_list_left_empty_after_the_commit_is_offered_for_removal", async () => {
  mockCommands({ commit_changes: () => "hash", changelists_remove: () => LISTS });
  await open();
  await fireEvent.click(checkOf(/^Other/));
  await fireEvent.click(checkOf(/^Changes/));
  mockCommands({
    repo_status: () => [change("a.txt"), change("b.txt")],
    commit_changes: () => "hash",
    changelists_remove: () => LISTS,
  });

  await fireEvent.input(messageField(), { target: { value: "message" } });
  await fireEvent.click(commitButton());

  const dialog = await screen.findByRole("dialog", { name: "Remove Empty Changelist" });
  expect(dialog.textContent).toContain("The empty changelist 'Other' is no longer active.");
  expect(dialog.textContent).toContain("Do you want to remove it?");
  await fireEvent.click(within(dialog).getByRole("button", { name: "Yes" }));

  await waitFor(() => expect(calls("changelists_remove")).toHaveLength(1));
  expect(calls("changelists_remove")[0][1]).toEqual({ id: "other" });
});

test("commit_of_the_active_list_asks_nothing", async () => {
  mockCommands({ commit_changes: () => "hash" });
  await open();
  mockCommands({ repo_status: () => [change("c.txt")], commit_changes: () => "hash" });

  await fireEvent.input(messageField(), { target: { value: "message" } });
  await fireEvent.click(commitButton());

  await screen.findByText("2 files committed: message");
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("commit_with_hunk_selection_uses_the_selection", async () => {
  mockCommands({ commit_changes_selected: () => "hash" });
  const { component } = await open();
  const selection = [{ hunk: "h1", lines: null }];
  (component as unknown as { setLineSelection: Function }).setLineSelection("a.txt", selection);

  await fireEvent.input(messageField(), { target: { value: "partial" } });
  await fireEvent.click(commitButton());

  await waitFor(() => expect(calls("commit_changes_selected")).toHaveLength(1));
  const args = calls("commit_changes_selected")[0][1] as { selections: unknown; request: { added: string[] } };
  expect(args.selections).toEqual([{ path: "a.txt", selection }]);
  expect(args.request.added).toEqual(["b.txt"]);
});

test("a_lost_selection_is_removed_and_reported", async () => {
  mockCommands({
    commit_changes_selected: () => {
      throw { kind: "selection_lost", message: "x", details: { path: "a.txt", hunks: ["h1"] } };
    },
    commit_changes: () => "hash",
  });
  const { component } = await open();
  (component as unknown as { setLineSelection: Function }).setLineSelection("a.txt", [
    { hunk: "h1", lines: null },
  ]);

  await fireEvent.input(messageField(), { target: { value: "partial" } });
  await fireEvent.click(commitButton());
  expect(await screen.findByText(/"a.txt" has changed/)).toBeTruthy();
  expect(checkOf("a.txt").getAttribute("aria-checked")).toBe("false");

  await fireEvent.click(commitButton());
  await waitFor(() => expect(calls("commit_changes")).toHaveLength(1));
  const { request } = calls("commit_changes")[0][1] as { request: { added: string[] } };
  expect(request.added).toEqual(["b.txt"]);
});

test("the_failed_commit_says_which_lines_were_lost", async () => {
  mockCommands({
    commit_changes_selected: () => {
      throw { kind: "selection_lost", message: "x", details: { path: "a.txt", hunks: ["h1"] } };
    },
  });
  const { component } = await open();
  (component as unknown as { setLineSelection: Function }).setLineSelection(
    "a.txt",
    [{ hunk: "h1", lines: null }],
    true,
    { h1: "lines 3–4" },
  );

  await fireEvent.input(messageField(), { target: { value: "partial" } });
  await fireEvent.click(commitButton());

  expect(await screen.findByText(/lines 3–4/)).toBeTruthy();
});

test("if_devin_changes_the_file_what_is_valid_is_kept_and_the_rest_is_reported", async () => {
  const lost: unknown[] = [];
  mockCommands({ file_hunks: () => [{ id: "h2", oldStart: 0, oldLines: 1, newStart: 0, newLines: 1 }] });
  const { component } = await open({ onselectionlost: (item: unknown) => lost.push(item) });
  const view = component as unknown as { setLineSelection: Function };
  view.setLineSelection(
    "a.txt",
    [
      { hunk: "h1", lines: null },
      { hunk: "h2", lines: [0] },
    ],
    true,
    { h1: "lines 3–4" },
  );

  repoChanged();

  expect(await screen.findByText(/"a.txt" has changed.*lines 3–4.*The rest stays selected/)).toBeTruthy();
  expect(checkOf("a.txt").getAttribute("aria-checked")).toBe("mixed");
  expect(lost).toEqual([
    expect.objectContaining({ path: "a.txt", hunks: ["h1"], text: expect.stringContaining("lines 3–4") }),
  ]);

  await fireEvent.input(messageField(), { target: { value: "partial" } });
  mockCommands({
    file_hunks: () => [{ id: "h2", oldStart: 0, oldLines: 1, newStart: 0, newLines: 1 }],
    commit_changes_selected: () => "hash",
  });
  await fireEvent.click(commitButton());
  await waitFor(() => expect(calls("commit_changes_selected")).toHaveLength(1));
  const args = calls("commit_changes_selected")[0][1] as { selections: unknown };
  expect(args.selections).toEqual([
    { path: "a.txt", selection: [{ hunk: "h2", lines: [0] }] },
  ]);
});

test("if_the_whole_selection_is_lost_the_file_leaves_the_commit", async () => {
  mockCommands({ file_hunks: () => [] });
  const { component } = await open();
  (component as unknown as { setLineSelection: Function }).setLineSelection("a.txt", [
    { hunk: "h1", lines: null },
  ]);

  repoChanged();

  await screen.findByText(/"a.txt" has changed/);
  expect(checkOf("a.txt").getAttribute("aria-checked")).toBe("false");
});

test("if_the_file_stops_having_changes_it_is_also_reported", async () => {
  let status = [change("a.txt"), change("b.txt")];
  mockCommands({ repo_status: () => status, file_hunks: () => [] });
  const { component } = await open();
  (component as unknown as { setLineSelection: Function }).setLineSelection("a.txt", [
    { hunk: "h1", lines: null },
  ]);

  status = [change("b.txt")];
  repoChanged();

  expect(await screen.findByText(/"a.txt" has changed/)).toBeTruthy();
});

test("a_rollback_of_the_file_does_not_report_a_lost_selection", async () => {
  let status = [change("a.txt"), change("b.txt")];
  mockCommands({
    repo_status: () => status,
    file_hunks: () => [],
    rollback_files: () => {
      status = [change("b.txt")];
      return "snapshot";
    },
  });
  const { component } = await open();
  (component as unknown as { setLineSelection: Function }).setLineSelection("a.txt", [
    { hunk: "h1", lines: null },
  ]);

  await fireEvent.contextMenu(row("a.txt"));
  await fireEvent.click(await screen.findByRole("menuitem", { name: "Rollback…" }));
  await fireEvent.click(await screen.findByRole("button", { name: "Rollback" }));

  await screen.findByText(/Rollback of/);
  expect(screen.queryByText(/has changed/)).toBeNull();
});

test("while_the_commit_runs_the_selection_is_not_taken_as_lost", async () => {
  let finish: (hash: string) => void = () => {};
  mockCommands({
    file_hunks: () => [],
    commit_changes_selected: () => new Promise((resolve) => (finish = resolve)),
  });
  const { component } = await open();
  (component as unknown as { setLineSelection: Function }).setLineSelection("a.txt", [
    { hunk: "h1", lines: null },
  ]);
  await fireEvent.input(messageField(), { target: { value: "partial" } });
  await fireEvent.click(commitButton());

  repoChanged();
  await new Promise((resolve) => setTimeout(resolve, 0));
  finish("hash");

  await screen.findByText(/committed: partial/);
  expect(screen.queryByText(/has changed/)).toBeNull();
});

test("a_new_selection_during_the_commit_is_not_lost", async () => {
  let finish: (hash: string) => void = () => {};
  const hunk = { id: "h2", oldStart: 0, oldLines: 1, newStart: 0, newLines: 1 };
  mockCommands({
    file_hunks: () => [hunk],
    commit_changes_selected: () => new Promise((resolve) => (finish = resolve)),
  });
  const { component } = await open();
  const view = component as unknown as { setLineSelection: Function };
  view.setLineSelection("a.txt", [{ hunk: "h2", lines: null }]);
  await fireEvent.input(messageField(), { target: { value: "partial" } });
  await fireEvent.click(commitButton());

  const newer = [{ hunk: "h2", lines: [1] }];
  view.setLineSelection("a.txt", newer);
  finish("hash");
  await screen.findByText(/committed: partial/);

  await fireEvent.input(messageField(), { target: { value: "another" } });
  await fireEvent.click(commitButton());
  await waitFor(() => expect(calls("commit_changes_selected")).toHaveLength(2));
  const args = calls("commit_changes_selected")[1][1] as { selections: unknown };
  expect(args.selections).toEqual([{ path: "a.txt", selection: newer }]);
});

test("choosing_in_the_diff_checks_and_unchecks_the_file", async () => {
  mockCommands();
  const { component } = await open();
  const view = component as unknown as { setLineSelection: Function };

  view.setLineSelection("c.txt", [{ hunk: "h1", lines: null }], true);
  await waitFor(() => expect(checkOf("c.txt").getAttribute("aria-checked")).toBe("mixed"));

  view.setLineSelection("c.txt", [], false);
  await waitFor(() => expect(checkOf("c.txt").getAttribute("aria-checked")).toBe("false"));
});

test("unchecking_a_file_deletes_its_selection_and_checking_it_brings_it_back_whole", async () => {
  mockCommands({ commit_changes: () => "hash" });
  const { component } = await open({ onselectionstate: (state: unknown) => sent.push(state) });
  (component as unknown as { setLineSelection: Function }).setLineSelection(
    "a.txt",
    [{ hunk: "h1", lines: [0] }],
    true,
  );

  await fireEvent.click(checkOf("a.txt"));
  await waitFor(() => expect(sent.at(-1)).toMatchObject({ selections: {} }));
  expect(checkOf("a.txt").getAttribute("aria-checked")).toBe("false");

  await fireEvent.click(checkOf("a.txt"));
  await fireEvent.input(messageField(), { target: { value: "whole" } });
  await fireEvent.click(commitButton());

  await waitFor(() => expect(calls("commit_changes")).toHaveLength(1));
  const { request } = calls("commit_changes")[0][1] as { request: { added: string[] } };
  expect(request.added).toContain("a.txt");
});

test("a_partly_chosen_file_shows_with_the_mixed_checkbox", async () => {
  mockCommands();
  const { component } = await open();

  (component as unknown as { setLineSelection: Function }).setLineSelection(
    "a.txt",
    [{ hunk: "h1", lines: [0] }],
    true,
  );

  await waitFor(() => expect(checkOf("a.txt").getAttribute("aria-checked")).toBe("mixed"));
  expect(checkOf(/^Changes/).getAttribute("aria-checked")).toBe("mixed");
});

test("if_the_hunks_of_a_split_file_cannot_be_read_there_is_no_commit", async () => {
  let readable = true;
  mockCommands({
    repo_status: () => [change("a.txt")],
    changelists_get: () => ({
      ...LISTS,
      assignments: { "a.txt": "default" },
      hunkAssignments: { "a.txt": { h1: "default", h2: "other" } },
    }),
    file_hunks: () => {
      if (!readable) throw new Error("unread");
      return [
        { id: "h1", oldStart: 0, oldLines: 1, newStart: 0, newLines: 1 },
        { id: "h2", oldStart: 9, oldLines: 1, newStart: 9, newLines: 1 },
      ];
    },
    commit_changes: () => "hash",
    commit_changes_selected: () => "hash",
  });
  await open();
  readable = false;
  repoChanged();
  await new Promise((resolve) => setTimeout(resolve, 0));
  await waitFor(() => expect(calls("file_hunks").length).toBeGreaterThan(1));

  await fireEvent.input(messageField(), { target: { value: "list" } });
  await fireEvent.click(commitButton());

  expect(await screen.findByText(/"a.txt".*split across lists.*the commit was not made/)).toBeTruthy();
  expect(calls("commit_changes")).toHaveLength(0);
  expect(calls("commit_changes_selected")).toHaveLength(0);
});

test("tells_the_diff_window_what_is_chosen", async () => {
  mockCommands();
  const { component } = await open({ onselectionstate: (state: unknown) => sent.push(state) });
  const selection = [{ hunk: "h1", lines: [0] }];

  (component as unknown as { setLineSelection: Function }).setLineSelection("a.txt", selection, true);

  await waitFor(() =>
    expect(sent.at(-1)).toEqual({
      included: expect.arrayContaining(["a.txt"]),
      selections: { "a.txt": selection },
      lists: expect.objectContaining({ active: "default" }),
    }),
  );
});

test("the_commit_of_a_list_carries_only_the_hunks_of_that_list", async () => {
  const hunks = [
    { id: "h1", oldStart: 0, oldLines: 1, newStart: 0, newLines: 1 },
    { id: "h2", oldStart: 9, oldLines: 1, newStart: 9, newLines: 1 },
  ];
  mockCommands({
    repo_status: () => [change("a.txt")],
    changelists_get: () => ({
      ...LISTS,
      assignments: { "a.txt": "default" },
      hunkAssignments: { "a.txt": { h1: "default", h2: "other" } },
    }),
    file_hunks: () => hunks,
    commit_changes_selected: () => "hash",
  });
  await open();
  expect(screen.getAllByRole("treeitem", { name: /a\.txt/ })).toHaveLength(2);

  await fireEvent.input(messageField(), { target: { value: "list" } });
  await fireEvent.click(commitButton());

  await waitFor(() => expect(calls("commit_changes_selected")).toHaveLength(1));
  const args = calls("commit_changes_selected")[0][1] as { selections: unknown };
  expect(args.selections).toEqual([
    { path: "a.txt", selection: [{ hunk: "h1", lines: null }] },
  ]);
});

test("if_only_part_of_the_selection_is_lost_the_rest_stays_checked_and_by_lines", async () => {
  mockCommands({
    commit_changes_selected: (args: { selections: { selection: unknown[] }[] }) => {
      if (args.selections[0].selection.length === 2) {
        throw { kind: "selection_lost", message: "x", details: { path: "a.txt", hunks: ["h1"] } };
      }
      return "hash";
    },
  });
  const { component } = await open();
  (component as unknown as { setLineSelection: Function }).setLineSelection("a.txt", [
    { hunk: "h1", lines: null },
    { hunk: "h2", lines: null },
  ]);

  await fireEvent.input(messageField(), { target: { value: "partial" } });
  await fireEvent.click(commitButton());
  await screen.findByText(/"a.txt" has changed/);
  expect(checkOf("a.txt").getAttribute("aria-checked")).toBe("mixed");

  await fireEvent.click(commitButton());

  await waitFor(() => expect(calls("commit_changes_selected")).toHaveLength(2));
  const last = calls("commit_changes_selected")[1][1] as { selections: unknown };
  expect(last.selections).toEqual([{ path: "a.txt", selection: [{ hunk: "h2", lines: null }] }]);
});

test("after_a_partial_commit_the_file_is_left_unchecked_and_is_not_sent_whole", async () => {
  mockCommands({ commit_changes_selected: () => "hash", commit_changes: () => "hash" });
  const { component } = await open();
  (component as unknown as { setLineSelection: Function }).setLineSelection("a.txt", [
    { hunk: "h1", lines: null },
  ]);

  await fireEvent.input(messageField(), { target: { value: "partial" } });
  await fireEvent.click(commitButton());
  await waitFor(() => expect(calls("commit_changes_selected")).toHaveLength(1));
  await screen.findByText("2 files committed: partial");

  expect(checkOf("a.txt").getAttribute("aria-checked")).toBe("false");

  await fireEvent.input(messageField(), { target: { value: "next" } });
  await fireEvent.click(commitButton());

  await waitFor(() => expect(calls("commit_changes")).toHaveLength(1));
  const { request } = calls("commit_changes")[0][1] as { request: { added: string[] } };
  expect(request.added).toEqual(["b.txt"]);
});

test("commit_without_message_or_files_says_so_like_intellij", async () => {
  mockCommands();
  await open();
  await fireEvent.click(checkOf(/^Changes/));

  await fireEvent.click(commitButton());

  expect(await screen.findByText("Select files to commit and specify commit message")).toBeTruthy();
  expect(calls("commit_changes")).toHaveLength(0);
});

test("commit_without_message_says_so", async () => {
  mockCommands();
  await open();

  await fireEvent.click(commitButton());

  expect(await screen.findByText("Specify commit message")).toBeTruthy();
});

const pushButton = () =>
  screen.getByRole("button", { name: "Commit and Push…" }) as HTMLButtonElement;

test("commit_and_push_is_disabled_without_anyone_to_push", async () => {
  mockCommands();
  await open();

  expect(pushButton().disabled).toBe(true);
});

test("commit_and_push_commits_and_then_pushes", async () => {
  const order: string[] = [];
  const onpush = vi.fn(async () => {
    order.push("push");
  });
  mockCommands({
    commit_changes: () => {
      order.push("commit");
      return "hash";
    },
  });
  await open({ onpreparepush: () => onpush });

  await fireEvent.input(messageField(), { target: { value: "message" } });
  await fireEvent.click(pushButton());

  await waitFor(() => expect(onpush).toHaveBeenCalledTimes(1));
  expect(order).toEqual(["commit", "push"]);
});

test("the_push_is_prepared_before_the_commit", async () => {
  const order: string[] = [];
  mockCommands({
    commit_changes: () => {
      order.push("commit");
      return "hash";
    },
  });
  await open({
    onpreparepush: () => {
      order.push("prepares");
      return async () => {
        order.push("pushes");
      };
    },
  });

  await fireEvent.input(messageField(), { target: { value: "message" } });
  await fireEvent.click(pushButton());

  await waitFor(() => expect(order).toEqual(["prepares", "commit", "pushes"]));
});

test("if_the_view_goes_away_during_the_commit_it_does_not_push", async () => {
  const pending = deferred<string>();
  const onpush = vi.fn(async () => {});
  mockCommands({ commit_changes: () => pending.promise });
  const { unmount } = await open({ onpreparepush: () => onpush });

  await fireEvent.input(messageField(), { target: { value: "message" } });
  await fireEvent.click(pushButton());
  await waitFor(() => expect(calls("commit_changes")).toHaveLength(1));
  unmount();
  pending.resolve("hash");
  await new Promise((resolve) => setTimeout(resolve));

  expect(onpush).not.toHaveBeenCalled();
});

test("cmd_alt_k_launches_commit_and_push_from_the_message", async () => {
  const onpush = vi.fn(async () => {});
  mockCommands({ commit_changes: () => "hash" });
  await open({ onpreparepush: () => onpush });

  await fireEvent.input(messageField(), { target: { value: "message" } });
  await fireEvent.keyDown(messageField(), {
    key: "˚",
    code: "KeyK",
    metaKey: true,
    altKey: true,
  });

  await waitFor(() => expect(onpush).toHaveBeenCalledTimes(1));
});

test("partial_commit_and_push_uses_the_line_commit", async () => {
  const onpush = vi.fn(async () => {});
  const selection = [{ hunk: "h1", lines: null }];
  mockCommands({ commit_changes_selected: () => "hash" });
  const { component } = await open({ onpreparepush: () => onpush });
  (component as unknown as { setLineSelection: Function }).setLineSelection("a.txt", selection);

  await fireEvent.input(messageField(), { target: { value: "message" } });
  await fireEvent.click(pushButton());

  await waitFor(() => expect(onpush).toHaveBeenCalledTimes(1));
  expect(calls("commit_changes_selected")).toHaveLength(1);
  expect(calls("commit_changes")).toHaveLength(0);
});

test("without_message_commit_and_push_does_not_push", async () => {
  const onpush = vi.fn(async () => {});
  mockCommands();
  await open({ onpreparepush: () => onpush });

  await fireEvent.click(pushButton());

  expect(await screen.findByText("Specify commit message")).toBeTruthy();
  expect(onpush).not.toHaveBeenCalled();
});

test("a_failing_commit_does_not_push", async () => {
  const onpush = vi.fn(async () => {});
  mockCommands({
    commit_changes: () => {
      throw { kind: "hook_failed", message: "A hook rejected the commit: no" };
    },
  });
  await open({ onpreparepush: () => onpush });

  await fireEvent.input(messageField(), { target: { value: "message" } });
  await fireEvent.click(pushButton());

  expect(await screen.findByRole("alert")).toBeTruthy();
  expect(onpush).not.toHaveBeenCalled();
});

test("cancelling_the_commit_warning_does_not_push", async () => {
  const onpush = vi.fn(async () => {});
  mockCommands({ commit_warnings: () => [{ kind: "detachedHead" }] });
  await open({ onpreparepush: () => onpush });

  await fireEvent.input(messageField(), { target: { value: "message" } });
  await fireEvent.click(pushButton());
  const warning = await screen.findByRole("dialog", { name: "Commit in Detached HEAD" });

  expect(within(warning).getByRole("button", { name: "Commit and Push" })).toBeTruthy();
  await fireEvent.click(within(warning).getByRole("button", { name: "Cancel" }));

  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(calls("commit_changes")).toHaveLength(0);
  expect(onpush).not.toHaveBeenCalled();
});

test("the large file and file name warnings come one after the other in the non_modal_view", async () => {
  mockCommands({
    commit_warnings: () => [
      { kind: "largeFiles", paths: [["big.bin", 70 * 1024 * 1024]] },
      { kind: "badFileNames", paths: [{ name: "aux.txt", problem: "reserved" }] },
    ],
    commit_changes: () => "hash",
  });
  await open();

  await fireEvent.input(messageField(), { target: { value: "message" } });
  await fireEvent.click(screen.getByRole("button", { name: "Commit" }));
  const large = await screen.findByRole("dialog", { name: "Commit Check Failed" });
  expect(calls("commit_changes")).toHaveLength(0);
  await fireEvent.click(within(large).getByRole("button", { name: "Commit" }));
  const names = await screen.findByRole("dialog", { name: "File Name Is Not Allowed on Windows" });
  await fireEvent.click(within(names).getByRole("button", { name: "Cancel" }));

  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(calls("commit_changes")).toHaveLength(0);
});

test("amend_and_push_pushes_after_the_amend_without_forcing_anything", async () => {
  const onpush = vi.fn(async () => {});
  mockCommands({ commit_changes: () => "hash", head_message: () => "before" });
  await open({ onpreparepush: () => onpush });

  await fireEvent.click(screen.getByRole("checkbox", { name: "Amend" }));
  await fireEvent.click(
    await screen.findByRole("button", { name: "Amend Commit and Push…" }),
  );

  await waitFor(() => expect(onpush).toHaveBeenCalledTimes(1));
  expect(onpush).toHaveBeenCalledWith();
  const [, args] = calls("commit_changes")[0] as [string, { request: { options: { amend: boolean } } }];
  expect(args.request.options.amend).toBe(true);
});

test("a_commit_failure_keeps_the_message_and_is_shown", async () => {
  mockCommands({
    commit_changes: () => {
      throw { kind: "hook_failed", message: "A hook rejected the commit: no" };
    },
  });
  await open();

  await fireEvent.input(messageField(), { target: { value: "message" } });
  await fireEvent.click(commitButton());

  expect(await screen.findByText("A hook rejected the commit: no")).toBeTruthy();
  expect((messageField() as HTMLTextAreaElement).value).toBe("message");
});

test("rollback_confirms_and_offers_to_restore", async () => {
  mockCommands({
    repo_status: () => [change("a.txt"), change("new.txt", "added")],
    changelists_get: () => ({
      ...LISTS,
      assignments: { "a.txt": "default", "new.txt": "default" },
    }),
    rollback_files: () => "snapshot-1",
    snapshots_restore: () => "snapshot-2",
  });
  await open();
  await fireEvent.click(row("a.txt"));

  await fireEvent.click(screen.getByRole("button", { name: "Rollback…" }));

  const dialog = await screen.findByRole("dialog", { name: "Rollback Changes" });
  expect(within(dialog).getByRole("checkbox", { name: "a.txt" })).toHaveProperty("checked", true);
  expect(within(dialog).getByRole("checkbox", { name: "new.txt" })).toHaveProperty(
    "checked",
    false,
  );
  const deleteAdded = within(dialog).getByRole("checkbox", {
    name: "Delete local copies of added files",
  });
  expect(deleteAdded).toHaveProperty("disabled", true);

  await fireEvent.click(within(dialog).getByRole("checkbox", { name: "new.txt" }));
  await fireEvent.click(deleteAdded);
  await fireEvent.click(within(dialog).getByRole("button", { name: "Rollback" }));

  await waitFor(() => expect(calls("rollback_files")).toHaveLength(1));
  expect(calls("rollback_files")[0][1]).toEqual({
    paths: ["a.txt", "new.txt"],
    deleteAdded: true,
  });

  await fireEvent.click(await screen.findByRole("button", { name: "Restore" }));

  await waitFor(() => expect(calls("snapshots_restore")).toHaveLength(1));
  expect(calls("snapshots_restore")[0][1]).toEqual({ id: "snapshot-1", expectedRepo: "/repo" });
});

test("closing_the_rollback_dialog_touches_nothing", async () => {
  mockCommands();
  await open();

  await fireEvent.click(screen.getByRole("button", { name: "Rollback…" }));
  const dialog = await screen.findByRole("dialog", { name: "Rollback Changes" });
  await fireEvent.click(within(dialog).getByRole("button", { name: "Close" }));

  expect(screen.queryByRole("dialog")).toBeNull();
  expect(calls("rollback_files")).toHaveLength(0);
});

async function contextMenu(name: string | RegExp) {
  await fireEvent.contextMenu(row(name));
  return screen.findByRole("menu");
}

test("create_a_list_from_the_menu", async () => {
  mockCommands({ changelists_create: () => LISTS });
  await open();

  const menu = await contextMenu("a.txt");
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "New Changelist..." }));
  const dialog = await screen.findByRole("dialog", { name: "New Changelist" });
  await fireEvent.input(within(dialog).getByLabelText("Name:"), { target: { value: "New" } });
  await fireEvent.click(within(dialog).getByRole("button", { name: "OK" }));

  await waitFor(() => expect(calls("changelists_create")).toHaveLength(1));
  expect(calls("changelists_create")[0][1]).toEqual({ name: "New" });
});

test("a_repeated_name_is_not_accepted", async () => {
  mockCommands();
  await open();

  const menu = await contextMenu("a.txt");
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "New Changelist..." }));
  const dialog = await screen.findByRole("dialog", { name: "New Changelist" });
  await fireEvent.input(within(dialog).getByLabelText("Name:"), { target: { value: "other" } });

  expect(within(dialog).getByText("A changelist with that name already exists")).toBeTruthy();
  expect(within(dialog).getByRole("button", { name: "OK" })).toHaveProperty("disabled", true);
});

test("move_to_another_list", async () => {
  mockCommands({ changelists_move: () => LISTS });
  await open();
  await fireEvent.click(row("a.txt"));

  await fireEvent.keyDown(window, { key: "m", code: "KeyM", metaKey: true, shiftKey: true });

  const dialog = await screen.findByRole("dialog", { name: "Move to Another Changelist" });
  await fireEvent.click(within(dialog).getByRole("option", { name: "Other" }));
  await fireEvent.click(within(dialog).getByRole("button", { name: "OK" }));

  await waitFor(() => expect(calls("changelists_move")).toHaveLength(1));
  expect(calls("changelists_move")[0][1]).toEqual({ paths: ["a.txt"], id: "other" });
});

test("moving_to_a_new_list_creates_it_first", async () => {
  const created = {
    ...LISTS,
    lists: [...LISTS.lists, { id: "new", name: "New", comment: "" }],
  };
  mockCommands({ changelists_create: () => created, changelists_move: () => created });
  await open();
  await fireEvent.click(row("a.txt"));

  await fireEvent.keyDown(window, { key: "m", code: "KeyM", metaKey: true, shiftKey: true });
  const dialog = await screen.findByRole("dialog", { name: "Move to Another Changelist" });
  await fireEvent.input(within(dialog).getByLabelText("Name:"), { target: { value: "New" } });
  await fireEvent.click(within(dialog).getByRole("button", { name: "OK" }));

  await waitFor(() => expect(calls("changelists_move")).toHaveLength(1));
  expect(calls("changelists_create")[0][1]).toEqual({ name: "New" });
  expect(calls("changelists_move")[0][1]).toEqual({ paths: ["a.txt"], id: "new" });
});

test("removing_a_list_with_files_asks_like_intellij", async () => {
  mockCommands({ changelists_remove: () => LISTS });
  await open();

  const menu = await contextMenu(/^Other/);
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Delete Changelist" }));
  const dialog = await screen.findByRole("dialog", { name: "Delete Changelist" });
  expect(dialog.textContent).toContain("Are you sure want to remove changelist 'Other'?");
  expect(dialog.textContent).toContain("All changes will be moved to the active changelist.");

  await fireEvent.click(within(dialog).getByRole("button", { name: "Yes" }));

  await waitFor(() => expect(calls("changelists_remove")).toHaveLength(1));
  expect(calls("changelists_remove")[0][1]).toEqual({ id: "other" });
});

test("removing_an_empty_list_does_not_ask", async () => {
  mockCommands({
    repo_status: () => [change("a.txt")],
    changelists_get: () => ({ ...LISTS, assignments: { "a.txt": "default" } }),
    changelists_remove: () => LISTS,
  });
  await open();

  const menu = await contextMenu(/^Other/);
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Delete Changelist" }));

  await waitFor(() => expect(calls("changelists_remove")).toHaveLength(1));
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("removing_the_active_list_asks_which_one_to_move_to", async () => {
  mockCommands({ changelists_set_active: () => LISTS, changelists_remove: () => LISTS });
  await open();

  const menu = await contextMenu(/^Changes/);
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Delete Changelist" }));
  const dialog = await screen.findByRole("dialog", { name: "Delete Active Changelist" });
  expect(dialog.textContent).toContain(
    "Select the changelist to which the changes should be moved:",
  );
  await fireEvent.click(within(dialog).getByRole("option", { name: "Other" }));
  await fireEvent.click(within(dialog).getByRole("button", { name: "OK" }));

  await waitFor(() => expect(calls("changelists_remove")).toHaveLength(1));
  const order = vi.mocked(invoke).mock.calls.map(([name]) => name);
  expect(order.indexOf("changelists_set_active")).toBeLessThan(order.indexOf("changelists_remove"));
  expect(calls("changelists_set_active")[0][1]).toEqual({ id: "other" });
});

test("the_refresh_button_reads_the_state_again", async () => {
  mockCommands();
  await open();
  const reads = calls("repo_status").length;

  await fireEvent.click(screen.getByRole("button", { name: "Refresh" }));

  await waitFor(() => expect(calls("repo_status").length).toBeGreaterThan(reads));
});

test("commit_file_leaves_only_that_file_checked", async () => {
  mockCommands();
  await open();
  await fireEvent.click(row("b.txt"));

  const menu = await contextMenu("b.txt");
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Commit File…" }));

  await waitFor(() => expect(checkOf("b.txt").getAttribute("aria-checked")).toBe("true"));
  expect(checkOf("a.txt").getAttribute("aria-checked")).toBe("false");
});

test("f2_renames_the_list", async () => {
  mockCommands({ changelists_rename: () => LISTS });
  await open();
  await fireEvent.click(row(/^Other/));

  await fireEvent.keyDown(row(/^Other/), { key: "F2", code: "F2" });
  const dialog = await screen.findByRole("dialog", { name: "Edit Changelist" });
  await fireEvent.input(within(dialog).getByLabelText("Name:"), { target: { value: "New" } });
  await fireEvent.click(within(dialog).getByRole("button", { name: "OK" }));

  await waitFor(() => expect(calls("changelists_rename")).toHaveLength(1));
});

test("the_menu_only_offers_what_applies", async () => {
  mockCommands();
  await open();

  const fileMenu = await contextMenu("a.txt");
  expect(within(fileMenu).queryByRole("menuitem", { name: "Delete Changelist" })).toBeNull();
  expect(within(fileMenu).getByRole("menuitem", { name: "Commit File…" })).toBeTruthy();
  await fireEvent.keyDown(fileMenu, { key: "Escape" });

  const listMenu = await contextMenu(/^Other/);
  expect(within(listMenu).getByRole("menuitem", { name: "Set Active Changelist" })).toBeTruthy();
  expect(within(listMenu).getByRole("menuitem", { name: "Edit Changelist..." })).toBeTruthy();
  expect(
    within(listMenu).getByRole("menuitem", { name: "Import Changelists from .idea" }),
  ).toBeTruthy();
  // `ChangesViewPopupMenu` is the same in either commit interface.
  expect(within(listMenu).getByRole("menuitem", { name: "Shelve Changes..." })).toBeTruthy();
});

test("import_changelists", async () => {
  mockCommands({ changelists_import: () => LISTS });
  await open();

  const menu = await contextMenu(/^Other/);
  await fireEvent.click(
    within(menu).getByRole("menuitem", { name: "Import Changelists from .idea" }),
  );

  await waitFor(() => expect(calls("changelists_import")).toHaveLength(1));
});

test("set_active_with_the_shortcut", async () => {
  mockCommands({ changelists_set_active: () => LISTS });
  await open();
  await fireEvent.click(row(/^Other/));

  await fireEvent.keyDown(window, { key: " ", code: "Space", ctrlKey: true });

  await waitFor(() => expect(calls("changelists_set_active")).toHaveLength(1));
  expect(calls("changelists_set_active")[0][1]).toEqual({ id: "other" });
});

test("double_click_enter_and_cmd_d_call_the_diff", async () => {
  mockCommands();
  const onshowdiff = vi.fn();
  await open({ onshowdiff });

  await fireEvent.dblClick(row("a.txt"));
  expect(onshowdiff).toHaveBeenCalledTimes(1);
  expect(onshowdiff.mock.calls[0][0].path).toBe("a.txt");

  await fireEvent.click(row("b.txt"));
  await fireEvent.keyDown(window, { key: "d", code: "KeyD", metaKey: true });
  expect(onshowdiff.mock.calls[1][0].path).toBe("b.txt");

  await fireEvent.keyDown(screen.getByRole("tree"), { key: "Enter" });
  expect(onshowdiff).toHaveBeenCalledTimes(3);
});

test("keyboard_in_the_tree", async () => {
  mockCommands();
  await open();
  const tree = screen.getByRole("tree");

  await fireEvent.keyDown(tree, { key: "ArrowDown" });
  expect(row(/^Changes/).getAttribute("aria-selected")).toBe("true");
  await fireEvent.keyDown(tree, { key: "ArrowDown" });
  expect(row("a.txt").getAttribute("aria-selected")).toBe("true");

  await fireEvent.keyDown(tree, { key: " " });
  expect(checkOf("a.txt").getAttribute("aria-checked")).toBe("false");

  await fireEvent.keyDown(tree, { key: "ArrowLeft" });
  expect(row(/^Changes/).getAttribute("aria-selected")).toBe("true");
  await fireEvent.keyDown(tree, { key: "ArrowLeft" });
  expect(screen.queryByRole("treeitem", { name: "a.txt" })).toBeNull();
  await fireEvent.keyDown(tree, { key: "ArrowRight" });
  expect(row("a.txt")).toBeTruthy();
  await fireEvent.keyDown(tree, { key: "ArrowRight" });
  expect(row("a.txt").getAttribute("aria-selected")).toBe("true");
});

test("the_selection_shortcuts_do_not_act_from_the_message", async () => {
  mockCommands();
  await open();
  await fireEvent.click(row("a.txt"));

  await fireEvent.keyDown(messageField(), { key: "F2", code: "F2" });
  await fireEvent.keyDown(messageField(), { key: "m", code: "KeyM", metaKey: true, shiftKey: true });

  expect(screen.queryByRole("dialog")).toBeNull();
});

test("cmd_k_goes_to_the_message_and_includes_what_is_new", async () => {
  mockCommands();
  await open();
  mockCommands({
    repo_status: () => [change("a.txt"), change("b.txt"), change("c.txt"), change("d.txt")],
    changelists_get: () => ({
      ...LISTS,
      assignments: { ...LISTS.assignments, "d.txt": "default" },
    }),
  });
  repoChanged();
  await screen.findByRole("treeitem", { name: "d.txt" });
  expect(checkOf("d.txt").getAttribute("aria-checked")).toBe("false");

  await fireEvent.keyDown(window, { key: "k", code: "KeyK", metaKey: true });

  expect(checkOf("d.txt").getAttribute("aria-checked")).toBe("true");
  expect(document.activeElement).toBe(messageField());
});

test("cmd_enter_commits", async () => {
  mockCommands({ commit_changes: () => "hash" });
  await open();
  await fireEvent.input(messageField(), { target: { value: "message" } });

  await fireEvent.keyDown(messageField(), { key: "Enter", code: "Enter", metaKey: true });

  await waitFor(() => expect(calls("commit_changes")).toHaveLength(1));
});

test("does_nothing_if_the_view_is_not_active", async () => {
  mockCommands();
  const onshowdiff = vi.fn();
  await open({ active: false, onshowdiff });
  await fireEvent.click(row("a.txt"));

  await fireEvent.keyDown(window, { key: "d", code: "KeyD", metaKey: true });

  expect(onshowdiff).not.toHaveBeenCalled();
});

test("group_by_directory_with_the_shortcut_and_it_is_remembered", async () => {
  mockCommands({ repo_status: () => [change("src/a/b.txt")] });
  await open();
  expect(screen.queryByRole("treeitem", { name: /^src\/a/ })).toBeTruthy();

  await fireEvent.keyDown(window, { key: "p", code: "KeyP", ctrlKey: true });

  expect(screen.queryByRole("treeitem", { name: /^src\/a/ })).toBeNull();
  cleanup();
  await open();
  expect(screen.queryByRole("treeitem", { name: /^src\/a/ })).toBeNull();
});

test("message_history", async () => {
  mockCommands({ messages_list: () => ["first", "second"] });
  await open();

  await fireEvent.keyDown(window, { key: "m", code: "KeyM", ctrlKey: true });
  const menu = await screen.findByRole("menu", { name: "Commit Message History" });
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "second" }));

  expect((messageField() as HTMLTextAreaElement).value).toBe("second");
});

test("amend_loads_the_message_and_restores_it_when_removed", async () => {
  mockCommands({ head_message: () => "earlier" });
  await open();
  await fireEvent.input(messageField(), { target: { value: "draft" } });

  await fireEvent.keyDown(window, { key: "µ", code: "KeyM", ctrlKey: true, altKey: true });

  await waitFor(() => expect((messageField() as HTMLTextAreaElement).value).toBe("earlier"));
  expect(screen.getByRole("button", { name: "Amend Commit" })).toBeTruthy();

  await fireEvent.click(screen.getByRole("checkbox", { name: "Amend" }));

  await waitFor(() => expect((messageField() as HTMLTextAreaElement).value).toBe("draft"));
});

test("without_commits_there_is_no_amend", async () => {
  mockCommands();
  await open({ repo: { ...REPO, head: null } });

  expect(screen.getByRole("checkbox", { name: "Amend" })).toHaveProperty("disabled", true);
});

test("commit_options", async () => {
  mockCommands({ commit_changes: () => "hash" });
  await open();

  await fireEvent.click(screen.getByRole("button", { name: "Show Commit Options" }));
  const dialog = await screen.findByRole("dialog", { name: "Commit Options" });
  await fireEvent.input(within(dialog).getByLabelText("Author:"), {
    target: { value: "Ana <ana@example.com>" },
  });
  await fireEvent.click(within(dialog).getByRole("checkbox", { name: "Sign-off commit" }));
  await fireEvent.click(within(dialog).getByRole("checkbox", { name: "Run Git hooks" }));
  await fireEvent.keyDown(dialog, { key: "Escape" });

  await fireEvent.input(messageField(), { target: { value: "message" } });
  await fireEvent.click(commitButton());

  await waitFor(() => expect(calls("commit_changes")).toHaveLength(1));
  const { request } = calls("commit_changes")[0][1] as { request: { options: unknown } };
  expect(request.options).toEqual({
    amend: false,
    signOff: true,
    skipHooks: true,
    author: "Ana <ana@example.com>",
    cleanup: false,
  });
});

test("recovery_notices", async () => {
  mockCommands();
  await open({
    repo: {
      ...REPO,
      recovery: { restored: [], skipped: ["100644 abc a.txt"], pending: true, error: "broken" },
    },
  });

  const banner = screen.getByRole("alert");
  expect(banner.textContent).toContain("100644 abc a.txt");
  expect(banner.textContent).toContain("There is another operation in progress");
  expect(banner.textContent).toContain("broken");
});

test("underlines_what_goes_past_the_margin", async () => {
  mockCommands();
  await open();

  await fireEvent.input(messageField(), { target: { value: "a".repeat(75) } });

  expect(screen.getByTestId("message-issue").textContent).toBe("aaa");
  expect(messageField().getAttribute("title")).toBe("Subject cannot exceed 72 characters");
});

test("with_5000_files_only_the_visible_rows_are_painted", async () => {
  const paths = Array.from({ length: 5000 }, (_, index) => `dir/file${index}.txt`);
  mockCommands({
    repo_status: () => paths.map((path) => change(path)),
    changelists_get: () => ({ ...LISTS, assignments: {} }),
  });
  await open();

  expect(screen.getAllByRole("treeitem").length).toBeLessThan(80);
  expect(screen.getByRole("tree").querySelector<HTMLElement>("[data-content]")?.style.height).toBe(
    `${(5000 + 3) * 24 + 8}px`,
  );
});

test("repo_changed_of_a_closed_view_does_nothing", async () => {
  mockCommands();
  const { unmount } = await open();
  const before = calls("repo_status").length;

  unmount();
  repoChanged();

  expect(calls("repo_status")).toHaveLength(before);
});

test("a_failure_reading_the_state_does_not_erase_the_tree", async () => {
  mockCommands();
  await open();
  mockCommands({
    repo_status: () => {
      throw { kind: "failed", message: "git failed" };
    },
  });

  repoChanged();

  expect(await screen.findByText("git failed")).toBeTruthy();
  expect(row("a.txt")).toBeTruthy();
});

test("restore_saved_snapshots", async () => {
  mockCommands({
    snapshots_list: () => [
      { id: "s1", createdMs: 1_700_000_000_000, reason: "rollback", entries: [] },
    ],
    snapshots_restore: () => "s2",
  });
  await open();

  const menu = await contextMenu("a.txt");
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Restore Snapshot…" }));
  const dialog = await screen.findByRole("dialog", { name: "Saved Snapshots" });
  await fireEvent.click(await within(dialog).findByRole("option", { name: /rollback/ }));
  await fireEvent.click(within(dialog).getByRole("button", { name: "Restore" }));

  await waitFor(() => expect(calls("snapshots_restore")).toHaveLength(1));
  expect(calls("snapshots_restore")[0][1]).toEqual({ id: "s1", expectedRepo: "/repo" });
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => (resolve = done));
  return { promise, resolve };
}

// A browser, unless `mousedown` prevents it, focuses the first focusable ancestor of the click.
async function pressLikeBrowser(target: HTMLElement) {
  if (await fireEvent.mouseDown(target)) {
    target.closest<HTMLElement>("[tabindex]")?.focus();
  }
}

test("clicking_a_row_leaves_the_focus_on_the_tree", async () => {
  mockCommands();
  await open();
  const tree = screen.getByRole("tree");
  messageField().focus();

  const browserMayMoveFocus = await fireEvent.mouseDown(row("a.txt"));

  expect(browserMayMoveFocus).toBe(false);
  expect(document.activeElement).toBe(tree);
  expect(tree.matches(":focus")).toBe(true);
});

test("clicking_a_checkbox_leaves_the_focus_on_the_tree", async () => {
  mockCommands();
  await open();
  messageField().focus();

  const browserMayMoveFocus = await fireEvent.mouseDown(checkOf("a.txt"));

  expect(browserMayMoveFocus).toBe(false);
  expect(document.activeElement).toBe(screen.getByRole("tree"));
});

test("clicking_the_empty_space_of_the_tree_also_gives_it_the_focus", async () => {
  mockCommands();
  await open();
  const tree = screen.getByRole("tree");
  messageField().focus();

  await fireEvent.mouseDown(tree);

  expect(document.activeElement).toBe(tree);
});

test("neither_rows_nor_checkboxes_can_have_the_focus", async () => {
  mockCommands();
  await open();
  const tree = screen.getByRole("tree");
  tree.focus();

  row("a.txt").focus();
  expect(document.activeElement).toBe(tree);

  checkOf("a.txt").focus();
  expect(document.activeElement).toBe(tree);
});

test("the_focus_survives_the_row_leaving_the_virtual_window", async () => {
  const paths = Array.from({ length: 200 }, (_, index) => `file${index}.txt`);
  mockCommands({
    repo_status: () => paths.map((path) => change(path)),
    changelists_get: () => ({ ...LISTS, assignments: {} }),
  });
  await open();
  const tree = screen.getByRole("tree");
  await pressLikeBrowser(row("file0.txt"));
  await fireEvent.click(row("file0.txt"));

  for (let press = 0; press < 150; press++) {
    await fireEvent.keyDown(document.activeElement ?? window, { key: "ArrowDown" });
  }

  expect(document.activeElement).toBe(tree);
  expect(tree.getAttribute("aria-activedescendant")).toBe("row-file:file150.txt");
});

test("if_the_chosen_row_disappears_the_one_that_takes_its_place_is_chosen", async () => {
  mockCommands();
  await open();
  await fireEvent.click(row("a.txt"));

  mockCommands({ repo_status: () => [change("b.txt"), change("c.txt")] });
  repoChanged();

  await waitFor(() => expect(screen.queryByRole("treeitem", { name: "a.txt" })).toBeNull());
  await waitFor(() => expect(row("b.txt").getAttribute("aria-selected")).toBe("true"));
  expect(screen.getByRole("tree").querySelectorAll("[aria-selected=true]")).toHaveLength(1);
});

test("a_repo_changed_before_the_commit_ends_does_not_change_what_is_counted", async () => {
  const pending = deferred<string>();
  mockCommands({
    repo_status: () => [change("c.txt")],
    changelists_get: () => ({ ...LISTS, assignments: { "c.txt": "other" } }),
  });
  await open();
  await fireEvent.click(checkOf(/^Other/));
  await fireEvent.input(messageField(), { target: { value: "message" } });
  mockCommands({
    repo_status: () => [],
    changelists_get: () => ({ ...LISTS, assignments: {} }),
    commit_changes: () => pending.promise,
    changelists_remove: () => LISTS,
  });

  await fireEvent.click(commitButton());
  repoChanged();
  await waitFor(() => expect(screen.queryByRole("treeitem", { name: "c.txt" })).toBeNull());
  pending.resolve("hash");

  expect(await screen.findByText("1 file committed: message")).toBeTruthy();
  expect(await screen.findByRole("dialog", { name: "Remove Empty Changelist" })).toBeTruthy();
});

test("the_message_cannot_be_edited_while_the_commit_is_made", async () => {
  const pending = deferred<string>();
  mockCommands({ commit_changes: () => pending.promise });
  await open();
  await fireEvent.input(messageField(), { target: { value: "message" } });

  await fireEvent.click(commitButton());

  await waitFor(() => expect(messageField()).toHaveProperty("readOnly", true));
  pending.resolve("hash");
  await waitFor(() => expect(messageField()).toHaveProperty("readOnly", false));
});

test("the_notice_counts_the_committed_text_even_if_the_field_changes_during_the_commit", async () => {
  const pending = deferred<string>();
  mockCommands({
    commit_changes: () => pending.promise,
    messages_list: () => ["from history"],
  });
  await open();
  await fireEvent.input(messageField(), { target: { value: "confirmed" } });

  await fireEvent.click(commitButton());
  await fireEvent.keyDown(window, { key: "m", code: "KeyM", ctrlKey: true });
  await fireEvent.keyDown(window, { key: "µ", code: "KeyM", ctrlKey: true, altKey: true });
  await new Promise((resolve) => setTimeout(resolve));
  expect(screen.queryByRole("menu")).toBeNull();
  pending.resolve("hash");

  expect(await screen.findByText("2 files committed: confirmed")).toBeTruthy();
  expect(screen.getByRole("button", { name: "Commit" })).toBeTruthy();
});

test("each_rollback_restores_its_own_snapshot", async () => {
  let snapshot = 0;
  mockCommands({
    rollback_files: () => `snapshot-${++snapshot}`,
    snapshots_restore: () => "previous",
  });
  await open();

  for (const name of ["a.txt", "b.txt"]) {
    await fireEvent.click(row(name));
    await fireEvent.click(screen.getByRole("button", { name: "Rollback…" }));
    const dialog = await screen.findByRole("dialog", { name: "Rollback Changes" });
    await fireEvent.click(within(dialog).getByRole("button", { name: "Rollback" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    await waitFor(() => expect(calls("rollback_files").length).toBe(snapshot));
    await screen.findByRole("button", { name: "Restore" });
  }
  await waitFor(() => expect(calls("rollback_files")).toHaveLength(2));
  await waitFor(() => expect(snapshot).toBe(2));

  const restores = screen.getAllByRole("button", { name: "Restore" });
  expect(restores).toHaveLength(1);
  await fireEvent.click(restores[0]);

  await waitFor(() => expect(calls("snapshots_restore")).toHaveLength(1));
  expect(calls("snapshots_restore")[0][1]).toEqual({ id: "snapshot-2", expectedRepo: "/repo" });
});

test("a_repeated_notice_without_action_still_appears_only_once", async () => {
  mockCommands();
  await open();
  mockCommands({
    repo_status: () => {
      throw { kind: "failed", message: "git failed" };
    },
  });

  repoChanged();
  await screen.findByText("git failed");
  repoChanged();
  await new Promise((resolve) => setTimeout(resolve));

  expect(screen.getAllByText("git failed")).toHaveLength(1);
});

test("the_warnings_show_up_before_the_commit_and_it_can_be_cancelled", async () => {
  mockCommands({
    commit_warnings: () => [{ kind: "detachedHead" }],
    commit_changes: () => "hash",
  });
  await open();
  await fireEvent.click(checkOf(/^a\.txt/));
  await fireEvent.input(messageField(), { target: { value: "message" } });

  await fireEvent.click(commitButton());
  const dialog = await screen.findByRole("dialog", { name: "Commit in Detached HEAD" });
  expect(calls("commit_changes")).toHaveLength(0);
  await fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));

  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(calls("commit_changes")).toHaveLength(0);

  await fireEvent.click(commitButton());
  const again = await screen.findByRole("dialog", { name: "Commit in Detached HEAD" });
  await fireEvent.click(within(again).getByRole("button", { name: "Commit" }));

  await waitFor(() => expect(calls("commit_changes")).toHaveLength(1));
});

test("the_warnings_receive_the_same_as_the_commit", async () => {
  mockCommands({ commit_warnings: () => [], commit_changes: () => "hash" });
  await open();
  await fireEvent.input(messageField(), { target: { value: "message" } });

  await fireEvent.click(commitButton());

  await waitFor(() => expect(calls("commit_changes")).toHaveLength(1));
  const [, sent] = calls("commit_warnings")[0] as [string, { request: unknown; selections: [] }];
  expect(sent.selections).toEqual([]);
  expect(sent.request).toEqual((calls("commit_changes")[0][1] as { request: unknown }).request);
});

test("⌥⌘Z opens the rollback and ⌥⌘A does nothing outside the staging area", async () => {
  mockCommands();
  await open();

  await fireEvent.keyDown(window, { key: "å", code: "KeyA", metaKey: true, altKey: true });
  expect(screen.queryByRole("dialog")).toBeNull();

  await fireEvent.keyDown(window, { key: "z", code: "KeyZ", metaKey: true, altKey: true });

  expect(await screen.findByRole("dialog", { name: "Rollback Changes" })).toBeTruthy();
});

// Conflicts: a task whose result arrives through `task-changed`.
let nextTask = 500;

function task(value: unknown = null) {
  return () => {
    const id = nextTask++;
    setTimeout(() =>
      listeners.get("task-changed")?.({
        id,
        title: "",
        text: "",
        fraction: null,
        state: "done",
        kind: null,
        value,
      }),
    );
    return id;
  };
}

const conflictEntry = (
  path: string,
  yours: Conflict["yours"] = "modified",
  theirs: Conflict["theirs"] = "modified",
): Conflict => ({ path, yours, theirs, kind: "both modified" });

const conflictSession = (...conflicts: Conflict[]): ConflictSession => ({
  conflicts,
  reversed: false,
  yoursColumn: "Yours (main)",
  theirsColumn: "Theirs (other)",
  description: "",
});

const CONFLICTED = {
  repo_status: () => [change("a.txt"), change("c.txt", "unmerged")],
  changelists_get: () => ({ ...LISTS, assignments: { "a.txt": "default" } }),
};

test("merge_conflicts_node_with_resolve_in_changelists", async () => {
  mockCommands(CONFLICTED);
  const onresolveconflicts = vi.fn();
  render(CommitView, { repo: REPO, active: true, onresolveconflicts });
  await screen.findByRole("treeitem", { name: "Merge Conflicts" });

  const roots = screen.getAllByRole("treeitem").filter((item) => item.getAttribute("aria-level") === "1");
  expect(roots.map((item) => item.getAttribute("aria-label"))).toEqual([
    "Merge Conflicts",
    "Changes 1 file",
    "Other",
  ]);
  const node = row("Merge Conflicts");
  expect(within(node).queryByRole("checkbox")).toBeNull();
  expect(node.textContent).not.toContain("file");
  expect(within(row("c.txt")).queryByRole("checkbox")).toBeNull();
  // The conflict is not in the list: the active one carries only a.txt.
  expect(row(/^Changes/).textContent).toContain("1 file");

  await fireEvent.click(within(node).getByRole("button", { name: "Resolve" }));

  expect(onresolveconflicts).toHaveBeenCalledTimes(1);
});

test("without_conflicts_there_is_no_merge_conflicts_node", async () => {
  mockCommands();
  await open();

  expect(screen.queryByRole("treeitem", { name: "Merge Conflicts" })).toBeNull();
  expect(screen.queryByRole("button", { name: "Resolve" })).toBeNull();
});

test("context_menu_of_a_conflict_starts_with_merge_accept_theirs_accept_yours", async () => {
  mockCommands(CONFLICTED);
  await open();

  const menu = await contextMenu("c.txt");

  const items = within(menu).getAllByRole("menuitem");
  expect(items.slice(0, 3).map((item) => item.textContent?.trim())).toEqual([
    "Merge…",
    "Accept Theirs",
    "Accept Yours",
  ]);
  expect(within(menu).getByRole("menuitem", { name: "Merge…" }).hasAttribute("disabled")).toBe(false);
});

test("a_normal_file_offers_neither_merge_nor_accept", async () => {
  mockCommands(CONFLICTED);
  await open();

  const menu = await contextMenu("a.txt");

  expect(within(menu).queryByRole("menuitem", { name: "Merge…" })).toBeNull();
  expect(within(menu).queryByRole("menuitem", { name: "Accept Yours" })).toBeNull();
});

test("accept_yours_and_theirs_from_the_menu_accept_the_chosen_conflicts", async () => {
  await trackTasks();
  mockCommands({ ...CONFLICTED, conflicts_accept: task() });
  await open();

  await fireEvent.click(within(await contextMenu("c.txt")).getByRole("menuitem", { name: "Accept Yours" }));
  await waitFor(() => expect(calls("conflicts_accept")).toHaveLength(1));
  expect(calls("conflicts_accept")[0][1]).toEqual({ paths: ["c.txt"], side: "yours" });

  await fireEvent.click(within(await contextMenu("c.txt")).getByRole("menuitem", { name: "Accept Theirs" }));
  await waitFor(() => expect(calls("conflicts_accept")).toHaveLength(2));
  expect(calls("conflicts_accept")[1][1]).toEqual({ paths: ["c.txt"], side: "theirs" });
  await waitFor(() => expect(calls("repo_status").length).toBeGreaterThan(2));
});

test("merge_from_the_menu_opens_the_merge_window", async () => {
  mockCommands({ ...CONFLICTED, merge_window_open: () => undefined });
  await open();

  await fireEvent.click(within(await contextMenu("c.txt")).getByRole("menuitem", { name: "Merge…" }));

  await waitFor(() => expect(calls("merge_window_open")).toHaveLength(1));
  expect(calls("merge_window_open")[0][1]).toEqual({ path: "c.txt" });
});

test("a_conflict_with_both_sides_deleted_leaves_merge_disabled", async () => {
  await trackTasks();
  mockCommands({
    repo_status: () => [change("c.txt", "unmerged"), change("d.txt", "unmerged")],
    conflicts_session: task(
      conflictSession(conflictEntry("c.txt", "deleted", "deleted"), conflictEntry("d.txt")),
    ),
  });
  await open();
  await waitFor(() => expect(calls("conflicts_session")).toHaveLength(1));
  // A moment for the conflict read to arrive.
  await new Promise((resolve) => setTimeout(resolve, 10));

  const only = await contextMenu("c.txt");
  expect(within(only).getByRole("menuitem", { name: "Merge…" }).hasAttribute("disabled")).toBe(true);
  expect(within(only).getByRole("menuitem", { name: "Accept Yours" }).hasAttribute("disabled")).toBe(false);
  await fireEvent.keyDown(only, { key: "Escape" });
  await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());

  const other = await contextMenu("d.txt");
  expect(within(other).getByRole("menuitem", { name: "Merge…" }).hasAttribute("disabled")).toBe(false);
});

test("commit_view_proposes_the_merge_message", async () => {
  mockCommands({ merge_message: () => "Merge branch 'other'\n\n# Conflicts:\n#\ta.txt\n" });
  await open();

  await waitFor(() =>
    expect((messageField() as HTMLTextAreaElement).value).toBe(
      "Merge branch 'other'\n\n# Conflicts:\n#\ta.txt\n",
    ),
  );
});

test("the_merge_message_does_not_overwrite_what_was_typed_nor_come_back_after_deleting_it", async () => {
  mockCommands({ merge_message: () => "Merge branch 'other'\n" });
  await open();
  await waitFor(() => expect((messageField() as HTMLTextAreaElement).value).not.toBe(""));

  await fireEvent.input(messageField(), { target: { value: "" } });
  repoChanged();
  await waitFor(() => expect(calls("repo_status").length).toBeGreaterThan(1));
  await new Promise((resolve) => setTimeout(resolve, 10));
  expect((messageField() as HTMLTextAreaElement).value).toBe("");

  await fireEvent.input(messageField(), { target: { value: "mine" } });
  mockCommands({ merge_message: () => "Merge branch 'new'\n" });
  repoChanged();
  await new Promise((resolve) => setTimeout(resolve, 20));
  expect((messageField() as HTMLTextAreaElement).value).toBe("mine");
});

test("without_a_merge_message_the_box_stays_empty", async () => {
  mockCommands({ merge_message: () => null });
  await open();

  await waitFor(() => expect(calls("merge_message").length).toBeGreaterThan(0));
  expect((messageField() as HTMLTextAreaElement).value).toBe("");
});

test("tells_whoever_uses_it_whether_there_are_conflicts", async () => {
  mockCommands(CONFLICTED);
  const onconflictschange = vi.fn();
  render(CommitView, { repo: REPO, active: true, onconflictschange });

  await waitFor(() => expect(onconflictschange).toHaveBeenLastCalledWith(true));

  mockCommands();
  repoChanged();
  await waitFor(() => expect(onconflictschange).toHaveBeenLastCalledWith(false));
});

const HEAD = REPO.state.head;
const proposal = (overrides: Record<string, unknown> = {}) => ({
  message: "fixup! one",
  rebaseFrom: "abc~",
  head: HEAD,
  ...overrides,
});
const rebaseButton = () => screen.queryByRole("button", { name: "Commit and Rebase…" });
const text = () => (messageField() as HTMLTextAreaElement).value;
const typed = (value: string) => fireEvent.input(messageField(), { target: { value } });

test("commit_and_rebase_commits_and_then_rebases", async () => {
  const order: string[] = [];
  const onrebase = vi.fn(async (base: string) => {
    order.push(`rebase ${base}`);
  });
  mockCommands({
    commit_changes: () => {
      order.push("commit");
      return "hash";
    },
  });
  const view = await open({ onrebase });
  // Without Fixup… or Squash Into… there is no executor.
  expect(rebaseButton()).toBeNull();

  view.component.propose(proposal());
  await waitFor(() => expect(text()).toBe("fixup! one"));
  await fireEvent.click(rebaseButton()!);

  await waitFor(() => expect(onrebase).toHaveBeenCalledTimes(1));
  expect(onrebase).toHaveBeenCalledWith("abc~");
  expect(order).toEqual(["commit", "rebase abc~"]);
  expect(calls("commit_changes")[0][1]).toMatchObject({ request: { message: "fixup! one" } });
  // The proposal was used up: the next commit no longer offers the rebase.
  await waitFor(() => expect(rebaseButton()).toBeNull());
});

test("without_a_base_there_is_no_commit_and_rebase", async () => {
  mockCommands();
  const view = await open({ onrebase: vi.fn(async () => {}) });

  view.component.propose(proposal({ message: "squash! two", rebaseFrom: null }));

  await waitFor(() => expect(text()).toBe("squash! two"));
  expect(rebaseButton()).toBeNull();
  expect(commitButton()).toBeTruthy();
});

test("the_proposal_returns_the_draft_when_it_ends", async () => {
  mockCommands({ commit_changes: () => "hash" });
  const view = await open({ onrebase: vi.fn(async () => {}) });
  await typed("my draft");

  view.component.propose(proposal());
  await waitFor(() => expect(text()).toBe("fixup! one"));
  await fireEvent.click(commitButton());

  // Like IntelliJ's modal dialog: the draft of the Commit view stays where it was.
  await waitFor(() => expect(calls("commit_changes")).toHaveLength(1));
  expect(calls("commit_changes")[0][1]).toMatchObject({ request: { message: "fixup! one" } });
  await waitFor(() => expect(text()).toBe("my draft"));
  expect(rebaseButton()).toBeNull();
});

test("the_proposal_returns_the_amend_draft", async () => {
  mockCommands({ head_message: () => "HEAD message" });
  const view = await open({ onrebase: vi.fn(async () => {}) });
  await typed("my draft");
  await fireEvent.click(screen.getByRole("checkbox", { name: "Amend" }));
  await waitFor(() => expect(text()).toBe("HEAD message"));

  view.component.propose(proposal());
  await waitFor(() => expect(text()).toBe("fixup! one"));
  expect((screen.getByRole("checkbox", { name: "Amend" }) as HTMLInputElement).checked).toBe(false);
  // Without a commit, HEAD stays the same: everything comes back, Amend included.
  await typed("");

  await waitFor(() => expect(text()).toBe("HEAD message"));
  expect((screen.getByRole("checkbox", { name: "Amend" }) as HTMLInputElement).checked).toBe(true);
  await fireEvent.click(screen.getByRole("checkbox", { name: "Amend" }));
  expect(text()).toBe("my draft");
});

test("after_the_commit_amend_does_not_come_back_and_the_previous_draft_remains", async () => {
  mockCommands({ commit_changes: () => "hash", head_message: () => "HEAD message" });
  const view = await open({ onrebase: vi.fn(async () => {}) });
  await typed("my draft");
  await fireEvent.click(screen.getByRole("checkbox", { name: "Amend" }));
  await waitFor(() => expect(text()).toBe("HEAD message"));

  view.component.propose(proposal());
  await waitFor(() => expect(text()).toBe("fixup! one"));
  await fireEvent.click(commitButton());

  // HEAD moved: amending now would change the new commit with the message of the earlier HEAD.
  await waitFor(() => expect(text()).toBe("my draft"));
  expect((screen.getByRole("checkbox", { name: "Amend" }) as HTMLInputElement).checked).toBe(false);
  expect(calls("commit_changes")[0][1]).toMatchObject({
    request: { message: "fixup! one", options: { amend: false } },
  });
});

// Final review: closing the window during a Fixup… or Squash Into… keeps the user's own draft as
// the changelist comment, not the proposed `fixup! …`, which comes back nowhere else.
test("closing_during_a_proposal_keeps_the_prior_draft", async () => {
  mockCommands();
  const view = await open({ onrebase: vi.fn(async () => {}) });
  await typed("my draft");
  view.component.propose(proposal());
  await waitFor(() => expect(text()).toBe("fixup! one"));

  const draft = view.component.changelistDraft();

  expect(draft?.comment).toBe("my draft");
});

test("deleting_the_proposed_message_abandons_the_proposal_and_returns_the_draft", async () => {
  mockCommands();
  const view = await open({ onrebase: vi.fn(async () => {}) });
  await typed("my draft");
  view.component.propose(proposal());
  await waitFor(() => expect(rebaseButton()).not.toBeNull());

  await typed("");

  await waitFor(() => expect(text()).toBe("my draft"));
  expect(rebaseButton()).toBeNull();
});

test("fixup_includes_only_the_default_list", async () => {
  mockCommands({ commit_changes: () => "hash" });
  const view = await open({ onrebase: vi.fn(async () => {}) });
  // The user had another file checked and one of the default list unchecked.
  await fireEvent.click(checkOf("c.txt"));
  await fireEvent.click(checkOf("a.txt"));
  expect(checkOf("a.txt").getAttribute("aria-checked")).toBe("false");

  view.component.propose(proposal());

  await waitFor(() => expect(checkOf("a.txt").getAttribute("aria-checked")).toBe("true"));
  expect(checkOf("b.txt").getAttribute("aria-checked")).toBe("true");
  expect(checkOf("c.txt").getAttribute("aria-checked")).toBe("false");
  await fireEvent.click(commitButton());
  await waitFor(() => expect(calls("commit_changes")).toHaveLength(1));
  expect(calls("commit_changes")[0][1]).toMatchObject({
    request: { added: ["a.txt", "b.txt"], message: "fixup! one" },
  });

  // When abandoning another proposal, the earlier checkboxes come back.
  mockCommands();
  view.component.propose(proposal());
  await waitFor(() => expect(rebaseButton()).not.toBeNull());
  await typed("");
  await waitFor(() => expect(rebaseButton()).toBeNull());
  expect(checkOf("c.txt").getAttribute("aria-checked")).toBe("true");
});

test("a_normal_commit_uses_up_the_proposal_and_does_not_rebase", async () => {
  const onrebase = vi.fn(async () => {});
  mockCommands({ commit_changes: () => "hash" });
  const view = await open({ onrebase });
  view.component.propose(proposal());
  await waitFor(() => expect(rebaseButton()).not.toBeNull());

  await fireEvent.click(commitButton());

  await waitFor(() => expect(calls("commit_changes")).toHaveLength(1));
  await waitFor(() => expect(rebaseButton()).toBeNull());
  expect(onrebase).not.toHaveBeenCalled();
});

test("a_failing_commit_does_not_rebase_and_the_proposal_stays", async () => {
  const onrebase = vi.fn(async () => {});
  mockCommands({
    commit_changes: () => {
      throw { kind: "hook_failed", message: "A hook rejected the commit: no" };
    },
  });
  const view = await open({ onrebase });
  view.component.propose(proposal());
  await waitFor(() => expect(rebaseButton()).not.toBeNull());

  await fireEvent.click(rebaseButton()!);

  expect(await screen.findByRole("alert")).toBeTruthy();
  expect(onrebase).not.toHaveBeenCalled();
  expect(rebaseButton()).not.toBeNull();
  expect(text()).toBe("fixup! one");
});

test("commit_and_rebase_with_an_operation_in_progress_or_a_moved_head_does_not_commit_and_returns_the_draft", async () => {
  for (const state of [
    { ...REPO.state, operation: "merging" },
    { ...REPO.state, head: "otrohead" },
  ]) {
    cleanup();
    const onrebase = vi.fn(async () => {});
    mockCommands({ commit_changes: () => "hash" });
    const view = await open({ onrebase });
    await typed("my draft");
    view.component.propose(proposal());
    await waitFor(() => expect(rebaseButton()).not.toBeNull());

    // Between Fixup… and the click the repository changed.
    mockCommands({ repo_state: () => state, commit_changes: () => "hash" });
    await fireEvent.click(rebaseButton()!);

    await waitFor(() => expect(rebaseButton()).toBeNull());
    expect(calls("commit_changes")).toHaveLength(0);
    expect(onrebase).not.toHaveBeenCalled();
    expect(text()).toBe("my draft");
    expect(await screen.findByText(/Commit and Rebase…/)).toBeTruthy();
  }
});

test("with_head_moved_from_outside_amend_does_not_come_back_and_the_previous_draft_remains", async () => {
  // Via `repo-changed` and when pressing Commit and Rebase…
  for (const via of ["repoChanged", "click"]) {
    cleanup();
    mockCommands({ commit_changes: () => "hash", head_message: () => "HEAD message" });
    const view = await open({ onrebase: vi.fn(async () => {}) });
    await typed("my draft");
    await fireEvent.click(screen.getByRole("checkbox", { name: "Amend" }));
    await waitFor(() => expect(text()).toBe("HEAD message"));
    view.component.propose(proposal());
    await waitFor(() => expect(rebaseButton()).not.toBeNull());

    mockCommands({
      repo_state: () => ({ ...REPO.state, head: "otrohead" }),
      commit_changes: () => "hash",
      head_message: () => "new HEAD message",
    });
    if (via === "repoChanged") repoChanged();
    else await fireEvent.click(rebaseButton()!);

    // Amending now would change the new HEAD with the message of the earlier one.
    await waitFor(() => expect(rebaseButton()).toBeNull());
    expect(text()).toBe("my draft");
    const amend = screen.getByRole("checkbox", { name: "Amend" }) as HTMLInputElement;
    expect(amend.checked, via).toBe(false);
    expect(calls("commit_changes")).toHaveLength(0);
  }
});

test("with_an_operation_in_progress_and_the_same_head_amend_comes_back", async () => {
  mockCommands({ head_message: () => "HEAD message" });
  const view = await open({ onrebase: vi.fn(async () => {}) });
  await typed("my draft");
  await fireEvent.click(screen.getByRole("checkbox", { name: "Amend" }));
  await waitFor(() => expect(text()).toBe("HEAD message"));
  view.component.propose(proposal());
  await waitFor(() => expect(rebaseButton()).not.toBeNull());

  mockCommands({
    repo_state: () => ({ ...REPO.state, operation: "merging" }),
    head_message: () => "HEAD message",
  });
  repoChanged();

  await waitFor(() => expect(rebaseButton()).toBeNull());
  expect(text()).toBe("HEAD message");
  expect((screen.getByRole("checkbox", { name: "Amend" }) as HTMLInputElement).checked).toBe(true);
});

test("head_moving_while_a_warning_is_open_stops_the_commit_and_the_rebase", async () => {
  let reads = 0;
  mockCommands({
    commit_warnings: () => [{ kind: "detachedHead" }],
    commit_changes: () => "hash",
    // Fine when the button is pressed, moved by the time the warning is confirmed.
    repo_state: () => (++reads === 1 ? REPO.state : { ...REPO.state, head: "otrohead" }),
  });
  const onrebase = vi.fn(async () => {});
  const view = await open({ onrebase });
  await typed("my draft");
  view.component.propose(proposal());
  await waitFor(() => expect(rebaseButton()).not.toBeNull());

  await fireEvent.click(rebaseButton()!);
  const warning = await screen.findByRole("dialog", { name: "Commit in Detached HEAD" });
  await fireEvent.click(within(warning).getByRole("button", { name: "Commit and Rebase" }));

  await waitFor(() => expect(rebaseButton()).toBeNull());
  expect(reads).toBe(2);
  expect(calls("commit_changes")).toHaveLength(0);
  expect(onrebase).not.toHaveBeenCalled();
  expect(text()).toBe("my draft");
});

test("a_repo_changed_that_leaves_the_repository_in_another_state_removes_the_executor", async () => {
  mockCommands();
  const view = await open({ onrebase: vi.fn(async () => {}) });
  await typed("my draft");
  view.component.propose(proposal());
  await waitFor(() => expect(rebaseButton()).not.toBeNull());

  mockCommands({ repo_state: () => ({ ...REPO.state, operation: "rebasing" }) });
  repoChanged();

  await waitFor(() => expect(rebaseButton()).toBeNull());
  expect(text()).toBe("my draft");
});

test("the_proposal_coexists_with_the_merge_message_and_it_does_not_come_back", async () => {
  mockCommands({ merge_message: () => "Merge branch 'other'" });
  const view = await open({ onrebase: vi.fn(async () => {}) });
  await waitFor(() => expect(text()).toBe("Merge branch 'other'"));

  view.component.propose(proposal());
  await waitFor(() => expect(text()).toBe("fixup! one"));
  repoChanged();
  await new Promise((resolve) => setTimeout(resolve, 20));

  // It is an explicit action: it wins over the proposed one, and what was proposed is not offered again.
  expect(text()).toBe("fixup! one");
});

test("sign-off is seeded and saved under the key the Commit Changes dialog uses", async () => {
  localStorage.setItem(signOffKey("/repo"), "true");
  mockCommands({ commit_changes: () => "hash" });
  await open();

  await fireEvent.click(screen.getByRole("button", { name: "Show Commit Options" }));
  const signOff = screen.getByRole("checkbox", { name: "Sign-off commit" }) as HTMLInputElement;
  expect(signOff.checked).toBe(true);
  await fireEvent.click(signOff);
  await fireEvent.keyDown(signOff, { key: "Escape" });
  await fireEvent.input(messageField(), { target: { value: "m" } });
  await fireEvent.click(commitButton());

  await waitFor(() => expect(calls("commit_changes")).toHaveLength(1));
  expect(calls("commit_changes")[0][1]).toMatchObject({ request: { options: { signOff: false } } });
  expect(localStorage.getItem(signOffKey("/repo"))).toBe("false");
});

test("a file deleted while a warning is open is not sent as it was", async () => {
  mockCommands({ commit_warnings: () => [{ kind: "detachedHead" }], commit_changes: () => "hash" });
  await open();
  await fireEvent.input(messageField(), { target: { value: "m" } });
  await fireEvent.click(commitButton());
  const warning = await screen.findByRole("dialog", { name: "Commit in Detached HEAD" });

  mockCommands({
    commit_warnings: () => [{ kind: "detachedHead" }],
    commit_changes: () => "hash",
    repo_status: () => [change("a.txt"), change("b.txt", "deleted"), change("c.txt")],
  });
  repoChanged();
  await waitFor(() => expect(calls("repo_status").length).toBeGreaterThan(1));
  await waitFor(() => expect(screen.getByText("1 deleted")).toBeTruthy());
  await fireEvent.click(within(warning).getByRole("button", { name: "Commit" }));

  await waitFor(() => expect(calls("commit_changes")).toHaveLength(1));
  expect(calls("commit_changes")[0][1]).toMatchObject({
    request: { added: ["a.txt"], removed: ["b.txt"] },
  });
});

test("with every file gone while a warning is open nothing is committed", async () => {
  mockCommands({ commit_warnings: () => [{ kind: "detachedHead" }], commit_changes: () => "hash" });
  await open();
  await fireEvent.input(messageField(), { target: { value: "m" } });
  await fireEvent.click(commitButton());
  const warning = await screen.findByRole("dialog", { name: "Commit in Detached HEAD" });

  mockCommands({
    commit_warnings: () => [{ kind: "detachedHead" }],
    commit_changes: () => "hash",
    repo_status: () => [],
  });
  repoChanged();
  await waitFor(() => expect(calls("repo_status").length).toBeGreaterThan(1));
  await waitFor(() => expect(screen.queryByRole("treeitem", { name: /a\.txt/ })).toBeNull());
  await fireEvent.click(within(warning).getByRole("button", { name: "Commit" }));

  await waitFor(() => expect(screen.queryByRole("dialog", { name: "Commit in Detached HEAD" })).toBeNull());
  await new Promise((resolve) => setTimeout(resolve, 20));
  expect(calls("commit_changes")).toHaveLength(0);
  expect(calls("commit_changes_selected")).toHaveLength(0);
});

// `ChangeListCommitMessagePolicy`: the message starts from the active changelist's comment, where
// a closed window left its draft, and again when the active list changes while it is empty.
test("the message starts from the active changelist's comment", async () => {
  mockCommands({
    changelists_get: () => ({
      ...LISTS,
      lists: [{ ...LISTS.lists[0], comment: "kept draft" }, LISTS.lists[1]],
    }),
  });
  await open();

  await vi.waitFor(() => expect((messageField() as HTMLTextAreaElement).value).toBe("kept draft"));
});

test("another active list brings its comment only into an empty message", async () => {
  let lists: Changelists = LISTS;
  mockCommands({ changelists_get: () => lists });
  await open();
  expect((messageField() as HTMLTextAreaElement).value).toBe("");

  lists = {
    ...LISTS,
    active: "other",
    lists: [LISTS.lists[0], { ...LISTS.lists[1], comment: "other draft" }],
  };
  repoChanged();
  await vi.waitFor(() => expect((messageField() as HTMLTextAreaElement).value).toBe("other draft"));

  await fireEvent.input(messageField(), { target: { value: "typed" } });
  lists = {
    ...LISTS,
    lists: [{ ...LISTS.lists[0], comment: "default draft" }, LISTS.lists[1]],
  };
  repoChanged();
  await vi.waitFor(() => expect(calls("changelists_get").length).toBeGreaterThan(2));
  await new Promise((resolve) => setTimeout(resolve, 20));
  expect((messageField() as HTMLTextAreaElement).value).toBe("typed");
});


// Ramify's own `Start the commit message with the ticket from the branch name`.
test("the_ticket_starts_the_message_follows_a_checkout_and_alone_is_no_message", async () => {
  mockCommands();
  const view = await open({ ticket: "HT-730" });

  await waitFor(() => expect((messageField() as HTMLTextAreaElement).value).toBe("HT-730 "));
  await fireEvent.click(commitButton());
  expect(await screen.findByText("Specify commit message")).toBeTruthy();

  await view.rerender({ ticket: "HT-9" });
  await waitFor(() => expect((messageField() as HTMLTextAreaElement).value).toBe("HT-9 "));
  await fireEvent.input(messageField(), { target: { value: "HT-9 Fix the form" } });
  await view.rerender({ ticket: null });
  expect((messageField() as HTMLTextAreaElement).value).toBe("HT-9 Fix the form");
});
