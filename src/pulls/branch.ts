import {
  githubPrCheckedOut,
  githubPrCheckout,
  githubPrShowInLog,
  onRepoChanged,
  type ApiError,
  type CheckedOutBranch,
  type Headed,
  type PullRequestDetails,
  type PullRequestRange,
  type RemoteCheckout,
  type RemoteChoice,
} from "../lib/api";
import { errorText } from "../lib/errors";
import { notify } from "../lib/notifications";
import { taskResult } from "../lib/tasks";
import { emptyFilters } from "../log/filters";
import { openLogTab } from "../log/tabKinds";
import { pullsGeneration } from "./store";

// The branch label of a pull request's tab and its chooser (P§6.6, P§7): GHPRBranchesViewModel,
// CodeReviewDetailsBranchComponentFactory and GitRemoteBranchesUtil. The work is the backend's
// (`src-tauri/src/hosting/pulls/checkout.rs`); the questions of the checkout are the ones of any
// remote branch, from `branches/actions.ts`, with the name the pull request suggests.

// `CTB:70`, `CTB:71`, `CTB:72`, `CTB:73`, `CTB:74` and `GitB:1387`.
export const checkoutLabel = (source: string) => `Checkout '${source}'…`;
export const showInLogLabel = (source: string) => `Show '${source}' in Git Log`;
export const adText = (target: string, source: string) => `${target} ← ${source}`;
export const BRANCH_CHECKOUT_FAILED = "Branch checkout failed";
export const NO_SOURCE_REMOTE = "Cannot resolve git remote for merge request source repository";
export const FETCH_FAILED = "Fetch Failed";

// GHPRBranchesViewModel: `owner:branch` for a fork or a branch named as the base, else the
// branch.
export function sourceText(details: PullRequestDetails): string {
  const head = details.headRepository;
  return head && (head.isFork || details.baseRefName === details.headRefName)
    ? `${head.owner}:${details.headRefName}`
    : details.headRefName;
}

// The answers of a remote checkout (`branchActions().answerRemote`): the name dialog, the local
// commits question and the overwritten files, each retried with `choice`.
export type RemoteAnswers = (
  branch: string,
  choice: Partial<RemoteChoice>,
  outcome: RemoteCheckout,
) => Promise<void>;

// What the backend does, already with the task's result.
export interface PullBranchApi {
  checkout(number: number): Promise<Headed<CheckedOutBranch>>;
  showInLog(number: number): Promise<Headed<PullRequestRange>>;
  checkedOut(number: number): Promise<boolean>;
}

const backend: PullBranchApi = {
  checkout: async (number) => taskResult(await githubPrCheckout(number)),
  showInLog: async (number) => taskResult(await githubPrShowInLog(number)),
  checkedOut: githubPrCheckedOut,
};

type Ready<T> = Extract<Headed<T>, { kind: "ready" }>;

// `Checkout '…'…` and `Show '…' in Git Log`. `answers` is null where nothing answers the
// checkout's questions. An answer that arrives after the window was forgotten (another
// repository opened) is dropped, failures included.
export function pullRequestBranch(answers: RemoteAnswers | null, api: PullBranchApi = backend) {
  // P§7.2 steps 3 and 4: what has no remote, or did not fetch, stops with its notification; a
  // deleted fork stops with none, as in 241. A cancellation is not a failure.
  async function ready<T>(work: () => Promise<Headed<T>>, failure: (error: unknown) => void) {
    const asked = pullsGeneration();
    let headed: Headed<T>;
    try {
      headed = await work();
    } catch (error) {
      if (asked === pullsGeneration() && (error as ApiError | null)?.kind !== "cancelled") failure(error);
      return null;
    }
    if (asked !== pullsGeneration()) return null;
    if (headed.kind === "noRemote") {
      notify({ kind: "error", title: BRANCH_CHECKOUT_FAILED, body: [NO_SOURCE_REMOTE, ...headed.urls].join("\n") });
    } else if (headed.kind === "fetchFailed") {
      notify({ kind: "error", title: FETCH_FAILED, body: headed.output });
    }
    return headed.kind === "ready" ? (headed as Ready<T>) : null;
  }

  async function checkout(number: number): Promise<void> {
    const done = await ready(
      () => api.checkout(number),
      (error) => notify({ kind: "error", title: BRANCH_CHECKOUT_FAILED, body: errorText(error) }),
    );
    if (done && answers) await answers(done.branch, { suggested: done.suggested }, done.outcome);
  }

  // A failure takes the title 241 gives Show in Git Log's own failure (`notifyRemoteError`,
  // `Branch checkout failed`), with the message as the body.
  async function showInLog(number: number): Promise<void> {
    const range = await ready(
      () => api.showInLog(number),
      (error) => notify({ kind: "error", title: BRANCH_CHECKOUT_FAILED, body: errorText(error) }),
    );
    if (!range) return;
    const { from, to } = range;
    const name = `${from}..${to}`;
    await openLogTab("range", name, () => ({ name, filters: { ...emptyFilters(), ranges: [[from, to]] } }));
  }

  return { checkout, showInLog };
}

// The checked-out icon (P§7.6): `refresh` asks once the details are loaded, and every
// `repo-changed` asks again. An answer for an older ask, for a window forgotten since or after
// `stop` is dropped; a failure leaves the icon as it was.
export function watchCheckedOut(
  number: number,
  show: (checkedOut: boolean) => void,
  api: PullBranchApi = backend,
): { refresh: () => void; stop: () => void } {
  let asks = 0;
  let started = false;
  let stopped = false;

  async function refresh() {
    if (stopped) return;
    started = true;
    const ask = ++asks;
    const asked = pullsGeneration();
    try {
      const checkedOut = await api.checkedOut(number);
      if (!stopped && ask === asks && asked === pullsGeneration()) show(checkedOut);
    } catch (failure) {
      console.warn("Could not tell whether the pull request is checked out:", errorText(failure));
    }
  }

  const unlisten = onRepoChanged(() => {
    if (started) void refresh();
  });

  return {
    refresh: () => void refresh(),
    stop: () => {
      stopped = true;
      void unlisten.then((stop) => stop()).catch(() => {});
    },
  };
}
