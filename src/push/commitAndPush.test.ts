import { get } from "svelte/store";
import { beforeEach, expect, test, vi } from "vitest";
import type { PushInfo, PushResult, Settings } from "../lib/api";
import { dismiss, notifications } from "../lib/notifications";
import { commitAndPush, shouldPreview } from "./commitAndPush";
import { pushFlow, type PushApi, type PushUi } from "./model";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const INFO: PushInfo = {
  repository: "ramify",
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
  mergeAutoApply: false,
  mergeWhitespace: "default",
  cherryPickSuffix: true,
} as Settings;

const CHOICE = {
  remote: "origin",
  target: "feature",
  tags: null,
  noVerify: false,
  force: false,
  preview: true,
  rememberTarget: false,
};

function api(overrides: Partial<PushApi> = {}): PushApi {
  return {
    info: vi.fn(async () => INFO),
    branchOf: vi.fn(async () => "feature"),
    outgoing: vi.fn(async () => []),
    push: vi.fn(async () => RESULT),
    updateAndPush: vi.fn(async () => ({ push: RESULT, update: null, attempts: 2 })),
    tags: vi.fn(async () => null),
    saveTags: vi.fn(async () => {}),
    settings: vi.fn(async () => SETTINGS),
    saveSettings: vi.fn(async () => {}),
    rememberForceTarget: vi.fn(async () => {}),
    ...overrides,
  };
}

function ui(answers: Record<string, unknown> = { push: CHOICE }) {
  const asked: { kind: string; preview?: boolean | null }[] = [];
  return {
    asked,
    ask: async (request: { kind: string }) => {
      asked.push(request);
      return (answers[request.kind] ?? null) as never;
    },
    updates: {
      exclusive: async (work: () => Promise<void>) => {
        await work();
        return true;
      },
      report: vi.fn(),
    },
  } as PushUi & { asked: { kind: string; preview?: boolean | null }[] };
}

beforeEach(() => {
  for (const { id } of get(notifications)) dismiss(id);
});

test("should_preview_is_the_condition_of_show_or_push", () => {
  const with_ = (previewPushOnCommit: boolean, previewPushProtectedOnly: boolean) => ({
    previewPushOnCommit,
    previewPushProtectedOnly,
  });

  expect(shouldPreview(with_(true, false), false, true)).toBe(true);
  expect(shouldPreview(with_(true, true), false, true)).toBe(false);
  expect(shouldPreview(with_(true, true), true, true)).toBe(true);
  expect(shouldPreview(with_(false, false), true, true)).toBe(false);
  expect(shouldPreview(with_(false, false), false, false)).toBe(true);
});

test("commit_and_push_opens_dialog_by_default", async () => {
  const backend = api();
  const dialogs = ui();

  await pushFlow(dialogs, backend).prepareCommitPush()();

  expect(dialogs.asked[0]).toMatchObject({ kind: "push", preview: true });
  expect(backend.push).toHaveBeenCalledTimes(1);
});

test("commit_and_push_without_dialog_if_only_protected", async () => {
  const backend = api({
    settings: vi.fn(async () => ({ ...SETTINGS, previewPushProtectedOnly: true })),
    tags: vi.fn(async () => "all" as const),
  });
  const dialogs = ui();

  await pushFlow(dialogs, backend).prepareCommitPush()();

  expect(dialogs.asked).toEqual([]);
  expect(backend.push).toHaveBeenCalledWith({
    source: "feature",
    remote: "origin",
    target: "feature",
    tags: "all",
    force: null,
    noVerify: false,
  });
});

test("with_only_protected_a_protected_branch_does_open_the_dialog", async () => {
  const info = { ...INFO, target: { remote: "origin", branch: "main", isNew: false } };
  const backend = api({
    info: vi.fn(async () => info),
    settings: vi.fn(async () => ({ ...SETTINGS, previewPushProtectedOnly: true })),
  });
  const dialogs = ui();

  await pushFlow(dialogs, backend).prepareCommitPush()();

  expect(dialogs.asked[0]).toMatchObject({ kind: "push" });
});

test("without_a_remote_it_cannot_push_and_opens_the_dialog", async () => {
  const info = { ...INFO, target: { remote: null, branch: "feature", isNew: false } };
  const backend = api({
    info: vi.fn(async () => info),
    settings: vi.fn(async () => ({ ...SETTINGS, previewPushOnCommit: false })),
  });
  const dialogs = ui({});

  await pushFlow(dialogs, backend).prepareCommitPush()();

  expect(dialogs.asked[0]).toMatchObject({ kind: "push" });
  expect(backend.push).not.toHaveBeenCalled();
});

test("unchecking_the_preview_saves_only_protected", async () => {
  const backend = api();

  await pushFlow(ui({ push: { ...CHOICE, preview: false } }), backend).prepareCommitPush()();

  expect(backend.saveSettings).toHaveBeenCalledWith({
    ...SETTINGS,
    previewPushProtectedOnly: true,
  });
  expect(backend.push).toHaveBeenCalled();
});

test("a_normal_push_does_not_touch_the_preview", async () => {
  const backend = api();

  await pushFlow(ui({ push: { ...CHOICE, preview: false } }), backend).push();

  expect(backend.saveSettings).not.toHaveBeenCalled();
});

test("failed_commit_does_not_push", async () => {
  const push = vi.fn(async () => {});

  await commitAndPush(async () => false, push);

  expect(push).not.toHaveBeenCalled();
});

test("throwing_commit_does_not_push", async () => {
  const push = vi.fn(async () => {});

  await expect(
    commitAndPush(async () => {
      throw new Error("no");
    }, push),
  ).rejects.toThrow("no");

  expect(push).not.toHaveBeenCalled();
});

test("if_the_view_is_gone_it_does_not_push", async () => {
  const push = vi.fn(async () => {});

  await commitAndPush(
    async () => true,
    push,
    () => false,
  );

  expect(push).not.toHaveBeenCalled();
});

test("successful_commit_pushes_once", async () => {
  const push = vi.fn(async () => {});

  await commitAndPush(async () => true, push);

  expect(push).toHaveBeenCalledTimes(1);
});

test("commit_and_push_with_amend_warns_of_rejection", async () => {
  const rejected: PushResult = {
    ...RESULT,
    status: { kind: "rejectedNoFf" },
    updatable: { branch: "feature", merges: false },
  };
  const backend = api({
    push: vi.fn(async () => rejected),
    settings: vi.fn(async () => ({ ...SETTINGS, previewPushOnCommit: false })),
  });
  const dialogs = ui({});

  await commitAndPush(async () => true, pushFlow(dialogs, backend).prepareCommitPush());

  expect(dialogs.asked).toMatchObject([
    { kind: "pushRejected", branch: "feature", method: "merge", merges: false },
  ]);
  expect(backend.push).toHaveBeenCalledTimes(1);
  expect(backend.push).toHaveBeenCalledWith(expect.objectContaining({ force: null }));
  expect(backend.updateAndPush).not.toHaveBeenCalled();
});

const silent = { ...SETTINGS, previewPushOnCommit: false };

function deferred<T>() {
  let resolve: (value: T) => void = () => {};
  const promise = new Promise<T>((settle) => (resolve = settle));
  return { promise, resolve };
}

test("switching_repository_during_the_commit_does_not_push", async () => {
  const backend = api({ settings: vi.fn(async () => silent) });
  const flow = pushFlow(ui(), backend);

  const push = flow.prepareCommitPush();
  flow.repositoryChanged();
  await push();

  expect(backend.info).not.toHaveBeenCalled();
  expect(backend.push).not.toHaveBeenCalled();
});

test.each(["info", "settings", "tags"] as const)(
  "switching_repository_while_reading_%s_does_not_push",
  async (slow) => {
    const pending = deferred<never>();
    const backend = api({ settings: vi.fn(async () => silent) });
    const original = backend[slow] as () => Promise<unknown>;
    backend[slow] = vi.fn(() => pending.promise.then(original)) as never;
    const flow = pushFlow(ui(), backend);

    const pushing = flow.prepareCommitPush()();
    await vi.waitFor(() => expect(backend[slow]).toHaveBeenCalled());
    flow.repositoryChanged();
    pending.resolve(undefined as never);
    await pushing;

    expect(backend.push).not.toHaveBeenCalled();
  },
);

test("switching_repository_with_the_dialog_open_neither_saves_nor_pushes", async () => {
  const answer = deferred<unknown>();
  const dialogs = ui();
  dialogs.ask = (async () => answer.promise) as never;
  const backend = api();
  const flow = pushFlow(dialogs, backend);

  const pushing = flow.push();
  await vi.waitFor(() => expect(backend.tags).toHaveBeenCalled());
  await Promise.resolve();
  flow.repositoryChanged();
  answer.resolve(CHOICE);
  await pushing;

  expect(backend.saveTags).not.toHaveBeenCalled();
  expect(backend.push).not.toHaveBeenCalled();
});

test("switching_repository_while_remembering_does_not_push", async () => {
  const saving = deferred<void>();
  const backend = api({ saveTags: vi.fn(() => saving.promise) });
  const flow = pushFlow(ui(), backend);

  const pushing = flow.push();
  await vi.waitFor(() => expect(backend.saveTags).toHaveBeenCalled());
  flow.repositoryChanged();
  saving.resolve();
  await pushing;

  expect(backend.push).not.toHaveBeenCalled();
});
