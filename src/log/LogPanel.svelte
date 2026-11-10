<script lang="ts">
  import { onMount, tick, untrack } from "svelte";
  import {
    logComparedBranch,
    logCurrentBranch,
    logGraphClick,
    logHistoryOpen,
    logNotPicked,
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
    taskCancel,
    type ApiError,
    type GraphTarget,
    type LogRef,
    type LogSummary,
    type NotPicked,
    type Person,
    type SortType,
    type ViewOpened,
    type ViewPatch,
    type ViewSpec,
  } from "../lib/api";
  import { errorText } from "../lib/errors";
  import Icon from "../lib/Icon.svelte";
  import { matchShortcut } from "../lib/keymap";
  import Menu, { type MenuItem } from "../lib/Menu.svelte";
  import { notify } from "../lib/notifications";
  import { taskResult } from "../lib/tasks";
  import BranchesPanel from "../branches/BranchesPanel.svelte";
  import { favorites } from "../branches/state";
  import type { LogChange } from "./changes";
  import CompareBranchesTab, { type HalfName } from "./CompareBranchesTab.svelte";
  import ChangesPanel from "./ChangesPanel.svelte";
  import DetailsPanel from "./DetailsPanel.svelte";
  import FilterBar from "./FilterBar.svelte";
  import type { VcsAction } from "./contextMenu";
  import {
    branchMenu,
    branchValues,
    displayText,
    emptyFilters,
    splitBranchValues,
    toLogFilter,
    type FilterState,
  } from "./filters";
  import LogActions from "./LogActions.svelte";
  import HistoryTab from "./HistoryTab.svelte";
  import {
    fileName,
    notFoundText,
    SHOW_ALL_BRANCHES,
    unsupportedFilterText,
    VIEW_IN_LOG,
  } from "./history";
  import {
    LOG_TAB_KINDS,
    onLogTabRequested,
    openLogTab,
    type CompareTarget,
    type HistoryTarget,
    type LogTabInit,
    type LogTabKind,
    type LogTabRequest,
  } from "./tabKinds";
  import LogTabs from "./LogTabs.svelte";
  import LogView from "./LogView.svelte";
  import { defaultOptions, historyOptions, readOptions, type LogOptions, type Splits } from "./options";
  import type { SelectionSnapshot } from "./selection";
  import ViewOptions from "./ViewOptions.svelte";

  const MAIN_TAB = 0;
  const TAB_NAME = "Log";
  const NEW_TAB = "Open New Git Log Tab";
  const NO_MATCHES = "No commits matching filters";
  const NO_COMMITS = "This repository has no commits yet";
  const MIN_SPLIT = 0.1;
  // `DeepCompareAction` and `DeepComparator` (B:1050, B:1052, B:1053).
  const SELECT_SOURCE_BRANCH = "Select Source Branch";
  const compareFailed = (branch: string) => `Could not compare with branch ${branch}`;
  const compareCancelled = (branch: string) =>
    `Highlighting of non-picked commits in '${branch}' has been cancelled, as branch filter was changed.`;
  // `setFadeoutTime(5000)`.
  const BALLOON_MS = 5000;
  // Half the menu's minimum width: `showInCenterOf` centres the popup.
  const SOURCE_MENU_HALF_WIDTH = 110;

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
    // What the tab is (`tabKinds.ts`), the key a request finds it by, and the name it was given.
    kind: LogTabKind;
    key: string | null;
    name: string;
    // A `history` tab: its file, and why the history could not be computed (`reset`: a filter the
    // history does not support, which `Reset filters` undoes).
    history: HistoryTarget | null;
    failure: { message: string; reset: boolean } | null;
    // A `compare` tab: its branches, and its lower log (`end..start`), a view with its own tab id;
    // the tab's own view and filters are the upper log (`start..end`).
    compare: CompareTarget | null;
    lower: LowerLog | null;
    // `Not Cherry-Picked Commits` (`DeepComparator`), while it runs or highlights.
    notPicked: NotPickedRun | null;
  }

  // The branch compared with the current one (`HEAD` when detached), the task that compares them
  // (null until it is started), and the commits not picked, once known (`nonPickedCommits`; null
  // again after a failure, while the toggle stays checked, as in 241).
  interface NotPickedRun {
    run: number;
    branch: string;
    current: string;
    task: number | null;
    commits: ReadonlySet<string> | null;
  }

  interface LowerLog {
    id: number;
    filters: FilterState;
    view: LogSummary | null;
    generation: number;
  }

  interface Props {
    active: boolean;
    onready?: () => void;
    // The branch operations of the context menu. What it returns is awaited where several go one
    // after the other (the Branches panel).
    onvcs?: (action: VcsAction) => void | Promise<void>;
    onfetch?: () => Promise<unknown>;
    // `Create Patch...` of the changes tree.
    oncreatepatch?: (changes: LogChange[]) => void;
    // The repository's folder, for the Paths filter's `Select in Tree…`.
    root?: string;
  }

  let { active, onready, onvcs, onfetch, oncreatepatch, root = "" }: Props = $props();

  const newTab = (id: number, options = defaultOptions()): LogTab => ({
    id,
    filters: emptyFilters(),
    options,
    collapsed: false,
    view: null,
    generation: 0,
    saved: null,
    kind: "log",
    key: null,
    name: "",
    history: null,
    failure: null,
    compare: null,
    lower: null,
    notPicked: null,
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
  // The changes panel's own changes of the selection, for the menu's Create Patch....
  let directChanges = $state.raw<LogChange[]>([]);
  let diffOpen = $state(false);
  let viewMenu = $state.raw<{ x: number; y: number } | null>(null);
  // `Select Source Branch`, for the tab that asked, and the balloon that says a comparison stopped.
  let sourceMenu = $state.raw<{ x: number; y: number; id: number } | null>(null);
  let balloon = $state<string | null>(null);
  let balloonTimer: ReturnType<typeof setTimeout> | undefined;
  let notPickedRuns = 0;
  let menuPress: { x: number; y: number } | null = null;
  let panelElement = $state<HTMLDivElement>();
  let logView = $state<LogView>();
  // The table of a history tab, which `HistoryTab` holds.
  let historyView = $state<LogView>();
  let filterBar = $state<FilterBar>();
  let logActions = $state<LogActions>();
  let historyTab = $state<HistoryTab>();
  let compareTab = $state<CompareBranchesTab>();
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

  // The table on screen: the Log's, that of a history tab, or the upper log of a compare tab (the
  // lower one keeps its selection in `openLower`).
  const table = () =>
    current.history ? historyView : current.compare ? compareTab?.table("upper") : logView;
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
    if (tab.kind !== "log") return LOG_TAB_KINDS[tab.kind].title(tab.name);
    const { filters } = tab;
    const values = [filters.text, ...branchValues(filters), ...filters.users, ...filters.paths];
    return LOG_TAB_KINDS.log.title(displayText(values.filter(Boolean)));
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
      tabs
        .filter((tab) => LOG_TAB_KINDS[tab.kind].persisted)
        .map((tab) => [tab.id, tab.history ? { ...tab.options, tab: savedTab(tab) } : tab.options]),
    );
    logOptionsSet(stored).catch(() => {});
  }

  // What brings a history tab back when the repository is opened again: 241 reopens them when the
  // log is created (`FileHistoryTabsManager.logCreated`).
  const savedTab = (tab: LogTab) => ({ kind: tab.kind, key: tab.key, name: tab.name, history: tab.history });

  function restoredTab(id: number, stored: unknown): LogTab {
    const { tab, ...options } = (typeof stored === "object" && stored !== null ? stored : {}) as {
      tab?: { kind?: unknown; key?: unknown; name?: unknown; history?: Partial<HistoryTarget> };
    };
    const history = tab?.history;
    const isHistory = id !== MAIN_TAB && tab?.kind === "history" && typeof history?.path === "string";
    const base = newTab(id, readOptions(options, id === MAIN_TAB, isHistory ? historyOptions() : undefined));
    if (!isHistory || typeof history?.path !== "string") return base;
    const revision = typeof history.revision === "string" ? history.revision : null;
    return {
      ...base,
      kind: "history",
      key: typeof tab.key === "string" ? tab.key : null,
      name: typeof tab.name === "string" ? tab.name : fileName(history.path),
      history: { path: history.path, revision },
      filters: { ...emptyFilters(), branches: revision ? [] : ["HEAD"] },
    };
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
    const shown = id === currentId ? table() : undefined;
    const snapshot = shown?.snapshot();
    patchTab(id, { view: summary, generation: tab.generation + 1 });
    if (!snapshot) return;
    await tick();
    await table()?.restore(snapshot, changed);
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
    // `GitLogHistoryHandler.throwIfFiltersNotSupported`: only branches and revisions start a
    // history; a range is refused before git is asked.
    if (tab.history && !tab.history.revision && tab.filters.ranges.length > 0) {
      const filter = displayText(branchValues(tab.filters));
      patchTab(id, { failure: { message: unsupportedFilterText(filter), reset: true } });
      return;
    }
    opening.set(id, token);
    working++;
    try {
      const task = tab.history
        ? await logHistoryOpen(id, specOf(tab), {
            path: tab.history.path,
            revision: tab.history.revision,
            branches: tab.history.revision ? [] : [...tab.filters.branches, ...tab.filters.revisions],
          })
        : await logViewOpen(id, specOf(tab));
      const { summary } = await taskResult<ViewOpened>(task);
      if (latest.get(id) === token) {
        if (tab.history) patchTab(id, { failure: null });
        await install(id, summary, changed);
      }
    } catch (failure) {
      // `ErrorVisiblePack`: the history tab says it in its table.
      if (cancelled(failure)) return;
      if (tab.history && latest.get(id) === token) {
        patchTab(id, { failure: { message: errorText(failure), reset: false } });
      }
      else if (!tab.history) showError(failure);
    } finally {
      if (opening.get(id) === token) opening.delete(id);
      working--;
    }
  }

  // The lower log of a compare tab: the reversed range, with its own filters and tab id.
  async function openLower(id: number, changed = false) {
    const tab = tabOf(id);
    const lower = tab?.lower;
    if (!tab || !lower) return;
    const token = nextToken(lower.id);
    working++;
    try {
      const task = await logViewOpen(lower.id, { ...specOf(tab), filter: toLogFilter(lower.filters) });
      const { summary } = await taskResult<ViewOpened>(task);
      const now = tabOf(id)?.lower;
      if (now && latest.get(lower.id) === token) {
        const shown = id === currentId ? compareTab?.table("lower") : undefined;
        const snapshot = shown?.snapshot();
        patchTab(id, { lower: { ...now, view: summary, generation: now.generation + 1 } });
        if (snapshot) {
          await tick();
          await compareTab?.table("lower")?.restore(snapshot, changed);
        }
      }
    } catch (failure) {
      if (!cancelled(failure)) showError(failure);
    } finally {
      working--;
    }
  }

  // Every view of a tab: a compare tab has two.
  async function openTabViews(id: number, changed = false) {
    await Promise.all([openView(id, changed), openLower(id, changed)]);
  }

  // A filter of one of the logs of a compare tab: only that log is read again.
  async function compareFilters(id: number, half: HalfName, filters: FilterState) {
    const tab = tabOf(id);
    if (!tab?.lower) return;
    if (half === "upper") {
      patchTab(id, { filters });
      await openView(id);
    } else {
      patchTab(id, { lower: { ...tab.lower, filters } });
      await openLower(id);
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
    tabs = [MAIN_TAB, ...others].map((id) => restoredTab(id, stored[id]));
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
            : openTabViews(tab.id, true),
        ),
      );
      if (summary.count > 0) await loadRefs();
      void notPickedRefreshed();
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
    const id = currentId;
    patchTab(id, { filters });
    await Promise.all([openView(id), notPickedFiltersChanged(id)]);
  }

  // Where the last press inside a menu was: the toggle chosen with the mouse shows the popup there
  // (`RelativePoint(MouseEvent)`), and with the keyboard in the centre of the log.
  function pressed(event: PointerEvent) {
    const inMenu = event.target instanceof Element && event.target.closest('[role="menu"]');
    menuPress = inMenu ? { x: event.clientX, y: event.clientY } : null;
  }

  function sourceMenuPoint(): { x: number; y: number } {
    if (menuPress) return menuPress;
    const bounds = panelElement?.getBoundingClientRect();
    if (!bounds) return { x: 0, y: 0 };
    return { x: bounds.left + bounds.width / 2 - SOURCE_MENU_HALF_WIDTH, y: bounds.top + bounds.height / 3 };
  }

  // `DeepCompareAction.setSelected`: with the branch of the filters it compares at once; otherwise
  // it asks which one.
  async function toggleNotPicked(id: number) {
    const at = sourceMenuPoint();
    const tab = tabOf(id);
    if (!tab) return;
    if (tab.notPicked) {
      stopNotPicked(id);
      return;
    }
    let branch: string | null;
    try {
      branch = await logComparedBranch(toLogFilter(tab.filters));
    } catch (failure) {
      showError(failure);
      return;
    }
    if (branch === null) {
      sourceMenu = { ...at, id };
      return;
    }
    // In 241 the comparison starts with the click: a filter change meanwhile is its `update`, which
    // stops it with the balloon when the filters name another branch.
    let asked = tab.filters;
    for (let now = tabOf(id)?.filters; now && now !== asked; now = tabOf(id)?.filters) {
      asked = now;
      let next: string | null = branch;
      try {
        next = await logComparedBranch(toLogFilter(now));
      } catch {
        // Without an answer the filters are taken as they were.
      }
      if (next !== branch) {
        showBalloon(compareCancelled(branch));
        return;
      }
    }
    if (!tabOf(id) || tabOf(id)?.notPicked) return;
    await startNotPicked(id, branch);
  }

  // `selectBranchAndPerformAction`: the branch replaces whatever was in the Branch filter, and is
  // compared.
  async function chooseSourceBranch(id: number, branch: string) {
    const tab = tabOf(id);
    if (!tab) return;
    patchTab(id, { filters: { ...tab.filters, ...splitBranchValues([branch]) } });
    await Promise.all([openView(id), startNotPicked(id, branch)]);
  }

  // What `BranchPopupBuilder` gives without recent ones: the favourites, then each group.
  function sourceBranchItems(id: number): MenuItem[] {
    const { favorites: favorite, groups } = branchMenu(refs, [], $favorites);
    const item = (name: string) => ({ label: name, run: () => void chooseSourceBranch(id, name) });
    return [
      ...favorite.map(item),
      ...(groups.length > 0 ? [null] : []),
      ...groups.map(({ label, values }) => ({ label, submenu: values.map(item) })),
    ];
  }

  // `DeepComparator.startTask`: only a branch of the log is compared, and the current branch is
  // that of the log as it was read.
  async function startNotPicked(id: number, branch: string) {
    const tab = tabOf(id);
    if (!tab) return;
    // The run is the tab's at once, as `comparedBranch` is set before the task: a toggle or a
    // filter change meanwhile already sees it, and a newer run makes this one stale.
    const run = ++notPickedRuns;
    const before = tab.notPicked;
    cancelNotPicked(before);
    // Comparing again keeps what was highlighted until the new result arrives (`onSuccess`).
    const kept = before?.branch === branch ? before.commits : null;
    patchTab(id, {
      notPicked: { run, branch, current: currentBranch ?? "HEAD", task: null, commits: kept },
    });
    const live = () => tabOf(id)?.notPicked?.run === run;
    let task: number | null;
    try {
      task = await logNotPicked(branch);
    } catch (failure) {
      if (!live()) return;
      patchTab(id, { notPicked: null });
      showError(failure);
      return;
    }
    if (!live()) {
      if (task !== null) taskCancel(task).catch(() => {});
      return;
    }
    // Not a branch of the log (`getRepositories` is empty): nothing is compared.
    if (task === null) {
      patchTab(id, { notPicked: null });
      return;
    }
    patchTab(id, { notPicked: { ...tabOf(id)!.notPicked!, task } });
    try {
      const { current, commits } = await taskResult<NotPicked>(task);
      if (!live()) return;
      patchTab(id, { notPicked: { run, branch, current, task: null, commits: new Set(commits) } });
    } catch (failure) {
      if (!live()) return;
      // `onSuccess` with an exception: `nonPickedCommits = null`, the toggle stays checked.
      patchTab(id, { notPicked: { ...tabOf(id)!.notPicked!, task: null, commits: null } });
      if (!cancelled(failure)) {
        notify({ kind: "error", title: compareFailed(branch), body: errorText(failure) });
      }
    }
  }

  function cancelNotPicked(state: NotPickedRun | null) {
    if (state?.task != null) taskCancel(state.task).catch(() => {});
  }

  function stopNotPicked(id: number) {
    cancelNotPicked(tabOf(id)?.notPicked ?? null);
    patchTab(id, { notPicked: null });
  }

  // `DeepComparator.update`: another branch in the filters stops it, and says so.
  async function notPickedFiltersChanged(id: number) {
    const tab = tabOf(id);
    const state = tab?.notPicked;
    if (!tab || !state) return;
    let branch: string | null;
    try {
      branch = await logComparedBranch(toLogFilter(tab.filters));
    } catch {
      return;
    }
    // Whatever run is the tab's now (a refresh may have started another meanwhile) is the one
    // these filters are checked against.
    const now = tabOf(id);
    const current = now?.notPicked;
    if (!now || !current || now.filters !== tab.filters) return;
    if (branch === current.branch) return;
    stopNotPicked(id);
    showBalloon(compareCancelled(current.branch));
  }

  // `DeepComparator.update` after a refresh: the filters with the new references name another
  // branch, and it stops with the balloon; otherwise it compares again, unless the branch is gone
  // or the current branch is another one, and then it stops without a word.
  async function notPickedRefreshed() {
    const current = currentBranch ?? "HEAD";
    await Promise.all(
      tabs.map(async ({ id, notPicked: state, filters }) => {
        if (!state) return;
        let branch: string | null = state.branch;
        try {
          branch = await logComparedBranch(toLogFilter(filters));
        } catch {
          // Without an answer the filters are taken as they were.
        }
        // A filter change meanwhile is checked by `notPickedFiltersChanged`, with the new filters.
        const now = tabOf(id);
        if (now?.notPicked?.run !== state.run || now.filters !== filters) return;
        if (branch !== state.branch) {
          stopNotPicked(id);
          showBalloon(compareCancelled(state.branch));
          return;
        }
        const exists = refs.some(
          (ref) => ref.name === state.branch && (ref.kind === "local" || ref.kind === "remote"),
        );
        if (exists && state.current === current) void startNotPicked(id, state.branch);
        else stopNotPicked(id);
      }),
    );
  }

  function showBalloon(text: string) {
    clearTimeout(balloonTimer);
    balloon = text;
    balloonTimer = setTimeout(() => (balloon = null), BALLOON_MS);
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

  // `GitUpdateInfoAsLog.findOrCreateLogUi`: the tab with that range, and if there is none, a new one
  // named after the moment. The path filter from the settings starts from it.
  export function openUpdateInfo(range: [string, string], paths: string[]): Promise<void> {
    return openLogTab("update", `${range[0]}..${range[1]}`, () => ({
      name: new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" }).format(new Date()),
      filters: { ...emptyFilters(), ranges: [range], paths },
    }));
  }

  // A request of `openLogTab`, once the log is loaded: back to the tab of that kind and key when the
  // kind is reused, otherwise a new tab.
  async function showKindTab(
    kind: LogTabKind,
    key: string,
    init: () => LogTabInit,
    { matches, select: target }: LogTabRequest,
  ) {
    await loaded();
    const rule = LOG_TAB_KINDS[kind];
    const existing = matches
      ? tabs.find((tab) => tab.kind === kind && matches(tab.filters))
      : rule.reused
        ? tabs.find((tab) => tab.kind === kind && tab.key === key)
        : undefined;
    if (existing) {
      if (existing.id !== currentId) await switchTab(existing.id);
      if (target) await jumpTo(existing.id, target);
      return;
    }
    const { name, filters, history = null, compare = null, options } = init();
    const id = nextTab++;
    const created = newTab(id, history ? historyOptions() : undefined);
    // `GitCompareBranchesUi.create`: `start..end` above, `asReversed()` below.
    const ranged = (from: string, to: string): FilterState => ({ ...filters, ranges: [[from, to]] });
    const lower = compare
      ? { id: nextTab++, filters: ranged(compare.end, compare.start), view: null, generation: 0 }
      : null;
    await switchTab(id, {
      ...created,
      kind,
      key,
      name,
      filters: compare ? ranged(compare.start, compare.end) : filters,
      history,
      compare,
      lower,
      options: { ...created.options, ...options },
    });
    if (rule.persisted) saveOptions();
    await openTabViews(id);
    // `selectRowWhenOpen`: the first row, or the commit asked for.
    if (target) await jumpTo(id, target);
    else if (history) {
      await tick();
      table()?.jumpToRow(0, true);
    }
  }

  // `jumpToNearestCommit` of a history tab. 241 does it silently; Ramify says it with `L:165-168` when
  // the commit is not in the history.
  async function jumpTo(id: number, hash: string) {
    const tab = tabOf(id);
    if (id !== currentId || !tab?.view) return;
    await tick();
    const found = await table()?.selectHash(hash);
    if (found !== false || !tab.history) return;
    const branches = !tab.history.revision && tab.filters.branches.length > 0;
    notify({
      kind: "warning",
      title: notFoundText(hash, fileName(tab.history.path), branches),
      actions: [
        ...(branches
          ? [{ label: SHOW_ALL_BRANCHES, run: () => void historyBranches(id, [], hash) }]
          : []),
        { label: VIEW_IN_LOG, run: () => void showInMainLog(hash) },
      ],
    });
  }

  // The Branch filter of a history tab (`FileHistoryFilterUi`), then the commit to show, if any.
  async function historyBranches(id: number, branches: string[], hash?: string) {
    const tab = tabOf(id);
    if (!tab) return;
    patchTab(id, { filters: { ...tab.filters, ...splitBranchValues(branches) } });
    await openView(id);
    if (hash) await jumpTo(id, hash);
  }

  // `GitShowCommitInLogAction` / `runInMainLog`: the commit selected in the main tab.
  async function showInMainLog(hash: string) {
    if (currentId !== MAIN_TAB) await switchTab(MAIN_TAB);
    await tick();
    await logView?.selectHash(hash);
  }

  async function switchTab(id: number, created?: LogTab) {
    const saved = table()?.snapshot() ?? null;
    patchTab(currentId, { saved });
    if (created) tabs = [...tabs, created];
    selection = [];
    change = null;
    diffOpen = false;
    currentId = id;
    await tick();
    const snapshot = tabOf(id)?.saved;
    if (snapshot) await table()?.restore(snapshot, false);
  }

  async function closeTab(id: number) {
    const fallback = tabs[tabs.findIndex((tab) => tab.id === id) - 1].id;
    if (id === currentId) await switchTab(fallback);
    const lower = tabOf(id)?.lower;
    cancelNotPicked(tabOf(id)?.notPicked ?? null);
    tabs = tabs.filter((tab) => tab.id !== id);
    latest.delete(id);
    clicks.delete(id);
    saveOptions();
    logViewClose(id).catch(() => {});
    if (lower) {
      latest.delete(lower.id);
      logViewClose(lower.id).catch(() => {});
    }
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
    if (current.kind === "history" && historyTab?.keydown(event, action)) {
      event.preventDefault();
      return;
    }
    if (current.compare) {
      compareTab?.keydown(event, action);
      return;
    }
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
    if (active && current.compare && compareTab) {
      const shown = compareTab;
      untrack(() => shown.focus());
      return;
    }
    const shown = current.history ? historyView : logView;
    if (active && shown) untrack(() => shown.focus());
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
    const stopRequests = onLogTabRequested(showKindTab);
    const stop = onRepoChanged(async () => {
      if (!tabs[0].view || needsLoad) return;
      try {
        if (await logRefsChanged()) needsLoad = true;
      } catch {
        // If git fails now, the next signal compares again.
      }
    });
    return () => {
      stopRequests();
      stop.then((unlisten) => unlisten());
      clearTimeout(balloonTimer);
      tabs.forEach((tab) => cancelNotPicked(tab.notPicked));
    };
  });
</script>

<svelte:window onpointerdowncapture={pressed} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="panel" bind:this={panelElement} onkeydown={keydown}>
  {#if loading && !view}
    <p class="status">Loading…</p>
  {:else if view && repoEmpty}
    <p class="status">{NO_COMMITS}</p>
  {:else if view || current.history || current.compare}
    <LogTabs
      tabs={tabs.map((tab) => ({ id: tab.id, name: tabName(tab), closable: tab.id !== MAIN_TAB }))}
      current={currentId}
      newTab={NEW_TAB}
      onselect={switchTab}
      onclose={closeTab}
      onnew={() => openTab()}
    />

    {#if current.history}
      {#key current.id}
        <HistoryTab
          bind:this={historyTab}
          bind:logView={historyView}
          {view}
          generation={current.generation}
          {options}
          history={current.history}
          filters={current.filters}
          {refs}
          failure={current.failure}
          busy={working > 0 || loading}
          {onready}
          onoptions={setOptions}
          onbranches={(branches) => historyBranches(current.id, branches)}
          onresetfilters={() => historyBranches(current.id, ["HEAD"])}
          onrefresh={refresh}
          onshowinlog={showInMainLog}
          {onvcs}
          {oncreatepatch}
        />
      {/key}
    {:else if current.compare && current.lower}
      {#key current.id}
        <CompareBranchesTab
          bind:this={compareTab}
          upper={{
            start: current.compare.start,
            end: current.compare.end,
            view: current.view,
            generation: current.generation,
            filters: current.filters,
          }}
          lower={{
            start: current.compare.end,
            end: current.compare.start,
            view: current.lower.view,
            generation: current.lower.generation,
            filters: current.lower.filters,
          }}
          {options}
          {refs}
          {users}
          {currentBranch}
          {root}
          busy={working > 0 || loading}
          onfilters={(half, filters) => compareFilters(current.id, half, filters)}
          onoptions={setOptions}
          onrefresh={refresh}
          onnewtab={() => openTab()}
          {onready}
          {onvcs}
          {oncreatepatch}
        />
      {/key}
    {:else if view}
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
                  {root}
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
                    notPicked={current.notPicked?.commits ?? null}
                  />
                {/key}
                {#if balloon}
                  <div class="balloon" role="status">{balloon}</div>
                {/if}
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
                  onchanges={(changes) => (directChanges = changes)}
                  {onvcs}
                  {oncreatepatch}
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
    paths={current.filters.paths}
    {oncreatepatch}
    changes={directChanges}
  />
{/if}

{#if viewMenu && view}
  {@const id = currentId}
  <ViewOptions
    {...viewMenu}
    {options}
    collapsible={view.collapsible}
    empty={view.count === 0}
    onoptions={setOptions}
    onsort={changeSort}
    onlongedges={changeLongEdges}
    oncollapse={changeCollapsed}
    notPicked={{ checked: current.notPicked !== null, toggle: () => void toggleNotPicked(id) }}
    onclose={() => (viewMenu = null)}
  />
{/if}

{#if sourceMenu}
  <Menu
    x={sourceMenu.x}
    y={sourceMenu.y}
    label={SELECT_SOURCE_BRANCH}
    title={SELECT_SOURCE_BRANCH}
    items={sourceBranchItems(sourceMenu.id)}
    onclose={() => (sourceMenu = null)}
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

  /* `MessageType.INFO.popupBackground`, at the top centre of the log. */
  .balloon {
    position: absolute;
    top: 8px;
    left: 50%;
    z-index: 3;
    max-width: 80%;
    padding: 6px 10px;
    color: var(--text);
    background: var(--panel);
    border: 1px solid var(--divider);
    border-radius: 6px;
    transform: translateX(-50%);
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
