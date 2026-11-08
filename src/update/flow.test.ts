import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { get } from "svelte/store";
import type {
  Aborted,
  Operation,
  PullInfo,
  PullOption,
  RepoInfo,
  RestoreOutcome,
  Settings,
  UpdateResult,
} from "../lib/api";
import { dismiss, notifications, type Shown } from "../lib/notifications";
import {
  UPDATE_ACTIONS,
  fetching,
  updateFlow,
  type PullChoice,
  type TrackedChoice,
  type UpdateApi,
  type UpdateChoice,
  type UpdateRequest,
} from "./flow";
import PublishedCommitsDialog from "../branches/PublishedCommitsDialog.svelte";
import FixTrackedDialog from "./FixTrackedDialog.svelte";
import RebaseMergesDialog from "./RebaseMergesDialog.svelte";
import UpdateDialog from "./UpdateDialog.svelte";

const PULL_INFO: PullInfo = {
  branch: "main",
  remotes: ["origin"],
  remoteBranches: ["origin/main", "origin/other"],
  tracked: ["origin", "main"],
};
const PULL: PullChoice = { remote: "origin", branch: "main", options: [] };
const SAVED = { sha: "abc", message: "Uncommitted changes before Update at today" };
const SETTINGS = {
  updateMethod: "merge",
  showUpdateDialog: false,
} as Settings;

const result = (overrides: Partial<UpdateResult> = {}): UpdateResult => ({
  outcome: { kind: "success" },
  title: null,
  files: 0,
  commits: 0,
  range: null,
  matching: null,
  saved: null,
  restore: null,
  inProgress: false,
  ...overrides,
});

function api(overrides: Partial<UpdateApi> = {}): UpdateApi {
  return {
    settings: vi.fn(async () => SETTINGS),
    saveSettings: vi.fn(async () => {}),
    update: vi.fn(async () => result()),
    pullInto: vi.fn(async () => result()),
    updateBranch: vi.fn(async () => {}),
    fetch: vi.fn(async () => ({ pruned: [], failed: [] })),
    abort: vi.fn(async () => "merge" as const),
    state: vi.fn(async () => ({ operation: "merging" as const, branch: "main", label: "Merging main", head: "abc" })),
    restore: vi.fn(async () => ({ kind: "restored" as const })),
    remoteBranches: vi.fn(async () => ["origin/main", "origin/other"]),
    currentBranch: vi.fn(async () => "main"),
    pullInfo: vi.fn(async () => PULL_INFO),
    pull: vi.fn(async () => result()),
    fetchRemote: vi.fn(async () => [] as string[]),
    pullOptions: vi.fn(async () => [] as PullOption[]),
    savePullOptions: vi.fn(async () => {}),
    ...overrides,
  };
}

const flow = (
  backend: UpdateApi,
  answers: unknown[] = [],
  viewCommits?: () => void,
  abortOperation?: (operation: Operation) => Promise<boolean>,
) => {
  const ask = vi.fn(async (_: UpdateRequest) => answers.shift() ?? null);
  return { ask, ...updateFlow({ ask: ask as never, viewCommits, abortOperation }, backend) };
};

const shown = () => get(notifications);
const titled = (title: string) => shown().find((item) => item.title === title) as Shown;
const titles = () => shown().map(({ title }) => title);
const click = (title: string, label: string) =>
  titled(title).actions?.find((action) => action.label === label)?.run();
const labels = (title: string) => titled(title).actions?.map(({ label }) => label);

beforeEach(() => shown().forEach(({ id }) => dismiss(id)));
afterEach(cleanup);

describe("dialog", () => {
  test("dialog_with_merge_and_rebase", async () => {
    const onanswer = vi.fn();
    render(UpdateDialog, { method: "rebase", hidden: false, onanswer });

    expect(screen.getByRole("dialog", { name: "Update Project" })).toBeTruthy();
    const merge = screen.getByRole("radio", {
      name: "Merge incoming changes into the current branch",
    }) as HTMLInputElement;
    const rebase = screen.getByRole("radio", {
      name: "Rebase the current branch on top of incoming changes",
    }) as HTMLInputElement;
    expect(rebase.checked).toBe(true);
    expect(merge.checked).toBe(false);

    await fireEvent.click(merge);
    await fireEvent.click(screen.getByRole("checkbox", { name: "Don't show again" }));
    await fireEvent.click(screen.getByRole("button", { name: "OK" }));

    expect(onanswer).toHaveBeenCalledWith({ method: "merge", hide: true });
  });

  test("cancelling_the_dialog_answers_nothing", async () => {
    const onanswer = vi.fn();
    render(UpdateDialog, { method: "merge", hidden: false, onanswer });

    await fireEvent.click(screen.getByRole("button", { name: "Cancel" }));

    expect(onanswer).toHaveBeenCalledWith(null);
  });

  test("dont_show_again_is_saved_on_accept", async () => {
    const backend = api({
      settings: vi.fn(async () => ({ ...SETTINGS, showUpdateDialog: true })),
    });
    const { updateProject, ask } = flow(backend, [
      { method: "rebase", hide: true } satisfies UpdateChoice,
    ]);

    await updateProject();

    expect(ask).toHaveBeenCalledWith({ kind: "updateProject", method: "merge", hidden: false });
    expect(backend.saveSettings).toHaveBeenCalledWith({
      ...SETTINGS,
      updateMethod: "rebase",
      showUpdateDialog: false,
    });
    expect(backend.update).toHaveBeenCalledWith("rebase", {});
  });

  test("cancelling_does_not_save", async () => {
    const backend = api({
      settings: vi.fn(async () => ({ ...SETTINGS, showUpdateDialog: true })),
    });

    await flow(backend, [null]).updateProject();

    expect(backend.saveSettings).not.toHaveBeenCalled();
    expect(backend.update).not.toHaveBeenCalled();
    expect(shown()).toEqual([]);
  });

  test("without_a_dialog_it_uses_the_saved_method", async () => {
    const backend = api({
      settings: vi.fn(async () => ({ ...SETTINGS, updateMethod: "rebase" }) as Settings),
    });
    const { updateProject, ask } = flow(backend);

    await updateProject();

    expect(ask).not.toHaveBeenCalled();
    expect(backend.update).toHaveBeenCalledWith("rebase", {});
  });

  test("shift_shows_it_even_if_hidden", async () => {
    const backend = api();
    const { updateProject, ask } = flow(backend, [{ method: "merge", hide: true }]);

    await updateProject({ shift: true });

    expect(ask).toHaveBeenCalledWith({ kind: "updateProject", method: "merge", hidden: true });
    expect(backend.update).toHaveBeenCalled();
  });

  test("a_second_update_does_not_start_while_the_first_one_runs", async () => {
    let finish = (_: UpdateResult) => {};
    const backend = api({
      update: vi.fn(() => new Promise<UpdateResult>((resolve) => (finish = resolve))),
    });
    const { updateProject } = flow(backend);

    const first = updateProject();
    await vi.waitFor(() => expect(backend.update).toHaveBeenCalled());
    await updateProject();
    finish(result());
    await first;

    expect(backend.update).toHaveBeenCalledTimes(1);
  });
});

describe("the actions share a flow", () => {
  test("vcs_updateproject_and_updatefiles_are_the_same", async () => {
    const backend = api();
    const { run } = flow(backend);

    await run("Vcs.UpdateProject");
    await run("UpdateFiles");

    expect(backend.update).toHaveBeenCalledTimes(2);
  });

  test("git_fetch_launches_the_fetch", async () => {
    const backend = api();

    await flow(backend).run("Git.Fetch");

    expect(backend.fetch).toHaveBeenCalled();
    expect(backend.update).not.toHaveBeenCalled();
  });

  test("update_of_the_current_branch_does_not_ask_and_goes_through_update_project", async () => {
    const backend = api({
      settings: vi.fn(async () => ({ ...SETTINGS, showUpdateDialog: true })),
    });
    const { updateBranch, ask } = flow(backend);

    await updateBranch("main");

    expect(ask).not.toHaveBeenCalled();
    expect(backend.update).toHaveBeenCalledWith("merge", {});
    expect(backend.updateBranch).not.toHaveBeenCalled();
  });

  test("update_of_another_branch_only_advances_it", async () => {
    const backend = api();

    await flow(backend).updateBranch("other");

    expect(backend.updateBranch).toHaveBeenCalledWith("other");
    expect(backend.update).not.toHaveBeenCalled();
    expect(titled("Updated branch:").body).toBe("other");
  });

  test("update_of_another_branch_that_fails_gives_update_failed", async () => {
    const backend = api({
      updateBranch: vi.fn(async () => {
        throw { kind: "failed", message: "non-fast-forward" };
      }),
    });

    await flow(backend).updateBranch("other");

    expect(titled("Update Failed").body).toBe("non-fast-forward");
  });
});

describe("fetch", () => {
  test("successful_fetch_lists_the_deleted_branches", async () => {
    const backend = api({
      fetch: vi.fn(async () => ({ pruned: ["origin/old"], failed: [] })),
    });

    await flow(backend).fetch();

    expect(titled("Fetch Successful").kind).toBe("info");
    expect(titled("Fetch Successful").body).toBe("origin/old");
  });

  test("failed_fetch_shows_stderr", async () => {
    const backend = api({
      fetch: vi.fn(async () => ({
        pruned: [],
        failed: [{ remote: "origin", error: "fatal: no" }],
      })),
    });

    await flow(backend).fetch();

    expect(titled("Fetch Failed").kind).toBe("error");
    expect(titled("Fetch Failed").body).toBe("fatal: no");
  });

  test("with_several_failed_remotes_it_says_which", async () => {
    const backend = api({
      fetch: vi.fn(async () => ({
        pruned: ["origin/old"],
        failed: [
          { remote: "a", error: "one" },
          { remote: "b", error: "two" },
        ],
      })),
    });

    await flow(backend).fetch();

    expect(titled("Fetch Failed").body).toBe("a: one\nb: two\norigin/old");
  });

  test("cancelling_the_fetch_does_not_warn", async () => {
    const backend = api({
      fetch: vi.fn(async () => {
        throw { kind: "cancelled", message: "" };
      }),
    });

    await flow(backend).fetch();

    expect(shown()).toEqual([]);
  });
});

describe("result", () => {
  test("result_n_files_updated_in_m_commits", async () => {
    const viewCommits = vi.fn();
    const range: [string, string] = ["a", "b"];
    const backend = api({
      update: vi.fn(async () => result({ files: 3, commits: 2, range })),
    });

    await flow(backend, [], viewCommits).updateProject();

    expect(titled("3 files updated in 2 commits").kind).toBe("info");
    click("3 files updated in 2 commits", "View Commits");
    expect(viewCommits).toHaveBeenCalledWith(range);
  });

  test("the_singular_is_intellijs", async () => {
    const backend = api({
      update: vi.fn(async () => result({ files: 1, commits: 1, range: ["a", "b"] })),
    });

    await flow(backend).updateProject();

    expect(titles()).toEqual(["1 file updated in 1 commit"]);
  });

  test("all_files_are_up_to_date", async () => {
    const backend = api({
      update: vi.fn(async () => result({ outcome: { kind: "nothingToUpdate" } })),
    });

    await flow(backend).updateProject();

    expect(titled("All files are up to date").kind).toBe("info");
  });

  test("fetch_error_gives_update_failed_and_update_canceled", async () => {
    const backend = api({
      update: vi.fn(async () =>
        result({
          outcome: { kind: "notReady", reason: "fatal: no remote" },
          title: "Update failed",
        }),
      ),
    });

    await flow(backend).updateProject();

    expect(titled("Update failed").kind).toBe("error");
    expect(titled("Update failed").body).toBe("fatal: no remote");
    expect(titled("Update canceled").kind).toBe("warning");
  });

  test("cancelling_warns_with_update_canceled", async () => {
    const backend = api({
      update: vi.fn(async () => result({ outcome: { kind: "cancelled" } })),
    });

    await flow(backend).updateProject();

    expect(titles()).toEqual(["Update canceled"]);
  });

  test("merge_error_with_restored_changes_does_not_mention_them", async () => {
    const backend = api({
      update: vi.fn(async () =>
        result({
          outcome: { kind: "error", reason: "Move or commit them before merge\nx" },
          title: "Untracked Files Prevent Merge",
          saved: SAVED,
          restore: { kind: "restored" },
        }),
      ),
    });

    await flow(backend).updateProject();

    expect(titles()).toEqual(["Untracked Files Prevent Merge", "Update canceled"]);
  });
});

describe("path filter", () => {
  const withMatching = (matching: number | null) =>
    api({
      update: vi.fn(async () =>
        result({ files: 2, commits: 3, range: ["a", "b"], matching }),
      ),
    });

  test("path_filter_counts_matching_commits", async () => {
    await flow(withMatching(2)).updateProject();

    expect(titled("2 files updated in 3 commits").body).toBe("2 commits matching filters");
  });

  test("one_matching_commit_is_singular", async () => {
    await flow(withMatching(1)).updateProject();

    expect(titled("2 files updated in 3 commits").body).toBe("1 commit matching filters");
  });

  test("no_matching_commit_says_so", async () => {
    await flow(withMatching(0)).updateProject();

    expect(titled("2 files updated in 3 commits").body).toBe("No commits matching filters");
  });

  test("without_a_filter_the_notification_has_no_body", async () => {
    await flow(withMatching(null)).updateProject();

    expect(titled("2 files updated in 3 commits").body).toBeUndefined();
  });
});

describe("half-done update", () => {
  const partial = (outcome: UpdateResult["outcome"]) =>
    api({
      update: vi.fn(async () =>
        result({ outcome, files: 2, commits: 3, range: ["a", "b"], matching: 1 }),
      ),
    });

  test("project_partially_updated_if_it_does_not_finish", async () => {
    const viewCommits = vi.fn();

    await flow(
      partial({ kind: "error", reason: "failure" }),
      [],
      viewCommits,
    ).updateProject();

    expect(titles()).toEqual(["Cannot update", "Project Partially Updated"]);
    const warning = titled("Project Partially Updated");
    expect(warning.kind).toBe("warning");
    expect(warning.body).toBe("2 files updated in 3 commits");
    click("Project Partially Updated", "View Commits");
    expect(viewCommits).toHaveBeenCalledWith(["a", "b"]);
  });

  test("cancelled_with_received_commits_is_also_partial", async () => {
    await flow(partial({ kind: "cancelled" })).updateProject();

    expect(titles()).toEqual(["Project Partially Updated"]);
  });

  test("without_received_commits_it_is_still_update_canceled", async () => {
    const backend = api({
      update: vi.fn(async () => result({ outcome: { kind: "cancelled" } })),
    });

    await flow(backend).updateProject();

    expect(titles()).toEqual(["Update canceled"]);
  });

  test("without_viewcommits_there_is_no_button", async () => {
    const backend = partial({ kind: "cancelled" });

    await flow(backend).updateProject();

    expect(labels("Project Partially Updated")).toBeUndefined();
  });
});

describe("pull", () => {
  const pulled = (update: Partial<UpdateResult>) =>
    api({ pull: vi.fn(async () => result(update)) });

  test("the_dialog_receives_the_info_and_the_remembered_options", async () => {
    const backend = api({ pullOptions: vi.fn(async () => ["squash", "rebase"] as PullOption[]) });
    const { pull, ask } = flow(backend, [null]);

    await pull();

    const request = ask.mock.calls[0][0];
    expect(request).toMatchObject({ kind: "pull", info: PULL_INFO, options: ["squash", "rebase"] });
    expect(backend.pull).not.toHaveBeenCalled();
  });

  test("accepting_launches_the_pull_and_remembers_the_options", async () => {
    const backend = api();
    const choice: PullChoice = { remote: "origin", branch: "other", options: ["noFf", "noVerify"] };

    await flow(backend, [choice]).pull();

    expect(backend.pull).toHaveBeenCalledWith("origin", "other", ["noFf", "noVerify"]);
    expect(backend.savePullOptions).toHaveBeenCalledWith(["noFf", "noVerify"]);
  });

  test("cancelling_launches_nothing_nor_saves_the_options", async () => {
    const backend = api();

    await flow(backend, [null]).pull();

    expect(backend.pull).not.toHaveBeenCalled();
    expect(backend.savePullOptions).not.toHaveBeenCalled();
    expect(shown()).toEqual([]);
  });

  test("if_the_options_cannot_be_read_or_saved_the_pull_goes_on", async () => {
    const fail = async () => {
      throw new Error("no");
    };
    const backend = api({ pullOptions: vi.fn(fail), savePullOptions: vi.fn(fail) });
    const { ask, pull } = flow(backend, [PULL]);

    await pull();

    expect(ask.mock.calls[0][0]).toMatchObject({ kind: "pull", options: [] });
    expect(backend.pull).toHaveBeenCalledWith("origin", "main", []);
    expect(titles()).toEqual(["All files are up to date"]);
  });

  test("pull_with_received_commits_gives_the_same_notification_as_update", async () => {
    const viewCommits = vi.fn();
    const range: [string, string] = ["a", "b"];
    const backend = pulled({ files: 3, commits: 2, range, matching: 1 });

    await flow(backend, [PULL], viewCommits).pull();

    expect(titled("3 files updated in 2 commits").body).toBe("1 commit matching filters");
    click("3 files updated in 2 commits", "View Commits");
    expect(viewCommits).toHaveBeenCalledWith(range);
  });

  test("pull_with_nothing_new_says_all_files_are_up_to_date", async () => {
    await flow(pulled({}), [PULL]).pull();

    expect(titles()).toEqual(["All files are up to date"]);
  });

  test("pull_no_commit_counts_the_files_of_the_prepared_merge", async () => {
    await flow(pulled({ files: 2 }), [PULL]).pull();

    expect(titles()).toEqual(["2 files updated"]);
  });

  test("the_git_error_comes_out_with_its_title_and_without_update_canceled", async () => {
    const backend = pulled({
      outcome: { kind: "error", reason: "fatal: Not possible to fast-forward, aborting." },
      title: "Git Pull failed",
    });

    await flow(backend, [PULL]).pull();

    expect(titles()).toEqual(["Git Pull failed"]);
    expect(titled("Git Pull failed").kind).toBe("error");
    expect(titled("Git Pull failed").body).toBe("fatal: Not possible to fast-forward, aborting.");
  });

  test("a_conflict_leaves_the_notification_with_abort_like_update", async () => {
    const backend = pulled({ outcome: { kind: "conflicts" }, inProgress: true });
    const { pull } = flow(backend, [PULL]);

    await pull();

    expect(titles()).toEqual(["Cannot complete update"]);
    expect(titled("Cannot complete update").sticky).toBe(true);
    expect(labels("Cannot complete update")).toEqual(["Abort"]);
    click("Cannot complete update", "Abort");
    await vi.waitFor(() => expect(backend.abort).toHaveBeenCalled());
  });

  test("cancelled_with_a_half_done_merge_offers_abort_and_without_it_says_nothing", async () => {
    const half = pulled({ outcome: { kind: "cancelled" }, inProgress: true });
    await flow(half, [PULL]).pull();
    expect(titles()).toEqual(["Cannot complete update"]);

    shown().forEach(({ id }) => dismiss(id));
    await flow(pulled({ outcome: { kind: "cancelled" } }), [PULL]).pull();
    expect(titles()).toEqual([]);
  });

  test("with_rebase_update_rules_apply_including_update_canceled", async () => {
    const backend = pulled({
      outcome: { kind: "notReady", reason: "fatal: no remote" },
      title: "Update failed",
    });

    await flow(backend, [{ ...PULL, options: ["rebase"] }]).pull();

    expect(titles()).toEqual(["Update failed", "Update canceled"]);
  });

  test("a_half_done_rebase_pull_leaves_the_saved_changes_like_update", async () => {
    const backend = pulled({
      outcome: { kind: "conflicts" },
      saved: SAVED,
      restore: null,
      inProgress: true,
    });

    await flow(backend, [{ ...PULL, options: ["rebase"] }]).pull();

    expect(titles()).toEqual(["Local changes were not restored"]);
  });

  test("a_failure_launching_the_pull_is_git_pull_failed", async () => {
    const backend = api({
      pull: vi.fn(async () => {
        throw new Error("boom");
      }),
    });

    await flow(backend, [PULL]).pull();

    expect(titles()).toEqual(["Git Pull failed"]);
  });

  test("reloading_branches_launches_fetch_of_the_remote", async () => {
    const fresh: PullInfo = { ...PULL_INFO, remoteBranches: ["origin/main", "origin/fresh"] };
    const backend = api({
      pullInfo: vi.fn().mockResolvedValueOnce(PULL_INFO).mockResolvedValueOnce(fresh),
      fetchRemote: vi.fn(async () => ["origin/old"]),
    });
    const { pull, ask } = flow(backend, [null]);

    await pull();
    const request = ask.mock.calls[0][0];
    if (request.kind !== "pull") throw new Error("not a pull");
    const reloaded = await request.reload("origin");

    expect(backend.fetchRemote).toHaveBeenCalledWith("origin");
    expect(reloaded).toEqual(fresh);
    expect(titled("Fetch Successful").body).toBe("origin/old");
  });

  test("reloading_that_fails_warns_and_brings_no_branches", async () => {
    const backend = api({
      fetchRemote: vi.fn(async () => {
        throw new Error("no network");
      }),
    });
    const { pull, ask } = flow(backend, [null]);

    await pull();
    const request = ask.mock.calls[0][0];
    if (request.kind !== "pull") throw new Error("not a pull");

    expect(await request.reload("origin")).toBeNull();
    expect(titled("Fetch Failed").body).toBe("no network");
    expect(backend.pullInfo).toHaveBeenCalledTimes(1);
  });

  test("git_pull_is_an_action_and_does_not_overlap_with_another_one_in_progress", async () => {
    const backend = api();
    const { run } = flow(backend, [PULL]);

    await run("Git.Pull");

    expect(backend.pull).toHaveBeenCalledWith("origin", "main", []);
    expect(UPDATE_ACTIONS).toEqual(["Vcs.UpdateProject", "UpdateFiles", "Git.Fetch", "Git.Pull"]);
  });
});

describe("saved changes", () => {
  const conflicts = () =>
    result({ outcome: { kind: "conflicts" }, saved: SAVED, inProgress: true });
  const NOT_RESTORED = "Local changes were not restored";

  test("saved_changes_offer_restore_changes", async () => {
    const backend = api({
      update: vi.fn(async () =>
        result({
          saved: SAVED,
          restore: { kind: "failed", reason: "index.lock" },
        }),
      ),
    });

    await flow(backend).updateProject();

    const notification = titled(NOT_RESTORED);
    expect(notification.sticky).toBe(true);
    expect(notification.body).toContain("index.lock");
    expect(notification.body).toContain(SAVED.message);
    expect(labels(NOT_RESTORED)).toEqual(["Restore Changes"]);
    click(NOT_RESTORED, "Restore Changes");
    await vi.waitFor(() => expect(backend.restore).toHaveBeenCalledWith(SAVED));
  });

  const blocked: RestoreOutcome = { kind: "restoreBlocked", reason: { files: ["a.txt", "b.txt"] } };

  test("devin_wrote_over_it_names_the_files_and_says_what_to_do", async () => {
    const backend = api({
      update: vi.fn(async () => result({ saved: SAVED, restore: blocked })),
    });

    await flow(backend).updateProject();

    expect(labels(NOT_RESTORED)).toEqual(["Restore Changes"]);
    const { body } = titled(NOT_RESTORED);
    expect(body).toContain("a.txt\nb.txt");
    expect(body).toContain("must be committed or moved first");
    expect(body).toContain(SAVED.message);
    expect(body).not.toMatch(/from Remote/);
  });

  test("a_real_conflict_does_not_mention_the_disk_version", async () => {
    const backend = api({
      update: vi.fn(async () => result({ saved: SAVED, restore: { kind: "conflicts" } })),
    });

    await flow(backend).updateProject();

    expect(titled(NOT_RESTORED).body).toContain("have conflicts with files from Remote");
    expect(titled(NOT_RESTORED).body).not.toContain("on disk");
  });

  test("restoring_again_when_blocked_names_the_files_again", async () => {
    const backend = api({
      update: vi.fn(async () => result({ saved: SAVED, restore: blocked })),
      restore: vi.fn(async () => blocked),
    });
    await flow(backend).updateProject();
    const { id } = titled(NOT_RESTORED);

    click(NOT_RESTORED, "Restore Changes");
    dismiss(id);

    await vi.waitFor(() => expect(titled(NOT_RESTORED)?.body).toContain("a.txt"));
  });

  // Item 1: the backend refuses with the operation half-done; here both buttons remain.
  test("restoring_with_something_half_done_keeps_both_buttons", async () => {
    const backend = api({
      update: vi.fn(async () => conflicts()),
      restore: vi.fn(async () => ({ kind: "failed" as const, reason: "Finish or abort first" })),
    });
    await flow(backend).updateProject();
    const { id } = titled(NOT_RESTORED);

    click(NOT_RESTORED, "Restore Changes");
    dismiss(id);

    await vi.waitFor(() => expect(titled(NOT_RESTORED)?.body).toContain("Finish or abort first"));
    expect(labels(NOT_RESTORED)).toEqual(["Restore Changes", "Abort"]);
  });

  test("if_the_entry_is_gone_the_card_goes_away_and_does_not_come_back", async () => {
    const restore = vi.fn(async (): Promise<RestoreOutcome> => ({ kind: "gone" }));
    const backend = api({
      update: vi.fn(async () => result({ saved: SAVED, restore: { kind: "conflicts" } })),
      restore,
    });
    await flow(backend).updateProject();
    const { id } = titled(NOT_RESTORED);

    click(NOT_RESTORED, "Restore Changes");
    dismiss(id);

    await vi.waitFor(() => expect(restore).toHaveBeenCalled());
    await restore.mock.results[0].value;
    await Promise.resolve();
    expect(titles()).not.toContain(NOT_RESTORED);
  });

  test("if_someone_else_took_the_entry_during_the_update_there_is_no_card", async () => {
    const backend = api({
      update: vi.fn(async () => result({ saved: SAVED, restore: { kind: "gone" } })),
    });

    await flow(backend).updateProject();

    expect(titles()).toEqual(["All files are up to date"]);
  });

  test("restoring_again_without_success_leaves_the_button", async () => {
    const backend = api({
      update: vi.fn(async () => result({ saved: SAVED, restore: { kind: "conflicts" } })),
      restore: vi.fn(async () => ({ kind: "failed" as const, reason: "still cannot" })),
    });
    await flow(backend).updateProject();
    const { id } = titled(NOT_RESTORED);
    click(NOT_RESTORED, "Restore Changes");
    dismiss(id);

    await vi.waitFor(() => expect(titled(NOT_RESTORED)?.body).toContain("still cannot"));
    expect(labels(NOT_RESTORED)).toEqual(["Restore Changes"]);
  });

  test("restoring_without_stage_never_suggests_deleting_the_entry", async () => {
    const backend = api({
      update: vi.fn(async () =>
        result({ saved: SAVED, restore: { kind: "restoredWithoutIndex" } }),
      ),
    });

    await flow(backend).updateProject();

    const title = "Local changes were restored without staged changes";
    expect(titled(title).sticky).toBe(true);
    expect(titled(title).body).toContain(SAVED.message);
    expect(titled(title).body).not.toMatch(/drop/i);
    expect(titled(title).actions).toBeUndefined();
  });

  test("the_button_that_restores_without_stage_does_not_suggest_deleting_either", async () => {
    const backend = api({
      update: vi.fn(async () => result({ saved: SAVED, restore: { kind: "conflicts" } })),
      restore: vi.fn(async () => ({ kind: "restoredWithoutIndex" as const })),
    });
    await flow(backend).updateProject();

    click(NOT_RESTORED, "Restore Changes");

    const title = "Local changes were restored without staged changes";
    await vi.waitFor(() => expect(titled(title)).toBeTruthy());
    expect(titled(title).body).not.toMatch(/drop/i);
  });

  test("conflicts_offer_abort", async () => {
    const backend = api({ update: vi.fn(async () => conflicts()) });

    await flow(backend).updateProject();

    const notification = titled(NOT_RESTORED);
    expect(notification.sticky).toBe(true);
    expect(notification.body).toContain("Update is not complete");
    expect(notification.body).not.toMatch(/drop/i);
    expect(labels(NOT_RESTORED)).toEqual(["Restore Changes", "Abort"]);
  });

  test("conflicts_with_nothing_saved_also_offer_abort", async () => {
    const backend = api({
      update: vi.fn(async () => result({ outcome: { kind: "conflicts" }, inProgress: true })),
    });

    await flow(backend).updateProject();

    expect(titled("Cannot complete update").body).toBe("There are pending unresolved conflicts.");
    expect(titled("Cannot complete update").sticky).toBe(true);
    expect(labels("Cannot complete update")).toEqual(["Abort"]);
  });

  test("aborting_does_not_restore_and_leaves_the_button", async () => {
    const backend = api({ update: vi.fn(async () => conflicts()) });
    await flow(backend).updateProject();
    const { id } = titled(NOT_RESTORED);

    click(NOT_RESTORED, "Abort");
    dismiss(id);

    await vi.waitFor(() => expect(titled("Merge abort succeeded")).toBeTruthy());
    expect(backend.abort).toHaveBeenCalled();
    expect(backend.restore).not.toHaveBeenCalled();
    expect(labels(NOT_RESTORED)).toEqual(["Restore Changes"]);
    expect(titled(NOT_RESTORED).body).not.toContain("Resolve conflicts");
  });

  test("aborting_a_rebase_says_so", async () => {
    const backend = api({
      update: vi.fn(async () => conflicts()),
      abort: vi.fn(async () => "rebase" as const),
    });
    await flow(backend).updateProject();

    click(NOT_RESTORED, "Abort");

    await vi.waitFor(() => expect(titled("Abort rebase succeeded")).toBeTruthy());
  });

  test("aborting_a_squash_pull_with_conflicts_has_its_own_text", async () => {
    const backend = api({
      pull: vi.fn(async () => result({ outcome: { kind: "conflicts" }, inProgress: true })),
      abort: vi.fn(async () => "squash" as const),
    });
    await flow(backend, [PULL]).pull();

    click("Cannot complete update", "Abort");

    await vi.waitFor(() => expect(titled("Pull abort succeeded")).toBeTruthy());
    expect(backend.abort).toHaveBeenCalled();
  });

  test("if_aborting_fails_both_buttons_remain", async () => {
    const backend = api({
      update: vi.fn(async () => conflicts()),
      abort: vi.fn(async () => {
        throw { kind: "failed", message: "could not" };
      }),
    });
    await flow(backend).updateProject();
    const { id } = titled(NOT_RESTORED);

    click(NOT_RESTORED, "Abort");
    dismiss(id);

    await vi.waitFor(() => expect(titled("Abort failed")?.body).toBe("could not"));
    expect(labels(NOT_RESTORED)).toEqual(["Restore Changes", "Abort"]);
  });
});

describe("when opening a repository", () => {
  const NOT_RESTORED = "Local changes were not restored";
  const pending = { sha: "abc", title: "Checkout", message: "Uncommitted changes before Checkout" };
  const calm = { operation: null, branch: "main", label: "main", head: "abc" };
  const midway = { ...calm, operation: "merging" as const, label: "Merging main" };
  const opened = (overrides: Partial<RepoInfo> = {}) =>
    ({ pending: [pending], state: calm, ...overrides }) as RepoInfo;

  test("after_restarting_the_card_comes_back_with_restore_changes", async () => {
    const backend = api();
    const { repositoryChanged } = flow(backend);

    repositoryChanged(opened());

    const { sticky, body } = titled(NOT_RESTORED);
    expect(sticky).toBe(true);
    expect(body).toContain("before Checkout");
    expect(labels(NOT_RESTORED)).toEqual(["Restore Changes"]);
    click(NOT_RESTORED, "Restore Changes");
    await vi.waitFor(() =>
      expect(backend.restore).toHaveBeenCalledWith({ sha: "abc", message: pending.message }),
    );
  });

  test("with_something_half_done_it_also_offers_abort", () => {
    const { repositoryChanged } = flow(api());

    repositoryChanged(opened({ state: midway }));

    expect(labels(NOT_RESTORED)).toEqual(["Restore Changes", "Abort"]);
  });

  // Fixes the phase 2 finding: without a stash entry no button to abort came back.
  test("cannot_complete_update_comes_back_with_abort_even_without_an_entry", async () => {
    const backend = api();
    const { repositoryChanged } = flow(backend);

    repositoryChanged(opened({ pending: [], state: midway }));

    expect(labels("Cannot complete update")).toEqual(["Abort"]);
    click("Cannot complete update", "Abort");
    await vi.waitFor(() => expect(backend.abort).toHaveBeenCalled());
  });

  // An Update only leaves a merge or a rebase: the rest was started by the user outside here.
  test("update_card_does_not_appear_with_a_foreign_cherry_pick", () => {
    const { repositoryChanged } = flow(api());

    for (const operation of ["cherryPicking", "reverting"] as const) {
      repositoryChanged(opened({ pending: [], state: { ...calm, operation } }));
      expect(shown()).toEqual([]);
    }
    // With something saved, Restore Changes comes back, but without Abort over what is not ours.
    repositoryChanged(opened({ state: { ...calm, operation: "cherryPicking" } }));

    expect(labels(NOT_RESTORED)).toEqual(["Restore Changes"]);
  });

  test("card_abort_without_an_entry_asks_for_confirmation", async () => {
    const abortOperation = vi.fn(async () => true);
    const backend = api();
    const { repositoryChanged } = flow(backend, [], undefined, abortOperation);

    repositoryChanged(opened({ pending: [], state: midway }));
    click("Cannot complete update", "Abort");

    await vi.waitFor(() => expect(abortOperation).toHaveBeenCalledWith("merging"));
    // It does not go straight to `update_abort`: the confirmation and the HEAD belong to the operations
    // flow.
    expect(backend.abort).not.toHaveBeenCalled();
  });

  test("cancelling_the_confirmation_leaves_the_card_with_abort", async () => {
    const abortOperation = vi.fn(async () => false);
    const { repositoryChanged } = flow(api(), [], undefined, abortOperation);
    repositoryChanged(opened({ pending: [], state: midway }));
    const first = titled("Cannot complete update");

    first.actions?.[0].run();
    dismiss(first.id);

    await vi.waitFor(() => expect(abortOperation).toHaveBeenCalled());
    // Cancelling the question brings the card back out.
    await vi.waitFor(() => expect(labels("Cannot complete update")).toEqual(["Abort"]));
  });

  test("the_abort_of_an_update_with_something_saved_also_confirms", async () => {
    const abortOperation = vi.fn(async () => true);
    const backend = api({
      update: vi.fn(async () => result({ outcome: { kind: "conflicts" }, saved: SAVED, inProgress: true })),
      state: vi.fn(async () => ({ ...calm, operation: "rebasing" as const, label: "Rebasing main" })),
    });
    const { updateProject } = flow(backend, [], undefined, abortOperation);
    await updateProject();

    const before = titled(NOT_RESTORED);
    before.actions?.find(({ label }) => label === "Abort")?.run();
    dismiss(before.id);

    await vi.waitFor(() => expect(abortOperation).toHaveBeenCalledWith("rebasing"));
    expect(backend.abort).not.toHaveBeenCalled();
    await vi.waitFor(() => expect(labels(NOT_RESTORED)).toEqual(["Restore Changes"]));
  });

  test("a_closed_card_comes_back_when_opening_again", () => {
    const { repositoryChanged } = flow(api());
    repositoryChanged(opened());
    dismiss(titled(NOT_RESTORED).id);

    repositoryChanged(opened());

    expect(titles()).toEqual([NOT_RESTORED]);
  });

  test("with_nothing_pending_there_is_no_card", () => {
    flow(api()).repositoryChanged(opened({ pending: [] }));

    expect(shown()).toEqual([]);
  });

  test("switching_repository_dismisses_the_cards_that_act_on_the_previous_one", async () => {
    const backend = api({
      update: vi.fn(async () =>
        result({ outcome: { kind: "conflicts" }, saved: SAVED, inProgress: true }),
      ),
    });
    const { updateProject, repositoryChanged } = flow(backend);
    await updateProject();

    repositoryChanged(opened({ pending: [] }));

    expect(shown()).toEqual([]);
  });

  test("switching_repository_also_dismisses_the_abort_with_nothing_saved", async () => {
    const backend = api({
      update: vi.fn(async () => result({ outcome: { kind: "conflicts" }, inProgress: true })),
    });
    const { updateProject, repositoryChanged } = flow(backend);
    await updateProject();

    repositoryChanged(opened({ pending: [] }));

    expect(shown()).toEqual([]);
  });

  // What finishes after switching repository cannot leave buttons: `Abort` would act on the new one.
  function waitingUpdate() {
    let finish = (_: UpdateResult) => {};
    const update = vi.fn(() => new Promise<UpdateResult>((resolve) => (finish = resolve)));
    return { update, finish: (value: UpdateResult) => finish(value) };
  }
  const buttons = () => shown().flatMap(({ actions }) => actions ?? []);

  test("an_update_that_stops_after_switching_repository_leaves_no_buttons", async () => {
    const { update, finish } = waitingUpdate();
    const { updateProject, repositoryChanged } = flow(api({ update }));
    const running = updateProject();
    await vi.waitFor(() => expect(update).toHaveBeenCalled());

    repositoryChanged(opened({ pending: [] }));
    finish(result({ outcome: { kind: "conflicts" }, saved: SAVED, inProgress: true }));
    await running;

    expect(buttons()).toEqual([]);
    expect(titles()).not.toContain(NOT_RESTORED);
  });

  test("what_finishes_after_switching_repository_warns_without_buttons", async () => {
    const { update, finish } = waitingUpdate();
    const { updateProject, repositoryChanged } = flow(api({ update }), [], vi.fn());
    const running = updateProject();
    await vi.waitFor(() => expect(update).toHaveBeenCalled());

    repositoryChanged(opened({ pending: [] }));
    finish(result({ files: 1, commits: 1, range: ["a", "b"] }));
    await running;

    expect(buttons()).toEqual([]);
    expect(titles()).toEqual(["The update of the previous repository finished"]);
  });

  test("choosing_the_tracked_branch_does_not_survive_another_repository_either", async () => {
    const backend = api({
      update: vi.fn(async () =>
        result({ outcome: { kind: "noTrackedBranch", reason: { branch: "suelta" } } }),
      ),
    });
    const { updateProject, repositoryChanged } = flow(backend);
    await updateProject();

    repositoryChanged(opened({ pending: [] }));

    expect(buttons()).toEqual([]);
  });

  test("rebasing_merge_commits_does_not_continue_in_another_repository", async () => {
    const update = vi.fn(async () => result());
    update.mockImplementationOnce(async () =>
      result({ outcome: { kind: "rebasingMergeCommits", reason: { branch: "feature" } } }),
    );
    const backend = api({ update });
    const { updateProject, repositoryChanged, ask } = flow(backend);
    ask.mockImplementation(async () => {
      repositoryChanged(opened({ pending: [] }));
      return "merge";
    });

    await updateProject();

    expect(update).toHaveBeenCalledTimes(1);
  });

  test("a_pull_that_stops_after_switching_repository_leaves_no_buttons", async () => {
    let finish = (_: UpdateResult) => {};
    const pull = vi.fn(() => new Promise<UpdateResult>((resolve) => (finish = resolve)));
    const backend = api({ pull });
    const answer = { remote: "origin", branch: "main", options: [] };
    const { pull: start, repositoryChanged } = flow(backend, [answer]);
    const running = start();
    await vi.waitFor(() => expect(pull).toHaveBeenCalled());

    repositoryChanged(opened({ pending: [] }));
    finish(result({ outcome: { kind: "conflicts" }, saved: SAVED, inProgress: true }));
    await running;

    expect(buttons()).toEqual([]);
  });

  test("an_abort_that_fails_after_switching_repository_does_not_bring_buttons_back", async () => {
    let fail = (_: unknown) => {};
    const abort = vi.fn(() => new Promise<Aborted>((_, reject) => (fail = reject)));
    const backend = api({
      abort,
      update: vi.fn(async () =>
        result({ outcome: { kind: "conflicts" }, saved: SAVED, inProgress: true }),
      ),
    });
    const { updateProject, repositoryChanged } = flow(backend);
    await updateProject();
    click(NOT_RESTORED, "Abort");

    repositoryChanged(opened({ pending: [] }));
    fail({ kind: "failed", message: "no" });
    await vi.waitFor(() => expect(titled("Abort failed")).toBeTruthy());

    expect(buttons()).toEqual([]);
  });

  test("restoring_without_success_after_switching_repository_does_not_bring_buttons_back", async () => {
    let finish = (_: RestoreOutcome) => {};
    const restore = vi.fn(() => new Promise<RestoreOutcome>((resolve) => (finish = resolve)));
    const backend = api({
      restore,
      update: vi.fn(async () => result({ saved: SAVED, restore: { kind: "conflicts" } })),
    });
    const { updateProject, repositoryChanged } = flow(backend);
    await updateProject();
    click(NOT_RESTORED, "Restore Changes");

    repositoryChanged(opened({ pending: [] }));
    finish({ kind: "failed", reason: "no" });
    await new Promise((resolve) => setTimeout(resolve));

    expect(buttons()).toEqual([]);
  });

  test("the_other_notifications_stay", async () => {
    const backend = api({
      update: vi.fn(async () => result({ outcome: { kind: "nothingToUpdate" } })),
    });
    const { updateProject, repositoryChanged } = flow(backend);
    await updateProject();

    repositoryChanged(opened({ pending: [] }));

    expect(titles()).toEqual(["All files are up to date"]);
  });
});

describe("tracked branch", () => {
  const untracked = () =>
    result({
      outcome: { kind: "noTrackedBranch", reason: { branch: "suelta" } },
      title: "Cannot update",
    });

  test("without_a_tracked_branch_it_offers_to_choose_one", async () => {
    const choice: TrackedChoice = { target: "origin/main", setUpstream: true, method: "rebase" };
    const update = vi.fn(async () => untracked());
    const backend = api({ update });
    const { updateProject, ask } = flow(backend, [choice]);

    await updateProject();

    expect(titled("Cannot update").body).toBe("suelta has no tracked branch");
    expect(titled("Update canceled")).toBeTruthy();
    update.mockImplementation(async () => result());
    click("Cannot update", "Choose upstream branch");
    await vi.waitFor(() =>
      expect(update).toHaveBeenLastCalledWith("rebase", {
        target: "origin/main",
        setUpstream: true,
      }),
    );
    expect(ask).toHaveBeenCalledWith({
      kind: "fixTracked",
      branch: "suelta",
      remoteBranches: ["origin/main", "origin/other"],
      method: "merge",
    });
  });

  test("the_dialog_proposes_the_one_with_the_same_name_and_sets_upstream", async () => {
    const onanswer = vi.fn();
    render(FixTrackedDialog, {
      branch: "other",
      remoteBranches: ["origin/main", "origin/other", "copy/x"],
      method: "merge",
      onanswer,
    });

    expect(screen.getByRole("dialog", { name: "Update" })).toBeTruthy();
    const branch = screen.getByPlaceholderText("Specify branch") as HTMLInputElement;
    expect(branch.value).toBe("other");

    await fireEvent.click(screen.getByRole("checkbox", { name: "Set as tracked branch" }));
    await fireEvent.click(
      screen.getByRole("radio", { name: "Rebase the current branch on top of incoming changes" }),
    );
    await fireEvent.click(screen.getByRole("button", { name: "Update" }));

    expect(onanswer).toHaveBeenCalledWith({
      target: "origin/other",
      setUpstream: true,
      method: "rebase",
    });
  });

  test("the_dialog_does_not_accept_an_empty_branch", async () => {
    const onanswer = vi.fn();
    render(FixTrackedDialog, {
      branch: "suelta",
      remoteBranches: ["origin/main"],
      method: "merge",
      onanswer,
    });

    await fireEvent.click(screen.getByRole("button", { name: "Update" }));

    expect(onanswer).not.toHaveBeenCalled();
  });
});

describe("Rebasing Merge Commits", () => {
  const stopped = () => result({ outcome: { kind: "rebasingMergeCommits", reason: { branch: "feature" } } });
  const rebasing = () => ({ ...SETTINGS, updateMethod: "rebase" }) as Settings;

  test("the_three_buttons", async () => {
    const onanswer = vi.fn();
    render(RebaseMergesDialog, { onanswer });

    expect(screen.getByRole("dialog", { name: "Rebasing Merge Commits" })).toBeTruthy();
    await fireEvent.click(screen.getByRole("button", { name: "Merge" }));
    await fireEvent.click(screen.getByRole("button", { name: "Rebase" }));
    await fireEvent.click(screen.getByRole("button", { name: "Cancel" }));

    expect(onanswer.mock.calls).toEqual([["merge"], ["rebase"], [null]]);
  });

  test("merge_repeats_with_merge_without_another_fetch", async () => {
    const update = vi.fn(async (): Promise<UpdateResult> => stopped());
    update.mockImplementationOnce(async () => stopped());
    update.mockImplementationOnce(async () => result());
    const backend = api({ update, settings: vi.fn(async () => rebasing()) });

    await flow(backend, ["merge"]).updateProject();

    expect(update).toHaveBeenLastCalledWith("merge", { fetched: true });
  });

  test("rebase_repeats_without_asking_again", async () => {
    const update = vi.fn(async (): Promise<UpdateResult> => stopped());
    update.mockImplementationOnce(async () => stopped());
    update.mockImplementationOnce(async () => result());
    const backend = api({ update, settings: vi.fn(async () => rebasing()) });

    await flow(backend, ["rebase"]).updateProject();

    expect(update).toHaveBeenLastCalledWith("rebase", { fetched: true, rebaseMerges: "feature" });
  });

  test("cancel_does_not_update", async () => {
    const update = vi.fn(async () => stopped());
    const backend = api({ update, settings: vi.fn(async () => rebasing()) });

    await flow(backend, [null]).updateProject();

    expect(update).toHaveBeenCalledTimes(1);
    expect(titles()).toEqual(["Update canceled"]);
  });
});

// Decision 1: Rust asks before saving, so `Cancel` has nothing to bring back.
describe("Rebasing Published Commit", () => {
  const stopped = () =>
    result({
      outcome: {
        kind: "rebasingPublishedCommits",
        reason: { branch: "feature", protected: "origin/main" },
      },
    });
  const rebasing = () => ({ ...SETTINGS, updateMethod: "rebase" }) as Settings;
  const stoppingOnce = () => {
    const update = vi.fn(async (): Promise<UpdateResult> => result());
    update.mockImplementationOnce(async () => stopped());
    return update;
  };

  test("the_outcome_opens_rebasing_published_commit_with_cancel_as_default", async () => {
    const update = stoppingOnce();
    const { updateProject, ask } = flow(api({ update, settings: vi.fn(async () => rebasing()) }));

    await updateProject();

    expect(ask).toHaveBeenCalledWith({ kind: "publishedCommits" });
    const onanswer = vi.fn();
    render(PublishedCommitsDialog, { onanswer });
    expect(screen.getByRole("dialog", { name: "Rebasing Published Commit" })).toBeTruthy();
    const cancel = screen.getByRole("button", { name: "Cancel" });
    expect(document.activeElement).toBe(cancel);
    expect(screen.getByRole("button", { name: "Rebase Anyway" })).toBeTruthy();
    await fireEvent.submit(cancel.closest("form") as HTMLFormElement);
    expect(onanswer).toHaveBeenLastCalledWith(false);
  });

  test("rebase_anyway_repeats_without_another_fetch_or_question", async () => {
    const update = stoppingOnce();
    const backend = api({ update, settings: vi.fn(async () => rebasing()) });
    const { updateProject, ask } = flow(backend, [true]);

    await updateProject();

    expect(ask).toHaveBeenCalledTimes(1);
    expect(update).toHaveBeenCalledTimes(2);
    expect(update).toHaveBeenLastCalledWith("rebase", { fetched: true, publishedConfirmed: "feature" });
    expect(titles()).toEqual(["All files are up to date"]);
  });

  test("cancel_shows_update_canceled", async () => {
    const update = vi.fn(async () => stopped());
    const backend = api({ update, settings: vi.fn(async () => rebasing()) });

    await flow(backend, [false]).updateProject();

    expect(update).toHaveBeenCalledTimes(1);
    expect(titles()).toEqual(["Update canceled"]);
  });

  test("closing_the_dialog_is_cancel", async () => {
    const update = vi.fn(async () => stopped());
    const backend = api({ update, settings: vi.fn(async () => rebasing()) });

    await flow(backend, [null]).updateProject();

    expect(update).toHaveBeenCalledTimes(1);
    expect(titles()).toEqual(["Update canceled"]);
  });

  test("update_selected_behaves_the_same", async () => {
    const update = stoppingOnce();
    const backend = api({ update, settings: vi.fn(async () => rebasing()) });
    const { run, ask } = flow(backend, [true]);

    await run("UpdateFiles");

    expect(ask).toHaveBeenCalledWith({ kind: "publishedCommits" });
    expect(update).toHaveBeenLastCalledWith("rebase", { fetched: true, publishedConfirmed: "feature" });
  });

  test("after_rebasing_merge_commits_it_keeps_the_first_answer", async () => {
    const update = vi.fn(async (): Promise<UpdateResult> => result());
    update.mockImplementationOnce(async () => result({ outcome: { kind: "rebasingMergeCommits", reason: { branch: "feature" } } }));
    update.mockImplementationOnce(async () => stopped());
    const backend = api({ update, settings: vi.fn(async () => rebasing()) });
    const { updateProject, ask } = flow(backend, ["rebase", true]);

    await updateProject();

    expect(ask.mock.calls.map(([request]) => request.kind)).toEqual([
      "rebaseMerges",
      "publishedCommits",
    ]);
    expect(update).toHaveBeenLastCalledWith("rebase", {
      fetched: true,
      rebaseMerges: "feature",
      publishedConfirmed: "feature",
    });
  });

  test("pull_into_with_rebase_asks_and_repeats_towards_the_same_branch", async () => {
    const update = vi.fn(async () => result());
    const backend = api({ update, pullInto: vi.fn(async () => stopped()) });

    await flow(backend, [true]).pullInto("origin/other", "rebase");

    expect(update).toHaveBeenCalledWith("rebase", {
      target: "origin/other",
      fetched: true,
      publishedConfirmed: "feature",
    });
  });

  test("it_does_not_continue_in_another_repository", async () => {
    const update = stoppingOnce();
    const backend = api({ update, settings: vi.fn(async () => rebasing()) });
    const { updateProject, repositoryChanged, ask } = flow(backend);
    ask.mockImplementation(async () => {
      repositoryChanged({ pending: [], state: { operation: null } } as unknown as RepoInfo);
      return true;
    });

    await updateProject();

    expect(update).toHaveBeenCalledTimes(1);
    expect(titles()).toEqual([]);
  });
});

test("a_push_that_updates_uses_the_same_turn_as_update", async () => {
  const backend = api();
  const { forPush, updateProject } = flow(backend, [{ method: "merge", hide: false }]);
  let release = () => {};

  const pushing = forPush.exclusive(() => new Promise<void>((resolve) => (release = resolve)));
  await updateProject();

  expect(backend.update).not.toHaveBeenCalled();
  expect(await forPush.exclusive(async () => {})).toBe(false);
  release();
  expect(await pushing).toBe(true);
});

test("after_a_push_the_notices_are_those_of_update_without_a_summary", () => {
  const { forPush } = flow(api());

  forPush.report(result({ outcome: { kind: "conflicts" }, inProgress: true }), true);

  expect(shown().map(({ title }) => title)).toEqual(["Cannot complete update"]);
});

test("after_a_push_what_ended_well_reports_nothing", () => {
  const { forPush } = flow(api());

  forPush.report(result({ commits: 2 }), true);

  expect(shown()).toEqual([]);
});

test("after_a_push_in_another_repository_there_are_no_buttons", () => {
  const { forPush } = flow(api());

  forPush.report(result({ outcome: { kind: "conflicts" }, inProgress: true }), false);

  expect(shown().map(({ title }) => title)).toEqual([
    "The update of the previous repository finished",
  ]);
});

describe("what another operation saved", () => {
  const NOT_RESTORED = "Local changes were not restored";
  const REBASE_SAVED = { sha: "r1", message: "Uncommitted changes before rebase" };
  const SAVING = { operation: "rebase", from: "main" };

  test("pull_into_is_an_update_towards_the_remote_with_its_notices", async () => {
    const backend = api({ pullInto: vi.fn(async () => result({ files: 2, commits: 1 })) });

    await flow(backend).pullInto("origin/other", "rebase");

    expect(backend.pullInto).toHaveBeenCalledWith("origin/other", "rebase");
    expect(titles()).toContain("2 files updated in 1 commit");
  });

  test("what_did_not_come_back_is_announced_with_restore_changes_and_its_text", () => {
    const update = flow(api());

    update.showRestored(
      [
        { saved: SAVED, restore: { kind: "restored" } },
        { saved: REBASE_SAVED, restore: { kind: "conflicts" } },
      ],
      SAVING,
    );

    expect(titles()).toEqual([NOT_RESTORED]);
    expect(titled(NOT_RESTORED).body).toBe(
      `Uncommitted changes that were saved before rebase have conflicts with files from main\nThey are in the stash: ${REBASE_SAVED.message}`,
    );
    expect(labels(NOT_RESTORED)).toEqual(["Restore Changes"]);
  });

  test("restoring_goes_from_last_to_first_and_stops_if_one_does_not_come_back", async () => {
    const restore = vi
      .fn()
      .mockResolvedValueOnce({ kind: "restoreBlocked", reason: { files: ["a.txt"] } });
    const update = flow(api({ restore }));

    await update.restoreSaved([SAVED, REBASE_SAVED], SAVING);

    expect(restore).toHaveBeenCalledTimes(1);
    expect(restore).toHaveBeenCalledWith(REBASE_SAVED);
    expect(shown().filter(({ title }) => title === NOT_RESTORED)).toHaveLength(2);
  });

  test("if_everything_comes_back_nothing_is_announced", async () => {
    const update = flow(api());

    await update.restoreSaved([REBASE_SAVED], SAVING);

    expect(shown()).toEqual([]);
  });
});

// Decision 5: Fetch is disabled while any fetch of the app runs, and the whole Update counts.
describe("fetching flag", () => {
  const pending = <T>() => {
    let settle = (_: T) => {};
    let fail = (_: unknown) => {};
    const promise = new Promise<T>((resolve, reject) => {
      settle = resolve;
      fail = reject;
    });
    return { promise, settle, fail };
  };
  const running = () => get(fetching);

  test("fetch_is_disabled_while_any_fetch_runs", async () => {
    expect(running()).toBe(false);

    const explicit = pending<{ pruned: string[]; failed: never[] }>();
    const run = flow(api({ fetch: vi.fn(() => explicit.promise) })).fetch();
    expect(running()).toBe(true);
    explicit.settle({ pruned: [], failed: [] });
    await run;
    expect(running()).toBe(false);

    const updating = pending<UpdateResult>();
    const updated = flow(api({ update: vi.fn(() => updating.promise) })).updateProject();
    await vi.waitFor(() => expect(running()).toBe(true));
    updating.settle(result());
    await updated;
    expect(running()).toBe(false);

    const pulling = pending<UpdateResult>();
    const pulled = flow(api({ pull: vi.fn(() => pulling.promise) }), [PULL]).pull();
    await vi.waitFor(() => expect(running()).toBe(true));
    pulling.settle(result());
    await pulled;
    expect(running()).toBe(false);
  });

  test("it_comes_back_after_a_failed_or_cancelled_fetch", async () => {
    await flow(
      api({
        fetch: vi.fn(async () => {
          throw { kind: "cancelled", message: "" };
        }),
      }),
    ).fetch();
    expect(running()).toBe(false);

    await flow(
      api({
        update: vi.fn(async () => {
          throw new Error("boom");
        }),
      }),
    ).updateProject();
    expect(running()).toBe(false);
  });

  test("it_is_subscribable_and_stays_on_until_the_last_overlapping_run_ends", async () => {
    const first = pending<{ pruned: string[]; failed: never[] }>();
    const second = pending<UpdateResult>();
    const seen: boolean[] = [];
    const stop = fetching.subscribe((value) => seen.push(value));

    const one = flow(api({ fetch: vi.fn(() => first.promise) })).fetch();
    const two = flow(api({ update: vi.fn(() => second.promise) })).updateProject();
    await vi.waitFor(() => expect(running()).toBe(true));
    first.settle({ pruned: [], failed: [] });
    await one;
    expect(running()).toBe(true);
    second.settle(result());
    await two;
    stop();

    expect(seen).toEqual([false, true, false]);
  });
});
