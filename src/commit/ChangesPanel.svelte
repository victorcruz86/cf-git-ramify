<script lang="ts" module>
  import type { Changelists, FileChange, Hunk } from "../lib/api";

  // What a refresh read. Whoever checks files keeps its checkboxes and chosen lines up to date with it.
  export interface Refreshed {
    status: FileChange[];
    // `null` with the staging area, which does not read the changelists.
    lists: Changelists | null;
    hunks: Map<string, Hunk[]>;
  }

  export type NoticeKind = "info" | "warning" | "error";
</script>

<script lang="ts">
  import { onDestroy, onMount, tick, untrack, type Snippet } from "svelte";
  import {
    changesAdd,
    changelistsCreate,
    changelistsGet,
    changelistsImport,
    changelistsMove,
    changelistsMoveHunks,
    changelistsRemove,
    changelistsRename,
    changelistsSetActive,
    conflictsAccept,
    conflictsSession,
    fileHunks,
    onRepoChanged,
    repoStatus,
    rollbackFiles,
    snapshotsRestore,
    stagingRevert,
    type ApiError,
    type ChangeList,
    type Comparison,
    type Conflict,
    type ConflictSession,
    type ConflictSide,
    type RepoInfo,
    type StageComparison,
  } from "../lib/api";
  import { errorText } from "../lib/errors";
  import Icon from "../lib/Icon.svelte";
  import type { ActionId } from "../lib/keymap";
  import { matchShortcut } from "../lib/keymap";
  import Menu, { type MenuItem } from "../lib/Menu.svelte";
  import { notify as showNotification } from "../lib/notifications";
  import { store, stored } from "../lib/storage";
  import { mergeFiles } from "../conflicts/flow";
  import { localEntries, patches, unreadablePatchText } from "../patch/flow";
  import { shelves, silentGroups, unreadableShelfText } from "../shelf/flow";
  import { taskResult } from "../lib/tasks";
  import ChangelistDialog from "./ChangelistDialog.svelte";
  import { findList } from "./changelists";
  import ChangesTree from "./ChangesTree.svelte";
  import { checkinFilesText } from "./checkin";
  import ConfirmDialog from "./ConfirmDialog.svelte";
  import ListChooserDialog from "./ListChooserDialog.svelte";
  import { requestExecutor } from "./dialogStore";
  import { latestOnly } from "./refresh";
  import { listSelections, liveLists, rollbackTargets } from "./request";
  import RollbackDialog from "./RollbackDialog.svelte";
  import { ADD_FAILED, ADD_PROBLEMS, STAGE_COPY_SAVED } from "../files/addFiles";
  import SnapshotsDialog from "./SnapshotsDialog.svelte";
  import {
    comparisonOf,
    hasThreeVersions,
    groupStaging,
    revertPaths,
    stageAllTargets,
    stagingTargets,
    stagingTree,
  } from "./staging";
  import { dropPlan, type DragLoad } from "./drag";
  import StagingView from "./StagingView.svelte";
  import {
    buildTree,
    collectFiles,
    flattenTree,
    listTree,
    UNVERSIONED_ID,
    pathOfKey,
    scopeOfKey,
    withoutCheck,
    type TreeNode,
  } from "./tree";

  const NOTICE_MS = 10_000;
  const GROUP_KEY = "commit.groupByDirectory";
  const NO_PATHS = new Set<string>();
  // `error.adding.files.notification.title` and `error.adding.files.prompt`.

  interface Props {
    repo: RepoInfo;
    active: boolean;
    // With checkboxes, as in the Commit view, or without them, as in Local Changes in modal mode.
    checkable: boolean;
    // `ChangesViewManager.configureToolbars`: horizontal only in the Commit tool window.
    toolbar: "horizontal" | "vertical";
    // With the staging area, the Staged and Unstaged groups instead of the changelists, which are left alone.
    stagingArea?: boolean;
    // What the last refresh read, for whoever owns the panel.
    changes?: FileChange[];
    lists?: Changelists | null;
    // With checkboxes: what is checked, and the files of which only a part goes in.
    included?: Set<string>;
    partial?: Set<string>;
    ontoggle?: (nodes: TreeNode[]) => void;
    // The files whose hunks each refresh reads, besides the ones split across lists.
    hunksOf?: () => Iterable<string>;
    onrefresh?: (refreshed: Refreshed) => void;
    // Double click and Enter. Without a viewer for the source they open the diff (decision 3).
    onopen: (file: FileChange, comparison?: StageComparison) => void;
    onshowdiff: (file: FileChange, comparison?: StageComparison) => void;
    // The selected files and changelists in the order of the tree, and the file under the cursor.
    onselection?: (paths: string[], lists: string[], lead: string | null) => void;
    // `Commit File…`: not offered without it.
    oncheckinfiles?: (paths: string[], lists: string[]) => void;
    // `Add to VCS` took these files to the index and to the panel's list: the dialog includes them.
    onadded?: (paths: string[]) => void;
    // Rolled back on purpose: what was chosen in these files has not been lost.
    onrolledback?: (paths: string[]) => void;
    // The `Resolve` link of the conflicts node: opens the Conflicts dialog.
    onresolveconflicts?: () => void;
    // Whether any file is in conflict: enables `Resolve Conflicts…` of the Git menu.
    onconflictschange?: (conflicts: boolean) => void;
    // A dialog or a menu of the owner is open: the shortcuts of the tree wait.
    suspended?: boolean;
    // Toolbar items of the owner: right after `Refresh` (`Commit…`) and at the end (`Preview Diff`).
    actions?: Snippet;
    trailing?: Snippet;
    // The browser of the Commit Changes dialog (`MultipleLocalChangeListsBrowser`): only this list's
    // files, without changelist nodes, with its own toolbar and menu, and its shortcuts only while the
    // focus is inside it. Empty until the dialog has chosen the list. `unversioned` says whether
    // `Unversioned Files` is shown.
    list?: string;
    unversioned?: boolean;
  }

  let {
    repo,
    active,
    checkable,
    toolbar,
    stagingArea = false,
    changes = $bindable([]),
    lists = $bindable(null),
    included = NO_PATHS,
    partial = NO_PATHS,
    ontoggle,
    hunksOf,
    onrefresh,
    onopen,
    onshowdiff,
    onselection,
    oncheckinfiles,
    onadded,
    onrolledback,
    onresolveconflicts,
    onconflictschange,
    suspended = false,
    actions,
    trailing,
    list,
    unversioned = true,
  }: Props = $props();

  type Dialogs =
    | { kind: "new" }
    | { kind: "edit"; list: ChangeList }
    | { kind: "move"; paths: string[] }
    | { kind: "confirm-remove"; list: ChangeList }
    | { kind: "remove-empty"; list: ChangeList }
    | { kind: "remove-active"; list: ChangeList }
    | { kind: "rollback"; files: FileChange[]; checked: string[] }
    | { kind: "snapshots" };

  interface Notice {
    id: number;
    kind: NoticeKind;
    text: string;
    action?: { label: string; run: () => void };
  }

  interface PopupMenu {
    x: number;
    y: number;
    label: string;
    items: MenuItem[];
  }

  let groupByDirectory = $state(stored(GROUP_KEY) !== "false");
  let collapsed = $state.raw(new Set([UNVERSIONED_ID]));
  let selected = $state.raw(new Set<string>());
  let lead = $state<string | null>(null);

  // The conflicts with the state of each side, to know which ones can be merged.
  let conflictInfo = $state.raw(new Map<string, Conflict>());
  let conflictKey = "";

  let dialog = $state.raw<Dialogs | null>(null);
  let menu = $state.raw<PopupMenu | null>(null);
  let notices = $state.raw<Notice[]>([]);
  let nextNotice = 0;
  let recoveryDismissed = $state(false);
  // The hunks the last refresh read: those of the files split across lists, for Copy as Patch.
  let hunksRead = $state.raw(new Map<string, Hunk[]>());
  let tree = $state<ChangesTree>();
  let staging = $state<StagingView>();
  let destroyed = false;
  const timers = new Set<ReturnType<typeof setTimeout>>();

  const groups = $derived(groupStaging(changes));
  const browser = $derived(list !== undefined);
  const roots = $derived.by(() => {
    if (stagingArea) return stagingTree(groups, groupByDirectory);
    if (!lists) return [];
    if (list !== undefined) {
      return list ? listTree(changes, lists, list, groupByDirectory, unversioned) : [];
    }
    return buildTree(changes, lists, groupByDirectory);
  });
  const shownRoots = $derived(checkable ? roots : roots.map(withoutCheck));
  const activeRoot = $derived(roots.find((node) => node.active));
  // In the order of the tree.
  const nodes = $derived(
    new Map(flattenTree(roots, new Set()).map((row) => [row.node.id, row.node])),
  );
  const selectedNodes = $derived([...nodes.values()].filter((node) => selected.has(node.id)));
  const selectedFiles = $derived(collectFiles(selectedNodes));
  const selectedVersioned = $derived(
    selectedFiles.filter((file) => file.state !== "untracked" && file.state !== "ignored"),
  );
  const selectedUnversioned = $derived(selectedFiles.filter((file) => file.state === "untracked"));
  const selectedConflicts = $derived(
    selectedFiles.filter((file) => file.state === "unmerged").map((file) => file.path),
  );
  const selectedLists = $derived(
    selectedNodes.flatMap((node) => (node.list ? [node.list] : [])),
  );
  const singleList = $derived(selectedLists.length === 1 ? selectedLists[0] : null);
  const hasVersioned = $derived(changes.some((change) => change.state !== "untracked"));
  const selectedKeys = $derived(selectedNodes.flatMap((node) => node.paths));
  const toRevert = $derived(revertPaths(selectedKeys));
  const toUnstage = $derived(stagingTargets(selectedKeys, changes).unstage);
  const recoveryLines = $derived(recoveryText());

  export function activePaths(): string[] {
    return activeRoot?.paths ?? [];
  }

  // Every key with a checkbox, in the order of the tree.
  export function shownKeys(): string[] {
    return roots.filter((node) => node.checkable !== false).flatMap((node) => node.paths);
  }

  export function unversionedPaths(): string[] {
    return nodes.get(UNVERSIONED_ID)?.paths ?? [];
  }

  // The lists other than the active one that have any of these files.
  export function inactiveListsWith(keys: Set<string>): ChangeList[] {
    return roots.flatMap((node) =>
      node.list && !node.active && node.paths.some((path) => keys.has(path)) ? [node.list] : [],
    );
  }

  export function selectedChanges(): { paths: string[]; lists: string[] } {
    return {
      paths: selectedFiles.map((file) => file.path),
      lists: selectedLists.map((list) => list.id),
    };
  }

  // `ChangesView.Revert` from the Git menu: the Rollback of the selection, or of the active list.
  // From the Git file menu, of the changes at or under its paths (`filterChangesUnderFiles`).
  export function requestRollback(paths?: string[]) {
    rollbackOf(paths);
  }

  export function focusTree() {
    (stagingArea ? staging : tree)?.focus();
  }

  export function refresh(): Promise<void> {
    return reload();
  }

  export function hasPopup(): boolean {
    return dialog !== null || menu !== null;
  }

  function recoveryText(): string[] {
    const report = repo.recovery;
    if (!report || recoveryDismissed || browser) return [];
    const lines: string[] = [];
    if (report.pending) {
      lines.push(
        "There is another operation in progress. If it does not finish, reopen the repository to finish recovering the stage.",
      );
    }
    if (report.error) lines.push(`Could not finish recovering the stage: ${report.error}`);
    if (report.skipped.length > 0) {
      lines.push(
        `These paths have not returned to the stage because they changed afterwards: ${report.skipped.join("; ")}`,
      );
    }
    return lines;
  }

  export function notify(kind: NoticeKind, text: string, action?: Notice["action"]) {
    // A notice with an action takes precedence over an identical one: its action points to the latest thing that happened.
    if (!action && notices.some((notice) => notice.text === text)) return;
    const id = nextNotice++;
    notices = [...notices.filter((notice) => notice.text !== text), { id, kind, text, action }];
    if (kind === "info" && !action) {
      const timer = setTimeout(() => {
        timers.delete(timer);
        dismiss(id);
      }, NOTICE_MS);
      timers.add(timer);
    }
  }

  function dismiss(id: number) {
    notices = notices.filter((notice) => notice.id !== id);
  }

  // A file that cannot be read right now is left out: nothing is decided without knowing its hunks.
  async function readHunks(paths: string[]): Promise<Map<string, Hunk[]>> {
    const read = await Promise.all(
      paths.map((path) =>
        fileHunks(path).then(
          (hunks) => [path, hunks] as const,
          () => null,
        ),
      ),
    );
    return new Map(read.filter((entry) => entry !== null));
  }

  // With the staging area the changelists are not read: reading them would bring them up to date with another usage mode.
  const reload = latestOnly(
    async () => {
      const status = await repoStatus(false);
      if (stagingArea) return { status, assigned: null, hunks: new Map<string, Hunk[]>() };
      const paths = status.map((change) => change.path);
      const assigned = await changelistsGet(paths);
      const wanted = new Set([...(hunksOf?.() ?? []), ...Object.keys(assigned.hunkAssignments)]);
      const hunks = await readHunks(paths.filter((path) => wanted.has(path)));
      return { status, assigned, hunks };
    },
    ({ status, assigned, hunks }) => {
      changes = status;
      hunksRead = hunks;
      void refreshMergeInfo(status);
      if (assigned) lists = liveLists(assigned, hunks);
      onrefresh?.({ status, lists: assigned ? lists : null, hunks });
    },
    (failure) => notify("error", errorText(failure)),
  );

  // Which sides each conflict has: it is read again only when the set of conflicts changes.
  async function refreshMergeInfo(status: FileChange[]) {
    const unmerged = status.filter((file) => file.state === "unmerged").map((file) => file.path);
    const key = unmerged.sort().join("\0");
    if (key === conflictKey) return;
    conflictKey = key;
    if (unmerged.length === 0) {
      conflictInfo = new Map();
      return;
    }
    try {
      const session = await taskResult<ConflictSession>(await conflictsSession());
      if (!destroyed && conflictKey === key) {
        conflictInfo = new Map(session.conflicts.map((entry) => [entry.path, entry]));
      }
    } catch {
      // Without that data Merge… stays enabled and the merge window will say what there is.
      if (conflictKey === key) conflictInfo = new Map();
    }
  }

  function firstSelectedFile(): FileChange | undefined {
    const node = lead ? nodes.get(lead) : undefined;
    return node?.change ?? selectedFiles[0];
  }

  function comparisonFor(node: TreeNode | undefined): Comparison | undefined {
    return stagingArea && node ? comparisonOf(node) : undefined;
  }

  function showDiff() {
    const file = firstSelectedFile();
    const node = lead ? nodes.get(lead) : selectedNodes[0];
    if (file) onshowdiff(file, comparisonFor(node));
  }

  function openNode(node: TreeNode) {
    if (node.change) onopen(node.change, comparisonFor(node));
  }

  function checkinFiles() {
    const { paths, lists: chosen } = selectedChanges();
    oncheckinfiles?.(paths, chosen);
  }

  async function run(operation: () => Promise<unknown>) {
    try {
      await operation();
      await reload();
    } catch (failure) {
      notify("error", errorText(failure));
    }
  }

  // Also an event handler: whatever it is given is not a list of paths.
  function openRollback() {
    rollbackOf();
  }

  function rollbackOf(paths?: string[]) {
    const under = (path: string) =>
      paths?.some((root) => root === "" || path === root || path.startsWith(`${root}/`));
    const chosen = paths
      ? changes.filter((change) => under(change.path)).map((change) => change.path)
      : selectedFiles.map((file) => file.path);
    if (stagingArea) {
      // `ChangesView.Revert` with files: the Rollback Changes dialog, back to HEAD, whatever the
      // staging tree has selected (G§0.1: without paths it is `Git.Stage.Revert`).
      if (paths) openStagingRollback(chosen);
      else revertOrUnstage();
      return;
    }
    if (!lists || !hasVersioned) return;
    dialog = { kind: "rollback", ...rollbackTargets(changes, lists, chosen) };
  }

  // With the staging area there are no lists: every change, the chosen ones checked, or all of
  // them when none was chosen (`RollbackChangesDialog.rollbackChanges(project)`).
  function openStagingRollback(chosen: string[]) {
    const files = changes.filter((change) => change.state !== "untracked" && change.state !== "ignored");
    if (files.length === 0) return;
    const checked = chosen.length > 0 ? chosen : files.map((file) => file.path);
    dialog = { kind: "rollback", files, checked };
  }

  // `Git.Stage.Revert`: no dialog, like IntelliJ, because a snapshot is always left.
  async function revert(paths: string[]) {
    const copy = await stagingRevert(paths);
    if (!copy) return;
    const files = paths.length === 1 ? "1 file" : `${paths.length} files`;
    notify("info", `Rollback of ${files}. A snapshot was saved.`, {
      label: "Restore",
      run: () => restore(copy),
    });
  }

  // `Git.Stage.Revert` and `Git.Stage.Reset` share a shortcut: each one applies to its own group.
  // Revert goes first, because after taking something out of the index it could no longer be
  // reverted.
  async function revertOrUnstage() {
    const [paths, unstage] = [toRevert, toUnstage];
    try {
      if (paths.length > 0) await revert(paths);
    } catch (failure) {
      notify("error", errorText(failure));
      await reload();
      return;
    }
    if (unstage.length > 0) await staging?.apply({ stage: [], unstage });
    else if (paths.length > 0) await reload();
  }

  async function revertOnly() {
    await run(() => revert(toRevert));
  }

  async function rollback(paths: string[], deleteAdded: boolean) {
    dialog = null;
    try {
      const snapshot = await rollbackFiles(paths, deleteAdded);
      onrolledback?.(paths);
      const files = paths.length === 1 ? "1 file" : `${paths.length} files`;
      notify("info", `Rollback of ${files}. A snapshot was saved.`, {
        label: "Restore",
        run: () => restore(snapshot),
      });
      await reload();
    } catch (failure) {
      notify("error", errorText(failure));
    }
  }

  async function restore(id: string) {
    dialog = null;
    await run(async () => {
      const previous = await snapshotsRestore(id, repo.path);
      notify("info", "Snapshot restored.", {
        label: "Undo",
        run: () => restore(previous),
      });
    });
  }

  function openNew() {
    dialog = { kind: "new" };
  }

  function openEdit() {
    if (singleList) dialog = { kind: "edit", list: singleList };
  }

  function openMove() {
    if (!stagingArea && lists && selectedVersioned.length > 0) {
      dialog = { kind: "move", paths: selectedVersioned.map((file) => file.path) };
    }
  }

  function canRemove(list: ChangeList | null): boolean {
    return list !== null && lists !== null && lists.lists.length > 1;
  }

  function removeList() {
    const list = singleList;
    if (!list || !lists || !canRemove(list)) return;
    if (list.id === lists.active) {
      dialog = { kind: "remove-active", list };
    } else if ((nodes.get(`list:${list.id}`)?.paths.length ?? 0) > 0) {
      dialog = { kind: "confirm-remove", list };
    } else {
      run(() => changelistsRemove(list.id));
    }
  }

  async function removeConfirmed(list: ChangeList) {
    dialog = null;
    await run(() => changelistsRemove(list.id));
  }

  // After a commit: a list other than the active one that was left empty is offered for removal.
  export function offerToRemoveEmpty(touched: ChangeList[]) {
    const empty = touched.find((list) => nodes.get(`list:${list.id}`)?.paths.length === 0);
    if (empty && !dialog) dialog = { kind: "remove-empty", list: empty };
  }

  // After Shelve Changes (`scheduleAutomaticEmptyChangeListDeletion(list, silently = true)`): the
  // list is read again and, if it is not the active one and was left empty, deleted without asking.
  export async function removeIfEmpty(id: string) {
    await refresh();
    if (!lists || id === lists.active) return;
    if (nodes.get(`list:${id}`)?.paths.length !== 0) return;
    await run(() => changelistsRemove(id));
  }

  function setActive() {
    const list = singleList;
    if (list && list.id !== lists?.active) run(() => changelistsSetActive(list.id));
  }

  async function createList(name: string, makeActive: boolean) {
    dialog = null;
    await run(async () => {
      const created = await changelistsCreate(name);
      const list = findList(created, name);
      if (makeActive && list) await changelistsSetActive(list.id);
    });
  }

  async function editList(list: ChangeList, name: string, makeActive: boolean) {
    dialog = null;
    await run(async () => {
      if (name !== list.name) await changelistsRename(list.id, name);
      if (makeActive) await changelistsSetActive(list.id);
    });
  }

  async function moveTo(paths: string[], name: string, makeActive: boolean) {
    dialog = null;
    const current = lists;
    if (!current) return;
    await run(async () => {
      let target = findList(current, name);
      if (!target) target = findList(await changelistsCreate(name), name);
      if (!target) return;
      await changelistsMove(paths, target.id);
      if (makeActive) await changelistsSetActive(target.id);
    });
  }

  async function removeActive(list: ChangeList, name: string) {
    dialog = null;
    const next = lists && findList(lists, name);
    if (!next) return;
    await run(async () => {
      await changelistsSetActive(next.id);
      await changelistsRemove(list.id);
    });
  }

  function importLists() {
    run(changelistsImport);
  }

  function toggleGrouping() {
    groupByDirectory = !groupByDirectory;
    store(GROUP_KEY, String(groupByDirectory));
  }

  function expandAll() {
    collapsed = new Set();
  }

  function collapseAll() {
    collapsed = new Set(roots.flatMap((node) => [node.id, ...groupIds(node)]));
  }

  function groupIds(node: TreeNode): string[] {
    return node.children.flatMap((child) =>
      child.children.length > 0 ? [child.id, ...groupIds(child)] : [],
    );
  }

  function changelistActions(): MenuItem[] {
    return [
      { label: "New Changelist...", run: openNew },
      { label: "Edit Changelist...", shortcut: "F2", disabled: !singleList, run: openEdit },
      { label: "Delete Changelist", disabled: !canRemove(singleList), run: removeList },
      {
        label: "Set Active Changelist",
        shortcut: "⌃Space",
        disabled: !singleList || singleList.id === lists?.active,
        run: setActive,
      },
      {
        label: "Move to Another Changelist...",
        shortcut: "⇧⌘M",
        disabled: selectedVersioned.length === 0,
        run: openMove,
      },
    ];
  }

  function openToolbarMenu(event: MouseEvent, label: string, items: MenuItem[]) {
    const bounds = (event.currentTarget as HTMLElement).getBoundingClientRect();
    menu =
      toolbar === "vertical"
        ? { x: bounds.right + 2, y: bounds.top, label, items }
        : { x: bounds.left, y: bounds.bottom + 2, label, items };
  }

  function openChangelistsMenu(event: MouseEvent) {
    openToolbarMenu(event, "Changelists", changelistActions());
  }

  // `ChangesViewToolWindowPanel`: `Group By` ▸, the only option Ramify has.
  function openViewOptions(event: MouseEvent) {
    openToolbarMenu(event, "View Options", [
      {
        label: "Group By",
        submenu: [
          { header: "Group By" },
          { label: "Directory", shortcut: "⌃P", checked: groupByDirectory, run: toggleGrouping },
        ],
      },
    ]);
  }

  function stagingCopy(snapshot: string) {
    notify("info", STAGE_COPY_SAVED, {
      label: "Restore",
      run: () => restore(snapshot),
    });
  }

  // `ChangesView.AddUnversioned` (`ScheduleForAdditionAction`): the unversioned files of the selection go
  // to the index and then to the active list or, in the dialog's browser, to the dialog's list, which
  // includes them in the commit (`moveAddedChangesTo`). Ignored files are not listed, so the
  // confirmation for them never applies.
  // `ScheduleForAdditionAction`: the selection goes to the dialog's list or the active one; a drop
  // passes its files and the list it landed on.
  async function addToVcs(
    paths = selectedUnversioned.map((file) => file.path),
    target = browser ? list : lists?.active,
  ) {
    if (paths.length === 0 || !target) return;
    try {
      const copy = await taskResult<string | null>(await changesAdd(repo.path, paths));
      if (copy) stagingCopy(copy);
    } catch (failure) {
      if ((failure as ApiError).kind !== "cancelled") {
        showNotification({
          kind: "error",
          title: ADD_FAILED,
          body: `${ADD_PROBLEMS}\n${errorText(failure)}`,
        });
      }
      await reload();
      return;
    }
    await run(() => changelistsMove(paths, target));
    const added = new Set(
      changes.filter((change) => change.state !== "untracked").map((change) => change.path),
    );
    onadded?.(paths.filter((path) => added.has(path)));
  }

  // `ChangesBrowserChangeListNode.acceptDrop`: the changes move first, then the unversioned files
  // are added to Git in that list. The files are read again: Devin may write during the drag.
  async function dropOnList(target: TreeNode, load: DragLoad) {
    if (!target.list || !lists) return;
    const plan = dropPlan(load, target.list.id, changes, lists);
    if (!plan) return;
    if (plan.versioned.length > 0 || plan.hunks.length > 0) {
      await run(async () => {
        if (plan.versioned.length > 0) await changelistsMove(plan.versioned, plan.list);
        for (const { path, ids } of plan.hunks) await changelistsMoveHunks(path, ids, plan.list);
      });
    }
    if (plan.unversioned.length > 0) await addToVcs(plan.unversioned, plan.list);
  }

  function stageSelected() {
    const { stage } = stagingTargets(selectedKeys, changes);
    if (stage.length > 0) staging?.apply({ stage, unstage: [] });
  }

  // `GitMergeConflictAction.isEnabled`: without both sides deleted there is nothing to merge.
  function canMerge(path: string): boolean {
    const entry = conflictInfo.get(path);
    return !(entry?.yours === "deleted" && entry.theirs === "deleted");
  }

  async function acceptConflicts(paths: string[], side: ConflictSide) {
    try {
      await taskResult<void>(await conflictsAccept(paths, side));
    } catch (failure) {
      if ((failure as ApiError).kind !== "cancelled") notify("error", errorText(failure));
    }
    await reload();
  }

  async function mergeConflicts(paths: string[]) {
    try {
      await mergeFiles(paths.filter(canMerge));
    } catch (failure) {
      notify("error", errorText(failure));
    }
  }

  // `Git.ChangesView.Conflicts` and the start of `Git.Stage.Tree.Menu`: only with conflicts.
  function conflictActions(): MenuItem[] {
    const paths = selectedConflicts;
    if (paths.length === 0) return [];
    return [
      {
        label: "Merge…",
        disabled: !paths.some(canMerge),
        run: () => mergeConflicts(paths),
      },
      { label: "Accept Theirs", run: () => acceptConflicts(paths, "theirs") },
      { label: "Accept Yours", run: () => acceptConflicts(paths, "yours") },
      null,
    ];
  }

  function openStagingMenu(x: number, y: number) {
    const targets = stagingTargets(selectedKeys, changes);
    const patchable = selectedKeys.some((key) =>
      ["staged", "unstaged", "untracked"].includes(scopeOfKey(key)),
    );
    const stashed = [...new Set([...targets.stage, ...targets.unstage])];
    const file = firstSelectedFile();
    const three = file && hasThreeVersions(file) ? file : null;
    menu = {
      x,
      y,
      label: "Changes",
      items: [
        ...conflictActions(),
        {
          label: "Stage",
          shortcut: "⌥⌘A",
          disabled: targets.stage.length === 0,
          run: () => staging?.apply({ stage: targets.stage, unstage: [] }),
        },
        {
          label: "Unstage",
          shortcut: "⌥⌘Z",
          disabled: targets.unstage.length === 0,
          run: () => staging?.apply({ stage: [], unstage: targets.unstage }),
        },
        // `Git.Stage.Stash.Files`: what is staged, unstaged or untracked, not the conflicts.
        {
          label: "Stash Files",
          disabled: stashed.length === 0,
          run: () => {
            const otherStaged = groups.staged.some(({ path }) => !stashed.includes(path));
            void staging?.stashFiles(repo.path, stashed, otherStaged);
          },
        },
        { label: "Rollback", shortcut: "⌥⌘Z", disabled: toRevert.length === 0, run: revertOnly },
        {
          label: "Show Diff",
          shortcut: "⌘D",
          disabled: selectedFiles.length === 0,
          run: showDiff,
        },
        // `Git.Stage.ThreeSideDiff`, only if the three versions exist.
        {
          label: "Compare HEAD, Staged and Local Versions",
          disabled: !three,
          run: () => three && onshowdiff(three, "three"),
        },
        // `GitStageCreatePatchActionProvider`: visible only when enabled, with anything staged,
        // unstaged or untracked chosen.
        ...(patchable
          ? [
              { label: "Create Patch...", run: () => staging?.createPatch(repo.path, selectedKeys) },
              {
                label: "Copy as Patch to Clipboard",
                run: () => staging?.copyAsPatch(repo.path, selectedKeys),
              },
            ]
          : []),
        null,
        { label: "Restore Snapshot…", run: () => (dialog = { kind: "snapshots" }) },
        null,
        { label: "Refresh", run: reload },
      ],
    };
  }

  // `ChangesView.GroupBy`, the toolbar popup of the dialog's browser.
  function openGroupBy(event: MouseEvent) {
    openToolbarMenu(event, "Group By", [
      { label: "Directory", shortcut: "⌃P", checked: groupByDirectory, run: toggleGrouping },
    ]);
  }

  // `MultipleLocalChangeListsBrowser.createPopupMenuActions`, without what Ramify does not have: `Show
  // Diff in a New Tab or Window` (the diff is always a window) and `Jump to Source`. Of the unversioned
  // group (`Unversioned.Files.Dialog.Popup`) there is only `Add to VCS`, shown even when disabled
  // because the place is not `ChangesViewPopupMenu`; `Move to Another Changelist` has only its shortcut.
  function openBrowserMenu(x: number, y: number) {
    const items: MenuItem[] = conflictActions();
    items.push(
      {
        label: "Show Diff",
        shortcut: "⌘D",
        disabled: selectedFiles.length === 0,
        run: showDiff,
      },
      { label: "Refresh", run: reload },
    );
    if (unversioned) {
      items.push({
        label: "Add to VCS",
        shortcut: "⌥⌘A",
        disabled: selectedUnversioned.length === 0,
        run: () => addToVcs(),
      });
    }
    if (!unversioned && selectedVersioned.length > 0) {
      items.push({ label: "Move to Another Changelist...", shortcut: "⇧⌘M", run: openMove });
    }
    items.push({
      label: "Rollback…",
      shortcut: "⌥⌘Z",
      disabled: !hasVersioned,
      run: openRollback,
    });
    menu = { x, y, label: "Changes", items };
  }

  // `ChangesViewPopupMenu`, without what Ramify does not have yet.
  function openContextMenu(x: number, y: number) {
    if (browser) {
      openBrowserMenu(x, y);
      return;
    }
    if (stagingArea) {
      openStagingMenu(x, y);
      return;
    }
    const count = selectedFiles.length;
    const items: MenuItem[] = conflictActions();
    // The status lists every file (`--untracked-files=all`) and folders are helper nodes, as in
    // `DirectoryChangesGroupingPolicy`: the roots are always files.
    if (count > 0 && oncheckinfiles) {
      const roots = selectedFiles.map((file) => ({ path: file.path, directory: false }));
      items.push({ label: checkinFilesText(roots), run: checkinFiles });
    }
    items.push({
      label: "Rollback…",
      shortcut: "⌥⌘Z",
      disabled: !hasVersioned,
      run: openRollback,
    });
    if (selectedVersioned.length > 0) {
      items.push({
        label: "Move to Another Changelist...",
        shortcut: "⇧⌘M",
        run: openMove,
      });
    }
    if (count > 0) items.push({ label: "Show Diff", shortcut: "⌘D", run: showDiff });
    // `ChangesView.AddUnversioned` is visible here only when enabled; `$Delete` and `Remove from VCS`,
    // the rest of its group, Ramify does not have.
    if (selectedUnversioned.length > 0) {
      items.push(null, { label: "Add to VCS", shortcut: "⌥⌘A", run: () => addToVcs() });
    }
    items.push(null, { label: "New Changelist...", run: openNew });
    if (canRemove(singleList)) items.push({ label: "Delete Changelist", run: removeList });
    if (singleList && singleList.id !== lists?.active) {
      items.push({ label: "Set Active Changelist", shortcut: "⌃Space", run: setActive });
    }
    if (singleList) items.push({ label: "Edit Changelist...", shortcut: "F2", run: openEdit });
    items.push(
      {
        label: "Create Patch from Local Changes...",
        disabled: !canCreatePatch(),
        run: () => requestExecutor("patch", { kind: "project", ...selectedChanges() }),
      },
      { label: "Copy as Patch to Clipboard", disabled: !canCopyPatch(), run: copyAsPatch },
      // `ChangesView.Shelve`, an executor action like Create Patch, with its enabled rule.
      {
        label: "Shelve Changes...",
        disabled: !canCreatePatch(),
        run: () => requestExecutor("shelve", { kind: "project", ...selectedChanges() }),
      },
      { label: "Import Changelists from .idea", run: importLists },
      null,
      { label: "Restore Snapshot…", run: () => (dialog = { kind: "snapshots" }) },
      null,
      { label: "Refresh", run: reload },
    );
    menu = { x, y, label: "Changes", items };
  }

  // `CreatePatchAction` (`AbstractCommitChangesAction.update` in the popup): one list that has
  // changes, or else some changes chosen.
  function canCreatePatch(): boolean {
    if (selectedLists.length > 0) {
      const node = roots.find((root) => root.list?.id === selectedLists[0].id);
      return selectedLists.length === 1 && (node?.paths.length ?? 0) > 0;
    }
    return selectedVersioned.length > 0;
  }

  // `CreatePatchFromChangesAction.defaultUpdate`: changes or unversioned files, and one list at most.
  function canCopyPatch(): boolean {
    return selectedFiles.length > 0 && selectedLists.length <= 1;
  }

  // `CreatePatchFromChangesAction.Clipboard`: what each chosen row has of its file (the hunks of
  // its list for a file split across lists), with the comment of the chosen list as the message.
  async function copyAsPatch() {
    if (!lists) return;
    const chosen = listSelections(new Set(selectedKeys), lists, hunksRead, new Map());
    if (chosen.unreadable.length > 0) {
      notify("error", unreadablePatchText(chosen.unreadable));
      return;
    }
    const files = selectedFiles.filter((file) => chosen.included.has(file.path));
    const entries = localEntries(files, chosen.selections);
    await patches.copyAsPatch({ repo: repo.path, entries }, singleList?.comment ?? null);
  }

  // `ShelveSilentlyActionBase.update`: some changes chosen (unversioned files are not changes), with
  // changelists, so not with the staging area.
  function canShelveSilently(): boolean {
    return !stagingArea && !browser && selectedVersioned.length > 0;
  }

  // `ChangesView.ShelveSilently` and `ChangesView.SaveToShelve` (no rollback): one shelf per
  // changelist of the chosen changes.
  async function shelveSilently(rollback: boolean) {
    if (!lists || !canShelveSilently()) return;
    const versioned = new Set(selectedVersioned.map((file) => file.path));
    const keys = selectedKeys.filter((key) => versioned.has(pathOfKey(key)));
    const built = silentGroups(keys, changes, lists, hunksRead);
    if (built.unreadable.length > 0) {
      notify("error", unreadableShelfText(built.unreadable));
      return;
    }
    await shelves.shelveSilently(repo.path, built.groups, rollback);
  }

  function perform(action: ActionId) {
    switch (action) {
      case "ChangesView.ShelveSilently":
      case "ChangesView.SaveToShelve":
        if (!canShelveSilently()) return false;
        void shelveSilently(action === "ChangesView.ShelveSilently");
        break;
      case "ChangesView.Revert":
        openRollback();
        break;
      // `Git.Stage.Add` takes the shortcut of `ChangesView.AddUnversioned`.
      case "ChangesView.AddUnversioned":
        if (stagingArea) stageSelected();
        else if (selectedUnversioned.length > 0) void addToVcs();
        else return false;
        break;
      case "ChangesView.Move":
        openMove();
        break;
      case "ChangesView.Rename":
        openEdit();
        break;
      case "ChangesView.SetDefault":
        setActive();
        break;
      case "ChangesView.GroupBy.Directory":
        toggleGrouping();
        break;
      case "Diff.ShowDiff":
        showDiff();
        break;
      case "ExpandAll":
        expandAll();
        break;
      case "CollapseAll":
        collapseAll();
        break;
      default:
        return false;
    }
    return true;
  }

  // These act on the tree selection: they must not trigger while typing.
  function keydown(event: KeyboardEvent) {
    if (!active || suspended || dialog || menu) return;
    const action = matchShortcut(event);
    if (!action) return;
    const typing =
      event.target instanceof HTMLTextAreaElement || event.target instanceof HTMLInputElement;
    if (typing) return;
    if (perform(action)) event.preventDefault();
  }

  $effect(() => {
    onselection?.(
      selectedFiles.map((file) => file.path),
      selectedLists.map((list) => list.id),
      firstSelectedFile()?.path ?? null,
    );
  });

  $effect(() => {
    onconflictschange?.(groups.conflicts.length > 0);
  });

  let shownStaging = untrack(() => stagingArea);

  $effect(() => {
    if (stagingArea === shownStaging) return;
    shownStaging = stagingArea;
    untrack(() => {
      reload();
      tick().then(() => {
        if (active) focusTree();
      });
    });
  });

  onMount(() => {
    const unlisten = onRepoChanged(() => reload());
    reload();
    if (active) focusTree();
    return () => {
      unlisten.then((stop) => stop());
    };
  });

  onDestroy(() => {
    destroyed = true;
    reload.cancel();
    timers.forEach(clearTimeout);
  });
</script>

<svelte:window onkeydown={browser ? undefined : keydown} />

{#snippet rollbackAction()}
  <button
    type="button"
    class="action"
    title="{stagingArea ? 'Rollback' : 'Rollback…'} (⌥⌘Z)"
    aria-label={stagingArea ? "Rollback" : "Rollback…"}
    disabled={stagingArea ? toRevert.length + toUnstage.length === 0 : !hasVersioned}
    onclick={openRollback}
  >
    <Icon name="revert" />
  </button>
{/snippet}

{#snippet showDiffAction()}
  <button
    type="button"
    class="action"
    title="Show Diff (⌘D)"
    aria-label="Show Diff"
    disabled={selectedFiles.length === 0}
    onclick={showDiff}
  >
    <Icon name="diff" />
  </button>
{/snippet}

{#snippet refreshAction()}
  <button type="button" class="action" title="Refresh" aria-label="Refresh" onclick={reload}>
    <Icon name="refresh" />
  </button>
{/snippet}

{#snippet treeActions()}
  <button
    type="button"
    class="action"
    title="Expand All (⌘+)"
    aria-label="Expand All"
    onclick={expandAll}
  >
    <Icon name="expandAll" />
  </button>
  <button
    type="button"
    class="action"
    title="Collapse All (⌘-)"
    aria-label="Collapse All"
    onclick={collapseAll}
  >
    <Icon name="collapseAll" />
  </button>
{/snippet}

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="panel {toolbar}" onkeydown={browser ? keydown : undefined}>
  {#if browser}
    <!-- `ChangesBrowserBase` and `TreeActionsToolbarPanel`: the browser's actions, then the tree's. -->
    <div class="toolbar" role="toolbar" aria-label="Changes" aria-orientation={toolbar}>
      {@render showDiffAction()}
      {@render rollbackAction()}
      {@render refreshAction()}
      <span class="toolbar-gap"></span>
      <button
        type="button"
        class="action"
        title="Group By"
        aria-label="Group By"
        onclick={openGroupBy}
      >
        <Icon name="groupBy" />
      </button>
      {@render treeActions()}
    </div>
  {:else}
    <div class="toolbar" role="toolbar" aria-label="Changes" aria-orientation={toolbar}>
      {@render refreshAction()}
      {@render actions?.()}
      {@render rollbackAction()}
      {@render showDiffAction()}
      {#if stagingArea}
        <button
          type="button"
          class="action"
          title="Stage All"
          aria-label="Stage All"
          disabled={groups.unstaged.length + groups.untracked.length === 0}
          onclick={() => staging?.apply(stageAllTargets(groups))}
        >
          <Icon name="add" />
        </button>
        <!-- `Git.Stash.Silently`: enabled while the staging area has changes. -->
        <button
          type="button"
          class="action"
          title="Stash Silently"
          aria-label="Stash Silently"
          disabled={groups.staged.length + groups.unstaged.length + groups.untracked.length + groups.conflicts.length === 0}
          onclick={() => staging?.stashSilently(repo.path)}
        >
          <Icon name="stash" />
        </button>
      {:else}
        <button
          type="button"
          class="action"
          title="Changelists"
          aria-label="Changelists"
          onclick={openChangelistsMenu}
        >
          <Icon name="changelist" />
        </button>
        <!-- `ChangesView.ShelveSilently`, last of `ChangesViewToolbar`; the New UI draws it with the
             stash icon. -->
        <button
          type="button"
          class="action"
          title="Shelve Silently (⇧⌘H)"
          aria-label="Shelve Silently"
          disabled={!canShelveSilently()}
          onclick={() => void shelveSilently(true)}
        >
          <Icon name="stash" />
        </button>
      {/if}
      <span class="toolbar-separator"></span>
      <button
        type="button"
        class="action"
        title="View Options"
        aria-label="View Options"
        onclick={openViewOptions}
      >
        <Icon name="show" />
      </button>
      {@render treeActions()}
      {#if trailing}
        <span class="toolbar-separator"></span>
        {@render trailing()}
      {/if}
    </div>
  {/if}

  <div class="body">
    {#if recoveryLines.length > 0}
      <div class="banner" role="alert">
        <Icon name="warning" />
        <div class="banner-text">
          {#each recoveryLines as line}
            <p>{line}</p>
          {/each}
        </div>
        <button
          type="button"
          class="action"
          aria-label="Close Notification"
          onclick={() => (recoveryDismissed = true)}
        >
          <Icon name="close" />
        </button>
      </div>
    {/if}

    {#if stagingArea}
      <StagingView
        bind:this={staging}
        {roots}
        {changes}
        bind:collapsed
        bind:selected
        bind:lead
        onchanged={reload}
        oncopy={stagingCopy}
        onerror={(text) => notify("error", text)}
        onopen={openNode}
        onmenu={openContextMenu}
        onresolve={() => onresolveconflicts?.()}
      />
    {:else}
      <ChangesTree
        bind:this={tree}
        roots={shownRoots}
        {included}
        {partial}
        bind:collapsed
        bind:selected
        bind:lead
        ontoggle={(targets) => ontoggle?.(targets)}
        onopen={openNode}
        onmenu={openContextMenu}
        onresolve={() => onresolveconflicts?.()}
        ondrop={browser ? undefined : dropOnList}
      />
    {/if}
  </div>

  <!-- Positioned against the owner's view, which is the one that is `position: relative`. -->
  {#if notices.length > 0}
    <div class="notices">
      {#each notices as notice (notice.id)}
        <div class="notice" role={notice.kind === "error" ? "alert" : "status"}>
          <Icon name={notice.kind === "error" ? "error" : notice.kind === "warning" ? "warning" : "info"} />
          <p>{notice.text}</p>
          {#if notice.action}
            <button
              type="button"
              class="link"
              onclick={() => {
                const action = notice.action;
                dismiss(notice.id);
                action?.run();
              }}
            >
              {notice.action.label}
            </button>
          {/if}
          <button
            type="button"
            class="action close"
            aria-label="Close Notification"
            onclick={() => dismiss(notice.id)}
          >
            <Icon name="close" />
          </button>
        </div>
      {/each}
    </div>
  {/if}
</div>

{#if menu}
  <Menu {...menu} onclose={() => (menu = null)} />
{/if}

{#if dialog?.kind === "new" && lists}
  <ChangelistDialog
    title="New Changelist"
    taken={lists.lists.map((list) => list.name)}
    canSetActive={true}
    onconfirm={createList}
    onclose={() => (dialog = null)}
  />
{:else if dialog?.kind === "edit" && lists}
  {@const list = dialog.list}
  <ChangelistDialog
    title="Edit Changelist"
    name={list.name}
    taken={lists.lists.filter((other) => other.id !== list.id).map((other) => other.name)}
    canSetActive={list.id !== lists.active}
    onconfirm={(name, makeActive) => editList(list, name, makeActive)}
    onclose={() => (dialog = null)}
  />
{:else if dialog?.kind === "move" && lists}
  {@const paths = dialog.paths}
  <ListChooserDialog
    title="Move to Another Changelist"
    lists={lists.lists}
    initial={lists.lists.find((list) => list.id !== lists?.assignments[paths[0]])?.name ?? ""}
    allowNew
    onconfirm={(name, makeActive) => moveTo(paths, name, makeActive)}
    onclose={() => (dialog = null)}
  />
{:else if dialog?.kind === "confirm-remove"}
  {@const list = dialog.list}
  <ConfirmDialog
    title="Delete Changelist"
    lines={[
      `Are you sure want to remove changelist '${list.name}'?`,
      "All changes will be moved to the active changelist.",
    ]}
    onconfirm={() => removeConfirmed(list)}
    onclose={() => (dialog = null)}
  />
{:else if dialog?.kind === "remove-empty"}
  {@const list = dialog.list}
  <ConfirmDialog
    title="Remove Empty Changelist"
    lines={[`The empty changelist '${list.name}' is no longer active.`, "Do you want to remove it?"]}
    onconfirm={() => removeConfirmed(list)}
    onclose={() => (dialog = null)}
  />
{:else if dialog?.kind === "remove-active" && lists}
  {@const list = dialog.list}
  {@const others = lists.lists.filter((other) => other.id !== list.id)}
  <ListChooserDialog
    title="Delete Active Changelist"
    prompt={(nodes.get(`list:${list.id}`)?.paths.length ?? 0) > 0
      ? "Select the changelist to which the changes should be moved:"
      : "Select the changelist to make active:"}
    lists={others}
    initial={others[0].name}
    onconfirm={(name) => removeActive(list, name)}
    onclose={() => (dialog = null)}
  />
{:else if dialog?.kind === "rollback"}
  <RollbackDialog
    files={dialog.files}
    checked={dialog.checked}
    onconfirm={rollback}
    onclose={() => (dialog = null)}
  />
{:else if dialog?.kind === "snapshots"}
  <SnapshotsDialog onconfirm={restore} onclose={() => (dialog = null)} />
{/if}

<style>
  .panel {
    display: flex;
    flex: 1;
    min-width: 0;
    min-height: 0;
  }

  .panel.horizontal {
    flex-direction: column;
  }

  .toolbar {
    display: flex;
    flex: none;
    gap: 4px;
    align-items: center;
  }

  .horizontal .toolbar {
    height: 30px;
    padding: 0 8px;
  }

  .vertical .toolbar {
    flex-direction: column;
    width: 30px;
    padding: 4px 0;
    border-right: 1px solid var(--divider);
  }

  .toolbar-gap {
    flex: 1;
  }

  .vertical .toolbar-separator {
    width: 16px;
    height: 1px;
    margin: 5px 0;
  }

  .body {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }

  .banner {
    display: flex;
    flex: none;
    gap: 8px;
    align-items: flex-start;
    padding: 8px 8px 8px 12px;
    background: var(--notice-warning);
    border-top: 1px solid var(--notice-warning-border);
    border-bottom: 1px solid var(--notice-warning-border);
  }

  .banner-text {
    flex: 1;
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .banner-text p {
    margin: 0 0 4px;
  }

  .notices {
    position: absolute;
    right: 12px;
    bottom: 12px;
    z-index: 10;
    display: flex;
    flex-direction: column;
    gap: 8px;
    width: min(420px, calc(100% - 24px));
  }

  .notice {
    display: flex;
    gap: 8px;
    align-items: flex-start;
    padding: 10px 6px 10px 12px;
    color: var(--notification-text);
    background: var(--notification-background);
    border: 1px solid var(--notification-border);
    border-radius: 8px;
    box-shadow: 0 4px 16px rgb(0 0 0 / 35%);
  }

  .notice p {
    flex: 1;
    min-width: 0;
    margin: 0;
    overflow-wrap: anywhere;
  }

  .link {
    padding: 0;
    font: inherit;
    color: var(--link);
    white-space: nowrap;
    background: none;
    border: 0;
  }

  .notice .close {
    width: 20px;
    height: 20px;
  }
</style>
