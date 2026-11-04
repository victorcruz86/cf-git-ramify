// Cherry-Pick and Revert Commit of the log: applies the commits one by one, opens Conflicts if they clash and
// reports the result. Ported from `GitApplyChangesProcess`; the texts are those of `GitBundle` and
// `DvcsBundle`.
import {
  applyCommitAfterConflicts,
  applyCommits,
  applyPaths,
  changelistsCreate,
  changelistsGet,
  changelistsMove,
  changelistsSetActive,
  commitDetails,
  effectiveStaging,
  notifyRepoChanged,
  repoState,
  repoStatus,
  settingsGet,
  type ApiError,
  type ApplyFinished,
  type ApplyKind,
  type ApplyResult,
  type ChangeList,
  type Changelists,
  type Operation,
  type PathsResult,
  type PathsSelection,
  type RepoInfo,
  type RepoState,
} from "../lib/api";
import { findList } from "../commit/changelists";
import type { ConflictsResult, ResolveOptions } from "../conflicts/flow";
import { errorText } from "../lib/errors";
import { dismiss, notify, type Notification } from "../lib/notifications";
import { taskResult } from "../lib/tasks";
import { ELSEWHERE } from "../operation/operation";
import type { AppliedCommit, VcsAction } from "./contextMenu";
import { shortHash } from "./draw";

interface Names {
  // `cherry.pick.name` and `revert.operation.name`.
  name: string;
  title: string;
  // `cherry.pick.applied` and `revert.operation.applied`.
  applied: string;
  operation: Operation;
}

const NAMES: Record<ApplyKind, Names> = {
  cherryPick: { name: "cherry-pick", title: "Cherry-pick", applied: "applied", operation: "cherryPicking" },
  revert: { name: "revert", title: "Revert", applied: "reverted", operation: "reverting" },
};

// `apply.changes.unresolved.conflicts.text`.
const UNRESOLVED = "There are unresolved conflicts in the working tree.";
const RESOLVE = "Resolve…";
// `UIUtil.HR` in a text without HTML.
const RULE = "\n\n";

const capitalised = (text: string) => text.charAt(0).toUpperCase() + text.slice(1);

export const isApplying = (action: VcsAction): action is Extract<VcsAction, { kind: ApplyKind }> =>
  action.kind === "cherryPick" || action.kind === "revert";

// The phase 1 changelist chooser (`ListChooserDialog`), requested from the branches dialog.
export interface ChangelistRequest {
  kind: "changelist";
  lists: ChangeList[];
  initial: string;
}

export interface ChosenChangelist {
  name: string;
  setActive: boolean;
}

export const isApplyingPaths = (
  action: VcsAction,
): action is Extract<VcsAction, { kind: "applyPaths" }> => action.kind === "applyPaths";

// `changes.progress.title.choice.revert.apply.changes`, `changes.dialog.message.failed.to.revert.apply.changes`
// and `changes.revert.apply.change.list.name`.
const PATHS_TEXT = {
  revert: { title: "Revert Changes", failed: "Failed to revert changes: ", list: "Revert: " },
  apply: { title: "Apply Changes", failed: "Failed to apply changes: ", list: "Apply: " },
};
// `patch.apply.success.applied.text`.
const PATCH_APPLIED = "Patch successfully applied";

export interface ApplyingUi {
  // `ChangeListChooser`, titled `Select Target Changelist`: `null` if cancelled.
  chooseChangelist(lists: ChangeList[], initial: string): Promise<ChosenChangelist | null>;
  // The Conflicts dialog (`src/conflicts/flow.ts`).
  resolveConflicts(options?: ResolveOptions): Promise<ConflictsResult>;
  // `GitAbortOperationAction.performInBackground`: `true` if there is nothing left to abort.
  abortOperation(operation: Operation): Promise<boolean>;
  // `showStagingArea`: the Commit view, which proposes the commit message.
  showStaging(): void;
}

export interface ApplyingApi {
  state(): Promise<RepoState>;
  stagingArea(): Promise<boolean>;
  apply(kind: ApplyKind, hashes: string[], expectedHead: string): Promise<ApplyResult>;
  commitAfterConflicts(kind: ApplyKind, hash: string): Promise<ApplyFinished>;
  applyPaths(selection: PathsSelection[], reverse: boolean, expectedHead: string): Promise<PathsResult>;
  // The changelists brought up to date with what is changed.
  changelists(): Promise<Changelists>;
  // The commit's subject, for the proposed name.
  subject(hash: string): Promise<string | null>;
  // The files move to the list (new if it does not exist), which becomes active if requested.
  moveToChangelist(paths: string[], name: string, setActive: boolean): Promise<void>;
}

const result = async <T>(id: Promise<number>) => taskResult<T>(await id);

// As in the Commit view: with the files that have changes, never with an empty list, which would
// forget which list each one belongs to.
export async function currentLists(): Promise<Changelists> {
  const status = await repoStatus(false);
  return changelistsGet(status.map((change) => change.path));
}

// The files move to the list (new if it does not exist), which becomes active if requested.
export async function moveToChangelist(paths: string[], name: string, setActive: boolean) {
  const target = findList(await currentLists(), name) ?? findList(await changelistsCreate(name), name);
  if (!target) return;
  await changelistsMove(paths, target.id);
  if (setActive) await changelistsSetActive(target.id);
  await notifyRepoChanged();
}

export const backend: ApplyingApi = {
  state: repoState,
  stagingArea: async () => effectiveStaging(await settingsGet()),
  apply: (kind, hashes, head) => result(applyCommits(kind, hashes, head)),
  commitAfterConflicts: (kind, hash) => result(applyCommitAfterConflicts(kind, hash)),
  applyPaths: (selection, reverse, head) => result(applyPaths(selection, reverse, head)),
  changelists: currentLists,
  subject: async (hash) => (await commitDetails([hash]))[0]?.subject ?? null,
  moveToChangelist,
};

// What goes on during a request: the commits already applied and skipped, for the notices.
interface Sequence {
  kind: ApplyKind;
  names: Names;
  started: number;
  subjects: Map<string, string>;
  applied: string[];
  skipped: string[];
  head: string;
}

export function applyingFlow(ui: ApplyingUi, api: ApplyingApi = backend) {
  // Those that act on the open repository: in another one, their buttons would do something else.
  const cards = new Set<number>();
  // Goes up when the repository changes. What finishes afterwards belonged to the previous one.
  let generation = 0;
  let root = "";
  // `VcsCherryPickManager.myIdsInProgress`.
  const inProgress = new Set<string>();

  function show(notification: Notification, started: number): number {
    if (generation === started) {
      const id = notify(notification);
      cards.add(id);
      return id;
    }
    const { body } = notification;
    return notify({
      ...notification,
      actions: undefined,
      body: body ? `${body}\n${ELSEWHERE}` : ELSEWHERE,
    });
  }

  function remove(id: number) {
    dismiss(id);
    cards.delete(id);
  }

  const details = (seq: Sequence, hash: string) =>
    `${shortHash(hash)} ${seq.subjects.get(hash) ?? ""}`;
  const list = (seq: Sequence, hashes: string[]) =>
    hashes.map((hash) => details(seq, hash)).join("\n");

  // `getSuccessfulCommitDetailsIfAny`.
  function successful(seq: Sequence): string {
    const { applied, names } = seq;
    if (applied.length === 0) return "";
    const commits = applied.length === 1 ? "commit" : "commits";
    return `${RULE}However, ${names.name} succeeded for the following ${commits}:\n${list(seq, applied)}`;
  }

  // `formSkippedDescription`.
  function skippedText(seq: Sequence, but: boolean): string {
    const hashes = seq.skipped.map(shortHash).join(", ");
    if (!but) return `All changes from ${hashes} have already been ${seq.names.applied}`;
    const verb = seq.skipped.length === 1 ? "was" : "were";
    return `${hashes} ${verb} skipped, because all changes have already been ${seq.names.applied}.`;
  }

  // `notifyResult`.
  function notifyResult(seq: Sequence) {
    const { applied, skipped, names, started } = seq;
    if (skipped.length === 0) {
      show({ kind: "info", title: `${names.title} successful`, body: list(seq, applied) }, started);
    } else if (applied.length > 0) {
      const count = applied.length === 1 ? "commit" : `${applied.length} commits`;
      show(
        {
          kind: "info",
          title: `${capitalised(names.applied)} ${count} from ${applied.length + skipped.length}`,
          body: `${list(seq, applied)}${RULE}${skippedText(seq, true)}`,
        },
        started,
      );
    } else {
      show({ kind: "warning", title: `Nothing to ${names.name}`, body: skippedText(seq, false) }, started);
    }
  }

  // `notifyError`: `hash` is the commit that failed, if known.
  function failed(seq: Sequence, hash: string | null, content: string, actions?: Notification["actions"]) {
    const first = hash ? `${details(seq, hash)}\n` : "";
    return show(
      {
        kind: "error",
        title: `${seq.names.title} failed`,
        body: `${first}${content}${successful(seq)}`,
        actions,
      },
      seq.started,
    );
  }

  function abortAction(seq: Sequence, id: () => number) {
    return {
      label: `Abort ${seq.names.title}`,
      run: () => {
        void ui.abortOperation(seq.names.operation).then((done) => {
          if (done) remove(id());
        });
      },
    };
  }

  // `notifyConflictWarning`.
  function warnConflicts(seq: Sequence, hash: string) {
    const id = show(
      {
        kind: "warning",
        title: `${seq.names.title} was performed with conflicts`,
        body: `${details(seq, hash)}\n${UNRESOLVED}${successful(seq)}`,
        actions: [
          { label: RESOLVE, run: () => void ui.resolveConflicts() },
          abortAction(seq, () => id),
        ],
      },
      seq.started,
    );
  }

  function untracked(seq: Sequence, files: string[]) {
    const { names } = seq;
    show(
      {
        kind: "error",
        title: `Untracked Files Prevent ${names.title}`,
        body: [`Move or commit them before ${names.name}`, ...files].join("\n") + successful(seq),
      },
      seq.started,
    );
  }

  const isCancelled = (error: unknown) => (error as ApiError).kind === "cancelled";

  async function stagingArea(): Promise<boolean> {
    try {
      return await api.stagingArea();
    } catch {
      return false;
    }
  }

  // What follows the resolved conflicts: the commit with the proposed message and the commits that
  // were left. With the staging area on nothing is committed (`StagingAreaCommit`): the Commit view
  // is left with the message and the sequence stops.
  async function resolved(seq: Sequence, hash: string, remaining: string[]) {
    if (generation !== seq.started) return;
    if (await stagingArea()) {
      ui.showStaging();
      return;
    }
    await commitConflicted(seq, hash, remaining);
  }

  async function commitConflicted(seq: Sequence, hash: string, remaining: string[]) {
    let finished: ApplyFinished;
    try {
      finished = await api.commitAfterConflicts(seq.kind, hash);
    } catch (error) {
      // The operation is left half done: what has to be done is retry or abort.
      if (isCancelled(error)) {
        warnConflicts(seq, hash);
        return;
      }
      const id = failed(seq, hash, errorText(error), [
        {
          label: "Retry",
          run: () => {
            remove(id);
            void commitConflicted(seq, hash, remaining);
          },
        },
        abortAction(seq, () => id),
      ]);
      return;
    }
    (finished === "committed" ? seq.applied : seq.skipped).push(hash);
    if (remaining.length === 0 || generation !== seq.started) {
      notifyResult(seq);
      return;
    }
    try {
      seq.head = (await api.state()).head;
    } catch (error) {
      failed(seq, null, errorText(error));
      return;
    }
    await proceed(seq, remaining);
  }

  // `ConflictResolver.merge()`: an empty `onpending`, because the card is this task's own
  // (`notifyUnresolvedRemain` does nothing).
  async function conflicted(seq: Sequence, hash: string, remaining: string[]) {
    let ran = false;
    const proceeding = async () => {
      ran = true;
      await resolved(seq, hash, remaining);
    };
    try {
      await ui.resolveConflicts({ onresolved: proceeding, onnothing: proceeding, onpending: () => {} });
    } catch {
      // Without knowing how the dialog ended, the operation is left half done: a notice is given.
    }
    if (!ran) warnConflicts(seq, hash);
  }

  async function proceed(seq: Sequence, hashes: string[]): Promise<void> {
    let outcome: ApplyResult;
    try {
      outcome = await api.apply(seq.kind, hashes, seq.head);
    } catch (error) {
      if (!isCancelled(error)) failed(seq, null, errorText(error));
      return;
    }
    seq.applied.push(...outcome.applied);
    seq.skipped.push(...outcome.skipped);
    const { stopped, remaining } = outcome;
    if (!stopped) {
      notifyResult(seq);
      return;
    }
    const { names } = seq;
    switch (stopped.kind) {
      case "conflicts":
        return conflicted(seq, stopped.reason.hash, remaining);
      case "localChanges":
        // `apply.changes.would.be.overwritten`.
        failed(
          seq,
          stopped.reason.hash,
          `Your local changes will be overwritten by ${names.name}.\nCommit your changes or stash them to proceed.`,
        );
        return;
      case "untracked":
        untracked(seq, stopped.reason.files);
        return;
      case "failed":
        failed(seq, stopped.reason.hash, stopped.reason.text);
        return;
    }
  }

  // `RevertCommittedStuffAbstractAction.actionPerformed`.
  async function runPaths(action: Extract<VcsAction, { kind: "applyPaths" }>): Promise<void> {
    const { reverse, selection, hashes } = action;
    if (selection.length === 0) return;
    const text = PATHS_TEXT[reverse ? "revert" : "apply"];
    const started = generation;
    const failure = (error: unknown) => {
      if (isCancelled(error)) return;
      const body = `${text.failed}${errorText(error)}`;
      show({ kind: "error", title: text.title, body }, started);
    };
    let head: string;
    let target: ChosenChangelist | null = null;
    try {
      // The HEAD is the one the user saw when choosing, before the dialog.
      head = (await api.state()).head;
      // With the staging area there are no changelists (`areChangeListsEnabled`).
      if (!(await stagingArea())) {
        const lists = await api.changelists();
        const subject = hashes.length === 1 ? await api.subject(hashes[0]) : null;
        const active = lists.lists.find((list) => list.id === lists.active) ?? lists.lists[0];
        const initial = subject ? `${text.list}${subject}` : (active?.name ?? "");
        target = await ui.chooseChangelist(lists.lists, initial);
        if (!target) return;
      }
    } catch (error) {
      failure(error);
      return;
    }
    let outcome: PathsResult;
    try {
      outcome = await api.applyPaths(selection, reverse, head);
    } catch (error) {
      failure(error);
      return;
    }
    if (outcome.kind === "failed") {
      failure(outcome.reason);
      return;
    }
    // What follows acts on the open repository, which may be another one.
    if (generation !== started) {
      show({ kind: "info", title: text.title }, started);
      return;
    }
    if (target) {
      try {
        const paths = selection.flatMap((group) => group.paths);
        await api.moveToChangelist(paths, target.name, target.setActive);
      } catch (error) {
        failure(error);
        return;
      }
    }
    if (outcome.kind === "conflicts") {
      await ui.resolveConflicts().catch(() => {});
      return;
    }
    show({ kind: "info", title: PATCH_APPLIED }, started);
  }

  async function run(action: VcsAction): Promise<void> {
    if (isApplyingPaths(action)) return runPaths(action);
    if (!isApplying(action) || action.commits.length === 0) return;
    const { kind, commits } = action;
    const hashes = commits.map(({ hash }) => hash);
    const names = NAMES[kind];
    const started = generation;
    if (kind === "cherryPick") {
      // `cherry.pick.process.is.already.started.for.commit.from.root`.
      const busy = hashes.find((hash) => inProgress.has(hash));
      if (busy) {
        notify({
          kind: "error",
          title: `Cherry-pick process is already started for commit ${shortHash(busy)} from root ${root}`,
        });
        return;
      }
      hashes.forEach((hash) => inProgress.add(hash));
    }
    try {
      let state: RepoState;
      try {
        state = await api.state();
      } catch (error) {
        if (!isCancelled(error)) {
          notify({ kind: "error", title: `${names.title} failed`, body: errorText(error) });
        }
        return;
      }
      const subjects = new Map(commits.map(({ hash, subject }: AppliedCommit) => [hash, subject]));
      await proceed(
        { kind, names, started, subjects, applied: [], skipped: [], head: state.head },
        hashes,
      );
    } finally {
      if (kind === "cherryPick") hashes.forEach((hash) => inProgress.delete(hash));
    }
  }

  function repositoryChanged({ path }: Pick<RepoInfo, "path">) {
    generation += 1;
    cards.forEach(dismiss);
    cards.clear();
    root = path.split("/").filter(Boolean).pop() ?? path;
  }

  return { run, repositoryChanged };
}
