// What can be done with a merge, rebase, cherry-pick or revert that is half done: abort it, continue it
// and skip a commit. The texts are the ones in `GitBundle`; the order of the actions is the one in
// `Git.Ongoing.Rebase.Actions`.
import {
  operationAbort,
  operationContinue,
  operationSkip,
  repoState,
  type Aborted,
  type ApiError,
  type Operation,
  type RebaseOutcome,
  type RepoInfo,
  type RepoState,
  type SavedChanges,
} from "../lib/api";
import type { ConflictsResult, ResolveOptions } from "../conflicts/flow";
import { errorText } from "../lib/errors";
import { dismiss, notify, type Notification } from "../lib/notifications";
import type { MenuItem } from "../lib/menu";
import { taskResult } from "../lib/tasks";

export const ELSEWHERE = "This happened in the previous repository.";
const NO_REBASE = "No rebase in progress";
const RESOLVE_ELSEWHERE = "Resolve the conflicts outside, then choose Continue.";
// `rebase.notification.saved.local.changes.part.stash.text`.
export const STASHED = "Local changes were stashed before rebase.";

// `abort.operation.*.name`, and `StringUtil.capitalizeWords(name, true)` for the titles.
const NAMES: Record<Operation, string> = {
  merging: "merge",
  rebasing: "rebase",
  cherryPicking: "cherry-pick",
  reverting: "revert",
};
const capitalised = (text: string) => text.charAt(0).toUpperCase() + text.slice(1);

// `abort.operation.succeeded` and `rebase.abort.notification.successful.message`. `nothing`: what
// was in progress was no longer that, and there is no success to announce.
export const ABORTED: Record<Aborted, string | null> = {
  merge: "Merge abort succeeded",
  rebase: "Abort rebase succeeded",
  cherryPick: "Cherry-pick abort succeeded",
  revert: "Revert abort succeeded",
  squash: "Pull abort succeeded",
  nothing: null,
};

export interface OngoingHandlers {
  abort(operation: Operation): void;
  continue(): void;
  skip(): void;
}

// The ids of the Git menu (`plugin.xml`) that apply while something is in progress.
const ABORT_IDS: Record<string, Operation> = {
  "Git.Merge.Abort": "merging",
  "Git.Rebase.Abort": "rebasing",
  "Git.CherryPick.Abort": "cherryPicking",
  "Git.Revert.Abort": "reverting",
};
const REBASE_GROUP = "Git.MainMenu.RebaseActions";
export const RESOLVE_ID = "Git.ResolveConflicts";

export type OngoingAction =
  | { do: "abort"; operation: Operation }
  | { do: "continue" }
  | { do: "skip" }
  | { do: "resolve" };

export function ongoingActionFor(id: string): OngoingAction | null {
  if (id in ABORT_IDS) return { do: "abort", operation: ABORT_IDS[id] };
  if (id === "Git.Rebase.Continue") return { do: "continue" };
  if (id === RESOLVE_ID) return { do: "resolve" };
  return id === "Git.Rebase.Skip" ? { do: "skip" } : null;
}

function ongoingIds({ operation }: RepoState): string[] {
  switch (operation) {
    case "merging":
      return ["Git.Merge.Abort"];
    case "rebasing":
      return [REBASE_GROUP, "Git.Rebase.Abort", "Git.Rebase.Continue", "Git.Rebase.Skip"];
    case "cherryPicking":
      return ["Git.CherryPick.Abort"];
    case "reverting":
      return ["Git.Revert.Abort"];
    default:
      return [];
  }
}

// `Merge…` and `Rebase…` apply with the repository in the normal state (`GitMerge.update`,
// `GitRebase.update`): neither with an operation half done nor with a detached HEAD.
const NORMAL_IDS = ["Git.Merge", "Git.Rebase"];

// What the Git menu enables: `Merge…` and `Rebase…` in the normal state, the group of each operation only
// while it is in progress, and `Resolve Conflicts…` with any conflict in the changelist
// (`GitResolveConflictsAction`).
export function gitMenuEnabled(state: RepoState, conflicts = false): string[] {
  const normal = !state.operation && state.branch !== null ? NORMAL_IDS : [];
  return [...normal, ...ongoingIds(state), ...(conflicts ? [RESOLVE_ID] : [])];
}

// `Git.Ongoing.Rebase.Actions` filtered by `GitOperationActionBase.update`: only those of the state.
// Without handlers they come out disabled, like the popup rows that have nobody to run them yet.
export function ongoingActions(state: RepoState, handlers?: OngoingHandlers): MenuItem[] {
  const { operation } = state;
  if (!operation) return [];
  const item = (label: string, run: () => void) => ({
    label,
    disabled: !handlers,
    run: handlers && run,
  });
  const abort = (label: string) => item(label, () => handlers?.abort(operation));
  if (operation === "merging") return [abort("Abort Merge")];
  if (operation === "cherryPicking") return [abort("Abort Cherry-Pick")];
  if (operation === "reverting") return [abort("Abort Revert")];
  return [
    abort("Abort Rebase"),
    item("Continue Rebase", () => handlers?.continue()),
    item("Skip Commit", () => handlers?.skip()),
  ];
}

export interface ConfirmRequest {
  kind: "confirm";
  title: string;
  message: string;
  ok: string;
}

export interface OperationUi {
  // `null` if cancelled.
  ask<T>(request: ConfirmRequest): Promise<T | null>;
  // The Conflicts dialog. Without it, `Resolve…` only reminds that the conflicts must be resolved outside.
  resolveConflicts?: (options?: ResolveOptions) => Promise<ConflictsResult>;
  // Give back what was saved before the rebase when it finishes or is aborted, with the Update notices if it
  // does not come back. Without it, it stays in the stash.
  restoreSaved?: (saved: SavedChanges[]) => Promise<void>;
}

export interface OperationApi {
  state(): Promise<RepoState>;
  abort(operation: Operation, expectedHead: string): Promise<Aborted>;
  continue(): Promise<RebaseOutcome>;
  skip(): Promise<RebaseOutcome>;
}

const result = async <T>(id: Promise<number>) => taskResult<T>(await id);

const backend: OperationApi = {
  state: repoState,
  abort: (operation, head) => result(operationAbort(operation, head)),
  continue: () => result(operationContinue()),
  skip: () => result(operationSkip()),
};

export interface StoppedHandlers {
  resolve(): void;
  resume(): void;
  abort(): void;
}

const withNote = (body: string | undefined) => (body ? `${body}\n${ELSEWHERE}` : ELSEWHERE);

// `GitRebaseProcess.notifyNotAllConflictsResolved`, `showStoppedForEditingMessage`, `showFatalError`
// and `showUntrackedFilesError`. `continuing`: the rebase was already in progress (`Continue rebase failed`).
// Without `handlers` it carries no buttons: it is the one of the previous repository. `stashed`: there are changes saved
// before the rebase (`mentionLocalChangesRemainingInStash`).
export function rebaseStoppedNotification(
  outcome: RebaseOutcome,
  continuing: boolean,
  handlers?: StoppedHandlers,
  stashed = false,
): Notification | null {
  const notification = stoppedNotification(outcome, continuing, handlers);
  if (!notification || !stashed) return notification;
  const { body } = notification;
  return { ...notification, body: body ? `${body}\n${STASHED}` : STASHED };
}

function stoppedNotification(
  outcome: RebaseOutcome,
  continuing: boolean,
  handlers?: StoppedHandlers,
): Notification | null {
  const buttons = (...list: [label: string, run: () => void][]) =>
    handlers ? list.map(([label, run]) => ({ label, run })) : undefined;
  const abort: [string, () => void] = ["Abort", () => handlers?.abort()];
  switch (outcome.kind) {
    case "conflicts":
      return {
        kind: "warning",
        sticky: true,
        title: "Rebase stopped due to conflicts",
        actions: buttons(
          ["Resolve…", () => handlers?.resolve()],
          ["Continue", () => handlers?.resume()],
          abort,
        ),
      };
    case "editing":
      return {
        kind: "info",
        sticky: true,
        title: "Rebase stopped for editing",
        actions: buttons(["Continue", () => handlers?.resume()], abort),
      };
    case "failed":
      return {
        kind: "error",
        sticky: true,
        title: continuing ? "Continue rebase failed" : "Rebase failed",
        body: outcome.reason,
        actions: buttons(["Retry", () => handlers?.resume()], abort),
      };
    case "untracked":
      return {
        kind: "error",
        sticky: true,
        title: "Untracked Files Prevent Rebase",
        body: ["Move or commit them before Rebase", ...outcome.reason.files].join("\n"),
        actions: buttons(["Retry", () => handlers?.resume()], abort),
      };
    default:
      return null;
  }
}

const confirmation = (operation: Operation, repository: string): ConfirmRequest => {
  const name = NAMES[operation];
  if (operation === "rebasing") {
    // `rebase.abort.dialog.*`: this one names the repository, the others do not (`GitUtil.mention`).
    return {
      kind: "confirm",
      title: "Abort Rebase",
      message: `Abort rebase in ${repository}?`,
      ok: "Abort",
    };
  }
  return {
    kind: "confirm",
    title: `Abort ${capitalised(name)}`,
    message: `Abort ${name}?`,
    ok: "Abort",
  };
};

const abortFailed = (operation: Operation) =>
  operation === "rebasing" ? "Abort rebase failed" : `${capitalised(NAMES[operation])} abort failed`;

export function operationFlow(ui: OperationUi, api: OperationApi = backend) {
  // Those that act on the open repository: in another one, their buttons would do something else.
  const cards = new Set<number>();
  // Goes up when the repository changes. What finishes afterwards belonged to the previous one.
  let generation = 0;
  let repository = "";
  // `GitRebaseSpec.getSaver`: what the stopped rebase saved when it started, until it finishes or is aborted.
  let saved: SavedChanges[] = [];

  function show(notification: Notification | null, started: number) {
    if (!notification) return;
    if (generation === started) cards.add(notify(notification));
    else {
      notify({ ...notification, actions: undefined, body: withNote(notification.body) });
    }
  }

  function repositoryChanged({ path }: Pick<RepoInfo, "path">) {
    generation += 1;
    saved = [];
    cards.forEach(dismiss);
    cards.clear();
    repository = path.split("/").filter(Boolean).pop() ?? path;
  }

  // A cancelled one is not a failure: whoever cancelled already knows.
  async function attempt<T>(failure: (error: ApiError) => string, work: () => Promise<T>) {
    try {
      return await work();
    } catch (error) {
      if ((error as ApiError).kind !== "cancelled") {
        notify({ kind: "error", title: failure(error as ApiError), body: errorText(error) });
      }
      return null;
    }
  }

  // `GitAbortOperationAction` and `GitAbortRebaseProcess.confirmAbort`. The HEAD is the one seen when
  // asking: if it changes while the dialog is open, the backend touches nothing. `false`: it was not
  // aborted because the question was cancelled or it failed; `true`: nothing is left to abort.
  async function abortOperation(operation: Operation): Promise<boolean> {
    const started = generation;
    const current = await attempt(() => "Abort failed", api.state);
    if (!current) return false;
    if (current.operation !== operation) {
      if (operation === "rebasing") {
        notify({ kind: "error", title: "Cannot abort rebase", body: NO_REBASE });
        // The rebase already finished outside these buttons: what was saved can come back.
        if (current.operation === null) await restoreSaved();
      }
      return true;
    }
    if (!(await ui.ask<boolean>(confirmation(operation, repository)))) return false;
    const aborted = await attempt(
      () => abortFailed(operation),
      () => api.abort(operation, current.head),
    );
    if (!aborted) return false;
    const title = ABORTED[aborted];
    if (title) show({ kind: "info", title }, started);
    // `GitAbortRebaseProcess`: after the abort, what was saved comes back.
    if (operation === "rebasing" && generation === started) await restoreSaved();
    return true;
  }

  async function restoreSaved() {
    const pending = saved;
    saved = [];
    if (pending.length > 0) await ui.restoreSaved?.(pending);
  }

  // A rebase stopped on conflicts opens the Conflicts dialog, like `GitRebaseProcess`: if everything is
  // resolved the rebase goes on, and if not the card stays with `Resolve…`. Without the dialog, the card.
  const stopped = async () => {
    if (!ui.resolveConflicts) {
      notifyRebaseStopped({ kind: "conflicts" }, true);
      return;
    }
    await resolveRebase(false);
  };

  // `RebaseConflictResolver`. `fromCard`: it comes from the `Resolve…` of the card, which is already on
  // screen (`notifyUnresolvedRemain` does nothing) and, if there was nothing to resolve, asks
  // whether to continue (`resolveConflicts`).
  async function resolveRebase(fromCard: boolean): Promise<void> {
    const started = generation;
    await ui.resolveConflicts?.({
      onresolved: () => continueRebase(),
      onpending: fromCard ? () => {} : () => notifyRebaseStopped({ kind: "conflicts" }, true),
      onnothing: async () => {
        if (fromCard) await offerToContinue(started);
        else notifyRebaseStopped({ kind: "conflicts" }, true);
      },
    });
  }

  // `rebase.notification.all.conflicts.resolved.*`.
  async function offerToContinue(started: number) {
    const yes = await ui.ask<boolean>({
      kind: "confirm",
      title: "Resolve Conflicts",
      message: "All conflicts have been resolved. Do you want to continue rebase?",
      ok: "Continue Rebase",
    });
    if (!yes || generation !== started) return;
    cards.forEach(dismiss);
    cards.clear();
    await continueRebase();
  }

  async function rebased(started: number) {
    let branch: string | null = null;
    try {
      branch = (await api.state()).branch;
    } catch {
      // It is a datum of the notice: without it, only the name is missing.
    }
    show(
      {
        kind: "info",
        title: "Rebase successful",
        body: branch ? `Rebased ${branch}` : "Rebased",
      },
      started,
    );
  }

  async function resume(
    work: () => Promise<RebaseOutcome>,
    onconflicts: () => Promise<void>,
  ): Promise<void> {
    const started = generation;
    let gone = false;
    const outcome = await attempt((error) => {
      gone = error.message === NO_REBASE;
      return gone ? "Cannot continue rebase" : "Continue rebase failed";
    }, work);
    if (!outcome) {
      // Without a rebase in progress, what was saved for it can come back now.
      if (gone && generation === started) await restoreSaved();
      return;
    }
    if (outcome.kind === "done") {
      await rebased(started);
      if (generation === started) await restoreSaved();
    } else if (outcome.kind === "conflicts") await onconflicts();
    else show(rebaseStoppedNotification(outcome, true, handlers(), saved.length > 0), started);
  }

  // `Continue` of the notification: IntelliJ also uses it for `Retry`.
  const continueRebase = (onconflicts: () => Promise<void> = stopped) =>
    resume(api.continue, onconflicts);
  const skipCommit = (onconflicts: () => Promise<void> = stopped) =>
    resume(api.skip, onconflicts);

  // `repo-changed`: if the stopped rebase finished or was aborted outside (a terminal, Devin, the Update
  // card), what was saved comes back without waiting for a button. The state is read now, not the one of the signal.
  async function repositoryRefreshed() {
    if (saved.length === 0) return;
    const started = generation;
    let current: RepoState;
    try {
      current = await api.state();
    } catch {
      return;
    }
    if (generation === started && current.operation === null) await restoreSaved();
  }

  function handlers(): StoppedHandlers {
    return {
      resolve: () => {
        if (ui.resolveConflicts) void resolveRebase(true);
        else notify({ kind: "info", title: RESOLVE_ELSEWHERE });
      },
      resume: () => void continueRebase(),
      abort: () => void abortOperation("rebasing"),
    };
  }

  // A rebase that has just stopped. `continuing`: it was already in progress before this command. `kept`: what
  // `rebase::Process` saved when it started, which comes back when the rebase finishes or is aborted.
  function notifyRebaseStopped(
    outcome: RebaseOutcome,
    continuing = false,
    kept?: SavedChanges[],
  ) {
    if (kept) saved = kept;
    const stashed = saved.length > 0;
    show(rebaseStoppedNotification(outcome, continuing, handlers(), stashed), generation);
  }

  return {
    abortOperation,
    continueRebase,
    skipCommit,
    notifyRebaseStopped,
    // A rebase that has just started and stopped on conflicts: like the one that follows `--continue`.
    resolveStoppedRebase: (kept?: SavedChanges[]) => {
      if (kept) saved = kept;
      return stopped();
    },
    repositoryChanged,
    repositoryRefreshed,
  };
}
