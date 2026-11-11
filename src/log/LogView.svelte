<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { logFindRows, logGraphHover, logRows, type GraphTarget, type LogRow } from "../lib/api";
  import {
    borderAt,
    columnWidths,
    dragWidths,
    effectiveWidths,
    drawRows,
    MAX_ROWS_TO_MEASURE,
    ROW_HEIGHT,
    SIGNATURE_WIDTH,
    tableLayout,
    type ColumnWidths,
    type GraphElement,
    type GraphHighlight,
    type LabelBounds,
    type Border,
    type SavedWidths,
  } from "./draw";
  import { arrowTooltip, type ArrowTooltip } from "./goto";
  import { elementAt, jumpRow, targetOf } from "./graphHit";
  import type { LogOptions } from "./options";
  import { RowCache, visibleRange } from "./rowCache";
  import { SIGNATURE_ROWS_AROUND, SignatureColumn, signatureText } from "./signatures";
  import {
    restoreSelection,
    snapshotSelection,
    type SelectionSnapshot,
  } from "./selection";

  const OVERSCAN = 5;
  const INITIAL_WIDTHS: ColumnWidths = { author: 150, date: 100, hash: 72, signature: SIGNATURE_WIDTH };
  // `Balloon.Position.atRight`, 5 from the mouse.
  const TOOLTIP_OFFSET = 5;

  interface Props {
    viewId: number;
    count: number;
    // Another view, or the same one with other content: what is in the cache is no longer valid.
    generation: number;
    options: LogOptions;
    onselect: (hashes: string[]) => void;
    onready?: () => void;
    // Click on a node or on an edge without an arrow.
    ongraphclick?: (target: GraphTarget) => void;
    oncontext?: (x: number, y: number) => void;
    // `Not Cherry-Picked Commits`: the commits that keep their style, the others in grey.
    notPicked?: ReadonlySet<string> | null;
    // A border dragged or reset: the widths the user set, for the tab to keep (`COLUMN_ID_WIDTH`).
    onwidths?: (widths: SavedWidths) => void;
  }

  let {
    viewId,
    count,
    generation,
    options,
    onselect,
    onready,
    ongraphclick,
    oncontext,
    notPicked = null,
    onwidths,
  }: Props = $props();

  // Props are re-read on any change in the parent: a new cache, only if this changes.
  const version = $derived(`${viewId}:${generation}`);
  const cache = $derived.by(() => {
    version;
    const id = untrack(() => viewId);
    return new RowCache<LogRow>((start, size) => logRows(id, start, size));
  });
  let scroller: HTMLDivElement;
  let canvas: HTMLCanvasElement;
  let context: CanvasRenderingContext2D | null = null;
  let selected = new Set<number>();
  // Where the extension with ⇧ starts, and where the cursor is.
  let anchor: number | null = null;
  let lead: number | null = null;
  let widths: ColumnWidths | null = null;
  let frame = 0;
  let ready = false;
  let selecting = 0;
  // The last thing reported to `onselect`: the cache does not hold the whole selection.
  let published: string[] = [];
  // While restoring, what is on screen does not yet say what was selected.
  let restoring = 0;
  let pending: SelectionSnapshot | null = null;
  // What is under the mouse and what that marks. The graph knows nothing about it: it is applied when painting.
  let hovered = "";
  let hovering = 0;
  // One hover question at a time: a quick sweep does not flood the rows thread with traversals.
  let asking = false;
  let nextHover: GraphTarget | null = null;
  let highlight: GraphHighlight | null = null;
  let hand = $state(false);
  // Set when the table is unmounted: what git answers afterwards (a tab switch, a closed panel) has
  // no table to select or scroll in.
  let destroyed = false;
  let tip = $state.raw<(ArrowTooltip & { x: number; y: number }) | null>(null);
  // The `GPG Signature` cell under the mouse: its presentation's text.
  let signatureTip = $state.raw<{ text: string; x: number; y: number } | null>(null);
  const signatures = new SignatureColumn(() => schedule());
  const labelBounds = new Map<number, LabelBounds>();
  // A border being dragged, the widths it gives so far, and what a view with no `onwidths` keeps.
  let resizing: { border: Border; startX: number; start: ColumnWidths; moved: boolean } | null = null;
  let live: SavedWidths | null = null;
  let kept: SavedWidths | null = null;
  let onBorder = $state(false);
  // The click that ends a press on a border must not select.
  let afterResize = false;

  export function scrollElement(): HTMLDivElement {
    return scroller;
  }

  export function focus() {
    scroller.focus();
  }

  export function snapshot(): SelectionSnapshot {
    if (pending) return pending;
    return snapshotSelection({
      selected: [...selected].sort((a, b) => a - b),
      hashes: published,
      hashAt: (row) => cache.get(row)?.hash,
      scrollTop: scroller.scrollTop,
      viewportHeight: scroller.clientHeight,
      rowHeight: ROW_HEIGHT,
      count,
    });
  }

  async function rowsOf(hashes: string[]): Promise<Map<string, number>> {
    if (hashes.length === 0) return new Map();
    const rows = await logFindRows(viewId, hashes);
    return new Map(
      hashes.flatMap((hash, index) => {
        const row = rows[index];
        return row === null ? [] : [[hash, row] as const];
      }),
    );
  }

  // `changed`: the commit list is a different one, not just its order or what is folded.
  export async function restore(saved: SelectionSnapshot, changed: boolean): Promise<void> {
    const token = ++restoring;
    pending = saved;
    const wanted = saved.target ? [...saved.hashes, saved.target.hash] : saved.hashes;
    try {
      const found = await rowsOf([...new Set(wanted)]);
      if (token !== restoring || destroyed) return;
      const { rows, scrollTop } = restoreSelection(saved, found, changed, ROW_HEIGHT);
      setSelection(rows);
      if (scrollTop !== null) scroller.scrollTop = scrollTop;
    } finally {
      if (token === restoring) pending = null;
    }
    await publish();
  }

  // `false` when the commit is not in the view, or the table is gone by the time git answers.
  export async function selectHash(hash: string): Promise<boolean> {
    const row = (await rowsOf([hash])).get(hash);
    if (row === undefined || destroyed) return false;
    setSelection([row]);
    reveal(row);
    await publish();
    return true;
  }

  // `VcsLogGraphTable.jumpToRow`.
  export function jumpToRow(row: number, focusTable: boolean) {
    if (row < 0 || row >= count) return;
    setSelection([row]);
    reveal(row);
    publish();
    if (focusTable) focus();
  }

  export const selectedRow = (): number | null => (selected.size === 1 ? [...selected][0] : null);

  export async function commitAt(row: number): Promise<LogRow | undefined> {
    const reading = cache;
    if (!reading.loaded(row)) await reading.ensure(row, row + 1);
    return reading.get(row);
  }

  // One by one: a selection with ⇧ can cross more than fits in the cache.
  export async function selectedCommits(): Promise<LogRow[]> {
    const commits: LogRow[] = [];
    for (const row of [...selected].sort((a, b) => a - b)) {
      const commit = await commitAt(row);
      if (commit) commits.push(commit);
    }
    return commits;
  }

  // `ScrollingUtil.isVisible`.
  export function isVisible(row: number): boolean {
    const top = row * ROW_HEIGHT;
    return top >= scroller.scrollTop && top + ROW_HEIGHT <= scroller.scrollTop + scroller.clientHeight;
  }

  function rowTop(row: number): number {
    return scroller.getBoundingClientRect().top + row * ROW_HEIGHT - scroller.scrollTop;
  }

  export function center(): { x: number; y: number } {
    const bounds = scroller.getBoundingClientRect();
    return { x: bounds.left + bounds.width / 2, y: bounds.top + bounds.height / 2 };
  }

  export function belowRow(row: number): { x: number; y: number } {
    return { x: scroller.getBoundingClientRect().left, y: rowTop(row) + ROW_HEIGHT };
  }

  // `getTooltipXCoordinate`: below the center of the labels.
  export function tooltipPoint(row: number): { x: number; y: number } {
    const bounds = scroller.getBoundingClientRect();
    const labels = labelBounds.get(row);
    const x = labels ? labels.x + labels.width / 2 : bounds.width / 2;
    return { x: bounds.left + x, y: rowTop(row) + ROW_HEIGHT };
  }

  function setSelection(rows: number[]) {
    selected = new Set(rows);
    lead = rows.at(-1) ?? null;
    anchor = rows[0] ?? null;
    schedule();
  }

  // A row may not be in the cache: a selection with ⇧ crosses more than fits in it.
  async function publish() {
    const token = ++selecting;
    const rows = [...selected].sort((a, b) => a - b);
    const hashes: string[] = [];
    for (const row of rows) {
      if (!cache.loaded(row)) await cache.ensure(row, row + 1);
      if (token !== selecting || destroyed) return;
      const hash = cache.get(row)?.hash;
      if (hash) hashes.push(hash);
    }
    if (destroyed) return;
    published = hashes;
    onselect(hashes);
  }

  function schedule() {
    if (frame === 0) frame = requestAnimationFrame(paint);
  }

  function measure() {
    if (widths || !context || !cache.loaded(0)) return;
    const rows: LogRow[] = [];
    for (let row = 0; row < Math.min(count, MAX_ROWS_TO_MEASURE) && cache.loaded(row); row++) {
      const found = cache.get(row);
      if (found) rows.push(found);
    }
    widths = columnWidths(context, rows, Date.now());
  }

  function paint() {
    frame = 0;
    // A page of rows can arrive when the view is already unmounted.
    if (!scroller) return;
    const width = scroller.clientWidth;
    const height = scroller.clientHeight;
    const scrollTop = scroller.scrollTop;
    const range = visibleRange(scrollTop, height, ROW_HEIGHT, count, OVERSCAN);
    requestSignatures(scrollTop, height);
    measure();
    if (context) {
      drawRows({
        context,
        width,
        height,
        scrollTop,
        range,
        rowAt: (index) => cache.get(index),
        selected,
        options,
        widths: shownWidths(),
        now: Date.now(),
        highlight,
        labelBounds,
        notPicked,
        signatures,
      });
    }
    if (range.end === range.start) return;
    if (!cache.loaded(range.start) || !cache.loaded(range.end - 1)) {
      const loading = cache;
      loading.ensure(range.start, range.end).then(() => {
        if (loading === cache) schedule();
      });
    } else if (!ready) {
      ready = true;
      onready?.();
    }
  }

  // Only while the column is shown, for the visible rows and `SIGNATURE_ROWS_AROUND` more: another
  // view, other rows, or rows that arrive later ask again. Rows past the painted ones are read too,
  // without waiting for them.
  function requestSignatures(scrollTop: number, height: number) {
    const around = visibleRange(scrollTop, height, ROW_HEIGHT, count, SIGNATURE_ROWS_AROUND);
    const shown = options.columns.includes("signature") && around.end > around.start;
    const reading = cache;
    const loaded = shown && reading.loaded(around.start) && reading.loaded(around.end - 1);
    if (shown && !loaded) {
      reading.ensure(around.start, around.end).then(() => {
        if (reading === cache) schedule();
      });
    }
    signatures.request(shown, `${version}:${around.start}:${around.end}:${loaded}`, () => {
      const hashes: string[] = [];
      for (let row = around.start; row < around.end; row++) {
        const hash = reading.get(row)?.hash;
        if (hash) hashes.push(hash);
      }
      return hashes;
    });
  }

  function signatureUnder(event: MouseEvent): string | null {
    if (!options.columns.includes("signature")) return null;
    const { row, x } = rowAt(event);
    const found = row < count ? cache.get(row) : undefined;
    if (!found) return null;
    const layout = tableLayout(scroller.clientWidth, options.columns, shownWidths());
    const cell = layout.cells.find((cell) => cell.column === "signature");
    if (!cell || x < cell.x || x >= cell.x + cell.width) return null;
    return signatureText(signatures.get(found.hash));
  }

  // `updateDynamicColumnsWidth`: the width being dragged, else the one the user set, else automatic.
  function shownWidths(): ColumnWidths {
    const saved = live ?? (onwidths ? options.columnWidths : (kept ?? options.columnWidths));
    return effectiveWidths(widths ?? INITIAL_WIDTHS, saved, scroller.clientWidth);
  }

  function borderUnder(event: MouseEvent): Border | null {
    const bounds = scroller.getBoundingClientRect();
    const layout = tableLayout(scroller.clientWidth, options.columns, shownWidths());
    return borderAt(layout, options.columns, event.clientX - bounds.left);
  }

  function savedNow(): SavedWidths {
    return { ...(onwidths ? options.columnWidths : (kept ?? options.columnWidths)) };
  }

  function report(next: SavedWidths) {
    if (onwidths) onwidths(next);
    else kept = next;
    schedule();
  }

  function mousedown(event: MouseEvent) {
    if (event.button !== 0) return;
    const border = borderUnder(event);
    if (!border) return;
    // No text selection or drag image; the table still takes the focus, as a press anywhere else.
    event.preventDefault();
    scroller.focus();
    resizing = { border, startX: event.clientX, start: shownWidths(), moved: false };
    onBorder = true;
    window.addEventListener("mousemove", resizeMove);
    window.addEventListener("mouseup", resizeEnd);
    window.addEventListener("blur", resizeEnd);
  }

  function resizeMove(event: MouseEvent) {
    if (!resizing) return;
    // The release was lost (another window, ⌘-Tab, a menu): the resize ends where it was.
    if ((event.buttons & 1) === 0) {
      resizeEnd();
      return;
    }
    const delta = event.clientX - resizing.startX;
    if (delta !== 0) resizing.moved = true;
    const dragged = dragWidths(resizing.start, resizing.border, delta, scroller.clientWidth, options.columns);
    live = { ...savedNow(), ...dragged };
    schedule();
  }

  function resizeEnd() {
    const done = resizing;
    const next = live;
    stopResize();
    afterResize = true;
    setTimeout(() => (afterResize = false));
    if (done?.moved && next) report(next);
    else schedule();
  }

  function stopResize() {
    window.removeEventListener("mousemove", resizeMove);
    window.removeEventListener("mouseup", resizeEnd);
    window.removeEventListener("blur", resizeEnd);
    resizing = null;
    live = null;
  }

  // A double click on a border: the column goes back to its automatic width.
  function dblclick(event: MouseEvent) {
    const border = borderUnder(event);
    if (!border) return;
    // `FileHistoryPanel.invokeOnDoubleClick`: no diff while a column is being resized.
    event.stopPropagation();
    const next = savedNow();
    delete next[border.column];
    report(next);
  }

  function resize() {
    const ratio = window.devicePixelRatio;
    canvas.width = scroller.clientWidth * ratio;
    canvas.height = scroller.clientHeight * ratio;
    context = canvas.getContext("2d", { alpha: false });
    context?.scale(ratio, ratio);
    // Resizing clears the canvas: waiting for the next frame would show a black one.
    cancelAnimationFrame(frame);
    paint();
  }

  function rowAt(event: MouseEvent): { row: number; x: number; y: number } {
    const bounds = scroller.getBoundingClientRect();
    const top = event.clientY - bounds.top + scroller.scrollTop;
    const row = Math.floor(top / ROW_HEIGHT);
    return { row, x: event.clientX - bounds.left, y: top - row * ROW_HEIGHT };
  }

  function between(from: number, to: number): number[] {
    const [start, end] = from < to ? [from, to] : [to, from];
    return Array.from({ length: end - start + 1 }, (_, index) => start + index);
  }

  function extendTo(row: number) {
    const start = anchor ?? row;
    selected = new Set(between(start, row));
    anchor = start;
    lead = row;
  }

  function elementUnder(event: MouseEvent): { row: number; element: GraphElement | null } {
    const { row, x, y } = rowAt(event);
    const found = row < count ? cache.get(row) : undefined;
    // The graph is drawn where the Subject column starts, after Author and Date in a file history.
    const { subjectX } = tableLayout(scroller.clientWidth, options.columns, shownWidths());
    return { row, element: found ? elementAt(found.print, x - subjectX, y) : null };
  }

  function mark(next: GraphHighlight | null, overGraph: boolean) {
    if (highlight !== next) schedule();
    highlight = next;
    hand = overGraph;
  }

  // `GraphAction.Type.MOUSE_OVER` with a null element, or after a click.
  function clearHover() {
    hovered = "";
    hovering++;
    nextHover = null;
    tip = null;
    mark(null, false);
  }

  async function showArrowTip(target: number, event: MouseEvent, token: number) {
    const { clientX, clientY } = event;
    const commit = await commitAt(target);
    if (commit && token === hovering) {
      tip = { ...arrowTooltip(commit), x: clientX + TOOLTIP_OFFSET, y: clientY };
    }
  }

  // `GraphCommitCellController.performMouseMove`.
  async function mousemove(event: MouseEvent) {
    if (resizing) return;
    onBorder = borderUnder(event) !== null;
    if (onBorder) {
      signatureTip = null;
      clearHover();
      return;
    }
    const signed = signatureUnder(event);
    signatureTip = signed === null ? null : { text: signed, x: event.clientX + TOOLTIP_OFFSET, y: event.clientY };
    const { row, element } = elementUnder(event);
    const key = element ? `${row}:${JSON.stringify(element)}` : "";
    if (key === hovered) return;
    if (!element) {
      clearHover();
      return;
    }
    hovered = key;
    tip = null;
    nextHover = null;
    const token = ++hovering;
    const target = jumpRow(element);
    if (target !== null) {
      mark({ kind: "element", row, element }, true);
      await showArrowTip(target, event, token);
      return;
    }
    nextHover = targetOf(row, element);
    if (!asking) await askHover();
  }

  async function askHover() {
    asking = true;
    while (nextHover) {
      const target = nextHover;
      const token = hovering;
      nextHover = null;
      const rows = await logGraphHover(viewId, target).catch(() => null);
      if (token === hovering) mark(rows && { kind: "rows", rows: new Set(rows) }, rows !== null);
    }
    asking = false;
  }

  // `GraphCommitCellController.shouldSelectCell`: a click on the graph does not select.
  function click(event: MouseEvent) {
    if (afterResize || borderUnder(event)) return;
    const { row, element } = elementUnder(event);
    const found = cache.get(row);
    if (!found || row >= count) return;
    if (element) {
      const target = jumpRow(element);
      clearHover();
      if (target !== null) jumpToRow(target, true);
      else ongraphclick?.(targetOf(row, element));
      return;
    }
    if (event.shiftKey) {
      extendTo(row);
    } else if (event.metaKey) {
      const next = new Set(selected);
      if (!next.delete(row)) next.add(row);
      selected = next;
      anchor = row;
      lead = row;
    } else {
      setSelection([row]);
    }
    schedule();
    publish();
  }

  // `JBTable`: with the right button a selection of several does not change.
  function contextmenu(event: MouseEvent) {
    event.preventDefault();
    const { row } = rowAt(event);
    if (row < count && selected.size < 2 && !selected.has(row)) {
      setSelection([row]);
      publish();
    }
    clearHover();
    oncontext?.(event.clientX, event.clientY);
  }

  function reveal(row: number) {
    const top = row * ROW_HEIGHT;
    if (top < scroller.scrollTop) scroller.scrollTop = top;
    else if (top + ROW_HEIGHT > scroller.scrollTop + scroller.clientHeight) {
      scroller.scrollTop = top + ROW_HEIGHT - scroller.clientHeight;
    }
  }

  function moveTo(row: number, extend: boolean) {
    const next = Math.max(0, Math.min(count - 1, row));
    if (extend) extendTo(next);
    else setSelection([next]);
    reveal(next);
    schedule();
    publish();
  }

  function keydown(event: KeyboardEvent) {
    if (count === 0) return;
    const page = Math.max(1, Math.floor(scroller.clientHeight / ROW_HEIGHT) - 1);
    const steps: Record<string, number> = {
      ArrowDown: (lead ?? -1) + 1,
      ArrowUp: (lead ?? 1) - 1,
      PageDown: (lead ?? 0) + page,
      PageUp: (lead ?? 0) - page,
      Home: 0,
      End: count - 1,
    };
    const target = steps[event.key];
    if (target === undefined) return;
    event.preventDefault();
    moveTo(target, event.shiftKey);
  }

  $effect(() => {
    cache;
    count;
    options;
    notPicked;
    untrack(() => {
      if (scroller) {
        clearHover();
        schedule();
      }
    });
  });

  onMount(() => {
    const observer = new ResizeObserver(resize);
    observer.observe(scroller);
    return () => {
      destroyed = true;
      stopResize();
      signatures.stop();
      observer.disconnect();
      cancelAnimationFrame(frame);
    };
  });
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions, a11y_click_events_have_key_events -->
<div
  class="scroller"
  bind:this={scroller}
  role="grid"
  aria-label="Commits"
  aria-rowcount={count}
  tabindex="0"
  style:cursor={onBorder ? "col-resize" : hand ? "pointer" : null}
  onscroll={schedule}
  onclick={click}
  onmousedown={mousedown}
  ondblclick={dblclick}
  oncontextmenu={contextmenu}
  onmousemove={mousemove}
  onmouseleave={() => {
    onBorder = false;
    signatureTip = null;
    clearHover();
  }}
  onkeydown={keydown}
>
  <canvas bind:this={canvas}></canvas>
  <div class="spacer" style:height="{count * ROW_HEIGHT}px"></div>
</div>

{#if signatureTip}
  <div class="tip" role="tooltip" style:left="{signatureTip.x}px" style:top="{signatureTip.y}px">
    {signatureTip.text}
  </div>
{/if}

{#if tip}
  <div class="tip" role="tooltip" style:left="{tip.x}px" style:top="{tip.y}px">
    Jump to <b>{tip.subject}</b>{tip.rest}
  </div>
{/if}

<style>
  .scroller {
    position: relative;
    height: 100%;
    overflow-y: scroll;
    outline: none;
  }

  canvas {
    position: sticky;
    top: 0;
    display: block;
    width: 100%;
    height: 100%;
  }

  .spacer {
    position: absolute;
    top: 0;
    left: 0;
    width: 1px;
  }

  .tip {
    position: fixed;
    z-index: 20;
    padding: 4px 8px;
    color: var(--text);
    white-space: nowrap;
    pointer-events: none;
    background: var(--popup-background);
    border: 1px solid var(--popup-border);
    border-radius: 6px;
    box-shadow: 0 6px 24px rgb(0 0 0 / 40%);
  }
</style>
