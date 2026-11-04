// Resolve the conflicts of a merge, rebase or cherry-pick: opens the Conflicts dialog and, when it
// is closed, decides what comes next. It is `GitConflictResolver.merge` (with `mergeNoProceed` for the
// notification); the texts are the ones in `GitBundle` and `VcsBundle`.
import {
  conflictsSession,
  mergeCommit,
  mergeWindowOpen,
  onMergeDone,
  type ApiError,
  type ConflictSession,
  type MergeDone,
} from "../lib/api";
import { errorText } from "../lib/errors";
import { dismiss, notify } from "../lib/notifications";
import { taskResult } from "../lib/tasks";

// `merge.unresolved.conflicts.remaining.notification.body` y `.title`.
const PENDING_BODY = "There are pending unresolved conflicts.";
const PENDING_TITLE = "Pending Unresolved conflicts";
// `conflict.resolver.unmerged.files.check.error.notification.description.text`.
const CHECK_ERROR = "Cannot check the working tree for unmerged files because of an error.";
const RESOLVE = "Resolve…";

export type ConflictsResult = "resolved" | "pending";

export interface ResolveOptions {
  // What comes next when no conflict is left after the dialog: continue the rebase, the commit
  // of the merge... It does not run if reopened from the notification, like `mergeNoProceed`.
  onresolved?: () => Promise<void>;
  // Replaces the notification when conflicts remain: the rebase already has its own.
  onpending?: () => void;
  // There was nothing to resolve at the start (`proceedIfNothingToMerge`).
  onnothing?: () => Promise<void>;
  // `Params.setErrorNotificationTitle` y `setErrorNotificationAdditionalDescription`.
  title?: string;
  additional?: string;
}

export interface ConflictsRequest {
  kind: "conflicts";
  session: ConflictSession;
}

export interface ConflictsUi {
  // Opens the dialog and waits for it to close.
  ask<T>(request: ConflictsRequest): Promise<T | null>;
}

export interface ConflictsApi {
  session(): Promise<ConflictSession>;
}

const backend: ConflictsApi = {
  session: async () => taskResult<ConflictSession>(await conflictsSession()),
};

// The commit of a merge that has just been resolved: the `onresolved` of a merge launched by the app.
export async function commitMerge(): Promise<void> {
  await taskResult<void>(await mergeCommit());
}

export function conflictsFlow(ui: ConflictsUi, api: ConflictsApi = backend) {
  const cards = new Set<number>();
  // Goes up when the repository changes: what finishes afterwards belonged to the previous one and continues nothing.
  let generation = 0;
  let running: Promise<ConflictsResult> | null = null;

  function card(notification: Parameters<typeof notify>[0], started: number) {
    if (generation === started) cards.add(notify(notification));
  }

  function checkFailed(error: unknown, options: ResolveOptions, started: number) {
    if ((error as ApiError).kind === "cancelled") return;
    const description = `${CHECK_ERROR}${options.additional ?? ""}`;
    card(
      {
        kind: "error",
        title: options.title || description,
        body: options.title ? `${description}\n${errorText(error)}` : errorText(error),
      },
      started,
    );
  }

  function notifyPending(options: ResolveOptions, again: boolean, started: number) {
    if (generation !== started) return;
    const reopen = () => {
      void begin({ ...options, onresolved: undefined }, true);
    };
    const id = notify({
      kind: "warning",
      sticky: true,
      title: again ? PENDING_TITLE : options.title || PENDING_BODY,
      body: again
        ? options.additional || undefined
        : options.title
          ? `${PENDING_BODY}${options.additional ?? ""}`
          : options.additional || undefined,
      actions: [
        {
          label: RESOLVE,
          run: () => {
            dismiss(id);
            cards.delete(id);
            reopen();
          },
        },
      ],
    });
    cards.add(id);
  }

  async function proceed(step: (() => Promise<void>) | undefined) {
    try {
      await step?.();
    } catch (error) {
      if ((error as ApiError).kind !== "cancelled") {
        notify({ kind: "error", title: errorText(error) });
      }
    }
  }

  // The phase of the dialog: read, ask and read again. What comes next (`step`) runs outside the guard,
  // because it may open another dialog: a rebase that stops again on the next commit.
  interface Phase {
    result: ConflictsResult;
    step?: () => Promise<void>;
  }

  async function dialogPhase(
    options: ResolveOptions,
    fromNotification: boolean,
    started: number,
  ): Promise<Phase> {
    let before: ConflictSession;
    try {
      before = await api.session();
    } catch (error) {
      checkFailed(error, options, started);
      return { result: "pending" };
    }
    if (generation !== started) return { result: "pending" };
    if (before.conflicts.length === 0) {
      return { result: "resolved", step: fromNotification ? undefined : options.onnothing };
    }
    await ui.ask<void>({ kind: "conflicts", session: before });
    if (generation !== started) return { result: "pending" };
    let after: ConflictSession;
    try {
      after = await api.session();
    } catch (error) {
      checkFailed(error, options, started);
      return { result: "pending" };
    }
    if (generation !== started) return { result: "pending" };
    if (after.conflicts.length === 0) {
      return { result: "resolved", step: fromNotification ? undefined : options.onresolved };
    }
    if (options.onpending) options.onpending();
    else notifyPending(options, fromNotification, started);
    return { result: "pending" };
  }

  // Only one dialog at a time: whoever asks for it while one is open waits for that same one. The
  // guard ends with the dialog, not with what comes next.
  function begin(options: ResolveOptions, fromNotification: boolean): Promise<ConflictsResult> {
    if (running) return running;
    const started = generation;
    let guard: Promise<ConflictsResult> | null = null;
    const phase = (async () => {
      try {
        return await dialogPhase(options, fromNotification, started);
      } finally {
        if (running === guard) running = null;
      }
    })();
    const current = phase.then((done) => done.result);
    guard = current;
    running = current;
    return phase.then(async ({ result, step }) => {
      if (step && generation === started) await proceed(step);
      return result;
    });
  }

  function repositoryChanged() {
    generation += 1;
    running = null;
    cards.forEach(dismiss);
    cards.clear();
  }

  return {
    resolveConflicts: (options: ResolveOptions = {}) => begin(options, false),
    repositoryChanged,
  };
}

export interface MergeApi {
  open(path: string): Promise<void>;
  onDone(handler: (done: MergeDone) => void): Promise<() => void>;
}

const mergeBackend: MergeApi = { open: mergeWindowOpen, onDone: onMergeDone };

export interface MergeFilesOptions {
  // What is no longer in conflict when its turn comes: it is not opened.
  skip?: (path: string) => boolean;
  // It is awaited: the dialog rereads the session before `skip` looks at the next one.
  ondone?: (done: MergeDone) => void | Promise<void>;
  signal?: AbortSignal;
}

// Opens the merge window for each file, one at a time: there is only one (decision 3), and
// `showMergeWindow` also opens them in series, each one modal. It ends with the last one or, if the
// window turns out to be for another file, without fighting it.
export async function mergeFiles(
  paths: string[],
  options: MergeFilesOptions = {},
  api: MergeApi = mergeBackend,
): Promise<void> {
  let waiting: { path: string; settle: (done: MergeDone | null) => void } | null = null;
  const stopListening = await api.onDone((done) => {
    const current = waiting;
    if (!current) return;
    waiting = null;
    current.settle(done.path === current.path ? done : null);
  });
  const abort = () => {
    const current = waiting;
    waiting = null;
    current?.settle(null);
  };
  options.signal?.addEventListener("abort", abort);
  try {
    for (const path of paths) {
      if (options.signal?.aborted) break;
      if (options.skip?.(path)) continue;
      const finished = new Promise<MergeDone | null>((settle) => {
        waiting = { path, settle };
      });
      try {
        await api.open(path);
      } catch (error) {
        waiting = null;
        throw error;
      }
      const done = await finished;
      if (!done) break;
      await options.ondone?.(done);
    }
  } finally {
    options.signal?.removeEventListener("abort", abort);
    stopListening();
  }
}
