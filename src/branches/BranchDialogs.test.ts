import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { get } from "svelte/store";
import type { PullInfo, PushInfo, TaskEvent } from "../lib/api";
import { dismiss, notifications } from "../lib/notifications";
import { trackTasks } from "../lib/tasks";
import BranchDialogs from "./BranchDialogs.svelte";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(), emit: vi.fn() }));

const HASH = "a1b2c3d4".repeat(5);
const BRANCHES = { local: [{ name: "main", tracked: null }], remote: [] };
let nextTask = 1;
let publish: (event: TaskEvent) => void;
let repoState: unknown;

// Each `tasks` command finishes right away with the value it is given.
const PULL_INFO: PullInfo = {
  branch: "main",
  remotes: ["origin"],
  remoteBranches: ["origin/main", "origin/other"],
  tracked: ["origin", "main"],
};

const SETTINGS = { updateMethod: "merge", pushAutoUpdate: false, previewPushOnCommit: true };
let settingsNow: Record<string, unknown> = SETTINGS;

function mockCommands(tasks: Record<string, unknown>) {
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "pull_info") return PULL_INFO;
    if (command === "push_info") return PUSH_INFO;
    if (command === "push_branch_of") return "main";
    if (command === "push_tags_get") return null;
    if (command === "push_tags_set") return undefined;
    if (command === "settings_get") return settingsNow;
    if (command === "settings_save") return undefined;
    if (command === "pull_options_get") return [];
    if (command === "pull_options_set") return undefined;
    if (command === "branch_list") return BRANCHES;
    if (command === "branch_current") return "main";
    if (command === "repo_state") return repoState;
    if (command === "repo_settings_get") return { protectedBranches: ["master", "main"] };
    if (command === "commit_details") {
      return [{ hash: HASH, subject: "one", author: "Ana", refs: [] }];
    }
    const id = nextTask++;
    publish({
      id,
      title: command,
      text: "",
      fraction: null,
      state: "done",
      kind: null,
      value: tasks[command],
    });
    return id;
  });
}

beforeEach(async () => {
  settingsNow = SETTINGS;
  vi.mocked(invoke).mockReset();
  vi.mocked(listen).mockImplementation(async (event, handler) => {
    // Only the tasks: the rest (`repo-changed`) must not overwrite `publish`.
    if (event === "task-changed") {
      publish = (payload) => (handler as (event: { payload: TaskEvent }) => void)({ payload });
    }
    return () => {};
  });
  await trackTasks();
});

afterEach(() => {
  cleanup();
  get(notifications).forEach(({ id }) => dismiss(id));
});

const PUSH_INFO: PushInfo = {
  repository: "rama",
  branch: "main",
  head: HASH,
  remotes: ["origin"],
  remoteBranches: ["origin/main"],
  target: { remote: "origin", branch: "main", isNew: false },
  prePushHook: false,
  protectedBranches: [],
  forcePushTargets: [],
};

test("push_opens_the_dialog_and_pushes_the_chosen", async () => {
  mockCommands({
    push_outgoing: [],
    git_push: {
      status: { kind: "success" },
      pushed: 3,
      source: "main",
      destination: "origin/main",
      tags: [],
      remote: "origin",
    },
  });
  const { component } = render(BranchDialogs);

  component.push();

  await fireEvent.click(await screen.findByRole("button", { name: "Push" }));

  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("git_push", {
      spec: {
        source: "main",
        remote: "origin",
        target: "main",
        tags: null,
        force: null,
        noVerify: false,
      },
    }),
  );
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  await waitFor(() =>
    expect(get(notifications).map(({ title }) => title)).toContain(
      "Pushed 3 commits to origin/main",
    ),
  );
  expect(invoke).toHaveBeenCalledWith("push_tags_set", { tags: null });
});

test("define_remote_is_injected_from_outside", async () => {
  mockCommands({ push_outgoing: [] });
  const ondefineremote = vi.fn(async () => false);
  const { component } = render(BranchDialogs, { ondefineremote });

  component.push();
  await fireEvent.click(await screen.findByRole("button", { name: "Remote" }));
  await fireEvent.click(screen.getByRole("menuitem", { name: "Define remote" }));

  await waitFor(() => expect(ondefineremote).toHaveBeenCalled());
});

test("push_from_the_menu_of_a_branch_and_of_a_commit", async () => {
  mockCommands({ push_outgoing: [] });
  const { component } = render(BranchDialogs);

  component.run({ kind: "ref", operation: "push", ref: { name: "other", kind: "local" } });
  await screen.findByRole("dialog", { name: "Push Commits to rama" });
  expect(invoke).toHaveBeenCalledWith("push_info", { branch: "other" });
  await fireEvent.click(screen.getByRole("button", { name: "Cancel" }));

  component.run({ kind: "pushUpTo", hash: HASH });
  await screen.findByRole("dialog", { name: "Push Commits to rama" });
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("push_outgoing", {
      source: HASH,
      remote: "origin",
      target: "main",
    }),
  );
});

test("new_branch_from_the_log_cleans_the_name_and_creates_the_branch", async () => {
  mockCommands({ branch_create: { kind: "done" } });
  const { component } = render(BranchDialogs);

  component.run({ kind: "newBranch", hash: HASH });

  const input = await screen.findByLabelText("New branch name:");
  await fireEvent.input(input, { target: { value: "my branch" } });
  expect((input as HTMLInputElement).value).toBe("my-branch");
  await fireEvent.click(screen.getByRole("button", { name: "Create" }));

  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("branch_create", {
      name: "my-branch",
      startPoint: HASH,
      checkout: true,
      overwrite: "no",
      mode: "plain",
    }),
  );
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
});

test("the_overwrite_checkbox_is_only_enabled_on_conflict", async () => {
  mockCommands({});
  const { component } = render(BranchDialogs);

  component.run({ kind: "newBranch", hash: HASH });

  const input = await screen.findByLabelText("New branch name:");
  const overwrite = screen.getByLabelText("Overwrite existing branch") as HTMLInputElement;
  expect(overwrite.disabled).toBe(true);
  await fireEvent.input(input, { target: { value: "main" } });
  expect(overwrite.disabled).toBe(false);
  expect(screen.getByText(/Change the name or overwrite existing branch/)).toBeTruthy();
  expect((screen.getByRole("button", { name: "Create" }) as HTMLButtonElement).disabled).toBe(true);
});

test("checkout_problem_with_its_three_buttons", async () => {
  mockCommands({ branch_checkout: { kind: "overwritten", files: ["a.txt"] } });
  const { component } = render(BranchDialogs);

  component.run({ kind: "checkout", target: "other" });

  expect(await screen.findByRole("dialog", { name: "Git Checkout Problem" })).toBeTruthy();
  expect(screen.getByText("a.txt")).toBeTruthy();
  expect(
    screen.getByText("Your local changes to the following files would be overwritten by checkout."),
  ).toBeTruthy();
  expect(screen.getByText("Can stash the changes, checkout and unstash them after that.")).toBeTruthy();
  expect(screen.getByRole("button", { name: "Force Checkout" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Smart Checkout" })).toBeTruthy();
  await fireEvent.click(screen.getByRole("button", { name: "Don't Checkout" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(vi.mocked(invoke).mock.calls.filter(([name]) => name === "branch_checkout")).toHaveLength(1);
});

test("the_update_project_dialog_launches_the_update_and_saves_the_method", async () => {
  const settings = { updateMethod: "merge", showUpdateDialog: true };
  const update = {
    outcome: { kind: "nothingToUpdate" },
    title: null,
    files: 0,
    commits: 0,
    range: null,
    saved: null,
    restore: null,
    matching: null,
    inProgress: false,
  };
  mockCommands({ update_project: update });
  const answered = vi.mocked(invoke).getMockImplementation()!;
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "settings_get") return settings;
    if (command === "settings_save") return settings;
    return answered(command, args);
  });
  const { component } = render(BranchDialogs);

  component.runUpdate("Vcs.UpdateProject");

  await fireEvent.click(
    await screen.findByRole("radio", {
      name: "Rebase the current branch on top of incoming changes",
    }),
  );
  await fireEvent.click(screen.getByRole("button", { name: "OK" }));

  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("update_project", { method: "rebase", options: {} }),
  );
  expect(invoke).toHaveBeenCalledWith("settings_save", {
    settings: { updateMethod: "rebase", showUpdateDialog: true },
  });
});

test("reset_hard_goes_straight_on_accept", async () => {
  mockCommands({ branch_reset: { kind: "done" } });
  const { component } = render(BranchDialogs);

  component.run({ kind: "reset", hash: HASH });

  await screen.findByRole("dialog", { name: "Git Reset" });
  await fireEvent.click(screen.getByLabelText(/^Hard/));
  await fireEvent.click(screen.getByRole("button", { name: "Reset" }));

  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("branch_reset", {
      target: HASH,
      mode: "hard",
      how: "plain",
    }),
  );
});

test("pull_opens_its_dialog_and_launches_git_pull_with_the_chosen", async () => {
  mockCommands({ git_pull: { outcome: { kind: "success" }, files: 0, commits: 0 } });
  const { component } = render(BranchDialogs);

  component.pull();

  expect(await screen.findByRole("dialog", { name: "Pull to main" })).toBeTruthy();
  await fireEvent.click(screen.getByRole("button", { name: "Modify options" }));
  await fireEvent.click(screen.getByRole("checkbox", { name: "--no-ff" }));
  await fireEvent.click(screen.getByRole("button", { name: "Pull" }));

  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("git_pull", {
      remote: "origin",
      branch: "main",
      options: ["noFf"],
    }),
  );
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  await waitFor(() =>
    expect(get(notifications).map(({ title }) => title)).toContain("All files are up to date"),
  );
});

test("view_commits_of_the_result_reaches_whoever_opens_the_tab", async () => {
  const range = ["a".repeat(40), "b".repeat(40)];
  mockCommands({
    git_pull: { outcome: { kind: "success" }, files: 2, commits: 1, range, matching: null },
  });
  const onviewcommits = vi.fn();
  const { component } = render(BranchDialogs, { onviewcommits });

  component.pull();
  await fireEvent.click(await screen.findByRole("button", { name: "Pull" }));

  const summary = () => get(notifications).find(({ title }) => title === "2 files updated in 1 commit");
  await waitFor(() => expect(summary()).toBeTruthy());
  summary()?.actions?.find(({ label }) => label === "View Commits")?.run();
  expect(onviewcommits).toHaveBeenCalledWith(range);
});

const PUSHED = {
  status: { kind: "success" },
  pushed: 1,
  source: "main",
  destination: "origin/main",
  tags: [],
  remote: "origin",
  updatable: null,
};

test("a_rejection_asks_and_the_update_goes_through_the_update_barriers", async () => {
  mockCommands({
    push_outgoing: [],
    git_push: {
      ...PUSHED,
      status: { kind: "rejectedNoFf" },
      updatable: { branch: "main", merges: false },
    },
    git_push_update: { push: PUSHED, update: null, attempts: 2 },
  });
  const { component } = render(BranchDialogs);

  component.push();
  await fireEvent.click(await screen.findByRole("button", { name: "Push" }));
  await fireEvent.click(await screen.findByRole("button", { name: "Merge" }));

  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith(
      "git_push_update",
      expect.objectContaining({ method: "merge", attempts: 1 }),
    ),
  );
  await waitFor(() =>
    expect(get(notifications).map(({ title }) => title)).toEqual(["Pushed 1 commit to origin/main"]),
  );
});

test("switching_repository_dismisses_force_push_anyway", async () => {
  mockCommands({
    push_outgoing: [],
    git_push: { ...PUSHED, status: { kind: "rejectedStale" } },
  });
  const { component } = render(BranchDialogs);

  component.push();
  await fireEvent.click(await screen.findByRole("button", { name: "More push options" }));
  await fireEvent.click(await screen.findByRole("menuitem", { name: "Force Push" }));
  await fireEvent.click(
    within(await screen.findByRole("dialog", { name: "Force Push" })).getByRole("button", {
      name: "Force Push",
    }),
  );
  await waitFor(() => expect(get(notifications)).toHaveLength(1));
  expect(get(notifications)[0].actions?.[0].label).toBe("Force push anyway");

  component.repositoryChanged({
    path: "/repo/a",
    pending: [],
    state: { operation: null },
  } as never);

  expect(get(notifications)).toEqual([]);
});

test("switching_repository_dismisses_restore_of_a_deleted_branch", async () => {
  mockCommands({
    branch_delete: { name: "b", tip: HASH, upstream: null, forced: false, base: null },
  });
  const { component } = render(BranchDialogs);

  component.run({ kind: "ref", operation: "delete", ref: { name: "b", kind: "local" } });
  await waitFor(() => expect(get(notifications)).toHaveLength(1));
  expect(get(notifications)[0].actions?.[0].label).toBe("Restore");

  component.repositoryChanged({
    path: "/repo/a",
    pending: [],
    state: { operation: null },
  } as never);

  expect(get(notifications)).toEqual([]);
});

test("the_tag_of_the_git_menu_asks_for_name_commit_and_message", async () => {
  mockCommands({ tag_create: null });
  const { component } = render(BranchDialogs);

  component.tag(null);

  const dialog = await screen.findByRole("dialog", { name: "Tag" });
  await fireEvent.input(within(dialog).getByLabelText("Tag Name:"), { target: { value: "v1" } });
  await fireEvent.click(within(dialog).getByLabelText("Force"));
  await fireEvent.input(within(dialog).getByLabelText("Message:"), { target: { value: "hello" } });
  await fireEvent.click(within(dialog).getByRole("button", { name: "Create Tag" }));

  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("tag_create", {
      name: "v1",
      target: "HEAD",
      message: "hello",
      force: true,
    }),
  );
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
});

test("reset_head_has_the_old_dialog_without_keep", async () => {
  mockCommands({ branch_reset: { kind: "done" } });
  const { component } = render(BranchDialogs);

  component.resetHead();

  const dialog = await screen.findByRole("dialog", { name: "Reset Head" });
  expect(within(dialog).getByText("Current Branch:")).toBeTruthy();
  expect(within(dialog).getByText("main")).toBeTruthy();
  const type = within(dialog).getByLabelText("Reset Type:") as HTMLSelectElement;
  expect([...type.options].map(({ text }) => text)).toEqual(["Mixed", "Soft", "Hard"]);
  expect(type.value).toBe("mixed");
  const commit = within(dialog).getByLabelText("To Commit:") as HTMLInputElement;
  expect(commit.value).toBe("HEAD");

  await fireEvent.change(type, { target: { value: "soft" } });
  await fireEvent.input(commit, { target: { value: " HEAD~1 " } });
  await fireEvent.click(within(dialog).getByRole("button", { name: "Reset" }));

  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("branch_reset", {
      target: "HEAD~1",
      mode: "soft",
      how: "plain",
    }),
  );
});

test("reset_head_with_an_empty_commit_is_head", async () => {
  mockCommands({ branch_reset: { kind: "done" } });
  const { component } = render(BranchDialogs);

  component.resetHead();

  const dialog = await screen.findByRole("dialog", { name: "Reset Head" });
  await fireEvent.input(within(dialog).getByLabelText("To Commit:"), { target: { value: "  " } });
  await fireEvent.click(within(dialog).getByRole("button", { name: "Reset" }));

  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("branch_reset", {
      target: "HEAD",
      mode: "mixed",
      how: "plain",
    }),
  );
});

test("after_a_commit_the_dialog_carries_the_preview_checkbox", async () => {
  mockCommands({ push_outgoing: [] });
  const { component } = render(BranchDialogs);

  component.prepareCommitPush()();

  expect(
    await screen.findByRole("checkbox", {
      name: "For Commit and Push to non-protected branches, preview commits before push",
    }),
  ).toBeTruthy();
});

// The `Abort` of the card of a repository that opens half-done, with nothing saved, goes through
// IntelliJ's confirmation and carries the HEAD the dialog saw.
test("card_abort_without_an_entry_asks_for_confirmation", async () => {
  const merging = { operation: "merging", branch: "main", label: "Merging main", head: HASH };
  repoState = merging;
  mockCommands({ operation_abort: "merge" });
  const { component } = render(BranchDialogs);
  component.repositoryChanged({ path: "/repo/a", pending: [], state: merging } as never);
  const card = () => get(notifications).find((item) => item.title === "Cannot complete update");

  card()?.actions?.[0].run();
  const dialog = await screen.findByRole("dialog", { name: "Abort Merge" });
  await fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));

  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  const commands = () => vi.mocked(invoke).mock.calls.map(([name]) => name);
  expect(commands()).not.toContain("operation_abort");
  expect(commands()).not.toContain("update_abort");
  // Cancelling leaves the card to try again.
  await waitFor(() => expect(card()).toBeTruthy());

  card()?.actions?.[0].run();
  await fireEvent.click(
    within(await screen.findByRole("dialog", { name: "Abort Merge" })).getByRole("button", {
      name: "Abort",
    }),
  );

  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("operation_abort", { kind: "merging", expectedHead: HASH }),
  );
  expect(commands()).not.toContain("update_abort");
});

// Each read gives the next value; the last one repeats.
function sequence<T>(...values: T[]) {
  let next = 0;
  return () => values[Math.min(next++, values.length - 1)];
}

const CONFLICT_SESSION = {
  conflicts: [{ path: "a.txt", yours: "modified", theirs: "modified", kind: "both modified" }],
  reversed: true,
  yoursColumn: "Yours (feature)",
  theirsColumn: "Theirs (main)",
  description: "Rebasing branch feature onto branch main",
};
const NO_CONFLICTS = { ...CONFLICT_SESSION, conflicts: [] };
const REBASING = { operation: "rebasing", branch: "feature", label: "Rebasing feature", head: HASH };

test("continue_rebase_with_conflicts_opens_conflicts_and_on_resolving_continues", async () => {
  repoState = REBASING;
  const continues = sequence({ kind: "conflicts" }, { kind: "done" });
  const sessions = sequence(CONFLICT_SESSION, NO_CONFLICTS);
  mockCommands({
    get operation_continue() {
      return continues();
    },
    get conflicts_session() {
      return sessions();
    },
    conflicts_accept: null,
  });
  const { component } = render(BranchDialogs);

  component.continueRebase();

  const dialog = await screen.findByRole("dialog", { name: "Conflicts" });
  expect(within(dialog).getByText("Rebasing branch feature onto branch main")).toBeTruthy();
  await fireEvent.click(within(dialog).getByRole("button", { name: "Accept Yours" }));

  await waitFor(() => expect(invoke).toHaveBeenCalledWith("conflicts_accept", { paths: ["a.txt"], side: "yours" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  const continued = () =>
    vi.mocked(invoke).mock.calls.filter(([name]) => name === "operation_continue");
  await waitFor(() => expect(continued()).toHaveLength(2));
  await waitFor(() =>
    expect(get(notifications).map(({ title }) => title)).toContain("Rebase successful"),
  );
  expect(get(notifications).map(({ title }) => title)).not.toContain(
    "Rebase stopped due to conflicts",
  );
});

test("closing_conflicts_with_pending_ones_in_a_rebase_leaves_the_rebase_card", async () => {
  repoState = REBASING;
  mockCommands({ operation_continue: { kind: "conflicts" }, conflicts_session: CONFLICT_SESSION });
  const { component } = render(BranchDialogs);

  component.continueRebase();
  const dialog = await screen.findByRole("dialog", { name: "Conflicts" });
  await fireEvent.click(within(dialog).getByRole("button", { name: "Close" }));

  await waitFor(() =>
    expect(get(notifications).map(({ title }) => title)).toEqual(["Rebase stopped due to conflicts"]),
  );
  expect(invoke).not.toHaveBeenCalledWith("merge_commit");
});

test("switching_repository_closes_conflicts_and_continues_nothing", async () => {
  repoState = REBASING;
  mockCommands({ operation_continue: { kind: "conflicts" }, conflicts_session: CONFLICT_SESSION });
  const { component } = render(BranchDialogs);
  component.continueRebase();
  await screen.findByRole("dialog", { name: "Conflicts" });

  const quiet = { operation: null, branch: "main", label: "main", head: HASH };
  component.repositoryChanged({ path: "/repo/other", pending: [], state: quiet } as never);

  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  await new Promise((resolve) => setTimeout(resolve, 20));
  const continued = vi.mocked(invoke).mock.calls.filter(([name]) => name === "operation_continue");
  expect(continued).toHaveLength(1);
  expect(get(notifications)).toEqual([]);
});

test("resolve_conflicts_from_the_menu_opens_the_dialog_and_continues_nothing_on_resolving", async () => {
  mockCommands({ conflicts_session: CONFLICT_SESSION });
  const { component } = render(BranchDialogs);

  component.resolveConflicts();

  const dialog = await screen.findByRole("dialog", { name: "Conflicts" });
  expect(within(dialog).getByRole("button", { name: "Merge…" })).toBeTruthy();
});

// A rebase that stops on conflicts again after resolving the previous one opens Conflicts again.
test.each([
  ["two", 2],
  ["three", 3],
])("%s_consecutive_stops_open_conflicts_each_time_and_finish", async (_name, stops) => {
  repoState = REBASING;
  const continues = sequence(
    ...Array(stops).fill({ kind: "conflicts" }),
    { kind: "done" },
  );
  const reads: unknown[] = [];
  for (let stop = 0; stop < stops; stop++) reads.push(CONFLICT_SESSION, NO_CONFLICTS, NO_CONFLICTS);
  const sessions = sequence(...reads);
  mockCommands({
    get operation_continue() {
      return continues();
    },
    get conflicts_session() {
      return sessions();
    },
    conflicts_accept: null,
  });
  const { component } = render(BranchDialogs);

  component.continueRebase();

  for (let stop = 0; stop < stops; stop++) {
    const dialog = await screen.findByRole("dialog", { name: "Conflicts" });
    await fireEvent.click(within(dialog).getByRole("button", { name: "Accept Yours" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  }
  const continued = () =>
    vi.mocked(invoke).mock.calls.filter(([name]) => name === "operation_continue");
  await waitFor(() => expect(continued()).toHaveLength(stops + 1));
  await waitFor(() =>
    expect(get(notifications).map(({ title }) => title)).toContain("Rebase successful"),
  );
});

describe("merge and rebase from the popup", () => {
  const NORMAL = { operation: null, branch: "main", label: "main", head: HASH };
  const SAVED = { sha: "abc", message: "Uncommitted changes before rebase" };
  const titles = () => get(notifications).map(({ title }) => title);
  const find = (title: string) => get(notifications).find((item) => item.title === title);

  test("popup_merge_notifies_and_delete_deletes_the_branch", async () => {
    repoState = NORMAL;
    mockCommands({
      branch_merge: {
        outcome: { kind: "merged", reason: { files: 1, commits: 1, range: null, matching: null } },
        saved: null,
        restore: null,
      },
      branch_delete: { name: "side", tip: HASH, upstream: null, forced: false, base: null },
    });
    const { component } = render(BranchDialogs);

    await component.run({ kind: "ref", operation: "merge", ref: { name: "side", kind: "local" } });

    expect(invoke).toHaveBeenCalledWith("branch_merge", {
      branch: "side",
      smart: false,
      expectedHead: HASH,
    });
    const merged = find("Merged side to main");
    expect(merged?.actions?.map(({ label }) => label)).toEqual(["Delete side"]);
    merged?.actions?.[0].run();
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("branch_delete", { name: "side" }));
  });

  test("stopped_popup_rebase_mentions_what_was_saved_and_brings_it_back_when_done", async () => {
    repoState = NORMAL;
    mockCommands({
      rebase_published: null,
      branch_rebase: {
        outcome: { kind: "editing" },
        title: null,
        saved: [{ saved: SAVED, restore: null }],
      },
      operation_continue: { kind: "done" },
      update_restore: { kind: "restored" },
    });
    const { component } = render(BranchDialogs);

    await component.run({ kind: "ref", operation: "rebase", ref: { name: "side", kind: "local" } });

    const stopped = find("Rebase stopped for editing");
    expect(stopped?.body).toBe("Local changes were stashed before rebase.");
    stopped?.actions?.find(({ label }) => label === "Continue")?.run();
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("update_restore", { saved: SAVED }));
    await waitFor(() => expect(titles()).toContain("Rebase successful"));
  });
});

describe("cherry-pick and revert from the log", () => {
  const NORMAL = { operation: null, branch: "main", label: "main", head: HASH };
  const PICKING = { ...NORMAL, operation: "cherryPicking", label: "Cherry-picking in main" };
  const STOPPED = {
    applied: [],
    skipped: [],
    stopped: { kind: "conflicts", reason: { hash: HASH } },
    remaining: [],
  };
  const COMMIT = { hash: HASH, subject: "one" };
  const titles = () => get(notifications).map(({ title }) => title);
  const find = (title: string) => get(notifications).find((item) => item.title === title);

  test("cherry_pick_with_conflicts_opens_conflicts_and_on_resolving_commits", async () => {
    repoState = NORMAL;
    const sessions = sequence(CONFLICT_SESSION, NO_CONFLICTS);
    mockCommands({
      apply_commits: STOPPED,
      get conflicts_session() {
        return sessions();
      },
      conflicts_accept: null,
      apply_commit_after_conflicts: "committed",
    });
    const { component } = render(BranchDialogs);

    const running = component.run({ kind: "cherryPick", commits: [COMMIT] });

    const dialog = await screen.findByRole("dialog", { name: "Conflicts" });
    expect(invoke).toHaveBeenCalledWith("apply_commits", {
      kind: "cherryPick",
      hashes: [HASH],
      expectedHead: HASH,
    });
    await fireEvent.click(within(dialog).getByRole("button", { name: "Accept Yours" }));
    await running;

    expect(invoke).toHaveBeenCalledWith("apply_commit_after_conflicts", {
      kind: "cherryPick",
      hash: HASH,
    });
    expect(find("Cherry-pick successful")?.body).toBe(`${HASH.slice(0, 8)} one`);
  });

  test("with_staging_area_it_does_not_commit_and_brings_the_commit_view", async () => {
    repoState = NORMAL;
    settingsNow = { ...SETTINGS, stagingArea: true };
    const sessions = sequence(CONFLICT_SESSION, NO_CONFLICTS);
    mockCommands({
      apply_commits: STOPPED,
      get conflicts_session() {
        return sessions();
      },
      conflicts_accept: null,
    });
    const onshowcommit = vi.fn();
    const { component } = render(BranchDialogs, { onshowcommit });

    const running = component.run({ kind: "cherryPick", commits: [COMMIT] });
    const dialog = await screen.findByRole("dialog", { name: "Conflicts" });
    await fireEvent.click(within(dialog).getByRole("button", { name: "Accept Yours" }));
    await running;

    expect(onshowcommit).toHaveBeenCalledTimes(1);
    const commands = vi.mocked(invoke).mock.calls.map(([name]) => name);
    expect(commands).not.toContain("apply_commit_after_conflicts");
    expect(titles()).toEqual([]);
  });

  test("card_abort_with_pending_conflicts_asks_for_confirmation_and_aborts", async () => {
    repoState = NORMAL;
    mockCommands({
      apply_commits: STOPPED,
      conflicts_session: CONFLICT_SESSION,
      operation_abort: "cherryPick",
    });
    const { component } = render(BranchDialogs);

    const running = component.run({ kind: "cherryPick", commits: [COMMIT] });
    const dialog = await screen.findByRole("dialog", { name: "Conflicts" });
    await fireEvent.click(within(dialog).getByRole("button", { name: "Close" }));
    await running;

    const card = find("Cherry-pick was performed with conflicts");
    expect(card?.actions?.map(({ label }) => label)).toEqual(["Resolve…", "Abort Cherry-pick"]);
    // To abort, the cherry-pick is already half-done.
    repoState = PICKING;
    card?.actions?.[1].run();
    const confirm = await screen.findByRole("dialog", { name: "Abort Cherry-pick" });
    await fireEvent.click(within(confirm).getByRole("button", { name: "Abort" }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("operation_abort", {
        kind: "cherryPicking",
        expectedHead: HASH,
      }),
    );
    await waitFor(() => expect(find("Cherry-pick was performed with conflicts")).toBeUndefined());
  });

  test("a_revert_without_conflicts_notifies_revert_successful", async () => {
    repoState = NORMAL;
    mockCommands({ apply_commits: { applied: [HASH], skipped: [], stopped: null, remaining: [] } });
    const { component } = render(BranchDialogs);

    await component.run({ kind: "revert", commits: [COMMIT] });

    expect(invoke).toHaveBeenCalledWith("apply_commits", {
      kind: "revert",
      hashes: [HASH],
      expectedHead: HASH,
    });
    expect(titles()).toEqual(["Revert successful"]);
  });
});
