<script lang="ts">
  import type { Snippet } from "svelte";
  import type { TreeEntry } from "../lib/api";
  import Icon from "../lib/Icon.svelte";
  import { visibleRange } from "../log/rowCache";
  import { namePieces, type TreeRow } from "./model";
  import type { TreeNavigation } from "./navigation.svelte";

  // Tree.rowHeight and Tree.border of the New UI, as the Changes tree.
  const ROW_HEIGHT = 24;
  const PADDING_Y = 4;
  const PADDING_X = 12;
  const INDENT = 20;
  const OVERSCAN = 8;
  // With no measurements yet (first paint) a tall window is assumed.
  const FALLBACK_HEIGHT = 600;

  interface Props {
    rows: TreeRow[];
    nav: TreeNavigation;
    // The tree's accessible name, and the prefix of its row ids.
    label: string;
    idPrefix: string;
    isOpen: (path: string) => boolean;
    // An open folder with nothing in it shows no chevron.
    isEmpty: (path: string) => boolean;
    // The colour of a name, or null for the default text.
    color?: (entry: TreeEntry) => string | null;
    // A name in bold (the Project tree's root).
    strong?: (entry: TreeEntry) => boolean;
    // What goes between the chevron and the icon (a checkbox), and what follows the name.
    before?: Snippet<[TreeEntry]>;
    after?: Snippet<[TreeEntry]>;
    onkeydown: (event: KeyboardEvent) => void;
    ondblclick: (row: TreeRow) => void;
    oncontextmenu: (event: MouseEvent, row: TreeRow | null) => void;
    ontoggle: (entry: TreeEntry) => void;
  }

  let {
    rows,
    nav,
    label,
    idPrefix,
    isOpen,
    isEmpty,
    color = () => null,
    strong = () => false,
    before,
    after,
    onkeydown,
    ondblclick,
    oncontextmenu,
    ontoggle,
  }: Props = $props();

  let scroller = $state<HTMLDivElement>();
  let scrollTop = $state(0);
  let height = $state(0);

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
  const leadIndex = $derived(nav.leadIndex);

  export function focus() {
    scroller?.focus();
  }

  // The rows a page moves by.
  export function pageSize(): number {
    return Math.max(1, Math.floor((scroller?.clientHeight || FALLBACK_HEIGHT) / ROW_HEIGHT) - 1);
  }

  export function reveal(index: number) {
    if (!scroller || index < 0) return;
    const top = index * ROW_HEIGHT + PADDING_Y;
    const view = scroller.clientHeight;
    if (top < scroller.scrollTop) scroller.scrollTop = top - PADDING_Y;
    else if (view > 0 && top + ROW_HEIGHT > scroller.scrollTop + view) {
      scroller.scrollTop = top + ROW_HEIGHT - view + PADDING_Y;
    }
    // `scroll` arrives in the next frame: the new row must already be painted.
    scrollTop = scroller.scrollTop;
  }

  // Back to the top, for a tree that shows something else now.
  export function scrollToTop() {
    if (scroller) scroller.scrollTop = 0;
    scrollTop = 0;
  }

  function clickChevron(event: MouseEvent, entry: TreeEntry) {
    event.stopPropagation();
    ontoggle(entry);
  }
</script>

<div class="view">
  <div
    bind:this={scroller}
    bind:clientHeight={height}
    class="tree"
    role="tree"
    tabindex="0"
    aria-label={label}
    aria-multiselectable="true"
    aria-activedescendant={leadIndex >= 0 ? `${idPrefix}-row-${leadIndex}` : undefined}
    onscroll={() => scroller && (scrollTop = scroller.scrollTop)}
    {onkeydown}
    onblur={() => (nav.query = "")}
    oncontextmenu={(event) => oncontextmenu(event, null)}
  >
    <div class="content" style:height="{rows.length * ROW_HEIGHT + PADDING_Y * 2}px">
      {#each windowRows as row, offset (row.entry.path)}
        {@const entry = row.entry}
        {@const index = range.start + offset}
        {@const isSelected = nav.selected.has(entry.path)}
        {@const open = entry.dir && isOpen(entry.path)}
        {@const empty = open && isEmpty(entry.path)}
        <!-- Focus and keyboard belong to the whole tree: a virtual row can leave the DOM, and the
             focus would go with it. The active row is announced with aria-activedescendant. -->
        <!-- svelte-ignore a11y_click_events_have_key_events, a11y_interactive_supports_focus -->
        <div
          id="{idPrefix}-row-{index}"
          class="row"
          class:selected={isSelected}
          class:joined-up={isSelected && nav.selected.has(rows[index - 1]?.entry.path)}
          class:joined-down={isSelected && nav.selected.has(rows[index + 1]?.entry.path)}
          class:lead={nav.lead === entry.path}
          role="treeitem"
          aria-label={entry.name}
          aria-level={row.depth + 1}
          aria-selected={isSelected}
          aria-expanded={entry.dir ? open : undefined}
          style:top="{index * ROW_HEIGHT + PADDING_Y}px"
          style:padding-left="{PADDING_X + row.depth * INDENT}px"
          onmousedown={(event) => {
            event.preventDefault();
            scroller?.focus();
          }}
          onclick={(event) => nav.click(event, entry.path)}
          ondblclick={() => ondblclick(row)}
          oncontextmenu={(event) => {
            event.stopPropagation();
            oncontextmenu(event, row);
          }}
        >
          <span class="chevron" role="presentation" onclick={(event) => clickChevron(event, entry)}>
            {#if entry.dir && !empty}
              <Icon name={open ? "chevronDown" : "chevronRight"} />
            {/if}
          </span>
          {@render before?.(entry)}
          <Icon name={entry.dir ? "folder" : "text"} />
          <span class="name" class:strong={strong(entry)} style:color={color(entry)}>
            {#each namePieces(entry.name, nav.query.trim()) as piece, pieceIndex (pieceIndex)}
              {#if piece.hit}<span class="hit">{piece.text}</span>{:else}{piece.text}{/if}
            {/each}
          </span>
          {@render after?.(entry)}
        </div>
      {/each}
    </div>
  </div>
  {#if nav.query}
    <div class="query">{nav.query}</div>
  {/if}
</div>

<style>
  .view {
    position: relative;
    display: flex;
    flex: 1;
    min-height: 0;
  }

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

  .chevron {
    display: inline-flex;
    flex: none;
    width: 16px;
    height: 16px;
  }

  .name {
    flex: none;
  }

  .name.strong {
    font-weight: 600;
  }

  .hit {
    color: var(--accent-text);
    background: var(--accent);
    border-radius: 2px;
  }

  .query {
    position: absolute;
    top: 2px;
    left: 8px;
    z-index: 2;
    padding: 1px 6px;
    color: var(--text);
    background: var(--panel);
    border: 1px solid var(--focus);
    border-radius: 4px;
  }
</style>
