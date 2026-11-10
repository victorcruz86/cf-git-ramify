import { derived, writable } from "svelte/store";
import {
  branchCurrent,
  branchList,
  branchPullInto,
  fetchAll,
  fetchRemote,
  gitPull,
  pullInfo,
  pullOptionsGet,
  pullOptionsSet,
  settingsGet,
  settingsSave,
  updateAbort,
  updateBranch as startUpdateBranch,
  updateProject as startUpdateProject,
  repoState,
  updateRestore,
  type Aborted,
  type ApiError,
  type FetchResult,
  type KeptChanges,
  type Operation,
  type PullInfo,
  type PullOption,
  type RepoInfo,
  type RepoState,
  type RestoreOutcome,
  type SavedChanges,
  type Settings,
  type UpdateMethod,
  type UpdateOptions,
  type UpdateResult,
} from "../lib/api";
import { restoredWithoutIndex } from "../branches/actions";
import { viewSavedChanges } from "../shelf/flow";
import { requestTab } from "../lib/tabs";
import type { PublishedRequest, Saving } from "../branches/merging";
import { ABORTED } from "../operation/operation";
import { errorText } from "../lib/errors";
import { dismiss, notify, type Notification } from "../lib/notifications";
import { taskResult } from "../lib/tasks";

export const UPDATE_ACTIONS = ["Vcs.UpdateProject", "UpdateFiles", "Git.Fetch", "Git.Pull"] as const;
export type UpdateAction = (typeof UPDATE_ACTIONS)[number];

// Decision 5: Fetch is disabled while any fetch of the app runs, whether it is an explicit Fetch or the
// one inside an Update or a Pull. A count, because they can overlap; the menu and the branches popup
// read it.
const runs = writable(0);
export const fetching = derived(runs, (count) => count > 0);

// Cleared in a `finally`: a failed or cancelled fetch gives Fetch back as well.
export async function whileFetching<T>(work: () => Promise<T>): Promise<T> {
  runs.update((count) => count + 1);
  try {
    return await work();
  } finally {
    runs.update((count) => count - 1);
  }
}

const UPDATE_CANCELED = "Update canceled";
const PULL_FAILED = "Git Pull failed";
const PARTIALLY_UPDATED = "Project Partially Updated";
// GitBundle (G:n is its line), with `<br/>` as line breaks.
const NOT_RESTORED = "Local changes were not restored"; // G:364
export const VIEW_SAVED_CHANGES = "View saved changes…"; // G:365, G:399
// `restore.notification.failed.shelf.message` and `.stash.message` (G:362, G:363).
const updateIncomplete = (place: string) =>
  `Before update your uncommitted changes were saved to ${place}.\nUpdate is not complete, you have unresolved merges in your working tree\nResolve conflicts, complete update and restore changes manually.`;
// Where a saver keeps the changes, as G:362 and G:363 name it.
export const placeOf = (saved: SavedChanges | null) => (saved?.kind === "shelve" ? "shelf" : "stash");
const RESTORE_CONFLICTS =
  "Uncommitted changes that were saved before Update have conflicts with files from Remote";
const RESTORE_BLOCKED =
  "These files changed on disk while your changes were saved. The version on disk must be committed or moved first.";
const AFTER_ABORT = "The update was aborted.";
// Decision 7: Ramify's own, as 241 only logs the failure of `GitSubmoduleUpdater`.
const SUBMODULE_UPDATE_FAILED = "Submodule update failed";

export type UpdateRequest =
  | { kind: "updateProject"; method: UpdateMethod; hidden: boolean }
  | { kind: "fixTracked"; branch: string; remoteBranches: string[]; method: UpdateMethod }
  | { kind: "rebaseMerges" }
  // `true` is `Rebase Anyway`.
  | PublishedRequest
  | {
      kind: "pull";
      info: PullInfo;
      options: PullOption[];
      // The fetch of the chosen remote, and the branches that exist afterwards. `null`: it failed.
      reload: (remote: string) => Promise<PullInfo | null>;
    };

export interface UpdateChoice {
  method: UpdateMethod;
  hide: boolean;
}

export interface PullChoice {
  remote: string;
  branch: string;
  options: PullOption[];
}

export interface TrackedChoice {
  target: string;
  setUpstream: boolean;
  method: UpdateMethod;
}

export interface UpdateUi {
  // `null` if cancelled.
  ask<T>(request: UpdateRequest): Promise<T | null>;
  // Opens the `Update Info` tab with that range. Without it `View Commits` does not appear.
  viewCommits?: (range: [string, string]) => void;
  // Abort with IntelliJ's confirmation and the HEAD that was seen (`src/operation`). `false`: it was
  // not aborted. Without it, `Abort` goes straight to `update_abort`.
  abortOperation?: (operation: Operation) => Promise<boolean>;
  // `View saved changes…` of a stash: the Stash tab, or the Unstash dialog without it. Without it,
  // the Stash tab is asked for.
  showStashes?: () => void;
}

export interface UpdateApi {
  settings(): Promise<Settings>;
  saveSettings(settings: Settings): Promise<unknown>;
  update(method: UpdateMethod, options: UpdateOptions): Promise<UpdateResult>;
  updateBranch(local: string): Promise<unknown>;
  fetch(): Promise<FetchResult>;
  // The deleted remote branches.
  fetchRemote(remote: string): Promise<string[]>;
  pullInfo(): Promise<PullInfo>;
  // The options of the last Pull of this repository.
  pullOptions(): Promise<PullOption[]>;
  savePullOptions(options: PullOption[]): Promise<unknown>;
  pull(remote: string, branch: string, options: PullOption[]): Promise<UpdateResult>;
  // `Pull into 'y' Using Merge` or `Using Rebase`: Update towards that remote branch.
  pullInto(remoteBranch: string, method: UpdateMethod): Promise<UpdateResult>;
  abort(): Promise<Aborted>;
  state(): Promise<RepoState>;
  restore(saved: SavedChanges): Promise<RestoreOutcome>;
  remoteBranches(): Promise<string[]>;
  currentBranch(): Promise<string | null>;
}

const result = async <T>(id: Promise<number>) => taskResult<T>(await id);

const backend: UpdateApi = {
  settings: settingsGet,
  saveSettings: settingsSave,
  update: (method, options) => result(startUpdateProject(method, options)),
  updateBranch: (local) => result(startUpdateBranch(local)),
  fetch: () => result(fetchAll()),
  fetchRemote: (remote) => result(fetchRemote(remote)),
  pullInfo,
  pullOptions: pullOptionsGet,
  savePullOptions: pullOptionsSet,
  pull: (remote, branch, options) => result(gitPull(remote, branch, options)),
  pullInto: (remoteBranch, method) => result(branchPullInto(remoteBranch, method)),
  abort: () => result(updateAbort()),
  state: repoState,
  restore: (saved) => result(updateRestore(saved)),
  remoteBranches: async () => (await branchList()).remote,
  currentBranch: branchCurrent,
};

// `{0,choice,1#{0} file|2#{0} files}`: zero is also singular.
export const count = (amount: number, noun: string) =>
  amount >= 2 ? `${amount} ${noun}s` : `${amount} ${noun}`;

export const updatedTitle = (files: number, commits: number) =>
  `${count(files, "file")} updated in ${count(commits, "commit")}`;

// `getBodyForUpdateNotification`: only with a path filter.
export const matchingBody = (matching: number | null) =>
  matching === null
    ? undefined
    : matching === 0
      ? "No commits matching filters"
      : `${count(matching, "commit")} matching filters`;

const NOT_SAVED_ANY_MORE: (RestoreOutcome["kind"] | undefined)[] = [
  "restored",
  "restoredWithoutIndex",
  "gone",
];

function stillSaved({ saved, restore }: Unfinished): boolean {
  return saved !== null && !NOT_SAVED_ANY_MORE.includes(restore?.kind);
}

// What an operation other than Update saved: `restore.conflict.dialog.description.label.text`.
const conflictsWith = ({ operation, from }: Saving) =>
  `Uncommitted changes that were saved before ${operation} have conflicts with files from ${from}`;

function restoreProblem(restore: RestoreOutcome | null, place: string, saving?: Saving): string {
  if (restore?.kind === "failed") return restore.reason;
  if (restore?.kind === "conflicts") return saving ? conflictsWith(saving) : RESTORE_CONFLICTS;
  if (restore?.kind === "restoreBlocked") {
    return `${restore.reason.files.join("\n")}\n${RESTORE_BLOCKED}`;
  }
  return updateIncomplete(place);
}

// Ramify's own last line: which entry or lists hold them.
const notRestoredBody = ({ saved, restore, saving }: Unfinished) =>
  `${restoreProblem(restore, placeOf(saved), saving)}\nThey are in the ${placeOf(saved)}: ${saved?.message}`;

// What is needed to go on with what was saved, whether it comes from an update or from opening the
// repository.
type Unfinished = Pick<UpdateResult, "saved" | "restore" | "inProgress"> & {
  // What is known to be half-done, if known: otherwise, it is asked when aborting.
  operation?: Operation;
  // Another operation (a rebase, a Smart Merge) saved it, not Update.
  saving?: Saving;
  // A record of an earlier build, found on opening: it keeps `Restore Changes` (decision 10).
  pending?: boolean;
};

export function updateFlow(ui: UpdateUi, api: UpdateApi = backend) {
  let running = false;
  // The ones that act on the open repository: in another one, their buttons would do something
  // else.
  const cards = new Set<number>();
  const card = (notification: Notification) => cards.add(notify(notification));
  // Goes up when the repository changes. What finishes afterwards belonged to the previous one, and
  // `Abort` and company would act on the open one: its card says where the changes are, without
  // buttons.
  let generation = 0;
  const showStashes = () => (ui.showStashes ? ui.showStashes() : requestTab("stash"));

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

  // `GitChangesSaver.notifyLocalChangesAreNotRestored`: the changes stay saved and `View saved
  // changes…` shows them (decision 10). A record of an earlier build keeps `Restore Changes`, and
  // after each try that does not bring it back the notification comes out again.
  function notifyNotRestored(update: Unfinished) {
    const saved = update.saved as SavedChanges;
    const actions: Notification["actions"] = update.pending
      ? [{ label: "Restore Changes", run: () => restoreChanges(update) }]
      : [{ label: VIEW_SAVED_CHANGES, run: () => viewSavedChanges(saved, showStashes) }];
    if (update.inProgress) actions.push({ label: "Abort", run: () => abort(update) });
    card({
      kind: "warning",
      sticky: true,
      title: NOT_RESTORED,
      body: notRestoredBody(update),
      actions,
    });
  }

  function notifyConflicts(update: Unfinished) {
    card({
      kind: "warning",
      sticky: true,
      title: "Cannot complete update",
      body: "There are pending unresolved conflicts.",
      actions: [{ label: "Abort", run: () => abort(update) }],
    });
  }

  function notifyUnfinished(update: Unfinished) {
    if (stillSaved(update)) notifyNotRestored(update);
    else if (update.inProgress) notifyConflicts(update);
  }

  async function restoreChanges(update: Unfinished) {
    const started = generation;
    const saved = update.saved as SavedChanges;
    let restore: RestoreOutcome;
    try {
      restore = await api.restore(saved);
    } catch (error) {
      restore = { kind: "failed", reason: errorText(error) };
    }
    if (restore.kind === "restored") notify({ kind: "info", title: "Local changes were restored" });
    else if (restore.kind === "restoredWithoutIndex") {
      notify(restoredWithoutIndex(saved.message, null));
    } else if (restore.kind !== "gone" && generation === started) {
      notifyNotRestored({ ...update, restore });
    }
  }

  // What another operation saved and already tried to bring back: what did not come back whole is
  // announced as here.
  function showRestored(kept: KeptChanges[], saving: Saving) {
    for (const { saved, restore } of kept) {
      if (restore?.kind === "restoredWithoutIndex") notify(restoredWithoutIndex(saved.message, null));
      const update: Unfinished = { saved, restore, inProgress: false, saving };
      if (restore && stillSaved(update)) notifyNotRestored(update);
    }
  }

  // Bring back now what another operation saved: after resolving a Smart Merge, or when a stopped
  // rebase finishes. From the last saved to the first; after one that does not come back it does not go on.
  async function restoreSaved(saved: SavedChanges[], saving: Saving) {
    const started = generation;
    let stopped: string | null = null;
    const kept: KeptChanges[] = [];
    for (const entry of [...saved].reverse()) {
      let restore: RestoreOutcome;
      if (stopped !== null) restore = { kind: "failed", reason: stopped };
      else {
        try {
          restore = await api.restore(entry);
        } catch (error) {
          restore = { kind: "failed", reason: errorText(error) };
        }
        if (!NOT_SAVED_ANY_MORE.includes(restore.kind)) {
          stopped = restoreProblem(restore, placeOf(entry), saving);
        }
      }
      kept.unshift({ saved: entry, restore });
    }
    if (generation === started) showRestored(kept, saving);
  }

  // Merge or rebase, the only things an Update leaves behind, with its confirmation. `null`: it is
  // aborted directly (a `pull --squash`, or without the `src/operation` flow).
  async function confirmable(update: Unfinished): Promise<Operation | null> {
    let operation = update.operation ?? null;
    if (!operation) {
      try {
        operation = (await api.state()).operation;
      } catch {
        return null;
      }
    }
    return operation === "merging" || operation === "rebasing" ? operation : null;
  }

  // Decision 6: after aborting nothing is restored by itself, and the button stays available.
  async function abort(update: Unfinished) {
    const started = generation;
    // Without the operations flow nothing is awaited before `update_abort`.
    const operation = ui.abortOperation ? await confirmable(update) : null;
    if (operation && ui.abortOperation) {
      const done = await ui.abortOperation(operation);
      if (generation !== started) return;
      if (done) afterAbort(update);
      else notifyUnfinished(update);
      return;
    }
    let aborted: Aborted;
    try {
      aborted = await api.abort();
    } catch (error) {
      notify({ kind: "error", title: "Abort failed", body: errorText(error) });
      if (generation === started) notifyUnfinished(update);
      return;
    }
    const title = ABORTED[aborted];
    if (title) notify({ kind: "info", title });
    if (generation !== started) return;
    afterAbort(update);
  }

  function afterAbort(update: Unfinished) {
    const after: Unfinished = {
      ...update,
      inProgress: false,
      restore: { kind: "failed", reason: AFTER_ABORT },
    };
    if (stillSaved(update)) notifyNotRestored(after);
  }

  function viewCommitsAction({ range }: UpdateResult): Notification["actions"] {
    const { viewCommits } = ui;
    return viewCommits && range
      ? [{ label: "View Commits", run: () => viewCommits(range) }]
      : undefined;
  }

  function notifySummary(update: UpdateResult) {
    const { files, commits, matching } = update;
    if (commits > 0) {
      notify({
        kind: "info",
        title: updatedTitle(files, commits),
        body: matchingBody(matching),
        actions: viewCommitsAction(update),
      });
    } else if (files > 0) {
      // A `--no-commit` pull receives no commits, but leaves changed files.
      notify({ kind: "info", title: `${count(files, "file")} updated` });
    } else notify({ kind: "info", title: "All files are up to date" });
  }

  // `GitUpdateSession` with an unfavorable result: what did arrive is counted, as a warning.
  function notifyPartial(update: UpdateResult) {
    notify({
      kind: "warning",
      title: PARTIALLY_UPDATED,
      body: updatedTitle(update.files, update.commits),
      actions: viewCommitsAction(update),
    });
  }

  const notifyCanceled = () => notify({ kind: "warning", title: UPDATE_CANCELED });

  // What did not go well, and what was left saved.
  function showProblems(update: UpdateResult) {
    const { outcome, title, saved, restore } = update;
    if (outcome.kind === "noTrackedBranch") {
      const { branch } = outcome.reason;
      card({
        kind: "error",
        title: title ?? "Cannot update",
        body: `${branch} has no tracked branch`,
        actions: [{ label: "Choose upstream branch", run: () => chooseUpstream(branch) }],
      });
    } else if (outcome.kind === "notReady" || outcome.kind === "error") {
      notify({ kind: "error", title: title ?? "Cannot update", body: outcome.reason });
    }
    if (update.submoduleError) {
      notify({ kind: "error", title: SUBMODULE_UPDATE_FAILED, body: update.submoduleError });
    }
    if (saved && restore?.kind === "restoredWithoutIndex") {
      notify(restoredWithoutIndex(saved.message, null));
    }
    notifyUnfinished(update);
  }

  // `AbstractCommonUpdateAction.onSuccess`: a result that did not go well ends in `Update canceled`.
  function show(update: UpdateResult) {
    const { outcome } = update;
    showProblems(update);
    if (outcome.kind === "success" || outcome.kind === "nothingToUpdate") notifySummary(update);
    else if (outcome.kind === "conflicts") return;
    else if (update.commits > 0) notifyPartial(update);
    else notifyCanceled();
  }

  // `GitMergeAction.handleResult`: a failure is just its error, without `Update canceled`.
  function showPull(update: UpdateResult) {
    const { outcome, title } = update;
    if (outcome.kind === "notReady" || outcome.kind === "error") {
      notify({ kind: "error", title: title ?? PULL_FAILED, body: outcome.reason });
    }
    notifyUnfinished(update);
    if (outcome.kind === "success") notifySummary(update);
  }

  // Without buttons: they would act on the open repository. Nothing offers the saved changes on
  // reopening (decision 10), so the card says where they are, in the words of G:362 and G:398.
  function notifyElsewhere(update: UpdateResult) {
    const { saved } = update;
    notify({
      kind: stillSaved(update) ? "warning" : "info",
      title: "The update of the previous repository finished",
      body:
        stillSaved(update) && saved
          ? `Your uncommitted changes were saved to ${placeOf(saved)}: ${saved.message}`
          : undefined,
    });
  }

  async function execute(method: UpdateMethod, options: UpdateOptions): Promise<void> {
    const started = generation;
    const update = await attempt("Update failed", () =>
      whileFetching(() => api.update(method, options)),
    );
    if (!update) return;
    await answered(update, options, started);
  }

  // What Rust stopped to ask before saving anything; the rest, with its notices. The repeat goes
  // through the same checks as the first Update, without its fetch.
  async function answered(update: UpdateResult, options: UpdateOptions, started: number) {
    if (generation !== started) notifyElsewhere(update);
    else if (update.outcome.kind === "rebasingMergeCommits") {
      await chooseRebaseOrMerge(update.outcome.reason.branch, options, started);
    } else if (update.outcome.kind === "rebasingPublishedCommits") {
      await confirmPublished(update.outcome.reason.branch, options, started);
    } else show(update);
  }

  // `GitRebaseProcess.askIfShouldRebasePublishedCommit`: closing it is `Cancel`. The answer is for
  // `branch`: if another one is current by the repeat, Rust checks it and asks again.
  async function confirmPublished(branch: string, options: UpdateOptions, started: number) {
    const anyway = await ui.ask<boolean>({ kind: "publishedCommits" });
    if (generation !== started) return;
    if (anyway !== true) notifyCanceled();
    else await execute("rebase", { ...options, fetched: true, publishedConfirmed: branch });
  }

  // `GitRebaseOverMergeProblem.Decision`. `Rebase` is for `branch`, as `confirmPublished`.
  async function chooseRebaseOrMerge(branch: string, options: UpdateOptions, started: number) {
    const decision = await ui.ask<UpdateMethod>({ kind: "rebaseMerges" });
    if (generation !== started) return;
    if (!decision) notifyCanceled();
    else if (decision === "merge") await execute("merge", { ...options, fetched: true });
    else await execute("rebase", { ...options, fetched: true, rebaseMerges: branch });
  }

  // The Update of a rejected push. What was received is reported by the push notification.
  function reportForPush(update: UpdateResult, here: boolean) {
    if (here) showProblems(update);
    else notifyElsewhere(update);
  }

  // One at a time, like `Update is already running`.
  async function exclusive(work: () => Promise<void>) {
    if (running) return;
    running = true;
    try {
      await work();
    } catch (error) {
      notify({ kind: "error", title: "Update failed", body: errorText(error) });
    } finally {
      running = false;
    }
  }

  // `false`: there was another one and `work` was not launched. The push wraps its own Rust call
  // in `whileFetching`, so that Fetch is enabled again while its questions wait on the user.
  async function exclusiveForPush(work: () => Promise<void>): Promise<boolean> {
    if (running) return false;
    await exclusive(work);
    return true;
  }

  // `AbstractCommonUpdateAction`: `Don't show again` and the method are only saved on accept.
  function updateProject({ shift = false }: { shift?: boolean } = {}) {
    return exclusive(async () => {
      const settings = await api.settings();
      let method = settings.updateMethod;
      if (settings.showUpdateDialog || shift) {
        const hidden = !settings.showUpdateDialog;
        const choice = await ui.ask<UpdateChoice>({ kind: "updateProject", method, hidden });
        if (!choice) return;
        method = choice.method;
        await api.saveSettings({
          ...settings,
          updateMethod: method,
          showUpdateDialog: !choice.hide,
        });
      }
      await execute(method, {});
    });
  }

  function chooseUpstream(branch: string) {
    return exclusive(async () => {
      const { updateMethod: method } = await api.settings();
      const remoteBranches = await api.remoteBranches();
      const choice = await ui.ask<TrackedChoice>({
        kind: "fixTracked",
        branch,
        remoteBranches,
        method,
      });
      if (!choice) return;
      const { target, setUpstream } = choice;
      await execute(choice.method, { target, setUpstream });
    });
  }

  // `GitBranchActionsUtil.updateBranches`: each branch that is not the current one is fetched into
  // itself (`Update Failed` for one that fails), then the current branch goes through Update
  // Project without a dialog, and one notification names those updated (`B:921`).
  function updateBranches(names: string[]) {
    return exclusive(async () => {
      const current = await api.currentBranch();
      const updated: string[] = [];
      for (const name of names.filter((name) => name !== current)) {
        const done = await attempt("Update Failed", async () => {
          await whileFetching(() => api.updateBranch(name));
          return true;
        });
        if (done) updated.push(name);
      }
      if (current !== null && names.includes(current)) {
        const { updateMethod } = await api.settings();
        await execute(updateMethod, {});
      }
      if (updated.length === 0) return;
      const title = updated.length === 1 ? "Updated branch:" : "Updated branches:";
      notify({ kind: "info", title, body: updated.join("\n") });
    });
  }

  const updateBranch = (name: string) => updateBranches([name]);

  // The branches of a remote, after a fetch of only that remote: what `GitPullDialog` does with ⌘R.
  async function reloadBranches(remote: string): Promise<PullInfo | null> {
    const pruned = await attempt("Fetch Failed", () =>
      whileFetching(() => api.fetchRemote(remote)),
    );
    if (!pruned) return null;
    notify({ kind: "info", title: "Fetch Successful", body: pruned.join("\n") || undefined });
    return attempt("Cannot load remote branches", api.pullInfo);
  }

  // `Git.Pull`.
  function pull() {
    return exclusive(async () => {
      const info = await attempt("Cannot load remote branches", api.pullInfo);
      if (!info) return;
      // A convenience: if they cannot be read or saved, the pull goes on.
      const remembered = await api.pullOptions().catch((): PullOption[] => []);
      const request: UpdateRequest = {
        kind: "pull",
        info,
        options: remembered,
        reload: reloadBranches,
      };
      const choice = await ui.ask<PullChoice>(request);
      if (!choice) return;
      await api.savePullOptions(choice.options).catch(() => {});
      const { remote, branch, options } = choice;
      const started = generation;
      const update = await attempt(PULL_FAILED, () =>
        whileFetching(() => api.pull(remote, branch, options)),
      );
      if (!update) return;
      // With `--rebase` there is no `git pull`: it is Update, and its notices.
      if (generation !== started) notifyElsewhere(update);
      else if (options.includes("rebase")) show(update);
      else showPull(update);
    });
  }

  // `GitPullBranchAction`: Update towards the remote branch, with the Update notices.
  function pullInto(remoteBranch: string, method: UpdateMethod) {
    return exclusive(async () => {
      const started = generation;
      const update = await attempt("Update failed", () =>
        whileFetching(() => api.pullInto(remoteBranch, method)),
      );
      if (!update) return;
      await answered(update, { target: remoteBranch }, started);
    });
  }

  // `GitFetchSupportImpl.buildMessage`.
  async function fetch() {
    const fetched = await whileFetching(() => attempt("Fetch Failed", api.fetch));
    if (!fetched) return;
    const { pruned, failed } = fetched;
    const errors = failed.map(({ remote, error }) =>
      failed.length > 1 ? `${remote}: ${error}` : error,
    );
    const body = [...errors, ...pruned].join("\n") || undefined;
    if (failed.length > 0) notify({ kind: "error", title: "Fetch Failed", body });
    else notify({ kind: "info", title: "Fetch Successful", body });
  }

  // `repo_open` already left in `pending` only what is still in the stash: records of earlier builds,
  // and of the stash saver. A half-done operation with nothing saved is also offered to abort.
  function repositoryChanged({ pending, state }: RepoInfo) {
    generation += 1;
    cards.forEach(dismiss);
    cards.clear();
    // An Update only leaves a merge or a rebase; a cherry-pick or a revert belong to whoever started them.
    const { operation } = state;
    const inProgress = operation === "merging" || operation === "rebasing";
    for (const { sha, title, message } of pending) {
      notifyNotRestored({
        saved: { kind: "stash", sha, message, lists: [] },
        restore: { kind: "failed", reason: `Your changes were saved before ${title}.` },
        inProgress,
        operation: inProgress ? operation : undefined,
        pending: true,
      });
    }
    if (pending.length === 0 && inProgress) {
      notifyConflicts({ saved: null, restore: null, inProgress, operation });
    }
  }

  // `Vcs.UpdateProject`, `UpdateFiles` and the popup row: with one repository, the same.
  function run(action: UpdateAction, options: { shift?: boolean } = {}): Promise<void> {
    if (action === "Git.Fetch") return fetch();
    return action === "Git.Pull" ? pull() : updateProject(options);
  }

  return {
    run,
    updateProject,
    updateBranch,
    updateBranches,
    fetch,
    pull,
    pullInto,
    showRestored,
    restoreSaved,
    repositoryChanged,
    forPush: { exclusive: exclusiveForPush, report: reportForPush },
  };
}
