import type { LogRef, PathsSelection, RewriteMenu } from "../lib/api";
import type { MenuAction, MenuItem } from "../lib/menu";
import { isProtectedBranch } from "../push/force";
import { shortHash } from "./draw";

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
// `action.description.cant.revert.merge.commit`.
const CANT_REVERT_MERGE = "Reverting merge commits is not allowed";

// `CopyRevisionNumberAction` joins with a space, and `CopyCommitSubjectAction` with a line break.
export const revisionsText = (hashes: string[]) => hashes.join(" ");

export const truncated = (name: string) =>
  name.length > MAX_NAME_LENGTH ? `${name.slice(0, SHORT_NAME_LENGTH)}…` : name;

export const isProtected = ({ name }: LogRef, patterns: string[]) =>
  isProtectedBranch(patterns, name.slice(name.indexOf("/") + 1));

// `GitLogBranchOperationsActionGroup`: those of `LocalBranchActions` and `RemoteBranchActions`, with the
// texts and rules of `branchActions` (`src/branches/model.ts`), without what belongs to phase 4. The current
// branch does not arrive here. Without the HEAD hash at hand, a detached HEAD is named `HEAD`.
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

// `Vcs.Log.ContextMenu` with `Git.Log.ContextMenu`, without what belongs to phase 4. The groups for a
// single commit disappear with several; the loose actions stay disabled.
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
    SEPARATOR,
    cherryPick,
    ...(single ? [checkoutItems(single, repo, vcs)] : []),
    SEPARATOR,
    onCommit("Reset Current Branch to Here…", "reset"),
    revert,
    ...uncommitItem(applied, vcs, handlers.editing),
    SEPARATOR,
    ...editingItems(applied, vcs, handlers.editing),
    // `GitPushUpToCommitAction.isEnabled`.
    onCommit("Push All up to Here…", "pushUpTo", repo.currentBranch !== null),
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
