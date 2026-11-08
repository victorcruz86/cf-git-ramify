<script lang="ts" generics="T">
  import { untrack, type Snippet } from "svelte";
  import { errorText } from "../lib/errors";
  import Icon from "../lib/Icon.svelte";
  import { matchShortcut } from "../lib/keymap";
  import Menu from "../lib/Menu.svelte";
  import type { MenuItem } from "../lib/menu";
  import { store, stored } from "../lib/storage";
  import SavedChangesList from "./SavedChangesList.svelte";
  import SavedDiffDialog from "./SavedDiffDialog.svelte";
  import SavedDiffPane from "./SavedDiffPane.svelte";
  import {
    SAVED_TEXTS,
    changeKey,
    patchMatches,
    shownChanges,
    type DiffMode,
    type SavedChange,
    type SavedDiff,
  } from "./savedPatches";

  // `TwoKeySplitter` (vertical orientation) and `OnePixelSplitter` of `SavedPatchesUi`, under IntelliJ's
  // keys, both 0.5 by default.
  const CHANGES_SPLITTER = "vcs.saved.patches.changes.splitter.vertical";
  const DIFF_SPLITTER = "vcs.saved.patches.diff.splitter";
  const PROPORTION = 0.5;
  const MIN_PROPORTION = 0.1;
  const MAX_PROPORTION = 0.9;

  // `SavedPatchesUi` with the preview inside the tab, as in a tool window at the bottom: the list over
  // the changes on the left, with the `Apply` and `Pop` bar, and the diff on the right. The provider
  // (the Stash tab, the Shelf tab) gives the rows, their changes, the texts and its own operations.
  // Everything shown comes from the repository and is text.
  interface Props {
    items: T[];
    // The list could not be read: the error is its only node.
    error?: string | null;
    // The list's name and that of its menu.
    label: string;
    menuLabel: string;
    keyOf: (item: T) => string;
    textOf: (item: T) => string;
    tooltipOf: (item: T) => string;
    // What goes to the right of the row (the stash's branch).
    badge?: Snippet<[T]>;
    loadChanges: (item: T) => Promise<SavedChange[]>;
    loadDiff: (item: T, change: SavedChange, mode: DiffMode) => Promise<SavedDiff>;
    diffTitle: (item: T, change: SavedChange | null) => string;
    // The provider's group of the list menu (`SavedPatchesOperationsGroup`), which goes first.
    operations: (item: T) => MenuItem[];
    // `Compare Before with Local` in the menu of the changes (the stash has it).
    compareBefore?: boolean;
    // What the provider adds at the end of the menu of the changes.
    changeOperations?: (item: T, change: SavedChange) => MenuItem[];
    // Whether a change takes part in that comparison; those that do not are left out of the chain.
    comparable?: (item: T, change: SavedChange, mode: DiffMode) => Promise<boolean>;
    onapply: (item: T) => void;
    onpop: (item: T) => void;
    // `$Delete` on the list.
    ondelete: (item: T) => void;
  }

  let {
    items,
    error = null,
    label,
    menuLabel,
    keyOf,
    textOf,
    tooltipOf,
    badge,
    loadChanges,
    loadDiff,
    diffTitle,
    operations,
    compareBefore = false,
    changeOperations,
    comparable,
    onapply,
    onpop,
    ondelete,
  }: Props = $props();

  type Loaded = { kind: "loading" } | { kind: "error"; text: string } | { kind: "changes" };

  let list = $state<HTMLDivElement>();
  let selectedKey = $state<string | null>(null);
  // What each patch's changes were when read: the speed search looks at the names of the ones known.
  let known = $state.raw(new Map<string, SavedChange[]>());
  let loaded = $state.raw<Loaded>({ kind: "loading" });
  let currentChange = $state.raw<SavedChange | null>(null);
  let query = $state("");
  let menu = $state.raw<{ x: number; y: number; items: MenuItem[]; label: string } | null>(null);
  let dialog = $state.raw<{
    item: T;
    changes: SavedChange[];
    start: number;
    mode: DiffMode;
  } | null>(null);
  let request = 0;
  let split = $state<HTMLDivElement>();
  let side = $state<HTMLDivElement>();
  const proportion = (key: string) => {
    const value = Number(stored(key));
    return value >= MIN_PROPORTION && value <= MAX_PROPORTION ? value : PROPORTION;
  };
  let changesProportion = $state(proportion(CHANGES_SPLITTER));
  let diffProportion = $state(proportion(DIFF_SPLITTER));

  const selected = $derived(items.find((item) => keyOf(item) === selectedKey) ?? null);
  // `treeChangesSplitter.secondComponent` is shown while a provider is not empty; an error counts.
  const hasDetails = $derived(items.length > 0 || error !== null);
  const changes = $derived(selected ? (known.get(keyOf(selected)) ?? []) : []);
  const changesEmpty = $derived(
    !selected
      ? SAVED_TEXTS.changesEmpty
      : loaded.kind === "loading"
        ? SAVED_TEXTS.loading
        : loaded.kind === "error"
          ? loaded.text
          : "",
  );
  // A list read again gives new rows for the same patches: the preview follows the change only.
  const preview = $derived.by(() => {
    const change = currentChange;
    const item = untrack(() => selected);
    return item && change ? () => loadDiff(item, change, "change") : null;
  });

  export function focus() {
    list?.focus();
  }

  // `SavedPatchesTreeStateStrategy.restoreState`: a selection that is gone goes to the first row.
  $effect(() => {
    if (!items.some((item) => keyOf(item) === selectedKey)) {
      selectedKey = items.length > 0 ? keyOf(items[0]) : null;
    }
  });

  // What was read for a patch that is gone is not kept.
  $effect(() => {
    const keys = new Set(items.map(keyOf));
    const stale = [...untrack(() => known).keys()].filter((key) => !keys.has(key));
    if (stale.length === 0) return;
    const kept = new Map(untrack(() => known));
    stale.forEach((key) => kept.delete(key));
    known = kept;
  });

  async function changesOf(item: T): Promise<SavedChange[]> {
    const cached = known.get(keyOf(item));
    if (cached) return cached;
    const read = await loadChanges(item);
    known = new Map(known).set(keyOf(item), read);
    return read;
  }

  // `SavedPatchesChangesBrowser.selectPatchObject`, and the preview on the first change.
  async function choose(item: T | null) {
    const token = ++request;
    currentChange = null;
    if (!item) return;
    loaded = known.has(keyOf(item)) ? { kind: "changes" } : { kind: "loading" };
    try {
      const read = await changesOf(item);
      if (token !== request) return;
      loaded = { kind: "changes" };
      currentChange = shownChanges(read)[0] ?? null;
    } catch (failure) {
      if (token === request) loaded = { kind: "error", text: errorText(failure) };
    }
  }

  // By key: the list is read again on every change of the repository, and the same stash keeps its
  // changes and the one chosen among them.
  $effect(() => {
    const key = selectedKey;
    untrack(() => void choose(items.find((item) => keyOf(item) === key) ?? null));
  });

  function select(item: T) {
    selectedKey = keyOf(item);
  }

  // `Show Diff` and `Compare with Local` of a patch: all its changes, from the first.
  async function openDiff(item: T, mode: DiffMode, from: SavedChange | null = null) {
    let all: SavedChange[];
    try {
      all = shownChanges(await changesOf(item));
    } catch (failure) {
      loaded = { kind: "error", text: errorText(failure) };
      return;
    }
    if (comparable && mode !== "change") {
      const keep = await Promise.all(all.map((change) => comparable(item, change, mode)));
      all = all.filter((_, index) => keep[index]);
    }
    if (all.length === 0) return;
    const start = from ? Math.max(0, all.findIndex((change) => changeKey(change) === changeKey(from))) : 0;
    dialog = { item, changes: all, start, mode };
  }

  const listMenu = (item: T): MenuItem[] => [
    ...operations(item),
    null,
    { label: SAVED_TEXTS.showDiff, shortcut: "⌘D", run: () => void openDiff(item, "change") },
    { label: SAVED_TEXTS.showStandaloneDiff, run: () => void openDiff(item, "change") },
    { label: SAVED_TEXTS.compareWithLocal, run: () => void openDiff(item, "local") },
  ];

  const changeMenu = (item: T, change: SavedChange): MenuItem[] => [
    { label: SAVED_TEXTS.showDiff, shortcut: "⌘D", run: () => void openDiff(item, "change", change) },
    { label: SAVED_TEXTS.showStandaloneDiff, run: () => void openDiff(item, "change", change) },
    { label: SAVED_TEXTS.compareWithLocal, run: () => void openDiff(item, "local", change) },
    ...(compareBefore
      ? [
          {
            label: SAVED_TEXTS.compareBeforeWithLocal,
            run: () => void openDiff(item, "beforeLocal", change),
          },
        ]
      : []),
    ...(changeOperations?.(item, change) ?? []),
  ];

  function rowMenu(event: MouseEvent, item: T) {
    event.preventDefault();
    select(item);
    menu = { x: event.clientX, y: event.clientY, items: listMenu(item), label: menuLabel };
  }

  function changesMenu(event: MouseEvent, change: SavedChange) {
    if (!selected) return;
    menu = { x: event.clientX, y: event.clientY, items: changeMenu(selected, change), label: "Changes" };
  }

  const matching = () => items.filter((item) => patchMatches(textOf(item), known.get(keyOf(item)), query));

  // `TreeSpeedSearch`: typing goes to the first row that matches, and the arrows move between those.
  function search(text: string) {
    query = text;
    if (!text) return;
    const first = matching()[0];
    if (first) select(first);
  }

  function move(step: number) {
    const rows = query ? matching() : items;
    if (rows.length === 0) return;
    const index = selected ? rows.indexOf(selected) : -1;
    const next = rows[Math.min(Math.max(index + step, 0), rows.length - 1)];
    select(next);
  }

  function listKeydown(event: KeyboardEvent) {
    const plain = !event.metaKey && !event.ctrlKey && !event.altKey;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      move(event.key === "ArrowDown" ? 1 : -1);
    } else if (event.key === "Escape" && query) {
      query = "";
    } else if (event.key === "Backspace" && query && plain) {
      search(query.slice(0, -1));
    } else if ((event.key === "Delete" || event.key === "Backspace") && !event.ctrlKey && !event.altKey) {
      if (selected) ondelete(selected);
    } else if (matchShortcut(event) === "Diff.ShowDiff") {
      if (selected) void openDiff(selected, "change");
    } else if (plain && event.key.length === 1 && event.key !== " ") {
      search(query + event.key);
    } else {
      return;
    }
    event.preventDefault();
  }

  // `OnePixelSplitter`: the proportion goes with the pointer and is remembered when it is let go.
  function resize(event: PointerEvent, area: HTMLElement | undefined, vertical: boolean) {
    const divider = event.currentTarget as HTMLElement;
    if (!area) return;
    divider.setPointerCapture?.(event.pointerId);
    const bounds = area.getBoundingClientRect();
    const move = (moved: PointerEvent) => {
      const part = vertical
        ? (moved.clientY - bounds.top) / bounds.height
        : (moved.clientX - bounds.left) / bounds.width;
      if (!Number.isFinite(part)) return;
      const value = Math.round(Math.min(MAX_PROPORTION, Math.max(MIN_PROPORTION, part)) * 1000) / 1000;
      if (vertical) changesProportion = value;
      else diffProportion = value;
    };
    // A drag the system cancels ends as a release does.
    const finish = () => {
      divider.removeEventListener("pointermove", move);
      divider.removeEventListener("pointerup", finish);
      divider.removeEventListener("pointercancel", finish);
      store(vertical ? CHANGES_SPLITTER : DIFF_SPLITTER, String(vertical ? changesProportion : diffProportion));
    };
    divider.addEventListener("pointermove", move);
    divider.addEventListener("pointerup", finish);
    divider.addEventListener("pointercancel", finish);
  }

  // `DEFAULT_COMMIT_ACTION_SHORTCUT` is registered on the whole panel.
  function panelKeydown(event: KeyboardEvent) {
    if (matchShortcut(event) !== "CommitView.Commit" || !selected) return;
    event.preventDefault();
    event.stopPropagation();
    onapply(selected);
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="saved" bind:this={split} onkeydown={panelKeydown}>
  <div class="side" bind:this={side} style:flex="{diffProportion} 1 0">
    <div class="list-pane" style:flex={hasDetails ? `${changesProportion} 1 0` : "1 1 0"}>
      <div
        bind:this={list}
        class="list"
        role="tree"
        aria-label={label}
        tabindex="0"
        onkeydown={listKeydown}
        onblur={() => (query = "")}
      >
        {#if error !== null}
          <p class="status error">{error}</p>
        {:else if items.length === 0}
          <p class="status">{SAVED_TEXTS.empty}</p>
        {/if}
        {#each items as item (keyOf(item))}
          <!-- The keys go to the list, which holds the selection. -->
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <div
            role="treeitem"
            aria-label={textOf(item)}
            aria-selected={keyOf(item) === selectedKey}
            tabindex="-1"
            class="row"
            class:current={keyOf(item) === selectedKey}
            class:matched={query !== "" && patchMatches(textOf(item), known.get(keyOf(item)), query)}
            title={tooltipOf(item)}
            onclick={() => select(item)}
            ondblclick={() => {
              select(item);
              void openDiff(item, "change");
            }}
            oncontextmenu={(event) => rowMenu(event, item)}
          >
            <span class="text">{textOf(item)}</span>
            {@render badge?.(item)}
          </div>
        {/each}
      </div>
      {#if query}
        <div class="query">{query}</div>
      {/if}
    </div>
    {#if hasDetails}
      <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
      <div
        class="splitter rows"
        role="separator"
        aria-orientation="horizontal"
        aria-valuenow={Math.round(changesProportion * 100)}
        onpointerdown={(event) => resize(event, side, true)}
      ></div>
      <div class="changes" style:flex="{1 - changesProportion} 1 0">
        <SavedChangesList
          {changes}
          empty={changesEmpty}
          current={currentChange}
          {query}
          onselect={(change) => (currentChange = change)}
          onopen={(change) => selected && void openDiff(selected, "change", change)}
          oncontext={changesMenu}
        />
      </div>
      <div class="bar">
        <button
          type="button"
          class="button default"
          disabled={!selected}
          onclick={() => selected && onapply(selected)}>{SAVED_TEXTS.apply}</button
        >
        <button
          type="button"
          class="button"
          disabled={!selected}
          onclick={() => selected && onpop(selected)}>{SAVED_TEXTS.pop}</button
        >
        <span class="help" role="img" aria-label="Help" title={SAVED_TEXTS.help}>
          <Icon name="info" size={14} />
        </span>
      </div>
    {/if}
  </div>
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div
    class="splitter columns"
    role="separator"
    aria-orientation="vertical"
    aria-valuenow={Math.round(diffProportion * 100)}
    onpointerdown={(event) => resize(event, split, false)}
  ></div>
  <section class="preview" aria-label="Diff Preview" style:flex="{1 - diffProportion} 1 0">
    {#if selected && currentChange}
      <SavedDiffPane load={preview} path={currentChange.file.path} />
    {/if}
  </section>
</div>

{#if menu}
  <Menu x={menu.x} y={menu.y} label={menu.label} items={menu.items} onclose={() => (menu = null)} />
{/if}

{#if dialog}
  {@const shown = dialog}
  <SavedDiffDialog
    changes={shown.changes}
    start={shown.start}
    titleOf={(change) => diffTitle(shown.item, change)}
    load={(change) => loadDiff(shown.item, change, shown.mode)}
    onclose={() => (dialog = null)}
  />
{/if}

<style>
  .saved {
    display: flex;
    flex: 1;
    min-width: 0;
    min-height: 0;
    background: var(--panel);
  }

  .side {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .list-pane {
    position: relative;
    display: flex;
    min-height: 0;
  }

  .splitter {
    position: relative;
    z-index: 2;
    flex: none;
  }

  .splitter.columns {
    width: 5px;
    margin: 0 -2px;
    cursor: col-resize;
    background: linear-gradient(
      90deg,
      transparent 2px,
      var(--border) 2px,
      var(--border) 3px,
      transparent 3px
    );
  }

  .splitter.rows {
    height: 5px;
    margin: -2px 0;
    cursor: row-resize;
    background: linear-gradient(
      180deg,
      transparent 2px,
      var(--border) 2px,
      var(--border) 3px,
      transparent 3px
    );
  }

  .list {
    flex: 1;
    padding: 2px 0;
    overflow: auto;
    outline: none;
  }

  .row {
    display: flex;
    gap: 8px;
    align-items: center;
    padding: 2px 8px;
    white-space: nowrap;
  }

  .row.current {
    background: var(--selection);
  }

  .row.matched .text {
    text-decoration: underline;
  }

  .text {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .query {
    position: absolute;
    top: 2px;
    left: 8px;
    padding: 1px 6px;
    color: var(--text);
    background: var(--panel);
    border: 1px solid var(--focus);
    border-radius: 4px;
  }

  .status {
    padding: 16px;
    margin: 0;
    color: var(--text-secondary);
    text-align: center;
  }

  .status.error {
    color: var(--error);
    text-align: left;
  }

  .changes {
    display: flex;
    min-height: 0;
  }

  .bar {
    display: flex;
    flex: none;
    gap: 6px;
    align-items: center;
    padding: 7px 5px;
    border-top: 1px solid var(--divider);
  }

  .help {
    display: inline-flex;
    color: var(--text-secondary);
  }

  .preview {
    display: flex;
    min-width: 0;
    min-height: 0;
  }
</style>
