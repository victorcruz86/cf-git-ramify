<script lang="ts">
  import { onMount, tick } from "svelte";
  import {
    appVersion,
    diffWindowOpen,
    emitComparison,
    emitSelectionLost,
    emitSelectionState,
    gitMenuEnable,
    initialRepo,
    notifyRepoChanged,
    onDiffReady,
    onGitAction,
    onLeaveRefused,
    onOpenSettings,
    onRepoChanged,
    onSelectionChanged,
    onSettingsChanged,
    perfEnabled,
    repoOpen,
    repoState,
    settingsGet,
    showWindow,
    type StageComparison,
    type FileChange,
    type LostSelection,
    type Operation,
    type RepoInfo,
    type RepoState,
    type SelectionState,
    type Settings,
  } from "./lib/api";
  import AskpassDialog from "./lib/AskpassDialog.svelte";
  import MessageEditorDialog from "./editor/MessageEditorDialog.svelte";
  import { errorText } from "./lib/errors";
  import Icon from "./lib/Icon.svelte";
  import { matchShortcut, type ActionId } from "./lib/keymap";
  import { notify } from "./lib/notifications";
  import Notifications from "./lib/Notifications.svelte";
  import { nextTab, onTabRequested, type Tab } from "./lib/tabs";
  import TaskStatus from "./lib/TaskStatus.svelte";
  import CommitView from "./commit/CommitView.svelte";
  import ConsoleView from "./console/ConsoleView.svelte";
  import { ROW_HEIGHT } from "./log/draw";
  import LogPanel from "./log/LogPanel.svelte";
  import BranchDialogs from "./branches/BranchDialogs.svelte";
  import BranchesPopup from "./branches/BranchesPopup.svelte";
  import { loadBranchState } from "./branches/state";
  import { gitMenuEnabled, ongoingActionFor, ongoingActions } from "./operation/operation";
  import RemotesHost from "./remotes/RemotesHost.svelte";
  import { UPDATE_ACTIONS, type UpdateAction } from "./update/flow";
  import { framesRunning, runPerf } from "./perf/run";
  import SettingsView from "./settings/SettingsView.svelte";

  const PUSH_ACTION: ActionId = "Vcs.Push";
  const COMMIT_ACTION: ActionId = "CheckinProject";
  const BRANCHES_ACTION: ActionId = "Git.Branches";
  const TABS: { id: Tab; label: string; icon: string; shortcut: string }[] = [
    { id: "commit", label: "Commit", icon: "commit", shortcut: "⌘0" },
    { id: "log", label: "Log", icon: "vcs", shortcut: "⌘9" },
    { id: "console", label: "Console", icon: "text", shortcut: "" },
  ];

  let version = $state("");
  let repoPath = $state("");
  let info = $state<RepoInfo | null>(null);
  let tab = $state<Tab>("commit");
  let opening = $state(false);
  let logPanel = $state<LogPanel>();
  let commitView = $state<CommitView>();
  let branchDialogs = $state<BranchDialogs>();
  let remotesHost = $state<RemotesHost>();
  // `info` does not change with a checkout: changing it would remount the views. This does: the branch and the
  // operation half done, which `repo-changed` makes be read again.
  let status = $state.raw<RepoState | undefined>(undefined);
  const repoNow = $derived(status ?? info?.state);
  const branch = $derived(repoNow?.branch ?? null);
  const branchLabel = $derived(repoNow?.label ?? "");
  // A rebase leaves HEAD detached: `branch` comes from `head-name` and is the one being rebased, which already exists.
  const suggestedBranch = $derived(repoNow?.operation === "rebasing" ? "" : (branch ?? ""));
  // There are conflicted files in the changes list: turns on `Resolve Conflicts…`.
  let hasConflicts = false;
  let perfStarted = false;
  let diffWindow = false;
  let selectionState: SelectionState | null = null;
  let comparison: StageComparison = "local";
  let settings = $state.raw<Settings | null>(null);
  let settingsOpen = $state(false);
  let branchWidget = $state<HTMLButtonElement>();
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

  async function open() {
    if (opening || !repoPath.trim()) return;
    opening = true;
    try {
      // If it does not open, we stay in the current repository, with whatever was half done.
      const opened = await repoOpen(repoPath.trim());
      tab = "commit";
      diffWindow = false;
      status = undefined;
      hasConflicts = false;
      branchesPopup = null;
      info = opened;
      syncMenu(opened.state);
      branchDialogs?.repositoryChanged(opened);
      await loadBranchState();
    } catch (failure) {
      showError(failure);
    } finally {
      opening = false;
    }
  }

  async function showTab(next: Tab) {
    tab = next;
    if (next !== "commit") return;
    await tick();
    commitView?.focusTree();
  }

  function showError(failure: unknown) {
    notify({ kind: "error", title: errorText(failure) });
  }

  async function checkinFromLog() {
    tab = "commit";
    await tick();
    commitView?.checkin();
  }

  function keydown(event: KeyboardEvent) {
    const action = matchShortcut(event);
    if (action === "ShowSettings") {
      settingsOpen = true;
      event.preventDefault();
      return;
    }
    if (!info) return;
    if (action === "ActivateCommitToolWindow") showTab(nextTab(tab, "commit"));
    else if (action === "ActivateVersionControlToolWindow") showTab(nextTab(tab, "log"));
    else if (action === "CheckinProject" && tab !== "commit") checkinFromLog();
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
  const modalOpen = () =>
    settingsOpen || document.querySelector('[role="dialog"][aria-modal="true"]') !== null;

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
    if (action === "Git.Configure.Remotes") return () => void remotesHost?.manage();
    if (action === "Git.OpenExcludeFile") return () => void remotesHost?.openExcludeFile();
    if ((UPDATE_ACTIONS as readonly string[]).includes(action)) {
      return () => void branchDialogs?.runUpdate(action as UpdateAction);
    }
    return null;
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

  // `open` reads the path from the toolbar field: that is how Clone and Create Git Repository open it too.
  async function openRepository(path: string) {
    repoPath = path;
    await open();
  }

  function submit(event: SubmitEvent) {
    event.preventDefault();
    open();
  }

  // The next visit to the tab loads it again.
  function committed() {
    logPanel?.refresh();
  }

  // With the staging area, the diff window compares according to the group it is opened from.
  function publishComparison() {
    if (settings?.stagingArea) emitComparison(comparison);
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

  // The Commit view keeps the selection. The diff window, if it exists, shows what is there.
  function publishSelection(state: SelectionState) {
    selectionState = state;
    if (diffWindow) emitSelectionState(state);
  }

  function settingsChanged(next: Settings) {
    const gitChanged = settings?.gitPath !== next.gitPath;
    settings = next;
    if (gitChanged) notifyRepoChanged();
  }

  function selectionLost(lost: LostSelection) {
    if (diffWindow) emitSelectionLost(lost);
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
    const stopTabRequests = onTabRequested(showTab);
    const stops = [
      onSelectionChanged((change) =>
        commitView?.setLineSelection(
          change.path,
          change.selection,
          change.included,
          change.places,
        ),
      ),
      onLeaveRefused(showError),
      onDiffReady(() => {
        diffWindow = true;
        if (selectionState) emitSelectionState(selectionState);
        publishComparison();
      }),
      onSettingsChanged(settingsChanged),
      onOpenSettings(() => (settingsOpen = true)),
      onGitAction(gitAction),
      onRepoChanged(refreshState),
    ];
    start();
    return () => {
      stopTabRequests();
      stops.forEach((stop) => stop.then((unlisten) => unlisten()));
    };
  });

  // The Git menu turns on what belongs to the operation in progress; if it cannot, the menu stays as it was.
  function syncMenu(state: RepoState) {
    gitMenuEnable(gitMenuEnabled(state, hasConflicts)).catch(() => {});
  }

  function conflictsChanged(conflicts: boolean) {
    if (conflicts === hasConflicts) return;
    hasConflicts = conflicts;
    if (repoNow) syncMenu(repoNow);
  }

  // If git fails now, the next signal reads it again.
  async function refreshState() {
    if (!info) return;
    try {
      status = await repoState();
    } catch {
      return;
    }
    syncMenu(status);
    void branchDialogs?.repositoryRefreshed();
  }

  async function start() {
    let measuring = false;
    try {
      version = await appVersion();
      settings = await settingsGet();
      const initial = await initialRepo();
      if (initial) {
        repoPath = initial;
        await open();
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

<svelte:window onkeydown={keydown} />

<main>
  <form class="main-toolbar" onsubmit={submit}>
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
      </button>
    {/if}
    <input
      class="input path"
      bind:value={repoPath}
      placeholder="Repository path"
      aria-label="Repository path"
    />
    <button class="button" disabled={opening}>Open</button>
    <span class="version">{version}</span>
  </form>

  {#if info}
    <div class="workspace">
      <div class="stripe" role="tablist" aria-orientation="vertical" aria-label="Tool windows">
        {#each TABS as { id, label, icon, shortcut } (id)}
          <button
            type="button"
            role="tab"
            class="stripe-button"
            aria-label={label}
            aria-selected={tab === id}
            title={shortcut ? `${label} (${shortcut})` : label}
            onclick={() => showTab(id)}
          >
            <Icon name={icon} size={20} />
          </button>
        {/each}
      </div>

      <div class="content">
        <div class="pane" class:hidden={tab !== "commit"}>
          {#key info}
            <CommitView
              bind:this={commitView}
              repo={info}
              active={tab === "commit"}
              stagingArea={settings?.stagingArea ?? false}
              onshowdiff={showDiff}
              oncommitted={committed}
              onselectionstate={publishSelection}
              onselectionlost={selectionLost}
              onpreparepush={() => branchDialogs?.prepareCommitPush() ?? (() => Promise.resolve())}
              onrebase={(base) => branchDialogs?.rebaseAfterCommit(base) ?? Promise.resolve()}
              onresolveconflicts={() => void branchDialogs?.resolveConflicts()}
              onconflictschange={conflictsChanged}
            />
          {/key}
        </div>

        <div class="pane" class:hidden={tab !== "log"}>
          {#key info}
            <LogPanel
              bind:this={logPanel}
              active={tab === "log"}
              onready={logReady}
              onvcs={(action) => branchDialogs?.run(action)}
              onfetch={() => branchDialogs?.fetch() ?? Promise.resolve()}
            />
          {/key}
        </div>

        <div class="pane" class:hidden={tab !== "console"}>
          <ConsoleView active={tab === "console"} />
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
  oncommitproposal={(proposal) => void showTab("commit").then(() => commitView?.propose(proposal))}
/>
<RemotesHost bind:this={remotesHost} current={() => info?.path ?? null} open={openRepository} />
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
<Notifications />

{#if settingsOpen && settings}
  <SettingsView {settings} onclose={() => (settingsOpen = false)} />
{/if}

<AskpassDialog />
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
    position: relative;
    display: flex;
    flex: 1;
    min-width: 0;
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
