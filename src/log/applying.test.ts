import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { get } from "svelte/store";
import type { ApplyResult, Changelists, PathsResult, RepoState } from "../lib/api";
import type { ResolveOptions } from "../conflicts/flow";
import { dismiss, notifications, type Shown } from "../lib/notifications";
import {
  applyingFlow,
  isApplying,
  isApplyingPaths,
  type ApplyingApi,
  type ApplyingUi,
} from "./applying";
import type { VcsAction } from "./contextMenu";

const H1 = "11111111".repeat(5);
const H2 = "22222222".repeat(5);
const H3 = "33333333".repeat(5);
const HEAD = "abcdef01".repeat(5);
const NEXT = "fedcba98".repeat(5);
const STATE: RepoState = { operation: null, branch: "main", label: "main", head: HEAD };
const COMMITS = [
  { hash: H3, subject: "three" },
  { hash: H2, subject: "two" },
  { hash: H1, subject: "one" },
];

const LISTS: Changelists = {
  lists: [
    { id: "default", name: "Changes", comment: "" },
    { id: "other", name: "Other", comment: "" },
  ],
  active: "default",
  assignments: {},
  hunkAssignments: {},
};

const done = (overrides: Partial<ApplyResult> = {}): ApplyResult => ({
  applied: [],
  skipped: [],
  stopped: null,
  remaining: [],
  ...overrides,
});

function api(overrides: Partial<ApplyingApi> = {}): ApplyingApi {
  return {
    state: vi.fn(async () => STATE),
    stagingArea: vi.fn(async () => false),
    apply: vi.fn(async () => done({ applied: [H1] })),
    commitAfterConflicts: vi.fn(async () => "committed" as const),
    applyPaths: vi.fn(async () => ({ kind: "applied" }) as PathsResult),
    changelists: vi.fn(async () => LISTS),
    subject: vi.fn(async () => "one"),
    moveToChangelist: vi.fn(async () => {}),
    ...overrides,
  };
}

function ui(resolve?: (options?: ResolveOptions) => Promise<"resolved" | "pending">) {
  return {
    chooseChangelist: vi.fn(async (_lists: unknown, initial: string) => ({
      name: initial,
      setActive: false,
    })),
    resolveConflicts: vi.fn(resolve ?? (async () => "pending" as const)),
    abortOperation: vi.fn(async () => true),
    showStaging: vi.fn(),
  } satisfies ApplyingUi;
}

// A Conflicts dialog in which the user resolved everything.
const resolving = (steps: "onresolved" | "onnothing" = "onresolved") =>
  ui(async (options) => {
    await options?.[steps]?.();
    return "resolved";
  });

const shown = () => get(notifications);
const last = (): Shown => shown()[shown().length - 1];
const pick = (...commits: typeof COMMITS): VcsAction => ({ kind: "cherryPick", commits });
const revert = (...commits: typeof COMMITS): VcsAction => ({ kind: "revert", commits });

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  shown().forEach(({ id }) => dismiss(id));
  vi.useRealTimers();
});

describe("result notices", () => {
  test("cherry_pick_successful_notification_with_one_line_per_commit", async () => {
    const backend = api({ apply: vi.fn(async () => done({ applied: [H1, H2] })) });
    const flow = applyingFlow(ui(), backend);

    await flow.run(pick(COMMITS[1], COMMITS[2]));

    expect(last()).toMatchObject({
      kind: "info",
      title: "Cherry-pick successful",
      body: "11111111 one\n22222222 two",
    });
    // From the log to the backend, as is: it is the backend that orders them.
    expect(backend.apply).toHaveBeenCalledWith("cherryPick", [H2, H1], HEAD);
  });

  test("a_revert_says_revert_successful", async () => {
    const backend = api({ apply: vi.fn(async () => done({ applied: [H3] })) });
    const flow = applyingFlow(ui(), backend);

    await flow.run(revert(COMMITS[0]));

    expect(last()).toMatchObject({ title: "Revert successful", body: "33333333 three" });
    expect(backend.apply).toHaveBeenCalledWith("revert", [H3], HEAD);
  });

  test("applied_2_commits_from_3_with_skipped", async () => {
    const backend = api({
      apply: vi.fn(async () => done({ applied: [H1, H2], skipped: [H3] })),
    });
    const flow = applyingFlow(ui(), backend);

    await flow.run(pick(...COMMITS));

    expect(last()).toMatchObject({
      kind: "info",
      title: "Applied 2 commits from 3",
      body: "11111111 one\n22222222 two\n\n33333333 was skipped, because all changes have already been applied.",
    });
  });

  test("one_applied_and_several_skipped_uses_were_and_the_singular", async () => {
    const backend = api({
      apply: vi.fn(async () => done({ applied: [H1], skipped: [H2, H3] })),
    });
    const flow = applyingFlow(ui(), backend);

    await flow.run(pick(...COMMITS));

    expect(last().title).toBe("Applied commit from 3");
    expect(last().body).toContain("22222222, 33333333 were skipped, because");
  });

  test("a_revert_with_skipped_says_reverted", async () => {
    const backend = api({
      apply: vi.fn(async () => done({ applied: [H3, H2], skipped: [H1] })),
    });
    const flow = applyingFlow(ui(), backend);

    await flow.run(revert(...COMMITS));

    expect(last().title).toBe("Reverted 2 commits from 3");
    expect(last().body).toContain("11111111 was skipped, because all changes have already been reverted.");
  });

  test("nothing_to_cherry_pick", async () => {
    const backend = api({ apply: vi.fn(async () => done({ skipped: [H1, H2] })) });
    const flow = applyingFlow(ui(), backend);

    await flow.run(pick(COMMITS[1], COMMITS[2]));

    expect(last()).toMatchObject({
      kind: "warning",
      title: "Nothing to cherry-pick",
      body: "All changes from 11111111, 22222222 have already been applied",
    });
  });

  test("nothing_to_revert", async () => {
    const backend = api({ apply: vi.fn(async () => done({ skipped: [H3] })) });
    const flow = applyingFlow(ui(), backend);

    await flow.run(revert(COMMITS[0]));

    expect(last()).toMatchObject({
      title: "Nothing to revert",
      body: "All changes from 33333333 have already been reverted",
    });
  });

  test("the_repository_subject_is_shown_as_text", async () => {
    const backend = api({ apply: vi.fn(async () => done({ applied: [H1] })) });
    const flow = applyingFlow(ui(), backend);

    await flow.run(pick({ hash: H1, subject: "<b>bold</b> &amp;" }));

    expect(last().body).toBe("11111111 <b>bold</b> &amp;");
  });
});

describe("conflicts", () => {
  const conflict = (remaining: string[] = [], applied: string[] = []) =>
    done({ applied, stopped: { kind: "conflicts", reason: { hash: H2 } }, remaining });

  test("with_conflicts_opens_conflicts_and_on_resolving_makes_the_commit", async () => {
    const backend = api({ apply: vi.fn(async () => conflict()) });
    const dialogs = resolving();
    const flow = applyingFlow(dialogs, backend);

    await flow.run(pick(COMMITS[1], COMMITS[2]));

    expect(dialogs.resolveConflicts).toHaveBeenCalledTimes(1);
    expect(backend.commitAfterConflicts).toHaveBeenCalledWith("cherryPick", H2);
    expect(last()).toMatchObject({
      title: "Cherry-pick successful",
      body: "22222222 two",
    });
  });

  test("after_resolving_continues_with_the_remaining_commits", async () => {
    const apply = vi
      .fn<ApplyingApi["apply"]>()
      .mockResolvedValueOnce(conflict([H1], [H3]))
      .mockResolvedValueOnce(done({ applied: [H1] }));
    const state = vi
      .fn<ApplyingApi["state"]>()
      .mockResolvedValueOnce(STATE)
      .mockResolvedValueOnce({ ...STATE, head: NEXT });
    const backend = api({ apply, state });
    const flow = applyingFlow(resolving(), backend);

    await flow.run(pick(...COMMITS));

    // The HEAD of the second time is that of the commit that has just been made.
    expect(apply).toHaveBeenNthCalledWith(2, "cherryPick", [H1], NEXT);
    expect(last()).toMatchObject({
      title: "Cherry-pick successful",
      body: "33333333 three\n22222222 two\n11111111 one",
    });
  });

  test("resolved_same_as_head_counts_as_skipped", async () => {
    const backend = api({
      apply: vi.fn(async () => conflict([], [H3])),
      commitAfterConflicts: vi.fn(async () => "empty" as const),
    });
    const flow = applyingFlow(resolving(), backend);

    await flow.run(pick(COMMITS[0], COMMITS[1]));

    expect(last().title).toBe("Applied commit from 2");
    expect(last().body).toContain("22222222 was skipped");
  });

  test("with_nothing_to_resolve_also_makes_the_commit", async () => {
    const backend = api({ apply: vi.fn(async () => conflict()) });
    const flow = applyingFlow(resolving("onnothing"), backend);

    await flow.run(pick(COMMITS[1]));

    expect(backend.commitAfterConflicts).toHaveBeenCalledWith("cherryPick", H2);
    expect(last().title).toBe("Cherry-pick successful");
  });

  test("a_revert_with_conflicts_makes_the_revert_commit", async () => {
    const backend = api({ apply: vi.fn(async () => conflict()) });
    const flow = applyingFlow(resolving(), backend);

    await flow.run(revert(COMMITS[1]));

    expect(backend.commitAfterConflicts).toHaveBeenCalledWith("revert", H2);
    expect(last().title).toBe("Revert successful");
  });

  test("with_staging_area_does_not_commit_and_leaves_the_staging_view", async () => {
    const backend = api({
      apply: vi.fn(async () => conflict([H1], [H3])),
      stagingArea: vi.fn(async () => true),
    });
    const dialogs = resolving();
    const flow = applyingFlow(dialogs, backend);

    await flow.run(pick(...COMMITS));

    expect(backend.commitAfterConflicts).not.toHaveBeenCalled();
    expect(backend.apply).toHaveBeenCalledTimes(1);
    expect(dialogs.showStaging).toHaveBeenCalledTimes(1);
    expect(shown()).toHaveLength(0);
  });

  test("pending_conflicts_warn_performed_with_conflicts_with_resolve_and_abort_cherry_pick", async () => {
    const backend = api({ apply: vi.fn(async () => conflict()) });
    const dialogs = ui();
    const flow = applyingFlow(dialogs, backend);

    await flow.run(pick(COMMITS[1]));

    const card = last();
    expect(card).toMatchObject({
      kind: "warning",
      title: "Cherry-pick was performed with conflicts",
      body: "22222222 two\nThere are unresolved conflicts in the working tree.",
    });
    expect(card.actions?.map(({ label }) => label)).toEqual(["Resolve…", "Abort Cherry-pick"]);
    expect(backend.commitAfterConflicts).not.toHaveBeenCalled();

    card.actions?.[0].run();
    expect(dialogs.resolveConflicts).toHaveBeenLastCalledWith();

    card.actions?.[1].run();
    await vi.advanceTimersByTimeAsync(0);
    expect(dialogs.abortOperation).toHaveBeenCalledWith("cherryPicking");
    expect(shown().some(({ id }) => id === card.id)).toBe(false);
  });

  test("an_abort_that_is_cancelled_leaves_the_card", async () => {
    const backend = api({ apply: vi.fn(async () => conflict()) });
    const dialogs = ui();
    dialogs.abortOperation.mockResolvedValue(false);
    const flow = applyingFlow(dialogs, backend);
    await flow.run(pick(COMMITS[1]));
    const card = last();

    card.actions?.[1].run();
    await vi.advanceTimersByTimeAsync(0);

    expect(shown().some(({ id }) => id === card.id)).toBe(true);
  });

  test("a_pending_revert_offers_abort_revert", async () => {
    const backend = api({ apply: vi.fn(async () => conflict()) });
    const dialogs = ui();
    const flow = applyingFlow(dialogs, backend);

    await flow.run(revert(COMMITS[1]));

    expect(last().title).toBe("Revert was performed with conflicts");
    expect(last().actions?.map(({ label }) => label)).toEqual(["Resolve…", "Abort Revert"]);
    last().actions?.[1].run();
    expect(dialogs.abortOperation).toHaveBeenCalledWith("reverting");
  });

  test("however_succeeded_for_the_following_commits", async () => {
    const backend = api({ apply: vi.fn(async () => conflict([], [H3])) });
    const flow = applyingFlow(ui(), backend);

    await flow.run(pick(COMMITS[0], COMMITS[1]));

    expect(last().body).toBe(
      [
        "22222222 two",
        "There are unresolved conflicts in the working tree.",
        "",
        "However, cherry-pick succeeded for the following commit:",
        "33333333 three",
      ].join("\n"),
    );
  });

  test("however_uses_the_plural_with_several", async () => {
    const backend = api({ apply: vi.fn(async () => conflict([], [H3, H1])) });
    const flow = applyingFlow(ui(), backend);

    await flow.run(pick(...COMMITS));

    expect(last().body).toContain("However, cherry-pick succeeded for the following commits:\n33333333 three\n11111111 one");
  });

  test("if_the_commit_fails_a_notice_with_retry_and_abort_remains", async () => {
    const commitAfterConflicts = vi
      .fn<ApplyingApi["commitAfterConflicts"]>()
      .mockRejectedValueOnce({ kind: "git", message: "the hook rejected the commit" })
      .mockResolvedValueOnce("committed");
    const backend = api({ apply: vi.fn(async () => conflict()), commitAfterConflicts });
    const dialogs = resolving();
    const flow = applyingFlow(dialogs, backend);

    await flow.run(pick(COMMITS[1]));

    const card = last();
    expect(card).toMatchObject({ kind: "error", title: "Cherry-pick failed" });
    expect(card.body).toContain("22222222 two");
    expect(card.body).toContain("the hook rejected the commit");
    expect(card.actions?.map(({ label }) => label)).toEqual(["Retry", "Abort Cherry-pick"]);

    card.actions?.[0].run();
    await vi.advanceTimersByTimeAsync(0);

    expect(commitAfterConflicts).toHaveBeenCalledTimes(2);
    expect(last().title).toBe("Cherry-pick successful");
  });

  test("a_conflicts_question_closed_halfway_leaves_the_notice", async () => {
    // The session could not be read: `resolveConflicts` returns `pending` without calling anything.
    const backend = api({ apply: vi.fn(async () => conflict()) });
    const flow = applyingFlow(ui(async () => "pending"), backend);

    await flow.run(pick(COMMITS[1]));

    expect(last().title).toBe("Cherry-pick was performed with conflicts");
  });

  test("conflicts_resolved_elsewhere_are_not_forgotten_either", async () => {
    // The dialog was already open for something else: returns `resolved` without running our steps.
    const backend = api({ apply: vi.fn(async () => conflict()) });
    const flow = applyingFlow(ui(async () => "resolved"), backend);

    await flow.run(pick(COMMITS[1]));

    expect(last().title).toBe("Cherry-pick was performed with conflicts");
    expect(backend.commitAfterConflicts).not.toHaveBeenCalled();
  });
});

describe("other stops", () => {
  test("local_changes_that_would_be_overwritten_give_cherry_pick_failed", async () => {
    const backend = api({
      apply: vi.fn(async () =>
        done({
          applied: [H3],
          stopped: { kind: "localChanges", reason: { hash: H2, files: ["a.txt"] } },
          remaining: [H1],
        }),
      ),
    });
    const flow = applyingFlow(ui(), backend);

    await flow.run(pick(...COMMITS));

    expect(last()).toMatchObject({ kind: "error", title: "Cherry-pick failed" });
    expect(last().body).toBe(
      [
        "22222222 two",
        "Your local changes will be overwritten by cherry-pick.",
        "Commit your changes or stash them to proceed.",
        "",
        "However, cherry-pick succeeded for the following commit:",
        "33333333 three",
      ].join("\n"),
    );
  });

  test("a_revert_that_would_overwrite_changes_says_revert", async () => {
    const backend = api({
      apply: vi.fn(async () =>
        done({ stopped: { kind: "localChanges", reason: { hash: H3, files: [] } } }),
      ),
    });
    const flow = applyingFlow(ui(), backend);

    await flow.run(revert(COMMITS[0]));

    expect(last().title).toBe("Revert failed");
    expect(last().body).toContain("Your local changes will be overwritten by revert.");
  });

  test("unversioned_files_give_untracked_files_prevent", async () => {
    const backend = api({
      apply: vi.fn(async () =>
        done({
          applied: [H3],
          stopped: { kind: "untracked", reason: { hash: H2, files: ["n.txt", "m.txt"] } },
        }),
      ),
    });
    const flow = applyingFlow(ui(), backend);

    await flow.run(pick(COMMITS[0], COMMITS[1]));

    expect(last()).toMatchObject({ kind: "error", title: "Untracked Files Prevent Cherry-pick" });
    expect(last().body).toBe(
      [
        "Move or commit them before cherry-pick",
        "n.txt",
        "m.txt",
        "",
        "However, cherry-pick succeeded for the following commit:",
        "33333333 three",
      ].join("\n"),
    );
  });

  test("another_git_failure_shows_its_text", async () => {
    const backend = api({
      apply: vi.fn(async () =>
        done({ stopped: { kind: "failed", reason: { hash: H1, text: "fatal: something odd" } } }),
      ),
    });
    const flow = applyingFlow(ui(), backend);

    await flow.run(pick(COMMITS[2]));

    expect(last()).toMatchObject({ kind: "error", title: "Cherry-pick failed" });
    expect(last().body).toBe("11111111 one\nfatal: something odd");
  });

  test("a_backend_error_is_a_failure_notice", async () => {
    const backend = api({
      apply: vi.fn(async () => {
        throw { kind: "git", message: "Reverting merge commits is not allowed" };
      }),
    });
    const flow = applyingFlow(ui(), backend);

    await flow.run(revert(COMMITS[0]));

    expect(last()).toMatchObject({ kind: "error", title: "Revert failed" });
    expect(last().body).toContain("Reverting merge commits is not allowed");
  });

  test("a_cancelled_one_is_not_a_failure", async () => {
    const backend = api({
      apply: vi.fn(async () => {
        throw { kind: "cancelled", message: "cancelled" };
      }),
    });
    const flow = applyingFlow(ui(), backend);

    await flow.run(pick(COMMITS[2]));

    expect(shown()).toHaveLength(0);
  });
});

describe("the process", () => {
  test("while_another_with_the_same_commit_runs_it_warns", async () => {
    let finish: (value: ApplyResult) => void = () => {};
    const apply = vi
      .fn<ApplyingApi["apply"]>()
      .mockImplementationOnce(
        () =>
          new Promise<ApplyResult>((resolve) => {
            finish = resolve;
          }),
      )
      .mockResolvedValue(done({ applied: [H1] }));
    const backend = api({ apply });
    const flow = applyingFlow(ui(), backend);
    flow.repositoryChanged({ path: "/tmp/mi-repo" });

    const first = flow.run(pick(COMMITS[2]));
    await vi.advanceTimersByTimeAsync(0);
    await flow.run(pick(COMMITS[2]));

    expect(last()).toMatchObject({
      kind: "error",
      title: "Cherry-pick process is already started for commit 11111111 from root mi-repo",
    });
    expect(backend.apply).toHaveBeenCalledTimes(1);

    finish(done({ applied: [H1] }));
    await first;
    // It already finished: it can be requested again.
    await flow.run(pick(COMMITS[2]));
    expect(backend.apply).toHaveBeenCalledTimes(2);
  });

  test("what_finishes_after_changing_repository_does_not_warn", async () => {
    let finish: (value: ApplyResult) => void = () => {};
    const backend = api({
      apply: vi.fn(
        () =>
          new Promise<ApplyResult>((resolve) => {
            finish = resolve;
          }),
      ),
    });
    const flow = applyingFlow(ui(), backend);
    const running = flow.run(pick(COMMITS[2]));
    await vi.advanceTimersByTimeAsync(0);

    flow.repositoryChanged({ path: "/another" });
    finish(done({ applied: [H1] }));
    await running;

    // It is said, but without buttons and with the note of where it happened.
    expect(last()).toMatchObject({
      title: "Cherry-pick successful",
      body: "11111111 one\nThis happened in the previous repository.",
    });
    expect(last().actions).toBeUndefined();
  });

  test("conflict_cards_go_away_when_changing_repository", async () => {
    const backend = api({
      apply: vi.fn(async () =>
        done({ stopped: { kind: "conflicts", reason: { hash: H2 } } }),
      ),
    });
    const flow = applyingFlow(ui(), backend);
    await flow.run(pick(COMMITS[1]));
    expect(shown()).toHaveLength(1);

    flow.repositoryChanged({ path: "/another" });

    expect(shown()).toHaveLength(0);
  });

  test("only_cherry_pick_and_revert_are_its_own", () => {
    expect(isApplying(pick(COMMITS[0]))).toBe(true);
    expect(isApplying(revert(COMMITS[0]))).toBe(true);
    expect(isApplying({ kind: "reset", hash: H1 })).toBe(false);
  });
});

// Cherry-Pick Selected Changes and Revert Selected Changes.
const SELECTION = [
  { hash: H1, base: H2, paths: ["src/f.txt", "src/old.txt"] },
  { hash: H3, base: null, paths: ["n.txt"] },
];
const PATHS = ["src/f.txt", "src/old.txt", "n.txt"];
const selecting = (reverse: boolean, hashes = [H1]): VcsAction => ({
  kind: "applyPaths",
  reverse,
  selection: SELECTION,
  hashes,
});

describe("selected changes", () => {
  test("the_suggested_changelist_is_revert_colon_name", async () => {
    const interface_ = ui();
    const flow = applyingFlow(interface_, api());

    await flow.run(selecting(true));
    await flow.run(selecting(false));

    // The names of `changes.revert.apply.change.list.name`, with the commit's subject.
    expect(interface_.chooseChangelist.mock.calls.map(([, name]) => name)).toEqual([
      "Revert: one",
      "Apply: one",
    ]);
    expect(interface_.chooseChangelist.mock.calls[0][0]).toEqual(LISTS.lists);
  });

  test("with_several_commits_the_active_list_is_proposed", async () => {
    const interface_ = ui();
    const flow = applyingFlow(interface_, api());

    await flow.run(selecting(false, [H1, H2]));

    expect(interface_.chooseChangelist.mock.calls[0][1]).toBe("Changes");
  });

  test("choosing_a_list_moves_the_changes_to_it", async () => {
    const backend = api();
    const interface_ = ui();
    interface_.chooseChangelist.mockResolvedValue({ name: "Other", setActive: true });
    const flow = applyingFlow(interface_, backend);

    await flow.run(selecting(true));

    // Revert, with the HEAD seen before the dialog.
    expect(backend.applyPaths).toHaveBeenCalledWith(SELECTION, true, HEAD);
    expect(backend.moveToChangelist).toHaveBeenCalledWith(PATHS, "Other", true);
    const order = [backend.applyPaths, backend.moveToChangelist].map(
      (mock) => vi.mocked(mock).mock.invocationCallOrder[0],
    );
    expect(order[0]).toBeLessThan(order[1]);
    expect(last()).toMatchObject({ kind: "info", title: "Patch successfully applied" });
  });

  test("cancelling_the_chooser_touches_nothing", async () => {
    const backend = api();
    const interface_ = ui();
    interface_.chooseChangelist.mockResolvedValue(null as never);
    const flow = applyingFlow(interface_, backend);

    await flow.run(selecting(false));

    expect(backend.applyPaths).not.toHaveBeenCalled();
    expect(backend.moveToChangelist).not.toHaveBeenCalled();
    expect(shown()).toHaveLength(0);
  });

  test("with_staging_area_there_is_no_chooser_or_list", async () => {
    const backend = api({ stagingArea: vi.fn(async () => true) });
    const interface_ = ui();
    const flow = applyingFlow(interface_, backend);

    await flow.run(selecting(false));

    expect(interface_.chooseChangelist).not.toHaveBeenCalled();
    expect(backend.applyPaths).toHaveBeenCalledWith(SELECTION, false, HEAD);
    expect(backend.moveToChangelist).not.toHaveBeenCalled();
  });

  test("failure_shows_failed_to_revert_changes", async () => {
    const backend = api({
      applyPaths: vi.fn(async () => ({ kind: "failed", reason: "patch does not apply" }) as PathsResult),
    });
    const flow = applyingFlow(ui(), backend);

    await flow.run(selecting(true));

    expect(last()).toMatchObject({
      kind: "error",
      title: "Revert Changes",
      body: "Failed to revert changes: patch does not apply",
    });
    expect(backend.moveToChangelist).not.toHaveBeenCalled();

    await flow.run(selecting(false));

    expect(last()).toMatchObject({
      title: "Apply Changes",
      body: "Failed to apply changes: patch does not apply",
    });
  });

  test("a_backend_error_is_also_failed_to_apply_changes", async () => {
    const backend = api({
      applyPaths: vi.fn(async () => {
        throw { kind: "failed", message: "Repository has already been changed" };
      }),
    });
    const flow = applyingFlow(ui(), backend);

    await flow.run(selecting(false));

    expect(last().body).toBe("Failed to apply changes: Repository has already been changed");
  });

  test("a_cancelled_one_shows_nothing", async () => {
    const backend = api({
      applyPaths: vi.fn(async () => {
        throw { kind: "cancelled", message: "Cancelado" };
      }),
    });
    const flow = applyingFlow(ui(), backend);

    await flow.run(selecting(false));

    expect(shown()).toHaveLength(0);
  });

  test("with_conflicts_moves_first_and_opens_the_conflicts_dialog", async () => {
    const backend = api({ applyPaths: vi.fn(async () => ({ kind: "conflicts" }) as PathsResult) });
    const interface_ = ui();
    const flow = applyingFlow(interface_, backend);

    await flow.run(selecting(false));

    expect(backend.moveToChangelist).toHaveBeenCalledWith(PATHS, "Apply: one", false);
    expect(interface_.resolveConflicts).toHaveBeenCalledTimes(1);
    expect(
      vi.mocked(backend.moveToChangelist).mock.invocationCallOrder[0],
    ).toBeLessThan(interface_.resolveConflicts.mock.invocationCallOrder[0]);
  });

  test("if_the_repository_changes_it_neither_moves_nor_opens_anything", async () => {
    let finish: (value: PathsResult) => void = () => {};
    const backend = api({
      applyPaths: vi.fn(() => new Promise<PathsResult>((resolve) => (finish = resolve))),
    });
    const interface_ = ui();
    const flow = applyingFlow(interface_, backend);
    const running = flow.run(selecting(false));
    await vi.advanceTimersByTimeAsync(0);

    flow.repositoryChanged({ path: "/another" });
    finish({ kind: "conflicts" });
    await running;

    expect(backend.moveToChangelist).not.toHaveBeenCalled();
    expect(interface_.resolveConflicts).not.toHaveBeenCalled();
  });

  test("only_applypaths_belongs_to_selected_changes", () => {
    expect(isApplyingPaths(selecting(true))).toBe(true);
    expect(isApplyingPaths(pick(COMMITS[0]))).toBe(false);
    expect(isApplying(selecting(true))).toBe(false);
  });
});
