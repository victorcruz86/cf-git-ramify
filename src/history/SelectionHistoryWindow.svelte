<script lang="ts">
  import { onMount, tick } from "svelte";
  import TextsDiff from "../diff/TextsDiff.svelte";
  import {
    annotationShowInLog,
    onSelectionHistoryOpen,
    onSelectionHistoryUpdate,
    selectionHistoryClose,
    selectionHistoryStart,
    selectionHistoryTarget,
    showWindow,
    taskCancel,
    type ApiError,
    type SelectionRequest,
    type SelectionUpdate,
  } from "../lib/api";
  import { CANNOT_OPEN, hostingItems, OPEN_ON_GITHUB } from "../hosting/links";
  import { errorText } from "../lib/errors";
  import Menu, { type MenuItem } from "../lib/Menu.svelte";
  import { stored, store } from "../lib/storage";
  import { taskResult, trackTasks } from "../lib/tasks";
  import { compareRows, compareWithLocalFile, copyRevisionNumbers, showAffectedFiles } from "./actions";
  import {
    authorText,
    authorTooltip,
    blockDiff,
    cannotLoad,
    CHANGES_ONLY,
    COLUMNS,
    comparedRows,
    COMMIT_MESSAGE_LABEL,
    dateText,
    emptyData,
    keptSelection,
    LOADING_REVISIONS,
    messageText,
    selectionActions,
    statusText,
    subjectText,
    versionText,
    visibleRows,
    WINDOW_TITLE,
    withUpdate,
    type HistoryData,
  } from "./selection";

  // `VcsSelectionHistoryDialog` (241), non-modal: the diff of the selected block on top (0.5),
  // below the table with `Changes only` and the loading status (0.8), then `Commit Message:`.
  // Revision texts, authors and messages are untrusted: shown as text only.

  // `VcsConfiguration.SHOW_ONLY_CHANGED_IN_SELECTION_DIFF`, on by default.
  const CHANGES_ONLY_KEY = "rama.selectionHistory.changesOnly";

  let request = $state.raw<SelectionRequest | null>(null);
  let data = $state.raw<HistoryData>(emptyData());
  let changesOnly = $state(stored(CHANGES_ONLY_KEY) !== "false");
  // The selected rows, as indexes of `data.rows`, in order.
  let selected = $state.raw<number[]>([]);
  // Where a ⇧-click extends the selection from, as an index of `data.rows`.
  let anchor: number | null = null;
  // `notifyError`'s balloon: `Cannot load revision contents: …`.
  let failure = $state<string | null>(null);
  let menu = $state.raw<{ x: number; y: number; items: MenuItem[] } | null>(null);
  let now = $state(Date.now());
  let table = $state<HTMLElement>();

  // The latest stream: an update or an end of an older one is dropped. A reopened window does not
  // start from the numbers of the one before it.
  let stream = Date.now() % 1_000_000_000;
  let task: number | null = null;
  // `myUpdateQueue` (`MergingUpdateQueue`, 300 ms): the blocks that arrive are shown together; the
  // history and the end are shown at once (`notifyUpdate(true)`).
  const MERGE_DELAY = 300;
  let queued: SelectionUpdate[] = [];
  let flushTimer: ReturnType<typeof setTimeout> | null = null;

  const visible = $derived(visibleRows(data, changesOnly));
  const diff = $derived(blockDiff(data, selected));
  // `setRequest(request, IntPair)`: the diff is built again only when what it shows changes, not on
  // each update of the stream. Strings and numbers compare by value, so an update that leaves the
  // pair as it was reaches no one.
  const older = $derived(diff.kind === "texts" ? diff.older : null);
  const newer = $derived(diff.kind === "texts" ? diff.newer : null);
  const olderText = $derived(older?.text ?? "");
  const newerText = $derived(newer?.text ?? "");
  const olderStart = $derived(older?.start ?? 0);
  const newerStart = $derived(newer?.start ?? 0);
  const olderTitle = $derived(older?.title ?? "");
  const newerTitle = $derived(newer?.title ?? "");
  const status = $derived(statusText(data));
  const actions = $derived(selectionActions(data.rows, selected));
  const message = $derived(selected.length === 1 ? messageText(data.rows[selected[0]]) : "");

  // `updateRevisionsList`: what was selected stays selected where it is still shown.
  function show(next: HistoryData) {
    data = next;
    now = Date.now();
    select(keptSelection(selected, visibleRows(next, changesOnly)));
  }

  // The same rows stay the same selection: what depends on it is not computed again.
  function select(next: number[]) {
    const same = next.length === selected.length && next.every((index, at) => index === selected[at]);
    if (!same) selected = next;
  }

  function flush() {
    if (flushTimer !== null) clearTimeout(flushTimer);
    flushTimer = null;
    if (queued.length === 0) return;
    const updates = queued;
    queued = [];
    show(updates.reduce(withUpdate, data));
  }

  function receive(update: SelectionUpdate) {
    if (update.stream !== stream) return;
    queued.push(update);
    if (update.kind === "revisions") flush();
    else if (flushTimer === null) flushTimer = setTimeout(flush, MERGE_DELAY);
  }

  function dropQueued() {
    if (flushTimer !== null) clearTimeout(flushTimer);
    flushTimer = null;
    queued = [];
  }

  function cancelTask() {
    if (task !== null) taskCancel(task).catch(() => {});
    task = null;
  }

  // `BlockLoader.start`, for a new selection: the previous stream is cancelled.
  async function begin(target: SelectionRequest) {
    const current = ++stream;
    cancelTask();
    dropQueued();
    request = target;
    data = emptyData();
    selected = [];
    anchor = null;
    failure = null;
    menu = null;
    try {
      const id = await selectionHistoryStart({ ...target, stream: current });
      if (current !== stream) {
        taskCancel(id).catch(() => {});
        return;
      }
      task = id;
      await taskResult<null>(id);
      if (current === stream) finish(null);
    } catch (error) {
      if (current !== stream) return;
      if ((error as ApiError)?.kind === "cancelled") {
        finish(null);
        return;
      }
      const text = errorText(error);
      finish(text);
      failure = cannotLoad(text);
    }
  }

  function finish(error: string | null) {
    task = null;
    flush();
    show({ ...data, loading: false, error });
  }

  async function take() {
    const target = await selectionHistoryTarget();
    if (target) void begin(target);
  }

  function toggleChangesOnly(on: boolean) {
    changesOnly = on;
    store(CHANGES_ONLY_KEY, String(on));
    selected = keptSelection(selected, visibleRows(data, on));
  }

  // `closeOnEsc`: the stream is cancelled with the window.
  function close() {
    cancelTask();
    selectionHistoryClose().catch(() => {});
  }

  async function reveal(index: number) {
    await tick();
    table?.querySelector<HTMLElement>(`[data-row="${index}"]`)?.scrollIntoView?.({ block: "nearest" });
  }

  function choose(index: number, event: MouseEvent | KeyboardEvent) {
    if (event.metaKey) {
      selected = selected.includes(index)
        ? selected.filter((one) => one !== index)
        : [...selected, index].sort((a, b) => a - b);
      anchor = index;
      return;
    }
    if (event.shiftKey && anchor !== null) {
      const [from, to] = [Math.min(anchor, index), Math.max(anchor, index)];
      selected = visible.filter((one) => one >= from && one <= to);
      return;
    }
    selected = [index];
    anchor = index;
  }

  function step(by: number, event: KeyboardEvent) {
    if (visible.length === 0) return;
    const lead = selected.length === 0 ? -1 : visible.indexOf(by > 0 ? selected.at(-1)! : selected[0]);
    const next = visible[Math.min(Math.max(lead + by, 0), visible.length - 1)];
    choose(next, event);
    void reveal(next);
  }

  const rowsOf = (indexes: number[]) => indexes.map((index) => data.rows[index]);

  function run(work: () => Promise<void>) {
    work().catch((error) => (failure = errorText(error)));
  }

  // `MyDiffAction`.
  function compare() {
    const pair = comparedRows(data.rows, selected);
    if (!actions.compare || !pair || !request) return;
    const path = request.path;
    run(() => compareRows(pair.older, pair.newer, path));
  }

  function compareWithLocal() {
    if (!actions.compareWithLocal || !request) return;
    const [row, path] = [data.rows[selected[0]], request.path];
    run(() => compareWithLocalFile(row, path));
  }

  function showAffected() {
    if (!actions.showAllAffected) return;
    const row = data.rows[selected[0]];
    run(() => showAffectedFiles(row));
  }

  // `Git.SelectInGitLog` (`GitShowCommitInLogAction`): the commit selected in the main Log, as a
  // click on an annotation does.
  function showInLog() {
    if (!actions.showInGitLog) return;
    const row = data.rows[selected[0]];
    if (row.kind === "revision") run(() => annotationShowInLog(row.revision.hash));
  }

  function copyNumbers() {
    if (!actions.copyRevisionNumber) return;
    const rows = rowsOf(selected);
    run(() => copyRevisionNumbers(rows));
  }

  // `VcsSelectionHistoryDialog.Popup` (`VA:479-484`), with the `Git.SelectInGitLog` git4idea adds
  // to it and `Git.Hosting.Open.In.Browser.Group` last (`g4x:522`), for the commit of the first
  // selected row. `MyDiffAction.update` names `Diff.ShowDiff` `Compare` (`V:163`) here, whatever the
  // selection.
  function popupItems(hosting: MenuItem[] = []): MenuItem[] {
    return [
      { label: "Compare", shortcut: "⌘D", disabled: !actions.compare, run: compare },
      { label: "Compare with Local", disabled: !actions.compareWithLocal, run: compareWithLocal },
      {
        label: "Show All Affected Files",
        shortcut: "⌃⌘A",
        disabled: !actions.showAllAffected,
        run: showAffected,
      },
      {
        label: "Copy Revision Number",
        shortcut: "⌥⇧⌘C",
        disabled: !actions.copyRevisionNumber,
        run: copyNumbers,
      },
      // git4idea `plugin.xml:40-42`: after `Vcs.CopyRevisionNumberAction` (`vcs.history.action.gitlog`).
      { label: "Show in Git Log", disabled: !actions.showInGitLog, run: showInLog },
      ...hosting,
    ];
  }

  // This window has no balloons: a failure is its own banner.
  const hostingFailed = (error: unknown) => (failure = `${CANNOT_OPEN}: ${errorText(error)}`);

  async function hostingOfSelection(): Promise<MenuItem[]> {
    const row = data.rows[selected[0]];
    if (!row || row.kind !== "revision") return [];
    return hostingItems(
      { kind: "commit", hash: row.revision.hash },
      "open",
      OPEN_ON_GITHUB,
      hostingFailed,
    );
  }

  async function contextmenu(index: number, event: MouseEvent) {
    event.preventDefault();
    if (!selected.includes(index)) {
      selected = [index];
      anchor = index;
    }
    const { clientX: x, clientY: y } = event;
    const hosting = await hostingOfSelection();
    menu = { x, y, items: popupItems(hosting) };
  }

  function keydown(event: KeyboardEvent) {
    if (menu) return;
    const key = event.key.toLowerCase();
    if (event.key === "Escape") {
      event.preventDefault();
      close();
    } else if (event.metaKey && !event.altKey && !event.ctrlKey && key === "d") {
      event.preventDefault();
      compare();
    } else if (event.metaKey && event.ctrlKey && key === "a") {
      event.preventDefault();
      showAffected();
    } else if (event.metaKey && event.altKey && event.shiftKey && event.code === "KeyC") {
      event.preventDefault();
      copyNumbers();
    }
  }

  function tableKeydown(event: KeyboardEvent) {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      step(event.key === "ArrowDown" ? 1 : -1, event);
    }
  }

  onMount(() => {
    document.title = WINDOW_TITLE;
    const stops = [
      onSelectionHistoryUpdate(receive),
      onSelectionHistoryOpen(() => void take().catch((error) => (failure = errorText(error)))),
      trackTasks(),
    ];
    (async () => {
      try {
        await Promise.all(stops);
        await take();
      } catch (error) {
        failure = errorText(error);
      } finally {
        await showWindow().catch(() => {});
        table?.focus();
      }
    })();
    return () => {
      stream++;
      cancelTask();
      dropQueued();
      stops.forEach((stop) => stop.then((unlisten) => unlisten()));
    };
  });
</script>

<svelte:window onkeydown={keydown} />

<main>
  <section class="diff" aria-label="Diff">
    {#if diff.kind === "texts"}
      <TextsDiff
        oldText={olderText}
        newText={newerText}
        path={request?.path ?? ""}
        titles={[olderTitle, newerTitle]}
        starts={[olderStart, newerStart]}
        place="Default"
      />
    {:else}
      <div class="message">{diff.text}</div>
    {/if}
  </section>

  <section class="bottom">
    <div class="list">
      <div class="bar">
        <label class="changes-only">
          <input
            type="checkbox"
            checked={changesOnly}
            onchange={(event) => toggleChangesOnly(event.currentTarget.checked)}
          />
          {CHANGES_ONLY}
        </label>
        <span class="status" role="status">
          {#if status}<span class="busy" aria-hidden="true"></span>{status}{/if}
        </span>
      </div>
      {#if failure}
        <div class="failure" role="alert">
          <span>{failure}</span>
          <button type="button" aria-label="Close" onclick={() => (failure = null)}>×</button>
        </div>
      {/if}
      <div
        class="table"
        role="grid"
        aria-label="Revisions"
        aria-multiselectable="true"
        tabindex="0"
        bind:this={table}
        onkeydown={tableKeydown}
      >
        <div class="row head" role="row">
          {#each COLUMNS as column (column)}
            <span class="cell" role="columnheader">{column}</span>
          {/each}
        </div>
        {#if data.rows.length === 0}
          <div class="empty">{LOADING_REVISIONS}</div>
        {/if}
        {#each visible as index (index)}
          {@const row = data.rows[index]}
          <div
            class="row"
            class:selected={selected.includes(index)}
            role="row"
            tabindex="-1"
            aria-selected={selected.includes(index)}
            data-row={index}
            onclick={(event) => choose(index, event)}
            oncontextmenu={(event) => contextmenu(index, event)}
            onkeydown={() => {}}
          >
            <span class="cell" role="gridcell">{versionText(row)}</span>
            <span class="cell" role="gridcell">{dateText(row, now)}</span>
            <span class="cell" role="gridcell" title={authorTooltip(row)}>{authorText(row)}</span>
            <span class="cell" role="gridcell">{subjectText(row)}</span>
          </div>
        {/each}
      </div>
    </div>
    <div class="comments">
      <div class="label">{COMMIT_MESSAGE_LABEL}</div>
      <div class="message-text" aria-label="Commit message">{message}</div>
    </div>
  </section>
</main>

{#if menu}
  <Menu x={menu.x} y={menu.y} label="Revisions" items={menu.items} onclose={() => (menu = null)} />
{/if}

<style>
  main {
    display: flex;
    flex-direction: column;
    height: 100vh;
    color: var(--text);
    background: var(--panel);
  }

  .diff {
    display: flex;
    flex: 1 1 50%;
    flex-direction: column;
    min-height: 0;
    background: var(--editor);
    border-bottom: 1px solid var(--divider);
  }

  .message {
    display: flex;
    flex: 1;
    align-items: center;
    justify-content: center;
    color: var(--text-secondary);
  }

  .bottom {
    display: flex;
    flex: 1 1 50%;
    flex-direction: column;
    min-height: 0;
  }

  .list {
    display: flex;
    flex: 4 1 0;
    flex-direction: column;
    min-height: 0;
    padding: 0 16px 4px;
  }

  .bar {
    display: grid;
    grid-template-columns: 1fr auto 1fr;
    align-items: center;
    min-height: 28px;
  }

  .changes-only {
    display: flex;
    gap: 6px;
    align-items: center;
  }

  .status {
    display: flex;
    gap: 6px;
    align-items: center;
    color: var(--text-secondary);
  }

  .busy {
    width: 12px;
    height: 12px;
    border: 2px solid var(--divider);
    border-top-color: var(--focus);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .failure {
    display: flex;
    gap: 8px;
    align-items: center;
    justify-content: space-between;
    padding: 4px 8px;
    margin-bottom: 4px;
    background: var(--notice-error);
    border: 1px solid var(--notice-error-border);
  }

  .failure button {
    color: inherit;
    background: none;
    border: none;
  }

  .table {
    flex: 1;
    min-height: 0;
    overflow: auto;
    background: var(--editor);
    border: 1px solid var(--border);
    outline: none;
  }

  .row {
    display: grid;
    grid-template-columns: 110px 150px 160px 1fr;
    align-items: center;
    height: 24px;
    cursor: default;
  }

  .row.head {
    position: sticky;
    top: 0;
    color: var(--text-secondary);
    background: var(--panel);
    border-bottom: 1px solid var(--divider);
  }

  .row.selected {
    background: var(--selection);
  }

  .cell {
    padding: 0 6px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .empty {
    padding: 16px;
    color: var(--text-secondary);
    text-align: center;
  }

  .comments {
    display: flex;
    flex: 1 1 0;
    flex-direction: column;
    gap: 4px;
    min-height: 0;
    padding: 0 16px 16px;
  }

  .message-text {
    flex: 1;
    min-height: 0;
    padding: 4px 6px;
    overflow: auto;
    white-space: pre-wrap;
    background: var(--editor);
    border: 1px solid var(--border);
  }
</style>
