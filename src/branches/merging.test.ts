import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { get } from "svelte/store";
import type { LogRef, RebaseRun, RepoState } from "../lib/api";
import type { MenuAction, MenuItem } from "../lib/menu";
import { dismiss, notifications, type Shown } from "../lib/notifications";
import { branchActions, type BranchNode } from "./model";
import {
  mergingFlow,
  type MergingApi,
  type MergingRequest,
  type MergingUi,
} from "./merging";
import PublishedCommitsDialog from "./PublishedCommitsDialog.svelte";

const HEAD = "a1b2c3d4".repeat(5);
const STATE: RepoState = { operation: null, branch: "main", label: "main", head: HEAD };
const SAVED = { kind: "stash" as const, sha: "abc", message: "Uncommitted changes before rebase", lists: [] };
const MERGE_SAVED = { kind: "stash" as const, sha: "def", message: "Uncommitted changes before Merge at today", lists: [] };
const DONE: RebaseRun = { outcome: { kind: "done" }, title: null, saved: [] };
const MERGED = {
  outcome: {
    kind: "merged" as const,
    reason: { files: 1, commits: 1, range: null, matching: null },
  },
  saved: null,
  restore: null,
};
const local = (name: string): LogRef => ({ name, kind: "local" });
const remote = (name: string): LogRef => ({ name, kind: "remote" });

function api(overrides: Partial<MergingApi> = {}): MergingApi {
  return {
    state: vi.fn(async () => STATE),
    protectedBranches: vi.fn(async () => ["master", "main"]),
    merge: vi.fn(async () => MERGED),
    commitMerge: vi.fn(async () => {}),
    rebase: vi.fn(async () => DONE),
    rebaseRemote: vi.fn(async () => ({ kind: "rebased" as const, reason: DONE })),
    published: vi.fn(async () => null),
    branches: vi.fn(async () => ({ local: ["main", "side"], remote: ["origin/feature"] })),
    ...overrides,
  };
}

function ui(
  answers: Partial<Record<MergingRequest["kind"], unknown>> = {},
  extra: Partial<MergingUi> = {},
) {
  const asked: MergingRequest[] = [];
  const dialogs = {
    asked,
    ask: async (request: MergingRequest) => {
      asked.push(request);
      return (answers[request.kind] ?? null) as never;
    },
    resolveConflicts: vi.fn(async () => "resolved" as const),
    rebaseStopped: vi.fn(),
    rebaseConflicts: vi.fn(async () => {}),
    restored: vi.fn(),
    restoreSaved: vi.fn(async () => {}),
    deleteBranch: vi.fn(async () => {}),
    pullInto: vi.fn(async () => {}),
    ...extra,
  };
  return dialogs;
}

const run = (flow: ReturnType<typeof mergingFlow>, operation: string, ref: LogRef) =>
  flow.run({ kind: "ref", operation: operation as never, ref });

const shown = () => get(notifications);
const last = (): Shown => shown()[shown().length - 1];
const titlesShown = () => shown().map(({ title }) => title);
const labels = (items: MenuItem[]) =>
  items.map((item) => (item === null ? "—" : "label" in item ? item.label : item.header));
const row = (items: MenuItem[], label: string) =>
  items.find((item) => item !== null && "label" in item && item.label === label) as MenuAction;

const NODE = (name: string, section: "local" | "remote"): BranchNode => ({
  kind: "branch",
  id: `${section}:${name}`,
  section,
  name,
  label: name,
  current: false,
  favorite: false,
  tracked: "origin/x",
  match: null,
});

beforeEach(() => {
  for (const { id } of shown()) dismiss(id);
});

afterEach(cleanup);

describe("the popup entries", () => {
  const state = { current: "main", remotes: true, onvcs: vi.fn() };

  test("merge_into_does_not_appear_on_the_current", () => {
    const current = labels(branchActions(NODE("main", "local"), state));
    const other = labels(branchActions(NODE("side", "local"), state));

    expect(current.some((label) => label.startsWith("Merge "))).toBe(false);
    expect(current.some((label) => label.startsWith("Rebase "))).toBe(false);
    expect(current.some((label) => label.startsWith("Checkout and Rebase"))).toBe(false);
    expect(other).toEqual([
      "Checkout",
      "New Branch from 'side'…",
      "Checkout and Rebase onto 'main'",
      "—",
      "Compare with 'main'",
      "Show Diff with Working Tree",
      "—",
      "Rebase 'main' onto 'side'",
      "Merge 'side' into 'main'",
      "—",
      "Update",
      "Push…",
      "—",
      "Rename…",
      "Delete",
    ]);
  });

  test("names_in_single_quotes", () => {
    const items = branchActions(NODE("origin/feature", "remote"), state);

    expect(labels(items)).toEqual([
      "Checkout",
      "New Branch from 'origin/feature'…",
      "Checkout and Rebase onto 'main'",
      "—",
      "Compare with 'main'",
      "Show Diff with Working Tree",
      "—",
      "Rebase 'main' onto 'origin/feature'",
      "Merge 'origin/feature' into 'main'",
      "—",
      "Pull into 'main' Using Merge",
      "Pull into 'main' Using Rebase",
      "—",
      "Delete",
    ]);
    // With a detached HEAD, the current one is the short hash, and the rebase is not possible.
    const detached = branchActions(NODE("side", "local"), { ...state, current: null, head: HEAD });
    expect(labels(detached)).toContain("Merge 'side' into 'a1b2c3d4'");
    const rebase = row(detached, "Rebase 'a1b2c3d4' onto 'side'");
    expect(rebase.disabled).toBe(true);
    expect(rebase.title).toBe("Rebase is not possible in the detached HEAD state");
    // Long names are shortened like the others.
    const long = "x".repeat(50);
    expect(labels(branchActions(NODE(long, "local"), state))).toContain(
      `Merge '${"x".repeat(39)}…' into 'main'`,
    );
  });

  test("pull_into_only_on_remotes", () => {
    const onvcs = vi.fn();
    const local = labels(branchActions(NODE("side", "local"), { ...state, onvcs }));
    const items = branchActions(NODE("origin/feature", "remote"), { ...state, onvcs });

    expect(local.some((label) => label.startsWith("Pull into"))).toBe(false);
    row(items, "Pull into 'main' Using Rebase").run?.();
    expect(onvcs).toHaveBeenCalledWith({
      kind: "ref",
      operation: "pullRebase",
      ref: { name: "origin/feature", kind: "remote" },
    });
  });

  test("pull_into_launches_the_update_towards_the_remote", async () => {
    const dialogs = ui();
    const flow = mergingFlow(dialogs, api());

    await run(flow, "pullMerge", remote("origin/feature"));
    await run(flow, "pullRebase", remote("origin/feature"));

    expect(dialogs.pullInto).toHaveBeenNthCalledWith(1, "origin/feature", "merge");
    expect(dialogs.pullInto).toHaveBeenNthCalledWith(2, "origin/feature", "rebase");
  });
});

describe("merge", () => {
  test("changes_that_would_be_overwritten_offer_smart_merge_and_dont_merge", async () => {
    const merge = vi
      .fn()
      .mockResolvedValueOnce({
        outcome: { kind: "localChanges", reason: { files: ["a.txt"] } },
        saved: null,
        restore: null,
      })
      .mockResolvedValue({ ...MERGED, saved: MERGE_SAVED, restore: { kind: "restored" } });
    const dialogs = ui({ problem: "smart" });
    await run(mergingFlow(dialogs, api({ merge })), "merge", local("side"));

    expect(dialogs.asked).toEqual([
      {
        kind: "problem",
        operation: "merge",
        title: "Git Merge Problem",
        smart: "Smart Merge",
        force: null,
        cancel: "Don't Merge",
        files: ["a.txt"],
      },
    ]);
    expect(merge).toHaveBeenNthCalledWith(1, "side", "local", false, HEAD);
    expect(merge).toHaveBeenNthCalledWith(2, "side", "local", true, HEAD);
    expect(dialogs.restored).toHaveBeenCalledWith(
      [{ saved: MERGE_SAVED, restore: { kind: "restored" } }],
      { operation: "merge", from: "side" },
    );
    expect(last().title).toBe("Merged side to main");

    // `Don't Merge` does nothing else, like `fatalLocalChangesError` with nothing done.
    for (const { id } of shown()) dismiss(id);
    const once = vi.fn().mockResolvedValue({
      outcome: { kind: "localChanges", reason: { files: ["a.txt"] } },
      saved: null,
      restore: null,
    });
    await run(mergingFlow(ui(), api({ merge: once })), "merge", local("side"));
    expect(once).toHaveBeenCalledTimes(1);
    expect(shown()).toEqual([]);
  });

  test("popup_merge_passes_the_kind_of_the_clicked_item_with_the_short_name", async () => {
    const merge = vi.fn(async () => MERGED);
    const backend = api({ merge });
    await run(mergingFlow(ui(), backend), "merge", { name: "x", kind: "tag" });
    await run(mergingFlow(ui(), backend), "merge", remote("origin/x"));
    await run(mergingFlow(ui(), backend), "merge", local("x"));

    expect(merge).toHaveBeenNthCalledWith(1, "x", "tag", false, HEAD);
    expect(merge).toHaveBeenNthCalledWith(2, "origin/x", "remote", false, HEAD);
    expect(merge).toHaveBeenNthCalledWith(3, "x", "local", false, HEAD);
    expect(last()).toMatchObject({ title: "Merged x to main" });
  });

  test("the_smart_merge_dialog_has_no_force", async () => {
    render(
      (await import("./CheckoutProblemDialog.svelte")).default,
      {
        operation: "merge",
        title: "Git Merge Problem",
        smart: "Smart Merge",
        force: null,
        cancel: "Don't Merge",
        files: ["a.txt"],
        saver: "stash",
        onanswer: vi.fn(),
      },
    );

    const buttons = screen.getAllByRole("button").map((button) => button.textContent?.trim());
    expect(buttons).toEqual(["Don't Merge", "Smart Merge"]);
    expect(screen.getByText("Your local changes to the following files would be overwritten by merge.")).toBeTruthy();
    expect(screen.getByText("Can stash the changes, merge and unstash them after that.")).toBeTruthy();
  });

  test("successful_merge_notifies_merged_x_to_y_with_delete_x", async () => {
    const dialogs = ui();
    await run(mergingFlow(dialogs, api()), "merge", local("side"));

    expect(last()).toMatchObject({ kind: "info", title: "Merged side to main" });
    expect(last().actions?.map(({ label }) => label)).toEqual(["Delete side"]);
    last().actions?.[0].run();
    expect(dialogs.deleteBranch).toHaveBeenCalledWith("side");

    // Without `Delete x` if remote or protected.
    for (const { id } of shown()) dismiss(id);
    await run(mergingFlow(ui(), api()), "merge", remote("origin/side"));
    expect(last()).toMatchObject({ title: "Merged origin/side to main" });
    expect(last().actions).toBeUndefined();

    const protectedOnes = api({ protectedBranches: vi.fn(async () => ["release.*"]) });
    await run(mergingFlow(ui(), protectedOnes), "merge", local("release-1"));
    expect(last()).toMatchObject({ title: "Merged release-1 to main" });
    expect(last().actions).toBeUndefined();
  });

  test("already_up_to_date", async () => {
    const merge = vi.fn(async () => ({ outcome: { kind: "upToDate" }, saved: null, restore: null }));
    await run(mergingFlow(ui(), api({ merge } as never)), "merge", local("side"));

    expect(last()).toMatchObject({ kind: "info", title: "Already up to date" });
    expect(last().actions?.map(({ label }) => label)).toEqual(["Delete side"]);
  });

  test("merge_with_conflicts_opens_conflicts_and_if_any_remain_warns_merged_with_conflicts", async () => {
    const conflicts = { outcome: { kind: "conflicts" }, saved: MERGE_SAVED, restore: null };
    const merge = vi.fn(async () => conflicts);
    const backend = api({ merge } as never);
    // Everything is resolved: the merge commit, the notice and, afterwards, what was saved.
    const resolving = ui(
      {},
      {
        resolveConflicts: vi.fn(async (options) => {
          await options?.onresolved?.();
          return "resolved" as const;
        }),
      },
    );
    await run(mergingFlow(resolving, backend), "merge", local("side"));

    expect(backend.commitMerge).toHaveBeenCalled();
    expect(resolving.restoreSaved).toHaveBeenCalledWith([MERGE_SAVED], {
      operation: "merge",
      from: "side",
    });
    expect(last().title).toBe("Merged side to main");

    // Conflicts remain: the notice with `Resolve…`, which reopens the dialog without confirming anything.
    for (const { id } of shown()) dismiss(id);
    const pending = ui(
      {},
      {
        resolveConflicts: vi.fn(async (options) => {
          options?.onpending?.();
          return "pending" as const;
        }),
      },
    );
    await run(mergingFlow(pending, api({ merge } as never)), "merge", local("side"));

    expect(last()).toMatchObject({ kind: "warning", title: "side Merged with Conflicts" });
    expect(last().actions?.map(({ label }) => label)).toEqual(["Resolve…"]);
    last().actions?.[0].run();
    expect(pending.resolveConflicts).toHaveBeenLastCalledWith();
  });

  test("if_the_merge_commit_fails_what_was_saved_is_still_brought_back", async () => {
    const conflicts = { outcome: { kind: "conflicts" }, saved: MERGE_SAVED, restore: null };
    const backend = api({
      merge: vi.fn(async () => conflicts),
      commitMerge: vi.fn().mockRejectedValue({ kind: "failed", message: "hook" }),
    } as never);
    const resolving = ui(
      {},
      {
        resolveConflicts: vi.fn(async (options) => {
          await options?.onresolved?.().catch(() => {});
          return "resolved" as const;
        }),
      },
    );

    await run(mergingFlow(resolving, backend), "merge", local("side"));

    expect(resolving.restoreSaved).toHaveBeenCalledWith([MERGE_SAVED], {
      operation: "merge",
      from: "side",
    });
    expect(titlesShown()).not.toContain("Merged side to main");
  });

  test("with_nothing_to_resolve_it_commits_and_brings_back_as_when_resolving", async () => {
    const conflicts = { outcome: { kind: "conflicts" }, saved: MERGE_SAVED, restore: null };
    const backend = api({ merge: vi.fn(async () => conflicts) } as never);
    const nothing = ui(
      {},
      {
        resolveConflicts: vi.fn(async (options) => {
          await options?.onnothing?.();
          return "resolved" as const;
        }),
      },
    );

    await run(mergingFlow(nothing, backend), "merge", local("side"));

    expect(backend.commitMerge).toHaveBeenCalled();
    expect(nothing.restoreSaved).toHaveBeenCalledWith([MERGE_SAVED], {
      operation: "merge",
      from: "side",
    });
    expect(last().title).toBe("Merged side to main");
  });

  test("unmerged_files_text_and_action_follow_241", async () => {
    const merge = vi.fn(async () => ({ outcome: { kind: "unmerged" }, saved: null, restore: null }));
    const dialogs = ui();
    await run(mergingFlow(dialogs, api({ merge } as never)), "merge", local("side"));

    expect(last()).toMatchObject({
      kind: "error",
      title: "Cannot merge because of unmerged files",
      body: "You need to resolve all merge conflicts before merge.\nAfter resolving conflicts do not forget to commit your files to the current branch before switching to another branch.",
    });
    expect(last().actions?.map(({ label }) => label)).toEqual(["Resolve conflicts…"]);
    last().actions?.[0].run();
    expect(dialogs.resolveConflicts).toHaveBeenCalled();
  });

  test("a_failure_is_could_not_merge_x", async () => {
    const merge = vi.fn(async () => ({
      outcome: { kind: "failed", reason: "fatal: bad" },
      saved: null,
      restore: null,
    }));
    await run(mergingFlow(ui(), api({ merge } as never)), "merge", local("side"));

    expect(last()).toMatchObject({ kind: "error", title: "Could Not Merge side", body: "fatal: bad" });
  });
});

describe("rebase", () => {
  test("published_rebase_asks_rebasing_published_commit_with_cancel_by_default", async () => {
    const backend = api({ published: vi.fn(async () => "origin/main") });
    const cancelled = ui();
    await run(mergingFlow(cancelled, backend), "rebase", local("side"));

    expect(backend.published).toHaveBeenCalledWith("side", "main");
    expect(cancelled.asked).toEqual([{ kind: "publishedCommits" }]);
    expect(backend.rebase).not.toHaveBeenCalled();

    await run(mergingFlow(ui({ publishedCommits: true }), backend), "rebase", local("side"));
    expect(backend.rebase).toHaveBeenCalledWith("side", null, HEAD);
  });

  test("the_published_dialog_cancels_with_enter", async () => {
    const onanswer = vi.fn();
    render(PublishedCommitsDialog, { onanswer });

    expect(screen.getByRole("dialog", { name: "Rebasing Published Commit" })).toBeTruthy();
    expect(
      screen.getByText("You are trying to rebase some commits already pushed to a protected branch."),
    ).toBeTruthy();
    expect(
      screen.getByText(
        "Rebasing them would duplicate commits, which is not recommended and most likely unwanted.",
      ),
    ).toBeTruthy();
    const cancel = screen.getByRole("button", { name: "Cancel" });
    expect(document.activeElement).toBe(cancel);
    await fireEvent.submit(cancel.closest("form") as HTMLFormElement);
    expect(onanswer).toHaveBeenLastCalledWith(false);
    await fireEvent.click(screen.getByRole("button", { name: "Rebase Anyway" }));
    expect(onanswer).toHaveBeenLastCalledWith(true);
  });

  test("successful_rebase_notifies_rebased_x_on_y", async () => {
    await run(mergingFlow(ui(), api()), "rebase", local("side"));

    expect(last()).toMatchObject({
      kind: "info",
      title: "Rebase successful",
      body: "Rebased main on side",
    });
  });

  test("checkout_and_rebase_notifies_checked_out_and_rebased", async () => {
    const after = { ...STATE, branch: "side" };
    const state = vi.fn().mockResolvedValueOnce(STATE).mockResolvedValue(after);
    const backend = api({ state });
    await run(mergingFlow(ui(), backend), "checkoutAndRebase", local("side"));

    expect(backend.published).toHaveBeenCalledWith("HEAD", "side");
    expect(backend.rebase).toHaveBeenCalledWith("HEAD", "side", HEAD);
    expect(last()).toMatchObject({
      title: "Rebase successful",
      body: "Checked out side and rebased it on main",
    });
  });

  test("checkout_and_rebase_of_a_remote_asks_like_intellij", async () => {
    const rebaseRemote = vi
      .fn()
      .mockResolvedValueOnce({ kind: "needsName", reason: { suggested: "feature" } })
      .mockResolvedValueOnce({ kind: "localHasCommits", reason: { local: "mine" } })
      .mockResolvedValue({ kind: "rebased", reason: DONE });
    const state = vi
      .fn()
      .mockResolvedValueOnce(STATE)
      .mockResolvedValue({ ...STATE, branch: "mine" });
    const backend = api({ rebaseRemote, state });
    const dialogs = ui({ newBranch: { name: "mine" }, localCommits: "drop" });

    await run(mergingFlow(dialogs, backend), "checkoutAndRebase", remote("origin/feature"));

    expect(backend.published).toHaveBeenCalledWith("HEAD", "origin/feature");
    expect(dialogs.asked).toMatchObject([
      { kind: "newBranch", title: "Checkout origin/feature", initialName: "feature" },
      {
        kind: "localCommits",
        title: "Checkout origin/feature",
        message:
          "Local branch 'mine' has commits that do not exist in 'origin/feature'. Do you want to drop local commits?",
        rebase: false,
      },
    ]);
    expect(rebaseRemote.mock.calls).toEqual([
      ["origin/feature", null, false, HEAD],
      ["origin/feature", "mine", false, HEAD],
      ["origin/feature", "mine", true, HEAD],
    ]);
    expect(last().body).toBe("Checked out mine and rebased it on main");
  });

  test("rebase_with_conflicts_follows_task_2", async () => {
    const stopped: RebaseRun = {
      outcome: { kind: "conflicts" },
      title: null,
      saved: [{ saved: SAVED, restore: null }],
    };
    const conflicts = ui();
    await run(mergingFlow(conflicts, api({ rebase: vi.fn(async () => stopped) })), "rebase", local("side"));
    expect(conflicts.rebaseConflicts).toHaveBeenCalledWith([SAVED]);

    const editing = ui();
    const paused = { ...stopped, outcome: { kind: "editing" as const } };
    await run(mergingFlow(editing, api({ rebase: vi.fn(async () => paused) })), "rebase", local("side"));
    expect(editing.rebaseStopped).toHaveBeenCalledWith({ kind: "editing" }, [SAVED]);
  });

  test("disallowed_rebase_says_so_with_its_title", async () => {
    const refused: RebaseRun = {
      outcome: { kind: "failed", reason: "There is an unfinished merge process in rama." },
      title: "Rebase not allowed",
      saved: [],
    };
    const dialogs = ui();
    await run(mergingFlow(dialogs, api({ rebase: vi.fn(async () => refused) })), "rebase", local("side"));

    expect(last()).toMatchObject({
      kind: "error",
      title: "Rebase not allowed",
      body: "There is an unfinished merge process in rama.",
    });
    expect(dialogs.rebaseStopped).not.toHaveBeenCalled();
  });

  test("what_does_not_come_back_after_the_rebase_is_reported", async () => {
    const blocked = { kind: "restoreBlocked" as const, reason: { files: ["a.txt"] } };
    const back: RebaseRun = {
      outcome: { kind: "done" },
      title: null,
      saved: [{ saved: SAVED, restore: blocked }],
    };
    const dialogs = ui();
    await run(mergingFlow(dialogs, api({ rebase: vi.fn(async () => back) })), "rebase", local("side"));

    expect(dialogs.restored).toHaveBeenCalledWith([{ saved: SAVED, restore: blocked }], {
      operation: "rebase",
      from: "side",
    });
    expect(last().title).toBe("Rebase successful");
  });
});
