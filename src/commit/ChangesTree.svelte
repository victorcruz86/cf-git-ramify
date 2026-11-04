<script lang="ts">
  import { untrack } from "svelte";
  import Icon from "../lib/Icon.svelte";
  import { visibleRange } from "../log/rowCache";
  import { rangeIds, reconcileSelection, sameIds, toggleId } from "./selection";
  import {
    CONFLICTS_ID,
    checkState,
    countText,
    flattenTree,
    originText,
    statusColor,
    type TreeNode,
    type TreeRow,
  } from "./tree";

  // Tree.rowHeight and Tree.border of the New UI.
  const ROW_HEIGHT = 24;
  const PADDING_Y = 4;
  const PADDING_X = 12;
  const INDENT = 20;
  const OVERSCAN = 8;
  // With no measurements yet (first paint) a tall window is assumed.
  const FALLBACK_HEIGHT = 600;
  const CHECKED = { on: "true", off: "false", mixed: "mixed" } as const;

  interface Props {
    roots: TreeNode[];
    included: Set<string>;
    partial?: Set<string>;
    collapsed: Set<string>;
    selected: Set<string>;
    lead: string | null;
    ontoggle: (nodes: TreeNode[]) => void;
    onopen: (node: TreeNode) => void;
    onmenu: (x: number, y: number) => void;
    // The `Resolve` link of the conflicts node.
    onresolve?: () => void;
    // The icon shown when hovering a row, as in `HoverChangesTree`.
    hoverIcon?: (node: TreeNode) => { icon: string; label: string } | null;
    onhover?: (node: TreeNode) => void;
  }

  let {
    roots,
    included,
    partial,
    collapsed = $bindable(),
    selected = $bindable(),
    lead = $bindable(),
    ontoggle,
    onopen,
    onmenu,
    onresolve,
    hoverIcon,
    onhover,
  }: Props = $props();

  let scroller: HTMLDivElement;
  let scrollTop = $state(0);
  let height = $state(0);
  let anchor: string | null = null;
  let leadIndex = 0;

  const rows = $derived(flattenTree(roots, collapsed));
  const range = $derived(
    visibleRange(
      Math.max(0, scrollTop - PADDING_Y),
      height || FALLBACK_HEIGHT,
      ROW_HEIGHT,
      rows.length,
      OVERSCAN,
    ),
  );
  const windowRows = $derived(rows.slice(range.start, range.end));

  $effect(() => {
    const current = rows;
    untrack(() => {
      const next = reconcileSelection(current, selected, lead, leadIndex);
      if (next.lead !== lead) lead = next.lead;
      if (!sameIds(next.selected, selected)) selected = next.selected;
      const index = current.findIndex((row) => row.node.id === next.lead);
      if (index >= 0) leadIndex = index;
    });
  });

  export function focus() {
    scroller.focus();
  }

  export function scrollElement(): HTMLDivElement {
    return scroller;
  }

  function keepFocus(event: MouseEvent) {
    event.preventDefault();
    scroller.focus();
  }

  function isExpandable(node: TreeNode): boolean {
    return node.children.length > 0;
  }

  function isOpen(node: TreeNode): boolean {
    return isExpandable(node) && !collapsed.has(node.id);
  }

  function setOpen(node: TreeNode, open: boolean) {
    const next = new Set(collapsed);
    if (open) next.delete(node.id);
    else next.add(node.id);
    // A row that gets hidden leaves its selection on the folder that is collapsed.
    if (!open && lead && lead !== node.id) {
      const hidden = flattenTree([node], new Set()).some((row) => row.node.id === lead);
      if (hidden) select(node.id);
    }
    collapsed = next;
  }

  function select(id: string) {
    selected = new Set([id]);
    lead = id;
    anchor = id;
  }

  function reveal(index: number) {
    const top = index * ROW_HEIGHT + PADDING_Y;
    const view = scroller.clientHeight;
    if (top < scroller.scrollTop) scroller.scrollTop = top - PADDING_Y;
    else if (view > 0 && top + ROW_HEIGHT > scroller.scrollTop + view) {
      scroller.scrollTop = top + ROW_HEIGHT - view + PADDING_Y;
    }
    // `scroll` arrives in the next frame: the new row must already be painted.
    scrollTop = scroller.scrollTop;
  }

  function moveTo(index: number, extend: boolean) {
    const row = rows[Math.max(0, Math.min(index, rows.length - 1))];
    if (!row) return;
    if (extend) {
      selected = new Set(rangeIds(rows, anchor ?? lead, row.node.id));
      lead = row.node.id;
    } else select(row.node.id);
    leadIndex = rows.indexOf(row);
    reveal(leadIndex);
  }

  function click(event: MouseEvent, row: TreeRow) {
    const id = row.node.id;
    if (event.shiftKey) {
      selected = new Set(rangeIds(rows, anchor ?? lead, id));
      lead = id;
    } else if (event.metaKey || event.ctrlKey) {
      selected = toggleId(selected, id);
      lead = id;
      anchor = id;
    } else select(id);
    leadIndex = rows.indexOf(row);
  }

  function clickChevron(event: MouseEvent, node: TreeNode) {
    event.stopPropagation();
    setOpen(node, !isOpen(node));
  }

  function clickCheck(event: MouseEvent, node: TreeNode) {
    event.stopPropagation();
    select(node.id);
    ontoggle([node]);
  }

  function doubleClick(event: MouseEvent, node: TreeNode) {
    if (isExpandable(node)) setOpen(node, !isOpen(node));
    else onopen(node);
  }

  function contextMenu(event: MouseEvent, row: TreeRow | null) {
    event.preventDefault();
    if (row && !selected.has(row.node.id)) {
      select(row.node.id);
      leadIndex = rows.indexOf(row);
    }
    onmenu(event.clientX, event.clientY);
  }

  function selectedNodes(): TreeNode[] {
    return rows.filter((row) => selected.has(row.node.id)).map((row) => row.node);
  }

  function keydown(event: KeyboardEvent) {
    if (event.metaKey || event.ctrlKey || event.altKey) {
      if (event.metaKey && event.key === "a") {
        selected = new Set(rows.map((row) => row.node.id));
        event.preventDefault();
      }
      return;
    }
    const index = rows.findIndex((row) => row.node.id === lead);
    const row = index >= 0 ? rows[index] : undefined;
    const page = Math.max(1, Math.floor((scroller.clientHeight || FALLBACK_HEIGHT) / ROW_HEIGHT) - 1);
    switch (event.key) {
      case "ArrowDown":
        moveTo(index + 1, event.shiftKey);
        break;
      case "ArrowUp":
        moveTo(index < 0 ? 0 : index - 1, event.shiftKey);
        break;
      case "Home":
        moveTo(0, event.shiftKey);
        break;
      case "End":
        moveTo(rows.length - 1, event.shiftKey);
        break;
      case "PageDown":
        moveTo(index + page, event.shiftKey);
        break;
      case "PageUp":
        moveTo(index - page, event.shiftKey);
        break;
      case "ArrowRight":
        if (row && isOpen(row.node)) moveTo(index + 1, false);
        else if (row && isExpandable(row.node)) setOpen(row.node, true);
        break;
      case "ArrowLeft":
        if (row && isOpen(row.node)) setOpen(row.node, false);
        else if (row) moveTo(parentIndex(index), false);
        break;
      case " ":
        if (selected.size > 0) ontoggle(selectedNodes());
        break;
      case "Enter":
        if (row && isExpandable(row.node)) setOpen(row.node, !isOpen(row.node));
        else if (row) onopen(row.node);
        break;
      default:
        return;
    }
    event.preventDefault();
  }

  function parentIndex(index: number): number {
    const { depth } = rows[index];
    for (let current = index - 1; current >= 0; current--) {
      if (rows[current].depth < depth) return current;
    }
    return index;
  }

  // `ChangesBrowserConflictsNode` does not count files; the staging `Unmerged` does.
  function label(node: TreeNode): string {
    if (node.kind === "file" || node.id === CONFLICTS_ID) return node.name;
    const count = countText(node);
    return count ? `${node.name} ${count}` : node.name;
  }
</script>

<div
  bind:this={scroller}
  bind:clientHeight={height}
  class="tree"
  role="tree"
  tabindex="0"
  aria-label="Changes"
  aria-multiselectable="true"
  aria-activedescendant={lead ? `row-${lead}` : undefined}
  onscroll={() => (scrollTop = scroller.scrollTop)}
  onmousedown={keepFocus}
  onkeydown={keydown}
  oncontextmenu={(event) => contextMenu(event, null)}
>
  <div data-content class="content" style:height="{rows.length * ROW_HEIGHT + PADDING_Y * 2}px">
    {#each windowRows as row, offset (row.node.id)}
      {@const node = row.node}
      {@const index = range.start + offset}
      {@const state = checkState(node, included, partial)}
      {@const hover = hoverIcon?.(node)}
      <!-- Focus and keyboard belong to the whole tree: a virtual row can leave the DOM, and the
           focus would go with it. The active row is announced with aria-activedescendant. -->
      <!-- svelte-ignore a11y_click_events_have_key_events, a11y_interactive_supports_focus -->
      <div
        id="row-{node.id}"
        class="row"
        class:selected={selected.has(node.id)}
        class:joined-up={selected.has(node.id) && selected.has(rows[index - 1]?.node.id)}
        class:joined-down={selected.has(node.id) && selected.has(rows[index + 1]?.node.id)}
        class:lead={lead === node.id}
        role="treeitem"
        aria-label={label(node)}
        aria-level={row.depth + 1}
        aria-selected={selected.has(node.id)}
        aria-expanded={isExpandable(node) ? isOpen(node) : undefined}
        style:top="{index * ROW_HEIGHT + PADDING_Y}px"
        style:padding-left="{PADDING_X + row.depth * INDENT}px"
        onclick={(event) => click(event, row)}
        ondblclick={(event) => doubleClick(event, node)}
        oncontextmenu={(event) => {
          event.stopPropagation();
          contextMenu(event, row);
        }}
      >
        <span
          class="chevron"
          data-testid="chevron"
          role="presentation"
          onclick={(event) => clickChevron(event, node)}
        >
          {#if isExpandable(node)}
            <Icon name={isOpen(node) ? "chevronDown" : "chevronRight"} />
          {/if}
        </span>
        {#if node.paths.length > 0 && node.checkable !== false}
          <!-- svelte-ignore a11y_interactive_supports_focus -->
          <span
            class="check"
            data-state={state}
            role="checkbox"
            aria-checked={CHECKED[state]}
            aria-label={node.name}
            onclick={(event) => clickCheck(event, node)}
          ></span>
        {/if}
        {#if node.kind === "directory"}
          <Icon name="folder" />
        {:else if node.kind === "file"}
          <Icon name="text" />
        {/if}
        {#if node.kind === "conflicts"}
          <span class="name bold">{node.name}</span>
          <button
            type="button"
            class="link"
            tabindex="-1"
            onclick={(event) => {
              event.stopPropagation();
              onresolve?.();
            }}
            ondblclick={(event) => event.stopPropagation()}
          >
            Resolve
          </button>
          {#if node.id !== CONFLICTS_ID}
            <span class="muted">{countText(node)}</span>
          {/if}
        {:else if node.kind === "changelist" || node.kind === "unversioned"}
          <span class="name" class:active={node.active}>{node.name}</span>
          <span class="muted">{countText(node)}</span>
        {:else if node.kind === "directory"}
          <span class="name">{node.name}</span>
          <span class="muted">{countText(node)}</span>
        {:else if node.change}
          <span class="name" style:color={statusColor(node.change.state)}>{node.name}</span>
          {#if originText(node.change)}
            <span class="origin">{originText(node.change)}</span>
          {/if}
          {#if node.location}
            <span class="muted">{node.location}</span>
          {/if}
        {/if}
        {#if hover}
          <button
            type="button"
            class="action hover-icon"
            tabindex="-1"
            title={hover.label}
            aria-label={hover.label}
            onclick={(event) => {
              event.stopPropagation();
              onhover?.(node);
            }}
            ondblclick={(event) => event.stopPropagation()}
          >
            <Icon name={hover.icon} />
          </button>
        {/if}
      </div>
    {/each}
  </div>
</div>

<style>
  .tree {
    flex: 1;
    min-height: 0;
    overflow: auto;
    outline: none;
    user-select: none;
    background: var(--panel);
  }

  .content {
    position: relative;
    min-width: 100%;
  }

  .row {
    position: absolute;
    right: 0;
    left: 0;
    display: flex;
    gap: 4px;
    align-items: center;
    height: 24px;
    white-space: nowrap;
  }

  .row::before {
    position: absolute;
    inset: 0 8px;
    z-index: 0;
    content: "";
    /* Tree.Selection.arc is 8, but that is the diameter of a RoundRectangle2D. */
    border-radius: 4px;
  }

  .row > :global(*) {
    position: relative;
    z-index: 1;
  }

  .row.selected::before {
    background: var(--selection-inactive);
  }

  .row.joined-up::before {
    border-top-left-radius: 0;
    border-top-right-radius: 0;
  }

  .row.joined-down::before {
    border-bottom-right-radius: 0;
    border-bottom-left-radius: 0;
  }

  .tree:focus .row.selected::before {
    background: var(--selection);
  }

  .tree:focus-visible .row.lead::before {
    outline: 1px solid var(--focus);
    outline-offset: -1px;
  }

  .hover-icon {
    flex: none;
    width: 20px;
    height: 20px;
    margin-right: 12px;
    margin-left: auto;
    opacity: 0;
  }

  .row:hover .hover-icon {
    opacity: 1;
  }

  .chevron {
    display: inline-flex;
    flex: none;
    width: 16px;
    height: 16px;
  }

  .name {
    flex: none;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .name.active,
  .name.bold {
    font-weight: 600;
  }

  .link {
    flex: none;
    padding: 0;
    font: inherit;
    font-weight: 600;
    color: var(--link);
    cursor: pointer;
    background: none;
    border: 0;
  }

  .link:hover {
    text-decoration: underline;
  }

  .origin {
    flex: none;
    color: var(--text);
  }

  .muted {
    min-width: 0;
    margin-left: 4px;
    overflow: hidden;
    color: var(--muted);
    text-overflow: ellipsis;
  }
</style>
