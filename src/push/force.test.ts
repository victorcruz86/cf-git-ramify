import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { get } from "svelte/store";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import type {
  PushInfo,
  PushResult,
  PushSpec,
  Settings,
  UpdatedPush,
  UpdateResult,
} from "../lib/api";
import { dismiss, notifications } from "../lib/notifications";
import { isProtectedBranch, silentTarget } from "./force";
import ForcePushDialog from "./ForcePushDialog.svelte";
import { pushFlow, type PushApi, type PushUi } from "./model";
import PushRejectedDialog from "./PushRejectedDialog.svelte";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const INFO: PushInfo = {
  repository: "rama",
  branch: "feature",
  head: "a".repeat(40),
  remotes: ["origin"],
  remoteBranches: ["origin/feature"],
  target: { remote: "origin", branch: "feature", isNew: false },
  prePushHook: false,
  protectedBranches: ["master", "main"],
  forcePushTargets: [],
};

const RESULT: PushResult = {
  status: { kind: "success" },
  pushed: 1,
  source: "feature",
  destination: "origin/feature",
  tags: [],
  remote: "origin",
  updatable: null,
};

const SETTINGS = {
  updateMethod: "merge",
  pushAutoUpdate: false,
  previewPushOnCommit: true,
  previewPushProtectedOnly: false,
} as Settings;

const UPDATED: UpdateResult = {
  outcome: { kind: "success" },
  title: null,
  files: 1,
  commits: 2,
  range: ["a", "b"],
  matching: null,
  saved: null,
  restore: null,
  inProgress: false,
};

const FORCE_CHOICE = {
  remote: "origin",
  target: "feature",
  tags: null,
  noVerify: false,
  force: true,
  preview: true,
  rememberTarget: false,
};

const rejected: PushResult = {
  ...RESULT,
  status: { kind: "rejectedNoFf" },
  updatable: { branch: "feature", merges: false },
};

const stale: PushResult = { ...RESULT, status: { kind: "rejectedStale" } };

function api(overrides: Partial<PushApi> = {}): PushApi {
  return {
    info: vi.fn(async () => INFO),
    branchOf: vi.fn(async () => "feature"),
    outgoing: vi.fn(async () => []),
    push: vi.fn(async () => RESULT),
    updateAndPush: vi.fn(
      async (): Promise<UpdatedPush> => ({ push: RESULT, update: UPDATED, attempts: 2 }),
    ),
    tags: vi.fn(async () => null),
    saveTags: vi.fn(async () => {}),
    settings: vi.fn(async () => SETTINGS),
    saveSettings: vi.fn(async () => {}),
    rememberForceTarget: vi.fn(async () => {}),
    ...overrides,
  };
}

function ui(answers: Record<string, unknown>): PushUi & { asked: { kind: string }[] } {
  const asked: { kind: string }[] = [];
  return {
    asked,
    ask: async (request: { kind: string }) => {
      asked.push(request);
      return (answers[request.kind] ?? null) as never;
    },
    updates: {
      exclusive: vi.fn(async (work: () => Promise<void>) => {
        await work();
        return true;
      }),
      report: vi.fn(),
    },
  } as PushUi & { asked: { kind: string }[] };
}

const shown = () => get(notifications);
const action = (label: string) =>
  shown()
    .flatMap(({ actions }) => actions ?? [])
    .find((item) => item.label === label);

beforeEach(() => {
  for (const { id } of shown()) dismiss(id);
});

afterEach(cleanup);

test("the_pattern_applies_in_full_to_the_name_without_refs", () => {
  expect(isProtectedBranch(["release.*"], "release-1")).toBe(true);
  expect(isProtectedBranch(["master"], "master2")).toBe(false);
  expect(isProtectedBranch(["main|dev"], "main-2")).toBe(false);
  expect(isProtectedBranch(["main"], "refs/heads/main")).toBe(true);
  expect(isProtectedBranch(["("], "(")).toBe(false);
  expect(isProtectedBranch(["(", "a"], "a")).toBe(true);
  expect(isProtectedBranch([], "main")).toBe(false);
});

test("force_push_dialog_literal", async () => {
  const onanswer = vi.fn();
  render(ForcePushDialog, { branch: "feature", onanswer });

  expect(screen.getByRole("dialog", { name: "Force Push" })).toBeTruthy();
  expect(
    screen.getByText(
      "You're going to force push to feature. It may overwrite commits at the remote. Are you sure you want to proceed?",
    ),
  ).toBeTruthy();
  expect(screen.getByRole("checkbox", { name: "Don't warn about this target" })).toBeTruthy();
  await fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(onanswer).toHaveBeenCalledWith(null);
});

test("dont_warn_remembers_the_target", async () => {
  const onanswer = vi.fn();
  render(ForcePushDialog, { branch: "feature", onanswer });

  await fireEvent.click(screen.getByRole("checkbox", { name: "Don't warn about this target" }));
  await fireEvent.click(screen.getByRole("button", { name: "Force Push" }));

  expect(onanswer).toHaveBeenCalledWith({ remember: true });

  const backend = api();
  const dialogs = ui({ push: { ...FORCE_CHOICE, rememberTarget: true } });
  await pushFlow(dialogs, backend).push();

  expect(backend.rememberForceTarget).toHaveBeenCalledWith("origin", "feature");
  expect(silentTarget("origin", "feature")).toBe("origin:feature");
});

test("cancelling_the_warning_neither_remembers_nor_pushes", async () => {
  const backend = api();

  await pushFlow(ui({ push: null }), backend).push();

  expect(backend.rememberForceTarget).not.toHaveBeenCalled();
  expect(backend.push).not.toHaveBeenCalled();
});

test("force_push_from_the_dialog_is_with_lease", async () => {
  const backend = api();

  await pushFlow(ui({ push: FORCE_CHOICE }), backend).push();

  expect(backend.push).toHaveBeenCalledWith(expect.objectContaining({ force: "lease" }));
  expect(backend.rememberForceTarget).not.toHaveBeenCalled();
});

test("normal_push_carries_no_force", async () => {
  const backend = api();

  await pushFlow(ui({ push: { ...FORCE_CHOICE, force: false } }), backend).push();

  expect(backend.push).toHaveBeenCalledWith(expect.objectContaining({ force: null }));
});

test("force_push_anyway_without_confirming", async () => {
  const push = vi.fn(async (spec: PushSpec) => (spec.force === "plain" ? RESULT : stale));
  const dialogs = ui({ push: FORCE_CHOICE });

  await pushFlow(dialogs, api({ push })).push();
  const notice = shown().find(({ title }) => title === "Push rejected");

  expect(notice?.body).toBe("Force-with-lease push feature to origin/feature was rejected");
  expect(push).toHaveBeenCalledTimes(1);

  action("Force push anyway")?.run();
  await vi.waitFor(() => expect(push).toHaveBeenCalledTimes(2));

  expect(push.mock.calls[1][0]).toEqual({ ...push.mock.calls[0][0], force: "plain" });
  expect(dialogs.asked.map(({ kind }) => kind)).toEqual(["push"]);
});

test("nothing_forces_without_the_click_on_the_notification", async () => {
  const push = vi.fn(async () => stale);

  await pushFlow(ui({ push: FORCE_CHOICE }), api({ push })).push();
  await new Promise((resolve) => setTimeout(resolve));

  expect(push).toHaveBeenCalledTimes(1);
  expect(push).not.toHaveBeenCalledWith(expect.objectContaining({ force: "plain" }));
});

test("a_normal_rejection_does_not_offer_to_force", async () => {
  const backend = api({ push: vi.fn(async () => ({ ...RESULT, status: { kind: "rejectedNoFf" as const } })) });

  await pushFlow(ui({ push: { ...FORCE_CHOICE, force: false } }), backend).push();

  expect(shown().map(({ title }) => title)).toEqual(["Push rejected"]);
  expect(action("Force push anyway")).toBeUndefined();
});

test("force_push_anyway_does_not_survive_switching_repository", async () => {
  const push = vi.fn(async () => stale);
  const flow = pushFlow(ui({ push: FORCE_CHOICE }), api({ push }));

  await flow.push();
  const anyway = action("Force push anyway");
  flow.repositoryChanged();

  expect(action("Force push anyway")).toBeUndefined();
  anyway?.run();
  await new Promise((resolve) => setTimeout(resolve));
  expect(push).toHaveBeenCalledTimes(1);
});

test("a_rejected_lease_after_switching_repository_does_not_offer_to_force", async () => {
  let finish: (result: PushResult) => void = () => {};
  const push = vi.fn(() => new Promise<PushResult>((resolve) => (finish = resolve)));
  const flow = pushFlow(ui({ push: FORCE_CHOICE }), api({ push }));

  const pushing = flow.push();
  await vi.waitFor(() => expect(push).toHaveBeenCalled());
  flow.repositoryChanged();
  finish(stale);
  await pushing;

  expect(action("Force push anyway")).toBeUndefined();
});

test("rejection_merge_rebase_cancel_literal", async () => {
  const onanswer = vi.fn();
  render(PushRejectedDialog, { branch: "master", method: "merge", merges: false, onanswer });

  expect(screen.getByRole("dialog", { name: "Push Rejected" })).toBeTruthy();
  expect(screen.getByText(/Push of the current branch "master" was rejected\./)).toBeTruthy();
  expect(screen.getByText(/Remote changes need to be merged before pushing\./)).toBeTruthy();
  expect(
    screen.getByRole("checkbox", {
      name: "Remember the update method and update silently in the future. Change this later in Settings | Version Control.",
    }),
  ).toBeTruthy();
  expect((screen.getByRole("button", { name: "Merge" }) as HTMLElement).className).toContain(
    "default",
  );
  await fireEvent.click(screen.getByRole("button", { name: "Rebase" }));
  expect(onanswer).toHaveBeenCalledWith({ method: "rebase", remember: false });
  await fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(onanswer).toHaveBeenLastCalledWith(null);
});

test("with_rebase_saved_rebase_is_the_default_button", () => {
  render(PushRejectedDialog, { branch: "main", method: "rebase", merges: false, onanswer: vi.fn() });

  expect((screen.getByRole("button", { name: "Rebase" }) as HTMLElement).className).toContain(
    "default",
  );
});

test("unpushed_merges_recommend_merge_and_rebase_anyway", async () => {
  const onanswer = vi.fn();
  render(PushRejectedDialog, { branch: "main", method: "rebase", merges: true, onanswer });

  expect(screen.getByText("merge is highly recommended").tagName).toBe("B");
  expect(screen.getByText(/Rebasing them can lead to problems\./)).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Rebase" })).toBeNull();
  expect((screen.getByRole("button", { name: "Merge" }) as HTMLElement).className).toContain(
    "default",
  );
  await fireEvent.click(screen.getByRole("button", { name: "Rebase Anyway" }));
  expect(onanswer).toHaveBeenCalledWith({ method: "rebase", remember: false });
});

test("remember_saves_method_and_autoupdate", async () => {
  const backend = api({ push: vi.fn(async () => rejected) });
  const dialogs = ui({
    push: { ...FORCE_CHOICE, force: false },
    pushRejected: { method: "rebase", remember: true },
  });

  await pushFlow(dialogs, backend).push();

  expect(backend.saveSettings).toHaveBeenCalledWith({
    ...SETTINGS,
    updateMethod: "rebase",
    pushAutoUpdate: true,
  });
  expect(backend.updateAndPush).toHaveBeenCalledWith(
    expect.objectContaining({ force: null }),
    "rebase",
    { rebaseMerges: true },
    1,
  );
});

test("with_merges_choosing_does_not_change_the_saved_method", async () => {
  const merges = { ...rejected, updatable: { branch: "feature", merges: true } };
  const backend = api({ push: vi.fn(async () => merges) });
  const dialogs = ui({
    push: { ...FORCE_CHOICE, force: false },
    pushRejected: { method: "merge", remember: false },
  });

  await pushFlow(dialogs, backend).push();

  expect(backend.saveSettings).toHaveBeenCalledWith({ ...SETTINGS, pushAutoUpdate: false });
  expect(backend.updateAndPush).toHaveBeenCalled();
});

test("cancelling_the_rejection_leaves_the_warning_and_does_not_save", async () => {
  const backend = api({ push: vi.fn(async () => rejected) });

  await pushFlow(ui({ push: { ...FORCE_CHOICE, force: false } }), backend).push();

  expect(backend.saveSettings).not.toHaveBeenCalled();
  expect(backend.updateAndPush).not.toHaveBeenCalled();
  expect(shown()[0]).toMatchObject({
    title: "Push rejected",
    body: "Push to origin/feature was rejected",
  });
});

test("with_autoupdate_it_does_not_ask_and_uses_the_saved_method", async () => {
  const backend = api({
    push: vi.fn(async () => rejected),
    settings: vi.fn(async () => ({ ...SETTINGS, pushAutoUpdate: true, updateMethod: "rebase" as const })),
  });
  const dialogs = ui({ push: { ...FORCE_CHOICE, force: false } });

  await pushFlow(dialogs, backend).push();

  expect(dialogs.asked.map(({ kind }) => kind)).toEqual(["push"]);
  expect(backend.updateAndPush).toHaveBeenCalledWith(expect.anything(), "rebase", {}, 1);
});

test("update_and_push_reports_what_was_received", async () => {
  const viewCommits = vi.fn();
  const backend = api({ push: vi.fn(async () => rejected) });
  const dialogs = ui({
    push: { ...FORCE_CHOICE, force: false },
    pushRejected: { method: "merge", remember: false },
  });

  await pushFlow({ ...dialogs, viewCommits }, backend).push();

  expect(shown()[0]).toMatchObject({
    kind: "info",
    title: "Pushed 1 commit to origin/feature",
    body: "2 commits received during the push operation",
  });
  action("View received commits")?.run();
  expect(viewCommits).toHaveBeenCalledWith(["a", "b"]);
});

test("a_conflict_stops_with_the_update_warnings", async () => {
  const conflicts: UpdateResult = { ...UPDATED, outcome: { kind: "conflicts" }, inProgress: true };
  const backend = api({
    push: vi.fn(async () => rejected),
    updateAndPush: vi.fn(async () => ({ push: rejected, update: conflicts, attempts: 1 })),
  });
  const dialogs = ui({
    push: { ...FORCE_CHOICE, force: false },
    pushRejected: { method: "merge", remember: false },
  });

  await pushFlow(dialogs, backend).push();

  expect(dialogs.updates.report).toHaveBeenCalledWith(conflicts, true);
  expect(backend.updateAndPush).toHaveBeenCalledTimes(1);
  expect(shown()[0]).toMatchObject({
    kind: "warning",
    title: "Push rejected",
    body: "Push was canceled, because not all conflicts were resolved during the update.\nResolve the conflicts and try pushing again",
  });
});

test("with_another_update_running_it_does_not_update", async () => {
  const backend = api({ push: vi.fn(async () => rejected) });
  const dialogs = ui({
    push: { ...FORCE_CHOICE, force: false },
    pushRejected: { method: "merge", remember: false },
  });
  dialogs.updates.exclusive = vi.fn(async () => false);

  await pushFlow(dialogs, backend).push();

  expect(backend.updateAndPush).not.toHaveBeenCalled();
  expect(shown()[0]).toMatchObject({ title: "Push rejected" });
});

test("rebasing_merge_commits_asks_and_the_count_continues", async () => {
  const stop: UpdateResult = { ...UPDATED, outcome: { kind: "rebasingMergeCommits" } };
  const updateAndPush = vi
    .fn(async (): Promise<UpdatedPush> => ({ push: RESULT, update: UPDATED, attempts: 4 }))
    .mockResolvedValueOnce({ push: rejected, update: stop, attempts: 3 });
  const backend = api({
    push: vi.fn(async () => rejected),
    updateAndPush,
    settings: vi.fn(async () => ({ ...SETTINGS, pushAutoUpdate: true, updateMethod: "rebase" as const })),
  });
  const dialogs = ui({ push: { ...FORCE_CHOICE, force: false }, rebaseMerges: "merge" });

  await pushFlow(dialogs, backend).push();

  expect(dialogs.asked.map(({ kind }) => kind)).toEqual(["push", "rebaseMerges"]);
  expect(updateAndPush).toHaveBeenLastCalledWith(expect.anything(), "merge", { fetched: true }, 3);
});

test("cancelling_rebasing_merge_commits_leaves_the_rejection", async () => {
  const stop: UpdateResult = { ...UPDATED, outcome: { kind: "rebasingMergeCommits" } };
  const backend = api({
    push: vi.fn(async () => rejected),
    updateAndPush: vi.fn(async () => ({ push: rejected, update: stop, attempts: 1 })),
    settings: vi.fn(async () => ({ ...SETTINGS, pushAutoUpdate: true })),
  });

  await pushFlow(ui({ push: { ...FORCE_CHOICE, force: false } }), backend).push();

  expect(backend.updateAndPush).toHaveBeenCalledTimes(1);
  expect(shown()[0]).toMatchObject({ body: "Push was rejected, and update was canceled" });
});

test("with_autoupdate_switching_repository_while_reading_the_settings_does_not_update", async () => {
  let finish: (settings: Settings) => void = () => {};
  const backend = api({
    push: vi.fn(async () => rejected),
    settings: vi.fn(() => new Promise<Settings>((resolve) => (finish = resolve))),
  });
  const flow = pushFlow(ui({ push: { ...FORCE_CHOICE, force: false } }), backend);

  const pushing = flow.push();
  await vi.waitFor(() => expect(backend.settings).toHaveBeenCalled());
  flow.repositoryChanged();
  finish({ ...SETTINGS, pushAutoUpdate: true });
  await pushing;

  expect(backend.updateAndPush).not.toHaveBeenCalled();
  expect(shown()[0]).toMatchObject({ title: "Push rejected" });
});

test("switching_repository_while_saving_the_choice_does_not_update", async () => {
  let finish: (value: unknown) => void = () => {};
  const backend = api({
    push: vi.fn(async () => rejected),
    saveSettings: vi.fn(() => new Promise((resolve) => (finish = resolve))),
  });
  const flow = pushFlow(
    ui({
      push: { ...FORCE_CHOICE, force: false },
      pushRejected: { method: "merge", remember: false },
    }),
    backend,
  );

  const pushing = flow.push();
  await vi.waitFor(() => expect(backend.saveSettings).toHaveBeenCalled());
  flow.repositoryChanged();
  finish(undefined);
  await pushing;

  expect(backend.updateAndPush).not.toHaveBeenCalled();
});

test("a_push_cancelled_after_updating_offers_what_was_received", async () => {
  const viewCommits = vi.fn();
  const backend = api({
    push: vi.fn(async () => rejected),
    updateAndPush: vi.fn(async () => ({ push: rejected, update: UPDATED, attempts: 1 })),
  });
  const dialogs = ui({
    push: { ...FORCE_CHOICE, force: false },
    pushRejected: { method: "merge", remember: false },
  });

  await pushFlow({ ...dialogs, viewCommits }, backend).push();

  expect(dialogs.updates.report).toHaveBeenCalledWith(UPDATED, true);
  action("View received commits")?.run();
  expect(viewCommits).toHaveBeenCalledWith(["a", "b"]);
});

test("what_was_saved_before_asking_about_the_merges_is_reported", async () => {
  const stop: UpdateResult = {
    ...UPDATED,
    outcome: { kind: "rebasingMergeCommits" },
    saved: { sha: "abc", message: "saved" },
    restore: { kind: "restoredWithoutIndex" },
  };
  const backend = api({
    push: vi.fn(async () => rejected),
    updateAndPush: vi.fn(async () => ({ push: rejected, update: stop, attempts: 2 })),
    settings: vi.fn(async () => ({ ...SETTINGS, pushAutoUpdate: true })),
  });
  const dialogs = ui({ push: { ...FORCE_CHOICE, force: false } });

  await pushFlow(dialogs, backend).push();

  expect(dialogs.updates.report).toHaveBeenCalledWith(stop, true);
});
