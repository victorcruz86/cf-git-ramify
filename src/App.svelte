<script lang="ts">
  import { onMount, tick, untrack } from "svelte";
  import {
    appVersion,
    diffWindowOpen,
    commitMode,
    effectiveStaging,
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
    type Settings,
  } from "./lib/api";
  import AskpassDialog from "./lib/AskpassDialog.svelte";
  import MessageEditorDialog from "./editor/MessageEditorDialog.svelte";
  import { errorText } from "./lib/errors";
  import Icon from "./lib/Icon.svelte";
  import { matchShortcut, type ActionId } from "./lib/keymap";
  import { notify } from "./lib/notifications";
  import Notifications from "./lib/Notifications.svelte";
  import {
    onTabRequested,
    toolWindows,
    type ContentId,
    type TabRequest,
    type ToolWindowId,
  } from "./lib/tabs";
  import TaskStatus from "./lib/TaskStatus.svelte";
  import type { CheckinRequest } from "./commit/checkin";
  import CommitDialog from "./commit/CommitDialog.svelte";
  import {
    closeCommitDialog,
    commitDialog,
    requestCommit,
    routeCommitRequests,
    type CommitRequestInput,
  } from "./commit/dialogStore";
  import CommitView, { type CommitDraft } from "./commit/CommitView.svelte";
  import LocalChangesView from "./commit/LocalChangesView.svelte";
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
  import VersionControlWindow from "./vcs/VersionControlWindow.svelte";

  const PUSH_ACTION: ActionId = "Vcs.Push";
  const COMMIT_ACTION: ActionId = "CheckinProject";
  const BRANCHES_ACTION: ActionId = "Git.Branches";
  // The contents that exist now. Shelf and Stash arrive with their views.
  const AVAILABLE: ContentId[] = ["local-changes", "log", "console"];
  // `ToolWindowChanges` is the branch icon in the new UI; `ToolWindowCommit` is the commit one.
  const WINDOWS: Record<ToolWindowId, { label: string; icon: string; shortcut: string }> = {
    commit: { label: "Commit", icon: "commit", shortcut: "⌘0" },
    git: { label: "Git", icon: "vcs", shortcut: "⌘9" },
  };

  let version = $state("");
  let repoPath = $state("");
  let info = $state<RepoInfo | null>(null);
  // The tool window in front and, per window, the tab it shows: a window comes back on the tab it had.
  let activeWindow = $state<ToolWindowId>("git");
  let selected = $state<Record<ToolWindowId, ContentId>>({ commit: "local-changes", git: "local-changes" });
  let opening = $state(false);
  let logPanel = $state<LogPanel>();
  let commitView = $state<CommitView>();
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
  const windows = $derived(toolWindows(mode, AVAILABLE));
  const stripeWindows = $derived((["commit", "git"] as const).filter((id) => windows[id]));
  const tab = $derived(windows[activeWindow] ? selected[activeWindow] : null);
  const branch = $derived(repoNow?.branch ?? null);
  const branchLabel = $derived(repoNow?.label ?? "");
  // A rebase leaves HEAD detached: `branch` comes from `head-name` and is the one being rebased, which already exists.
  const suggestedBranch = $derived(repoNow?.operation === "rebasing" ? "" : (branch ?? ""));
  // There are conflicted files in the changes list: turns on `Resolve Conflicts…`.
  let hasConflicts = false;
  let perfStarted = false;
  let diffWindow = false;
  // What the Commit view and the open dialog last published: only the owner's reaches the diff window.
  let commitViewSelection: PublishedSelection | null = null;
  let dialogSelection: PublishedSelection | null = null;
  let commitDialogRef = $state<CommitDialog>();
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
      select("commit");
      closeCommitDialog();
      commitDraft = null;
      diffWindow = false;
      commitViewSelection = null;
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

  function windowOf(id: ContentId): ToolWindowId | null {
    return (["commit", "git"] as const).find((window) => windows[window]?.includes(id)) ?? null;
  }

  // `commit` is wherever Local Changes is: its own window or a tab of `Git`.
  function select(request: TabRequest): ContentId | null {
    const id = request === "commit" ? "local-changes" : request;
    const owner = windowOf(id);
    if (!owner) return null;
    selected[owner] = id;
    activeWindow = owner;
    return id;
  }

  async function showTab(request: TabRequest) {
    if (select(request) !== "local-changes") return;
    await tick();
    (mode === "modal" ? localChanges : commitView)?.focusTree();
  }

  // ⌘0 and ⌘9, or the stripe: a window that does not exist now does nothing, and neither does the one in front.
  function activateWindow(id: ToolWindowId) {
    if (!windows[id] || activeWindow === id) return;
    void showTab(selected[id]);
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
    if (mode === before) return;
    selected = { commit: windows.commit?.[0] ?? "local-changes", git: windows.git[0] };
    activeWindow = windows.commit ? "commit" : "git";
  }

  function showError(failure: unknown) {
    notify({ kind: "error", title: errorText(failure) });
  }

  // What a commit entry point does in non-modal mode (`requestCommit` decides): the Commit tool window comes
  // to the front and takes the request, or the message of Fixup… and Squash Into… with its executor.
  async function commitInView({ prefilled, rebaseFrom, head }: CommitRequestInput) {
    await showTab("commit");
    if (prefilled === undefined) {
      commitView?.checkin();
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

  function keydown(event: KeyboardEvent) {
    // The Commit Changes dialog is modal: what is underneath does not act.
    if ($commitDialog) return;
    const action = matchShortcut(event);
    if (action === "ShowSettings") {
      settingsOpen = true;
      event.preventDefault();
      return;
    }
    if (!info) return;
    if (action === "ActivateCommitToolWindow") activateWindow("commit");
    else if (action === "ActivateVersionControlToolWindow") activateWindow("git");
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
      onDiffReady(() => {
        diffWindow = true;
        emitSelectionState(currentSelection());
        publishComparison();
      }),
      onSettingsChanged(settingsChanged),
      onOpenSettings(() => (settingsOpen = true)),
      onGitAction(gitAction),
      onRepoChanged(refreshState),
    ];
    start();
    return () => {
      routeCommitRequests(null);
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
      applySettings(await settingsGet());
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
        {#each stripeWindows as id (id)}
          {@const { label, icon, shortcut } = WINDOWS[id]}
          <button
            type="button"
            role="tab"
            class="stripe-button"
            aria-label={label}
            aria-selected={activeWindow === id}
            title="{label} ({shortcut})"
            onclick={() => activateWindow(id)}
          >
            <Icon name={icon} size={20} />
          </button>
        {/each}
      </div>

      <div class="content">
        {#if activeWindow === "git" && windows.git.length > 0}
          <VersionControlWindow
            contents={windows.git}
            selected={selected.git}
            onselect={(id, focus) => void (focus ? showTab(id) : select(id))}
          />
        {/if}

        <div class="panes">
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
      onexecute={input.onexecute}
      branch={branchLabel}
      hasHead={(repoNow?.head ?? "") !== ""}
      clearInitial={settings?.clearInitialCommitMessage ?? false}
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
