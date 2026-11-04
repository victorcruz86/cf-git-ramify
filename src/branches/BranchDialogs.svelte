<script lang="ts">
  import { requestCommit } from "../commit/dialogStore";
  import ListChooserDialog from "../commit/ListChooserDialog.svelte";
  import ConflictsDialog from "../conflicts/ConflictsDialog.svelte";
  import { conflictsFlow, type ConflictsRequest } from "../conflicts/flow";
  import FilesDialog from "../git/FilesDialog.svelte";
  import { gitFlow, type GitRequest } from "../git/flow";
  import MergeDialog from "../git/MergeDialog.svelte";
  import RebaseDialog from "../git/RebaseDialog.svelte";
  import {
    applyingFlow,
    isApplying,
    isApplyingPaths,
    type ChangelistRequest,
    type ChosenChangelist,
  } from "../log/applying";
  import CommitMessageDialog from "../log/CommitMessageDialog.svelte";
  import type { VcsAction } from "../log/contextMenu";
  import {
    editingFlow,
    isEditing,
    type CommitMessageRequest,
    type InteractiveRebaseRequest,
  } from "../log/editing";
  import InteractiveRebaseDialog from "../rebase/InteractiveRebaseDialog.svelte";
  import type { KeptChanges, RebasePlan, RepoInfo } from "../lib/api";
  import ConfirmDialog from "../operation/ConfirmDialog.svelte";
  import { operationFlow, type ConfirmRequest } from "../operation/operation";
  import { pushFlow, type PushDialogRequest } from "../push/model";
  import PushDialog from "../push/PushDialog.svelte";
  import PushRejectedDialog from "../push/PushRejectedDialog.svelte";
  import FixTrackedDialog from "../update/FixTrackedDialog.svelte";
  import { updateFlow, type UpdateRequest } from "../update/flow";
  import PullDialog from "../update/PullDialog.svelte";
  import RebaseMergesDialog from "../update/RebaseMergesDialog.svelte";
  import UpdateDialog from "../update/UpdateDialog.svelte";
  import { branchActions, type DialogRequest } from "./actions";
  import CheckoutProblemDialog from "./CheckoutProblemDialog.svelte";
  import CheckoutRefDialog from "./CheckoutRefDialog.svelte";
  import DeleteRemoteDialog from "./DeleteRemoteDialog.svelte";
  import LocalCommitsDialog from "./LocalCommitsDialog.svelte";
  import { isMerging, mergingFlow, type MergingRequest, type Saving } from "./merging";
  import PublishedCommitsDialog from "./PublishedCommitsDialog.svelte";
  import NewBranchDialog from "./NewBranchDialog.svelte";
  import RenameBranchDialog from "./RenameBranchDialog.svelte";
  import ResetDialog from "./ResetDialog.svelte";
  import ResetHeadDialog from "./ResetHeadDialog.svelte";
  import TagDialog from "./TagDialog.svelte";

  type Request =
    | DialogRequest
    | UpdateRequest
    | PushDialogRequest
    | ConfirmRequest
    | ConflictsRequest
    | GitRequest
    | MergingRequest
    | ChangelistRequest
    | CommitMessageRequest
    | InteractiveRebaseRequest;

  interface Props {
    onviewcommits?: (range: [string, string]) => void;
    // `Define remote` of the push dialog. `true` if it was defined.
    ondefineremote?: () => Promise<boolean>;
    // The Commit view, for the cherry-pick or revert that was resolved with the staging area on.
    onshowcommit?: () => void;
  }

  let { onviewcommits, ondefineremote, onshowcommit }: Props = $props();

  // The dialogs of the branch operations, for the log and the branches popup. It does not live inside
  // the log: an operation reloads it, and the dialog that follows must not go away with it.
  let open = $state.raw<{ request: Request; settle: (answer: unknown) => void } | null>(null);
  const request = $derived(open?.request);

  const ask = <T,>(asked: Request) =>
    new Promise<T | null>((resolve) => {
      // A notification can open another one on top: the previous one counts as cancelled.
      open?.settle(null);
      open = { request: asked, settle: (answer) => resolve(answer as T | null) };
    });

  // What `rebase::Process` saved and comes back when the stopped rebase finishes or is aborted.
  const REBASE_SAVING: Saving = { operation: "rebase", from: "HEAD" };
  const conflicts = conflictsFlow({ ask });
  const operation = operationFlow({
    ask,
    resolveConflicts: conflicts.resolveConflicts,
    restoreSaved: (saved) => update.restoreSaved(saved, REBASE_SAVING),
  });
  // What an operation saved and did not come back whole: the Update notices, with `Restore Changes`.
  const restored = (kept: KeptChanges[], saving: Saving) => update.showRestored(kept, saving);
  // Merge… and Rebase…, with the Conflicts dialog and the stopped-rebase card of `src/operation`.
  const git = gitFlow({
    ask,
    resolveConflicts: conflicts.resolveConflicts,
    rebaseStopped: (outcome, saved) => operation.notifyRebaseStopped(outcome, false, saved),
    rebaseConflicts: (saved) => operation.resolveStoppedRebase(saved),
    restored,
    viewCommits: (range) => onviewcommits?.(range),
  });
  const update = updateFlow({
    ask,
    viewCommits: (range) => onviewcommits?.(range),
    abortOperation: operation.abortOperation,
  });
  const pushing = pushFlow({
    ask,
    updates: update.forPush,
    get defineRemote() {
      return ondefineremote;
    },
    viewCommits: (range) => onviewcommits?.(range),
  });
  const actions = branchActions({
    ask,
    rebaseStopped: (outcome, saved) => operation.notifyRebaseStopped(outcome, false, saved),
    rebaseConflicts: (saved) => operation.resolveStoppedRebase(saved),
    restored,
    updateBranch: update.updateBranch,
    pushBranch: pushing.pushBranch,
    pushUpTo: pushing.pushUpTo,
  });
  // Merge, Rebase, Checkout and Rebase and Pull into of the branches popup.
  const merging = mergingFlow({
    ask,
    resolveConflicts: conflicts.resolveConflicts,
    rebaseStopped: (outcome, saved) => operation.notifyRebaseStopped(outcome, false, saved),
    rebaseConflicts: (saved) => operation.resolveStoppedRebase(saved),
    restored,
    restoreSaved: (saved, saving) => update.restoreSaved(saved, saving),
    deleteBranch: (name) => actions.run({ kind: "ref", operation: "delete", ref: { name, kind: "local" } }),
    pullInto: update.pullInto,
  });

  // Cherry-Pick and Revert Commit of the log.
  const applying = applyingFlow({
    chooseChangelist: (lists, initial) =>
      ask<ChosenChangelist>({ kind: "changelist", lists, initial }),
    resolveConflicts: conflicts.resolveConflicts,
    abortOperation: operation.abortOperation,
    showStaging: () => onshowcommit?.(),
  });

  // Edit Commit Message, Drop, Squash, Undo Commit, Fixup…, Squash Into… and Interactively Rebase from
  // Here… of the log, with the same tracking of a stopped rebase.
  const editing = editingFlow({
    editMessage: (asked) => ask<string>(asked),
    interactiveRebase: (commits) => ask<RebasePlan>({ kind: "interactiveRebase", commits }),
    chooseChangelist: (lists, initial) =>
      ask<ChosenChangelist>({ kind: "changelist", lists, initial }),
    // The dialog or the Commit view with the message of Fixup… or Squash Into… and, if offered, `Commit and Rebase…`.
    showCommit: (input) => void requestCommit(input),
    rebaseStopped: (outcome, saved) => operation.notifyRebaseStopped(outcome, false, saved),
    rebaseConflicts: (saved) => operation.resolveStoppedRebase(saved),
    restored,
  });

  function answer(value: unknown) {
    open?.settle(value);
    open = null;
  }

  export const run = (action: VcsAction) => {
    if (isApplying(action) || isApplyingPaths(action)) return applying.run(action);
    if (isEditing(action)) return editing.run(action);
    return isMerging(action) ? merging.run(action) : actions.run(action);
  };
  export const { checkout, checkoutRef, newBranch, tag, resetHead } = actions;
  // The rebase of `Commit and Rebase…`, once the commit is made.
  export const { rebaseAfterCommit } = editing;
  export const { run: runUpdate, fetch, pull } = update;
  export const { push, prepareCommitPush } = pushing;
  export const { abortOperation } = operation;
  // `Git.Merge` and `Git.Rebase`, of the Git menu.
  export const { merge, rebase } = git;
  // The Conflicts dialog: the Git menu, the node link and the notifications go through here.
  export const { resolveConflicts } = conflicts;
  export const continueRebase = () => operation.continueRebase();
  // After `repo-changed`: what was saved for a stopped rebase comes back if that rebase is gone, and
  // the Undo of a rewrite that is no longer valid expires.
  export const repositoryRefreshed = () =>
    Promise.all([operation.repositoryRefreshed(), editing.repositoryRefreshed()]);
  export const skipCommit = () => operation.skipCommit();

  // What is in notifications acts on the open repository: what was there before must not stay.
  export function repositoryChanged(info: RepoInfo) {
    actions.repositoryChanged();
    merging.repositoryChanged();
    applying.repositoryChanged(info);
    editing.repositoryChanged();
    pushing.repositoryChanged();
    update.repositoryChanged(info);
    operation.repositoryChanged(info);
    git.repositoryChanged(info);
    // The Conflicts dialog belonged to the previous repository: it will not receive `merge-done`,
    // because the merge window is destroyed when another one opens. It closes, and what it was
    // waiting for does not continue.
    conflicts.repositoryChanged();
    // The Merge… and Rebase… dialogs belonged to this one: their HEAD and their branches are no longer
    // valid.
    const kind = open?.request.kind;
    const own = ["conflicts", "merge", "rebase", "changelist", "commitMessage", "interactiveRebase"];
    if (kind && own.includes(kind)) {
      answer(null);
    }
  }
</script>

{#if request?.kind === "commitMessage"}
  <CommitMessageDialog {request} onanswer={answer} />
{:else if request?.kind === "interactiveRebase"}
  <InteractiveRebaseDialog commits={request.commits} onanswer={answer} />
{:else if request?.kind === "confirm"}
  <ConfirmDialog {...request} onanswer={answer} />
{:else if request?.kind === "conflicts"}
  <ConflictsDialog session={request.session} onanswer={() => answer(null)} />
{:else if request?.kind === "changelist"}
  <ListChooserDialog
    title="Select Target Changelist"
    lists={request.lists}
    initial={request.initial}
    allowNew
    onconfirm={(name, setActive) => answer({ name, setActive })}
    onclose={() => answer(null)}
  />
{:else if request?.kind === "merge"}
  <MergeDialog {...request} onanswer={answer} />
{:else if request?.kind === "rebase"}
  <RebaseDialog {...request} onanswer={answer} />
{:else if request?.kind === "publishedCommits"}
  <PublishedCommitsDialog onanswer={answer} />
{:else if request?.kind === "files"}
  <FilesDialog {...request} onanswer={() => answer(null)} />
{:else if request?.kind === "newBranch"}
  <NewBranchDialog {...request} onanswer={answer} />
{:else if request?.kind === "rename"}
  <RenameBranchDialog {...request} onanswer={answer} />
{:else if request?.kind === "problem"}
  <CheckoutProblemDialog {...request} onanswer={answer} />
{:else if request?.kind === "localCommits"}
  <LocalCommitsDialog {...request} onanswer={answer} />
{:else if request?.kind === "unmerged"}
  <LocalCommitsDialog {...request} onanswer={(value) => answer(value === "restore")} />
{:else if request?.kind === "deleteRemote"}
  <DeleteRemoteDialog {...request} onanswer={answer} />
{:else if request?.kind === "reset"}
  <ResetDialog {...request} onanswer={answer} />
{:else if request?.kind === "resetHead"}
  <ResetHeadDialog {...request} onanswer={answer} />
{:else if request?.kind === "tag"}
  <TagDialog {...request} onanswer={answer} />
{:else if request?.kind === "checkoutRef"}
  <CheckoutRefDialog onanswer={answer} />
{:else if request?.kind === "updateProject"}
  <UpdateDialog {...request} onanswer={answer} />
{:else if request?.kind === "fixTracked"}
  <FixTrackedDialog {...request} onanswer={answer} />
{:else if request?.kind === "rebaseMerges"}
  <RebaseMergesDialog onanswer={answer} />
{:else if request?.kind === "pull"}
  <PullDialog {...request} onanswer={answer} />
{:else if request?.kind === "push"}
  <PushDialog {...request} onanswer={answer} />
{:else if request?.kind === "pushRejected"}
  <PushRejectedDialog {...request} onanswer={answer} />
{/if}
