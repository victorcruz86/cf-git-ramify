import { beforeEach, describe, expect, test, vi } from "vitest";
import { get } from "svelte/store";
import type { LogRef } from "../lib/api";
import { dismiss, notifications, type Shown } from "../lib/notifications";
import { takeShelfTabRequest } from "../shelf/flow";
import {
  branchActions,
  deletedNotification,
  type BranchApi,
  type BranchUi,
  type DialogRequest,
} from "./actions";

const HASH = "a1b2c3d4".repeat(5);
const local = (name: string): LogRef => ({ name, kind: "local" });
const remote = (name: string): LogRef => ({ name, kind: "remote" });
const tag = (name: string): LogRef => ({ name, kind: "tag" });
const DONE = { kind: "done" };
const STASH = { kind: "stash" as const, sha: "abc", message: "Uncommitted changes before Checkout at today", lists: [] };

function api(overrides: Partial<BranchApi> = {}): BranchApi {
  return {
    checkout: vi.fn(async () => DONE),
    checkoutRemote: vi.fn(async () => DONE),
    rebaseOnto: vi.fn(async () => ({ outcome: DONE, title: null, saved: [] })),
    published: vi.fn(async () => null),
    create: vi.fn(async () => DONE),
    rename: vi.fn(async () => null),
    remove: vi.fn(),
    restore: vi.fn(async () => null),
    deleteRemote: vi.fn(async () => ({ local: [], failures: [] })),
    trackers: vi.fn(async () => []),
    reset: vi.fn(async () => DONE),
    tag: vi.fn(async () => null),
    deleteTag: vi.fn(),
    restoreTag: vi.fn(async () => null),
    unmerged: vi.fn(async () => []),
    branches: vi.fn(async () => ({ local: ["main", "other"], remote: ["origin/main"] })),
    currentBranch: vi.fn(async () => "main"),
    commit: vi.fn(async () => ({ hash: HASH, subject: "one", author: "Ana" })),
    resetMode: vi.fn(async () => "mixed" as const),
    saveResetMode: vi.fn(async () => {}),
    ...overrides,
  } as BranchApi;
}

// Answers each dialog with whatever applies and records which ones came up.
function ui(
  answers: Partial<Record<DialogRequest["kind"], unknown>> = {},
  extra: Partial<BranchUi> = {},
) {
  const asked: DialogRequest[] = [];
  return {
    ...extra,
    asked,
    ask: async (request: DialogRequest) => {
      asked.push(request);
      return (answers[request.kind] ?? null) as never;
    },
  };
}

const shown = () => get(notifications);
const last = (): Shown => shown()[shown().length - 1];

beforeEach(() => {
  for (const { id } of shown()) dismiss(id);
});

describe("checkout", () => {
  test("plain_checkout_notifies", async () => {
    const backend = api();
    await branchActions(ui(), backend).run({ kind: "checkout", target: "other" });

    expect(backend.checkout).toHaveBeenCalledWith("other", "plain");
    expect(last().title).toBe("Checked out other");
  });

  test("problem_offers_smart_and_force_and_retries", async () => {
    const checkout = vi
      .fn()
      .mockResolvedValueOnce({ kind: "overwritten", files: ["a.txt"] })
      .mockResolvedValue(DONE);
    const dialogs = ui({ problem: "smart" });
    await branchActions(dialogs, api({ checkout })).run({ kind: "checkout", target: "other" });

    expect(dialogs.asked).toMatchObject([
      {
        kind: "problem",
        operation: "checkout",
        title: "Git Checkout Problem",
        smart: "Smart Checkout",
        force: "Force Checkout",
        cancel: "Don't Checkout",
        files: ["a.txt"],
      },
    ]);
    expect(checkout).toHaveBeenLastCalledWith("other", "smart");
  });

  test("dont_checkout_does_nothing_else", async () => {
    const checkout = vi.fn().mockResolvedValue({ kind: "overwritten", files: ["a.txt"] });
    await branchActions(ui(), api({ checkout })).run({ kind: "checkout", target: "other" });

    expect(checkout).toHaveBeenCalledTimes(1);
    expect(shown()).toEqual([]);
  });

  test("conflicts_on_restoring_warn_and_stay", async () => {
    const checkout = vi.fn().mockResolvedValue({ kind: "conflicts", stash: STASH, failure: null });
    await branchActions(ui(), api({ checkout })).run({ kind: "checkout", target: "other" });

    expect(last()).toMatchObject({
      kind: "warning",
      title: "Local changes were restored with conflicts",
      sticky: true,
    });
  });

  // G:398-401, with `View saved changes…` first, as `UnstashConflictResolver` adds them.
  test("unstash_with_conflicts_says_241s_text_and_offers_resolve", async () => {
    const checkout = vi.fn().mockResolvedValue({ kind: "conflicts", stash: STASH, failure: null });
    const resolveConflicts = vi.fn(async () => "resolved" as const);
    const showStashes = vi.fn();
    await branchActions(ui({}, { resolveConflicts, showStashes }), api({ checkout })).run({
      kind: "checkout",
      target: "other",
    });

    expect(last().body).toBe(
      "Your uncommitted changes were saved to stash.\nUnstash is not complete, you have unresolved merges in your working tree\nResolve conflicts and drop the stash.",
    );
    expect(last().actions?.map(({ label }) => label)).toEqual([
      "View saved changes…",
      "Resolve conflicts…",
    ]);
    last().actions?.[0].run();
    expect(showStashes).toHaveBeenCalledTimes(1);
    last().actions?.[1].run();
    expect(resolveConflicts).toHaveBeenCalledTimes(1);
  });

  test("smart_checkout_with_shelve", async () => {
    const message = "Uncommitted changes before Checkout at today";
    const shelf = {
      kind: "shelve" as const,
      sha: "",
      message,
      lists: [{ name: "Uncommitted_changes", description: `${message} [Changes]`, changelist: "default" }],
    };
    const showStashes = vi.fn();
    const resolveConflicts = vi.fn();
    takeShelfTabRequest();
    const checkout = vi
      .fn()
      .mockResolvedValueOnce({ kind: "conflicts", stash: shelf, failure: null })
      .mockResolvedValueOnce({ kind: "notRestored", stash: shelf, reason: "it broke" })
      .mockResolvedValueOnce({ kind: "gone", stash: shelf, failure: null });
    const actions = branchActions(ui({}, { showStashes, resolveConflicts }), api({ checkout }));

    await actions.run({ kind: "checkout", target: "other" });

    expect(last()).toMatchObject({ kind: "warning", title: "Local changes were not restored", sticky: true });
    expect(last().body).toContain(`were left in the shelf: ${message}`);
    expect(last().body).not.toMatch(/stash/);
    expect(last().actions?.map(({ label }) => label)).toEqual(["View saved changes…"]);
    last().actions?.[0].run();
    expect(takeShelfTabRequest()).toEqual({ select: "Uncommitted_changes" });
    expect(showStashes).not.toHaveBeenCalled();
    expect(resolveConflicts).not.toHaveBeenCalled();

    await actions.run({ kind: "checkout", target: "other" });
    expect(last().body).toBe(`it broke\nThey are in the shelf: ${message}`);
    expect(last().actions?.map(({ label }) => label)).toEqual(["View saved changes…"]);

    await actions.run({ kind: "checkout", target: "other" });
    expect(last().body).toBe(`They are no longer in the shelf: ${message}`);
    expect(last().actions).toBeUndefined();
  });

  test("conflicts_with_the_failed_operation_report_both", async () => {
    const checkout = vi
      .fn()
      .mockResolvedValue({ kind: "conflicts", stash: STASH, failure: "git failed" });
    await branchActions(ui(), api({ checkout })).run({ kind: "checkout", target: "other" });

    expect(last().title).toBe("Local changes were restored with conflicts");
    expect(last().body).toContain("git failed");
  });

  test("without_the_stage_warns_and_never_offers_to_drop_the_entry", async () => {
    const checkout = vi
      .fn()
      .mockResolvedValue({ kind: "restoredWithoutIndex", stash: STASH, failure: null });
    await branchActions(ui(), api({ checkout })).run({ kind: "checkout", target: "other" });

    expect(last()).toMatchObject({
      kind: "warning",
      sticky: true,
      title: "Local changes were restored without staged changes",
    });
    expect(last().body).toContain(STASH.message);
    expect(last().body).not.toMatch(/drop|conflict/i);
  });

  test("what_does_not_come_back_says_where_it_is", async () => {
    const checkout = vi
      .fn()
      .mockResolvedValue({ kind: "notRestored", stash: STASH, reason: "it broke" });
    await branchActions(ui(), api({ checkout })).run({ kind: "checkout", target: "other" });

    expect(last()).toMatchObject({ kind: "error", title: "Local changes were not restored" });
    expect(last().body).toContain(STASH.message);
    expect(last().body).toContain("it broke");
  });

  test("failure_notifies_and_cancelling_does_not", async () => {
    const checkout = vi
      .fn()
      .mockRejectedValueOnce({ kind: "failed", message: "git failed" })
      .mockRejectedValueOnce({ kind: "cancelled", message: "cancelled" });
    const actions = branchActions(ui(), api({ checkout }));

    await actions.run({ kind: "checkout", target: "other" });
    expect(last()).toMatchObject({ kind: "error", title: "Could not checkout other", body: "git failed" });

    dismiss(last().id);
    await actions.run({ kind: "checkout", target: "other" });
    expect(shown()).toEqual([]);
  });

  test("typed_reference_goes_with_caret_zero", async () => {
    const backend = api();
    await branchActions(ui({ checkoutRef: "v1.0" }), backend).checkoutRef();

    expect(backend.checkout).toHaveBeenCalledWith("v1.0^0", "plain");
  });
});

test("branch_update_goes_to_the_flow_and_never_to_delete", async () => {
  const backend = api();
  const updateBranch = vi.fn(async () => {});
  const { run } = branchActions({ ask: vi.fn(), updateBranch }, backend);

  await run({ kind: "ref", operation: "update", ref: local("other") });

  expect(updateBranch).toHaveBeenCalledWith("other");
  expect(backend.remove).not.toHaveBeenCalled();
});

test("an_entry_that_is_gone_does_not_say_it_is_still_in_the_stash", async () => {
  const backend = api({
    checkout: vi.fn(async () => ({ kind: "gone" as const, stash: STASH, failure: null })),
  });
  const { checkout } = branchActions({ ask: vi.fn() }, backend);

  await checkout("other");

  const [notification] = get(notifications);
  expect(notification.title).toBe("Local changes were not restored");
  expect(notification.sticky).toBe(true);
  expect(notification.body).toContain("no longer in the stash");
  expect(notification.body).toContain(STASH.message);
  expect(notification.body).not.toContain("They are in the stash");
});

test("branch_push_opens_the_dialog_and_does_not_delete_it", async () => {
  const backend = api();
  const pushBranch = vi.fn(async () => {});

  await branchActions({ ...ui(), pushBranch }, backend).run({
    kind: "ref",
    operation: "push",
    ref: { name: "other", kind: "local" },
  });

  expect(pushBranch).toHaveBeenCalledWith("other");
  expect(backend.remove).not.toHaveBeenCalled();
});

test("push_all_up_to_here_opens_the_dialog_with_that_commit", async () => {
  const pushUpTo = vi.fn(async () => {});

  await branchActions({ ...ui(), pushUpTo }, api()).run({ kind: "pushUpTo", hash: "abc" });

  expect(pushUpTo).toHaveBeenCalledWith("abc");
});

describe("checkout of a remote branch", () => {
  const action = { kind: "ref", operation: "checkout", ref: remote("origin/x") } as const;

  test("goes_through_the_remote", async () => {
    const backend = api();
    await branchActions(ui(), backend).run(action);

    expect(backend.checkoutRemote).toHaveBeenCalledWith("origin/x", {
      name: null,
      dropCommits: false,
      mode: "plain",
    });
    expect(last().title).toBe("Checked out origin/x");
  });

  test("with_own_commits_rebase_drop_or_cancel", async () => {
    const checkoutRemote = vi
      .fn()
      .mockResolvedValueOnce({ kind: "localHasCommits", local: "x" })
      .mockResolvedValue(DONE);
    const dialogs = ui({ localCommits: "drop" });
    await branchActions(dialogs, api({ checkoutRemote })).run(action);

    expect(dialogs.asked[0]).toMatchObject({
      kind: "localCommits",
      title: "Checkout origin/x",
      message:
        "Local branch 'x' has commits that do not exist in 'origin/x'. Rebase 'x' onto 'origin/x', or drop local commits?",
    });
    expect(checkoutRemote).toHaveBeenLastCalledWith("origin/x", {
      name: null,
      dropCommits: true,
      mode: "plain",
    });
  });

  test("rebase_onto_remote", async () => {
    const backend = api({
      checkoutRemote: vi.fn(async () => ({ kind: "localHasCommits", local: "x" })) as never,
    });
    await branchActions(ui({ localCommits: "rebase" }), backend).run(action);

    expect(backend.rebaseOnto).toHaveBeenCalledWith("origin/x", "x");
  });

  test("stopped_rebase_warns_with_resolve_continue_and_abort", async () => {
    const rebaseStopped = vi.fn();
    const backend = api({
      checkoutRemote: vi.fn(async () => ({ kind: "localHasCommits", local: "x" })) as never,
      rebaseOnto: vi.fn(async () => ({
        outcome: { kind: "conflicts" },
        title: null,
        saved: [],
      })) as never,
    });
    await branchActions(ui({ localCommits: "rebase" }, { rebaseStopped }), backend).run(action);

    expect(rebaseStopped).toHaveBeenCalledWith({ kind: "conflicts" }, []);
    expect(shown()).toEqual([]);
  });

  test("a_rebase_that_finishes_reports_nothing_stopped", async () => {
    const rebaseStopped = vi.fn();
    const backend = api({
      checkoutRemote: vi.fn(async () => ({ kind: "localHasCommits", local: "x" })) as never,
    });
    await branchActions(ui({ localCommits: "rebase" }, { rebaseStopped }), backend).run(action);

    expect(rebaseStopped).not.toHaveBeenCalled();
  });

  test("failing_rebase_says_so", async () => {
    const backend = api({
      checkoutRemote: vi.fn(async () => ({ kind: "localHasCommits", local: "x" })) as never,
      rebaseOnto: vi.fn().mockRejectedValue({ kind: "failed", message: "dirty" }),
    });
    await branchActions(ui({ localCommits: "rebase" }), backend).run(action);

    expect(last()).toMatchObject({ kind: "error", title: "Rebase failed", body: "dirty" });
  });

  test("asks_for_a_name_with_the_branch_dialog", async () => {
    const checkoutRemote = vi
      .fn()
      .mockResolvedValueOnce({ kind: "needsName", suggested: "x" })
      .mockResolvedValue(DONE);
    const dialogs = ui({ newBranch: { name: "mine", checkout: true, overwrite: false } });
    await branchActions(dialogs, api({ checkoutRemote })).run(action);

    expect(dialogs.asked[0]).toMatchObject({
      kind: "newBranch",
      title: "Checkout origin/x",
      initialName: "x",
      showCheckout: false,
      showReset: true,
      okLabel: "Checkout",
    });
    expect(checkoutRemote).toHaveBeenLastCalledWith("origin/x", {
      name: "mine",
      dropCommits: false,
      mode: "plain",
    });
  });

  test("problem_retries_with_the_chosen_mode", async () => {
    const checkoutRemote = vi
      .fn()
      .mockResolvedValueOnce({ kind: "overwritten", files: ["a.txt"] })
      .mockResolvedValue(DONE);
    await branchActions(ui({ problem: "force" }), api({ checkoutRemote })).run(action);

    expect(checkoutRemote).toHaveBeenLastCalledWith("origin/x", {
      name: null,
      dropCommits: false,
      mode: "force",
    });
  });
});

describe("new branch", () => {
  test("from_a_commit", async () => {
    const backend = api();
    const dialogs = ui({ newBranch: { name: "n", checkout: true, overwrite: false } });
    await branchActions(dialogs, backend).run({ kind: "newBranch", hash: HASH });

    expect(dialogs.asked[0]).toMatchObject({
      title: "Create Branch from a1b2c3d4",
      showCheckout: true,
      showReset: true,
      okLabel: "Create",
      names: { local: ["main", "other"], remote: ["origin/main"] },
    });
    expect(backend.create).toHaveBeenCalledWith({
      name: "n",
      start: HASH,
      checkout: true,
      overwrite: "no",
      mode: "plain",
    });
    expect(last().title).toBe("Checked out new branch n from a1b2c3d4");
  });

  test("from_a_remote_suggests_the_name_without_the_remote", async () => {
    const dialogs = ui();
    await branchActions(dialogs, api()).run({
      kind: "ref",
      operation: "newBranch",
      ref: remote("origin/a/b"),
    });

    expect(dialogs.asked[0]).toMatchObject({
      title: "Create Branch from origin/a/b",
      initialName: "a/b",
    });
  });

  test("overwrite_with_own_commits_asks_drop", async () => {
    const create = vi
      .fn()
      .mockResolvedValueOnce({ kind: "localHasCommits", local: "other" })
      .mockResolvedValue(DONE);
    const dialogs = ui({
      newBranch: { name: "other", checkout: false, overwrite: true },
      localCommits: "drop",
    });
    await branchActions(dialogs, api({ create })).run({ kind: "newBranch", hash: HASH });

    expect(dialogs.asked[1]).toMatchObject({
      kind: "localCommits",
      rebase: false,
      message: `Local branch 'other' has commits that do not exist in 'a1b2c3d4'. Do you want to drop local commits?`,
    });
    expect(create).toHaveBeenNthCalledWith(1, expect.objectContaining({ overwrite: "try" }));
    expect(create).toHaveBeenLastCalledWith(expect.objectContaining({ overwrite: "drop" }));
  });
});

describe("reset", () => {
  test("reset_offers_the_remembered_mode_of_the_repository", async () => {
    const backend = api({ resetMode: vi.fn(async () => "keep" as const) });
    const dialogs = ui({ reset: "keep" });
    await branchActions(dialogs, backend).run({ kind: "reset", hash: HASH });

    expect(dialogs.asked[0]).toMatchObject({ kind: "reset", mode: "keep" });
  });

  test("if_the_mode_cannot_be_read_it_is_mixed", async () => {
    const backend = api({
      resetMode: vi.fn(async () => {
        throw new Error("no");
      }),
    });
    const dialogs = ui({ reset: "soft" });
    await branchActions(dialogs, backend).run({ kind: "reset", hash: HASH });

    expect(dialogs.asked[0]).toMatchObject({ kind: "reset", mode: "mixed" });
    expect(backend.reset).toHaveBeenCalledWith(HASH, "soft", "plain");
  });

  test("the_dialog_saves_the_choice", async () => {
    const backend = api();
    const dialogs = ui({ reset: "soft" });
    await branchActions(dialogs, backend).run({ kind: "reset", hash: HASH });

    expect(dialogs.asked[0]).toMatchObject({ kind: "reset", mode: "mixed", source: "main" });
    expect(backend.saveResetMode).toHaveBeenCalledWith("soft");
  });

  test("if_the_mode_cannot_be_saved_the_reset_goes_on", async () => {
    const backend = api({
      saveResetMode: vi.fn(async () => {
        throw new Error("disk full");
      }),
    });
    await branchActions(ui({ reset: "soft" }), backend).run({ kind: "reset", hash: HASH });

    expect(backend.reset).toHaveBeenCalledWith(HASH, "soft", "plain");
  });

  test("cancelling_saves_and_touches_nothing", async () => {
    const backend = api();
    await branchActions(ui(), backend).run({ kind: "reset", hash: HASH });

    expect(backend.reset).not.toHaveBeenCalled();
    expect(backend.saveResetMode).not.toHaveBeenCalled();
  });

  test("hard_asks_for_no_further_confirmation", async () => {
    const backend = api();
    const dialogs = ui({ reset: "hard" });
    await branchActions(dialogs, backend).run({ kind: "reset", hash: HASH });

    expect(dialogs.asked).toHaveLength(1);
    expect(backend.reset).toHaveBeenCalledWith(HASH, "hard", "plain");
    expect(last().title).toBe("Reset successful");
  });

  test("keep_with_conflict_offers_hard_reset", async () => {
    const reset = vi
      .fn()
      .mockResolvedValueOnce({ kind: "overwritten", files: ["a.txt"] })
      .mockResolvedValue(DONE);
    const dialogs = ui({ reset: "keep", problem: "force" });
    await branchActions(dialogs, api({ reset })).run({ kind: "reset", hash: HASH });

    expect(dialogs.asked[1]).toMatchObject({
      operation: "reset",
      title: "Git Reset Problem",
      smart: "Smart Reset",
      force: "Hard Reset",
      cancel: "Don't Reset",
    });
    expect(reset).toHaveBeenLastCalledWith(HASH, "keep", "force");
  });

  test("failure_says_reset_failed", async () => {
    const reset = vi.fn().mockRejectedValue({ kind: "failed", message: "no" });
    await branchActions(ui({ reset: "hard" }), api({ reset })).run({ kind: "reset", hash: HASH });

    expect(last()).toMatchObject({ kind: "error", title: "Reset failed", body: "no" });
  });
});

describe("delete", () => {
  const merged = { name: "b", tip: HASH, upstream: null, forced: false, base: null };
  const forced = { ...merged, forced: true, base: "main" };

  test("delete_notification_offers_restore_and_view_commits", () => {
    const handlers = { restore: vi.fn(), viewCommits: vi.fn() };

    const unmerged = deletedNotification(forced, handlers);
    expect(unmerged).toMatchObject({
      kind: "info",
      sticky: true,
      title: "Deleted Branch: b",
      body: "Unmerged commits were discarded",
    });
    expect(unmerged.actions?.map(({ label }) => label)).toEqual(["Restore", "View Commits"]);
    unmerged.actions?.[0].run();
    expect(handlers.restore).toHaveBeenCalled();
    unmerged.actions?.[1].run();
    expect(handlers.viewCommits).toHaveBeenCalled();

    const clean = deletedNotification(merged, handlers);
    expect(clean.body).toBeUndefined();
    expect(clean.actions?.map(({ label }) => label)).toEqual(["Restore"]);
  });

  test("deleting_a_local_does_not_ask_and_restore_brings_it_back", async () => {
    const backend = api({ remove: vi.fn(async () => merged) as never });
    const dialogs = ui();
    await branchActions(dialogs, backend).run({
      kind: "ref",
      operation: "delete",
      ref: local("b"),
    });

    expect(dialogs.asked).toEqual([]);
    last().actions?.[0].run();
    await vi.waitFor(() => expect(backend.restore).toHaveBeenCalledWith(merged));
  });

  test("view_commits_opens_the_list_and_ok_restores", async () => {
    const commits = [{ hash: HASH, subject: "one", author: "Ana", time: 1 }];
    const backend = api({
      remove: vi.fn(async () => forced) as never,
      unmerged: vi.fn(async () => commits),
    });
    const dialogs = ui({ unmerged: true });
    await branchActions(dialogs, backend).run({
      kind: "ref",
      operation: "delete",
      ref: local("b"),
    });

    last().actions?.[1].run();
    await vi.waitFor(() => expect(backend.restore).toHaveBeenCalledWith(forced));
    expect(backend.unmerged).toHaveBeenCalledWith("main", HASH);
    expect(dialogs.asked[0]).toMatchObject({
      kind: "unmerged",
      title: "Branch Was Not Fully Merged",
      message:
        "The branch b was not fully merged to main. Below is the list of unmerged commits.",
      commits,
    });
  });

  test("deleting_a_remote_asks_and_offers_the_local", async () => {
    const backend = api({
      trackers: vi.fn(async () => ["x"]),
      deleteRemote: vi.fn(async () => ({ local: [merged], failures: [] })) as never,
    });
    const dialogs = ui({ deleteRemote: { alsoLocal: true } });
    await branchActions(dialogs, backend).run({
      kind: "ref",
      operation: "delete",
      ref: remote("origin/x"),
    });

    expect(dialogs.asked[0]).toMatchObject({
      kind: "deleteRemote",
      branches: ["origin/x"],
      tracking: ["x"],
    });
    expect(backend.deleteRemote).toHaveBeenCalledWith("origin/x", true);
    expect(last()).toMatchObject({
      title: "Deleted remote branch origin/x",
      body: "Also deleted local branch: b",
    });
  });

  // `GitDeleteRemoteBranchOperation.execute`: the trackers of all of them, one question
  // (`B:1253-1257`), then each deletion; a branch only takes its own trackers with it.
  test("deleting_several_remote_branches_asks_once", async () => {
    const backend = api({
      trackers: vi.fn(async (branch: string) => (branch === "origin/x" ? ["x"] : [])) as never,
    });
    const dialogs = ui({ deleteRemote: { alsoLocal: true } });

    await branchActions(dialogs, backend).run({ kind: "deleteRemotes", names: ["origin/x", "origin/y"] });

    expect(dialogs.asked).toEqual([
      { kind: "deleteRemote", branches: ["origin/x", "origin/y"], tracking: ["x"] },
    ]);
    expect(vi.mocked(backend.deleteRemote).mock.calls).toEqual([
      ["origin/x", true],
      ["origin/y", false],
    ]);
    expect(shown().map(({ title }) => title)).toEqual([
      "Deleted remote branch origin/x",
      "Deleted remote branch origin/y",
    ]);
  });

  test("updating_several_branches_goes_to_the_update_flow_at_once", async () => {
    const updateBranches = vi.fn(async () => {});
    await branchActions(ui({}, { updateBranches }), api()).run({
      kind: "updateBranches",
      names: ["main", "side"],
    });

    expect(updateBranches).toHaveBeenCalledWith(["main", "side"]);
  });

  test("cancelling_the_remote_does_not_delete", async () => {
    const backend = api();
    await branchActions(ui(), backend).run({
      kind: "ref",
      operation: "delete",
      ref: remote("origin/x"),
    });

    expect(backend.deleteRemote).not.toHaveBeenCalled();
  });

  test("tag_is_deleted_without_asking_and_comes_back", async () => {
    const deleted = { name: "v1", tip: HASH };
    const backend = api({ deleteTag: vi.fn(async () => deleted) as never });
    const dialogs = ui();
    await branchActions(dialogs, backend).run({ kind: "ref", operation: "delete", ref: tag("v1") });

    expect(dialogs.asked).toEqual([]);
    expect(last()).toMatchObject({ title: "Deleted Tag: v1", sticky: true });
    last().actions?.[0].run();
    await vi.waitFor(() => expect(backend.restoreTag).toHaveBeenCalledWith(deleted));
  });
});

describe("rename and tags", () => {
  test("rename", async () => {
    const backend = api();
    const dialogs = ui({ rename: "renamed" });
    await branchActions(dialogs, backend).run({
      kind: "ref",
      operation: "rename",
      ref: local("other"),
    });

    expect(dialogs.asked[0]).toMatchObject({ kind: "rename", branch: "other" });
    expect(backend.rename).toHaveBeenCalledWith("other", "renamed");
    expect(last().title).toBe("Branch other was renamed to renamed");
  });

  test("new_tag_on_a_commit", async () => {
    const backend = api();
    const dialogs = ui({ tag: { name: "v1", target: HASH, message: "", force: false } });
    await branchActions(dialogs, backend).run({ kind: "newTag", hash: HASH });

    expect(dialogs.asked[0]).toMatchObject({ kind: "tag", hash: HASH });
    expect(backend.tag).toHaveBeenCalledWith("v1", HASH, "", false);
    expect(last().title).toBe("Created tag v1 successfully.");
  });

  test("failing_tag", async () => {
    const backend = api({ tag: vi.fn().mockRejectedValue({ kind: "failed", message: "already exists" }) });
    const dialogs = ui({ tag: { name: "v1", target: HASH, message: "", force: false } });
    await branchActions(dialogs, backend).run({ kind: "newTag", hash: HASH });

    expect(last()).toMatchObject({ title: "Could not create tag", body: "already exists" });
  });
});

describe("switching repository", () => {
  const merged = { name: "b", tip: HASH, upstream: null, forced: false, base: null };
  const forced = { ...merged, forced: true, base: "main" };
  const remove = { kind: "ref", operation: "delete", ref: local("b") } as const;
  const removeTag = { kind: "ref", operation: "delete", ref: tag("v1") } as const;
  const stopped = { outcome: { kind: "conflicts" }, title: null, saved: [] };
  const checkoutRemote = vi.fn(async () => ({ kind: "localHasCommits", local: "x" })) as never;
  const rebase = { kind: "ref", operation: "checkout", ref: remote("origin/x") } as const;

  // The operation stays half-done until `finish` lets it go.
  function held<T>(value: T) {
    let finish = () => {};
    const work = vi.fn(
      () => new Promise<T>((resolve) => (finish = () => resolve(value))),
    );
    return { work, finish: () => finish() };
  }

  test("dismisses_the_branch_tag_and_rebase_cards", async () => {
    const backend = api({
      remove: vi.fn(async () => forced) as never,
      deleteTag: vi.fn(async () => ({ name: "v1", tip: HASH })) as never,
      checkoutRemote,
      rebaseOnto: vi.fn(async () => stopped) as never,
    });
    // Without the `src/operation` flow to draw it, the card belongs to this one.
    const actions = branchActions(ui({ localCommits: "rebase" }), backend);
    await actions.run(remove);
    await actions.run(removeTag);
    await actions.run(rebase);
    expect(shown()).toHaveLength(3);

    actions.repositoryChanged();

    expect(shown()).toEqual([]);
  });

  test("does_not_dismiss_what_does_not_act_on_the_repository", async () => {
    const actions = branchActions(ui(), api());
    await actions.run({ kind: "checkout", target: "other" });

    actions.repositoryChanged();

    expect(last().title).toBe("Checked out other");
  });

  test("a_delete_that_finishes_later_does_not_offer_restore", async () => {
    const { work, finish } = held(forced);
    const actions = branchActions(ui(), api({ remove: work as never }));
    const running = actions.run(remove);
    await vi.waitFor(() => expect(work).toHaveBeenCalled());

    actions.repositoryChanged();
    finish();
    await running;

    expect(last().title).toBe("Deleted Branch: b");
    expect(last().actions).toBeUndefined();
    expect(last().body).toContain("previous repository");
  });

  test("a_tag_delete_that_finishes_later_does_not_offer_restore", async () => {
    const { work, finish } = held({ name: "v1", tip: HASH });
    const actions = branchActions(ui(), api({ deleteTag: work as never }));
    const running = actions.run(removeTag);
    await vi.waitFor(() => expect(work).toHaveBeenCalled());

    actions.repositoryChanged();
    finish();
    await running;

    expect(last().title).toBe("Deleted Tag: v1");
    expect(last().actions).toBeUndefined();
  });

  test("a_rebase_that_stops_later_does_not_offer_abort", async () => {
    const { work, finish } = held(stopped);
    const actions = branchActions(
      ui({ localCommits: "rebase" }),
      api({ checkoutRemote, rebaseOnto: work as never }),
    );
    const running = actions.run(rebase);
    await vi.waitFor(() => expect(work).toHaveBeenCalled());

    actions.repositoryChanged();
    finish();
    await running;

    expect(last().title).toBe("Rebase stopped due to conflicts");
    expect(last().actions).toBeUndefined();
    expect(last().body).toContain("previous repository");
  });

  test("what_was_already_launched_does_not_dismiss_the_new_card", async () => {
    const actions = branchActions(ui(), api({ remove: vi.fn(async () => merged) as never }));
    actions.repositoryChanged();

    await actions.run(remove);

    expect(last().actions?.map(({ label }) => label)).toEqual(["Restore"]);
  });

  test("half_done_view_commits_does_not_restore_in_the_other", async () => {
    const { work, finish } = held([{ hash: HASH, subject: "one", author: "Ana", time: 1 }]);
    const backend = api({ remove: vi.fn(async () => forced) as never, unmerged: work as never });
    const dialogs = ui({ unmerged: true });
    const actions = branchActions(dialogs, backend);
    await actions.run(remove);
    last().actions?.[1].run();
    await vi.waitFor(() => expect(work).toHaveBeenCalled());

    actions.repositoryChanged();
    finish();
    await new Promise((resolve) => setTimeout(resolve));

    expect(dialogs.asked).toEqual([]);
    expect(backend.restore).not.toHaveBeenCalled();
  });

  test("view_commits_with_the_dialog_open_does_not_restore_in_the_other", async () => {
    const backend = api({ remove: vi.fn(async () => forced) as never });
    let answer: (value: boolean) => void = () => {};
    const asked = vi.fn(() => new Promise((resolve) => (answer = resolve)));
    const actions = branchActions({ ask: asked as never }, backend);
    await actions.run(remove);
    last().actions?.[1].run();
    await vi.waitFor(() => expect(asked).toHaveBeenCalled());

    actions.repositoryChanged();
    answer(true);
    await new Promise((resolve) => setTimeout(resolve));

    expect(backend.restore).not.toHaveBeenCalled();
  });
});

describe("reset head from the git menu", () => {
  const CHOICE = { mode: "mixed", commit: "HEAD" };

  test("asks_current_branch_and_does_not_remember_the_mode", async () => {
    const backend = api();
    const dialogs = ui();
    await branchActions(dialogs, backend).resetHead();

    expect(dialogs.asked).toEqual([{ kind: "resetHead", branch: "main" }]);
    expect(backend.resetMode).not.toHaveBeenCalled();
    expect(backend.reset).not.toHaveBeenCalled();
  });

  test("detached_head_says_there_is_no_branch", async () => {
    const dialogs = ui();
    await branchActions(dialogs, api({ currentBranch: vi.fn(async () => null) })).resetHead();

    expect(dialogs.asked[0]).toMatchObject({ branch: "<no active branch>" });
  });

  test("launches_the_reset_and_like_gitresethead_does_not_warn_on_success", async () => {
    const backend = api();
    await branchActions(ui({ resetHead: { ...CHOICE, mode: "hard" } }), backend).resetHead();

    expect(backend.reset).toHaveBeenCalledWith("HEAD", "hard", "plain");
    expect(backend.saveResetMode).not.toHaveBeenCalled();
    expect(shown()).toEqual([]);
  });

  test("the_failure_carries_the_title_of_gitresethead", async () => {
    const backend = api({ reset: vi.fn().mockRejectedValue({ kind: "failed", message: "no" }) });
    await branchActions(ui({ resetHead: CHOICE }), backend).resetHead();

    expect(last()).toMatchObject({ kind: "error", title: "Resetting HEAD…", body: "no" });
  });

  test("overwritten_files_offer_the_problem", async () => {
    const reset = vi
      .fn()
      .mockResolvedValueOnce({ kind: "overwritten", files: ["a.txt"] })
      .mockResolvedValue(DONE);
    const dialogs = ui({ resetHead: CHOICE, problem: "force" });
    await branchActions(dialogs, api({ reset })).resetHead();

    expect(dialogs.asked[1]).toMatchObject({ kind: "problem", title: "Git Reset Problem" });
    expect(reset).toHaveBeenLastCalledWith("HEAD", "mixed", "force");
  });
});
