import { get } from "svelte/store";
import { beforeEach, describe, expect, test, vi } from "vitest";
import type { PushInfo, PushResult, UpdateOutcome, UpdateResult } from "../lib/api";
import { dismiss, notifications } from "../lib/notifications";
import {
  commitTooltip,
  pushFlow,
  pushNotification,
  pushSource,
  pushTitle,
  shownCommits,
  targetIsNew,
  targetNames,
  targetProblem,
  type PushApi,
  type PushRequest,
} from "./model";

const HASH = "a1b2c3d4".repeat(5);

const INFO: PushInfo = {
  repository: "rama",
  branch: "main",
  head: HASH,
  remotes: ["origin", "upstream"],
  remoteBranches: ["origin/main", "origin/feature/x", "upstream/dev"],
  target: { remote: "origin", branch: "main", isNew: false },
  prePushHook: false,
  protectedBranches: ["master", "main"],
  forcePushTargets: [],
};

const RESULT: PushResult = {
  status: { kind: "success" },
  pushed: 2,
  source: "main",
  destination: "origin/main",
  tags: [],
  remote: "origin",
  updatable: null,
};

function api(overrides: Partial<PushApi> = {}): PushApi {
  return {
    info: vi.fn(async () => INFO),
    branchOf: vi.fn(async () => "main"),
    outgoing: vi.fn(async () => []),
    push: vi.fn(async () => RESULT),
    updateAndPush: vi.fn(),
    tags: vi.fn(async () => null),
    saveTags: vi.fn(async () => {}),
    settings: vi.fn(),
    saveSettings: vi.fn(async () => {}),
    rememberForceTarget: vi.fn(async () => {}),
    ...overrides,
  };
}

const UPDATE: UpdateResult = {
  outcome: { kind: "success" },
  title: null,
  files: 1,
  commits: 2,
  range: ["a", "b"],
  matching: null,
  saved: null,
  restore: null,
  inProgress: false,
  submoduleError: null,
};

const CHOICE = {
  remote: "origin",
  target: "main",
  tags: null,
  noVerify: false,
  force: false,
  preview: true,
  rememberTarget: false,
};

function ui(answer: unknown = CHOICE) {
  const asked: PushRequest[] = [];
  return {
    asked,
    ask: async (request: PushRequest) => {
      asked.push(request);
      return answer as never;
    },
    updates: { exclusive: async () => true, report: () => {} },
  };
}

const shown = () => get(notifications);
const titles = () => shown().map(({ title }) => title);

beforeEach(() => {
  for (const { id } of shown()) dismiss(id);
});

test("title_push_commits_to_repo", () => {
  expect(pushTitle("rama")).toBe("Push Commits to rama");
});

test("shows_20_commits_and_the_rest_with_dots", () => {
  const commits = Array.from({ length: 45 }, (_, index) => index);

  expect(shownCommits(commits, 20)).toEqual({ shown: commits.slice(0, 20), more: true });
  expect(shownCommits(commits, 40).more).toBe(true);
  expect(shownCommits(commits, 80)).toEqual({ shown: commits, more: false });
  expect(shownCommits(commits.slice(0, 20), 20).more).toBe(false);
});

test("the_source_is_the_branch_or_the_commit", () => {
  expect(pushSource(INFO)).toEqual({ revision: "main", label: "main" });
  expect(pushSource(INFO, HASH)).toEqual({ revision: HASH, label: HASH.slice(0, 8) });
  expect(pushSource({ ...INFO, branch: null })).toEqual({
    revision: HASH,
    label: HASH.slice(0, 8),
  });
});

test("new_is_the_one_the_remote_does_not_know", () => {
  expect(targetIsNew(INFO, "origin", "main")).toBe(false);
  expect(targetIsNew(INFO, "upstream", "main")).toBe(true);
  expect(targetIsNew(INFO, "origin", "other")).toBe(true);
  expect(targetIsNew(INFO, "origin", "feature/x")).toBe(false);
});

test("a_special_reference_is_always_new", () => {
  const info = { ...INFO, target: { remote: "origin", branch: "refs/for/main", isNew: true } };
  expect(targetIsNew(info, "origin", "refs/for/main")).toBe(true);
});

test("what_the_remote_no_longer_has_is_new_even_if_the_starting_target_was_not_new", () => {
  const info = { ...INFO, target: { remote: "origin", branch: "gone", isNew: false } };
  expect(targetIsNew(info, "origin", "gone")).toBe(true);
});

test("names_to_complete_the_target_are_those_of_that_remote", () => {
  expect(targetNames(INFO, "origin")).toEqual(["main", "feature/x"]);
  expect(targetNames(INFO, "upstream")).toEqual(["dev"]);
});

test("an_invalid_target_says_so", () => {
  expect(targetProblem("main")).toBeNull();
  expect(targetProblem("feature/x")).toBeNull();
  for (const name of ["", " ", "a b", "a:b", "-x", "+x", "a..b", "a~1", "x.lock"]) {
    expect(targetProblem(name), name).toBe(`Invalid destination branch name: ${name}`);
  }
});

test("a_commit_tooltip_says_who_and_which", () => {
  const text = commitTooltip({ hash: HASH, subject: "one", author: "Ana", time: 0 });

  expect(text).toContain(HASH.slice(0, 8));
  expect(text).toContain("Ana");
  expect(text).toContain("one");
});

describe("notifications", () => {
  const title = (result: Partial<PushResult>) => pushNotification({ ...RESULT, ...result }).title;

  test("success_notifications_literal", () => {
    expect(title({})).toBe("Pushed 2 commits to origin/main");
    expect(title({ pushed: 1 })).toBe("Pushed 1 commit to origin/main");
    expect(title({ status: { kind: "newBranch" } })).toBe(
      "Pushed main to new branch origin/main",
    );
    expect(title({ status: { kind: "forced" } })).toBe("Force pushed main to origin/main");
    expect(pushNotification(RESULT).kind).toBe("info");
  });

  test("con_tags", () => {
    expect(title({ tags: ["v1"] })).toBe(
      "Pushed 2 commits to origin/main, and tag v1 to origin",
    );
    expect(title({ tags: ["v1", "v2"] })).toBe(
      "Pushed 2 commits to origin/main, and 2 tags to origin",
    );
    const newBranch = { status: { kind: "newBranch" } } as const;
    expect(title({ ...newBranch, tags: ["v1"] })).toBe(
      "Pushed main to new branch origin/main, and tag v1 to origin",
    );
    expect(title({ ...newBranch, tags: ["v1", "v2"] })).toBe(
      "Pushed main to new branch origin/main, and 2 tags to origin",
    );
  });

  test("everything_is_up_to_date", () => {
    const upToDate = { status: { kind: "upToDate" } } as const;
    expect(title(upToDate)).toBe("Everything is up to date");
    expect(title({ ...upToDate, tags: ["v1"] })).toBe("Pushed tag v1 to origin");
    expect(title({ ...upToDate, tags: ["v1", "v2"] })).toBe("Pushed 2 tags to origin");
  });

  test("rejections", () => {
    const of = (status: PushResult["status"]) => pushNotification({ ...RESULT, status });

    expect(of({ kind: "rejectedNoFf" })).toMatchObject({
      kind: "warning",
      title: "Push rejected",
      body: "Push to origin/main was rejected",
    });
    expect(of({ kind: "rejectedStale" })).toMatchObject({
      kind: "warning",
      title: "Push rejected",
      body: "Force-with-lease push main to origin/main was rejected",
    });
    expect(of({ kind: "rejectedOther", reason: "hook declined" })).toMatchObject({
      kind: "warning",
      title: "Push rejected",
      body: "Push main to origin/main was rejected by the remote\nhook declined",
    });
  });

  test("a_rejection_after_updating_says_what_happened_with_the_update", () => {
    const rejected: PushResult = { ...RESULT, status: { kind: "rejectedNoFf" } };
    const after = (outcome: UpdateOutcome) =>
      pushNotification(rejected, { ...UPDATE, outcome }).body;

    expect(after({ kind: "success" })).toBe("Push to origin/main was rejected");
    expect(after({ kind: "nothingToUpdate" })).toBe("Push to origin/main was rejected");
    expect(after({ kind: "cancelled" })).toBe("Push was rejected, and update was canceled");
    expect(after({ kind: "error", reason: "x" })).toBe(
      "Push was rejected, and update failed with an error",
    );
    expect(after({ kind: "notReady", reason: "x" })).toBe(
      "Push was rejected, and update failed with an error",
    );
    expect(after({ kind: "conflicts" })).toBe(
      "Push was canceled, because not all conflicts were resolved during the update.\nResolve the conflicts and try pushing again",
    );
  });

  test("what_was_received_is_singular_if_it_is_one", () => {
    expect(pushNotification(RESULT, { ...UPDATE, commits: 1 }).body).toBe(
      "1 commit received during the push operation",
    );
    expect(pushNotification(RESULT, { ...UPDATE, commits: 0 }).body).toBeUndefined();
  });

  test("a_failure_carries_what_git_said", () => {
    expect(
      pushNotification({ ...RESULT, status: { kind: "error", reason: "fatal: no" } }),
    ).toMatchObject({ kind: "error", title: "Push failed", body: "fatal: no" });
  });
});

describe("flow", () => {
  test("cmd_shift_k_alone_is_the_current_branch", async () => {
    const backend = api();
    const dialogs = ui();

    await pushFlow(dialogs, backend).push();

    expect(backend.info).toHaveBeenCalledWith(null);
    expect(dialogs.asked[0]).toMatchObject({
      kind: "push",
      info: INFO,
      source: { revision: "main", label: "main" },
      tags: null,
    });
    expect(backend.push).toHaveBeenCalledWith({
      source: "main",
      remote: "origin",
      target: "main",
      tags: null,
      force: null,
      noVerify: false,
    });
    expect(titles()).toEqual(["Pushed 2 commits to origin/main"]);
  });

  test("cancelling_neither_pushes_nor_remembers_anything", async () => {
    const backend = api();

    await pushFlow(ui(null), backend).push();

    expect(backend.push).not.toHaveBeenCalled();
    expect(backend.saveTags).not.toHaveBeenCalled();
  });

  test("push_of_a_branch_opens_with_that_branch", async () => {
    const info = { ...INFO, branch: "other" };
    const backend = api({ info: vi.fn(async () => info) });
    const dialogs = ui();

    await pushFlow(dialogs, backend).pushBranch("other");

    expect(backend.info).toHaveBeenCalledWith("other");
    expect(dialogs.asked[0]).toMatchObject({ source: { revision: "other", label: "other" } });
  });

  test("push_all_up_to_here_opens_with_that_commit", async () => {
    const backend = api({ branchOf: vi.fn(async () => "branch") });
    const dialogs = ui();

    await pushFlow(dialogs, backend).pushUpTo(HASH);

    expect(backend.branchOf).toHaveBeenCalledWith(HASH);
    expect(backend.info).toHaveBeenCalledWith("branch");
    expect(dialogs.asked[0]).toMatchObject({
      source: { revision: HASH, label: HASH.slice(0, 8) },
    });
    expect(backend.push).toHaveBeenCalledWith(expect.objectContaining({ source: HASH }));
  });

  test("push_all_up_to_here_outside_local_branches_opens_the_push_dialog", async () => {
    // With a current branch the target of the dialog is `<remote>/<current>`.
    const backend = api({ branchOf: vi.fn(async () => null) });
    const dialogs = ui();

    await pushFlow(dialogs, backend).pushUpTo(HASH);

    expect(backend.info).toHaveBeenCalledWith(null);
    expect(dialogs.asked[0]).toMatchObject({
      source: { revision: HASH, label: HASH.slice(0, 8) },
      info: { target: { remote: "origin", branch: "main" } },
    });
    expect(backend.push).toHaveBeenCalledWith(
      expect.objectContaining({ source: HASH, remote: "origin", target: "main" }),
    );
    expect(shown().filter(({ kind }) => kind === "error")).toEqual([]);
  });

  test("push_all_up_to_here_on_detached_head_targets_a_new_detached_branch", async () => {
    const info = {
      ...INFO,
      branch: null,
      target: { remote: "origin", branch: "detached", isNew: true },
    };
    const backend = api({ branchOf: vi.fn(async () => null), info: vi.fn(async () => info) });
    const dialogs = ui({ ...CHOICE, target: "detached" });

    await pushFlow(dialogs, backend).pushUpTo(HASH);

    expect(dialogs.asked[0]).toMatchObject({
      source: { revision: HASH, label: HASH.slice(0, 8) },
      info: { target: { branch: "detached", isNew: true } },
    });
    expect(backend.push).toHaveBeenCalledWith(
      expect.objectContaining({ source: HASH, target: "detached" }),
    );
    expect(shown().filter(({ kind }) => kind === "error")).toEqual([]);
  });

  test("push_tags_remembers_the_choice", async () => {
    const backend = api({ tags: vi.fn(async () => "currentBranch" as const) });
    const dialogs = ui({ ...CHOICE, tags: "all" });

    await pushFlow(dialogs, backend).push();

    expect(dialogs.asked[0]).toMatchObject({ tags: "currentBranch" });
    expect(backend.saveTags).toHaveBeenCalledWith("all");
    expect(backend.push).toHaveBeenCalledWith(expect.objectContaining({ tags: "all" }));
  });

  test("no_tags_is_remembered_too", async () => {
    const backend = api({ tags: vi.fn(async () => "all" as const) });

    await pushFlow(ui(), backend).push();

    expect(backend.saveTags).toHaveBeenCalledWith(null);
  });

  test("if_the_choice_cannot_be_read_or_saved_the_push_goes_on", async () => {
    const backend = api({
      tags: vi.fn(async () => {
        throw new Error("no");
      }),
      saveTags: vi.fn(async () => {
        throw new Error("no");
      }),
    });
    const dialogs = ui();

    await pushFlow(dialogs, backend).push();

    expect(dialogs.asked[0]).toMatchObject({ tags: null });
    expect(backend.push).toHaveBeenCalled();
  });

  test("the_commits_come_from_the_chosen_source", async () => {
    const backend = api();
    const dialogs = ui();

    await pushFlow(dialogs, backend).pushUpTo(HASH);
    await dialogs.asked[0].outgoing("upstream", "dev");

    expect(backend.outgoing).toHaveBeenCalledWith(HASH, "upstream", "dev");
  });

  test("a_failure_on_open_is_reported_and_there_is_no_dialog", async () => {
    const backend = api({
      info: vi.fn(async () => {
        throw { kind: "failed", message: "could not" };
      }),
    });
    const dialogs = ui();

    await pushFlow(dialogs, backend).push();

    expect(dialogs.asked).toEqual([]);
    expect(shown().at(-1)).toMatchObject({ kind: "error", body: "could not" });
  });

  test("cancelling_the_push_in_progress_is_not_a_failure", async () => {
    const backend = api({
      push: vi.fn(async () => {
        throw { kind: "cancelled", message: "" };
      }),
    });

    await pushFlow(ui(), backend).push();

    expect(shown()).toEqual([]);
  });

  test("a_new_push_dismisses_the_previous_warnings", async () => {
    const rejected: PushResult = { ...RESULT, status: { kind: "rejectedNoFf" } };
    const push = vi.fn(async () => RESULT).mockResolvedValueOnce(rejected);
    const flow = pushFlow(ui(), api({ push }));

    await flow.push();
    expect(titles()).toEqual(["Push rejected"]);

    await flow.push();
    expect(titles()).toEqual(["Pushed 2 commits to origin/main"]);
  });

  test("define_remote_is_done_by_whoever_is_injected_and_info_is_reread", async () => {
    const backend = api();
    const defineRemote = vi.fn(async () => true);
    const dialogs = ui();

    await pushFlow({ ...dialogs, defineRemote }, backend).push();
    const fresh = await dialogs.asked[0].defineRemote?.();

    expect(defineRemote).toHaveBeenCalled();
    expect(backend.info).toHaveBeenCalledTimes(2);
    expect(fresh).toEqual(INFO);
  });

  test("cancelled_define_remote_does_not_reread", async () => {
    const backend = api();
    const dialogs = ui();

    await pushFlow({ ...dialogs, defineRemote: async () => false }, backend).push();

    expect(await dialogs.asked[0].defineRemote?.()).toBeNull();
    expect(backend.info).toHaveBeenCalledTimes(1);
  });

  test("without_a_remote_definer_the_dialog_does_not_offer_it", async () => {
    const dialogs = ui();

    await pushFlow(dialogs, api()).push();

    expect(dialogs.asked[0].defineRemote).toBeUndefined();
  });
});
