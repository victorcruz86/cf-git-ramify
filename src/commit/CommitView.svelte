<script lang="ts">
  import { onDestroy, onMount, tick, untrack } from "svelte";
  import {
    changelistsCreate,
    changelistsGet,
    changelistsImport,
    changelistsMove,
    changelistsRemove,
    changelistsRename,
    changelistsSetActive,
    commitChanges,
    commitChangesSelected,
    commitStaged,
    commitWarnings,
    commitWarningsStaged,
    conflictsAccept,
    conflictsSession,
    fileHunks,
    headMessage,
    mergeMessage,
    messagesList,
    onRepoChanged,
    repoState,
    repoStatus,
    rollbackFiles,
    snapshotsRestore,
    stagingRevert,
    type ApiError,
    type ChangeList,
    type Changelists,
    type CommitOptions,
    type Comparison,
    type StageComparison,
    type Conflict,
    type ConflictSession,
    type ConflictSide,
    type FileChange,
    type Hunk,
    type LineSelection,
    type LostSelection,
    type RepoInfo,
    type RepoState,
    type SelectionLost,
    type SelectionState,
  } from "../lib/api";
  import { errorText, lostText } from "../lib/errors";
  import Icon from "../lib/Icon.svelte";
  import type { ActionId } from "../lib/keymap";
  import { matchShortcut } from "../lib/keymap";
  import Menu, { type MenuItem } from "../lib/Menu.svelte";
  import { mergeFiles } from "../conflicts/flow";
  import type { CommitProposal } from "../log/editing";
  import { taskResult } from "../lib/tasks";
  import { commitAndPush } from "../push/commitAndPush";
  import ChangelistDialog from "./ChangelistDialog.svelte";
  import { findList } from "./changelists";
  import ChangesTree from "./ChangesTree.svelte";
  import CommitMessage from "./CommitMessage.svelte";
  import ConfirmDialog from "./ConfirmDialog.svelte";
  import {
    activateInclusion,
    forceInclusion,
    noteInclusion,
    retainInclusion,
    toggleInclusion,
    type Inclusion,
  } from "./inclusion";
  import ListChooserDialog from "./ListChooserDialog.svelte";
  import OptionsDialog from "./OptionsDialog.svelte";
  import { latestOnly } from "./refresh";
  import {
    buildCommit,
    dropLostHunks,
    legend,
    listSelections,
    liveLists,
    rollbackTargets,
    type Selections,
  } from "./request";
  import RollbackDialog from "./RollbackDialog.svelte";
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
  import StagingView from "./StagingView.svelte";
  import {
    buildTree,
    collectFiles,
    flattenTree,
    partKey,
    partLists,
    pathOfKey,
    UNVERSIONED_ID,
    type TreeNode,
  } from "./tree";
  import WarningsGate from "./WarningsGate.svelte";

  const NOTICE_MS = 10_000;
  const SPLIT_KEY = "commit.messageHeight";
  const GROUP_KEY = "commit.groupByDirectory";
  const MIN_MESSAGE_HEIGHT = 120;
  const MIN_TREE_HEIGHT = 120;
  // Actions that act on the tree selection must not trigger while typing.
  const TEXT_SAFE: ActionId[] = [
    "CheckinProject",
    "CommitView.Commit",
    "Git.Commit.And.Push.Executor",
    "Vcs.ShowMessageHistory",
    "Vcs.ToggleAmendCommitMode",
  ];

  interface Props {
    repo: RepoInfo;
    active: boolean;
    // With the staging area, the Staged and Unstaged groups instead of the changelists, which are left alone.
    stagingArea?: boolean;
    onshowdiff?: (file: FileChange, comparison?: StageComparison) => void;
    oncommitted?: () => void;
    onselectionstate?: (state: SelectionState) => void;
    onselectionlost?: (lost: LostSelection) => void;
    // `Commit and Push…`: called before the commit, and what it returns is pushed after the commit is made.
    onpreparepush?: () => () => Promise<void>;
    // `Commit and Rebase…` of Fixup… and Squash Into…: the rebase from `base`, once the commit is made.
    onrebase?: (base: string) => Promise<void>;
    // The `Resolve` link of the conflicts node: opens the Conflicts dialog.
    onresolveconflicts?: () => void;
    // Whether any file is in conflict: enables `Resolve Conflicts…` of the Git menu.
    onconflictschange?: (conflicts: boolean) => void;
  }

  let {
    repo,
    active,
    stagingArea = false,
    onshowdiff,
    oncommitted,
    onselectionstate,
    onselectionlost,
    onpreparepush,
    onrebase,
    onresolveconflicts,
    onconflictschange,
  }: Props = $props();

  type Dialogs =
    | { kind: "new" }
    | { kind: "edit"; list: ChangeList }
    | { kind: "move"; paths: string[] }
    | { kind: "confirm-remove"; list: ChangeList }
    | { kind: "remove-empty"; list: ChangeList }
    | { kind: "remove-active"; list: ChangeList }
    | { kind: "rollback"; files: FileChange[]; checked: string[] }
    | { kind: "options" }
    | { kind: "snapshots" };

  interface Notice {
    id: number;
    kind: "info" | "warning" | "error";
    text: string;
    action?: { label: string; run: () => void };
  }

  interface PopupMenu {
    x: number;
    y: number;
    above?: boolean;
    label: string;
    items: MenuItem[];
  }

  function stored(key: string): string | null {
    try {
      return localStorage.getItem(key);
    } catch {
      return null;
    }
  }

  function store(key: string, value: string) {
    try {
      localStorage.setItem(key, value);
    } catch {
      // Without storage, all that is lost is what is remembered between sessions.
    }
  }

  let changes = $state.raw<FileChange[]>([]);
  let lists = $state.raw<Changelists | null>(null);
  let groupByDirectory = $state(stored(GROUP_KEY) !== "false");
  let inclusion = $state.raw<Inclusion>({ included: new Set(), known: new Set() });
  let collapsed = $state.raw(new Set([UNVERSIONED_ID]));
  let selected = $state.raw(new Set<string>());
  let lead = $state<string | null>(null);
  let lineSelections = $state.raw<Selections>(new Map());
  // The hunks of what is chosen or split across lists, up to date with the last refresh.
  let fileHunksKnown = new Map<string, Hunk[]>();
  // Where each chosen hunk was, to say what was lost when it no longer exists.
  const places = new Map<string, string>();

  // The conflicts with the state of each side, to know which ones can be merged.
  let conflictInfo = $state.raw(new Map<string, Conflict>());
  let conflictKey = "";
  // The last merge message that was offered: it is not offered again if it is deleted on purpose.
  let proposedMerge: string | null = null;

  let message = $state("");
  // Fixup… or Squash Into… in progress: what was proposed, the repository state that was seen and
  // what was in the view before (the draft, Amend and the checkboxes), which comes back when it
  // ends. `rebaseFrom` is the base of `Commit and Rebase…`.
  interface Prior {
    message: string;
    amend: boolean;
    amendRestore: { before: string; loaded: string } | null;
    inclusion: Inclusion;
    lineSelections: Selections;
  }
  let proposal = $state.raw<{
    rebaseFrom: string | null;
    head: string;
    prior: Prior;
  } | null>(null);
  const rebaseFrom = $derived(proposal?.rebaseFrom ?? null);
  let amend = $state(false);
  let amendRestore: { before: string; loaded: string } | null = null;
  let author = $state("");
  let signOff = $state(false);
  let runHooks = $state(true);
  let committed = $state(false);
  let problem = $state<string | null>(null);
  let committing = $state(false);
  let focused = $state(false);

  let dialog = $state.raw<Dialogs | null>(null);
  let menu = $state.raw<PopupMenu | null>(null);
  let notices = $state.raw<Notice[]>([]);
  let nextNotice = 0;
  let recoveryDismissed = $state(false);
  let messageHeight = $state(Number(stored(SPLIT_KEY)) || 220);
  let root: HTMLElement;
  let tree = $state<ChangesTree>();
  let staging = $state<StagingView>();
  let gate: WarningsGate;
  let editor: CommitMessage;
  let initialized = false;
  let destroyed = false;
  const timers = new Set<ReturnType<typeof setTimeout>>();

  const hasHead = $derived(repo.head !== null || committed);
  const groups = $derived(groupStaging(changes));
  const roots = $derived.by(() => {
    if (stagingArea) return stagingTree(groups, groupByDirectory);
    return lists ? buildTree(changes, lists, groupByDirectory) : [];
  });
  const activeRoot = $derived(roots.find((node) => node.active));
  const nodes = $derived(
    new Map(flattenTree(roots, new Set()).map((row) => [row.node.id, row.node])),
  );
  const selectedNodes = $derived(
    [...selected].flatMap((id) => {
      const node = nodes.get(id);
      return node ? [node] : [];
    }),
  );
  const selectedFiles = $derived(collectFiles(selectedNodes));
  const selectedVersioned = $derived(
    selectedFiles.filter((file) => file.state !== "untracked" && file.state !== "ignored"),
  );
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
  const partialFiles = $derived(new Set(lineSelections.keys()));
  const includedFiles = $derived(
    new Set(
      stagingArea
        ? groups.staged.map((change) => change.path)
        : [...inclusion.included].map(pathOfKey),
    ),
  );
  const legendItems = $derived(legend(changes, includedFiles));
  const recoveryLines = $derived(recoveryText());

  function activePaths(): string[] {
    return activeRoot?.paths ?? [];
  }

  function unversionedPaths(): string[] {
    return nodes.get(UNVERSIONED_ID)?.paths ?? [];
  }

  function recoveryText(): string[] {
    const report = repo.recovery;
    if (!report || recoveryDismissed) return [];
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

  function notify(kind: Notice["kind"], text: string, action?: Notice["action"]) {
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
      const wanted = new Set([...lineSelections.keys(), ...Object.keys(assigned.hunkAssignments)]);
      const hunks = await readHunks(paths.filter((path) => wanted.has(path)));
      return { status, assigned, hunks };
    },
    ({ status, assigned, hunks }) => {
      changes = status;
      void refreshMergeInfo(status);
      void proposeMergeMessage();
      void checkProposal();
      if (!assigned) return;
      lists = liveLists(assigned, hunks);
      fileHunksKnown = hunks;
      const active = activePaths();
      if (initialized) {
        const parts = status.flatMap((change) =>
          lists ? partLists(lists, change.path).map((id) => partKey(change.path, id)) : [],
        );
        inclusion = retainInclusion(
          inclusion,
          [...status.map((change) => change.path), ...parts],
          active,
        );
      } else {
        initialized = true;
        inclusion = activateInclusion(inclusion, active, unversionedPaths());
      }
      dropLostSelections(new Set(status.map((change) => change.path)), hunks);
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

  // `GitCheckinEnvironment.getDefaultMessageFor`: the message git left for this commit, if the box
  // is empty.
  async function proposeMergeMessage() {
    let text: string | null;
    try {
      text = await mergeMessage();
    } catch {
      return;
    }
    if (destroyed) return;
    if (!text?.trim()) {
      proposedMerge = null;
      return;
    }
    if (text === proposedMerge) return;
    proposedMerge = text;
    if (!amend && !message.trim()) message = text;
  }

  function lostPlaces(lost: SelectionLost): string[] {
    return lost.hunks.flatMap((hunk) => {
      const place = places.get(hunk);
      return place ? [place] : [];
    });
  }

  function dropLost(lost: SelectionLost) {
    lineSelections = dropLostHunks(lineSelections, lost);
    if (!lineSelections.has(lost.path)) exclude(lost.path);
  }

  // Devin can change a file with chosen lines: what still exists is kept and the user is told about
  // the rest. During a commit the hunks change because of the commit itself.
  function dropLostSelections(paths: Set<string>, hunks: Map<string, Hunk[]>) {
    if (committing) return;
    for (const [path, selection] of lineSelections) {
      const current = paths.has(path) ? hunks.get(path) : [];
      if (!current) continue;
      const known = new Set(current.map((hunk) => hunk.id));
      const gone = selection.map((entry) => entry.hunk).filter((id) => !known.has(id));
      if (gone.length === 0) continue;
      const lost = { path, hunks: [...new Set(gone)] };
      const text = lostText(lost, lostPlaces(lost));
      dropLost(lost);
      notify("warning", text);
      onselectionlost?.({ ...lost, text });
    }
  }

  // The diff window chooses hunks or lines of a file; without `included` the checkbox is left alone.
  export function setLineSelection(
    path: string,
    selection: LineSelection[],
    included?: boolean,
    where: Record<string, string> = {},
  ) {
    const next = new Map(lineSelections);
    if (selection.length > 0) next.set(path, selection);
    else next.delete(path);
    lineSelections = next;
    for (const [hunk, place] of Object.entries(where)) places.set(hunk, place);
    if (included === undefined || included === inclusion.included.has(path)) return;
    const paths = new Set(inclusion.included);
    if (included) paths.add(path);
    else paths.delete(path);
    inclusion = noteInclusion({ included: paths, known: inclusion.known }, activePaths());
    problem = null;
  }

  export function focusTree() {
    (stagingArea ? staging : tree)?.focus();
  }

  export function checkin() {
    if (!stagingArea) inclusion = activateInclusion(inclusion, activePaths(), unversionedPaths());
    problem = null;
    editor.focus();
  }

  function commitFiles() {
    inclusion = forceInclusion(
      selectedFiles.map((file) => file.path),
      activePaths(),
    );
    dropUnchecked();
    problem = null;
    editor.focus();
  }

  // Unchecking a file removes what was chosen from it; checking it again means the whole file.
  function dropUnchecked() {
    const kept = [...lineSelections].filter(([path]) => inclusion.included.has(path));
    if (kept.length !== lineSelections.size) lineSelections = new Map(kept);
  }

  function toggle(targets: TreeNode[]) {
    const paths = [...new Set(targets.flatMap((node) => node.paths))];
    const included = toggleInclusion(inclusion.included, paths);
    inclusion = noteInclusion({ included, known: inclusion.known }, activePaths());
    dropUnchecked();
    problem = null;
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
    if (file) onshowdiff?.(file, comparisonFor(node));
  }

  function openNode(node: TreeNode) {
    if (node.change) onshowdiff?.(node.change, comparisonFor(node));
  }

  function missingMessage(): string | null {
    if (stagingArea && groups.conflicts.length > 0) return "Resolve conflicts in order to commit";
    const nothing = stagingArea ? groups.staged.length === 0 : inclusion.included.size === 0;
    const noChanges = !amend && nothing;
    const noMessage = !message.trim();
    const choose = stagingArea ? "Stage files to commit" : "Select files to commit";
    if (noChanges && noMessage) return `${choose} and specify commit message`;
    if (noChanges) return choose;
    if (noMessage) return "Specify commit message";
    return null;
  }

  function committedText(count: number, text: string): string {
    const subject = text.split("\n")[0];
    if (count === 0) return `Committed: ${subject}`;
    return `${count === 1 ? "1 file" : `${count} files`} committed: ${subject}`;
  }

  function currentOptions(): CommitOptions {
    return {
      amend,
      signOff,
      skipHooks: !runHooks,
      author: author.trim() || null,
      cleanup: false,
    };
  }

  function commitLabel(): string {
    return amend ? "Amend Commit" : "Commit";
  }

  function finishCommit(count: number, text: string, files?: Set<string>) {
    notify("info", committedText(count, text));
    message = "";
    amend = false;
    amendRestore = null;
    author = "";
    committed = true;
    // A commit, with whichever executor, uses up the proposal and brings back what was there before.
    endProposal(files, true);
  }

  // Fixup… and Squash Into…: it is an explicit action, so what is proposed replaces the message in
  // the box and the checkboxes become the changes of the default list (`changeList.changes`). Since
  // IntelliJ's modal dialog does not touch the Commit view, what was there is kept. The merge
  // message that was offered before does not come back: `proposedMerge` is still that text.
  export function propose({ message: proposed, rebaseFrom: base, head }: CommitProposal) {
    if (committing) return;
    // A proposal over another one keeps what was there first.
    const prior = proposal?.prior ?? { message, amend, amendRestore, inclusion, lineSelections };
    amend = false;
    amendRestore = null;
    problem = null;
    message = proposed;
    if (!stagingArea) {
      inclusion = forceInclusion(activePaths(), activePaths());
      dropUnchecked();
    }
    proposal = { rebaseFrom: base, head, prior };
    tick().then(() => editor.focus());
  }

  // Ends the proposal: the earlier draft, Amend and checkboxes come back. If HEAD moved (after a
  // commit, or from outside), Amend does not come back, because it would amend the new commit with
  // the message of the earlier HEAD, and the draft from before activating it remains. The line
  // selections of already committed files (`files`) do not come back either.
  function endProposal(files?: Set<string>, headMoved = false) {
    const current = proposal;
    if (!current) return;
    proposal = null;
    const { prior } = current;
    if (headMoved && prior.amend) {
      message = prior.amendRestore?.before ?? "";
      amend = false;
      amendRestore = null;
    } else {
      message = prior.message;
      amend = prior.amend;
      amendRestore = prior.amendRestore;
    }
    if (stagingArea) return;
    inclusion = prior.inclusion;
    const back = [...prior.lineSelections].filter(
      ([path]) => !files?.has(path) && !lineSelections.has(path),
    );
    if (back.length > 0) lineSelections = new Map([...lineSelections, ...back]);
  }

  // The executor was valid for the state that was seen when proposing.
  const stale = (state: RepoState, current: NonNullable<typeof proposal>) =>
    state.operation !== null || state.head !== current.head;

  function dropStaleProposal(state: RepoState, current: NonNullable<typeof proposal>) {
    endProposal(undefined, state.head !== current.head);
    notify(
      "warning",
      "The repository has changed since Fixup… or Squash Into…: Commit and Rebase… is no longer offered and the earlier message is back",
    );
  }

  // After a `repo-changed`: if what was seen when proposing is no longer so, the executor goes away.
  async function checkProposal() {
    const current = proposal;
    if (!current?.rebaseFrom || committing) return;
    try {
      const state = await repoState();
      if (proposal === current && !committing && !destroyed && stale(state, current)) {
        dropStaleProposal(state, current);
      }
    } catch {
      // Without reading the state nothing is decided: it is checked again before rebasing.
    }
  }

  // The index as it is: what is not in it does not get in. `true` if the commit was made.
  async function commitIndex(label: string): Promise<boolean> {
    committing = true;
    const count = groups.staged.length;
    const text = message;
    let done = false;
    try {
      if (!(await gate.confirm(await commitWarningsStaged(), label))) return false;
      await commitStaged(currentOptions(), text);
      done = true;
      finishCommit(count, text);
      oncommitted?.();
      committing = false;
      await reload();
    } catch (failure) {
      notify("error", errorText(failure));
    } finally {
      committing = false;
    }
    return done;
  }

  // `true` if the commit was made: `Commit and Push…` pushes only then.
  async function commit(label = commitLabel()): Promise<boolean> {
    if (committing) return false;
    problem = missingMessage();
    if (problem) return false;
    if (stagingArea) return commitIndex(label);
    const chosen = lists
      ? listSelections(inclusion.included, lists, fileHunksKnown, lineSelections)
      : { included: inclusion.included, selections: lineSelections, unreadable: [] };
    if (chosen.unreadable.length > 0) {
      const files = chosen.unreadable.map((path) => `"${path}"`).join(", ");
      notify(
        "error",
        `Could not read the changes of ${files}, split across lists: the commit was not made`,
      );
      return false;
    }
    committing = true;
    const { request, selections } = buildCommit(
      changes,
      chosen.included,
      chosen.selections,
      message,
      currentOptions(),
    );
    const sent = lineSelections;
    // A `repo-changed` can change the tree before the commit responds.
    const count = chosen.included.size;
    const text = message;
    const touched = roots.flatMap((node) =>
      node.list && !node.active && node.paths.some((path) => inclusion.included.has(path))
        ? [node.list]
        : [],
    );
    let done = false;
    try {
      if (!(await gate.confirm(await commitWarnings(request, selections), label))) return false;
      if (selections.length > 0) await commitChangesSelected(request, selections);
      else await commitChanges(request);
      done = true;
      finishCommit(count, text, new Set([...chosen.included].map(pathOfKey)));
      // A new selection in the diff window made while the commit was running was not part of it.
      selections.forEach(({ path }) => {
        if (lineSelections.get(path) === sent.get(path)) forgetSelection(path);
      });
      oncommitted?.();
      committing = false;
      await reload();
      offerToRemoveEmpty(touched);
    } catch (failure) {
      const lost = (failure as ApiError).details;
      const where = lost ? lostPlaces(lost) : [];
      const text = errorText(failure, where);
      if (lost) {
        dropLost(lost);
        onselectionlost?.({ ...lost, text });
      }
      notify("error", text);
    } finally {
      committing = false;
    }
    return done;
  }

  // `GitRebaseCheckinHandlerFactory`: the rebase runs only if the commit was made, and if the
  // repository is still as it was seen when proposing (no operation in progress and the same HEAD).
  async function commitThenRebase() {
    const current = proposal;
    const base = current?.rebaseFrom;
    if (!current || !base || !onrebase || committing) return;
    try {
      const state = await repoState();
      if (destroyed || proposal !== current) return;
      if (stale(state, current)) {
        dropStaleProposal(state, current);
        return;
      }
      if ((await commit("Commit and Rebase")) && !destroyed) await onrebase(base);
    } catch (failure) {
      notify("error", errorText(failure));
    }
  }

  function commitThenPush() {
    if (!onpreparepush) return;
    const label = `${commitLabel()} and Push`;
    commitAndPush(() => commit(label), onpreparepush(), () => !destroyed).catch((failure) =>
      notify("error", errorText(failure)),
    );
  }

  // Nobody chose what remains of the file: without this, the next commit would send it whole.
  function forgetSelection(path: string) {
    const next = new Map(lineSelections);
    next.delete(path);
    lineSelections = next;
    exclude(path);
  }

  function exclude(path: string) {
    const included = new Set(
      [...inclusion.included].filter((key) => pathOfKey(key) !== path),
    );
    inclusion = { included, known: inclusion.known };
  }

  function offerToRemoveEmpty(touched: ChangeList[]) {
    const empty = touched.find((list) => nodes.get(`list:${list.id}`)?.paths.length === 0);
    if (empty && !dialog) dialog = { kind: "remove-empty", list: empty };
  }

  async function toggleAmend() {
    if (committing) return;
    problem = null;
    if (amend) {
      amend = false;
      if (amendRestore && sameText(message, amendRestore.loaded)) message = amendRestore.before;
      amendRestore = null;
      return;
    }
    if (!hasHead) return;
    amend = true;
    try {
      const loaded = await headMessage();
      if (!amend || sameText(message, loaded)) return;
      amendRestore = { before: message, loaded };
      message = loaded;
    } catch (failure) {
      amend = false;
      notify("error", errorText(failure));
    }
  }

  function sameText(left: string, right: string): boolean {
    return left.replace(/\s+/g, "") === right.replace(/\s+/g, "");
  }

  async function run(operation: () => Promise<unknown>) {
    try {
      await operation();
      await reload();
    } catch (failure) {
      notify("error", errorText(failure));
    }
  }

  function openRollback() {
    if (stagingArea) {
      revertOrUnstage();
      return;
    }
    if (!lists || !hasVersioned) return;
    const chosen = selectedFiles.map((file) => file.path);
    dialog = { kind: "rollback", ...rollbackTargets(changes, lists, chosen) };
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
      // What was chosen in these files was undone on purpose: it has not been lost.
      paths.filter((path) => lineSelections.has(path)).forEach(forgetSelection);
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
      const previous = await snapshotsRestore(id);
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

  function openChangelistsMenu(event: MouseEvent) {
    const bounds = (event.currentTarget as HTMLElement).getBoundingClientRect();
    menu = {
      x: bounds.left,
      y: bounds.bottom + 2,
      label: "Changelists",
      items: changelistActions(),
    };
  }

  function openGroupMenu(event: MouseEvent) {
    const bounds = (event.currentTarget as HTMLElement).getBoundingClientRect();
    menu = {
      x: bounds.left,
      y: bounds.bottom + 2,
      label: "Group By",
      items: [
        { label: "Directory", shortcut: "⌃P", checked: groupByDirectory, run: toggleGrouping },
      ],
    };
  }

  function stagingCopy(snapshot: string) {
    notify("info", "A snapshot was saved of what existed only in the stage.", {
      label: "Restore",
      run: () => restore(snapshot),
    });
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
          run: () => three && onshowdiff?.(three, "three"),
        },
        null,
        { label: "Restore Snapshot…", run: () => (dialog = { kind: "snapshots" }) },
        null,
        { label: "Refresh", run: reload },
      ],
    };
  }

  function openContextMenu(x: number, y: number) {
    if (stagingArea) {
      openStagingMenu(x, y);
      return;
    }
    const count = selectedFiles.length;
    const items: MenuItem[] = conflictActions();
    if (count > 0) {
      items.push({ label: `Commit ${count === 1 ? "File" : "Files"}…`, run: commitFiles });
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
    items.push(null, { label: "New Changelist...", run: openNew });
    if (canRemove(singleList)) items.push({ label: "Delete Changelist", run: removeList });
    if (singleList && singleList.id !== lists?.active) {
      items.push({ label: "Set Active Changelist", shortcut: "⌃Space", run: setActive });
    }
    if (singleList) items.push({ label: "Edit Changelist...", shortcut: "F2", run: openEdit });
    items.push(
      { label: "Import Changelists from .idea", run: importLists },
      null,
      { label: "Restore Snapshot…", run: () => (dialog = { kind: "snapshots" }) },
      null,
      { label: "Refresh", run: reload },
    );
    menu = { x, y, label: "Changes", items };
  }

  async function openHistory(x: number, y: number) {
    try {
      const messages = await messagesList();
      const items: MenuItem[] = messages.map((text) => ({
        label: text,
        run: () => {
          message = text;
          problem = null;
          tick().then(() => editor.focus());
        },
      }));
      if (items.length === 0) {
        items.push({ label: "History is empty", disabled: true, run: () => {} });
      }
      menu = { x, y: y - 4, above: true, label: "Commit Message History", items };
    } catch (failure) {
      notify("error", errorText(failure));
    }
  }

  function perform(action: ActionId) {
    switch (action) {
      case "CheckinProject":
        checkin();
        break;
      case "CommitView.Commit":
        commit();
        break;
      case "Git.Commit.And.Push.Executor":
        commitThenPush();
        break;
      case "Vcs.ShowMessageHistory":
        editor.showHistory();
        break;
      case "Vcs.ToggleAmendCommitMode":
        toggleAmend();
        break;
      case "ChangesView.Revert":
        openRollback();
        break;
      case "Git.Stage.Add":
        if (!stagingArea) return false;
        stageSelected();
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

  function keydown(event: KeyboardEvent) {
    if (!active || dialog || menu) return;
    const action = matchShortcut(event);
    if (!action) return;
    const typing =
      event.target instanceof HTMLTextAreaElement || event.target instanceof HTMLInputElement;
    if (typing && !TEXT_SAFE.includes(action)) return;
    if (perform(action)) event.preventDefault();
  }

  function startResize(event: PointerEvent) {
    const handle = event.currentTarget as HTMLElement;
    handle.setPointerCapture(event.pointerId);
    const start = event.clientY;
    const height = messageHeight;
    const max = root.clientHeight - MIN_TREE_HEIGHT;
    const move = (next: PointerEvent) => {
      messageHeight = Math.max(MIN_MESSAGE_HEIGHT, Math.min(max, height + start - next.clientY));
    };
    const end = () => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", end);
      store(SPLIT_KEY, String(messageHeight));
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", end);
  }

  $effect(() => {
    onselectionstate?.({
      included: [...inclusion.included],
      selections: Object.fromEntries(lineSelections),
      lists,
    });
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

<svelte:window onkeydown={keydown} />

<section
  bind:this={root}
  class="view"
  aria-label="Commit"
  onfocusin={() => (focused = true)}
  onfocusout={() => (focused = false)}
>
  <header class="title">Commit</header>

  <div class="toolbar" role="toolbar" aria-label="Changes">
    <button type="button" class="action" title="Refresh" aria-label="Refresh" onclick={reload}>
      <Icon name="refresh" />
    </button>
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
    {/if}
    <span class="toolbar-separator"></span>
    <button
      type="button"
      class="action"
      title="Group By"
      aria-label="Group By"
      onclick={openGroupMenu}
    >
      <Icon name="show" />
    </button>
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
  </div>

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
      {roots}
      included={inclusion.included}
      partial={partialFiles}
      bind:collapsed
      bind:selected
      bind:lead
      ontoggle={toggle}
      onopen={openNode}
      onmenu={openContextMenu}
      onresolve={() => onresolveconflicts?.()}
    />
  {/if}

  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div
    class="splitter"
    role="separator"
    aria-orientation="horizontal"
    onpointerdown={startResize}
  ></div>

  <div class="commit" style:height="{messageHeight}px">
    <CommitMessage
      bind:this={editor}
      bind:message
      {amend}
      amendAvailable={hasHead}
      legend={legendItems}
      {problem}
      busy={committing}
      primary={focused}
      ontoggleamend={toggleAmend}
      onhistory={openHistory}
      onoptions={() => (dialog = { kind: "options" })}
      oncommit={() => commit()}
      onpush={onpreparepush && commitThenPush}
      onrebase={rebaseFrom && onrebase ? commitThenRebase : undefined}
      oninput={() => {
        problem = null;
        // Deleting the proposed message abandons the proposal.
        if (proposal && !message.trim()) endProposal();
      }}
    />
  </div>

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
</section>

{#if menu}
  <Menu {...menu} onclose={() => (menu = null)} />
{/if}

<WarningsGate bind:this={gate} repo={repo.path} />

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
{:else if dialog?.kind === "options"}
  <OptionsDialog
    bind:author
    bind:signOff
    bind:runHooks
    onclose={() => (dialog = null)}
  />
{:else if dialog?.kind === "snapshots"}
  <SnapshotsDialog onconfirm={restore} onclose={() => (dialog = null)} />
{/if}

<style>
  .view {
    position: relative;
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
    height: 100%;
    background: var(--panel);
  }

  .title {
    display: flex;
    flex: none;
    align-items: center;
    height: 42px;
    padding: 0 12px;
    font-weight: 600;
    color: var(--text);
  }

  .toolbar {
    display: flex;
    flex: none;
    gap: 4px;
    align-items: center;
    height: 30px;
    padding: 0 8px;
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

  .splitter {
    flex: none;
    height: 5px;
    margin: -2px 0;
    cursor: row-resize;
    background: linear-gradient(transparent 2px, var(--border) 2px, var(--border) 3px, transparent 3px);
    position: relative;
    z-index: 2;
  }

  .commit {
    display: flex;
    flex: none;
    flex-direction: column;
    min-height: 0;
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
