<script lang="ts">
  import { onMount, tick, untrack } from "svelte";
  import {
    appVersion,
    branchList,
    changelistsLists,
    changelistsSetComment,
    commitDraftSaved,
    diffWindowOpen,
    commitMode,
    effectiveStaging,
    emitComparison,
    emitSelectionLost,
    emitSelectionState,
    gitMenuCurrentFile,
    gitMenuEnable,
    initialTarget,
    notifyRepoChanged,
    onAppActivated,
    onMenuOwnerGained,
    onAnnotationShowInLog,
    onSelectionHistoryShowAffected,
    onAppDeactivated,
    onDiffReady,
    onGitAction,
    onGitFileAction,
    onCommitDraftSave,
    onLeaveRefused,
    onOpenSettings,
    onOpenProjectChooser,
    onOpenPath,
    onOpenRecent,
    openProjectsReopen,
    projectFocusOpen,
    projectNewWindow,
    projectResolve,
    recentProjectsRemove,
    type RecentProject,
    branchIncomingOutgoing,
    onBranchSyncChanged,
    onExternalChanges,
    type BranchSync,
    onRepoChanged,
    onSelectionChanged,
    onSettingsChanged,
    perfEnabled,
    repoOpen,
    repoSettingsGet,
    repoState,
    settingsGet,
    settingsSave,
    shelfLists,
    shelfSides,
    showWindow,
    stashList,
    type ChangeList,
    type StageComparison,
    type FileChange,
    type LostSelection,
    type Operation,
    type RepoInfo,
    type RepoState,
    type PublishedSelection,
    type SelectionOwner,
    type SelectionState,
    type RepoChange,
    type Settings,
    type ShelfEntry,
    type ShelfSettings,
    type ShelfSides,
    type StashEntry,
  } from "./lib/api";
  import { labelFor, ownerOf, ownLabel } from "./lib/windows";
  import AskpassDialog from "./lib/AskpassDialog.svelte";
  import MessageEditorDialog from "./editor/MessageEditorDialog.svelte";
  import { homeDir } from "@tauri-apps/api/path";
  import { chooseFolder } from "./lib/choosers";
  import { errorText } from "./lib/errors";
  import Icon from "./lib/Icon.svelte";
  import { matchShortcut, type ActionId } from "./lib/keymap";
  import { optionsOf, ramifyOptions } from "./lib/ramifyOptions";
  import { ticketOf } from "./commit/ticket";
  import { externalNotifier } from "./lib/external";
  import { notify } from "./lib/notifications";
  import Notifications from "./lib/Notifications.svelte";
  import {
    keepSelected,
    onTabRequested,
    stripeWindows,
    toolWindows,
    type ContentId,
    type SelectedContents,
    type TabRequest,
  } from "./lib/tabs";
  import {
    activation,
    forgetToolWindows,
    onWindowActivationRequested,
    registeredToolWindows,
    toolWindow,
    windowOfAction,
    type ToolWindowId,
  } from "./shell/toolWindows";
  import { taskResult } from "./lib/tasks";
  import TaskStatus from "./lib/TaskStatus.svelte";
  import type { CheckinRequest } from "./commit/checkin";
  import CommitDialog from "./commit/CommitDialog.svelte";
  import type { Executor, ExecutorRun } from "./commit/dialog";
  import {
    closeCommitDialog,
    commitDialog,
    requestCommit,
    requestExecutor,
    routeCommitRequests,
    type CommitRequestInput,
  } from "./commit/dialogStore";
  import CommitView, { type CommitDraft } from "./commit/CommitView.svelte";
  import LocalChangesView from "./commit/LocalChangesView.svelte";
  import { showOnDoubleClickItems } from "./commit/showOnDoubleClick";
  import ConsoleView from "./console/ConsoleView.svelte";
  import TerminalView from "./terminal/TerminalView.svelte";
  import { ROW_HEIGHT } from "./log/draw";
  import LogPanel from "./log/LogPanel.svelte";
  import BranchDialogs from "./branches/BranchDialogs.svelte";
  import BranchesPopup from "./branches/BranchesPopup.svelte";
  import { loadBranchState, refreshProtectedBranches, refsTracker } from "./branches/state";
  import { checkDefaultAccount } from "./hosting/accounts";
  import LoginHost from "./hosting/LoginHost.svelte";
  import MergeRebaseWidget from "./operation/MergeRebaseWidget.svelte";
  import { gitMenuEnabled, ongoingActionFor, ongoingActions } from "./operation/operation";
  import RemotesHost from "./remotes/RemotesHost.svelte";
  import { UPDATE_ACTIONS, fetching, type UpdateAction } from "./update/flow";
  import {
    applies,
    applyInProgress,
    applying,
    clipboardWatcher,
    defaultApplyApi,
    localEntries,
    logEntries,
    patches,
    type ApplyContext,
  } from "./patch/flow";
  import PatchDialogs from "./patch/PatchDialogs.svelte";
  import { defaultShelfTabApi, shelfFlow, shelfTabFlow, showShelf } from "./shelf/flow";
  import { shelfDialogsUi } from "./shelf/dialogs";
  import { shelfTabRequest, takeShelfTabRequest } from "./shelf/request";
  import ShelfDialogs from "./shelf/ShelfDialogs.svelte";
  import ShelfTab from "./shelf/ShelfTab.svelte";
  import {
    loadedShelfSettings,
    NO_SHELF_SETTINGS,
    resetShelfSettings,
    saveShelfSetting,
    shelfSettings,
    shelfSettingsGeneration,
  } from "./shelf/settings";
  import type { ShelfSelection } from "./shelf/shelfTree";
  import { framesRunning, runPerf } from "./perf/run";
  import SettingsView from "./settings/SettingsView.svelte";
  import MissingProjectDialog from "./recent/MissingProjectDialog.svelte";
  import OpenProjectDialog from "./project/OpenProjectDialog.svelte";
  import ProjectWidget from "./project/ProjectWidget.svelte";
  import {
    isOpenElsewhere,
    openProject,
    projectName,
    type OpenAnswer,
    type OpenProjectDeps,
  } from "./project/open";
  import { showTabStep } from "./stash/flow";
  import StashTab from "./stash/StashTab.svelte";
  import VersionControlWindow from "./vcs/VersionControlWindow.svelte";
  import ProjectTree from "./tree/ProjectTree.svelte";
  import ChangesToolWindow from "./changes/ChangesToolWindow.svelte";
  import RepositoryBrowser from "./repositories/RepositoryBrowser.svelte";
  import PullRequestsWindow from "./pulls/PullRequestsWindow.svelte";
  import { checkPullRequestsWindow, showPullRequestsWindow, viewPullRequestList } from "./pulls/store";
  import GetDialog from "./changes/GetDialog.svelte";
  import { openInViewer } from "./viewer/window";
  import {
    CURRENT_FILE_ACTIONS,
    currentFileItems,
    currentFileTitle,
    fileActionRun,
    gitSubmenu,
    isFileAction,
    runInPlace,
    type FileTarget,
    type GitFileActionId,
    type GitFileSelection,
    type GitFileState,
  } from "./files/gitFileMenu";
  import { addFiles } from "./files/addFiles";
  import { showAllAffected, showHistoryOf } from "./compare/actions";
  import { emptyFilters } from "./log/filters";
  import { openLogTab } from "./log/tabKinds";
  import { nothingToCompare } from "./compare/popups";

  const PUSH_ACTION: ActionId = "Vcs.Push";
  const COMMIT_ACTION: ActionId = "CheckinProject";
  const BRANCHES_ACTION: ActionId = "Git.Branches";

  let version = $state("");
  let repoPath = $state("");
  let info = $state<RepoInfo | null>(null);
  // The tool window in front, the one before it (which comes back when the one in front hides) and, per
  // window, the tab it shows: a window comes back on the tab it had.
  let activeWindow = $state<ToolWindowId>("git");
  let previousWindow: ToolWindowId | null = null;
  let selected = $state<SelectedContents>({
    project: null,
    commit: "local-changes",
    changes: null,
    repositories: null,
    pulls: null,
    git: "local-changes",
  });
  let opening = $state(false);
  // The Project tree reads the disk and `git status --ignored` on every change: it is built the
  // first time its window comes to the front in this repository, and kept from then on.
  let projectShown = $state(false);
  let logPanel = $state<LogPanel>();
  let commitView = $state<CommitView>();
  let stashTab = $state<StashTab>();
  let contentArea = $state<HTMLDivElement>();
  // `GitStashTracker`: read again whenever the repository changes. The Stash tab exists only while there
  // are stashes (`isStashTabVisible`, with the tabs not combined).
  let stashes = $state.raw<StashEntry[]>([]);
  // `GitStashTracker.Stashes.Error`: the tab stays, with the error as its node.
  let stashError = $state<string | null>(null);
  let stashRequest = 0;
  // `ShelveChangesManager`'s lists: the Shelf tab exists while there is one (`ContentPredicate`),
  // whatever its state. They are read again whenever the repository changes.
  let shelfEntries = $state.raw<ShelfEntry[]>([]);
  let shelfRequest = 0;
  // The last read of the lists failed: the next `repo-changed` reads them again, whatever it says.
  let shelfUnread = false;
  let shelfTab = $state<ShelfTab>();
  let localChanges = $state<LocalChangesView>();
  // What the Commit view had when the mode went modal: it comes back with the non-modal mode.
  let commitDraft = $state.raw<CommitDraft | null>(null);
  let branchDialogs = $state<BranchDialogs>();
  let remotesHost = $state<RemotesHost>();
  // `info` does not change with a checkout: changing it would remount the views. This does: the branch and the
  // operation half done, which `repo-changed` makes be read again.
  let status = $state.raw<RepoState | undefined>(undefined);
  const repoNow = $derived(status ?? info?.state);
  // Modal: everything is in the `Git` window. Non-modal: Local Changes is in the `Commit` window.
  const mode = $derived(settings ? commitMode(settings) : "modal");
  // The contents that exist now.
  const available = $derived<ContentId[]>([
    "local-changes",
    ...(shelfEntries.length > 0 ? (["shelf"] as const) : []),
    ...(stashes.length > 0 || stashError !== null ? (["stash"] as const) : []),
    "log",
    "console",
    "terminal",
  ]);
  const windows = $derived(toolWindows(mode, available, $registeredToolWindows));
  const stripe = $derived(stripeWindows(windows));
  // A selected tab that went away (the last stash dropped) leaves its window on the first one.
  const shown = $derived(keepSelected(windows, selected));
  const tab = $derived(windows[activeWindow] ? shown[activeWindow] : null);
  // The Pull Requests window goes away with the last GitHub remote: if it was in front, Git comes.
  $effect(() => {
    if (activeWindow === "pulls" && !windows.pulls) front("git");
  });
  // `selectorVm` is lazy in 241: the window connects only once it has come to the front.
  $effect(() => {
    if (activeWindow === "pulls" && windows.pulls) showPullRequestsWindow();
  });
  const branch = $derived(repoNow?.branch ?? null);
  const branchLabel = $derived(repoNow?.label ?? "");
  // Ramify's own `Start the commit message with the ticket from the branch name`.
  const commitTicket = $derived($ramifyOptions.commitMessageTicket ? ticketOf(repoNow?.branch) : null);
  // A rebase leaves HEAD detached: `branch` comes from `head-name` and is the one being rebased, which already exists.
  const suggestedBranch = $derived(repoNow?.operation === "rebasing" ? "" : (branch ?? ""));
  // There are conflicted files in the changes list: turns on `Resolve Conflicts…`.
  let hasConflicts = $state(false);
  let perfStarted = false;
  let diffWindow = false;
  // What the Commit view and the open dialog last published: only the owner's reaches the diff window.
  let commitViewSelection: PublishedSelection | null = null;
  let dialogSelection: PublishedSelection | null = null;
  let commitDialogRef = $state<CommitDialog>();
  let comparison: StageComparison = "local";
  let settings = $state.raw<Settings | null>(null);
  let settingsOpen = $state(false);
  // A recent project whose path no longer exists, while its dialog asks what to do.
  let missingProject = $state<string | null>(null);
  // The Open Project dialog of the open flow, while it asks where the project goes.
  let openQuestion = $state.raw<{ name: string; answer: (answer: OpenAnswer) => void } | null>(null);
  let branchWidget = $state<HTMLButtonElement>();
  // The home folder, for the project widget's tooltip.
  let home = $state<string | null>(null);
  void (async () => {
    try {
      home = (await homeDir()).replace(/\/$/, "");
    } catch {
      // Without it the tooltip shows the whole path.
    }
  })();
  // `null` means in the center of the window, as from the Git menu.
  let branchesPopup = $state.raw<{ anchor: { x: number; y: number } | null } | null>(null);

  const ongoingHandlers = {
    abort: (operation: Operation) => void branchDialogs?.abortOperation(operation),
    continue: () => void branchDialogs?.continueRebase(),
    skip: () => void branchDialogs?.skipCommit(),
  };

  function showBranchesFromWidget() {
    if (branchesPopup) {
      branchesPopup = null;
      return;
    }
    const bounds = branchWidget?.getBoundingClientRect();
    branchesPopup = { anchor: bounds ? { x: bounds.left, y: bounds.bottom } : null };
  }

  // The start opens its initial target here, without the open flow.
  async function open() {
    if (opening || !repoPath.trim()) return;
    opening = true;
    try {
      await openHere(repoPath.trim());
    } catch (failure) {
      showError(failure);
    } finally {
      opening = false;
    }
  }

  // This Window: today's `repo_open` path. If it does not open, we stay in the current
  // repository, with whatever was half done.
  async function openHere(path: string) {
    repoPath = path;
    const opened = await repoOpen(path);
    select("commit");
    projectShown = false;
    forgetToolWindows();
    stashes = [];
    stashError = null;
    shelfEntries = [];
    resetShelfSettings();
    closeCommitDialog();
    commitDraft = null;
    diffWindow = false;
    commitViewSelection = null;
    status = undefined;
    hasConflicts = false;
    branchesPopup = null;
    info = opened;
    syncMenu(opened.state);
    treeFiles = [];
    treeChanges = false;
    branchSync = null;
    void syncRefs.note().then(readBranchSync);
    void readBranchesToCompare();
    branchDialogs?.repositoryChanged(opened);
    void loadStashes();
    void loadShelf();
    await loadBranchState();
    void checkDefaultAccount(
      () => (settingsOpen = true),
      () => info?.path === opened.path,
    );
    void checkPullRequestsWindow(() => info?.path === opened.path);
  }

  const projectDeps: OpenProjectDeps = {
    resolve: projectResolve,
    focusOpen: projectFocusOpen,
    current: () => info?.path ?? null,
    setting: () => settings?.openProjectIn ?? "ask",
    openHere,
    newWindow: projectNewWindow,
    ask: (name) =>
      new Promise((resolve) => {
        openQuestion = {
          name,
          answer: (answer) => {
            openQuestion = null;
            resolve(answer);
          },
        };
      }),
    remember: async (openProjectIn) => {
      await settingsSave({ ...(await settingsGet()), openProjectIn });
    },
    focusHere: () => showWindow(),
  };

  // Open, a typed path, Recent Projects, Clone, Create Git Repository and a folder handed to the
  // app: the open flow (`openProject`), one at a time.
  async function openTarget(target: string) {
    if (opening || !target.trim()) return;
    opening = true;
    try {
      await openProject(target.trim(), projectDeps);
    } catch (failure) {
      // Another window started opening it meanwhile, and has come to the front.
      if (!isOpenElsewhere(failure)) showError(failure);
    } finally {
      opening = false;
    }
  }

  function windowOf(id: ContentId): ToolWindowId | null {
    return stripe.find((window) => windows[window]?.includes(id)) ?? null;
  }

  function front(id: ToolWindowId) {
    if (id === "project") projectShown = true;
    if (activeWindow === id) return;
    previousWindow = activeWindow;
    activeWindow = id;
  }

  // `commit` is wherever Local Changes is: its own window or a tab of `Git`.
  function select(request: TabRequest): ContentId | null {
    const id = request === "commit" ? "local-changes" : request;
    const owner = windowOf(id);
    if (!owner) return null;
    selected[owner] = id;
    front(owner);
    return id;
  }

  async function showTab(request: TabRequest) {
    const id = select(request);
    if (id !== "local-changes" && id !== "stash" && id !== "shelf") return;
    await tick();
    if (id === "stash") stashTab?.focus();
    else if (id === "shelf") {
      // `activateView(list)`: the list asked for, once the tab shows it.
      const asked = takeShelfTabRequest();
      if (asked?.select) shelfTab?.select(asked.select);
      shelfTab?.focus();
    } else (mode === "modal" ? localChanges : commitView)?.focusTree();
  }

  // A request for the Shelf tab made before it existed (the first list just shelved) is taken
  // once the lists are read.
  $effect(() => {
    const asked = $shelfTabRequest;
    if (asked && available.includes("shelf")) untrack(() => void showTab("shelf"));
  });

  // `Vcs.Show.Shelf` and `Vcs.Show.Log` (`VcsShowToolWindowTabAction`), only while the tab is there.
  function toggleTab(id: "shelf" | "log") {
    const owner = windowOf(id);
    if (!owner) return;
    const focused = contentArea?.contains(document.activeElement) ?? false;
    const step = showTabStep(activeWindow === owner && focused, shown[owner] === id);
    if (step !== "hide") void showTab(id);
  }
  const toggleShelf = () => toggleTab("shelf");

  // The Git file menu of the main window (`Git ▸` of the Project tree, `Git > Current File`): the
  // tree's selection while the Project window is the active one. Choosing an entry of the viewer's
  // editor menu that only this window can do arrives as `git-file-action`.
  let treeFiles = $state.raw<FileTarget[]>([]);
  let treeChanges = $state(false);
  let branchesToCompare = $state(false);
  let windowFocused = $state(document.hasFocus());
  // Whether any window of Ramify has the focus (`app-activated` / `app-deactivated`).
  let appFocused = $state(document.hasFocus());

  const fileMenuState = (): GitFileState => ({
    hasChanges: treeChanges,
    conflicts: hasConflicts,
    hasCommits: (repoNow?.head ?? "") !== "",
    branchesToCompare,
    run: (id, files) => void runFileAction(id, files),
  });
  const mainFileSelection = $derived<GitFileSelection>({
    files: info && projectShown && activeWindow === "project" ? treeFiles : [],
    editor: false,
  });

  // `GitFileActionGroup.update`: while this window has the focus it says what Current File is.
  function sendCurrentFile() {
    const selection = mainFileSelection;
    const state = fileMenuState();
    gitMenuCurrentFile(currentFileTitle(selection), currentFileItems(selection, state)).catch(
      () => {},
    );
  }
  function sendNoCurrentFile() {
    const selection: GitFileSelection = { files: [], editor: false };
    gitMenuCurrentFile(currentFileTitle(selection), currentFileItems(selection, fileMenuState())).catch(
      () => {},
    );
  }
  $effect(() => {
    if (!windowFocused) return;
    sendCurrentFile();
  });

  // `GitBranchSyncStatus.calcForCurrentBranch`: the widget's Incoming and Outgoing icons. Read on
  // open, on each `repo-changed` and on `branch-sync-changed`; a read started for another branch or
  // repository is dropped, and one that fails shows nothing.
  // The status is kept with the branch it is for: after a checkout the old branch's icons do not
  // show next to the new name while its own are read.
  let branchSync = $state<{ branch: string; sync: BranchSync | null } | null>(null);
  const shownSync = $derived(
    branchSync && branchSync.branch === (repoNow?.branch ?? null) ? branchSync.sync : null,
  );
  // `repo-changed` fires for any file under the work tree: the status is read again only when the
  // refs changed, as the branches popup does.
  const syncRefs = refsTracker();
  let syncRequest = 0;
  async function readBranchSync() {
    const token = ++syncRequest;
    const name = repoNow?.branch ?? null;
    if (!info || !name) {
      branchSync = null;
      return;
    }
    let next: BranchSync | null;
    try {
      next = (await branchIncomingOutgoing())[name] ?? null;
    } catch {
      next = null;
    }
    if (token === syncRequest) branchSync = { branch: name, sync: next };
  }

  async function readBranchSyncIfRefsChanged() {
    if (await syncRefs.changed()) await readBranchSync();
  }

  // On open and on each `repo-changed`: what arrives for an earlier request is dropped.
  let branchesRequest = 0;
  async function readBranchesToCompare() {
    const token = ++branchesRequest;
    let some: boolean;
    try {
      some = !nothingToCompare(await branchList());
    } catch {
      some = false;
    }
    if (token === branchesRequest) branchesToCompare = some;
  }

  async function runFileAction(id: GitFileActionId, files: FileTarget[]) {
    const current = info;
    if (!current) return;
    try {
      if (await runInPlace(id, files)) return;
      const paths = files.map((target) => target.path);
      if (id === "CheckinFiles") await requestCommit({ request: { kind: "files", paths, lists: [] } });
      else if (id === "Git.Add") await addFiles(current.path, paths);
      else if (id === "ChangesView.Revert") await rollbackLocalChanges(paths);
      else if (id === "Git.ResolveConflicts") void branchDialogs?.resolveConflicts();
      else if (id === "Vcs.ShowTabbedFileHistory") await showHistoryOf(files);
    } catch (failure) {
      showError(failure);
    }
  }

  // From the viewer: its file, which it has checked the entry for. The targets are built as local
  // files with no status: the ids that come here (Commit File…, Add, Show History, Rollback…,
  // Resolve Conflicts…) only use the paths, and the viewer only has files.
  function fileActionFromViewer({ id, paths }: { id: string; paths: string[] }) {
    if (!isFileAction(id) || modalOpen() || !info) return;
    showWindow().catch(() => {});
    void runFileAction(
      id,
      paths.map((path) => ({ path, dir: false, state: null })),
    );
  }

  // A click on an annotated line in the viewer (`GitAnnotationAspect.showAffectedPaths`): its commit
  // selected in the Log's main tab, the first `log` tab (`VcsLogNavigationUtil.jumpToRevisionAsync`).
  // Ramify's own notice of Git changes made outside it (`src/lib/external.ts`).
  const reportExternal = externalNotifier({
    showInLog: (hash) => {
      if (modalOpen() || !info) return;
      showWindow().catch(() => {});
      void openLogTab("log", "", () => ({ name: "", filters: emptyFilters() }), hash ? { matches: () => true, select: hash } : {});
    },
    showStash: () => {
      if (modalOpen()) return;
      showWindow().catch(() => {});
      showGitStash();
    },
  });

  function annotationInLog(hash: string) {
    if (modalOpen() || !info) return;
    showWindow().catch(() => {});
    void openLogTab("log", "", () => ({ name: "", filters: emptyFilters() }), {
      matches: () => true,
      select: hash,
    });
  }

  // `Show All Affected Files` of a row of the `History for Selection` window: the commit in the
  // `Changes` window, with the file as it was there chosen.
  function affectedFromSelectionHistory({ hash, path }: { hash: string; path: string }) {
    if (modalOpen() || !info) return;
    showWindow().catch(() => {});
    showAllAffected(hash, path).catch(showError);
  }

  // On open, when `repo-changed` says a command wrote the store, and on any `repo-changed` after a read
  // that failed. What arrives for an earlier request is dropped.
  async function loadShelf() {
    const token = ++shelfRequest;
    const started = shelfSettingsGeneration();
    try {
      const [lists, repoSettings] = await Promise.all([
        taskResult<ShelfEntry[]>(await shelfLists()),
        repoSettingsGet().catch(() => null),
      ]);
      if (token !== shelfRequest) return;
      if (JSON.stringify(lists) !== JSON.stringify(shelfEntries)) shelfEntries = lists;
      // A save since the read began wins over what it read.
      if (repoSettings) loadedShelfSettings(repoSettings.shelf ?? NO_SHELF_SETTINGS, started);
      shelfUnread = false;
    } catch {
      if (token === shelfRequest) shelfUnread = true;
    }
  }

  // The Shelf tab's actions, tied to the repository they were started in.
  const shelfActions = shelfTabFlow(defaultShelfTabApi, shelfDialogsUi, {
    dialog: (context, request) => applies.unshelveWithDialog(context, request),
    silently: (context, request) => applies.unshelveSilently(context, request),
  });

  // A list chosen whole takes all its files: the files chosen in it are not a part of it.
  const partFiles = (selection: ShelfSelection) => {
    const whole = new Set(selection.exact.map((entry) => entry.list.name));
    return selection.files.filter((files) => !whole.has(files.name));
  };

  async function unshelveFromTab(selection: ShelfSelection, withDialog: boolean) {
    const context = await applyContext();
    if (!context) return;
    if (withDialog) await shelfActions.unshelve(context, selection.lists, partFiles(selection));
    else await shelfActions.unshelveSilently(context, selection.lists, partFiles(selection));
  }

  // One field at a time, over what the other writers saved (`src/shelf/settings.ts`).
  function shelfSettingsChanged(next: ShelfSettings) {
    const before = $shelfSettings;
    const changed = Object.fromEntries(
      Object.entries(next).filter(([key, value]) => before[key as keyof ShelfSettings] !== value),
    );
    void saveShelfSetting(changed).catch(showError);
  }

  const shelfTabApi = {
    sides: async (name: string, file: ShelfEntry["changes"][number]["file"], withLocal: boolean) =>
      taskResult<ShelfSides>(await shelfSides(name, file, withLocal)),
  };

  // `Git.Show.Stash` (`VcsShowToolWindowTabAction`), which `Unstash Changes…` tries first. The window is
  // active when it is in front with the focus inside. Ramify has no hidden tool window, so the step that
  // hides it leaves everything as it is. `false` if the tab is not there.
  function showGitStash(): boolean {
    const owner = windowOf("stash");
    if (!owner) return false;
    const focused = contentArea?.contains(document.activeElement) ?? false;
    const step = showTabStep(activeWindow === owner && focused, shown[owner] === "stash");
    if (step !== "hide") void showTab("stash");
    return true;
  }

  // On open and when the watcher says the stash changed. What arrives for an earlier request is dropped.
  async function loadStashes() {
    const token = ++stashRequest;
    try {
      const list = await taskResult<StashEntry[]>(await stashList());
      if (token !== stashRequest) return;
      stashError = null;
      // The same list does not rebuild the tab.
      if (JSON.stringify(list) !== JSON.stringify(stashes)) stashes = list;
    } catch (failure) {
      if (token !== stashRequest) return;
      stashes = [];
      stashError = errorText(failure);
    }
  }

  // The selection follows what `shown` already shows, so that a tab that comes back is not selected.
  $effect(() => {
    if (shown !== selected) selected = shown;
  });

  // A window that exists comes to the front on the tab it had.
  function bringToFront(id: ToolWindowId) {
    if (!windows[id] || activeWindow === id) return;
    const content = selected[id];
    if (content) void showTab(content);
    else front(id);
  }

  // ⌘1, ⌘0 and ⌘9, or the stripe: `ActivateToolWindowAction` (`toolWindows.ts`).
  function toggleWindow(id: ToolWindowId) {
    const next = activation(id, activeWindow, previousWindow, (window) => windows[window] !== null);
    if (next) bringToFront(next);
  }

  // The mode is the only thing that moves contents between windows. Each window starts on its first tab
  // (`ChangesViewContentManager.updateToolWindowMappings`), and the `Commit` window comes to the front.
  function applySettings(next: Settings) {
    const before = mode;
    // The Commit view is unmounted in modal mode (decision 9).
    if (before === "non-modal" && commitMode(next) === "modal") {
      commitDraft = commitView?.saveDraft() ?? null;
    }
    settings = next;
    ramifyOptions.set(optionsOf(next));
    if (mode === before) return;
    selected = { ...selected, commit: windows.commit?.[0] ?? "local-changes", git: windows.git?.[0] ?? null };
    front(windows.commit ? "commit" : "git");
  }

  function showError(failure: unknown) {
    notify({ kind: "error", title: errorText(failure) });
  }

  // What a commit entry point does in non-modal mode (`requestCommit` decides): the Commit tool window comes
  // to the front and takes the request, or the message of Fixup… and Squash Into… with its executor.
  async function commitInView({ request, prefilled, rebaseFrom, head }: CommitRequestInput) {
    await showTab("commit");
    if (prefilled === undefined) {
      if (request.kind === "files") commitView?.checkinFiles(request.paths);
      else commitView?.checkin();
      return;
    }
    commitView?.propose({
      message: prefilled,
      rebaseFrom: rebaseFrom ?? null,
      head: head ?? "",
    });
  }

  const wholeProject: CheckinRequest = { kind: "project", paths: [], lists: [] };

  // After a commit from the dialog: the log is read again, and a list other than the active one
  // that was left empty is offered for removal, as after a commit from the Commit view.
  function dialogCommitted(list: ChangeList) {
    committed();
    void localChanges?.offerToRemoveEmpty(list);
  }

  // ⌘K from outside Local Changes, the Git menu and the branches popup.
  const checkinFromLog = () => requestCommit({ request: wholeProject });

  // `Create Patch…` of the Create Patch dialog: Patch File Settings, then the patch of what it
  // included, from the repository it was opened in.
  const createPatchExecutor: ExecutorRun = async (changes, context) => {
    const selections = new Map(changes.selections.map((file) => [file.path, file.selection]));
    const entries = localEntries(changes.files, selections);
    await patches.createPatch({ repo: changes.repo, entries }, changes.message, context.close);
  };

  // `Shelve Changes` of the Shelve Changes dialog: the Shelf tab with the new list, and the list it
  // shelved from deleted if it was left empty, wherever Local Changes is.
  const shelving = shelfFlow(undefined, {
    openShelf: showShelf,
    removeIfEmpty: (list) => (mode === "modal" ? localChanges : commitView)?.removeIfEmpty(list),
  });
  const shelveExecutor: ExecutorRun = (changes, context) => shelving.shelveChanges(changes, context);

  const executorRun = (executor: Executor | undefined): ExecutorRun | undefined =>
    executor === "patch" ? createPatchExecutor : executor === "shelve" ? shelveExecutor : undefined;

  function keydown(event: KeyboardEvent) {
    // The Commit Changes dialog is modal: what is underneath does not act. So is a patch being applied.
    if ($commitDialog || applyInProgress()) return;
    const action = matchShortcut(event);
    if (action === "ShowSettings") {
      settingsOpen = true;
      event.preventDefault();
      return;
    }
    if (!info) return;
    const target = windowOfAction(action);
    if (target) toggleWindow(target);
    else if (action === "CheckinProject" && tab !== "local-changes") void checkinFromLog();
    // `Commit and Push…` is an executor: IntelliJ's action works with a dialog open only, so outside it the
    // modal commit opens the dialog, where the executor is in the drop-down.
    else if (action === "Git.Commit.And.Push.Executor" && mode === "modal") void checkinFromLog();
    else if (action === "Vcs.Log.Refresh" && tab === "log") logPanel?.refresh();
    else if (action === "Vcs.UpdateProject") branchDialogs?.runUpdate(action);
    else if (action === "Vcs.Push") branchDialogs?.push();
    else return;
    event.preventDefault();
  }

  // `View Commits`: the `Update Info` tab of the log, already filtered by the paths of the settings.
  async function viewCommits(range: [string, string]) {
    await showTab("log");
    await tick();
    await logPanel?.openUpdateInfo(range, settings?.updatePaths ?? []);
  }

  // With a modal dialog open the menu does not act, like IntelliJ's: what is underneath is not touched.
  // A patch being applied counts too: its conflicts wait in the merge window, which is modal there.
  const modalOpen = () =>
    settingsOpen ||
    applyInProgress() ||
    document.querySelector('[role="dialog"][aria-modal="true"]') !== null;

  // What each action of the Git menu does, or `null` if it is not handled now: without a repository,
  // only Clone and Create Git Repository apply.
  function gitActionRunner(action: string): (() => void) | null {
    if (action === "Git.Clone") return () => void remotesHost?.clone();
    if (action === "Git.Init") return () => void remotesHost?.init();
    const current = info;
    if (!current) return null;
    const ongoing = ongoingActionFor(action);
    if (ongoing?.do === "abort") return () => void branchDialogs?.abortOperation(ongoing.operation);
    if (ongoing?.do === "continue") return () => void branchDialogs?.continueRebase();
    if (ongoing?.do === "skip") return () => void branchDialogs?.skipCommit();
    if (ongoing?.do === "resolve") return () => void branchDialogs?.resolveConflicts();
    if (action === "Git.Merge") return () => void branchDialogs?.merge();
    if (action === "Git.Rebase") return () => void branchDialogs?.rebase();
    if (action === COMMIT_ACTION) return checkinFromLog;
    if (action === PUSH_ACTION) return () => void branchDialogs?.push();
    if (action === BRANCHES_ACTION) return () => (branchesPopup = { anchor: null });
    if (action === "Git.CreateNewBranch") {
      return () => void branchDialogs?.newBranch(null, "HEAD", suggestedBranch);
    }
    if (action === "Git.Tag") return () => void branchDialogs?.tag(null);
    if (action === "Git.Reset") return () => void branchDialogs?.resetHead();
    // `ChangesView.CreatePatch`, an executor action: there is none with the staging area.
    if (action === "ChangesView.CreatePatch") {
      if (settings && effectiveStaging(settings)) return null;
      return () => requestExecutor("patch", wholeProject);
    }
    // `ChangesView.Shelve`, the same kind of action; `Show Shelf` asks for the Shelf tab.
    if (action === "ChangesView.Shelve") {
      if (settings && effectiveStaging(settings)) return null;
      return () => requestExecutor("shelve", wholeProject);
    }
    if (action === "Vcs.Show.Shelf") return toggleShelf;
    if (action === "Vcs.Show.Log") return () => toggleTab("log");
    // `Git > Current File` is the focused window's: without the focus it is not this one's.
    if ((CURRENT_FILE_ACTIONS as string[]).includes(action)) {
      return document.hasFocus() ? fileActionRun(mainFileSelection, fileMenuState(), action) : null;
    }
    // Ramify's own: the Shelf tab's `Import from .idea`, also while the tab is not there.
    if (action === "Shelf.ImportFromIdea") return () => void shelfActions.importIdea(current.path);
    // `ChangesView.Revert`: Local Changes' Rollback, in front.
    if (action === "ChangesView.Revert") return () => void rollbackLocalChanges();
    if (action === "ChangesView.ApplyPatch") {
      return () => void applyContext().then((context) => context && applies.applyPatch(context));
    }
    if (action === "ChangesView.ApplyPatchFromClipboard") {
      return () => void applyContext().then((context) => context && applies.applyFromClipboard(context));
    }
    if (action === "Git.Stash") return () => void branchDialogs?.stashChanges();
    if (action === "Git.Unstash") return () => void branchDialogs?.unstashChanges();
    if (action === "Git.Show.Stash") return () => void showGitStash();
    // `Github.View.Pull.Request`, only while the Pull Requests window is available.
    if (action === "Github.View.Pull.Request") return windows.pulls ? viewPullRequestList : null;
    if (action === "Git.Configure.Remotes") return () => void remotesHost?.manage();
    if (action === "Git.OpenExcludeFile") return () => void remotesHost?.openExcludeFile();
    if ((UPDATE_ACTIONS as readonly string[]).includes(action)) {
      return () => void branchDialogs?.runUpdate(action as UpdateAction);
    }
    return null;
  }

  async function rollbackLocalChanges(paths?: string[]) {
    await showTab("commit");
    (mode === "modal" ? localChanges : commitView)?.rollback(paths);
  }

  // Apply Patch is tied to the repository and the HEAD it was opened on, and to its lists when
  // there are changelists (not with the staging area).
  async function applyContext(): Promise<ApplyContext | null> {
    const current = info;
    if (!current) return null;
    const staging = settings ? effectiveStaging(settings) : false;
    const lists = staging ? null : await changelistsLists().catch(() => null);
    return {
      repo: current.path,
      head: repoNow?.head ?? current.head,
      lists,
      current: () => info?.path ?? null,
    };
  }

  // `PatchClipboardListener`: the clipboard is looked at when the app becomes active (the backend
  // says so; moving between Ramify's own windows is not), with a repository open and nothing modal,
  // only in the project window that was activated (IntelliJ asks in the activated frame).
  const onTheFly = clipboardWatcher(defaultApplyApi, (text) => {
    if (modalOpen()) return;
    void applyContext().then((context) => context && applies.applyText(context, text));
  });

  function appActivated(owner: string | null) {
    appFocused = true;
    if (owner !== ownerOf(ownLabel())) return;
    if (info && !modalOpen()) void onTheFly.activated();
  }

  function appDeactivated() {
    appFocused = false;
    if (info) void onTheFly.deactivated();
  }

  // The Git actions requested by whoever is outside the window, like the Git menu: the action id
  // arrives, the same as the shortcut's. The window is only raised if the action is going to be handled;
  // with a modal dialog open, or without a repository, the menu does not move it.
  function gitAction(action: string) {
    if (modalOpen()) return;
    const run = gitActionRunner(action);
    if (!run) return;
    showWindow().catch(() => {});
    run();
  }

  function submit(event: SubmitEvent) {
    event.preventDefault();
    void openTarget(repoPath);
  }

  // File > Recent Projects: an existing path goes through the open flow like a typed one.
  function openRecent({ path, exists }: RecentProject) {
    if (!exists) {
      missingProject = path;
      return;
    }
    void openTarget(path);
  }

  async function answerMissingProject(remove: boolean) {
    const path = missingProject;
    missingProject = null;
    if (!remove || path === null) return;
    try {
      await recentProjectsRemove(path);
    } catch (failure) {
      showError(failure);
    }
  }

  // Open and File > Open…: IntelliJ's `Open File or Project` chooser, for folders only. It starts in
  // the typed folder, or in the home folder; the chosen folder goes through the open flow.
  async function chooseAndOpen() {
    if (opening) return;
    try {
      const typed = repoPath.trim();
      const chosen = await chooseFolder({
        title: "Open File or Project",
        defaultPath: typed || (await homeDir().catch(() => undefined)),
      });
      if (chosen === null) return;
      await openTarget(chosen);
    } catch (failure) {
      showError(failure);
    }
  }

  // The next visit to the tab loads it again.
  function committed() {
    logPanel?.refresh();
  }

  // With the staging area, the diff window compares according to the group it is opened from.
  function publishComparison() {
    if (settings && effectiveStaging(settings)) emitComparison(comparison);
  }

  async function openDiff(file: FileChange, chosen: StageComparison = "local") {
    comparison = chosen;
    await diffWindowOpen(file.path);
    diffWindow = true;
    publishComparison();
  }

  async function showDiff(file: FileChange, chosen?: StageComparison) {
    try {
      await openDiff(file, chosen);
    } catch (failure) {
      showError(failure);
    }
  }

  // Who keeps the line checkboxes: the dialog while it is open (`ALLOW_EXCLUDE_FROM_COMMIT`), otherwise the
  // Commit view in non-modal mode, and in modal mode nobody (the diff of Local Changes has none).
  const selectionOwner = (): SelectionOwner =>
    $commitDialog ? "dialog" : mode === "non-modal" ? "commit-view" : "none";

  function currentSelection(): SelectionState {
    const owner = selectionOwner();
    const kept =
      owner === "dialog" ? dialogSelection : owner === "commit-view" ? commitViewSelection : null;
    return { owner, ...(kept ?? { included: [], selections: {}, lists: null }) };
  }

  // The diff window, if it exists, shows what the owner has chosen.
  function publishSelection() {
    if (diffWindow) emitSelectionState(currentSelection());
  }

  function commitViewPublished(state: PublishedSelection) {
    commitViewSelection = state;
    publishSelection();
  }

  function dialogPublished(state: PublishedSelection) {
    dialogSelection = state;
    publishSelection();
  }

  // Opening or closing the dialog, and switching the mode, change the owner: the window is told, and what
  // the previous owner kept is dropped when it goes away.
  $effect(() => {
    void [$commitDialog, mode];
    untrack(() => {
      if (!$commitDialog) dialogSelection = null;
      if (mode === "modal") commitViewSelection = null;
      publishSelection();
    });
  });

  function settingsChanged(next: Settings) {
    const gitChanged = settings?.gitPath !== next.gitPath;
    applySettings(next);
    if (gitChanged) notifyRepoChanged();
  }

  // A lost selection is reported to the owner's window only: the other one has nothing of it on screen.
  // The dialog reports a failed background commit after it has closed, when it no longer owns the window.
  function selectionLost(lost: LostSelection, from: SelectionOwner) {
    if (diffWindow && (from === "dialog" || from === selectionOwner())) emitSelectionLost(lost);
  }

  async function logReady() {
    if (!logPanel || !(await perfEnabled())) return;
    startPerf(logPanel.perfTarget().scroller);
  }

  function startPerf(scroller: HTMLElement | null) {
    if (perfStarted) return;
    perfStarted = true;
    const { viewId, count } = logPanel?.perfTarget() ?? { viewId: 0, count: 0 };
    runPerf({
      scroller,
      openCommit: (hash) => logPanel?.openCommit(hash) ?? Promise.resolve(),
      viewId,
      expectedScrollHeight: count * ROW_HEIGHT,
      showCommit: () => showTab("commit"),
      showDiff: openDiff,
    });
  }

  onMount(() => {
    routeCommitRequests({ mode: () => mode, nonModal: commitInView });
    const stopTabRequests = onTabRequested(showTab);
    const stopWindowRequests = onWindowActivationRequested(bringToFront);
    const stops = [
      // The change goes back only to the owner.
      onSelectionChanged((change) => {
        const owner = selectionOwner();
        if (owner === "dialog") commitDialogRef?.setLineSelection(change);
        else if (owner === "commit-view") {
          commitView?.setLineSelection(change.path, change.selection, change.included, change.places);
        }
      }),
      onLeaveRefused(showError),
      onCommitDraftSave(() => void saveDraftBeforeClose()),
      onDiffReady(() => {
        diffWindow = true;
        emitSelectionState(currentSelection());
        publishComparison();
      }),
      onSettingsChanged(settingsChanged),
      onOpenSettings(() => (settingsOpen = true)),
      // Like the Git menu, with a modal dialog open they do not act.
      onOpenProjectChooser(() => {
        if (!modalOpen()) void chooseAndOpen();
      }),
      onOpenRecent((project) => {
        if (!modalOpen()) openRecent(project);
      }),
      onOpenPath((path) => {
        if (!modalOpen()) void openTarget(path);
      }),
      onGitAction(gitAction),
      onGitFileAction(fileActionFromViewer),
      onAnnotationShowInLog(annotationInLog),
      onSelectionHistoryShowAffected(affectedFromSelectionHistory),
      onAppActivated(appActivated),
      onMenuOwnerGained(menuOwnerGained),
      onAppDeactivated(appDeactivated),
      onRepoChanged(refreshState),
      onBranchSyncChanged(() => void readBranchSync()),
      onExternalChanges(reportExternal),
    ];
    start();
    return () => {
      routeCommitRequests(null);
      stopTabRequests();
      stopWindowRequests();
      stops.forEach((stop) => stop.then((unlisten) => unlisten()));
    };
  });

  // The Git menu turns on what belongs to the operation in progress; if it cannot, the menu stays as it was.
  // `Git.Fetch` goes off while any fetch of the app runs (`GitFetch.update`).
  // `Github.View.Pull.Request` is there while the Pull Requests window is available
  // (`GithubViewPullRequestsAction.update`), and its GitHub submenu with it.
  function syncMenu(state: RepoState) {
    const ids = gitMenuEnabled(state, hasConflicts);
    const fetch = fetchRunning ? [] : ["Git.Fetch"];
    const github = pullsAvailable ? ["Github.View.Pull.Request"] : [];
    gitMenuEnable([...ids, ...fetch, ...github]).catch(() => {});
  }

  // The menu bar is the app's: this window puts its own state in it when it gains the focus, and
  // when the backend hands the menu to it (the page's `focus` can arrive before the backend knows,
  // and the focus may be on one of its auxiliary windows).
  function windowGainedFocus() {
    windowFocused = true;
    if (repoNow) syncMenu(repoNow);
  }
  // Current File is this window's from now on: its own while it has the focus (or is getting it),
  // the viewer's while the viewer has it (the viewer says so), and otherwise nothing to act on,
  // since the entries only act in the focused window. Never the previous owner's.
  function menuOwnerGained(focused: string | null) {
    if (repoNow) syncMenu(repoNow);
    const own = ownLabel();
    if (windowFocused || focused === own) sendCurrentFile();
    else if (focused !== labelFor("viewer", ownerOf(own) ?? own)) sendNoCurrentFile();
  }

  const pullsAvailable = $derived(windows.pulls !== null);
  // Whether the window was available at the last sync this effect made: the first run, with no
  // window yet, adds nothing to what `open` syncs.
  let menuHadPulls = false;
  $effect(() => {
    if (!pullsAvailable && !menuHadPulls) return;
    menuHadPulls = pullsAvailable;
    untrack(() => {
      if (repoNow) syncMenu(repoNow);
    });
  });

  // `subscribe` runs the callback at once, inside the effect: untracked, so that neither `repoNow` nor
  // `hasConflicts` (read by `syncMenu`) makes the effect subscribe again.
  let fetchRunning = false;
  $effect(() =>
    fetching.subscribe((running) => {
      fetchRunning = running;
      untrack(() => {
        if (repoNow) syncMenu(repoNow);
      });
    }),
  );

  function conflictsChanged(conflicts: boolean) {
    if (conflicts === hasConflicts) return;
    hasConflicts = conflicts;
    if (repoNow) syncMenu(repoNow);
  }

  // §5.8 step 2: before this window closes, or the app quits, its commit message draft goes to the
  // changelist comment, where the Commit Changes dialog keeps it: the open dialog's as its Cancel
  // keeps it, the Commit tool window's to the active list. The backend waits for the answer, which
  // comes whatever happens.
  async function saveDraftBeforeClose() {
    try {
      if ($commitDialog && commitDialogRef) await commitDialogRef.saveDraft();
      else if (info && mode !== "modal") {
        const draft = commitView?.changelistDraft();
        if (draft) await changelistsSetComment(draft.id, draft.comment, info.path);
      }
    } catch (failure) {
      showError(failure);
    } finally {
      await commitDraftSaved().catch(() => {});
    }
  }

  // The open list's paths that no longer exist: skipped at start, and said once.
  function reportMissing(missing: string[]) {
    if (missing.length === 0) return;
    notify({
      kind: "warning",
      title: `Some projects could not be reopened because their folders no longer exist: ${missing.map(projectName).join(", ")}`,
    });
  }

  // If git fails now, the next signal reads it again.
  async function refreshState(change: RepoChange = { stash: false, shelf: false }) {
    if (!info) return;
    if (change.stash) void loadStashes();
    // `SHELF_TOPIC`: only a command that wrote the store makes the lists be read again.
    if (change.shelf || shelfUnread) void loadShelf();
    try {
      status = await repoState();
    } catch {
      return;
    }
    syncMenu(status);
    void readBranchSyncIfRefsChanged();
    void readBranchesToCompare();
    // A fetch may have loaded GitHub's protected branch rules.
    void refreshProtectedBranches();
    // A remote added, removed or changed shows or hides the Pull Requests window.
    const checked = info.path;
    void checkPullRequestsWindow(() => info?.path === checked);
    void branchDialogs?.repositoryRefreshed();
  }

  async function start() {
    let measuring = false;
    try {
      version = await appVersion();
      applySettings(await settingsGet());
      // `Ramify <file>[:<line>]` also shows the file, once its repository is open (decision 1).
      const initial = await initialTarget();
      // Reopening (§5.8): with the open list's first project as this window's initial target, the
      // other ones open in new windows meanwhile. Nothing for a start with a path.
      void openProjectsReopen()
        .then(({ missing }) => reportMissing(missing))
        .catch(showError);
      if (initial) {
        repoPath = initial.repo;
        await open();
        if (info && initial.file !== null) {
          await openInViewer({ path: initial.file, revision: null, line: initial.line });
        }
      }
      if (info && (await perfEnabled())) {
        measuring = true;
        showTab("log");
      }
    } catch (failure) {
      showError(failure);
    } finally {
      await showWindow();
      performance.mark("window-shown");
      requestAnimationFrame(() => performance.mark("first-paint"));
      // The log is only painted with frames: without them the measurement would stay waiting for it.
      if (measuring && !(await framesRunning())) startPerf(null);
    }
  }
</script>

<svelte:window
  onkeydown={keydown}
  onfocus={windowGainedFocus}
  onblur={() => (windowFocused = false)}
/>

<main>
  <!-- An apply is modal from its OK to its end (`applyInProgress`): what is underneath is inert. -->
  <form class="main-toolbar" onsubmit={submit} inert={$applying}>
    <ProjectWidget
      path={info?.path ?? null}
      {home}
      onopen={openTarget}
      onmissing={(path) => openRecent({ path, exists: false })}
      onchoose={() => void chooseAndOpen()}
      onclone={() => void remotesHost?.clone()}
    />
    {#if info}
      <button
        bind:this={branchWidget}
        type="button"
        class="branch"
        aria-label="Git Branch: {branchLabel}"
        onpointerdown={(event) => event.stopPropagation()}
        onclick={showBranchesFromWidget}
      >
        <Icon name="vcs" />
        {branchLabel}
        <!-- `GitToolbarWidgetAction`: the right icons, with no text of their own. -->
        {#if shownSync?.incoming}<span class="sync" data-sync="incoming"><Icon name="incoming" /></span>{/if}
        {#if shownSync?.outgoing}<span class="sync" data-sync="outgoing"><Icon name="outgoing" /></span>{/if}
      </button>
      {#if repoNow}
        <MergeRebaseWidget
          state={repoNow}
          conflicts={hasConflicts}
          handlers={ongoingHandlers}
          onresolve={() => void branchDialogs?.resolveConflicts()}
        />
      {/if}
    {/if}
    <input
      class="input path"
      bind:value={repoPath}
      placeholder="Repository path"
      aria-label="Repository path"
    />
    <button type="button" class="button" disabled={opening} onclick={chooseAndOpen}>Open</button>
    <span class="version">{version}</span>
  </form>

  {#if info}
    <div class="workspace" inert={$applying}>
      <div class="stripe" role="tablist" aria-orientation="vertical" aria-label="Tool windows">
        {#each stripe as id (id)}
          {@const { label, icon, shortcut } = toolWindow(id)}
          <button
            type="button"
            role="tab"
            class="stripe-button"
            aria-label={label}
            aria-selected={activeWindow === id}
            title={shortcut ? `${label} (${shortcut})` : label}
            onclick={() => toggleWindow(id)}
          >
            <Icon name={icon} size={20} />
          </button>
        {/each}
      </div>

      <div class="content" bind:this={contentArea}>
        <!-- The Commit window shows its strip only with more than one content (New UI). -->
        {#if activeWindow === "commit" && windows.commit && windows.commit.length > 1}
          <VersionControlWindow
            toolWindow="commit"
            label="Commit"
            contents={windows.commit}
            selected={shown.commit ?? windows.commit[0]}
            onselect={(id, focus) => void (focus ? showTab(id) : select(id))}
          />
        {/if}
        {#if activeWindow === "git" && windows.git && windows.git.length > 0}
          {@const gitSelected = shown.git ?? windows.git[0]}
          <VersionControlWindow
            toolWindow="git"
            label="Git"
            contents={windows.git}
            selected={gitSelected}
            options={gitSelected === "local-changes" && mode === "modal"
              ? showOnDoubleClickItems
              : undefined}
            onselect={(id, focus) => void (focus ? showTab(id) : select(id))}
          />
        {/if}

        <div class="panes">
          <!-- The Project tool window: the repository's folders as the disk has them. -->
          <div class="pane" class:hidden={activeWindow !== "project"} role="region" aria-label="Project">
            {#key info}
              {#if info && projectShown}
                <ProjectTree
                  root={info.path}
                  active={activeWindow === "project"}
                  gitMenu={(files) => gitSubmenu({ files, editor: false }, fileMenuState())}
                  onfiles={(files, changed) => {
                    treeFiles = files;
                    treeChanges = changed;
                  }}
                />
              {/if}
            {/key}
          </div>

          <!-- The Changes tool window: registered on first use, one tab (`ChangesBrowserToolWindow`). -->
          {#if windows.changes}
            <div class="pane" class:hidden={activeWindow !== "changes"} role="region" aria-label="Changes">
              <ChangesToolWindow repo={info.path} active={activeWindow === "changes"} />
            </div>
          {/if}

          <!-- The Repositories tool window: registered on first use, the repository at a revision
               (`RepositoryBrowser`); one repository per window, so one tab. -->
          {#if windows.repositories}
            <div class="pane" class:hidden={activeWindow !== "repositories"} role="region" aria-label="Repositories">
              <RepositoryBrowser repo={info.path} active={activeWindow === "repositories"} />
            </div>
          {/if}

          <!-- The Pull Requests tool window: registered while a remote points to a GitHub repository
               (`GHPRToolWindowFactory`). -->
          {#if windows.pulls}
            <div class="pane" class:hidden={activeWindow !== "pulls"} role="region" aria-label="Pull Requests">
              <PullRequestsWindow
                active={activeWindow === "pulls"}
                focused={appFocused}
                answers={(branch, choice, outcome) => branchDialogs?.answerRemote(branch, choice, outcome) ?? Promise.resolve()}
              />
            </div>
          {/if}

          <!-- Local Changes: without checkboxes in modal mode, the Commit view in non-modal mode. -->
          <div class="pane" class:hidden={tab !== "local-changes"}>
            {#key info}
              {#if mode === "modal"}
                <LocalChangesView
                  bind:this={localChanges}
                  repo={info}
                  active={tab === "local-changes" && !$commitDialog}
                  oncheckin={(request) => void requestCommit({ request })}
                  onshowdiff={showDiff}
                  onresolveconflicts={() => void branchDialogs?.resolveConflicts()}
                  onconflictschange={conflictsChanged}
                />
              {:else}
                <CommitView
                  bind:this={commitView}
                  ticket={commitTicket}
                  repo={info}
                  active={tab === "local-changes"}
                  draft={commitDraft}
                  stagingArea={settings ? effectiveStaging(settings) : false}
                  onshowdiff={showDiff}
                  oncommitted={committed}
                  onselectionstate={commitViewPublished}
                  onselectionlost={(lost) => selectionLost(lost, "commit-view")}
                  onpreparepush={() => branchDialogs?.prepareCommitPush() ?? (() => Promise.resolve())}
                  onrebase={(base) => branchDialogs?.rebaseAfterCommit(base) ?? Promise.resolve()}
                  onresolveconflicts={() => void branchDialogs?.resolveConflicts()}
                  onconflictschange={conflictsChanged}
                />
              {/if}
            {/key}
          </div>

          {#if available.includes("shelf")}
            <div class="pane" class:hidden={tab !== "shelf"}>
              {#key info}
                <ShelfTab
                  bind:this={shelfTab}
                  entries={shelfEntries}
                  settings={$shelfSettings}
                  active={tab === "shelf"}
                  api={shelfTabApi}
                  onunshelve={(selection, withDialog) => void unshelveFromTab(selection, withDialog)}
                  ondelete={(selection) =>
                    info &&
                    void shelfActions.remove(
                      info.path,
                      selection.exact.map((entry) => entry.list.name),
                      partFiles(selection),
                    )}
                  onrestore={(names) => info && void shelfActions.restore(info.path, names)}
                  onrename={(name, description) => info && void shelfActions.rename(info.path, name, description)}
                  onimport={() => info && void shelfActions.importPatches(info.path)}
                  onimportidea={() => info && void shelfActions.importIdea(info.path)}
                  oncreatepatch={(files) => info && void patches.createPatch({ repo: info.path, entries: [], shelf: files }, null)}
                  oncopypatch={(files) => info && void patches.copyAsPatch({ repo: info.path, entries: [], shelf: files })}
                  onsettings={shelfSettingsChanged}
                />
              {/key}
            </div>
          {/if}
          {#if available.includes("stash")}
            <div class="pane" class:hidden={tab !== "stash"}>
              {#key info}
                <StashTab
                  bind:this={stashTab}
                  {stashes}
                  error={stashError}
                  currentBranch={branch}
                  onunstash={(stash, pop) => branchDialogs?.unstashEntry(stash, pop)}
                  onunstashas={(stash) => branchDialogs?.unstashAs(stash)}
                  ondrop={(stash) => branchDialogs?.dropStash(stash)}
                  onvcs={(action) => branchDialogs?.run(action)}
                />
              {/key}
            </div>
          {/if}

          <div class="pane" class:hidden={tab !== "log"}>
            {#key info}
              <LogPanel
                bind:this={logPanel}
                active={tab === "log"}
                root={info?.path ?? ""}
                onready={logReady}
                onvcs={(action) => branchDialogs?.run(action)}
                onfetch={() => branchDialogs?.fetch() ?? Promise.resolve()}
                oncreatepatch={(changes) => {
                  const entries = logEntries(changes);
                  if (info) void patches.createPatch({ repo: info.path, entries }, null);
                }}
              />
            {/key}
          </div>

          <div class="pane" class:hidden={tab !== "console"}>
            <ConsoleView active={tab === "console"} />
          </div>

          <div class="pane" class:hidden={tab !== "terminal"}>
            {#key info?.path}
              <TerminalView active={tab === "terminal"} />
            {/key}
          </div>
        </div>
      </div>
    </div>
  {:else if opening}
    <p class="status">Loading…</p>
  {/if}

  <TaskStatus />
</main>

<BranchDialogs
  bind:this={branchDialogs}
  onviewcommits={viewCommits}
  ondefineremote={() => remotesHost?.defineRemote() ?? Promise.resolve(false)}
  onshowcommit={() => void showTab("commit")}
  onshowstash={showGitStash}
/>
<RemotesHost bind:this={remotesHost} current={() => info?.path ?? null} open={openTarget} />
{#if branchesPopup}
  <BranchesPopup
    anchor={branchesPopup.anchor}
    onclose={() => (branchesPopup = null)}
    onvcs={(action) => branchDialogs?.run(action)}
    onnewbranch={() => branchDialogs?.newBranch(null, "HEAD", suggestedBranch)}
    oncheckoutref={() => branchDialogs?.checkoutRef()}
    onupdateproject={(options) => branchDialogs?.runUpdate("Vcs.UpdateProject", options)}
    updatedialog={settings?.showUpdateDialog}
    oncommit={checkinFromLog}
    onpush={() => branchDialogs?.push()}
    onfetch={() => branchDialogs?.fetch() ?? Promise.resolve()}
    ongoing={repoNow ? ongoingActions(repoNow, ongoingHandlers) : []}
    head={repoNow?.head ?? null}
  />
{/if}
{#if $commitDialog && info}
  {@const input = $commitDialog}
  {#key input}
    <CommitDialog
      repo={info}
      request={input.request}
      executor={input.executor ?? "commit"}
      prefilled={input.prefilled}
      rebaseFrom={input.rebaseFrom}
      head={input.head}
      onrebase={(base) => branchDialogs?.rebaseAfterCommit(base) ?? Promise.resolve()}
      onexecute={input.onexecute ?? executorRun(input.executor)}
      branch={branchLabel}
      hasHead={(repoNow?.head ?? "") !== ""}
      clearInitial={settings?.clearInitialCommitMessage ?? false}
      ticket={commitTicket}
      onshowdiff={showDiff}
      onpreparepush={() => branchDialogs?.prepareCommitPush() ?? (() => Promise.resolve())}
      oncommitted={dialogCommitted}
      bind:this={commitDialogRef}
      onselectionstate={dialogPublished}
      onselectionlost={(lost) => selectionLost(lost, "dialog")}
      sameRepo={(path) => info?.path === path}
      onclose={closeCommitDialog}
    />
  {/key}
{/if}
<!-- After the Create Patch dialog: Patch File Settings opens on top of it. -->
<PatchDialogs />
<ShelfDialogs />
<GetDialog />
<Notifications />

{#if settingsOpen && settings}
  <SettingsView {settings} onclose={() => (settingsOpen = false)} />
{/if}

{#if missingProject !== null}
  <MissingProjectDialog path={missingProject} onanswer={answerMissingProject} />
{/if}

{#if openQuestion}
  <OpenProjectDialog name={openQuestion.name} onanswer={openQuestion.answer} />
{/if}

<AskpassDialog />
<LoginHost />
<MessageEditorDialog root={info?.path ?? ""} />

<style>
  main {
    display: flex;
    flex-direction: column;
    height: 100%;
  }

  .main-toolbar {
    display: flex;
    flex: none;
    gap: 8px;
    align-items: center;
    height: 40px;
    box-sizing: border-box;
    padding: 0 12px;
    background: var(--panel);
    border-bottom: 1px solid var(--border);
  }

  .branch {
    display: inline-flex;
    gap: 6px;
    align-items: center;
    height: 28px;
    padding: 0 8px;
    margin-right: 8px;
    font: inherit;
    color: var(--text);
    white-space: nowrap;
    background: none;
    border: 0;
    border-radius: 6px;
  }

  .branch:hover {
    background: var(--hover);
  }

  .branch .sync {
    display: inline-flex;
    flex: none;
  }

  .path {
    flex: 0 1 420px;
    min-width: 120px;
  }

  .version {
    margin-left: auto;
    color: var(--muted);
  }

  .workspace {
    display: flex;
    flex: 1;
    min-height: 0;
  }

  .stripe {
    display: flex;
    flex: none;
    flex-direction: column;
    gap: 4px;
    align-items: center;
    width: 40px;
    padding-top: 8px;
    background: var(--panel);
    border-right: 1px solid var(--border);
  }

  .stripe-button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 30px;
    height: 30px;
    padding: 0;
    background: none;
    border: 0;
    border-radius: 6px;
  }

  .stripe-button:hover {
    background: var(--hover);
  }

  .stripe-button[aria-selected="true"] {
    background: var(--stripe-selected);
  }

  .stripe-button[aria-selected="true"] :global(.icon) {
    color: var(--accent-text);
  }

  .content {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
  }

  .panes {
    position: relative;
    display: flex;
    flex: 1;
    min-width: 0;
    min-height: 0;
  }

  .pane {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }

  .pane.hidden {
    display: none;
  }

  .status {
    color: var(--text-secondary);
  }

  .status {
    padding: 16px;
    margin: 0;
  }
</style>
