<script lang="ts" module>
  import type { Inclusion } from "./inclusion";
  import type { Selections } from "./request";

  // What outlives the view when it is mounted again, as on a switch of the commit mode: the draft
  // and what was chosen for the commit.
  export interface CommitDraft {
    message: string;
    inclusion: Inclusion;
    lineSelections: Selections;
    places: Map<string, string>;
  }
</script>

<script lang="ts">
  import { onDestroy, tick, untrack } from "svelte";
  import {
    commitChanges,
    commitChangesSelected,
    commitStaged,
    commitWarnings,
    commitWarningsStaged,
    headMessage,
    mergeMessage,
    messagesList,
    repoState,
    type ApiError,
    type Changelists,
    type CommitOptions,
    type StageComparison,
    type FileChange,
    type Hunk,
    type LineSelection,
    type LostSelection,
    type RepoInfo,
    type RepoState,
    type SelectionLost,
    type PublishedSelection,
  } from "../lib/api";
  import { errorText, lostText } from "../lib/errors";
  import type { ActionId } from "../lib/keymap";
  import { matchShortcut } from "../lib/keymap";
  import Menu, { type MenuItem } from "../lib/Menu.svelte";
  import { store, stored } from "../lib/storage";
  import type { CommitProposal } from "../log/editing";
  import { commitAndPush } from "../push/commitAndPush";
  import ChangesPanel, { type Refreshed } from "./ChangesPanel.svelte";
  import CommitMessage from "./CommitMessage.svelte";
  import {
    activateInclusion,
    forceInclusion,
    noteInclusion,
    retainInclusion,
    toggleInclusion,
  } from "./inclusion";
  import OptionsDialog from "./OptionsDialog.svelte";
  import {
    chooseCommit,
    committedText,
    dropLostHunks,
    findLost,
    legend,
    unreadableText,
  } from "./request";
  import { storedSignOff, storeSignOff } from "./options";
  import { isBare } from "./ticket";
  import { expandPaths } from "./dialog";
  import { groupStaging } from "./staging";
  import { partKey, partLists, pathOfKey, type TreeNode } from "./tree";
  import WarningsGate from "./WarningsGate.svelte";

  const SPLIT_KEY = "commit.messageHeight";
  const MIN_MESSAGE_HEIGHT = 120;
  const MIN_TREE_HEIGHT = 120;

  interface Props {
    repo: RepoInfo;
    active: boolean;
    // With the staging area, the Staged and Unstaged groups instead of the changelists, which are left alone.
    stagingArea?: boolean;
    // What a previous mount of the view left (`saveDraft`).
    draft?: CommitDraft | null;
    onshowdiff?: (file: FileChange, comparison?: StageComparison) => void;
    oncommitted?: () => void;
    onselectionstate?: (state: PublishedSelection) => void;
    onselectionlost?: (lost: LostSelection) => void;
    // `Commit and Push…`: called before the commit, and what it returns is pushed after the commit is made.
    onpreparepush?: () => () => Promise<void>;
    // `Commit and Rebase…` of Fixup… and Squash Into…: the rebase from `base`, once the commit is made.
    onrebase?: (base: string) => Promise<void>;
    // The `Resolve` link of the conflicts node: opens the Conflicts dialog.
    onresolveconflicts?: () => void;
    // Whether any file is in conflict: enables `Resolve Conflicts…` of the Git menu.
    onconflictschange?: (conflicts: boolean) => void;
    // The ticket of the current branch while `Start the commit message with the ticket…` is on
    // (`src/commit/ticket.ts`); `null` otherwise.
    ticket?: string | null;
  }

  let {
    repo,
    active,
    stagingArea = false,
    draft = null,
    onshowdiff,
    oncommitted,
    onselectionstate,
    onselectionlost,
    onpreparepush,
    onrebase,
    onresolveconflicts,
    onconflictschange,
    ticket = null,
  }: Props = $props();

  interface PopupMenu {
    x: number;
    y: number;
    above?: boolean;
    label: string;
    items: MenuItem[];
  }

  const restored = untrack(() => draft);

  // Read by the panel on each refresh.
  let changes = $state.raw<FileChange[]>([]);
  let lists = $state.raw<Changelists | null>(null);
  let inclusion = $state.raw<Inclusion>(
    restored?.inclusion ?? { included: new Set(), known: new Set() },
  );
  let lineSelections = $state.raw<Selections>(restored?.lineSelections ?? new Map());
  // The hunks of what is chosen or split across lists, up to date with the last refresh.
  let fileHunksKnown = new Map<string, Hunk[]>();
  // Where each chosen hunk was, to say what was lost when it no longer exists.
  const places = new Map<string, string>(restored?.places ?? []);

  // The last merge message that was offered: it is not offered again if it is deleted on purpose.
  let proposedMerge: string | null = null;

  let message = $state(restored?.message ?? "");
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
  let signOff = $state(storedSignOff(untrack(() => repo.path)));
  let runHooks = $state(true);
  let committed = $state(false);
  let problem = $state<string | null>(null);
  let committing = $state(false);
  let focused = $state(false);

  let optionsOpen = $state(false);
  let menu = $state.raw<PopupMenu | null>(null);
  let messageHeight = $state(Number(stored(SPLIT_KEY)) || 220);
  let root: HTMLElement;
  let panel = $state<ChangesPanel>();
  let gate: WarningsGate;
  let editor: CommitMessage;
  // A restored draft already has its checkboxes: the first refresh keeps them as any other.
  let initialized = Boolean(restored);
  let destroyed = false;

  const hasHead = $derived(repo.head !== null || committed);
  const groups = $derived(groupStaging(changes));
  const partialFiles = $derived(new Set(lineSelections.keys()));
  const includedFiles = $derived(
    new Set(
      stagingArea
        ? groups.staged.map((change) => change.path)
        : [...inclusion.included].map(pathOfKey),
    ),
  );
  const legendItems = $derived(legend(changes, includedFiles));

  function activePaths(): string[] {
    return panel?.activePaths() ?? [];
  }

  function unversionedPaths(): string[] {
    return panel?.unversionedPaths() ?? [];
  }

  function notify(kind: "info" | "warning" | "error", text: string, action?: { label: string; run: () => void }) {
    panel?.notify(kind, text, action);
  }

  async function reload() {
    await panel?.refresh();
  }

  export function saveDraft(): CommitDraft {
    return { message, inclusion, lineSelections, places: new Map(places) };
  }

  // The window closes or the app quits (§5.8): the message, to be kept as the active changelist's
  // comment, where the Commit Changes dialog keeps its draft and where this view starts from. An
  // emptied message (committed, say) empties the comment too, or the old draft would come back.
  // Nothing when they already agree, nor with the staging area, which leaves the changelists alone.
  // During a Fixup… or Squash Into… it is the draft from before the proposal: the proposed
  // `fixup! …` text is not the user's.
  export function changelistDraft(): { id: string; comment: string } | null {
    if (stagingArea || !lists) return null;
    const active = lists.active;
    const comment = lists.lists.find((list) => list.id === active)?.comment ?? "";
    const draft = proposal?.prior.message ?? message;
    if (draft === comment || (bare(draft) && !comment.trim())) return null;
    return { id: active, comment: bare(draft) ? "" : draft };
  }

  // Ramify's own: nothing but the ticket counts as no message. An empty message gets the ticket of
  // the branch, and a message that is still the old branch's ticket follows a checkout; one being
  // amended or proposed (Fixup…, Squash Into…) is left alone.
  const bare = (text: string) => isBare(text, ticket);
  let prefilledWith = "";
  $effect(() => {
    const prefix = ticket ? `${ticket} ` : "";
    untrack(() => {
      if (!amend && !proposal && (message === "" || message === prefilledWith)) message = prefix;
      prefilledWith = prefix;
    });
  });

  // `ChangeListCommitMessagePolicy`: the message starts from the active changelist's comment, and
  // takes the new one's whenever the active list changes while the message is empty.
  let commentListSeen: string | null = null;
  function adoptComment(read: Changelists) {
    if (stagingArea || read.active === commentListSeen) return;
    commentListSeen = read.active;
    if (!bare(message) || proposal) return;
    const comment = read.lists.find((list) => list.id === read.active)?.comment ?? "";
    if (comment.trim()) message = comment;
  }

  function refreshed({ status, lists: read, hunks }: Refreshed) {
    void proposeMergeMessage();
    void checkProposal();
    if (!read) return;
    adoptComment(read);
    fileHunksKnown = hunks;
    const active = activePaths();
    if (initialized) {
      const parts = status.flatMap((change) =>
        partLists(read, change.path).map((id) => partKey(change.path, id)),
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
    if (!amend && bare(message)) message = text;
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
    for (const lost of findLost(lineSelections, paths, hunks)) {
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
    panel?.focusTree();
  }

  // Without paths, the Rollback of the selection; with them (the Git file menu), of their changes.
  export function rollback(paths?: string[]) {
    panel?.requestRollback(paths);
  }

  // After Shelve Changes from the dialog: the list it shelved from goes if it was left empty.
  export async function removeIfEmpty(id: string) {
    await panel?.removeIfEmpty(id);
  }

  export function checkin() {
    if (!stagingArea) inclusion = activateInclusion(inclusion, activePaths(), unversionedPaths());
    problem = null;
    editor.focus();
  }

  // `Commit File…` of the Git file menu: a folder stands for the changes under it.
  export function checkinFiles(paths: string[]) {
    const request = { kind: "files" as const, paths, lists: [] };
    commitFiles(expandPaths(request, activePaths(), panel?.unversionedPaths() ?? []).paths);
  }

  function commitFiles(paths: string[]) {
    inclusion = forceInclusion(paths, activePaths());
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

  function showDiff(file: FileChange, comparison?: StageComparison) {
    onshowdiff?.(file, comparison);
  }

  // What was chosen in these files was undone on purpose: it has not been lost.
  function rolledBack(paths: string[]) {
    paths.filter((path) => lineSelections.has(path)).forEach(forgetSelection);
  }

  function missingMessage(): string | null {
    if (stagingArea && groups.conflicts.length > 0) return "Resolve conflicts in order to commit";
    const nothing = stagingArea ? groups.staged.length === 0 : inclusion.included.size === 0;
    const noChanges = !amend && nothing;
    const noMessage = bare(message);
    const choose = stagingArea ? "Stage files to commit" : "Select files to commit";
    if (noChanges && noMessage) return `${choose} and specify commit message`;
    if (noChanges) return choose;
    if (noMessage) return "Specify commit message";
    return null;
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
    message = ticket ? `${ticket} ` : "";
    prefilledWith = message;
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
  async function commitIndex(label: string, stillValid?: () => Promise<boolean>): Promise<boolean> {
    committing = true;
    const count = groups.staged.length;
    const text = message;
    let done = false;
    try {
      if (!(await gate.confirm(await commitWarningsStaged(), label))) return false;
      if (stillValid && !(await stillValid())) return false;
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

  // What a commit of the checkboxes sends now, or nothing if a file split across lists cannot be read.
  function chooseNow() {
    const chosen = chooseCommit({
      changes,
      included: inclusion.included,
      lists,
      hunks: fileHunksKnown,
      lineSelections,
      message,
      options: currentOptions(),
    });
    if (!("unreadable" in chosen)) return chosen;
    notify("error", unreadableText(chosen.unreadable));
    return null;
  }

  // `true` if the commit was made: `Commit and Push…` pushes only then.
  // `stillValid` runs after the checks, before anything is committed: a warning can stay open for long.
  async function commit(label = commitLabel(), stillValid?: () => Promise<boolean>): Promise<boolean> {
    if (committing) return false;
    problem = missingMessage();
    if (problem) return false;
    storeSignOff(repo.path, signOff);
    if (stagingArea) return commitIndex(label, stillValid);
    const checked = chooseNow();
    if (!checked) return false;
    committing = true;
    let done = false;
    try {
      const warnings = await commitWarnings(checked.request, checked.selections);
      if (!(await gate.confirm(warnings, label))) return false;
      if (stillValid && !(await stillValid())) return false;
      // What the checks saw may have changed while a warning was open: the commit takes what there is
      // now, as IntelliJ builds the commit state after the checks. With nothing left, nothing is sent.
      const chosen = chooseNow();
      if (!chosen || chosen.files.size === 0) return false;
      const { request, selections, files } = chosen;
      const sent = lineSelections;
      // A `repo-changed` can change the tree before the commit responds.
      const count = files.size;
      const text = message;
      const touched = panel?.inactiveListsWith(inclusion.included) ?? [];
      if (selections.length > 0) await commitChangesSelected(request, selections);
      else await commitChanges(request);
      done = true;
      finishCommit(count, text, files);
      // A new selection in the diff window made while the commit was running was not part of it.
      selections.forEach(({ path }) => {
        if (lineSelections.get(path) === sent.get(path)) forgetSelection(path);
      });
      oncommitted?.();
      committing = false;
      await reload();
      panel?.offerToRemoveEmpty(touched);
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
    // Read before the checks and again after them, because a warning can stay open for long and the
    // rebase takes no expected HEAD.
    const stillValid = async () => {
      const state = await repoState();
      if (destroyed || proposal !== current) return false;
      if (!stale(state, current)) return true;
      dropStaleProposal(state, current);
      return false;
    };
    try {
      if (!(await stillValid())) return;
      if ((await commit("Commit and Rebase", stillValid)) && !destroyed) await onrebase(base);
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
      default:
        return false;
    }
    return true;
  }

  // The actions of the message and the commit also work while typing the message. Those of the
  // tree are the panel's.
  function keydown(event: KeyboardEvent) {
    if (!active || optionsOpen || menu || panel?.hasPopup()) return;
    const action = matchShortcut(event);
    if (action && perform(action)) event.preventDefault();
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

  onDestroy(() => {
    destroyed = true;
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

  <ChangesPanel
    bind:this={panel}
    bind:changes
    bind:lists
    {repo}
    {active}
    checkable
    toolbar="horizontal"
    {stagingArea}
    included={inclusion.included}
    partial={partialFiles}
    ontoggle={toggle}
    hunksOf={() => lineSelections.keys()}
    onrefresh={refreshed}
    onopen={showDiff}
    onshowdiff={showDiff}
    oncheckinfiles={commitFiles}
    onrolledback={rolledBack}
    {onresolveconflicts}
    {onconflictschange}
    suspended={optionsOpen || menu !== null}
  />

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
      onoptions={() => (optionsOpen = true)}
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
</section>

{#if menu}
  <Menu {...menu} onclose={() => (menu = null)} />
{/if}

<WarningsGate bind:this={gate} repo={repo.path} />

{#if optionsOpen}
  <OptionsDialog
    bind:author
    bind:signOff
    bind:runHooks
    onclose={() => (optionsOpen = false)}
  />
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
</style>
