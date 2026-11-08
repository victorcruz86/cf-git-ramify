// Edit Commit Message, Drop Commits and Squash Commits of the log, with their Undo, and Undo Commit…, Fixup… and
// Squash Into…. Ported from `GitRewordAction`, `GitDropLogAction`, `GitSquashLogAction`,
// `GitCommitEditingActionBase` (`actionPerformed`), `GitNewCommitMessageActionDialog` (`validate`),
// `GitCommitEditingNotifications`, `GitUncommitAction`, `GitAutoSquashCommitAction` and
// `GitRebaseCheckinHandlerFactory`; and Interactively Rebase from Here…: `GitInteractiveRebaseAction`,
// `interactivelyRebaseUsingLog` and `startInteractiveRebase`. The texts are those of `GitBundle` and
// `VcsBundle`.
import { get } from "svelte/store";
import {
  commitDetails,
  effectiveStaging,
  repoState,
  repoStatus,
  rewriteAutosquash,
  rewriteCheck,
  rewriteCheckSingle,
  rewriteDrop,
  rewriteInteractive,
  rewriteInteractiveStart,
  rewriteMenu,
  rewriteReword,
  rewriteSquash,
  rewriteUncommit,
  rewriteUndo,
  rewriteUndoCheck,
  rewriteValidate,
  settingsGet,
  settingsSave,
  type ApiError,
  type ChangeList,
  type Changelists,
  type CommitDetails,
  type InteractiveStart,
  type RebaseCommit,
  type RebasePlan,
  type RebaseRun,
  type RepoState,
  type RewriteKind,
  type RewriteMenu,
  type Rewritten,
  type SingleAction,
  type Undone,
} from "../lib/api";
import { finishRebase, rebasedMessage, type RebaseUi, type Saving } from "../branches/merging";
import type { CheckinRequest } from "../commit/checkin";
import type { CommitRequestInput } from "../commit/dialogStore";
import { listFiles } from "../commit/tree";
import { errorText } from "../lib/errors";
import { dismiss, notifications, notify, type Notification } from "../lib/notifications";
import { taskResult } from "../lib/tasks";
import { ELSEWHERE } from "../operation/operation";
import { currentLists, moveToChangelist, type ChosenChangelist } from "./applying";
import type { AppliedCommit, SelectedCommit, VcsAction } from "./contextMenu";
import { shortHash } from "./draw";

export type EditingAction = Extract<
  VcsAction,
  { kind: RewriteKind | "uncommit" | "fixup" | "squashInto" | "interactiveRebase" }
>;

export const isEditing = (action: VcsAction): action is EditingAction =>
  action.kind === "reword" ||
  action.kind === "drop" ||
  action.kind === "squash" ||
  action.kind === "uncommit" ||
  action.kind === "fixup" ||
  action.kind === "squashInto" ||
  action.kind === "interactiveRebase";

// What the non-modal Commit view receives from Fixup… and Squash Into…: the proposed message and, if the
// `Commit and Rebase…` executor is offered, the rebase base (`<hash>~`).
// `head` is the HEAD seen when proposing: if it changes, the executor is no longer valid.
export interface CommitProposal {
  message: string;
  rebaseFrom: string | null;
  head: string;
}

// `GitNewCommitMessageActionDialog`. `validate` runs on OK: the error text, or `null`.
export interface CommitMessageRequest {
  kind: "commitMessage";
  title: string;
  label: string;
  message: string;
  validate: () => Promise<string | null>;
}

// `GitInteractiveRebaseDialog` with the rows the log generated.
export interface InteractiveRebaseRequest {
  kind: "interactiveRebase";
  commits: RebaseCommit[];
}

// `GitDropLogAction`'s `MessageDialogBuilder.okCancel(...).asWarning().doNotAsk(...)`.
export interface DropConfirmRequest {
  kind: "dropConfirm";
  count: number;
}

export interface DropAnswer {
  ok: boolean;
  // The `Don't ask again` checkbox. It only counts with `ok`.
  doNotAsk: boolean;
}

export interface EditingUi extends RebaseUi {
  // The Drop Commits question, with its warning icon and the `Don't ask again` checkbox.
  confirmDrop(count: number): Promise<DropAnswer>;
  // The message dialog: `null` if cancelled.
  editMessage(request: CommitMessageRequest): Promise<string | null>;
  // `ChangeListChooser`, titled `Select Target Changelist`: `null` if cancelled.
  chooseChangelist(lists: ChangeList[], initial: string): Promise<ChosenChangelist | null>;
  // `CommitChangeListDialog.showCommitDialog`: the dialog (or the Commit view) with the proposed message.
  showCommit(input: CommitRequestInput): void;
  // `GitInteractiveRebaseDialog` with the log's rows: what was chosen, or `null` if cancelled.
  interactiveRebase(commits: RebaseCommit[]): Promise<RebasePlan | null>;
}

export interface EditingApi {
  state(): Promise<RepoState>;
  // `GitVcsApplicationSettings.isShowDropCommitDialog`.
  askBeforeDropping(): Promise<boolean>;
  // `Don't ask again` of the Drop Commits question: `isShowDropCommitDialog = false`.
  stopAskingBeforeDropping(): Promise<void>;
  // `lastCheckCommitsEditingAvailability`: the reason, or `null` if possible.
  check(hashes: string[], kind: RewriteKind): Promise<string | null>;
  details(hashes: string[]): Promise<CommitDetails[]>;
  reword(hash: string, message: string, expectedHead: string): Promise<Rewritten>;
  drop(hashes: string[], expectedHead: string): Promise<Rewritten>;
  squash(hashes: string[], message: string, expectedHead: string): Promise<Rewritten>;
  undo(kind: RewriteKind, rewritten: Rewritten): Promise<Undone>;
  // `checkUndoPossibility`: why it is no longer possible, or `null`.
  undoImpossible(rewritten: Rewritten): Promise<string | null>;
  // `GitNewCommitMessageActionDialog.validate`, on OK: the error text, or `null`.
  validate(hashes: string[], expectedHead: string): Promise<string | null>;
  // Undo Commit…, Fixup… and Squash Into… on click: the reason, or `null` if possible.
  checkSingle(hash: string, action: SingleAction): Promise<string | null>;
  // `reset --soft` to the parent: the commit's paths.
  uncommit(hash: string, expectedHead: string): Promise<string[]>;
  // The rebase of `Commit and Rebase…`, from `<hash>~`.
  autosquash(base: string): Promise<RebaseRun>;
  // With the staging area there are no changelists (`areChangeListsEnabled`).
  stagingArea(): Promise<boolean>;
  // The changelists brought up to date with what is changed.
  changelists(): Promise<Changelists>;
  // The files move to the list (new if it does not exist), which becomes active if requested.
  moveToChangelist(paths: string[], name: string, setActive: boolean): Promise<void>;
  // The changes of the active list (`changeList.changes` of the repository) as a request for the Commit
  // Changes dialog, or `null` if it has none.
  activeChanges(): Promise<CheckinRequest | null>;
  // Interactively Rebase from Here…: the base and the log's rows, if they are usable.
  interactiveStart(hash: string): Promise<InteractiveStart>;
  // `rebase --interactive <parent>`; without `plan`, git generates the list and the window shows it.
  interactive(hash: string, plan: RebasePlan | null, expectedHead: string): Promise<RebaseRun>;
}

const result = async <T>(id: Promise<number>) => taskResult<T>(await id);

export const backend: EditingApi = {
  state: repoState,
  askBeforeDropping: async () => (await settingsGet()).showDropCommitDialog,
  // Read again to save: whatever else changed in the meantime is kept.
  stopAskingBeforeDropping: async () => {
    await settingsSave({ ...(await settingsGet()), showDropCommitDialog: false });
  },
  check: (hashes, kind) => result(rewriteCheck(hashes, kind)),
  details: commitDetails,
  reword: (hash, message, head) => result(rewriteReword(hash, message, head)),
  drop: (hashes, head) => result(rewriteDrop(hashes, head)),
  squash: (hashes, message, head) => result(rewriteSquash(hashes, message, head)),
  undo: (kind, { oldHead, newHead }) => result(rewriteUndo(kind, oldHead, newHead)),
  undoImpossible: ({ oldHead, newHead }) => result(rewriteUndoCheck(oldHead, newHead)),
  validate: (hashes, head) => result(rewriteValidate(hashes, head)),
  checkSingle: (hash, action) => result(rewriteCheckSingle(hash, action)),
  uncommit: (hash, head) => result(rewriteUncommit(hash, head)),
  autosquash: (base) => result(rewriteAutosquash(base)),
  stagingArea: async () => effectiveStaging(await settingsGet()),
  changelists: currentLists,
  // Only what is still changed: after the reset, a path that is no longer there has no list.
  moveToChangelist: async (paths, name, setActive) => {
    const present = new Set((await repoStatus(false)).map((change) => change.path));
    await moveToChangelist(
      paths.filter((path) => present.has(path)),
      name,
      setActive,
    );
  },
  activeChanges: async () => {
    const status = await repoStatus(false);
    // With the staging area the changelists are not read, and everything versioned counts as the active
    // list.
    if (effectiveStaging(await settingsGet())) {
      const paths = status
        .filter(({ state }) => !["untracked", "ignored", "unmerged"].includes(state))
        .map(({ path }) => path);
      return paths.length > 0 ? { kind: "project", paths, lists: [] } : null;
    }
    const lists = await currentLists();
    const paths = listFiles(status, lists).get(lists.active) ?? [];
    return paths.length > 0 ? { kind: "project", paths, lists: [lists.active] } : null;
  },
  interactiveStart: (hash) => result(rewriteInteractiveStart(hash)),
  interactive: (hash, plan, head) => result(rewriteInteractive(hash, plan, head)),
};

// What the log menu shows for each entry for the selection.
export const menuChecks = (hashes: string[]) => result<RewriteMenu>(rewriteMenu(hashes));

// `Git.Reword.Commit` with F2 or ⇧F6 in the table: like the menu entry, only if it is enabled.
export async function rewordFromKeyboard(
  commits: SelectedCommit[],
  checks: (hashes: string[]) => Promise<RewriteMenu>,
  vcs: ((action: VcsAction) => void) | undefined,
): Promise<boolean> {
  if (commits.length !== 1 || !vcs) return false;
  const [{ hash, subject }] = commits;
  const reason = await checks([hash]).then(
    (menu) => menu.reword,
    // Without knowing it, it is not turned off: the core checks it again.
    () => null,
  );
  if (reason !== null) return false;
  vcs({ kind: "reword", commit: { hash, subject } });
  return true;
}

interface Texts {
  // `getFailureTitle`.
  failure: string;
  success: (count: number) => string;
  impossible: string;
  undoFailed: string;
}

const TEXTS: Record<RewriteKind, Texts> = {
  reword: {
    failure: "Could not Edit Commit Message",
    success: () => "Commit Message Changed",
    impossible: "Cannot Undo Commit Message Edit",
    undoFailed: "Undo Commit Message Edit Failed",
  },
  drop: {
    failure: "Could not Drop Commits",
    success: (count) => `Dropped ${count} ${count === 1 ? "commit" : "commits"}`,
    impossible: "Cannot undo drop",
    undoFailed: "Undo drop failed",
  },
  squash: {
    failure: "Could not Squash Commits",
    success: () => "Commits Squashed",
    impossible: "Cannot Undo Squash",
    undoFailed: "Undo Squash Failed",
  },
};

// `git.undo.action.*`.
const UNCOMMIT = {
  failure: "Cannot Undo Commit",
  successful: "Undo commit successful",
  failed: "Undo commit failed",
};
// `rebase.log.create.fixup.commit.action.failure.title`, `rebase.log.create.squash.commit.action.failure.title`
// and the prefix of `GitCommitFixupBySubjectAction` and `GitCommitSquashBySubjectAction`.
const AUTOSQUASH = {
  fixup: { failure: "Cannot Create Fixup Commit", prefix: "fixup!" },
  squashInto: { failure: "Cannot Create Squash Commit", prefix: "squash!" },
};
// `commit.dialog.no.changes.detected.title` and `commit.dialog.no.changes.detected.text`.
const NOTHING_TO_COMMIT = { title: "Nothing to Commit", body: "No changes detected" };
// `rebase.notification.successful.title`, `rebase.notification.failed.rebase.title`.
const REBASE_SUCCESSFUL = "Rebase successful";
const REBASE_FAILED = "Rebase failed";
// `rebase.log.interactive.action.failure.title`, `rebase.notification.action.retry.text` and
// `rebase.using.log.couldnt.start.error` without the IDE name, as `rewrite.rs` returns it.
const CANNOT_START_REBASE = "Cannot Start Rebase";
const RETRY = "Retry";
export const INVALID_TODO =
  "Cannot perform the rebase because an invalid 'git-rebase-todo' file was generated. Do you want to retry the rebase using Git native file generation?";

// What `rebase::Process` saved, for the notices about what does not come back.
const REBASE_SAVING: Saving = { operation: "rebase", from: "HEAD" };
const UNDO = "Undo";
// `"\n".repeat(3)` of `GitSquashLogAction`.
const SQUASH_SEPARATOR = "\n\n\n";

// `VcsCommitMetadata.getFullMessage`, rebuilt from the subject and body of the details.
const fullMessage = ({ subject, body }: CommitDetails) => (body ? `${subject}\n\n${body}` : subject);

// `MAX_COMMITS_IN_NOTIFICATION` and the length of `StringUtil.shortenTextWithEllipsis(subject, 40, 0)`.
const MAX_DROPPED_LISTED = 10;
const MAX_SUBJECT = 40;
const ELLIPSIS = "...";

// The body of the Drop success: the first subjects in straight double quotes, one per line, and `...` if
// there are more.
function droppedSubjects(commits: AppliedCommit[]): string {
  const lines = commits.slice(0, MAX_DROPPED_LISTED).map(({ subject }) => {
    const chars = [...subject];
    const text =
      chars.length > MAX_SUBJECT
        ? chars.slice(0, MAX_SUBJECT - ELLIPSIS.length).join("") + ELLIPSIS
        : subject;
    return `"${text}"`;
  });
  if (commits.length > MAX_DROPPED_LISTED) lines.push(ELLIPSIS);
  return lines.join("\n");
}

const isCancelled = (error: unknown) => (error as ApiError).kind === "cancelled";

interface Card {
  kind: RewriteKind;
  rewritten: Rewritten;
}

export function editingFlow(ui: EditingUi, api: EditingApi = backend) {
  // The cards with Undo that follow the view, of the open repository.
  const cards = new Map<number, Card>();
  // Goes up when the repository changes. What finishes afterwards belonged to the previous one.
  let generation = 0;

  function failed(kind: RewriteKind, error: unknown) {
    if (isCancelled(error)) return;
    notify({ kind: "error", title: TEXTS[kind].failure, body: errorText(error) });
  }

  async function undo(id: number, card: Card) {
    dismiss(id);
    cards.delete(id);
    const texts = TEXTS[card.kind];
    let undone: Undone;
    try {
      undone = await api.undo(card.kind, card.rewritten);
    } catch (error) {
      if (!isCancelled(error)) {
        notify({ kind: "error", title: texts.undoFailed, body: errorText(error) });
      }
      return;
    }
    if (undone.kind === "impossible") {
      notify({ kind: "error", title: texts.impossible, body: undone.reason });
    } else if (undone.kind === "failed") {
      notify({ kind: "error", title: texts.undoFailed, body: undone.reason });
    }
  }

  // `notifySuccess`: the card with Undo, which stays until the repository changes and it is no longer valid.
  function succeeded(
    kind: RewriteKind,
    count: number,
    rewritten: Rewritten,
    started: number,
    body?: string,
  ) {
    const title = TEXTS[kind].success(count);
    if (generation !== started) {
      notify({ kind: "info", title, body: ELSEWHERE });
      return;
    }
    const card: Card = { kind, rewritten };
    // Not expiring by time: it stays until the repository changes and Undo is no longer valid
    // (`repositoryRefreshed`).
    const notification: Notification = {
      kind: "info",
      title,
      body,
      sticky: true,
      actions: [{ label: UNDO, run: () => void undo(id, card) }],
    };
    const id = notify(notification);
    cards.set(id, card);
  }

  const ongoing = async () => {
    try {
      return (await api.state()).operation === "rebasing";
    } catch {
      return true;
    }
  };

  async function finish(
    kind: RewriteKind,
    count: number,
    work: () => Promise<Rewritten>,
    started: number,
    body?: string,
  ) {
    let rewritten: Rewritten;
    try {
      rewritten = await work();
    } catch (error) {
      failed(kind, error);
      return;
    }
    if (generation !== started) {
      if (rewritten.outcome.kind === "done" && !rewritten.title) {
        notify({ kind: "info", title: TEXTS[kind].success(count), body: ELSEWHERE });
      }
      return;
    }
    await finishRebase(
      rewritten,
      ui,
      REBASE_SAVING,
      () => succeeded(kind, count, rewritten, started, body),
      ongoing,
    );
  }

  // The checks against the history (`lastCheckCommitsEditingAvailability`): `false` if it was refused.
  async function allowed(kind: RewriteKind, hashes: string[]): Promise<boolean> {
    try {
      const reason = await api.check(hashes, kind);
      if (reason === null) return true;
      notify({ kind: "error", title: TEXTS[kind].failure, body: reason });
    } catch (error) {
      failed(kind, error);
    }
    return false;
  }

  // The HEAD seen now.
  async function headState(kind: RewriteKind): Promise<RepoState | null> {
    try {
      return await api.state();
    } catch (error) {
      failed(kind, error);
      return null;
    }
  }

  // What is common before acting: the checks and then the HEAD.
  async function prepare(kind: RewriteKind, hashes: string[]): Promise<RepoState | null> {
    return (await allowed(kind, hashes)) ? headState(kind) : null;
  }

  // The message dialog, which does not accept if HEAD moved while it was open or if the oldest
  // is already on a protected branch: the error appears in the dialog, which stays with what was typed.
  function askMessage(title: string, label: string, message: string, head: string, hashes: string[]) {
    return ui.editMessage({
      kind: "commitMessage",
      title,
      label,
      message,
      validate: async () => {
        try {
          return await api.validate(hashes, head);
        } catch (error) {
          return errorText(error);
        }
      },
    });
  }

  // `GitRewordAction.actionPerformedAfterChecks`.
  async function reword({ hash }: AppliedCommit, started: number) {
    const state = await prepare("reword", [hash]);
    if (!state) return;
    let commit: CommitDetails | undefined;
    try {
      [commit] = await api.details([hash]);
    } catch (error) {
      failed("reword", error);
      return;
    }
    if (!commit || generation !== started) return;
    // `VcsUserUtil.getShortPresentation`.
    const author = commit.author || commit.authorEmail;
    const label = `Edit message for commit ${shortHash(hash)} by ${author}`;
    const message = await askMessage("Edit Commit Message", label, fullMessage(commit), state.head, [hash]);
    if (message === null || generation !== started) return;
    await finish("reword", 1, () => api.reword(hash, message, state.head), started);
  }

  // `GitSquashLogAction.actionPerformedAfterChecks`: the messages, in the order of the selection.
  async function squash(commits: AppliedCommit[], started: number) {
    const hashes = commits.map(({ hash }) => hash);
    const state = await prepare("squash", hashes);
    if (!state) return;
    let all: CommitDetails[];
    try {
      all = await api.details(hashes);
    } catch (error) {
      failed("squash", error);
      return;
    }
    if (generation !== started) return;
    const joined = all.map(fullMessage).join(SQUASH_SEPARATOR);
    const label = "Edit message for squashed commit";
    const message = await askMessage("Squashed Commit Message", label, joined, state.head, hashes);
    if (message === null || generation !== started) return;
    await finish("squash", hashes.length, () => api.squash(hashes, message, state.head), started);
  }

  // `GitDropLogAction.actionPerformedAfterChecks`: the question (unless turned off) comes after the checks
  // and before anything is read. `Don't ask again` is saved only on OK.
  async function drop(commits: AppliedCommit[], started: number) {
    const hashes = commits.map(({ hash }) => hash);
    if (!(await allowed("drop", hashes))) return;
    try {
      if (await api.askBeforeDropping()) {
        const answer = await ui.confirmDrop(hashes.length);
        if (!answer.ok) return;
        if (answer.doNotAsk) await api.stopAskingBeforeDropping();
      }
    } catch (error) {
      failed("drop", error);
      return;
    }
    if (generation !== started) return;
    const state = await headState("drop");
    if (!state) return;
    await finish("drop", hashes.length, () => api.drop(hashes, state.head), started, droppedSubjects(commits));
  }

  // `GitUncommitAction.actionPerformedAfterChecks`: the changelist chooser (if there are any) and
  // `reset --soft`; the commit's paths then move to the chosen list.
  async function uncommit({ hash, subject }: AppliedCommit, started: number) {
    const error = (title: string, failure: unknown) => {
      if (!isCancelled(failure)) notify({ kind: "error", title, body: errorText(failure) });
    };
    let head: string;
    let target: ChosenChangelist | null = null;
    try {
      const reason = await api.checkSingle(hash, "uncommit");
      if (reason !== null) {
        notify({ kind: "error", title: UNCOMMIT.failure, body: reason });
        return;
      }
      head = (await api.state()).head;
      if (!(await api.stagingArea())) {
        const lists = await api.changelists();
        target = await ui.chooseChangelist(lists.lists, subject);
        if (!target) return;
      }
    } catch (failure) {
      error(UNCOMMIT.failure, failure);
      return;
    }
    // While the chooser was up another repository could have been opened: the reset would no longer be of this one.
    if (generation !== started) return;
    let paths: string[];
    try {
      paths = await api.uncommit(hash, head);
    } catch (failure) {
      error(UNCOMMIT.failed, failure);
      return;
    }
    notify({ kind: "info", title: UNCOMMIT.successful });
    if (!target || generation !== started) return;
    try {
      await api.moveToChangelist(paths, target.name, target.setActive);
    } catch (failure) {
      error("Cannot move the changes to the changelist", failure);
    }
  }

  // `GitAutoSquashCommitAction.actionPerformedAfterChecks`: the commit dialog with the active list's changes
  // and `fixup! <subject>` or `squash! <subject>`, and `Commit and Rebase…` if there is no operation in progress.
  async function autosquash(kind: "fixup" | "squashInto", { hash, subject }: AppliedCommit, started: number) {
    const texts = AUTOSQUASH[kind];
    try {
      const reason = await api.checkSingle(hash, "autosquash");
      if (reason !== null) {
        notify({ kind: "error", title: texts.failure, body: reason });
        return;
      }
      const request = await api.activeChanges();
      if (!request) {
        notify({ kind: "info", ...NOTHING_TO_COMMIT });
        return;
      }
      const state = await api.state();
      if (generation !== started) return;
      // `getProhibitedStateMessage`: only with the normal repository or with a detached HEAD.
      const executor = state.operation === null ? { rebaseFrom: `${hash}~`, head: state.head } : {};
      ui.showCommit({ request, prefilled: `${texts.prefix} ${subject}`, ...executor });
    } catch (failure) {
      if (!isCancelled(failure)) {
        notify({ kind: "error", title: texts.failure, body: errorText(failure) });
      }
    }
  }

  // `GitRebaseCheckinHandlerFactory.checkinSuccessful`: `rebase --interactive <base>` once the commit
  // is made. Without Undo: it is not a rewrite of the log.
  async function rebaseAfterCommit(base: string) {
    const started = generation;
    let run: RebaseRun;
    try {
      run = await api.autosquash(base);
    } catch (failure) {
      if (!isCancelled(failure)) {
        notify({ kind: "error", title: REBASE_FAILED, body: errorText(failure) });
      }
      return;
    }
    await rebased(run, base, started);
  }

  // What follows a rebase that finished, stopped or did not start: `Rebase successful` on `base`, or that of
  // `finishRebase`.
  async function rebased(run: RebaseRun, base: string, started: number) {
    if (generation !== started) {
      if (run.outcome.kind === "done" && !run.title) {
        notify({ kind: "info", title: REBASE_SUCCESSFUL, body: ELSEWHERE });
      }
      return;
    }
    const done = async () => {
      let branch: string | null = null;
      try {
        branch = (await api.state()).branch;
      } catch {
        // It is a datum of the notice: without it, only the name is missing.
      }
      notify({ kind: "info", title: REBASE_SUCCESSFUL, body: rebasedMessage(branch, base, false) });
    };
    await finishRebase(run, ui, REBASE_SAVING, done, ongoing);
  }

  // `startInteractiveRebase`. If git did not write the list the log generated, repeating with the
  // git one is offered (the text asks for it).
  async function startInteractive(
    hash: string,
    plan: RebasePlan | null,
    head: string,
    base: string,
    started: number,
  ) {
    let run: RebaseRun;
    try {
      run = await api.interactive(hash, plan, head);
    } catch (failure) {
      if (isCancelled(failure)) return;
      const body = errorText(failure);
      const retry =
        plan !== null && body === INVALID_TODO && generation === started
          ? [{ label: RETRY, run: () => void startInteractive(hash, null, head, base, started) }]
          : undefined;
      notify({ kind: "error", title: REBASE_FAILED, body, actions: retry });
      return;
    }
    await rebased(run, base, started);
  }

  // `GitInteractiveRebaseAction.actionPerformedAfterChecks` with `interactivelyRebaseUsingLog`: if the
  // log generates the list, the dialog appears before launching git, and cancelling it launches nothing. If not, git
  // generates the list and the dialog appears when it asks for it.
  async function interactiveRebase({ hash }: AppliedCommit, started: number) {
    let head: string;
    let start: InteractiveStart;
    try {
      const reason = await api.checkSingle(hash, "rebase");
      if (reason !== null) {
        notify({ kind: "error", title: CANNOT_START_REBASE, body: reason });
        return;
      }
      head = (await api.state()).head;
      start = await api.interactiveStart(hash);
    } catch (failure) {
      if (!isCancelled(failure)) {
        notify({ kind: "error", title: CANNOT_START_REBASE, body: errorText(failure) });
      }
      return;
    }
    if (generation !== started) return;
    let plan: RebasePlan | null = null;
    if (start.commits) {
      plan = await ui.interactiveRebase(start.commits);
      if (!plan || generation !== started) return;
    }
    await startInteractive(hash, plan, head, start.base, started);
  }

  async function run(action: VcsAction): Promise<void> {
    if (!isEditing(action)) return;
    const started = generation;
    switch (action.kind) {
      case "interactiveRebase":
        return interactiveRebase(action.commit, started);
      case "reword":
        return reword(action.commit, started);
      case "uncommit":
        return uncommit(action.commit, started);
      case "fixup":
      case "squashInto":
        return autosquash(action.kind, action.commit, started);
    }
    if (action.commits.length === 0) return;
    if (action.kind === "drop") return drop(action.commits, started);
    return squash(action.commits, started);
  }

  // After `repo-changed`: the one that can no longer be undone goes away, like `notification.expire()`.
  async function repositoryRefreshed() {
    const started = generation;
    const visible = new Set(get(notifications).map(({ id }) => id));
    for (const [id, card] of [...cards]) {
      if (!visible.has(id)) {
        cards.delete(id);
        continue;
      }
      let reason: string | null;
      try {
        reason = await api.undoImpossible(card.rewritten);
      } catch {
        continue;
      }
      if (reason !== null && generation === started) {
        dismiss(id);
        cards.delete(id);
      }
    }
  }

  // Another repository: what was before must not remain there.
  function repositoryChanged() {
    generation += 1;
    cards.forEach((_, id) => dismiss(id));
    cards.clear();
  }

  return { run, rebaseAfterCommit, repositoryRefreshed, repositoryChanged };
}
