// Stash Changes…, Unstash Changes…, Stash Silently and Stash Files, and what the Stash tab shares with
// them. Ported from `GitStash`, `GitUnstash`, `GitStashSilentlyAction`, `GitStageStashFilesAction`,
// `GitUnstashDialog`, `GitUnstashAsDialog` and `GitStashOperations`; the texts are the ones in
// `GitBundle` and `VcsBundle`. Stash messages come from the repository: they are shown as text.
import {
  branchList,
  repoState,
  stashChanges,
  stashClear,
  stashDrop,
  stashList,
  stashPushPaths,
  stashSave,
  stashUnstash,
  type ApiError,
  type BranchList,
  type FileSides,
  type RepoInfo,
  type RepoState,
  type Side,
  type StashChanges,
  type StashEntry,
  type Unstashed,
} from "../lib/api";
import { isValidRefName, type BranchNames } from "../branches/refName";
import type { ConflictsResult, ResolveOptions } from "../conflicts/flow";
import { errorText } from "../lib/errors";
import { dismiss, notify } from "../lib/notifications";
import { requestTab } from "../lib/tabs";
import { taskResult } from "../lib/tasks";
import type { SavedChange, SavedDiff } from "./savedPatches";
import { stashChangesBackend, stashViewChanges, stashViewDiff } from "./stashChanges";

// `common.no.active.branch`.
export const NO_ACTIVE_BRANCH = "<no active branch>";
// `stash.files.success`, `stash.view.stashes.link`, `stash.error`, `stash.files.error.message`.
const STASHED = "Files stashed successfully";
const VIEW_STASHES = "View Git stashes";
const STASH_FILES_FAILED = "Failed to Stash Files";
// `stash.changes.non.empty.index.notification.action` and its `git.stash.notify.non.empty.index`.
const DONT_SHOW_AGAIN = "Don't show again";
const NON_EMPTY_INDEX_KEY = "stash.notifyNonEmptyIndex";
// `patch.apply.success.applied.text`.
const APPLIED = "Patch successfully applied";
// `notification.title.unstash.failed` and `.index.conflict`.
const UNSTASH_FAILED = "Unstash failed";
const INDEX_CONFLICT = "Unstash failed due to an index conflict";
// `unstash.unstashed.with.conflicts.error.title` and `unstash.dialog.unresolved.conflict.warning.*`.
const UNSTASHED_WITH_CONFLICTS = "Unstashed with conflicts";
const NOT_RESOLVED_TITLE = "Conflicts were not resolved during unstash";
const NOT_RESOLVED_BODY =
  "Unstash is not complete, you have unresolved merges in your working tree.";
const RESOLVE_CONFLICTS = "Resolve conflicts…";
// `GitUntrackedFilesHelper` and `LocalChangesWouldBeOverwrittenHelper` with `unstash.operation.name`.
const UNTRACKED_TITLE = "Untracked Files Prevent Unstash";
const UNTRACKED_BODY = "Move or commit them before unstash";
const UNTRACKED_DIALOG = "Untracked Files Preventing Unstash";
const UNTRACKED_MESSAGE = "These untracked files will be overwritten by unstash";
const VIEW_FILES = "View Files…";
const LOCAL_CHANGES_TITLE = "Git Unstash Failed";
const LOCAL_CHANGES =
  "Your local changes will be overwritten by merge.\nCommit, stash, or revert them to proceed.";
const LOCAL_CHANGES_DIALOG = "Local Changes Prevent from Unstash";
const VIEW_THEM = "View them";
// `unstash.dialog.show.stashes.error.dialog.title`.
export const LIST_FAILED = "Could not Show the List of Stashes";

export interface StashChoice {
  message: string;
  keepIndex: boolean;
}

// `branch` empty: pop or apply, each option as it was chosen (decision 11).
export interface UnstashChoice {
  entry: StashEntry;
  branch: string;
  pop: boolean;
  reinstateIndex: boolean;
}

// What the Unstash dialog does while it is open: it reloads the list after Drop and Clear, and it
// closes only when `unstash` says it completed (`GitUnstashDialog.doOKAction`).
export interface UnstashActions {
  load(): Promise<StashEntry[]>;
  drop(entry: StashEntry): Promise<void>;
  clear(): Promise<void>;
  // `View` (`viewStash(…, compareWithLocal = true)`): the stash's changes, or `null` if they could not
  // be read (already notified), and the diff of each one with the local file.
  view(entry: StashEntry): Promise<SavedChange[] | null>;
  viewDiff(entry: StashEntry, change: SavedChange): Promise<SavedDiff>;
  unstash(choice: UnstashChoice): Promise<boolean>;
}

export type StashRequest =
  | { kind: "stash"; root: string; branch: string }
  | ({ kind: "unstash"; root: string; branch: string; names: BranchNames } & UnstashActions)
  | { kind: "unstashAs"; root: string; branch: string; entry: StashEntry; names: BranchNames }
  | { kind: "stashConfirm"; title: string; message: string }
  | { kind: "files"; title: string; message: string; files: string[] };

export interface StashUi {
  // `null` if cancelled.
  ask<T>(request: StashRequest): Promise<T | null>;
  resolveConflicts(options?: ResolveOptions): Promise<ConflictsResult>;
  // `Git.Show.Stash`: `false` when the Stash tab is not there, because there are no stashes.
  showStash?: () => boolean;
}

// `VcsShowToolWindowTabAction.actionPerformed`: what Show Git Stash (and Show Shelf) does, by whether its
// tool window is active and whether the tab is the selected one.
export type ShowTabStep = "hide" | "select" | "activate" | "activateAndSelect";

export function showTabStep(windowActive: boolean, tabSelected: boolean): ShowTabStep {
  if (windowActive) return tabSelected ? "hide" : "select";
  return tabSelected ? "activate" : "activateAndSelect";
}

export interface StashApi {
  state(): Promise<RepoState>;
  branches(): Promise<BranchList>;
  list(): Promise<StashEntry[]>;
  save(message: string, keepIndex: boolean): Promise<void>;
  unstash(
    sha: string,
    pop: boolean,
    reinstateIndex: boolean,
    branch: string | null,
    expectedHead: string,
  ): Promise<Unstashed>;
  drop(sha: string): Promise<void>;
  clear(): Promise<void>;
  changes(sha: string): Promise<StashChanges>;
  sides(sha: string, path: string, oldPath: string | null): Promise<FileSides>;
  local(path: string): Promise<Side>;
}

const result = async <T>(id: Promise<number>) => taskResult<T>(await id);

const backend: StashApi = {
  state: repoState,
  branches: branchList,
  list: () => result<StashEntry[]>(stashList()),
  save: (message, keepIndex) => result<void>(stashSave(message, keepIndex)),
  unstash: (sha, pop, reinstateIndex, branch, head) =>
    result<Unstashed>(stashUnstash(sha, pop, reinstateIndex, branch, head)),
  drop: (sha) => result<void>(stashDrop(sha)),
  clear: () => result<void>(stashClear()),
  changes: (sha) => result<StashChanges>(stashChanges(sha)),
  sides: stashChangesBackend.sides,
  local: stashChangesBackend.local,
};

const isCancelled = (error: unknown) => (error as ApiError).kind === "cancelled";

// `VcsImplUtil.getShortVcsRootName`: the folder's name.
export function rootName(root: string): string {
  const trimmed = root.replace(/\/+$/, "");
  return trimmed.slice(trimmed.lastIndexOf("/") + 1) || trimmed;
}

const label = (entry: StashEntry) => `stash@{${entry.index}}`;

// `git.unstash.drop.confirmation.*`: the `<br/>` is a line break.
export function dropConfirmation(entry: StashEntry): { title: string; message: string } {
  return {
    title: `Remove Stash ${label(entry)}?`,
    message: `Do you want to remove ${label(entry)}?\n"${entry.message}"`,
  };
}

// `git.unstash.clear.confirmation.*`.
export function clearConfirmation(): { title: string; message: string } {
  return { title: "Remove All Stashes?", message: "Remove all stashes? This cannot be undone." };
}

// `validateName`: the text as typed, against the repository's branches.
export function unstashBranchProblem(text: string, names: BranchNames): string | null {
  if (text.trim() === "") return "Specify name for the new branch";
  if (text.toUpperCase() === "HEAD") return "HEAD is a reserved keyword";
  if (!isValidRefName(text)) return `Branch name ${text} is not valid`;
  if (names.local.includes(text)) return `Branch name ${text} already exists`;
  if (names.remote.includes(text)) {
    return `Branch name ${text} conflicts with remote branch with the same name`;
  }
  return null;
}

// `GitStashOperations.showSuccessNotification`: the Stash tab is on (`git.enable.stash.toolwindow`).
export function notifyStashed(): void {
  notify({
    kind: "info",
    title: STASHED,
    actions: [{ label: VIEW_STASHES, run: () => requestTab("stash") }],
  });
}

function notifyStashFailed(root: string, error: unknown) {
  if (isCancelled(error)) return;
  notify({ kind: "error", title: `Failed to stash '${rootName(root)}'`, body: errorText(error) });
}

function nonEmptyIndexShown(): boolean {
  try {
    return localStorage.getItem(NON_EMPTY_INDEX_KEY) !== "false";
  } catch {
    return true;
  }
}

// `Stash Silently` (`GitStashSilentlyAction`): `stash save` without a message.
export async function stashSilently(
  root: string,
  save: (message: string, keepIndex: boolean) => Promise<void> = backend.save,
): Promise<void> {
  try {
    await save("", false);
  } catch (error) {
    notifyStashFailed(root, error);
    return;
  }
  notifyStashed();
}

// `Stash Files` (`GitStashOperation`). `otherStaged`: the stage had files that were not chosen.
export async function stashFiles(
  root: string,
  paths: string[],
  otherStaged: boolean,
  push: (paths: string[]) => Promise<void> = (chosen) => result<void>(stashPushPaths(chosen)),
): Promise<void> {
  try {
    await push(paths);
  } catch (error) {
    if (!isCancelled(error)) {
      notify({ kind: "error", title: STASH_FILES_FAILED, body: errorText(error) });
    }
    return;
  }
  notifyStashed();
  if (!otherStaged || !nonEmptyIndexShown()) return;
  const id = notify({
    kind: "warning",
    title: `Staging area in '${rootName(root)}' contains modifications to other files that were saved to the stash`,
    actions: [
      {
        label: DONT_SHOW_AGAIN,
        run: () => {
          try {
            localStorage.setItem(NON_EMPTY_INDEX_KEY, "false");
          } catch {
            // Without storage it is asked again next time.
          }
          dismiss(id);
        },
      },
    ],
  });
}

export function stashFlow(ui: StashUi, api: StashApi = backend) {
  let root = "";
  let running = false;
  // Those that act on the open repository: in another one, their buttons would make no sense.
  const cards = new Set<number>();
  // Goes up when the repository changes: what finishes afterwards belonged to the previous one.
  let generation = 0;

  const card = (notification: Parameters<typeof notify>[0]) => {
    const id = notify(notification);
    cards.add(id);
    return id;
  };

  // One at a time: with a dialog open, another request does not open another on top.
  async function exclusive(failure: string, work: () => Promise<void>) {
    if (running) return;
    running = true;
    try {
      await work();
    } catch (error) {
      if (!isCancelled(error)) notify({ kind: "error", title: failure, body: errorText(error) });
    } finally {
      running = false;
    }
  }

  const branchText = (state: RepoState) => state.branch ?? NO_ACTIVE_BRANCH;

  async function names(): Promise<BranchNames> {
    const list = await api.branches();
    return { local: list.local.map(({ name }) => name), remote: list.remote };
  }

  // `GitStash`.
  function stashChanges(): Promise<void> {
    return exclusive(`Failed to stash '${rootName(root)}'`, async () => {
      const started = generation;
      const state = await api.state();
      const choice = await ui.ask<StashChoice>({ kind: "stash", root, branch: branchText(state) });
      if (!choice || generation !== started) return;
      try {
        await api.save(choice.message, choice.keepIndex);
      } catch (error) {
        notifyStashFailed(root, error);
        return;
      }
      notifyStashed();
    });
  }

  function notifyUnresolved(started: number) {
    if (generation !== started) return;
    const id = card({
      kind: "warning",
      title: NOT_RESOLVED_TITLE,
      body: NOT_RESOLVED_BODY,
      actions: [
        {
          label: RESOLVE_CONFLICTS,
          run: () => {
            dismiss(id);
            cards.delete(id);
            void ui.resolveConflicts(conflictOptions(started));
          },
        },
      ],
    });
  }

  // `UnstashConflictResolver`: the dialog's texts come from the backend, which knows the stash.
  const conflictOptions = (started: number): ResolveOptions => ({
    title: UNSTASHED_WITH_CONFLICTS,
    onpending: () => notifyUnresolved(started),
  });

  async function showFiles(title: string, message: string, files: string[]) {
    await ui.ask<void>({ kind: "files", title, message, files });
  }

  // `GitStashOperations.unstash`, in the order it looks at the result. `true` if it completed: applied,
  // or with conflicts that ended resolved.
  async function show(outcome: Unstashed, started: number): Promise<boolean> {
    switch (outcome.kind) {
      case "applied":
        notify({ kind: "info", title: APPLIED });
        return true;
      case "indexConflict":
        notify({ kind: "error", title: INDEX_CONFLICT, body: outcome.reason });
        return false;
      case "conflicts": {
        const resolved = (await ui.resolveConflicts(conflictOptions(started))) === "resolved";
        if (resolved && generation === started) notify({ kind: "info", title: APPLIED });
        return resolved;
      }
      case "untracked": {
        const { files } = outcome.reason;
        card({
          kind: "error",
          title: UNTRACKED_TITLE,
          body: UNTRACKED_BODY,
          actions: [
            {
              label: VIEW_FILES,
              run: () => void showFiles(UNTRACKED_DIALOG, UNTRACKED_MESSAGE, files),
            },
          ],
        });
        return false;
      }
      case "localChanges": {
        const { files } = outcome.reason;
        card({
          kind: "error",
          title: LOCAL_CHANGES_TITLE,
          body: LOCAL_CHANGES,
          actions: [
            {
              label: VIEW_THEM,
              run: () => void showFiles(LOCAL_CHANGES_DIALOG, LOCAL_CHANGES, files),
            },
          ],
        });
        return false;
      }
      case "failed":
        notify({ kind: "error", title: UNSTASH_FAILED, body: outcome.reason });
        return false;
    }
  }

  async function runUnstash(choice: UnstashChoice, head: string, started: number): Promise<boolean> {
    const branch = choice.branch.trim();
    let outcome: Unstashed;
    try {
      outcome = branch
        ? await api.unstash(choice.entry.sha, false, false, branch, head)
        : await api.unstash(choice.entry.sha, choice.pop, choice.reinstateIndex, null, head);
    } catch (error) {
      if (!isCancelled(error)) {
        notify({ kind: "error", title: UNSTASH_FAILED, body: errorText(error) });
      }
      return false;
    }
    return generation === started && (await show(outcome, started));
  }

  // The Unstash dialog's own Drop and Clear (it asks first itself and reloads afterwards) and its unstash.
  const actions = (head: string, started: number): UnstashActions => ({
    load: async () => {
      try {
        return await api.list();
      } catch (error) {
        notify({ kind: "error", title: LIST_FAILED, body: errorText(error) });
        return [];
      }
    },
    drop: async (entry) => {
      try {
        await api.drop(entry.sha);
      } catch (error) {
        if (!isCancelled(error)) notify({ kind: "error", title: errorText(error) });
      }
    },
    clear: async () => {
      try {
        await api.clear();
      } catch (error) {
        if (!isCancelled(error)) notify({ kind: "error", title: errorText(error) });
      }
    },
    view: async (entry) => {
      try {
        return stashViewChanges(entry, await api.changes(entry.sha));
      } catch (error) {
        if (!isCancelled(error)) {
          const title = `Paths Affected by stash@{${entry.index}}`;
          notify({ kind: "error", title, body: errorText(error) });
        }
        return null;
      }
    },
    viewDiff: (_entry, change) => stashViewDiff(api, change),
    unstash: (choice) => runUnstash(choice, head, started),
  });

  // `GitUnstash`: `Git.Show.Stash` while the Stash tab is there; otherwise, with nothing to show, the
  // old `GitUnstashDialog`, which runs the unstash itself and stays open until it completes.
  async function unstashChanges(): Promise<void> {
    if (ui.showStash?.()) return;
    return exclusive(UNSTASH_FAILED, async () => {
      const started = generation;
      const [state, known] = await Promise.all([api.state(), names()]);
      await ui.ask<UnstashChoice>({
        kind: "unstash",
        root,
        branch: branchText(state),
        names: known,
        ...actions(state.head, started),
      });
    });
  }

  // `Unstash…` of the Stash tab (`GitUnstashAsDialog`).
  function unstashAs(entry: StashEntry): Promise<void> {
    return exclusive(UNSTASH_FAILED, async () => {
      const started = generation;
      const [state, known] = await Promise.all([api.state(), names()]);
      const choice = await ui.ask<UnstashChoice>({
        kind: "unstashAs",
        root: rootName(root),
        // `CurrentBranchComponent.getCurrentBranch`: nothing on a detached HEAD.
        branch: state.branch ?? "",
        entry,
        names: known,
      });
      if (!choice || generation !== started) return;
      await runUnstash(choice, state.head, started);
    });
  }

  // `Pop` and `Apply` of the Stash tab: never with `--index`.
  function unstash(entry: StashEntry, pop: boolean): Promise<void> {
    return exclusive(UNSTASH_FAILED, async () => {
      const started = generation;
      const state = await api.state();
      const choice = { entry, branch: "", pop, reinstateIndex: false };
      await runUnstash(choice, state.head, started);
    });
  }

  // `dropStashWithConfirmation`. `true` if it was dropped.
  async function drop(entry: StashEntry): Promise<boolean> {
    const confirmed = await ui.ask<boolean>({ kind: "stashConfirm", ...dropConfirmation(entry) });
    if (!confirmed) return false;
    try {
      await api.drop(entry.sha);
      return true;
    } catch (error) {
      if (!isCancelled(error)) notify({ kind: "error", title: errorText(error) });
      return false;
    }
  }

  function repositoryChanged(info: Pick<RepoInfo, "path">) {
    root = info.path;
    generation += 1;
    cards.forEach(dismiss);
    cards.clear();
  }

  return { stashChanges, unstashChanges, unstashAs, unstash, drop, repositoryChanged };
}
