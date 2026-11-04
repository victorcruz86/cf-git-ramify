// Merge, Rebase, Checkout and Rebase and Pull into of the branches popup, and what is common to every
// rebase that goes through `rebase::Process`. Ported from `GitMergeBranchAction`,
// `GitRebaseBranchAction`, `GitCheckoutWithRebaseAction`, `GitPullBranchAction`,
// `GitMergeOperation`, `GitCheckoutAndRebaseRemoteBranchWorkflow` and `GitRebaseProcess`; the texts
// are those of `GitBundle`.
import {
  branchList,
  branchMerge,
  branchRebase,
  branchRebaseRemote,
  rebasePublished,
  repoSettingsGet,
  repoState,
  type ApiError,
  type BranchMergeResult,
  type KeptChanges,
  type LogRef,
  type RebaseOutcome,
  type RebaseRun,
  type RemoteRebase,
  type RepoState,
  type SavedChanges,
  type UpdateMethod,
} from "../lib/api";
import { commitMerge, type ConflictsResult, type ResolveOptions } from "../conflicts/flow";
import { errorText } from "../lib/errors";
import { dismiss, notify, type Notification } from "../lib/notifications";
import { taskResult } from "../lib/tasks";
import type { RefOperation, VcsAction } from "../log/contextMenu";
import { shortHash } from "../log/draw";
import { rebaseStoppedNotification } from "../operation/operation";
import { isProtectedBranch } from "../push/force";
import type { BranchChoice, DialogRequest } from "./actions";
import type { BranchNames } from "./refName";

// `merge.operation.name` and `rebase.operation.name`, for the warnings about what does not come back.
const MERGE = "merge";
const REBASE = "rebase";
const HEAD = "HEAD";
const REBASE_SUCCESSFUL = "Rebase successful";
const CHECKOUT_AND_REBASE_FAILED = "Checkout and Rebase failed";
const UP_TO_DATE = "Already up to date";
// `branch.ui.handler.unmerged.files.error.notification`, with `merge.operation.name`.
const UNMERGED_FILES =
  "You have to resolve all merge conflicts before merge.\nAfter resolving conflicts you also probably would want to commit your files to the current branch.";

// The `RefOperation` operations that lead to this flow.
export const MERGING_OPERATIONS: RefOperation[] = [
  "merge",
  "rebase",
  "checkoutAndRebase",
  "pullMerge",
  "pullRebase",
];

export const isMerging = (action: VcsAction) =>
  action.kind === "ref" && MERGING_OPERATIONS.includes(action.operation);

// `rebase.confirmation.dialog.published.commits.*`: `true` is `Rebase Anyway`.
export interface PublishedRequest {
  kind: "publishedCommits";
}

export type MergingRequest =
  | PublishedRequest
  | Extract<DialogRequest, { kind: "problem" | "newBranch" | "localCommits" }>;

// Who saved and from where: the text of what does not come back with conflicts.
export interface Saving {
  operation: string;
  from: string;
}

// What whoever shows the result of a `rebase::Process` rebase needs.
export interface RebaseUi {
  // A stopped rebase, in the `src/operation` card. `saved`: what is still in the stash.
  rebaseStopped(outcome: RebaseOutcome, saved: SavedChanges[]): void;
  // A rebase stopped on conflicts: opens Conflicts and, if they are resolved, continues.
  rebaseConflicts?: (saved: SavedChanges[]) => Promise<void>;
  // What was already tried to bring back: what did not come back is announced as in Update.
  restored?: (kept: KeptChanges[], saving: Saving) => void;
}

export interface MergingUi extends RebaseUi {
  ask<T>(request: MergingRequest): Promise<T | null>;
  resolveConflicts(options?: ResolveOptions): Promise<ConflictsResult>;
  // Bring back what a Smart Merge saved, once its conflicts are resolved.
  restoreSaved?: (saved: SavedChanges[], saving: Saving) => Promise<void>;
  // The branch deletion of phase 2, for `Delete x`.
  deleteBranch?: (name: string) => Promise<void>;
  // Update towards that remote branch (`src/update`).
  pullInto?: (remoteBranch: string, method: UpdateMethod) => Promise<void>;
}

export interface MergingApi {
  state(): Promise<RepoState>;
  protectedBranches(): Promise<string[]>;
  merge(branch: string, smart: boolean, expectedHead: string): Promise<BranchMergeResult>;
  commitMerge(): Promise<void>;
  rebase(onto: string, branch: string | null, expectedHead: string): Promise<RebaseRun>;
  rebaseRemote(
    remoteBranch: string,
    name: string | null,
    dropCommits: boolean,
    expectedHead: string,
  ): Promise<RemoteRebase>;
  published(upstream: string | null, branch: string | null): Promise<string | null>;
  branches(): Promise<BranchNames>;
}

const result = async <T>(id: Promise<number>) => taskResult<T>(await id);

const backend: MergingApi = {
  state: repoState,
  protectedBranches: async () => (await repoSettingsGet()).protectedBranches,
  merge: (branch, smart, head) => result(branchMerge(branch, smart, head)),
  commitMerge,
  rebase: (onto, branch, head) => result(branchRebase(onto, branch, head)),
  rebaseRemote: (remoteBranch, name, dropCommits, head) =>
    result(branchRebaseRemote(remoteBranch, name, dropCommits, head)),
  published: (upstream, branch) => result(rebasePublished(upstream, branch)),
  branches: async () => {
    const { local, remote } = await branchList();
    return { local: local.map(({ name }) => name), remote };
  },
};

// `GitSuccessfulRebase.formatMessage`.
export function rebasedMessage(
  branch: string | null,
  base: string | null,
  checkout: boolean,
): string {
  const head = checkout ? "Checked out" : "Rebased";
  const rebased = checkout ? " and rebased it" : "";
  return `${head}${branch ? ` ${branch}` : ""}${rebased}${base ? ` on ${base}` : ""}`;
}

// `GitRebaseProcess.checkForRebasingPublishedCommits`: `false` if it is not rebased. If it cannot be
// known, it is asked, as when IntelliJ cannot read the commits.
export async function confirmPublished(
  ask: (request: PublishedRequest) => Promise<boolean | null>,
  published: (upstream: string | null, branch: string | null) => Promise<string | null>,
  upstream: string | null,
  branch: string | null,
): Promise<boolean> {
  const found = await Promise.resolve()
    .then(() => published(upstream, branch))
    .catch(() => "");
  if (found === null) return true;
  return (await ask({ kind: "publishedCommits" })) === true;
}

// What follows a `RebaseRun`. `done` is the success notice; `ongoing`, whether the rebase was left in
// progress, for a failure: if it never started, there is nothing to continue or abort.
export async function finishRebase(
  run: RebaseRun,
  ui: RebaseUi,
  saving: Saving,
  done: () => void | Promise<void>,
  ongoing: () => Promise<boolean>,
) {
  const { outcome, title, saved } = run;
  if (title) {
    notify({
      kind: "error",
      title,
      body: outcome.kind === "failed" ? outcome.reason : undefined,
    });
    return;
  }
  const tried = saved.filter(({ restore }) => restore !== null);
  if (tried.length > 0) ui.restored?.(tried, saving);
  const kept = saved.filter(({ restore }) => restore === null).map(({ saved }) => saved);
  if (outcome.kind === "done") return done();
  if (outcome.kind === "conflicts" && ui.rebaseConflicts) return ui.rebaseConflicts(kept);
  if (outcome.kind !== "failed" && outcome.kind !== "untracked") {
    return ui.rebaseStopped(outcome, kept);
  }
  if (await ongoing()) return ui.rebaseStopped(outcome, kept);
  const plain = rebaseStoppedNotification(outcome, false);
  if (plain) notify(plain);
}

export function mergingFlow(ui: MergingUi, api: MergingApi = backend) {
  // The ones that act on the open repository: in another one, their buttons would do something
  // else.
  const cards = new Set<number>();
  // Goes up when the repository changes. What finishes afterwards belonged to the previous one.
  let generation = 0;

  const card = (notification: Notification, started: number) => {
    if (generation === started) cards.add(notify(notification));
    else notify({ ...notification, actions: undefined });
  };

  // A cancellation is not a failure: whoever cancelled already knows.
  async function attempt<T>(failure: string, work: () => Promise<T>): Promise<T | null> {
    try {
      return await work();
    } catch (error) {
      if ((error as ApiError).kind !== "cancelled") {
        notify({ kind: "error", title: failure, body: errorText(error) });
      }
      return null;
    }
  }

  // `getCurrentBranchPresentation`: with a detached HEAD, its short hash.
  const currentOf = (state: RepoState) => state.branch ?? shortHash(state.head);

  const ongoing = async () => {
    try {
      return (await api.state()).operation === "rebasing";
    } catch {
      return true;
    }
  };

  // `GitMergeOperation.notifySuccess` with `DeleteOnMergeOption.PROPOSE`: `Delete x` uses the deletion
  // of phase 2.
  function merged(title: string, branch: string, propose: boolean, started: number) {
    const { deleteBranch } = ui;
    const actions =
      propose && deleteBranch
        ? [{ label: `Delete ${branch}`, run: () => void deleteBranch(branch) }]
        : undefined;
    card({ kind: "info", title, actions }, started);
  }

  interface Merging {
    branch: string;
    current: string;
    head: string;
    propose: boolean;
    started: number;
  }

  // `GitMergeOperation.execute`.
  async function runMerge(merging: Merging, smart: boolean): Promise<void> {
    const { branch, current, head, propose, started } = merging;
    const failure = `Could Not Merge ${branch}`;
    const answer = await attempt(failure, () => api.merge(branch, smart, head));
    if (!answer || generation !== started) return;
    const { outcome, saved, restore } = answer;
    const saving = { operation: MERGE, from: branch };
    if (saved && restore) ui.restored?.([{ saved, restore }], saving);
    const success = `Merged ${branch} to ${current}`;
    switch (outcome.kind) {
      case "merged":
        merged(success, branch, propose, started);
        break;
      case "upToDate":
        merged(UP_TO_DATE, branch, propose, started);
        break;
      case "conflicts": {
        // `GitMergeCommittingConflictResolver`: everything resolved (or nothing to resolve,
        // `proceedIfNothingToMerge`), the merge commit and the notice. What was saved is brought back
        // whatever happens, like `restoreLocalChanges` at the end of `execute`: if the commit fails, the
        // merge stays half-done and `Restore Changes` appears.
        const commit = async () => {
          try {
            await api.commitMerge();
            merged(success, branch, propose, started);
          } finally {
            if (saved) await ui.restoreSaved?.([saved], saving);
          }
        };
        await ui.resolveConflicts({
          onresolved: commit,
          onnothing: commit,
          onpending: () => {
            card(
              {
                kind: "warning",
                sticky: true,
                title: `${branch} Merged with Conflicts`,
                actions: [{ label: "Resolve…", run: () => void ui.resolveConflicts() }],
              },
              started,
            );
            if (saved) void ui.restoreSaved?.([saved], saving);
          },
        });
        break;
      }
      case "localChanges": {
        if (smart) {
          notify({ kind: "error", title: failure, body: outcome.reason.files.join("\n") });
          break;
        }
        // `GitSmartOperationDialog` without `Force`: `Don't Merge` does nothing else.
        const how = await ui.ask<string>({
          kind: "problem",
          operation: MERGE,
          title: "Git Merge Problem",
          smart: "Smart Merge",
          force: null,
          cancel: "Don't Merge",
          files: outcome.reason.files,
        });
        if (how === "smart" && generation === started) await runMerge(merging, true);
        break;
      }
      case "untracked":
        card(
          {
            kind: "error",
            title: "Untracked Files Prevent Merge",
            body: ["Move or commit them before merge", ...outcome.reason.files].join("\n"),
          },
          started,
        );
        break;
      case "unmerged":
        card(
          {
            kind: "error",
            title: "Cannot merge because of unmerged files",
            body: UNMERGED_FILES,
            actions: [{ label: "Resolve…", run: () => void ui.resolveConflicts() }],
          },
          started,
        );
        break;
      case "failed":
        notify({ kind: "error", title: failure, body: outcome.reason });
        break;
    }
  }

  // `GitMergeBranchAction.deleteOnMerge`: only a local one that is not protected.
  async function merge({ name, kind }: LogRef) {
    const started = generation;
    const state = await attempt(`Could Not Merge ${name}`, api.state);
    if (!state) return;
    const patterns = await api.protectedBranches().catch(() => ["master", "main"]);
    const propose = kind === "local" && !isProtectedBranch(patterns, name);
    await runMerge(
      { branch: name, current: currentOf(state), head: state.head, propose, started },
      false,
    );
  }

  async function notifyRebased(base: string | null, checkout: boolean, started: number) {
    let branch: string | null = null;
    try {
      branch = (await api.state()).branch;
    } catch {
      // It is a piece of the notice: without it, only the name is missing.
    }
    card({ kind: "info", title: REBASE_SUCCESSFUL, body: rebasedMessage(branch, base, checkout) }, started);
  }

  async function showRun(run: RebaseRun, base: string | null, checkout: boolean, started: number) {
    if (generation !== started) return;
    await finishRebase(
      run,
      ui,
      { operation: REBASE, from: base ?? HEAD },
      () => notifyRebased(base, checkout, started),
      ongoing,
    );
  }

  const askPublished = (upstream: string | null, branch: string | null) =>
    confirmPublished((request) => ui.ask<boolean>(request), api.published, upstream, branch);

  // `GitBranchWorker.rebase`: `git rebase x`.
  async function rebase({ name }: LogRef) {
    const started = generation;
    const state = await attempt("Rebase failed", api.state);
    if (!state || !(await askPublished(name, state.branch))) return;
    const run = await attempt("Rebase failed", () => api.rebase(name, null, state.head));
    if (run) await showRun(run, name, false, started);
  }

  async function askName(remoteBranch: string, suggested: string): Promise<string | null> {
    const choice = await ui.ask<BranchChoice>({
      kind: "newBranch",
      title: `Checkout ${remoteBranch}`,
      initialName: suggested,
      names: await api.branches(),
      showCheckout: false,
      showReset: true,
      okLabel: "Checkout",
    });
    return choice?.name ?? null;
  }

  // `GitCheckoutAndRebaseRemoteBranchWorkflow`: the name if needed, whether to drop the local's own
  // commits, and the rebase.
  async function rebaseRemote(remoteBranch: string, state: RepoState, started: number) {
    let name: string | null = null;
    let dropCommits = false;
    for (;;) {
      const answer = await attempt(CHECKOUT_AND_REBASE_FAILED, () =>
        api.rebaseRemote(remoteBranch, name, dropCommits, state.head),
      );
      if (!answer || generation !== started) return;
      if (answer.kind === "rebased") return showRun(answer.reason, state.branch, true, started);
      if (answer.kind === "needsName") {
        name = await askName(remoteBranch, answer.reason.suggested);
        if (!name) return;
      } else {
        const { local } = answer.reason;
        const choice = await ui.ask<"rebase" | "drop">({
          kind: "localCommits",
          title: `Checkout ${remoteBranch}`,
          message: `Local branch '${local}' has commits that do not exist in '${remoteBranch}'. Do you want to drop local commits?`,
          rebase: false,
        });
        if (choice !== "drop") return;
        name = local;
        dropCommits = true;
      }
    }
  }

  // `GitBrancher.rebaseOnCurrent`: `git rebase HEAD x`, which leaves `x` current and rebased.
  async function checkoutAndRebase({ name, kind }: LogRef) {
    const started = generation;
    const state = await attempt(CHECKOUT_AND_REBASE_FAILED, api.state);
    // The local of a remote comes out of it: `HEAD..origin/x` are the same commits.
    if (!state || !(await askPublished(HEAD, name))) return;
    if (kind === "remote") return rebaseRemote(name, state, started);
    const run = await attempt("Rebase failed", () => api.rebase(HEAD, name, state.head));
    if (run) await showRun(run, state.branch, true, started);
  }

  function run(action: VcsAction): Promise<void> {
    if (action.kind !== "ref") return Promise.resolve();
    const { operation, ref } = action;
    if (operation === "merge") return merge(ref);
    if (operation === "rebase") return rebase(ref);
    if (operation === "checkoutAndRebase") return checkoutAndRebase(ref);
    if (operation === "pullMerge") return ui.pullInto?.(ref.name, "merge") ?? Promise.resolve();
    if (operation === "pullRebase") return ui.pullInto?.(ref.name, "rebase") ?? Promise.resolve();
    return Promise.resolve();
  }

  function repositoryChanged() {
    generation += 1;
    cards.forEach(dismiss);
    cards.clear();
  }

  return { run, repositoryChanged };
}
