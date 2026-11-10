import { get } from "svelte/store";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { onRepoChanged, type HeadRepository, type PullRequestDetails } from "../lib/api";
import { dismiss, notifications } from "../lib/notifications";
import { emptyFilters } from "../log/filters";
import { openLogTab } from "../log/tabKinds";
import {
  adText,
  checkoutLabel,
  pullRequestBranch,
  showInLogLabel,
  sourceText,
  watchCheckedOut,
  type PullBranchApi,
} from "./branch";

vi.mock("../log/tabKinds", () => ({ openLogTab: vi.fn(async () => {}) }));
vi.mock("../lib/api", async (original) => ({
  ...(await original<typeof import("../lib/api")>()),
  onRepoChanged: vi.fn(),
}));
const generation = vi.hoisted(() => ({ value: 0 }));
vi.mock("./store", () => ({ pullsGeneration: () => generation.value }));

const settle = () => new Promise((done) => setTimeout(done, 0));

function held<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve: (value: T) => void = () => {};
  const promise = new Promise<T>((done) => (resolve = done));
  return { promise, resolve };
}

const fork: HeadRepository = {
  owner: "forker",
  isFork: true,
  nameWithOwner: "forker/r",
  url: "https://github.com/forker/r",
  sshUrl: "git@github.com:forker/r.git",
};

const pullRequest = (extra: Partial<PullRequestDetails> = {}): PullRequestDetails => ({
  number: 7,
  url: "https://github.com/o/r/pull/7",
  title: "Fix",
  body: "",
  state: "OPEN",
  isDraft: false,
  baseRefName: "main",
  baseRefOid: "1".repeat(40),
  headRefName: "feature",
  headRefOid: "2".repeat(40),
  headRepository: fork,
  reviewers: [],
  ...extra,
});

function api(overrides: Partial<PullBranchApi> = {}): PullBranchApi {
  return {
    checkout: vi.fn(async () => ({ kind: "gone" as const })),
    showInLog: vi.fn(async () => ({ kind: "gone" as const })),
    checkedOut: vi.fn(async () => false),
    ...overrides,
  };
}

const shown = () => get(notifications);

// What `repo-changed` would call.
let repoChanged: (() => void) | null;
let unlistened: number;

beforeEach(() => {
  generation.value = 0;
  repoChanged = null;
  unlistened = 0;
  vi.mocked(openLogTab).mockClear();
  vi.mocked(onRepoChanged).mockImplementation(async (handler) => {
    repoChanged = () => handler({ stash: false, shelf: false });
    return () => {
      unlistened++;
      repoChanged = null;
    };
  });
});

afterEach(() => {
  for (const { id } of shown()) dismiss(id);
});

test("the_popup_texts_are_241s", () => {
  // CTB:70-72; the source is `owner:branch` for a fork or a branch named as the base (P§6.6).
  expect(sourceText(pullRequest())).toBe("forker:feature");
  expect(sourceText(pullRequest({ headRepository: { ...fork, isFork: false } }))).toBe("feature");
  expect(sourceText(pullRequest({ headRepository: { ...fork, isFork: false }, headRefName: "main" }))).toBe(
    "forker:main",
  );
  expect(sourceText(pullRequest({ headRepository: null }))).toBe("feature");
  expect(checkoutLabel("forker:feature")).toBe("Checkout 'forker:feature'…");
  expect(showInLogLabel("forker:feature")).toBe("Show 'forker:feature' in Git Log");
  expect(adText("main", "forker:feature")).toBe("main ← forker:feature");
});

test("show_in_git_log_opens_a_range_tab_from_the_heads_remote", async () => {
  const backend = api({
    showInLog: vi.fn(async () => ({ kind: "ready" as const, from: "forker/main", to: "forker/feature" })),
  });

  await pullRequestBranch(null, backend).showInLog(7);

  expect(backend.showInLog).toHaveBeenCalledWith(7);
  expect(openLogTab).toHaveBeenCalledTimes(1);
  const [kind, key, init] = vi.mocked(openLogTab).mock.calls[0];
  expect(kind).toBe("range");
  expect(key).toBe("forker/main..forker/feature");
  expect(init()).toEqual({
    name: "forker/main..forker/feature",
    filters: { ...emptyFilters(), ranges: [["forker/main", "forker/feature"]] },
  });
  expect(shown()).toEqual([]);
});

test("checkout_answers_the_questions_with_the_suggested_name", async () => {
  const answers = vi.fn(async () => {});
  const outcome = { kind: "needsName" as const, suggested: "fork/feature" };
  const backend = api({
    checkout: vi.fn(async () => ({
      kind: "ready" as const,
      branch: "forker/feature",
      suggested: "fork/feature",
      outcome,
    })),
  });

  await pullRequestBranch(answers, backend).checkout(7);

  expect(answers).toHaveBeenCalledWith("forker/feature", { suggested: "fork/feature" }, outcome);
});

test("a_failed_fetch_says_fetch_failed_with_gits_output", async () => {
  const answers = vi.fn(async () => {});
  const output = "fatal: couldn't find remote ref feature";
  const flow = pullRequestBranch(
    answers,
    api({
      checkout: vi.fn(async () => ({ kind: "fetchFailed" as const, output })),
      showInLog: vi.fn(async () => ({ kind: "fetchFailed" as const, output })),
    }),
  );

  await flow.checkout(7);
  await flow.showInLog(7);

  // GitB:1387, then git's output.
  expect(shown()).toMatchObject([
    { kind: "error", title: "Fetch Failed", body: output },
    { kind: "error", title: "Fetch Failed", body: output },
  ]);
  expect(answers).not.toHaveBeenCalled();
  expect(openLogTab).not.toHaveBeenCalled();
});

test("no_remote_says_branch_checkout_failed_with_the_urls", async () => {
  const urls = ["https://github.com/forker/r", "git@github.com:forker/r.git"];
  await pullRequestBranch(null, api({ checkout: vi.fn(async () => ({ kind: "noRemote" as const, urls })) })).checkout(
    7,
  );

  // CTB:73-74, with each URL on its own line.
  expect(shown()).toMatchObject([
    {
      kind: "error",
      title: "Branch checkout failed",
      body:
        "Cannot resolve git remote for merge request source repository\n" +
        "https://github.com/forker/r\ngit@github.com:forker/r.git",
    },
  ]);
});

test("a_deleted_fork_and_a_cancel_say_nothing", async () => {
  const answers = vi.fn(async () => {});
  await pullRequestBranch(answers, api()).checkout(7);
  await pullRequestBranch(answers, api()).showInLog(7);
  const cancelled = vi.fn(async () => Promise.reject({ kind: "cancelled", message: "cancelled" }));
  await pullRequestBranch(answers, api({ checkout: cancelled })).checkout(7);

  expect(shown()).toEqual([]);
  expect(answers).not.toHaveBeenCalled();
  expect(openLogTab).not.toHaveBeenCalled();
});

// Both take 241's title of their failure (`Branch checkout failed`, CTB:73, which `notifyRemoteError`
// gives Show in Git Log too), with the message as the body.
test("a_failure_is_shown_with_its_message", async () => {
  const failing = vi.fn(async () => Promise.reject({ kind: "failed", message: "Branch name -x is not valid" }));
  await pullRequestBranch(null, api({ checkout: failing, showInLog: failing })).checkout(7);
  await pullRequestBranch(null, api({ checkout: failing, showInLog: failing })).showInLog(7);

  expect(shown()).toMatchObject([
    { kind: "error", title: "Branch checkout failed", body: "Branch name -x is not valid" },
    { kind: "error", title: "Branch checkout failed", body: "Branch name -x is not valid" },
  ]);
});

test("an_answer_for_a_forgotten_window_is_dropped", async () => {
  const answers = vi.fn(async () => {});
  const checkout = held<never>();
  const range = held<never>();
  const flow = pullRequestBranch(
    answers,
    api({ checkout: vi.fn(() => checkout.promise), showInLog: vi.fn(() => range.promise) }),
  );

  const checking = flow.checkout(7);
  const showing = flow.showInLog(7);
  // Another repository opened meanwhile.
  generation.value++;
  checkout.resolve({ kind: "ready", branch: "forker/feature", suggested: "fork/feature", outcome: { kind: "done" } } as never);
  range.resolve({ kind: "fetchFailed", output: "fatal" } as never);
  await Promise.all([checking, showing]);

  expect(answers).not.toHaveBeenCalled();
  expect(shown()).toEqual([]);
});

test("the_checked_out_icon_follows_repo_changed", async () => {
  const seen: boolean[] = [];
  const checkedOut = vi.fn<PullBranchApi["checkedOut"]>(async () => false);
  const watch = watchCheckedOut(7, (value) => seen.push(value), api({ checkedOut }));
  await settle();
  // Nothing is asked until the details are there.
  expect(checkedOut).not.toHaveBeenCalled();

  watch.refresh();
  await settle();
  expect(seen).toEqual([false]);

  // A checkout from the terminal.
  checkedOut.mockResolvedValueOnce(true);
  repoChanged?.();
  await settle();
  expect(seen).toEqual([false, true]);
  expect(checkedOut).toHaveBeenLastCalledWith(7);

  // An older answer that arrives after a newer one is dropped.
  const slow = held<boolean>();
  checkedOut.mockReturnValueOnce(slow.promise).mockResolvedValueOnce(false);
  repoChanged?.();
  repoChanged?.();
  await settle();
  slow.resolve(true);
  await settle();
  expect(seen).toEqual([false, true, false]);

  // Nor is one for a window that was forgotten.
  const late = held<boolean>();
  checkedOut.mockReturnValueOnce(late.promise);
  repoChanged?.();
  generation.value++;
  late.resolve(true);
  await settle();
  expect(seen).toEqual([false, true, false]);

  watch.stop();
  await settle();
  expect(unlistened).toBe(1);
  checkedOut.mockResolvedValue(true);
  watch.refresh();
  await settle();
  expect(seen).toEqual([false, true, false]);
});
