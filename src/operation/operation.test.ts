import { beforeEach, describe, expect, test, vi } from "vitest";
import { get } from "svelte/store";
import type { Aborted, Operation, RebaseOutcome, RepoInfo, RepoState } from "../lib/api";
import { dismiss, notifications, type Shown } from "../lib/notifications";
import { conflictsFlow, type ConflictsApi, type ConflictsRequest, type ResolveOptions } from "../conflicts/flow";
import {
  ABORTED,
  gitMenuEnabled,
  ongoingActionFor,
  ongoingActions,
  operationFlow,
  rebaseStoppedNotification,
  type ConfirmRequest,
  type OperationApi,
  type OperationUi,
} from "./operation";

const HEAD = "a1b2c3d4".repeat(5);
const state = (operation: Operation | null, label = "main"): RepoState => ({
  operation,
  branch: "main",
  label,
  head: HEAD,
});
const NORMAL = state(null);
const REBASING = state("rebasing", "Rebasing main");
const DONE: RebaseOutcome = { kind: "done" };
const CONFLICTS: RebaseOutcome = { kind: "conflicts" };

function api(overrides: Partial<OperationApi> = {}): OperationApi {
  return {
    state: vi.fn(async () => REBASING),
    abort: vi.fn(async (): Promise<Aborted> => "rebase"),
    continue: vi.fn(async () => DONE),
    skip: vi.fn(async () => DONE),
    ...overrides,
  };
}

// Answers the confirmation with whatever is needed and records what was asked.
function ui(confirm: boolean | null = true, extra: Partial<OperationUi> = {}) {
  const asked: ConfirmRequest[] = [];
  return {
    asked,
    ask: async (request: ConfirmRequest) => {
      asked.push(request);
      return confirm as never;
    },
    ...extra,
  } as OperationUi & { asked: ConfirmRequest[] };
}

// The app's `resolveConflicts`: opens the dialog and returns.
const resolver = () => async (): Promise<"resolved"> => "resolved";

const shown = () => get(notifications);
const last = (): Shown => shown()[shown().length - 1];
const titles = () => shown().map(({ title }) => title);
const labels = (notification: Shown | undefined) =>
  notification?.actions?.map(({ label }) => label);
const opened = (path = "/work/rama") => ({ path }) as RepoInfo;
const click = (notification: Shown, label: string) =>
  notification.actions?.find((action) => action.label === label)?.run();

beforeEach(() => {
  for (const { id } of shown()) dismiss(id);
});

describe("ongoing actions", () => {
  const names = (current: RepoState) =>
    ongoingActions(current, { abort() {}, continue() {}, skip() {} }).map((item) =>
      item && "label" in item ? item.label : null,
    );

  test("only_those_of_the_current_state_appear", () => {
    expect(names(NORMAL)).toEqual([]);
    expect(names(REBASING)).toEqual(["Abort Rebase", "Continue Rebase", "Skip Commit"]);
    expect(names(state("merging"))).toEqual(["Abort Merge"]);
    expect(names(state("cherryPicking"))).toEqual(["Abort Cherry-Pick"]);
    expect(names(state("reverting"))).toEqual(["Abort Revert"]);
  });

  test("they_call_what_they_should", () => {
    const handlers = { abort: vi.fn(), continue: vi.fn(), skip: vi.fn() };
    const [abort, resume, skip] = ongoingActions(REBASING, handlers) as { run: () => void }[];

    abort.run();
    resume.run();
    skip.run();

    expect(handlers.abort).toHaveBeenCalledWith("rebasing");
    expect(handlers.continue).toHaveBeenCalledTimes(1);
    expect(handlers.skip).toHaveBeenCalledTimes(1);
  });

  test("without_handlers_they_come_out_disabled", () => {
    const items = ongoingActions(REBASING) as { disabled?: boolean }[];

    expect(items.map((item) => item.disabled)).toEqual([true, true, true]);
  });

  test("git_menu_shows_the_merge_group_only_while_merging", () => {
    expect(gitMenuEnabled(NORMAL)).toEqual(["Git.Merge", "Git.Rebase"]);
    expect(gitMenuEnabled(state("merging"))).toEqual(["Git.Merge.Abort"]);
    expect(gitMenuEnabled(REBASING)).toEqual([
      "Git.MainMenu.RebaseActions",
      "Git.Rebase.Abort",
      "Git.Rebase.Continue",
      "Git.Rebase.Skip",
    ]);
    expect(gitMenuEnabled(state("cherryPicking"))).toEqual(["Git.CherryPick.Abort"]);
    expect(gitMenuEnabled(state("reverting"))).toEqual(["Git.Revert.Abort"]);
  });

  test("resolve_conflicts_only_shows_with_conflicts", () => {
    expect(gitMenuEnabled(NORMAL, false)).toEqual(["Git.Merge", "Git.Rebase"]);
    expect(gitMenuEnabled(NORMAL, true)).toEqual(["Git.Merge", "Git.Rebase", "Git.ResolveConflicts"]);
    expect(gitMenuEnabled(state("merging"), true)).toEqual([
      "Git.Merge.Abort",
      "Git.ResolveConflicts",
    ]);
    expect(gitMenuEnabled(REBASING, true)).toContain("Git.ResolveConflicts");
    expect(gitMenuEnabled(REBASING, false)).not.toContain("Git.ResolveConflicts");
    expect(ongoingActionFor("Git.ResolveConflicts")).toEqual({ do: "resolve" });
  });

  test("each_menu_id_knows_what_it_does", () => {
    expect(ongoingActionFor("Git.Merge.Abort")).toEqual({ do: "abort", operation: "merging" });
    expect(ongoingActionFor("Git.Rebase.Abort")).toEqual({ do: "abort", operation: "rebasing" });
    expect(ongoingActionFor("Git.CherryPick.Abort")).toEqual({
      do: "abort",
      operation: "cherryPicking",
    });
    expect(ongoingActionFor("Git.Revert.Abort")).toEqual({ do: "abort", operation: "reverting" });
    expect(ongoingActionFor("Git.Rebase.Continue")).toEqual({ do: "continue" });
    expect(ongoingActionFor("Git.Rebase.Skip")).toEqual({ do: "skip" });
    expect(ongoingActionFor("Git.Fetch")).toBeNull();
  });
});

describe("abort", () => {
  test("abort_asks_for_the_intellij_confirmation", async () => {
    const backend = api({
      state: vi.fn(async () => state("merging")),
      abort: vi.fn(async (): Promise<Aborted> => "merge"),
    });
    const dialogs = ui();
    const flow = operationFlow(dialogs, backend);
    flow.repositoryChanged(opened());

    await flow.abortOperation("merging");

    expect(dialogs.asked).toEqual([
      { kind: "confirm", title: "Abort Merge", message: "Abort merge?", ok: "Abort" },
    ]);
    // The HEAD the dialog saw is the one passed back.
    expect(backend.abort).toHaveBeenCalledWith("merging", HEAD);
    expect(last().title).toBe("Merge abort succeeded");
  });

  test("the_rebase_says_which_repository", async () => {
    const dialogs = ui();
    const flow = operationFlow(dialogs, api());
    flow.repositoryChanged(opened("/work/rama"));

    await flow.abortOperation("rebasing");

    expect(dialogs.asked).toEqual([
      { kind: "confirm", title: "Abort Rebase", message: "Abort rebase in rama?", ok: "Abort" },
    ]);
    expect(last().title).toBe("Abort rebase succeeded");
  });

  test("cherry_pick_and_revert_use_their_name", async () => {
    const dialogs = ui();
    const backend = api({
      state: vi.fn(async () => state("cherryPicking")),
      abort: vi.fn(async (): Promise<Aborted> => "cherryPick"),
    });
    const flow = operationFlow(dialogs, backend);

    await flow.abortOperation("cherryPicking");

    expect(dialogs.asked[0]).toMatchObject({
      title: "Abort Cherry-pick",
      message: "Abort cherry-pick?",
    });
    expect(last().title).toBe("Cherry-pick abort succeeded");
    expect(ABORTED.revert).toBe("Revert abort succeeded");
  });

  test("cancelling_the_confirmation_does_not_abort", async () => {
    const backend = api();
    const flow = operationFlow(ui(null), backend);

    await flow.abortOperation("rebasing");

    expect(backend.abort).not.toHaveBeenCalled();
    expect(shown()).toEqual([]);
  });

  test("if_that_operation_is_gone_it_neither_asks_nor_aborts", async () => {
    const backend = api({ state: vi.fn(async () => NORMAL) });
    const dialogs = ui();
    const flow = operationFlow(dialogs, backend);

    await flow.abortOperation("merging");
    await flow.abortOperation("rebasing");

    expect(dialogs.asked).toEqual([]);
    expect(backend.abort).not.toHaveBeenCalled();
    // A rebase that is no longer there is reported, like `GitRebaseUtils.abort`.
    expect(shown()).toHaveLength(1);
    expect(last()).toMatchObject({
      kind: "error",
      title: "Cannot abort rebase",
      body: "No rebase in progress",
    });
  });

  test("having_nothing_to_abort_announces_no_success", async () => {
    const backend = api({ abort: vi.fn(async (): Promise<Aborted> => "nothing") });

    await operationFlow(ui(), backend).abortOperation("rebasing");

    expect(shown()).toEqual([]);
  });

  test("a_failure_when_aborting_is_reported", async () => {
    const backend = api({
      abort: vi.fn().mockRejectedValue({ kind: "failed", message: "git failed" }),
    });

    await operationFlow(ui(), backend).abortOperation("rebasing");

    expect(last()).toMatchObject({ kind: "error", title: "Abort rebase failed", body: "git failed" });
  });

  test("the_changed_head_fails_with_its_text", async () => {
    const backend = api({
      state: vi.fn(async () => state("merging")),
      abort: vi
        .fn()
        .mockRejectedValue({ kind: "failed", message: "Repository has already been changed" }),
    });

    await operationFlow(ui(), backend).abortOperation("merging");

    expect(last()).toMatchObject({
      title: "Merge abort failed",
      body: "Repository has already been changed",
    });
  });
});

describe("continue and skip", () => {
  test("continue_with_conflicts_calls_onconflicts", async () => {
    const backend = api({ continue: vi.fn(async () => CONFLICTS) });
    const onconflicts = vi.fn(async () => {});

    await operationFlow(ui(), backend).continueRebase(onconflicts);

    expect(onconflicts).toHaveBeenCalledTimes(1);
    expect(shown()).toEqual([]);
  });

  test("continue_rebase_with_conflicts_opens_conflicts", async () => {
    const backend = api({
      continue: vi.fn().mockResolvedValueOnce(CONFLICTS).mockResolvedValue(DONE),
    });
    const resolveConflicts = vi.fn(resolver());

    await operationFlow(ui(true, { resolveConflicts }), backend).continueRebase();

    expect(resolveConflicts).toHaveBeenCalledTimes(1);
    expect(shown()).toEqual([]);
    const [options] = resolveConflicts.mock.calls[0] as unknown as [ResolveOptions];
    await options.onresolved?.();
    expect(backend.continue).toHaveBeenCalledTimes(2);
    expect(last().title).toBe("Rebase successful");
  });

  test("skip_with_conflicts_opens_conflicts_and_if_any_remain_leaves_the_card", async () => {
    const backend = api({ skip: vi.fn(async () => CONFLICTS) });
    const resolveConflicts = vi.fn(resolver());

    await operationFlow(ui(true, { resolveConflicts }), backend).skipCommit();

    expect(resolveConflicts).toHaveBeenCalledTimes(1);
    const [options] = resolveConflicts.mock.calls[0] as unknown as [ResolveOptions];
    options.onpending?.();
    expect(last().title).toBe("Rebase stopped due to conflicts");
    expect(labels(last())).toEqual(["Resolve…", "Continue", "Abort"]);
  });

  test("skip_with_conflicts_calls_onconflicts", async () => {
    const backend = api({ skip: vi.fn(async () => CONFLICTS) });
    const onconflicts = vi.fn(async () => {});

    await operationFlow(ui(), backend).skipCommit(onconflicts);

    expect(backend.skip).toHaveBeenCalled();
    expect(onconflicts).toHaveBeenCalledTimes(1);
  });

  test("without_onconflicts_the_conflicts_leave_the_rebase_notification", async () => {
    const backend = api({ continue: vi.fn(async () => CONFLICTS) });

    await operationFlow(ui(), backend).continueRebase();

    expect(last().title).toBe("Rebase stopped due to conflicts");
  });

  test("finishing_notifies_with_the_branch", async () => {
    const backend = api({ state: vi.fn(async () => state(null, "main")) });

    await operationFlow(ui(), backend).continueRebase(vi.fn());

    expect(last()).toMatchObject({ kind: "info", title: "Rebase successful", body: "Rebased main" });
  });

  test("stopped_for_editing_offers_continue_and_abort", async () => {
    const backend = api({ skip: vi.fn(async () => ({ kind: "editing" }) as const) });

    await operationFlow(ui(), backend).skipCommit(vi.fn());

    expect(last().title).toBe("Rebase stopped for editing");
    expect(labels(last())).toEqual(["Continue", "Abort"]);
  });

  test("a_failure_when_continuing_says_continue_rebase_failed", async () => {
    const backend = api({ continue: vi.fn(async () => ({ kind: "failed", reason: "dirty" }) as const) });

    await operationFlow(ui(), backend).continueRebase(vi.fn());

    expect(last()).toMatchObject({
      kind: "error",
      title: "Continue rebase failed",
      body: "dirty",
    });
    expect(labels(last())).toEqual(["Retry", "Abort"]);
  });

  test("without_a_rebase_in_progress_says_cannot_continue", async () => {
    const backend = api({
      continue: vi.fn().mockRejectedValue({ kind: "failed", message: "No rebase in progress" }),
    });

    await operationFlow(ui(), backend).continueRebase(vi.fn());

    expect(last()).toMatchObject({
      kind: "error",
      title: "Cannot continue rebase",
      body: "No rebase in progress",
    });
  });

  test("another_error_when_continuing_is_reported_without_buttons", async () => {
    const backend = api({
      skip: vi.fn().mockRejectedValue({ kind: "failed", message: "git is missing" }),
    });

    await operationFlow(ui(), backend).skipCommit(vi.fn());

    expect(last()).toMatchObject({ kind: "error", title: "Continue rebase failed", body: "git is missing" });
    expect(last().actions).toBeUndefined();
  });

  test("cancelling_is_not_a_failure", async () => {
    const backend = api({
      continue: vi.fn().mockRejectedValue({ kind: "cancelled", message: "cancelled" }),
    });

    await operationFlow(ui(), backend).continueRebase(vi.fn());

    expect(shown()).toEqual([]);
  });
});

describe("several commits with conflicts", () => {
  const session = (...paths: string[]) => ({
    conflicts: paths.map((path) => ({
      path,
      yours: "modified" as const,
      theirs: "modified" as const,
      kind: "both modified" as const,
    })),
    reversed: true,
    yoursColumn: "Yours (feature)",
    theirsColumn: "Theirs (main)",
    description: "",
  });

// With the real `conflictsFlow`: each stop opens its dialog and, once resolved, continues.
  async function rebaseStopping(times: number) {
    const outcomes: RebaseOutcome[] = [...Array(times).fill(CONFLICTS), DONE];
    const backend = api({ continue: vi.fn(async () => outcomes.shift() ?? DONE) });
    const reads: ReturnType<typeof session>[] = [];
    for (let stop = 0; stop < times; stop++) reads.push(session(`f${stop}.txt`), session());
    const conflictsApi: ConflictsApi = { session: vi.fn(async () => reads.shift() ?? session()) };
    const opened: ConflictsRequest[] = [];
    const conflicts = conflictsFlow(
      {
        ask: async <T,>(request: ConflictsRequest) => {
          opened.push(request);
          return null as T | null;
        },
      },
      conflictsApi,
    );
    const flow = operationFlow(ui(true, { resolveConflicts: conflicts.resolveConflicts }), backend);
    await flow.continueRebase();
    return { backend, opened, conflicts };
  }

  test("two_consecutive_stops_open_conflicts_twice_and_finish", async () => {
    const { backend, opened } = await rebaseStopping(2);

    expect(opened).toHaveLength(2);
    expect(backend.continue).toHaveBeenCalledTimes(3);
    expect(last().title).toBe("Rebase successful");
    expect(titles()).not.toContain("Rebase stopped due to conflicts");
  });

  test("three_consecutive_stops_do_not_block_either", async () => {
    const { backend, opened } = await rebaseStopping(3);

    expect(opened).toHaveLength(3);
    expect(backend.continue).toHaveBeenCalledTimes(4);
    expect(last().title).toBe("Rebase successful");
  });

  test("after_resolving_the_guard_is_free_for_another_resolve_conflicts", async () => {
    const { opened, conflicts } = await rebaseStopping(1);

    await conflicts.resolveConflicts();

    // The session is already empty: there is no dialog, but it does not hang either.
    expect(opened).toHaveLength(1);
  });
});

describe("stopped rebase", () => {
  test("stopped_rebase_notifies_with_resolve_continue_and_abort", () => {
    const flow = operationFlow(ui(), api());

    flow.notifyRebaseStopped(CONFLICTS);

    expect(last()).toMatchObject({
      kind: "warning",
      sticky: true,
      title: "Rebase stopped due to conflicts",
    });
    expect(labels(last())).toEqual(["Resolve…", "Continue", "Abort"]);
  });

  test("resolve_calls_the_apps_resolver", () => {
    const resolveConflicts = vi.fn(resolver());
    const flow = operationFlow(ui(true, { resolveConflicts }), api());
    flow.notifyRebaseStopped(CONFLICTS);

    click(last(), "Resolve…");

    expect(resolveConflicts).toHaveBeenCalledTimes(1);
  });

  test("resolving_from_the_card_continues_the_rebase_and_without_resolving_leaves_it", async () => {
    const backend = api();
    const resolveConflicts = vi.fn(resolver());
    const flow = operationFlow(ui(true, { resolveConflicts }), backend);
    flow.notifyRebaseStopped(CONFLICTS);
    const card = last();

    click(card, "Resolve…");
    const [options] = resolveConflicts.mock.calls[0] as unknown as [ResolveOptions];
    options.onpending?.();
    expect(shown()).toEqual([card]);

    await options.onresolved?.();
    expect(backend.continue).toHaveBeenCalledTimes(1);
    expect(last().title).toBe("Rebase successful");
  });

  test("everything_resolved_from_the_rebase_notification_asks_continue_rebase", async () => {
    const backend = api();
    const dialogs = ui(true, { resolveConflicts: vi.fn(resolver()) });
    const flow = operationFlow(dialogs, backend);
    flow.notifyRebaseStopped(CONFLICTS);
    click(last(), "Resolve…");
    const [options] = vi.mocked(dialogs.resolveConflicts!).mock.calls[0] as unknown as [ResolveOptions];

    await options.onnothing?.();

    expect(dialogs.asked).toEqual([
      {
        kind: "confirm",
        title: "Resolve Conflicts",
        message: "All conflicts have been resolved. Do you want to continue rebase?",
        ok: "Continue Rebase",
      },
    ]);
    expect(backend.continue).toHaveBeenCalledTimes(1);
    expect(titles()).not.toContain("Rebase stopped due to conflicts");
  });

  test("if_the_continue_rebase_question_is_cancelled_it_does_not_continue", async () => {
    const backend = api();
    const dialogs = ui(null, { resolveConflicts: vi.fn(resolver()) });
    const flow = operationFlow(dialogs, backend);
    flow.notifyRebaseStopped(CONFLICTS);
    click(last(), "Resolve…");
    const [options] = vi.mocked(dialogs.resolveConflicts!).mock.calls[0] as unknown as [ResolveOptions];

    await options.onnothing?.();

    expect(backend.continue).not.toHaveBeenCalled();
    expect(titles()).toContain("Rebase stopped due to conflicts");
  });

  test("continue_and_abort_of_the_notification_do_their_job", async () => {
    const backend = api();
    const dialogs = ui();
    const flow = operationFlow(dialogs, backend);
    flow.notifyRebaseStopped(CONFLICTS);
    const stopped = last();

    click(stopped, "Continue");
    await vi.waitFor(() => expect(backend.continue).toHaveBeenCalled());
    click(stopped, "Abort");
    await vi.waitFor(() => expect(backend.abort).toHaveBeenCalledWith("rebasing", HEAD));

    expect(dialogs.asked[0].title).toBe("Abort Rebase");
  });

  test("a_failure_when_starting_says_rebase_failed", () => {
    const flow = operationFlow(ui(), api());

    flow.notifyRebaseStopped({ kind: "failed", reason: "dirty" });

    expect(last()).toMatchObject({ kind: "error", title: "Rebase failed", body: "dirty" });
    expect(labels(last())).toEqual(["Retry", "Abort"]);
  });

  test("untracked_files_are_listed", () => {
    const flow = operationFlow(ui(), api());

    flow.notifyRebaseStopped({ kind: "untracked", reason: { files: ["a.txt", "b.txt"] } });

    expect(last()).toMatchObject({
      kind: "error",
      title: "Untracked Files Prevent Rebase",
      body: "Move or commit them before Rebase\na.txt\nb.txt",
    });
    expect(labels(last())).toEqual(["Retry", "Abort"]);
  });

  test("done_is_not_a_stopped_rebase", () => {
    expect(rebaseStoppedNotification(DONE, false)).toBeNull();
  });

  test("changing_repository_withdraws_the_cards", () => {
    const flow = operationFlow(ui(), api());
    flow.notifyRebaseStopped(CONFLICTS);
    flow.notifyRebaseStopped({ kind: "editing" });
    expect(titles()).toHaveLength(2);

    flow.repositoryChanged(opened());

    expect(shown()).toEqual([]);
  });

  test("a_rebase_that_stops_in_another_repository_offers_no_buttons", async () => {
    let finish = (_outcome: RebaseOutcome) => {};
    const backend = api({
      continue: vi.fn(() => new Promise<RebaseOutcome>((resolve) => (finish = resolve))),
    });
    const flow = operationFlow(ui(), backend);
    const running = flow.continueRebase(vi.fn());
    await vi.waitFor(() => expect(backend.continue).toHaveBeenCalled());

    flow.repositoryChanged(opened("/work/other"));
    finish({ kind: "editing" });
    await running;

    expect(last().title).toBe("Rebase stopped for editing");
    expect(last().actions).toBeUndefined();
    expect(last().body).toContain("previous repository");
  });
});

describe("what was saved before the rebase", () => {
  const SAVED = { sha: "abc", message: "Uncommitted changes before rebase" };

  test("stopped_with_saved_changes_says_so", () => {
    const flow = operationFlow(ui());

    flow.notifyRebaseStopped(CONFLICTS, false, [SAVED]);

    expect(last()).toMatchObject({
      title: "Rebase stopped due to conflicts",
      body: "Local changes were stashed before rebase.",
    });
    const failed = rebaseStoppedNotification({ kind: "failed", reason: "bad" }, false, undefined, true);
    expect(failed?.body).toBe("bad\nLocal changes were stashed before rebase.");
  });

  test("when_the_stopped_rebase_finishes_it_gives_back_what_was_saved", async () => {
    const restoreSaved = vi.fn(async () => {});
    const flow = operationFlow(ui(true, { restoreSaved }), api());
    flow.notifyRebaseStopped(CONFLICTS, false, [SAVED]);

    await flow.continueRebase();

    expect(restoreSaved).toHaveBeenCalledWith([SAVED]);
    expect(titles()).toContain("Rebase successful");
    // Already given back: another rebase that finishes does not try again.
    await flow.continueRebase();
    expect(restoreSaved).toHaveBeenCalledTimes(1);
  });

  test("when_the_stopped_rebase_is_aborted_it_gives_back_what_was_saved", async () => {
    const restoreSaved = vi.fn(async () => {});
    const flow = operationFlow(ui(true, { restoreSaved }), api());
    flow.notifyRebaseStopped(CONFLICTS, false, [SAVED]);

    await flow.abortOperation("rebasing");

    expect(restoreSaved).toHaveBeenCalledWith([SAVED]);
  });

  test("with_nothing_saved_nothing_is_given_back_or_mentioned", async () => {
    const restoreSaved = vi.fn(async () => {});
    const flow = operationFlow(ui(true, { restoreSaved }), api());
    flow.notifyRebaseStopped(CONFLICTS, false, []);

    expect(last().body).toBeUndefined();
    await flow.continueRebase();
    expect(restoreSaved).not.toHaveBeenCalled();
  });

  test("if_the_rebase_finished_outside_abort_gives_back_what_was_saved", async () => {
    const restoreSaved = vi.fn(async () => {});
    const flow = operationFlow(ui(true, { restoreSaved }), api({ state: vi.fn(async () => NORMAL) }));
    flow.notifyRebaseStopped(CONFLICTS, false, [SAVED]);

    expect(await flow.abortOperation("rebasing")).toBe(true);

    expect(titles()).toContain("Cannot abort rebase");
    expect(restoreSaved).toHaveBeenCalledWith([SAVED]);
  });

  test("if_the_rebase_finished_outside_continue_and_skip_give_back_what_was_saved", async () => {
    const gone = vi.fn().mockRejectedValue({ kind: "failed", message: "No rebase in progress" });
    const restoreSaved = vi.fn(async () => {});
    const flow = operationFlow(ui(true, { restoreSaved }), api({ continue: gone, skip: gone }));
    flow.notifyRebaseStopped(CONFLICTS, false, [SAVED]);

    await flow.continueRebase();

    expect(titles()).toContain("Cannot continue rebase");
    expect(restoreSaved).toHaveBeenCalledWith([SAVED]);

    const other = vi.fn(async () => {});
    const skipping = operationFlow(ui(true, { restoreSaved: other }), api({ skip: gone }));
    skipping.notifyRebaseStopped(CONFLICTS, false, [SAVED]);
    await skipping.skipCommit();
    expect(other).toHaveBeenCalledWith([SAVED]);
  });

  test("another_failure_when_continuing_gives_back_nothing", async () => {
    const failing = vi.fn().mockRejectedValue({ kind: "failed", message: "index.lock" });
    const restoreSaved = vi.fn(async () => {});
    const flow = operationFlow(ui(true, { restoreSaved }), api({ continue: failing }));
    flow.notifyRebaseStopped(CONFLICTS, false, [SAVED]);

    await flow.continueRebase();

    expect(restoreSaved).not.toHaveBeenCalled();
  });

  test("repo_changed_without_a_rebase_gives_back_what_was_saved_and_with_a_rebase_does_not", async () => {
    const restoreSaved = vi.fn(async () => {});
    const state = vi.fn().mockResolvedValueOnce(REBASING).mockResolvedValue(NORMAL);
    const flow = operationFlow(ui(true, { restoreSaved }), api({ state }));

    // With nothing saved it is not even looked at.
    await flow.repositoryRefreshed();
    expect(state).not.toHaveBeenCalled();

    flow.notifyRebaseStopped(CONFLICTS, false, [SAVED]);
    await flow.repositoryRefreshed();
    expect(restoreSaved).not.toHaveBeenCalled();

    await flow.repositoryRefreshed();
    expect(restoreSaved).toHaveBeenCalledWith([SAVED]);
    await flow.repositoryRefreshed();
    expect(restoreSaved).toHaveBeenCalledTimes(1);
  });

  test("changing_repository_forgets_it", async () => {
    const restoreSaved = vi.fn(async () => {});
    const flow = operationFlow(ui(true, { restoreSaved }), api());
    flow.notifyRebaseStopped(CONFLICTS, false, [SAVED]);
    flow.repositoryChanged(opened("/work/other"));

    await flow.continueRebase();

    expect(restoreSaved).not.toHaveBeenCalled();
  });
});
