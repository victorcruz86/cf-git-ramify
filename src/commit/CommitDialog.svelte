<script lang="ts">
  import { onDestroy, onMount, tick, untrack } from "svelte";
  import {
    changelistsSetComment,
    commitChanges,
    commitChangesSelected,
    commitWarnings,
    headMessage,
    lastCommitMessage,
    mergeMessage,
    messagesList,
    messagesPush,
    repoHasCommitHooks,
    repoState,
    type ApiError,
    type ChangeList,
    type Changelists,
    type CommitOptions,
    type CommitRequest,
    type FileChange,
    type FileSelection,
    type Hunk,
    type LostSelection,
    type PublishedSelection,
    type RepoInfo,
    type SelectionChange,
  } from "../lib/api";
  import { errorText, lostText } from "../lib/errors";
  import Dialog from "../lib/Dialog.svelte";
  import Icon from "../lib/Icon.svelte";
  import { matchShortcut } from "../lib/keymap";
  import Menu, { type MenuItem } from "../lib/Menu.svelte";
  import { notify } from "../lib/notifications";
  import { store, stored } from "../lib/storage";
  import { runLocal } from "../lib/tasks";
  import FilePane from "../diff/FilePane.svelte";
  import { loadOptions, storeWrap } from "../diff/options";
  import { commitAndPush } from "../push/commitAndPush";
  import ChangelistChooser from "./ChangelistChooser.svelte";
  import ChangesPanel, { type Refreshed } from "./ChangesPanel.svelte";
  import type { CheckinRequest } from "./checkin";
  import CommitMessage from "./CommitMessage.svelte";
  import CommitOptionsPanel from "./CommitOptionsPanel.svelte";
  import { historyMenu } from "./messageHistory";
  import { storedSignOff, storeSignOff } from "./options";
  import {
    canCommit,
    DETAILS_PROPORTION_KEY,
    DETAILS_SHOWN_KEY,
    dialogTexts,
    expandPaths,
    initialInclusion,
    initialList,
    initialMessage,
    knownMessage,
    onClosed,
    onListChanged,
    SIZE_KEY,
    SPLITTER_KEY,
    beforeExecutor,
    type Executor,
    type ExecutorChanges,
    type ExecutorRun,
  } from "./dialog";
  import { toggleInclusion } from "./inclusion";
  import {
    chooseCommit,
    committedText,
    dropLostHunks,
    findLost,
    legend,
    unreadableText,
    type Selections,
  } from "./request";
  import { listFiles, listKey, pathOfKey, type TreeNode } from "./tree";
  import WarningsGate from "./WarningsGate.svelte";

  const DEFAULT_SIZE = { width: 900, height: 700 };
  const MIN_SIZE = { width: 560, height: 420 };
  // `SPLITTER_PROPORTION_OPTION_DEFAULT` and `DETAILS_SPLITTER_PROPORTION_OPTION_DEFAULT`; a stored
  // proportion out of 0.05–0.95 falls back to the default (`getSplitterInitialProportion`).
  const SPLITTER_DEFAULT = 0.5;
  const DETAILS_DEFAULT = 0.6;
  const FOCUSABLE =
    "input:not(:disabled), textarea:not(:disabled), button:not(:disabled), [tabindex='0']";

  interface Props {
    repo: RepoInfo;
    request: CheckinRequest;
    executor: Executor;
    prefilled?: string;
    // `Commit and Rebase…` of Fixup… and Squash Into…: the base (`<hash>~`) and the HEAD that was seen when
    // proposing it. The executor is only valid for that state.
    rebaseFrom?: string;
    head?: string;
    // The text of the branch widget, shown under the tree (`CurrentBranchComponent`).
    branch: string;
    // Without commits there is nothing to amend.
    hasHead: boolean;
    // `Clear initial commit message`.
    clearInitial: boolean;
    // Shelve and Create Patch: what to do with the changes. It closes the dialog when it goes ahead.
    onexecute?: ExecutorRun;
    onshowdiff?: (file: FileChange) => void;
    // `Commit and Push…`: called before the commit, and what it returns is pushed after it.
    onpreparepush?: () => () => Promise<void>;
    // `Commit and Rebase…`: the rebase from the base, once the commit is made.
    onrebase?: (base: string) => Promise<void>;
    oncommitted?: (list: ChangeList) => void;
    onselectionlost?: (lost: LostSelection) => void;
    // What the diff window shows while the dialog is open: it owns the line checkboxes.
    onselectionstate?: (state: PublishedSelection) => void;
    // Whether `path` is still the open repository: the commit finishes after the dialog has closed, and
    // what follows it is only done in the repository it was made in.
    sameRepo?: (path: string) => boolean;
    onclose: () => void;
  }

  let {
    repo,
    request,
    executor,
    prefilled,
    rebaseFrom,
    head,
    branch,
    hasHead,
    clearInitial,
    onexecute,
    onshowdiff,
    onpreparepush,
    onrebase,
    oncommitted,
    onselectionlost,
    onselectionstate,
    sameRepo,
    onclose,
  }: Props = $props();

  const texts = $derived(dialogTexts(executor));
  let options = $state.raw(loadOptions("CommitDialog"));

  function setWrap(wrap: boolean) {
    options = { ...options, wrap };
    storeWrap("CommitDialog", wrap);
  }

  let changes = $state.raw<FileChange[]>([]);
  let lists = $state.raw<Changelists | null>(null);
  // The list the dialog commits, chosen on the first refresh.
  let listId = $state<string | null>(null);
  // Checkbox keys, as in the tree: a path, or the part key of a file split across lists. The keys of
  // the other lists stay, for when the chooser comes back to them.
  let included = $state.raw(new Set<string>());
  let lineSelections = $state.raw<Selections>(new Map());
  const places = new Map<string, string>();
  let hunksKnown = new Map<string, Hunk[]>();
  // The drafts written to other lists while the dialog is open: their comments are read from here.
  const drafts = new Map<string, string>();

  let message = $state("");
  // `lastKnownComment`, and the text the dialog opened with (`AmendCommitHandler.initialMessage`).
  let known: string | undefined;
  let openedWith: string | null = null;
  let amend = $state(false);
  let amendRestore: { before: string; loaded: string } | null = null;
  let author = $state("");
  // `GitVcsSettings.SIGN_OFF_COMMIT` seeds it.
  let signOff = $state(storedSignOff(untrack(() => repo.path)));
  let runHooks = $state(true);
  let hooks = $state(false);
  // The checks run: the buttons wait.
  let checking = $state(false);
  let emptyMessage = $state(false);
  // The base of `Commit and Rebase…` while it is still valid.
  let rebaseBase = $state(untrack(() => rebaseFrom));
  interface PopupMenu {
    x: number;
    y: number;
    above?: boolean;
    label: string;
    items: MenuItem[];
  }
  let menu = $state.raw<PopupMenu | null>(null);
  let lead = $state<string | null>(null);
  let version = $state(0);

  let size = $state(storedSize());
  let splitter = $state(storedProportion(SPLITTER_KEY, SPLITTER_DEFAULT));
  let details = $state(storedProportion(DETAILS_PROPORTION_KEY, DETAILS_DEFAULT));
  let detailsShown = $state(stored(DETAILS_SHOWN_KEY) !== "false");

  let box: HTMLDivElement;
  let main: HTMLDivElement;
  let content: HTMLDivElement;
  let top: HTMLDivElement;
  let detailsPane = $state<HTMLElement>();
  let historyButton: HTMLButtonElement;
  let arrowButton = $state<HTMLButtonElement>();
  let panel = $state<ChangesPanel>();
  let editor: CommitMessage;
  let gate: WarningsGate;
  let destroyed = false;
  const previous = document.activeElement;
  // `MERGE_MSG` or `SQUASH_MSG`, and `LAST_COMMIT_MESSAGE`, read while the first refresh comes.
  const sources = Promise.all([
    mergeMessage().catch(() => null),
    lastCommitMessage().catch(() => null),
  ]);

  const current = $derived(lists?.lists.find((list) => list.id === listId) ?? null);
  const filesOfLists = $derived(lists ? listFiles(changes, lists) : new Map<string, string[]>());
  const versionedKeys = $derived.by(() => {
    const read = lists;
    const id = listId;
    return read && id ? (filesOfLists.get(id) ?? []).map((path) => listKey(read, id, path)) : [];
  });
  const unversionedPaths = $derived(
    texts.unversioned
      ? changes.filter((change) => change.state === "untracked").map((change) => change.path)
      : [],
  );
  const shownKeys = $derived(new Set([...versionedKeys, ...unversionedPaths]));
  // What the commit takes: only what the dialog shows.
  const chosenKeys = $derived(new Set([...included].filter((key) => shownKeys.has(key))));
  const legendItems = $derived(legend(changes, new Set([...chosenKeys].map(pathOfKey))));
  const enabled = $derived(canCommit(chosenKeys) && !checking);
  const partial = $derived(new Set(lineSelections.keys()));
  const amendAvailable = $derived(hasHead);
  const diffPath = $derived(lead ?? (versionedKeys[0] ? pathOfKey(versionedKeys[0]) : null));

  function storedSize(): { width: number; height: number } {
    try {
      const saved = JSON.parse(stored(SIZE_KEY) ?? "null");
      if (Number(saved?.width) > 0 && Number(saved?.height) > 0) {
        return { width: Number(saved.width), height: Number(saved.height) };
      }
    } catch {
      // A damaged value: the first size again.
    }
    return DEFAULT_SIZE;
  }

  function storedProportion(key: string, fallback: number): number {
    const value = Number(stored(key));
    return value > 0.05 && value < 0.95 ? value : fallback;
  }

  // The comment of a list, with what this dialog already wrote to it.
  function withDraft(list: ChangeList): ChangeList {
    const draft = drafts.get(list.id);
    return draft === undefined ? list : { ...list, comment: draft };
  }

  function saveComment(save: { id: string; comment: string } | null, path = repo.path) {
    if (!save) return;
    changelistsSetComment(save.id, save.comment, path).catch((failure) =>
      notify({ kind: "error", title: errorText(failure) }),
    );
  }

  // Every key there is now, in any list: what is not here leaves the inclusion.
  function allKeys(status: FileChange[], read: Changelists): Set<string> {
    const files = listFiles(status, read);
    const keys = read.lists.flatMap((list) =>
      (files.get(list.id) ?? []).map((path) => listKey(read, list.id, path)),
    );
    const untracked = status.filter((change) => change.state === "untracked");
    return new Set([...keys, ...untracked.map((change) => change.path)]);
  }

  function refreshed({ status, lists: read, hunks }: Refreshed) {
    version++;
    if (!read) return;
    hunksKnown = hunks;
    if (listId === null) {
      start(status, read);
    } else {
      // `ChangeListChooser.setAvailableLists`: a list that is gone gives way to the first one.
      if (!read.lists.some((list) => list.id === listId)) {
        listId = read.lists[0]?.id ?? null;
        lineSelections = new Map();
      }
      // Review Focus 1: a file that is gone leaves the inclusion; a new one comes in unchecked.
      const all = allKeys(status, read);
      included = new Set([...included].filter((key) => all.has(key)));
    }
    dropLostSelections(new Set(status.map((change) => change.path)), hunks);
  }

  // `CheckinActionUtil` and `SingleChangeListCommitMessagePolicy.init`, once the lists are known.
  function start(status: FileChange[], read: Changelists) {
    const files = listFiles(status, read);
    const withFiles = read.lists.map((list) => ({ ...list, files: files.get(list.id) ?? [] }));
    const unversioned = texts.unversioned
      ? status.filter((change) => change.state === "untracked").map((change) => change.path)
      : [];
    const wanted = expandPaths(request, [...new Set([...files.values()].flat())], unversioned);
    const id = initialList(wanted, withFiles, read.active);
    const list = withFiles.find((candidate) => candidate.id === id);
    if (!list) return;
    listId = id;
    included = new Set(
      [...initialInclusion(wanted, list, unversioned)].map((path) => listKey(read, id, path)),
    );
    void sources.then(([merge, last]) => {
      if (destroyed) return;
      const input = {
        prefilled,
        list,
        mergeMessage: merge?.trim() ? merge : undefined,
        lastMessage: last ?? undefined,
        clearInitial,
      };
      // What the user typed in the meantime stays.
      if (message === "") message = initialMessage(input);
      known = clearInitial ? prefilled : knownMessage(input);
      openedWith = message;
    });
  }

  function dropLostSelections(paths: Set<string>, hunks: Map<string, Hunk[]>) {
    for (const lost of findLost(lineSelections, paths, hunks)) {
      const where = lost.hunks.flatMap((hunk) => places.get(hunk) ?? []);
      const text = lostText(lost, where);
      lineSelections = dropLostHunks(lineSelections, lost);
      if (!lineSelections.has(lost.path)) exclude(lost.path);
      panel?.notify("warning", text);
      onselectionlost?.({ ...lost, text });
    }
  }

  function keyOf(path: string): string {
    return lists && listId ? listKey(lists, listId, path) : path;
  }

  function exclude(path: string) {
    const key = keyOf(path);
    if (included.has(key)) included = new Set([...included].filter((other) => other !== key));
  }

  // Unchecking a file removes what was chosen from it; checking it again means the whole file.
  function dropUnchecked() {
    const kept = [...lineSelections].filter(([path]) => included.has(keyOf(path)));
    if (kept.length !== lineSelections.size) lineSelections = new Map(kept);
  }

  // `Add to VCS` in the browser: the files are in the dialog's list now, and included.
  function include(paths: string[]) {
    if (paths.length > 0) included = new Set([...included, ...paths.map(keyOf)]);
  }

  function toggle(targets: TreeNode[]) {
    included = toggleInclusion(included, [...new Set(targets.flatMap((node) => node.paths))]);
    dropUnchecked();
  }

  // The line checkboxes of the Diff panel.
  function selectionChanged(change: SelectionChange) {
    const next = new Map(lineSelections);
    if (change.selection.length > 0) next.set(change.path, change.selection);
    else next.delete(change.path);
    lineSelections = next;
    for (const [hunk, place] of Object.entries(change.places)) places.set(hunk, place);
    const key = keyOf(change.path);
    if (change.included === included.has(key)) return;
    const paths = new Set(included);
    if (change.included) paths.add(key);
    else paths.delete(key);
    included = paths;
  }

  // The diff window chooses lines of a file of the dialog: the same as the Diff panel.
  export function setLineSelection(change: SelectionChange) {
    selectionChanged(change);
  }

  // The window knows paths: the keys of the list the dialog commits, as paths.
  $effect(() => {
    onselectionstate?.({
      included: [...chosenKeys].map(pathOfKey),
      selections: Object.fromEntries(lineSelections),
      lists,
    });
  });

  function rolledBack(paths: string[]) {
    const kept = [...lineSelections].filter(([path]) => !paths.includes(path));
    if (kept.length !== lineSelections.size) lineSelections = new Map(kept);
  }

  // `onChangelistChanged`. The line exclusions go with the list (`resetExcludedFromCommitMarkers`).
  function chooseList(id: string) {
    const from = current;
    const to = lists?.lists.find((list) => list.id === id);
    if (!from || !to || id === listId) return;
    listId = id;
    lineSelections = new Map();
    if (clearInitial || from.name === to.name) return;
    const { save, message: next } = onListChanged(message, withDraft(from), withDraft(to), known);
    drafts.set(save.id, save.comment);
    saveComment(save);
    message = next;
  }

  function sameText(left: string, right: string): boolean {
    return left.replace(/\s+/g, "") === right.replace(/\s+/g, "");
  }

  // `AmendCommitHandlerImpl`: the last commit's message comes in only if the text is still the one
  // the dialog opened with, and goes away when amend is turned off if nobody touched it. The title
  // and the button stay.
  async function toggleAmend() {
    if (!amendAvailable || checking) return;
    if (amend) {
      amend = false;
      if (amendRestore && message === amendRestore.loaded) message = amendRestore.before;
      amendRestore = null;
      return;
    }
    amend = true;
    if (openedWith !== null && message !== openedWith) return;
    try {
      const loaded = await headMessage();
      if (destroyed || !amend || (openedWith !== null && message !== openedWith)) return;
      if (!loaded.trim() || sameText(message, loaded)) return;
      amendRestore = { before: message, loaded };
      // `setAmendMessage`: the text written so far goes to the history before it is replaced.
      messagesPush(message).catch(() => {});
      message = loaded;
      editor.focus();
    } catch (failure) {
      amend = false;
      panel?.notify("error", errorText(failure));
    }
  }

  async function showHistory() {
    if (checking) return;
    const bounds = historyButton.getBoundingClientRect();
    try {
      const messages = await messagesList();
      const items = historyMenu(messages, (text) => {
        message = text;
        // `onCommitMessageReset`.
        known = text;
        tick().then(() => editor.focus());
      });
      menu = { x: bounds.left, y: bounds.bottom + 2, label: "Commit Message History", items };
    } catch (failure) {
      panel?.notify("error", errorText(failure));
    }
  }

  function openActions() {
    if (!arrowButton || !enabled) return;
    const bounds = arrowButton.getBoundingClientRect();
    const items: MenuItem[] = [
      {
        label: texts.dropdown[0],
        shortcut: "⌥⌘K",
        disabled: !onpreparepush,
        run: () => void execute("push"),
      },
    ];
    if (rebaseBase) {
      items.push({
        label: "Commit and Rebase…",
        disabled: !onrebase,
        run: () => void execute("rebase"),
      });
    }
    menu = { x: bounds.left, y: bounds.top - 4, above: true, label: "Commit Actions", items };
  }

  // The executor was valid for the state that was seen when proposing: no operation in progress and the
  // same HEAD (without the HEAD that was seen nothing can be said, so it is not valid). If it is not, it
  // goes away and nothing is committed.
  async function rebaseStillValid(): Promise<boolean> {
    try {
      const state = await repoState();
      if (state.operation === null && head !== undefined && state.head === head) return true;
    } catch (failure) {
      panel?.notify("error", errorText(failure));
      return false;
    }
    rebaseBase = undefined;
    panel?.notify(
      "warning",
      "The repository has changed since Fixup… or Squash Into…: Commit and Rebase… is no longer offered",
    );
    return false;
  }

  function currentOptions(): CommitOptions {
    return {
      amend,
      signOff,
      // `GitSkipHooksConfigurationPanel.saveState`: only skipped when the checkbox is shown.
      skipHooks: hooks && !runHooks,
      author: author.trim() || null,
      cleanup: false,
    };
  }

  // `GitCommitOptionsUi.saveState`, which runs when a commit is attempted and, as a list-specific
  // option, on Cancel.
  function saveOptions() {
    if (texts.options) storeSignOff(repo.path, signOff);
  }

  function cancel() {
    saveOptions();
    if (current) {
      const versioned = versionedKeys.every((key) => chosenKeys.has(key));
      saveComment(onClosed({ committed: false, draft: message, list: current, fullyIncluded: versioned }));
    }
    onclose();
  }

  // `SingleChangeListCommitWorkflowHandler`: an empty message is an error dialog; the checks run with
  // the dialog open, and a warning that is cancelled keeps it open. Once they pass, the dialog closes
  // and then the commit runs, with its progress in the task status and its outcome as a notification.
  async function execute(how: "commit" | "push" | "rebase") {
    if (!enabled || !current || !lists) return;
    if (!message.trim()) {
      emptyMessage = true;
      return;
    }
    const base = how === "rebase" ? rebaseBase : undefined;
    if (how === "rebase") {
      if (!base || !onrebase) return;
      checking = true;
      const valid = await rebaseStillValid().finally(() => (checking = false));
      if (!valid || destroyed) return;
    }
    saveOptions();
    const checked = chooseNow();
    if (!checked) return;
    if (executor !== "commit") {
      await executeOther(checked);
      return;
    }
    const label =
      how === "push" ? "Commit and Push" : how === "rebase" ? "Commit and Rebase" : "Commit";
    checking = true;
    let proceed = false;
    try {
      const warnings = await commitWarnings(checked.request, checked.selections);
      // A gate that is gone never answers.
      if (destroyed) return;
      proceed = await gate.confirm(warnings, label);
    } catch (failure) {
      panel?.notify("error", errorText(failure));
    } finally {
      checking = false;
    }
    if (!proceed || destroyed) return;
    // A warning can stay open for long, and the rebase takes no expected HEAD: it is checked again.
    if (base) {
      checking = true;
      const valid = await rebaseStillValid().finally(() => (checking = false));
      if (!valid || destroyed) return;
    }
    // The tree may have changed while a warning was open: the commit takes what the dialog shows now,
    // as IntelliJ builds the commit state after the checks. With nothing left the dialog stays.
    const chosen = chooseNow();
    const list = current;
    if (!chosen || !list || chosen.files.size === 0) return;
    const fullyIncluded = versionedKeys.every((key) => chosenKeys.has(key));
    const draft = message;
    const push = how === "push" ? onpreparepush?.() : undefined;
    const rebase = base ? onrebase : undefined;
    const finished = oncommitted;
    const lostTo = onselectionlost;
    const where = new Map(places);
    // The props are gone once the dialog has closed (the application hands them over from the store).
    const title = texts.title;
    // Another repository may be opened while the commit runs: the draft, the lists, the push and the
    // rebase are those of the repository the commit was made in, and are left alone if it is no longer
    // open. Its outcome is still notified.
    const path = repo.path;
    const isOpen = sameRepo;
    const here = () => isOpen?.(path) ?? true;
    onclose();
    const commit = () =>
      commitInBackground(chosen, { title, list, fullyIncluded, draft, where, path, here, finished, lostTo });
    if (push) {
      commitAndPush(commit, push, here).catch((failure) =>
        notify({ kind: "error", title: errorText(failure) }),
      );
    } else if (base && rebase) {
      // `GitRebaseCheckinHandlerFactory.checkinSuccessful`: the rebase runs only if the commit was made.
      commit()
        .then((made) => (made && here() ? rebase(base) : undefined))
        .catch((failure) => notify({ kind: "error", title: errorText(failure) }));
    } else {
      void commit();
    }
  }

  // Shelve and Create Patch (decision 8: no commit checks). The message goes to the history and,
  // with the whole list in, to its comment, before the executor runs; the executor closes the dialog
  // when it goes ahead. Whatever it throws is notified. It is told the repository it works on and
  // whether that one is still open.
  async function executeOther(checked: { selections: FileSelection[]; files: Set<string> }) {
    const run = onexecute;
    const list = current;
    if (!run || !list || checked.files.size === 0) return;
    const draft = message;
    const fullyIncluded = versionedKeys.every((key) => chosenKeys.has(key));
    const path = repo.path;
    const isOpen = sameRepo;
    const byPath = new Map(changes.map((change) => [change.path, change]));
    const changesOut: ExecutorChanges = {
      repo: path,
      list: list.id,
      message: draft,
      files: [...checked.files].map((file) => ({
        path: file,
        oldPath: byPath.get(file)?.oldPath ?? null,
      })),
      selections: checked.selections,
    };
    messagesPush(draft).catch(() => {});
    saveComment(beforeExecutor({ draft, list, fullyIncluded }), path);
    let closed = false;
    const context = {
      close: () => {
        if (closed) return;
        closed = true;
        onclose();
      },
      here: () => isOpen?.(path) ?? true,
    };
    try {
      await run(changesOut, context);
    } catch (failure) {
      notify({ kind: "error", title: errorText(failure) });
    }
  }

  // What a commit of what the dialog shows sends now, or nothing if a file split across lists cannot be
  // read.
  function chooseNow() {
    if (!lists) return null;
    const chosen = chooseCommit({
      changes,
      included: chosenKeys,
      lists,
      hunks: hunksKnown,
      lineSelections,
      message,
      options: currentOptions(),
    });
    if (!("unreadable" in chosen)) return chosen;
    panel?.notify("error", unreadableText(chosen.unreadable));
    return null;
  }

  // After the dialog has closed: nothing here touches its state.
  async function commitInBackground(
    chosen: { request: CommitRequest; selections: FileSelection[]; files: Set<string> },
    after: {
      title: string;
      list: ChangeList;
      fullyIncluded: boolean;
      draft: string;
      where: Map<string, string>;
      path: string;
      here: () => boolean;
      finished?: (list: ChangeList) => void;
      lostTo?: (lost: LostSelection) => void;
    },
  ): Promise<boolean> {
    const { request: sent, selections, files } = chosen;
    const { list, fullyIncluded, draft, where, path, here } = after;
    try {
      await runLocal(after.title, "Committing…", () =>
        selections.length > 0 ? commitChangesSelected(sent, selections) : commitChanges(sent),
      );
    } catch (failure) {
      const lost = (failure as ApiError).details;
      const text = errorText(failure, lost ? lost.hunks.flatMap((hunk) => where.get(hunk) ?? []) : []);
      if (lost && here()) after.lostTo?.({ ...lost, text });
      notify({ kind: "error", title: text });
      if (!here()) return false;
      saveComment(onClosed({ committed: false, draft, list, fullyIncluded }), path);
      return false;
    }
    notify({ kind: "info", title: committedText(files.size, sent.message) });
    if (!here()) return true;
    saveComment(onClosed({ committed: true, draft, list, fullyIncluded }), path);
    after.finished?.(list);
    return true;
  }

  function showDiff(file: FileChange) {
    onshowdiff?.(file);
  }

  function focusable(): HTMLElement[] {
    return [...box.querySelectorAll<HTMLElement>(FOCUSABLE)];
  }

  // The keys stay in the dialog. What the tree handles does not get here twice.
  function keydown(event: KeyboardEvent) {
    event.stopPropagation();
    if (event.defaultPrevented) return;
    if (event.key === "Escape") {
      event.preventDefault();
      cancel();
      return;
    }
    if (event.key === "Tab") {
      const items = focusable();
      const edge = event.shiftKey ? items[0] : items[items.length - 1];
      if (document.activeElement === edge) {
        event.preventDefault();
        (event.shiftKey ? items[items.length - 1] : items[0])?.focus();
      }
      return;
    }
    switch (matchShortcut(event)) {
      case "CommitView.Commit":
        void execute("commit");
        break;
      case "Git.Commit.And.Push.Executor":
        if (executor === "commit") void execute("push");
        break;
      case "Vcs.ToggleAmendCommitMode":
        if (texts.options) void toggleAmend();
        break;
      case "Vcs.ShowMessageHistory":
        void showHistory();
        break;
      default:
        return;
    }
    event.preventDefault();
  }

  function drag(event: PointerEvent, move: (next: PointerEvent) => void) {
    const handle = event.currentTarget as HTMLElement;
    handle.setPointerCapture?.(event.pointerId);
    const end = () => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", end);
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", end);
  }

  function clamp(value: number, low: number, high: number): number {
    return Math.max(low, Math.min(high, value));
  }

  function resizeMessage(event: PointerEvent) {
    drag(event, (next) => {
      const bounds = main.getBoundingClientRect();
      if (bounds.height > 0) splitter = clamp((next.clientY - bounds.top) / bounds.height, 0.1, 0.9);
    });
  }

  function resizeDetails(event: PointerEvent) {
    drag(event, (next) => {
      const bounds = content.getBoundingClientRect();
      if (bounds.height > 0) details = clamp((next.clientY - bounds.top) / bounds.height, 0.1, 0.9);
    });
  }

  // The dialog stays centred, so it grows on both sides.
  function resizeDialog(event: PointerEvent) {
    const start = { x: event.clientX, y: event.clientY, ...size };
    drag(event, (next) => {
      size = {
        width: Math.max(MIN_SIZE.width, start.width + 2 * (next.clientX - start.x)),
        height: Math.max(MIN_SIZE.height, start.height + 2 * (next.clientY - start.y)),
      };
    });
  }

  // `SplitterWithSecondHideable`: showing or hiding the Diff panel grows or shrinks the dialog by its
  // height.
  function toggleDetails() {
    if (detailsShown) {
      const height = detailsPane?.getBoundingClientRect().height ?? 0;
      size = { ...size, height: Math.max(MIN_SIZE.height, size.height - height) };
      detailsShown = false;
      return;
    }
    const above = top.getBoundingClientRect().height;
    const height = above > 0 ? (above * (1 - details)) / details : 0;
    size = { ...size, height: Math.min(window.innerHeight - 40, size.height + height) };
    detailsShown = true;
  }

  onMount(() => {
    editor.focus();
    repoHasCommitHooks()
      .then((found) => {
        if (!destroyed) hooks = found;
      })
      .catch(() => {});
  });

  // Saved on dispose, as IntelliJ does.
  onDestroy(() => {
    destroyed = true;
    store(SIZE_KEY, JSON.stringify(size));
    store(SPLITTER_KEY, String(splitter));
    store(DETAILS_PROPORTION_KEY, String(details));
    store(DETAILS_SHOWN_KEY, String(detailsShown));
    if (previous instanceof HTMLElement) previous.focus();
  });
</script>

<div class="backdrop">
  <div
    bind:this={box}
    class="dialog"
    role="dialog"
    aria-modal="true"
    aria-label={texts.title}
    tabindex="-1"
    style:width="{size.width}px"
    style:height="{size.height}px"
    onkeydown={keydown}
  >
    <h2>{texts.title}</h2>
    <div bind:this={content} class="content">
      <div bind:this={top} class="top pane" style:flex-grow={detailsShown ? details : 1}>
        <div bind:this={main} class="main">
          {#if lists && listId}
            <ChangelistChooser
              lists={lists.lists}
              active={lists.active}
              selected={listId}
              onchange={chooseList}
            />
          {/if}
          <div class="browser pane" style:flex-grow={splitter}>
            <ChangesPanel
              bind:this={panel}
              bind:changes
              bind:lists
              {repo}
              active
              checkable
              toolbar="horizontal"
              list={listId ?? ""}
              unversioned={texts.unversioned}
              {included}
              {partial}
              ontoggle={toggle}
              hunksOf={() => lineSelections.keys()}
              onrefresh={refreshed}
              onopen={showDiff}
              onshowdiff={showDiff}
              onselection={(_paths, _lists, path) => (lead = path)}
              onrolledback={rolledBack}
              onadded={include}
              suspended={menu !== null || emptyMessage || checking}
            />
          </div>
          <div class="browser-bottom">
            {#if executor === "commit" && branch}
              <span class="branch"><Icon name="vcs" />{branch}</span>
            {/if}
            {#if legendItems.length > 0}
              <span class="legend" aria-label="Legend">
                {#each legendItems as item (item.key)}
                  <span style:color="var(--status-{item.key})">{item.text}</span>
                {/each}
              </span>
            {/if}
          </div>
          <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
          <div
            class="splitter"
            role="separator"
            aria-orientation="horizontal"
            aria-label="Commit Message Splitter"
            onpointerdown={resizeMessage}
          ></div>
          <div class="message pane" style:flex-grow={1 - splitter}>
            <div class="separator">
              <span>Commit Message</span>
              <span class="line"></span>
              <button
                bind:this={historyButton}
                type="button"
                class="action"
                title="Commit Message History (⌃M)"
                aria-label="Commit Message History"
                disabled={checking}
                onclick={showHistory}
              >
                <Icon name="history" />
              </button>
            </div>
            <div class="editor">
              <CommitMessage bind:this={editor} bind:message editorOnly busy={checking} />
            </div>
          </div>
        </div>
        {#if texts.options}
          <CommitOptionsPanel
            bind:author
            bind:signOff
            bind:runHooks
            {amend}
            {amendAvailable}
            {hooks}
            actionName={texts.primary}
            ontoggleamend={toggleAmend}
          />
        {/if}
      </div>

      {#if detailsShown}
        <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
        <div
          class="splitter"
          role="separator"
          aria-orientation="horizontal"
          aria-label="Diff Splitter"
          onpointerdown={resizeDetails}
        ></div>
      {/if}
      <div class="separator details-title">
        <button
          type="button"
          class="toggle"
          aria-expanded={detailsShown}
          aria-controls="commit-dialog-diff"
          onclick={toggleDetails}
        >
          <Icon name={detailsShown ? "chevronDown" : "chevronRight"} />
          Diff
        </button>
        <span class="line"></span>
      </div>
      {#if detailsShown}
        <section
          bind:this={detailsPane}
          id="commit-dialog-diff"
          class="details pane"
          aria-label="Diff"
          style:flex-grow={1 - details}
        >
          {#if diffPath && lists}
            {#key diffPath}
              <FilePane
                path={diffPath}
                {version}
                {options}
                included={included.has(keyOf(diffPath))}
                selection={lineSelections.get(diffPath) ?? []}
                {lists}
                embedded
                onselection={selectionChanged}
                onnotice={(kind, text, action) => panel?.notify(kind, text, action)}
                onwrap={setWrap}
              />
            {/key}
          {/if}
        </section>
      {/if}
    </div>

    <div class="footer">
      <div class="primary">
        <button
          type="button"
          class="button default"
          class:split={texts.dropdown.length > 0}
          disabled={!enabled}
          onclick={() => void execute("commit")}
        >
          {texts.primary}
        </button>
        {#if texts.dropdown.length > 0}
          <button
            bind:this={arrowButton}
            type="button"
            class="button default arrow"
            aria-label="Other Commit Actions"
            disabled={!enabled}
            onclick={openActions}
          >
            <Icon name="chevronDown" />
          </button>
        {/if}
      </div>
      <button type="button" class="button" onclick={cancel}>Cancel</button>
    </div>
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="grip" onpointerdown={resizeDialog}></div>

    {#if menu}
      <Menu {...menu} onclose={() => (menu = null)} />
    {/if}
  </div>
</div>

<WarningsGate bind:this={gate} repo={repo.path} onclosewindow={cancel} />

{#if emptyMessage}
  <!-- `Messages.showErrorDialog` of `confirmCommitWithEmptyMessage`. -->
  <Dialog
    title="Comment Is Empty"
    width={340}
    onclose={() => {
      emptyMessage = false;
      editor.focus();
    }}
    onsubmit={() => {
      emptyMessage = false;
      editor.focus();
    }}
  >
    <p class="error"><Icon name="error" />Please specify comment</p>
    {#snippet footer()}
      <button class="button default" data-autofocus>OK</button>
    {/snippet}
  </Dialog>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 30;
    display: flex;
    align-items: center;
    justify-content: center;
    background: var(--backdrop);
  }

  .dialog {
    position: relative;
    display: flex;
    flex-direction: column;
    box-sizing: border-box;
    max-width: calc(100vw - 40px);
    max-height: calc(100vh - 40px);
    background: var(--panel);
    border: 1px solid var(--popup-border);
    border-radius: 8px;
    box-shadow: 0 8px 32px rgb(0 0 0 / 45%);
  }

  h2 {
    flex: none;
    margin: 0;
    padding: 12px 16px 8px;
    font-size: 13px;
    font-weight: 600;
    color: var(--text);
    text-align: center;
  }

  .content {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-height: 0;
    padding: 0 16px;
  }

  /* The splitters share out the height by `flex-grow`. */
  .pane {
    flex-shrink: 1;
    flex-basis: 0;
  }

  .top {
    display: flex;
    min-height: 0;
  }

  .main {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }

  .browser {
    position: relative;
    display: flex;
    min-height: 80px;
    border: 1px solid var(--divider);
    border-radius: 4px;
  }

  .browser-bottom {
    display: flex;
    flex: none;
    align-items: center;
    min-height: 24px;
    white-space: nowrap;
  }

  .branch {
    display: inline-flex;
    gap: 4px;
    align-items: center;
    padding-right: 16px;
    color: var(--text-secondary);
  }

  .legend {
    display: inline-flex;
    gap: 1.2em;
  }

  .splitter {
    flex: none;
    height: 5px;
    cursor: row-resize;
  }

  .message {
    display: flex;
    flex-direction: column;
    min-height: 80px;
  }

  .separator {
    display: flex;
    flex: none;
    gap: 8px;
    align-items: center;
    height: 26px;
    color: var(--text-secondary);
  }

  .line {
    flex: 1;
    height: 1px;
    background: var(--divider);
  }

  .editor {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-height: 0;
    border: 1px solid var(--divider);
    border-radius: 4px;
    overflow: hidden;
  }

  .toggle {
    display: inline-flex;
    gap: 4px;
    align-items: center;
    padding: 0;
    font: inherit;
    color: inherit;
    background: none;
    border: 0;
  }

  .details {
    display: flex;
    flex-direction: column;
    min-height: 60px;
    border: 1px solid var(--divider);
    border-radius: 4px;
    overflow: hidden;
  }

  .footer {
    display: flex;
    flex: none;
    gap: 8px;
    justify-content: flex-end;
    padding: 12px 16px;
  }

  .primary {
    display: flex;
  }

  .split {
    border-top-right-radius: 0;
    border-bottom-right-radius: 0;
  }

  .arrow {
    display: inline-flex;
    align-items: center;
    padding: 0 6px;
    border-left: 1px solid var(--accent-text);
    border-top-left-radius: 0;
    border-bottom-left-radius: 0;
  }

  .grip {
    position: absolute;
    right: 0;
    bottom: 0;
    width: 14px;
    height: 14px;
    cursor: nwse-resize;
  }

  .error {
    display: flex;
    gap: 8px;
    align-items: center;
    margin: 0;
  }
</style>
