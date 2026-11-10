import {
  branchCheckout,
  branchCheckoutRemote,
  branchCreate,
  branchCurrent,
  branchDelete,
  branchDeleteRemote,
  branchRebaseOnto,
  branchRename,
  rebasePublished,
  branchReset,
  branchRestore,
  branchTrackers,
  branchUnmergedCommits,
  resetModeGet,
  resetModeSet,
  commitDetails,
  branchList,
  tagCreate,
  tagDelete,
  tagRestore,
  type ApiError,
  type CheckoutMode,
  type CheckoutOutcome,
  type CleanTree,
  type DeletedBranch,
  type DeletedRemote,
  type DeletedTag,
  type LogRef,
  type LostCommit,
  type NewBranch,
  type KeptChanges,
  type RebaseOutcome,
  type RebaseRun,
  type RemoteCheckout,
  type RemoteChoice,
  type ResetMode,
  type SavedChanges,
} from "../lib/api";
import { errorText } from "../lib/errors";
import { dismiss, notify, type Notification } from "../lib/notifications";
import { ELSEWHERE, rebaseStoppedNotification } from "../operation/operation";
import { viewSavedChanges } from "../shelf/flow";
import { requestTab } from "../lib/tabs";
import {
  confirmPublished,
  finishRebase,
  rebasedMessage,
  type PublishedRequest,
  type Saving,
} from "./merging";
import { taskResult } from "../lib/tasks";
import type { VcsAction } from "../log/contextMenu";
import { shortHash } from "../log/draw";
import { isEditing } from "../log/editing";
import type { BranchNames } from "./refName";

const DEFAULT_RESET_MODE: ResetMode = "mixed";
const FULL_HASH = /^[0-9a-f]{40}$/;
const NO_BRANCH = "<no active branch>";
const RESET_HEAD_FAILED = "Resetting HEAD…";

export interface ResetTarget {
  hash: string;
  subject: string;
  author: string;
}

export interface BranchChoice {
  name: string;
  checkout: boolean;
  overwrite: boolean;
}

export interface ResetHeadChoice {
  mode: ResetMode;
  commit: string;
}

export interface TagChoice {
  name: string;
  target: string;
  message: string;
  force: boolean;
}

export type DialogRequest =
  | {
      kind: "newBranch";
      title: string;
      initialName: string;
      names: BranchNames;
      showCheckout: boolean;
      showReset: boolean;
      okLabel: string;
    }
  | { kind: "rename"; branch: string; names: BranchNames }
  | {
      kind: "problem";
      operation: string;
      title: string;
      smart: string;
      // The saver the dialog names; the dialogs fill it with the repository's.
      saver?: CleanTree;
      // Without it forcing is not offered: Smart Merge.
      force: string | null;
      cancel: string;
      files: string[];
    }
  | { kind: "localCommits"; title: string; message: string; rebase: boolean }
  | { kind: "unmerged"; title: string; message: string; commits: LostCommit[] }
  // `confirmRemoteBranchDeletion`: one question for all the remote branches, with the local
  // branches that track any of them, except the current one.
  | { kind: "deleteRemote"; branches: string[]; tracking: string[] }
  | { kind: "reset"; source: string; target: ResetTarget; mode: ResetMode }
  | { kind: "resetHead"; branch: string }
  | { kind: "tag"; hash: string | null }
  | { kind: "checkoutRef" }
  | PublishedRequest;

export interface BranchUi {
  // `null` if cancelled.
  ask<T>(request: DialogRequest): Promise<T | null>;
  // A rebase that stopped, in the card of the `src/operation` flow, which knows how to continue and
  // abort it. `saved`: what `rebase::Process` saved and is still in the stash.
  rebaseStopped?: (outcome: RebaseOutcome, saved: SavedChanges[]) => void;
  // A rebase stopped on conflicts: the Conflicts dialog and, if they are resolved, continue.
  rebaseConflicts?: (saved: SavedChanges[]) => Promise<void>;
  // What was tried to bring back and did not come back whole: the Update notices.
  restored?: (kept: KeptChanges[], saving: Saving) => void;
  // The Conflicts dialog, for the `Resolve conflicts…` of the unstash notice.
  resolveConflicts?: () => Promise<unknown>;
  // `View saved changes…` of a stash: the Stash tab, or the Unstash dialog without it. Without it,
  // the Stash tab is asked for.
  showStashes?: () => void;
  // `Update` of a branch: it is the Update Project flow, which lives in `src/update`.
  updateBranch?: (name: string) => Promise<void>;
  // `updateBranches`: several at once, one notification, the current one last.
  updateBranches?: (names: string[]) => Promise<void>;
  // The push dialog, which lives in `src/push`.
  pushBranch?: (name: string) => Promise<void>;
  pushUpTo?: (hash: string) => Promise<void>;
}

// What the backend does, already with the task's result.
export interface BranchApi {
  checkout(target: string, mode: CheckoutMode): Promise<CheckoutOutcome>;
  checkoutRemote(branch: string, choice: RemoteChoice): Promise<RemoteCheckout>;
  rebaseOnto(start: string, branch: string): Promise<RebaseRun>;
  // The protected remote branch with commits that the rebase would rewrite, or `null`.
  published(upstream: string | null, branch: string | null): Promise<string | null>;
  create(branch: NewBranch): Promise<RemoteCheckout>;
  rename(old: string, name: string): Promise<unknown>;
  remove(name: string): Promise<DeletedBranch>;
  restore(deleted: DeletedBranch): Promise<unknown>;
  deleteRemote(branch: string, alsoLocal: boolean): Promise<DeletedRemote>;
  trackers(branch: string): Promise<string[]>;
  reset(target: string, mode: ResetMode, how: CheckoutMode): Promise<CheckoutOutcome>;
  tag(name: string, target: string, message: string, force: boolean): Promise<unknown>;
  deleteTag(name: string): Promise<DeletedTag>;
  restoreTag(deleted: DeletedTag): Promise<unknown>;
  unmerged(base: string, tip: string): Promise<LostCommit[]>;
  // From git, and not from the log: the log is only loaded when its tab is opened.
  branches(): Promise<BranchNames>;
  currentBranch(): Promise<string | null>;
  commit(hash: string): Promise<ResetTarget & { refs?: LogRef[] }>;
  // The last mode of this repository.
  resetMode(): Promise<ResetMode>;
  saveResetMode(mode: ResetMode): Promise<unknown>;
}

const result = async <T>(id: Promise<number>) => taskResult<T>(await id);

const backend: BranchApi = {
  checkout: (target, mode) => result(branchCheckout(target, mode)),
  checkoutRemote: (branch, choice) => result(branchCheckoutRemote(branch, choice)),
  rebaseOnto: (start, branch) => result(branchRebaseOnto(start, branch)),
  published: (upstream, branch) => result(rebasePublished(upstream, branch)),
  create: (branch) => result(branchCreate(branch)),
  rename: (old, name) => result(branchRename(old, name)),
  remove: (name) => result(branchDelete(name)),
  restore: (deleted) => result(branchRestore(deleted)),
  deleteRemote: (branch, alsoLocal) => result(branchDeleteRemote(branch, alsoLocal)),
  trackers: branchTrackers,
  reset: (target, mode, how) => result(branchReset(target, mode, how)),
  tag: (name, target, message, force) => result(tagCreate(name, target, message, force)),
  deleteTag: (name) => result(tagDelete(name)),
  restoreTag: (deleted) => result(tagRestore(deleted)),
  unmerged: branchUnmergedCommits,
  branches: async () => {
    const { local, remote } = await branchList();
    return { local: local.map(({ name }) => name), remote };
  },
  currentBranch: branchCurrent,
  commit: async (hash) => (await commitDetails([hash]))[0],
  resetMode: async () => (await resetModeGet()) ?? DEFAULT_RESET_MODE,
  saveResetMode: resetModeSet,
};

// `GitDeleteBranchOperation.notifySuccess`. It stays: loose commits can only be recovered with it.
export function deletedNotification(
  deleted: DeletedBranch,
  handlers: { restore: () => void; viewCommits: () => void },
): Notification {
  return {
    kind: "info",
    sticky: true,
    title: `Deleted Branch: ${deleted.name}`,
    body: deleted.forced ? "Unmerged commits were discarded" : undefined,
    actions: [
      { label: "Restore", run: handlers.restore },
      ...(deleted.forced ? [{ label: "View Commits", run: handlers.viewCommits }] : []),
    ],
  };
}

// `GitBranchActionsUtil.calculateNewBranchInitialName`.
const localName = ({ name, kind }: LogRef) =>
  kind === "remote" && name.includes("/") ? name.slice(name.indexOf("/") + 1) : name;

// `GitCreateNewBranchAction.suggestBranchName`.
function suggestedName(refs: LogRef[]): string {
  const branches = refs.filter(({ kind }) => kind === "local" || kind === "remote");
  const names = new Set(branches.map(localName));
  return names.size === 1 ? [...names][0] : "";
}

const shown = (revision: string) => (FULL_HASH.test(revision) ? shortHash(revision) : revision);

function problemRequest(operation: string, force: string, files: string[]): DialogRequest {
  return {
    kind: "problem",
    operation: operation.toLowerCase(),
    title: `Git ${operation} Problem`,
    smart: `Smart ${operation}`,
    force,
    cancel: `Don't ${operation}`,
    files,
  };
}

const withFailure = (failure: string | null, text: string) =>
  failure ? `${failure}\n${text}` : text;

const VIEW_SAVED_CHANGES = "View saved changes…"; // G:399
const placeOf = (saved: SavedChanges) => (saved.kind === "shelve" ? "shelf" : "stash");

// `UnstashConflictResolver.notifyUnresolvedRemain` (G:398-401): the stash entry stays, with
// `View saved changes…` and `Resolve conflicts…`.
function restoredWithConflicts(
  failure: string | null,
  view: () => void,
  resolve?: () => void,
): Notification {
  const actions = [{ label: VIEW_SAVED_CHANGES, run: view }];
  if (resolve) actions.push({ label: "Resolve conflicts…", run: resolve });
  return {
    kind: "warning",
    sticky: true,
    title: "Local changes were restored with conflicts",
    body: withFailure(
      failure,
      "Your uncommitted changes were saved to stash.\nUnstash is not complete, you have unresolved merges in your working tree\nResolve conflicts and drop the stash.",
    ),
    actions,
  };
}

// Ramify's own: an unshelve writes no conflict, so what conflicts stays in the shelf, untouched on disk.
function shelvedWithConflicts(message: string, failure: string | null, view: () => void): Notification {
  return {
    kind: "warning",
    sticky: true,
    title: "Local changes were not restored",
    body: withFailure(
      failure,
      `Some of your uncommitted changes conflict with the files now in the working tree and were left in the shelf: ${message}`,
    ),
    actions: [{ label: VIEW_SAVED_CHANGES, run: view }],
  };
}

// There is nothing to resolve, and the entry is the only thing that holds what was in the stage.
export function restoredWithoutIndex(message: string, failure: string | null): Notification {
  return {
    kind: "warning",
    sticky: true,
    title: "Local changes were restored without staged changes",
    body: withFailure(
      failure,
      `Your files are back, but the staged changes could not be applied. They are kept in the stash: ${message}`,
    ),
  };
}

function notRestored(saved: SavedChanges, reason: string, view: () => void): Notification {
  return {
    kind: "error",
    sticky: true,
    title: "Local changes were not restored",
    body: `${reason}\nThey are in the ${placeOf(saved)}: ${saved.message}`,
    actions: [{ label: VIEW_SAVED_CHANGES, run: view }],
  };
}

function gone(saved: SavedChanges, failure: string | null): Notification {
  return {
    kind: "error",
    sticky: true,
    title: "Local changes were not restored",
    body: withFailure(failure, `They are no longer in the ${placeOf(saved)}: ${saved.message}`),
  };
}

type Outcome = CheckoutOutcome | RemoteCheckout;

interface Finish {
  // `null` if nothing is announced on success.
  success: string | null;
  operation: string;
  force: string;
  retry: (how: CheckoutMode) => Promise<void>;
}

export function branchActions(ui: BranchUi, api: BranchApi = backend) {
  // The ones that act on the open repository: in another one, their buttons would do something
  // else.
  const cards = new Set<number>();
  // Goes up when the repository changes. What finishes afterwards belonged to the previous one.
  let generation = 0;

  function card(notification: Notification, started: number) {
    if (generation === started) cards.add(notify(notification));
    else {
      const { body } = notification;
      notify({ ...notification, body: withFailure(body ?? null, ELSEWHERE), actions: undefined });
    }
  }

  function repositoryChanged() {
    generation += 1;
    cards.forEach(dismiss);
    cards.clear();
  }

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

  const showStashes = () => (ui.showStashes ? ui.showStashes() : requestTab("stash"));
  const viewing = (saved: SavedChanges) => () => viewSavedChanges(saved, showStashes);

  // What is common to everything that switches branch. Saved work that does not come back is only
  // known from here.
  async function finish(outcome: Outcome, { success, operation, force, retry }: Finish) {
    if (outcome.kind === "done") {
      if (success) notify({ kind: "info", title: success });
    } else if (outcome.kind === "conflicts" && outcome.stash.kind === "shelve") {
      notify(shelvedWithConflicts(outcome.stash.message, outcome.failure, viewing(outcome.stash)));
    } else if (outcome.kind === "conflicts") {
      notify(
        restoredWithConflicts(
          outcome.failure,
          viewing(outcome.stash),
          ui.resolveConflicts ? () => void ui.resolveConflicts?.() : undefined,
        ),
      );
    } else if (outcome.kind === "restoredWithoutIndex") {
      notify(restoredWithoutIndex(outcome.stash.message, outcome.failure));
    } else if (outcome.kind === "notRestored") {
      notify(notRestored(outcome.stash, outcome.reason, viewing(outcome.stash)));
    } else if (outcome.kind === "gone") {
      notify(gone(outcome.stash, outcome.failure));
    } else if (outcome.kind === "overwritten") {
      const how = await ui.ask<CheckoutMode>(problemRequest(operation, force, outcome.files));
      if (how) await retry(how);
    }
  }

  const finishCheckout = (success: string, retry: Finish["retry"]): Finish => ({
    success,
    operation: "Checkout",
    force: "Force Checkout",
    retry,
  });

  async function checkout(target: string, mode: CheckoutMode = "plain", label = shown(target)) {
    const outcome = await attempt(`Could not checkout ${label}`, () => api.checkout(target, mode));
    if (!outcome) return;
    await finish(
      outcome,
      finishCheckout(`Checked out ${label}`, (how) => checkout(target, how, label)),
    );
  }

  async function checkoutRef() {
    const reference = await ui.ask<string>({ kind: "checkoutRef" });
    if (reference) await checkout(`${reference}^0`, "plain", reference);
  }

  // `Rebase onto Remote`, through `rebase::Process`: the question about published commits, what was
  // saved and the success notice. A stopped rebase stays as git leaves it, with `Resolve…`, `Continue`
  // and `Abort`.
  async function rebaseOnto(start: string, branch: string) {
    const started = generation;
    const ask = (request: PublishedRequest) => ui.ask<boolean>(request);
    if (!(await confirmPublished(ask, api.published, start, branch))) return;
    const run = await attempt("Rebase failed", () => api.rebaseOnto(start, branch));
    if (!run) return;
    if (generation !== started) {
      const stopped = rebaseStoppedNotification(run.outcome, false);
      if (stopped) {
        notify({ ...stopped, actions: undefined, body: withFailure(stopped.body ?? null, ELSEWHERE) });
      }
      return;
    }
    const stoppedHere = (outcome: RebaseOutcome, saved: SavedChanges[]) => {
      if (ui.rebaseStopped) ui.rebaseStopped(outcome, saved);
      else {
        const stopped = rebaseStoppedNotification(outcome, false, undefined, saved);
        if (stopped) card(stopped, started);
      }
    };
    await finishRebase(
      run,
      { rebaseStopped: stoppedHere, rebaseConflicts: ui.rebaseConflicts, restored: ui.restored },
      { operation: "rebase", from: start },
      () => {
        notify({
          kind: "info",
          title: "Rebase successful",
          body: rebasedMessage(branch, start, true),
        });
      },
      // As until now: the failure goes to the rebase card, with `Retry` and `Abort`.
      async () => true,
    );
  }

  async function askName(start: string, suggested: string): Promise<string | null> {
    const choice = await ui.ask<BranchChoice>({
      kind: "newBranch",
      title: `Checkout ${start}`,
      initialName: suggested,
      names: await api.branches(),
      showCheckout: false,
      showReset: true,
      okLabel: "Checkout",
    });
    return choice?.name ?? null;
  }

  async function checkoutRemote(branch: string, choice: Partial<RemoteChoice> = {}) {
    const chosen = { name: null, dropCommits: false, mode: "plain" as const, ...choice };
    const outcome = await attempt(`Could not checkout ${branch}`, () =>
      api.checkoutRemote(branch, chosen),
    );
    if (outcome) await answerRemote(branch, chosen, outcome);
  }

  // What a checkout of `branch` with `choice` answered, here or elsewhere (a pull request's
  // `Checkout '…'…`): the name dialog and the local commits question, each retried with the same
  // choice, or what is common to every checkout.
  async function answerRemote(branch: string, choice: Partial<RemoteChoice>, outcome: RemoteCheckout) {
    const chosen = { name: null, dropCommits: false, mode: "plain" as const, ...choice };
    if (outcome.kind === "needsName") {
      const name = await askName(branch, outcome.suggested);
      if (name) await checkoutRemote(branch, { ...chosen, name });
    } else if (outcome.kind === "localHasCommits") {
      const { local } = outcome;
      const answer = await ui.ask<"rebase" | "drop">({
        kind: "localCommits",
        title: `Checkout ${branch}`,
        message: `Local branch '${local}' has commits that do not exist in '${branch}'. Rebase '${local}' onto '${branch}', or drop local commits?`,
        rebase: true,
      });
      if (answer === "rebase") await rebaseOnto(branch, local);
      else if (answer === "drop") await checkoutRemote(branch, { ...chosen, dropCommits: true });
    } else {
      await finish(
        outcome,
        finishCheckout(`Checked out ${branch}`, (mode) =>
          checkoutRemote(branch, { ...chosen, mode }),
        ),
      );
    }
  }

  async function create(branch: NewBranch, label: string) {
    const { name } = branch;
    const outcome = await attempt(`Could not create new branch ${name}`, () => api.create(branch));
    if (!outcome) return;
    if (outcome.kind === "localHasCommits") {
      const question = branch.checkout
        ? `Rebase '${name}' onto '${label}', or drop local commits?`
        : "Do you want to drop local commits?";
      const answer = await ui.ask<"rebase" | "drop">({
        kind: "localCommits",
        title: `Checkout ${label}`,
        message: `Local branch '${name}' has commits that do not exist in '${label}'. ${question}`,
        rebase: branch.checkout,
      });
      if (answer === "rebase") await rebaseOnto(branch.start ?? "HEAD", name);
      else if (answer === "drop") await create({ ...branch, overwrite: "drop" }, label);
    } else {
      const success = branch.checkout
        ? `Checked out new branch ${name} from ${label}`
        : `Branch ${name} was created`;
      await finish(
        outcome,
        finishCheckout(success, (mode) => create({ ...branch, mode }, label)),
      );
    }
  }

  // `GitBranchPopupActions.createOrCheckoutNewBranch`. Without `start`, it starts from HEAD.
  async function newBranch(start: string | null, label: string, initialName: string) {
    const choice = await ui.ask<BranchChoice>({
      kind: "newBranch",
      title: start === null ? "Create New Branch" : `Create Branch from ${label}`,
      initialName,
      names: await api.branches(),
      showCheckout: true,
      showReset: true,
      okLabel: "Create",
    });
    if (!choice) return;
    const { name, checkout, overwrite } = choice;
    await create(
      { name, start, checkout, overwrite: overwrite ? "try" : "no", mode: "plain" },
      label,
    );
  }

  async function newBranchAt(hash: string) {
    const commit = await attempt("Could not read the commit", () => api.commit(hash));
    if (commit) await newBranch(hash, shortHash(hash), suggestedName(commit.refs ?? []));
  }

  async function rename(branch: string) {
    const name = await ui.ask<string>({ kind: "rename", branch, names: await api.branches() });
    if (!name) return;
    const done = await attempt(`Could not rename ${branch} to ${name}`, async () => {
      await api.rename(branch, name);
      return true;
    });
    if (done) notify({ kind: "info", title: `Branch ${branch} was renamed to ${name}` });
  }

  async function restore(deleted: DeletedBranch) {
    await attempt(`Could not Restore ${deleted.name}`, () => api.restore(deleted));
  }

  async function viewCommits(deleted: DeletedBranch) {
    const { name, tip, base } = deleted;
    if (!base) return;
    const started = generation;
    const commits = await attempt("Could not read the commits", () => api.unmerged(base, tip));
    if (!commits || generation !== started) return;
    const wanted = await ui.ask<boolean>({
      kind: "unmerged",
      title: "Branch Was Not Fully Merged",
      message: `The branch ${name} was not fully merged to ${base}. Below is the list of unmerged commits.`,
      commits,
    });
    if (wanted && generation === started) await restore(deleted);
  }

  const notifyDeleted = (deleted: DeletedBranch, started: number) =>
    card(
      deletedNotification(deleted, {
        restore: () => restore(deleted),
        viewCommits: () => viewCommits(deleted),
      }),
      started,
    );

  async function deleteLocal(name: string) {
    const started = generation;
    const deleted = await attempt(`Branch ${name} wasn't deleted`, () => api.remove(name));
    if (deleted) notifyDeleted(deleted, started);
  }

  // `GitDeleteRemoteBranchOperation.execute`: the trackers of every branch, one question for all of
  // them, then each deletion with its own notification (`doDeleteRemote`, `notifySuccessfulDeletion`);
  // with `Delete tracking local branches`, each takes its own trackers with it.
  async function deleteRemote(...branches: string[]) {
    const trackers: string[][] = [];
    for (const branch of branches) {
      const found = await attempt(failedRemote(branch), () => api.trackers(branch));
      if (!found) return;
      trackers.push(found);
    }
    const tracking = [...new Set(trackers.flat())];
    const answer = await ui.ask<{ alsoLocal: boolean }>({ kind: "deleteRemote", branches, tracking });
    if (!answer) return;
    for (const [index, branch] of branches.entries()) {
      await deleteOneRemote(branch, answer.alsoLocal && trackers[index].length > 0);
    }
  }

  const failedRemote = (branch: string) => `Failed to delete remote branch ${branch}`;

  async function deleteOneRemote(branch: string, alsoLocal: boolean) {
    const failure = failedRemote(branch);
    const started = generation;
    const outcome = await attempt(failure, () => api.deleteRemote(branch, alsoLocal));
    if (!outcome) return;
    const also = outcome.local.map(({ name }) => name);
    const plural = also.length === 1 ? "branch" : "branches";
    notify({
      kind: "info",
      title: `Deleted remote branch ${branch}`,
      body: also.length > 0 ? `Also deleted local ${plural}: ${also.join(", ")}` : undefined,
    });
    for (const deleted of outcome.local) {
      if (deleted.forced) notifyDeleted(deleted, started);
    }
    for (const reason of outcome.failures) {
      notify({ kind: "error", title: "Local branch wasn't deleted", body: reason });
    }
  }

  async function deleteTag(name: string) {
    const started = generation;
    const deleted = await attempt(`Tag ${name} wasn't deleted`, () => api.deleteTag(name));
    if (!deleted) return;
    card(
      {
        kind: "info",
        sticky: true,
        title: `Deleted Tag: ${name}`,
        actions: [
          {
            label: "Restore",
            run: () => attempt(`Could not Restore ${name}`, () => api.restoreTag(deleted)),
          },
        ],
      },
      started,
    );
  }

  async function runReset(hash: string, mode: ResetMode, how: CheckoutMode) {
    const outcome = await attempt("Reset failed", () => api.reset(hash, mode, how));
    if (!outcome) return;
    await finish(outcome, {
      success: "Reset successful",
      operation: "Reset",
      force: "Hard Reset",
      retry: (next) => runReset(hash, mode, next),
    });
  }

  // `GitResetAction`: `Hard` asks for nothing more, because the copy is saved first.
  async function reset(hash: string) {
    const details = await attempt("Reset failed", async () => ({
      commit: await api.commit(hash),
      branch: await api.currentBranch(),
    }));
    if (!details) return;
    // A convenience: if it cannot be read, `Mixed`.
    const remembered = await api.resetMode().catch(() => DEFAULT_RESET_MODE);
    const mode = await ui.ask<ResetMode>({
      kind: "reset",
      source: details.branch ?? "HEAD",
      target: details.commit,
      mode: remembered,
    });
    if (!mode) return;
    api.saveResetMode(mode).catch(() => {});
    await runReset(hash, mode, "plain");
  }

  async function runResetHead(commit: string, mode: ResetMode, how: CheckoutMode) {
    const outcome = await attempt(RESET_HEAD_FAILED, () => api.reset(commit, mode, how));
    if (!outcome) return;
    await finish(outcome, {
      success: null,
      operation: "Reset",
      force: "Hard Reset",
      retry: (next) => runResetHead(commit, mode, next),
    });
  }

  // `GitResetHead`: the old dialog of the Git menu, without `Keep`, without a remembered mode and silent
  // on success.
  async function resetHead() {
    const branch = await api.currentBranch().catch(() => null);
    const choice = await ui.ask<ResetHeadChoice>({ kind: "resetHead", branch: branch ?? NO_BRANCH });
    if (choice) await runResetHead(choice.commit, choice.mode, "plain");
  }

  async function tag(hash: string | null) {
    const choice = await ui.ask<TagChoice>({ kind: "tag", hash });
    if (!choice) return;
    const { name, target, message, force } = choice;
    const done = await attempt("Could not create tag", async () => {
      await api.tag(name, target, message, force);
      return true;
    });
    if (done) notify({ kind: "info", title: `Created tag ${name} successfully.` });
  }

  function runOnRef(operation: string, ref: LogRef): Promise<void> {
    const { name, kind } = ref;
    if (operation === "update") return ui.updateBranch?.(name) ?? Promise.resolve();
    if (operation === "push") return ui.pushBranch?.(name) ?? Promise.resolve();
    if (operation === "checkout") return kind === "remote" ? checkoutRemote(name) : checkout(name);
    if (operation === "newBranch") return newBranch(name, name, localName(ref));
    if (operation === "rename") return rename(name);
    if (kind === "remote") return deleteRemote(name);
    return kind === "tag" ? deleteTag(name) : deleteLocal(name);
  }

  function run(action: VcsAction): Promise<void> {
    if (action.kind === "checkout") return checkout(action.target);
    if (action.kind === "reset") return reset(action.hash);
    if (action.kind === "newBranch") return newBranchAt(action.hash);
    if (action.kind === "newTag") return tag(action.hash);
    if (action.kind === "ref") return runOnRef(action.operation, action.ref);
    if (action.kind === "deleteRemotes") return deleteRemote(...action.names);
    if (action.kind === "updateBranches") {
      return ui.updateBranches?.(action.names) ?? Promise.resolve();
    }
    // Cherry-Pick, Revert Commit and the selected changes belong to `src/log/applying.ts`.
    if (action.kind === "cherryPick" || action.kind === "revert") return Promise.resolve();
    if (action.kind === "applyPaths") return Promise.resolve();
    // Edit Commit Message, Drop, Squash, Undo Commit, Fixup… and Squash Into… belong to
    // `src/log/editing.ts`.
    if (isEditing(action)) return Promise.resolve();
    return ui.pushUpTo?.(action.hash) ?? Promise.resolve();
  }

  return { run, checkout, checkoutRef, answerRemote, newBranch, tag, resetHead, repositoryChanged };
}
