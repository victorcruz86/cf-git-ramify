<script lang="ts" module>
  import type { MenuAction, MenuHeader, MenuItem } from "./menu";
  import { speedMatches } from "./speedSearch";

  export type { MenuAction, MenuHeader, MenuItem };

  const isAction = (item: MenuItem): item is MenuAction => item !== null && "label" in item;

  // The rows that match, each one under its header if any is left.
  function searched(items: MenuItem[], query: string): MenuItem[] {
    const found: MenuItem[] = [];
    let header: MenuHeader | null = null;
    for (const item of items) {
      if (item === null) {
        header = null;
      } else if (!isAction(item)) {
        header = item;
      } else if (!item.disabled && speedMatches(item.label, query)) {
        if (header) found.push(header);
        header = null;
        found.push(item);
      }
    }
    return found;
  }
</script>

<script lang="ts">
  import { onMount, tick } from "svelte";
  import Icon from "./Icon.svelte";
  import Menu from "./Menu.svelte";

  interface Props {
    x: number;
    y: number;
    // Opens upwards from `y`.
    above?: boolean;
    items: MenuItem[];
    label: string;
    onclose: () => void;
    // Speed search: typing shows these rows instead of `items`, like the ones that
    // `FlatSpeedSearchPopup` only lets you see while there is text.
    typing?: MenuItem[];
    // It is a submenu. `flip` is where to end up if it does not fit on the right.
    nested?: boolean;
    flip?: number;
    focused?: boolean;
    onback?: () => void;
    // `ActionSelectionAid.NUMBERING`: the rows carry a number, and that key chooses them.
    numbered?: boolean;
    // The title of a titled `ListPopup`, shown above the rows.
    title?: string;
  }

  let {
    x,
    y,
    above = false,
    items,
    label,
    onclose,
    typing,
    nested = false,
    flip,
    focused = true,
    onback,
    numbered = false,
    title,
  }: Props = $props();

  const MARGIN = 8;
  const SUBMENU_MARK = "›";
  const NUMBERS = 10;
  // The menu padding: the first row of the submenu lines up with its own.
  const PADDING = 7;

  let box: HTMLDivElement;
  const rows: HTMLElement[] = [];
  let current = $state(-1);
  let left = $state(0);
  let top = $state(0);
  let query = $state("");
  let open = $state.raw<{
    index: number;
    x: number;
    y: number;
    flip: number;
    focused: boolean;
  } | null>(null);
  let shown = $derived(query ? searched(typing ?? [], query) : items);
  // The starred rows touched: the menu stays open and its rows are not recomputed.
  let flipped = $state.raw<Set<MenuAction>>(new Set());

  function enabled(): number[] {
    return shown.flatMap((item, index) => (isAction(item) && !item.disabled ? [index] : []));
  }

  const starred = (item: MenuAction) => item.star?.on !== flipped.has(item);

  function toggleStar(event: MouseEvent, item: MenuAction) {
    event.stopPropagation();
    item.star?.toggle();
    const next = new Set(flipped);
    if (!next.delete(item)) next.add(item);
    flipped = next;
  }

  // 0 is the tenth.
  const numberOf = (index: number) => {
    const position = enabled().indexOf(index);
    return position < NUMBERS ? (position + 1) % NUMBERS : null;
  };

  function move(step: number) {
    const order = enabled();
    if (order.length === 0) return;
    const position = order.indexOf(current);
    const next = position < 0 && step < 0 ? order.length - 1 : position + step;
    current = order[(next + order.length) % order.length];
  }

  function search(text: string) {
    query = text;
    open = null;
    current = text ? (enabled()[0] ?? -1) : -1;
  }

  // It runs once the menu is closed: closing it gives focus back, and the action may move it.
  function choose(item: MenuAction) {
    if (item.disabled) return;
    onclose();
    if (item.run) tick().then(item.run);
  }

  function openSubmenu(index: number, focus: boolean) {
    const bounds = rows[index].getBoundingClientRect();
    open = { index, x: bounds.right, y: bounds.top - PADDING, flip: bounds.left, focused: focus };
  }

  function closeSubmenu() {
    open = null;
    box.focus();
  }

  function activate(index: number) {
    // -1 if no row is highlighted.
    const item = shown[index];
    if (!item || !isAction(item)) return;
    if (item.submenu) openSubmenu(index, true);
    else choose(item);
  }

  function hover(index: number, item: MenuAction) {
    current = item.disabled ? -1 : index;
    if (!item.submenu) open = null;
    else if (open?.index !== index) openSubmenu(index, false);
  }

  function keydown(event: KeyboardEvent) {
    event.stopPropagation();
    const item = shown[current];
    const isSubmenu = item && isAction(item) && item.submenu;
    if (event.key === "ArrowDown") move(1);
    else if (event.key === "ArrowUp") move(-1);
    else if (event.key === "ArrowRight" && isSubmenu) openSubmenu(current, true);
    else if (event.key === "ArrowLeft" && nested) onback?.();
    else if (event.key === "Escape" && query) search("");
    else if (event.key === "Escape" && nested) onback?.();
    else if (event.key === "Escape" || event.key === "Tab") onclose();
    else if (event.key === "Enter" || (event.key === " " && !query)) activate(current);
    else if (event.key === "Backspace" && query) search(query.slice(0, -1));
    else if (numbered && /^\d$/.test(event.key)) {
      activate(enabled()[(Number(event.key) + NUMBERS - 1) % NUMBERS] ?? -1);
    }
    else if (typing && event.key.length === 1 && !event.metaKey && !event.ctrlKey) {
      search(query + event.key);
    } else return;
    event.preventDefault();
  }

  // A submenu lives inside its menu: the topmost one already counts its clicks as inside.
  function pointerdown(event: PointerEvent) {
    if (!nested && !box.contains(event.target as Node)) onclose();
  }

  onMount(() => {
    const bounds = box.getBoundingClientRect();
    const fits = x + bounds.width <= window.innerWidth - MARGIN;
    const wanted = !fits && flip !== undefined ? flip - bounds.width : x;
    left = Math.max(MARGIN, Math.min(wanted, window.innerWidth - bounds.width - MARGIN));
    const start = above ? y - bounds.height : y;
    top = Math.max(MARGIN, Math.min(start, window.innerHeight - bounds.height - MARGIN));
    if (!focused) return;
    const previous = document.activeElement;
    box.focus();
    if (nested) move(1);
    return () => {
      if (previous instanceof HTMLElement && previous.isConnected) previous.focus();
    };
  });
</script>

<svelte:window onpointerdown={pointerdown} />

<div
  bind:this={box}
  class="menu"
  role="menu"
  aria-label={label}
  tabindex="-1"
  style:left="{left}px"
  style:top="{top}px"
  onkeydown={keydown}
>
  {#if title}
    <div class="title">{title}</div>
  {/if}
  {#if query}
    <div class="query">{query}</div>
  {/if}
  {#each shown as item, index}
    {#if item === null}
      <div class="separator" role="separator"></div>
    {:else if isAction(item)}
      <button
        bind:this={rows[index]}
        type="button"
        role="menuitem"
        tabindex="-1"
        aria-label={item.label}
        class:current={index === current}
        disabled={item.disabled}
        title={item.title}
        onpointerenter={() => hover(index, item)}
        onclick={() => activate(index)}
      >
        <span class="label">
          {#if item.star}
            <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
            <span
              class="star"
              data-star={starred(item) ? "on" : "off"}
              onclick={(event) => toggleStar(event, item)}
            >
              {#if starred(item)}
                <Icon name="favorite" />
              {:else if index === current}
                <Icon name="notFavorite" />
              {/if}
            </span>
          {/if}
          {#if item.checked !== undefined}
            <span class="tick">{item.checked ? "✓" : ""}</span>
          {/if}
          {#if numbered && numberOf(index) !== null}
            {numberOf(index)}.{" "}
          {/if}
          {item.label}
        </span>
        {#if item.shortcut || item.submenu}
          <span class="shortcut">{item.submenu ? SUBMENU_MARK : item.shortcut}</span>
        {/if}
      </button>
    {:else}
      <div class="header" role="separator" aria-label={item.header}>{item.header}</div>
    {/if}
  {/each}
  {#if open}
    {#key open}
      {@const parent = shown[open.index] as MenuAction}
      <Menu
        nested
        x={open.x}
        y={open.y}
        flip={open.flip}
        focused={open.focused}
        items={parent.submenu ?? []}
        label={parent.label}
        {onclose}
        onback={closeSubmenu}
      />
    {/key}
  {/if}
</div>

<style>
  .menu {
    position: fixed;
    z-index: 20;
    min-width: 220px;
    max-width: 560px;
    max-height: calc(100vh - 16px);
    padding: 6px 0;
    overflow: auto;
    background: var(--popup-background);
    border: 1px solid var(--popup-border);
    border-radius: 8px;
    outline: none;
    box-shadow: 0 6px 24px rgb(0 0 0 / 40%);
  }

  .title {
    padding: 2px 14px 6px;
    margin-bottom: 4px;
    font-weight: 600;
    color: var(--text);
    white-space: nowrap;
    border-bottom: 1px solid var(--menu-separator);
  }

  .query {
    height: 24px;
    margin: 0 14px 4px;
    overflow: hidden;
    line-height: 24px;
    color: var(--text);
    text-overflow: ellipsis;
    white-space: nowrap;
    border-bottom: 1px solid var(--menu-separator);
  }

  button {
    display: flex;
    gap: 24px;
    align-items: center;
    justify-content: space-between;
    width: calc(100% - 12px);
    height: 28px;
    margin: 0 6px;
    padding: 0 8px;
    font: inherit;
    color: var(--text);
    text-align: left;
    white-space: nowrap;
    background: none;
    border: 0;
    /* Popup.Selection.arc is 8, but fillRoundRect takes it as a diameter. */
    border-radius: 4px;
  }

  button.current {
    background: var(--selection);
  }

  button:disabled {
    color: var(--disabled);
  }

  .label {
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .tick {
    display: inline-block;
    width: 18px;
  }

  .star {
    display: inline-flex;
    width: 22px;
    vertical-align: text-bottom;
  }

  .shortcut {
    color: var(--shortcut);
  }

  .separator {
    height: 1px;
    margin: 4px 14px;
    background: var(--menu-separator);
  }

  .header {
    margin: 4px 14px;
    color: var(--muted);
  }
</style>
