<script lang="ts">
  import { onMount, tick, untrack } from "svelte";
  import {
    logCurrentBranch,
    logGraphClick,
    logOpen,
    logOptionsGet,
    logOptionsSet,
    logRefs,
    logRefsChanged,
    logUsers,
    logViewClose,
    logViewOpen,
    logViewSet,
    onRepoChanged,
    type ApiError,
    type GraphTarget,
    type LogRef,
    type LogSummary,
    type Person,
    type SortType,
    type ViewOpened,
    type ViewPatch,
    type ViewSpec,
  } from "../lib/api";
  import { errorText } from "../lib/errors";
  import Icon from "../lib/Icon.svelte";
  import { matchShortcut } from "../lib/keymap";
  import { notify } from "../lib/notifications";
  import { taskResult } from "../lib/tasks";
  import BranchesPanel from "../branches/BranchesPanel.svelte";
  import type { LogChange } from "./changes";
  import ChangesPanel from "./ChangesPanel.svelte";
  import DetailsPanel from "./DetailsPanel.svelte";
  import FilterBar from "./FilterBar.svelte";
  import type { VcsAction } from "./contextMenu";
  import {
    branchValues,
    displayText,
    emptyFilters,
    splitBranchValues,
    toLogFilter,
    type FilterState,
  } from "./filters";
  import LogActions from "./LogActions.svelte";
  import LogTabs from "./LogTabs.svelte";
  import LogView from "./LogView.svelte";
  import { defaultOptions, readOptions, type LogOptions, type Splits } from "./options";
  import type { SelectionSnapshot } from "./selection";
  import ViewOptions from "./ViewOptions.svelte";

  const MAIN_TAB = 0;
  const TAB_NAME = "Log";
  const UPDATE_TAB_NAME = "Update Info";
  const NEW_TAB = "Open New Git Log Tab";
  const NO_MATCHES = "No commits matching filters";
  const NO_COMMITS = "This repository has no commits yet";
  const MIN_SPLIT = 0.1;

  interface LogTab {
    id: number;
    filters: FilterState;
    options: LogOptions;
    // The last fold button, which survives a reload.
    collapsed: boolean;
    view: LogSummary | null;
    generation: number;
    // What was selected when the tab was left.
    saved: SelectionSnapshot | null;
    // `Update Info`: what an update received. It is not remembered from one session to the next.
    update?: { range: [string, string]; name: string };
  }

  interface Props {
    active: boolean;
    onready?: () => void;
    // The branch operations of the context menu, which Task 9 wires up.
    onvcs?: (action: VcsAction) => void;
    onfetch?: () => Promise<unknown>;
  }

  let { active, onready, onvcs, onfetch }: Props = $props();

  const newTab = (id: number, options = defaultOptions()): LogTab => ({
    id,
    filters: emptyFilters(),
    options,
    collapsed: false,
    view: null,
    generation: 0,
    saved: null,
  });

  let tabs = $state.raw<LogTab[]>([newTab(MAIN_TAB, defaultOptions(true))]);
  let currentId = $state(MAIN_TAB);
  let needsLoad = $state(true);
  let loading = $state(false);
  let working = $state(0);
  let refs = $state.raw<LogRef[]>([]);
  let currentBranch = $state<string | null>(null);
  let users = $state.raw<Person[]>([]);
  let selection = $state.raw<string[]>([]);
  let change = $state.raw<LogChange | null>(null);
  let diffOpen = $state(false);
  let viewMenu = $state.raw<{ x: number; y: number } | null>(null);
  let logView = $state<LogView>();
  let filterBar = $state<FilterBar>();
  let logActions = $state<LogActions>();
  let repoEmpty = $state(false);
  let optionsRead = false;
  let nextTab = MAIN_TAB + 1;
  let diffOpened: (() => void) | null = null;
  // Per tab, the last view request: an earlier response is no longer valid.
  const latest = new Map<number, number>();
  // The same goes for clicks on the graph: a click does not invalidate a view change in progress, and
  // both arrive in order because `change_view` queues them.
  const clicks = new Map<number, number>();
  // Tabs with a `log_view_open` in progress: that command already went out with another specification.
  const opening = new Map<number, number>();
  // Goes up with each read of the log, which leaves the earlier views dead.
  let reloads = 0;
  // Until `log_open` responds there is no view to change: `load` will read the options when it finishes.
  let waitingLog = false;
  // Whoever asked for a tab before the log was loaded.
  let loadWaiters: (() => void)[] = [];

  const current = $derived(tabs.find((tab) => tab.id === currentId) ?? tabs[0]);
  const options = $derived(current.options);
  const view = $derived(current.view);
  const showDiff = $derived((options.diffPreview || diffOpen) && selection.length > 0);

  function isFiltered(filters: FilterState): boolean {
    const { regex: _regex, matchCase: _matchCase, ...values } = filters;
    return Object.values(values).some((value) =>
      Array.isArray(value) ? value.length > 0 : Boolean(value),
    );
  }

  function tabName(tab: LogTab): string {
    if (tab.id === MAIN_TAB) return TAB_NAME;
    if (tab.update) return `${UPDATE_TAB_NAME}: ${tab.update.name}`;
    const { filters } = tab;
    const values = [filters.text, ...branchValues(filters), ...filters.users, ...filters.paths];
    const shown = displayText(values.filter(Boolean));
    return shown ? `${TAB_NAME}: ${shown}` : TAB_NAME;
  }

  function patchTab(id: number, change: Partial<LogTab>) {
    tabs = tabs.map((tab) => (tab.id === id ? { ...tab, ...change } : tab));
  }

  const tabOf = (id: number) => tabs.find((tab) => tab.id === id);

  const specOf = (tab: LogTab): ViewSpec => ({
    sort: tab.options.sort,
    longEdges: tab.options.longEdges,
    collapsed: tab.collapsed,
    filter: toLogFilter(tab.filters),
  });

  function plainView(tab: LogTab): boolean {
    const { sort, longEdges } = tab.options;
    return sort === "off" && !longEdges && !tab.collapsed && !isFiltered(tab.filters);
  }

  function showError(failure: unknown) {
    notify({ kind: "error", title: errorText(failure) });
  }

  const cancelled = (failure: unknown) => (failure as ApiError)?.kind === "cancelled";

  // The repository remembers them: if they cannot be saved, they last until it is closed.
  function saveOptions() {
    const stored = Object.fromEntries(
      tabs.filter((tab) => !tab.update).map((tab) => [tab.id, tab.options]),
    );
    logOptionsSet(stored).catch(() => {});
  }

  function setOptions(change: Partial<LogOptions>, id = currentId) {
    const tab = tabOf(id);
    if (!tab) return;
    patchTab(id, { options: { ...tab.options, ...change } });
    saveOptions();
  }

  // Rows change places: the selection and the scroll follow the commits.
  async function install(id: number, summary: LogSummary, changed: boolean) {
    const tab = tabOf(id);
    if (!tab) return;
    const shown = id === currentId ? logView : undefined;
    const snapshot = shown?.snapshot();
    patchTab(id, { view: summary, generation: tab.generation + 1 });
    if (!snapshot) return;
    await tick();
    await logView?.restore(snapshot, changed);
  }

  function nextToken(id: number, tokens = latest): number {
    const token = (tokens.get(id) ?? 0) + 1;
    tokens.set(id, token);
    return token;
  }

  async function openView(id: number, changed = false) {
    const tab = tabOf(id);
    if (!tab) return;
    const token = nextToken(id);
    opening.set(id, token);
    working++;
    try {
      const { summary } = await taskResult<ViewOpened>(await logViewOpen(id, specOf(tab)));
      if (latest.get(id) === token) await install(id, summary, changed);
    } catch (failure) {
      if (!cancelled(failure)) showError(failure);
    } finally {
      if (opening.get(id) === token) opening.delete(id);
      working--;
    }
  }

  // What the receiver of the result of changing a view checks.
  function changeGuard(id: number, viewId: number, tokens = latest) {
    const token = nextToken(id, tokens);
    const epoch = reloads;
    return () =>
      tokens.get(id) === token && reloads === epoch && tabOf(id)?.view?.viewId === viewId;
  }

  async function changeView(patch: ViewPatch) {
    if (!view || waitingLog) return;
    const { id } = current;
    if (opening.has(id)) {
      await openView(id);
      return;
    }
    const valid = changeGuard(id, view.viewId);
    working++;
    try {
      const summary = await logViewSet(view.viewId, patch);
      if (valid()) await install(id, summary, false);
    } catch (failure) {
      if (valid()) showError(failure);
    } finally {
      working--;
    }
  }

  function readTabs(stored: Record<string, unknown>) {
    const ids = Object.keys(stored).map(Number).filter(Number.isInteger);
    const others = ids.filter((id) => id !== MAIN_TAB).sort((a, b) => a - b);
    nextTab = Math.max(MAIN_TAB, ...others) + 1;
    tabs = [MAIN_TAB, ...others].map((id) =>
      newTab(id, readOptions(stored[id], id === MAIN_TAB)),
    );
  }

  async function load() {
    needsLoad = false;
    loading = true;
    reloads++;
    waitingLog = true;
    try {
      if (!optionsRead) {
        readTabs(await logOptionsGet().catch(() => ({})));
        optionsRead = true;
      }
      performance.mark("log-open-start");
      const summary = await logOpen();
      performance.mark("log-open-end");
      waitingLog = false;
      repoEmpty = summary.count === 0;
      await Promise.all(
        tabs.map((tab) =>
          tab.id === MAIN_TAB && plainView(tab)
            ? install(MAIN_TAB, summary, true)
            : openView(tab.id, true),
        ),
      );
      if (summary.count > 0) await loadRefs();
    } catch (failure) {
      showError(failure);
    } finally {
      waitingLog = false;
      loading = false;
      loadWaiters.splice(0).forEach((resume) => resume());
    }
  }

  async function loadRefs() {
    [refs, users, currentBranch] = await Promise.all([logRefs(), logUsers(), logCurrentBranch()]);
  }

  $effect(() => {
    if (!active) {
      // A failure is retried when returning to the tab.
      if (!untrack(() => tabs[0].view)) needsLoad = true;
    } else if (needsLoad && !loading) {
      untrack(load);
    }
  });

  export function refresh() {
    needsLoad = true;
  }

  export async function applyFilters(change: Partial<FilterState>) {
    await filtersChanged({ ...current.filters, ...change });
  }

  // `updateLogBranchFilter`: replaces whatever was in the branch filter.
  function filterByBranches(branches: string[]) {
    applyFilters(splitBranchValues(branches));
  }

  async function filtersChanged(filters: FilterState) {
    patchTab(currentId, { filters });
    await openView(currentId);
  }

  function changeSort(sort: SortType) {
    setOptions({ sort });
    patchTab(currentId, { collapsed: false });
    changeView({ sort });
  }

  function changeLongEdges(longEdges: boolean) {
    setOptions({ longEdges });
    changeView({ longEdges });
  }

  function changeCollapsed(collapsed: boolean) {
    patchTab(currentId, { collapsed });
    changeView({ collapsed });
  }

  async function graphClick(target: GraphTarget) {
    const { id } = current;
    if (!view || loading || opening.has(id)) return;
    const valid = changeGuard(id, view.viewId, clicks);
    try {
      const { changed, summary } = await logGraphClick(view.viewId, target);
      if (changed && valid()) await install(id, summary, false);
    } catch (failure) {
      if (valid()) showError(failure);
    }
  }

  // Restoring the selection after a reload must not request the details again.
  function select(hashes: string[]) {
    const same =
      hashes.length === selection.length && hashes.every((hash, i) => hash === selection[i]);
    if (!same) selection = hashes;
  }

  export async function openTab(filters = emptyFilters()) {
    const id = nextTab++;
    await switchTab(id, { ...newTab(id), filters });
    saveOptions();
    await openView(id);
  }

  const loaded = () =>
    new Promise<void>((resume) => {
      if (optionsRead && !loading && !needsLoad && tabs[0].view) resume();
      else loadWaiters.push(resume);
    });

  // `GitUpdateInfoAsLog.findOrCreateLogUi`: the tab with that range, and if there is none, a new one. The
  // path filter from the settings starts from it.
  export async function openUpdateInfo(range: [string, string], paths: string[]) {
    await loaded();
    const same = (tab: LogTab) =>
      tab.update?.range[0] === range[0] && tab.update.range[1] === range[1];
    const existing = tabs.find(same);
    if (existing) {
      if (existing.id !== currentId) await switchTab(existing.id);
      return;
    }
    const id = nextTab++;
    const name = new Intl.DateTimeFormat(undefined, {
      dateStyle: "medium",
      timeStyle: "short",
    }).format(new Date());
    const filters = { ...emptyFilters(), ranges: [range], paths };
    await switchTab(id, { ...newTab(id), filters, update: { range, name } });
    await openView(id);
  }

  async function switchTab(id: number, created?: LogTab) {
    const saved = logView?.snapshot() ?? null;
    patchTab(currentId, { saved });
    if (created) tabs = [...tabs, created];
    selection = [];
    change = null;
    diffOpen = false;
    currentId = id;
    await tick();
    const snapshot = tabOf(id)?.saved;
    if (snapshot) await logView?.restore(snapshot, false);
  }

  async function closeTab(id: number) {
    const fallback = tabs[tabs.findIndex((tab) => tab.id === id) - 1].id;
    if (id === currentId) await switchTab(fallback);
    tabs = tabs.filter((tab) => tab.id !== id);
    latest.delete(id);
    clicks.delete(id);
    saveOptions();
    logViewClose(id).catch(() => {});
  }

  function showViewOptions(anchor: HTMLElement) {
    const bounds = anchor.getBoundingClientRect();
    viewMenu = { x: bounds.left, y: bounds.bottom };
  }

  // `FocusTextFilterAction`.
  function toggleTextFilter() {
    if (filterBar?.hasFocus()) logView?.focus();
    else filterBar?.focusText();
  }

  function keydown(event: KeyboardEvent) {
    const action = matchShortcut(event);
    if (action === "Vcs.Log.FocusTextFilter") {
      toggleTextFilter();
      event.preventDefault();
    } else if (action === "Diff.ShowDiff") {
      if (selection.length === 0) return;
      diffOpen = true;
      event.preventDefault();
    } else {
      logActions?.keydown(event, action);
    }
  }

  function closeDiff() {
    diffOpen = false;
    logView?.focus();
  }

  // Activating the log window moves the focus to the table, and the keyboard listens on this panel.
  $effect(() => {
    if (active && logView) untrack(() => logView?.focus());
  });

  export function openCommit(hash: string): Promise<void> {
    return new Promise((resolve) => {
      diffOpened = resolve;
      diffOpen = true;
      logView?.selectHash(hash);
    });
  }

  export function perfTarget() {
    return {
      scroller: logView?.scrollElement() ?? null,
      viewId: view?.viewId ?? 0,
      count: view?.count ?? 0,
    };
  }

  function drag(event: PointerEvent, key: keyof Splits, vertical: boolean) {
    const divider = event.currentTarget as HTMLElement;
    const bounds = divider.parentElement!.getBoundingClientRect();
    divider.setPointerCapture?.(event.pointerId);
    const move = (moved: PointerEvent) => {
      const part = vertical
        ? (moved.clientY - bounds.top) / bounds.height
        : (moved.clientX - bounds.left) / bounds.width;
      const value = Math.min(1 - MIN_SPLIT, Math.max(MIN_SPLIT, part));
      patchTab(currentId, {
        options: { ...options, splits: { ...options.splits, [key]: value } },
      });
    };
    const finish = () => {
      divider.removeEventListener("pointermove", move);
      divider.removeEventListener("pointerup", finish);
      saveOptions();
    };
    divider.addEventListener("pointermove", move);
    divider.addEventListener("pointerup", finish);
  }

  onMount(() => {
    const stop = onRepoChanged(async () => {
      if (!tabs[0].view || needsLoad) return;
      try {
        if (await logRefsChanged()) needsLoad = true;
      } catch {
        // If git fails now, the next signal compares again.
      }
    });
    return () => {
      stop.then((unlisten) => unlisten());
    };
  });
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="panel" onkeydown={keydown}>
  {#if loading && !view}
    <p class="status">Cargando…</p>
  {:else if view && repoEmpty}
    <p class="status">{NO_COMMITS}</p>
  {:else if view}
    <LogTabs
      tabs={tabs.map((tab) => ({ id: tab.id, name: tabName(tab), closable: tab.id !== MAIN_TAB }))}
      current={currentId}
      newTab={NEW_TAB}
      onselect={switchTab}
      onclose={closeTab}
      onnew={() => openTab()}
    />

    <div class="body">
      {#if options.showBranches}
        <div class="branches" style:flex={options.splits.branches}>
          <BranchesPanel
            {active}
            onfilter={filterByBranches}
            onnavigate={(branch) => logActions?.goToBranch(branch)}
            onhide={() => setOptions({ showBranches: false })}
            {onvcs}
            {onfetch}
          />
        </div>
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
          class="divider columns"
          role="separator"
          aria-orientation="vertical"
          onpointerdown={(event) => drag(event, "branches", false)}
        ></div>
      {:else}
        <button
          type="button"
          class="branches-stripe"
          aria-label="Branches"
          title="Branches"
          onclick={() => setOptions({ showBranches: true })}
        >
          <Icon name="right" size={14} />
        </button>
      {/if}
      <div
        class="frame"
        data-location={options.diffPreviewAt}
        style:flex={options.showBranches ? 1 - options.splits.branches : 1}
      >
        <div class="main" style:flex={showDiff ? options.splits.diff : 1}>
          <div class="table" style:flex={options.splits.table}>
            {#key current.id}
              <FilterBar
                bind:this={filterBar}
                filters={current.filters}
                {refs}
                {users}
                busy={working > 0 || loading}
                onchange={filtersChanged}
                onrefresh={refresh}
                onviewoptions={showViewOptions}
                ongoto={() => logActions?.goToRef()}
              />
            {/key}
            <div class="rows">
              {#key current.id}
                <LogView
                  bind:this={logView}
                  viewId={view.viewId}
                  count={view.count}
                  generation={current.generation}
                  {options}
                  onselect={select}
                  {onready}
                  ongraphclick={graphClick}
                  oncontext={(x, y) => logActions?.showContextMenu(x, y)}
                />
              {/key}
              {#if view.count === 0}
                <div class="empty">
                  <p>{NO_MATCHES}</p>
                  <button type="button" class="link" onclick={() => filtersChanged(emptyFilters())}>Reset filters</button>
                </div>
              {/if}
            </div>
          </div>

          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div
            class="divider columns"
            role="separator"
            aria-orientation="vertical"
            onpointerdown={(event) => drag(event, "table", false)}
          ></div>

          <div class="side" style:flex={1 - options.splits.table}>
            <div class="pane" style:flex={options.showDetails ? options.splits.details : 1}>
              <ChangesPanel
                hashes={selection}
                paths={current.filters.paths}
                {options}
                onoptions={setOptions}
                onselect={(next) => (change = next)}
                onempty={() => diffOpened?.()}
                onopen={() => (diffOpen = true)}
                {onvcs}
              />
            </div>
            {#if options.showDetails}
              <!-- svelte-ignore a11y_no_static_element_interactions -->
              <div
                class="divider rows-split"
                role="separator"
                aria-orientation="horizontal"
                onpointerdown={(event) => drag(event, "details", true)}
              ></div>
              <div class="pane" style:flex={1 - options.splits.details}>
                <DetailsPanel
                  hashes={selection}
                  generation={current.generation}
                  onnavigate={(hash) => logView?.selectHash(hash)}
                />
              </div>
            {/if}
          </div>
        </div>

        {#if showDiff}
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div
            class="divider"
            class:columns={options.diffPreviewAt === "right"}
            class:rows-split={options.diffPreviewAt === "bottom"}
            role="separator"
            onpointerdown={(event) => drag(event, "diff", options.diffPreviewAt === "bottom")}
          ></div>
          <section class="preview" style:flex={1 - options.splits.diff} aria-label="Diff Preview">
            {#if diffOpen && !options.diffPreview}
              <button
                type="button"
                class="action close-diff"
                aria-label="Close"
                onclick={closeDiff}
              >
                <Icon name="close" size={14} />
              </button>
            {/if}
            {#if change}
              {#await import("../diff/DiffView.svelte") then { default: DiffView }}
                <DiffView
                  hash={change.hash}
                  file={change.file}
                  base={change.base}
                  onopened={() => diffOpened?.()}
                />
              {/await}
            {/if}
          </section>
        {/if}
      </div>
    </div>
  {/if}
</div>

{#if view}
  <LogActions
    bind:this={logActions}
    {logView}
    viewId={view.viewId}
    {refs}
    {currentBranch}
    hashes={selection}
    filtered={isFiltered(current.filters)}
    onresetfilters={() => filtersChanged(emptyFilters())}
    onnewtab={() => openTab()}
    {onvcs}
  />
{/if}

{#if viewMenu && view}
  <ViewOptions
    {...viewMenu}
    {options}
    collapsible={view.collapsible}
    empty={view.count === 0}
    onoptions={setOptions}
    onsort={changeSort}
    onlongedges={changeLongEdges}
    oncollapse={changeCollapsed}
    onclose={() => (viewMenu = null)}
  />
{/if}

<style>
  .panel {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    height: 100%;
  }

  .status {
    padding: 16px;
    margin: 0;
    color: var(--text-secondary);
  }

  .body {
    display: flex;
    flex: 1;
    min-height: 0;
  }

  .branches {
    min-width: 0;
    min-height: 0;
  }

  .branches-stripe {
    display: flex;
    flex: none;
    justify-content: center;
    width: 24px;
    padding: 6px 0 0;
    color: var(--icon);
    background: var(--panel);
    border: 0;
    border-right: 1px solid var(--divider);
  }

  .frame {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }

  .frame[data-location="right"] {
    flex-direction: row;
  }

  .main {
    display: flex;
    min-width: 0;
    min-height: 0;
  }

  .table,
  .side {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }

  .rows {
    position: relative;
    flex: 1;
    min-height: 0;
  }

  .empty {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
    align-items: center;
    justify-content: center;
    color: var(--text-secondary);
  }

  .empty p {
    margin: 0;
  }

  .link {
    padding: 0;
    font: inherit;
    color: var(--link);
    background: none;
    border: 0;
  }

  .pane {
    min-height: 0;
  }

  .divider {
    flex: none;
    background: var(--divider);
  }

  .divider.columns {
    width: 1px;
    cursor: col-resize;
    touch-action: none;
  }

  .divider.rows-split {
    height: 1px;
    cursor: row-resize;
    touch-action: none;
  }

  .preview {
    position: relative;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
    background: var(--editor);
  }

  .close-diff {
    position: absolute;
    top: 4px;
    right: 12px;
    z-index: 2;
  }
</style>
