<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { logFindRows, logGraphHover, logRows, type GraphTarget, type LogRow } from "../lib/api";
  import {
    columnWidths,
    drawRows,
    MAX_ROWS_TO_MEASURE,
    ROW_HEIGHT,
    type ColumnWidths,
    type GraphElement,
    type GraphHighlight,
    type LabelBounds,
  } from "./draw";
  import { arrowTooltip, type ArrowTooltip } from "./goto";
  import { elementAt, jumpRow, targetOf } from "./graphHit";
  import type { LogOptions } from "./options";
  import { RowCache, visibleRange } from "./rowCache";
  import {
    restoreSelection,
    snapshotSelection,
    type SelectionSnapshot,
  } from "./selection";

  const OVERSCAN = 5;
  const INITIAL_WIDTHS: ColumnWidths = { author: 150, date: 100, hash: 72 };
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
  let tip = $state.raw<(ArrowTooltip & { x: number; y: number }) | null>(null);
  const labelBounds = new Map<number, LabelBounds>();

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
      if (token !== restoring) return;
      const { rows, scrollTop } = restoreSelection(saved, found, changed, ROW_HEIGHT);
      setSelection(rows);
      if (scrollTop !== null) scroller.scrollTop = scrollTop;
    } finally {
      if (token === restoring) pending = null;
    }
    await publish();
  }

  export async function selectHash(hash: string): Promise<void> {
    const row = (await rowsOf([hash])).get(hash);
    if (row === undefined) return;
    setSelection([row]);
    reveal(row);
    await publish();
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
      if (token !== selecting) return;
      const hash = cache.get(row)?.hash;
      if (hash) hashes.push(hash);
    }
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
        widths: widths ?? INITIAL_WIDTHS,
        now: Date.now(),
        highlight,
        labelBounds,
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
    return { row, element: found ? elementAt(found.print, x, y) : null };
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
  style:cursor={hand ? "pointer" : null}
  onscroll={schedule}
  onclick={click}
  oncontextmenu={contextmenu}
  onmousemove={mousemove}
  onmouseleave={clearHover}
  onkeydown={keydown}
>
  <canvas bind:this={canvas}></canvas>
  <div class="spacer" style:height="{count * ROW_HEIGHT}px"></div>
</div>

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
