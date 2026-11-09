import type { LogRef, PathsSelection, RewriteMenu } from "../lib/api";
import type { MenuAction, MenuItem } from "../lib/menu";
import { isProtectedBranch } from "../push/force";
import { shortHash } from "./draw";
import { revisionNumbersText } from "../lib/revisions";
import { errorText } from "../lib/errors";
import { notify } from "../lib/notifications";
import { showRepositoryAtRevision } from "../repositories/tab";
import {
  compareBranches,
  compareRevisions,
  compareWithLocal,
  showDiffWithWorkingTree,
} from "../compare/actions";

export interface SelectedCommit {
  hash: string;
  subject: string;
  refs: LogRef[];
  // More than one: a merge. Without data, it counts as one.
  parentCount?: number;
}

// What cherry-pick and revert need from each chosen commit.
export interface AppliedCommit {
  hash: string;
  subject: string;
}

// `parents` and `children` are the rows that can be jumped to.
export interface LogSelection {
  commits: SelectedCommit[];
  parents: number[];
  children: number[];
}

export interface RepoState {
  currentBranch: string | null;
}

export type RefOperation =
  | "checkout"
  | "newBranch"
  | "update"
  | "push"
  | "rename"
  | "delete"
  // Merge, Rebase, Checkout and Rebase, and Pull into… (`src/branches/merging.ts`).
  | "checkoutAndRebase"
  | "rebase"
  | "merge"
  | "pullMerge"
  | "pullRebase";

export type VcsAction =
  | { kind: "checkout"; target: string }
  // The commits go in the log's order, from the most recent to the oldest.
  | { kind: "cherryPick"; commits: AppliedCommit[] }
  | { kind: "revert"; commits: AppliedCommit[] }
  // Edit Commit Message, Drop Commits and Squash Commits (`src/log/editing.ts`).
  | { kind: "reword"; commit: AppliedCommit }
  | { kind: "drop"; commits: AppliedCommit[] }
  | { kind: "squash"; commits: AppliedCommit[] }
  // Undo Commit…, Fixup…, Squash Into… and Interactively Rebase from Here… (`src/log/editing.ts`): a
  // single commit.
  | { kind: "uncommit" | "fixup" | "squashInto" | "interactiveRebase"; commit: AppliedCommit }
  // Cherry-Pick Selected Changes (`reverse` false) and Revert Selected Changes, from the changes panel.
  // `hashes` are the commits chosen in the log.
  | { kind: "applyPaths"; reverse: boolean; selection: PathsSelection[]; hashes: string[] }
  | { kind: "reset" | "pushUpTo" | "newBranch" | "newTag"; hash: string }
  // Several branches of the Branches panel at once (`MultipleLocalBranchActions`): Update Selected
  // (`updateBranches`) and the remote half of Delete branches (`deleteRemoteBranches`).
  | { kind: "updateBranches"; names: string[] }
  | { kind: "deleteRemotes"; names: string[] }
  | { kind: "ref"; operation: RefOperation; ref: LogRef };

export interface ContextHandlers {
  copy: (text: string) => void;
  goToChild: () => void;
  goToParent: () => void;
  // The branch operations. Without them those rows come out disabled.
  vcs?: (action: VcsAction) => void;
  // This repository's patterns. Without them nothing is turned off: the core is the one that refuses.
  protectedBranches?: string[];
  // Why the selection cannot be edited (`rewrite_menu`). Without it nothing is turned off: the core
  // checks again on click.
  editing?: RewriteMenu;
  // The paths of the Paths filter (`VcsLogUtil.getAffectedPaths`), which Compare Versions and
  // Compare with Local are limited to.
  paths?: string[];
  // `Create Patch...` of the chosen commits (`VcsLogCreatePatchActionProvider`), given only when
  // they have changes. Without it, it comes out disabled.
  createPatch?: () => void;
}

const SEPARATOR = null;
// `GitLogBranchOperationsActionGroup`.
const MAX_BRANCH_GROUPS = 2;
const MAX_TAG_GROUPS = 1;
// `GitBranchPopupActions`.
const MAX_NAME_LENGTH = 44;
const SHORT_NAME_LENGTH = 39;
// `branches.rebase.is.not.possible.in.the.detached.head.state`.
const DETACHED = "Rebase is not possible in the detached HEAD state";
// `action.not.possible.in.fresh.repo.description` with `action.not.possible.in.fresh.repo.generic`.
const FRESH = "Action is not possible before the first commit.";
// `action.description.cant.revert.merge.commit`.
const CANT_REVERT_MERGE = "Reverting merge commits is not allowed";

// `CopyRevisionNumberAction`, oldest first; `CopyCommitSubjectAction` joins with a line break.
export const revisionsText = revisionNumbersText;

export const truncated = (name: string) =>
  name.length > MAX_NAME_LENGTH ? `${name.slice(0, SHORT_NAME_LENGTH)}…` : name;

export const isProtected = ({ name }: LogRef, patterns: string[]) =>
  isProtectedBranch(patterns, name.slice(name.indexOf("/") + 1));

const failed = (failure: unknown) => notify({ kind: "error", title: errorText(failure) });

// `GitCompareWithBranchAction` (`B:958`, its tooltip `B:956`) and `GitShowDiffWithBranchAction`
// (`B:955`), then the separator before Rebase. `current` names the current branch as the menu
// shows it (quoted and shortened) and `currentName` whole; the range starts at `start`, the current
// branch or `HEAD` (`GitCompareBranchesUi.createRangeFilter`). Without `compare`, the branch is the
// current one (`disabledForCurrent`). Before the first commit both are off
// (`GitSingleBranchAction.update`, `disableActionIfAnyRepositoryIsFresh`, `D:44`, `B:682`).
export function comparisonItems(
  name: string,
  { start, current, currentName, compare, fresh = false }: {
    start: string;
    current: string;
    currentName: string;
    compare: boolean;
    fresh?: boolean;
  },
): MenuItem[] {
  const off = fresh ? { disabled: true, title: FRESH } : {};
  return [
    ...(compare
      ? [
          {
            label: `Compare with ${current}`,
            title: `Show commits in '${name}' that are missing in '${currentName}'`,
            run: () => void compareBranches(name, start).catch(failed),
            ...off,
          },
        ]
      : []),
    {
      label: "Show Diff with Working Tree",
      run: () => void showDiffWithWorkingTree(name).catch(failed),
      ...off,
    },
    SEPARATOR,
  ];
}

// `GitLogBranchOperationsActionGroup`: those of `LocalBranchActions` and `RemoteBranchActions`, with the
// texts and rules of `branchActions` (`src/branches/model.ts`). The current branch does not arrive here. Without the HEAD hash at hand, a detached HEAD is named `HEAD`.
function operations(
  ref: LogRef,
  vcs: ContextHandlers["vcs"],
  currentBranch: string | null,
  protectedBranches: string[] = [],
): MenuItem[] {
  const item = (
    label: string,
    operation: RefOperation,
    disabled = false,
    title?: string,
  ): MenuAction => ({
    label,
    disabled,
    title: disabled ? title : undefined,
    run: () => vcs?.({ kind: "ref", operation, ref }),
  });
  const newBranch = item(`New Branch from '${truncated(ref.name)}'…`, "newBranch");
  if (ref.kind === "tag") return [item("Delete", "delete")];
  const target = `'${truncated(currentBranch ?? "HEAD")}'`;
  const selected = `'${truncated(ref.name)}'`;
  const merging: MenuItem[] = [
    item(`Checkout and Rebase onto ${target}`, "checkoutAndRebase"),
    SEPARATOR,
    ...comparisonItems(ref.name, {
      start: currentBranch ?? "HEAD",
      current: target,
      currentName: currentBranch ?? "HEAD",
      compare: true,
    }),
    item(`Rebase ${target} onto ${selected}`, "rebase", currentBranch === null, DETACHED),
    item(`Merge ${selected} into ${target}`, "merge"),
    SEPARATOR,
  ];
  if (ref.kind === "remote") {
    return [
      item("Checkout", "checkout"),
      newBranch,
      ...merging,
      item(`Pull into ${target} Using Merge`, "pullMerge"),
      item(`Pull into ${target} Using Rebase`, "pullRebase"),
      SEPARATOR,
      item("Delete", "delete", isProtected(ref, protectedBranches)),
    ];
  }
  return [
    item("Checkout", "checkout"),
    newBranch,
    ...merging,
    item("Update", "update"),
    item("Push…", "push"),
    SEPARATOR,
    item("Rename…", "rename"),
    item("Delete", "delete"),
  ];
}

// `GitLogBranchOperationsActionGroup`: a few references go as single rows, otherwise in a submenu.
function refGroup(
  refs: LogRef[],
  title: string,
  single: string,
  limit: number,
  { currentBranch }: RepoState,
  { vcs, protectedBranches }: ContextHandlers,
): MenuItem[] {
  if (refs.length === 0) return [];
  const disabled = !vcs;
  const row = (ref: LogRef, label: string): MenuAction => ({
    label,
    disabled,
    submenu: operations(ref, vcs, currentBranch, protectedBranches),
  });
  if (refs.length <= limit) return refs.map((ref) => row(ref, `${single} '${ref.name}'`));
  return [{ label: title, disabled, submenu: refs.map((ref) => row(ref, ref.name)) }];
}

// `GitCheckoutActionGroup`.
function checkoutItems(
  { hash, refs }: SelectedCommit,
  { currentBranch }: RepoState,
  vcs: ContextHandlers["vcs"],
): MenuAction {
  const disabled = !vcs;
  const checkout = (label: string, target: string): MenuAction => ({
    label,
    disabled,
    run: () => vcs?.({ kind: "checkout", target }),
  });
  const branches = refs.filter(({ kind, name }) => kind === "local" && name !== currentBranch);
  const short = shortHash(hash);
  if (branches.length === 0) return checkout(`Checkout Revision '${short}'`, hash);
  return {
    label: "Checkout",
    disabled,
    submenu: [
      ...branches.map(({ name }) => checkout(name, name)),
      checkout(`Revision '${short}'`, hash),
    ],
  };
}

// `GitUncommitAction`: Undo Commit…, which only exists with a single commit. If a check of
// `GitCommitEditingActionBase.update` fails, or it is not the last one, it is turned off with the reason.
function uncommitItem(
  commits: AppliedCommit[],
  vcs: ContextHandlers["vcs"],
  editing: RewriteMenu | undefined,
): MenuAction[] {
  if (commits.length !== 1) return [];
  const reason = editing?.uncommit ?? null;
  return [
    {
      label: "Undo Commit…",
      disabled: !vcs || reason !== null,
      title: reason ?? undefined,
      run: () => vcs?.({ kind: "uncommit", commit: commits[0] }),
    },
  ];
}

// `GitRewordAction`, `GitCommitFixupBySubjectAction`, `GitCommitSquashBySubjectAction`,
// `GitDropLogAction`, `GitSquashLogAction` and `GitInteractiveRebaseAction`: if a check of
// `GitCommitEditingActionBase.update` fails, the entry is turned off with the reason in the description.
// Edit Commit Message, Fixup… and Squash Into… only exist with a single commit; Squash Commits, with one,
// is only seen turned off.
function editingItems(
  commits: AppliedCommit[],
  vcs: ContextHandlers["vcs"],
  editing: RewriteMenu | undefined,
): MenuAction[] {
  const item = (label: string, reason: string | null, action: VcsAction): MenuAction => ({
    label,
    disabled: !vcs || reason !== null,
    title: reason ?? undefined,
    run: () => vcs?.(action),
  });
  const items: MenuAction[] = [];
  if (commits.length === 1) {
    const [commit] = commits;
    const reword = item("Edit Commit Message…", editing?.reword ?? null, {
      kind: "reword",
      commit,
    });
    items.push({ ...reword, shortcut: "F2" });
    const autosquash = editing?.autosquash ?? null;
    items.push(
      item("Fixup…", autosquash, { kind: "fixup", commit }),
      item("Squash Into…", autosquash, { kind: "squashInto", commit }),
    );
  }
  const drop = editing?.drop ?? null;
  // `rebase.log.drop.action.custom.text` is only set if it passes the checks.
  const dropLabel = drop === null && commits.length === 1 ? "Drop Commit" : "Drop Commits";
  items.push(item(dropLabel, drop, { kind: "drop", commits }));
  const squash = commits.length > 1 ? (editing?.squash ?? null) : drop;
  if (commits.length > 1 || squash !== null) {
    items.push(item("Squash Commits…", squash, { kind: "squash", commits }));
  }
  // `GitInteractiveRebaseAction`: a single commit, like those of `GitSingleCommitEditingAction`.
  if (commits.length === 1) {
    const [commit] = commits;
    const rebase = editing?.rebase ?? null;
    items.push(item("Interactively Rebase from Here…", rebase, { kind: "interactiveRebase", commit }));
  }
  return items;
}

// `Vcs.Log.ContextMenu` (`LX:188-197`) with `Git.Log.ContextMenu`. The groups for a single commit
// disappear with several; the loose actions stay disabled.
export function contextMenu(
  selection: LogSelection,
  repo: RepoState,
  handlers: ContextHandlers,
): MenuItem[] {
  const { commits, parents, children } = selection;
  if (commits.length === 0) return [];
  const { vcs } = handlers;
  const single = commits.length === 1 ? commits[0] : null;
  const onCommit = (
    label: string,
    kind: "reset" | "pushUpTo" | "newBranch" | "newTag",
    enabled = true,
  ): MenuAction => ({
    label,
    disabled: !single || !vcs || !enabled,
    run: () => single && vcs?.({ kind, hash: single.hash }),
  });
  const applied: AppliedCommit[] = commits.map(({ hash, subject }) => ({ hash, subject }));
  const cherryPick: MenuAction = {
    label: "Cherry-Pick",
    disabled: !vcs,
    run: () => vcs?.({ kind: "cherryPick", commits: applied }),
  };
  // `GitRevertAction.update`: `Revert {0,choice,1#Commit|2#Commits}`, and turned off with any merge.
  const merges = commits.some(({ parentCount = 1 }) => parentCount > 1);
  const revert: MenuAction = {
    label: commits.length === 1 ? "Revert Commit" : "Revert Commits",
    disabled: !vcs || merges,
    title: merges ? CANT_REVERT_MERGE : undefined,
    run: () => vcs?.({ kind: "revert", commits: applied }),
  };
  // `GitBrowseRepoAtRevisionAction` (`B:659`): visible with any selection, enabled with one commit.
  const browse: MenuAction = {
    label: "Show Repository at Revision",
    disabled: !single,
    run: () => single && showRepositoryAtRevision(single.hash),
  };
  // `CompareRevisionsFromLogAction` (visible only with two) and
  // `ShowDiffAfterWithLocalFromLogActionProvider` (visible only with one): `showDiffForPaths` lists
  // the Paths filter's paths, or the whole repository, in the `Changes` window, the older commit on
  // the left.
  const affected = (handlers.paths ?? []).map((path) => ({ path, folder: true }));
  const comparisons: MenuAction[] =
    commits.length === 2
      ? [
          {
            label: "Compare Versions",
            run: () =>
              void compareRevisions(commits[1].hash, commits[0].hash, affected).catch(failed),
          },
        ]
      : single
        ? [
            {
              label: "Compare with Local",
              run: () => void compareWithLocal(single.hash, affected).catch(failed),
            },
          ]
        : [];
  const refs = single?.refs ?? [];
  const branches = refs.filter(
    ({ kind, name }) => (kind === "local" && name !== repo.currentBranch) || kind === "remote",
  );
  const tags = refs.filter(({ kind }) => kind === "tag");

  return [
    {
      label: "Copy Revision Number",
      shortcut: "⌥⇧⌘C",
      run: () => handlers.copy(revisionsText(commits.map(({ hash }) => hash))),
    },
    {
      label: "Copy Commit Subject",
      run: () => handlers.copy(commits.map(({ subject }) => subject).join("\n")),
    },
    {
      label: "Create Patch...",
      disabled: !handlers.createPatch,
      run: () => handlers.createPatch?.(),
    },
    // `Dvcs.Log.ContextMenu` (`dvcs.xml:50-53`) goes right after Create Patch..., before the
    // separator of `Vcs.Log.ContextMenu`.
    cherryPick,
    SEPARATOR,
    // `Git.Log.ContextMenu.CheckoutBrowse` (`X:301-306`): Checkout, then Show Repository at Revision,
    // before Compare Versions.
    ...(single ? [checkoutItems(single, repo, vcs)] : []),
    browse,
    ...comparisons,
    SEPARATOR,
    onCommit("Reset Current Branch to Here…", "reset"),
    revert,
    ...uncommitItem(applied, vcs, handlers.editing),
    SEPARATOR,
    ...editingItems(applied, vcs, handlers.editing),
    // `GitPushUpToCommitAction` needs no current branch and no local branch that contains the commit.
    onCommit("Push All up to Here…", "pushUpTo"),
    SEPARATOR,
    ...refGroup(branches, "Branches", "Branch", MAX_BRANCH_GROUPS, repo, handlers),
    ...refGroup(tags, "Tags", "Tag", MAX_TAG_GROUPS, repo, handlers),
    onCommit("New Branch…", "newBranch"),
    onCommit("New Tag…", "newTag"),
    SEPARATOR,
    {
      label: "Go to Child Commit",
      shortcut: "←",
      disabled: children.length === 0,
      run: handlers.goToChild,
    },
    {
      label: "Go to Parent Commit",
      shortcut: "→",
      disabled: parents.length === 0,
      run: handlers.goToParent,
    },
  ];
}
