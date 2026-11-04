<script lang="ts">
  import { onMount, tick, untrack } from "svelte";
  import {
    branchIncomingOutgoing,
    branchList,
    onBranchSyncChanged,
    onRepoChanged,
    type BranchList,
    type BranchSync,
  } from "../lib/api";
  import { errorText } from "../lib/errors";
  import Icon from "../lib/Icon.svelte";
  import { matchShortcut } from "../lib/keymap";
  import Menu, { type MenuItem } from "../lib/Menu.svelte";
  import { notify } from "../lib/notifications";
  import type { VcsAction } from "../log/contextMenu";
  import {
    autoExpanded,
    branchActions,
    buildTree,
    findBranch,
    highlighted,
    nextLeaf,
    pathTo,
    preferredId,
    refOf,
    syncTooltip,
    visibleRows,
    type BranchNode,
    type GroupNode,
    type TreeNode,
  } from "./model";
  import {
    branchOptions,
    favorites,
    protectedBranches,
    refsTracker,
    setBranchOptions,
    toggleFavorite,
  } from "./state";

  // `GitBranchesTreePopup` of the New UI. What cannot be done yet arrives without a handler and its row
  // comes out disabled.
  interface Props {
    // Below the toolbar widget; without it, in the center of the window.
    anchor?: { x: number; y: number } | null;
    onclose: () => void;
    onvcs?: (action: VcsAction) => void;
    onnewbranch?: () => void;
    oncheckoutref?: () => void;
    // With ⇧ the dialog appears even if it is hidden.
    onupdateproject?: (options: { shift: boolean }) => void;
    updatedialog?: boolean;
    oncommit?: () => void;
    onpush?: () => void;
    onfetch?: () => Promise<unknown>;
    // `Git.Ongoing.Rebase.Actions`: those of the ongoing operation, if any, at the very top.
    ongoing?: MenuItem[];
    // With a detached HEAD, the hash that names the current one in the branch entries.
    head?: string | null;
  }

  let {
    anchor = null,
    onclose,
    onvcs,
    onnewbranch,
    oncheckoutref,
    onupdateproject,
    updatedialog = false,
    oncommit,
    onpush,
    onfetch,
    ongoing = [],
    head = null,
  }: Props = $props();

  const ROW_HEIGHT = 24;
  const MAX_VISIBLE_ROWS = 20;
  const MIN_WIDTH = 350;
  const MIN_HEIGHT = 300;
  const MARGIN = 8;
  // The menu padding: the first row of the submenu lines up with its own.
  const MENU_PADDING = 7;
  // `GitBranchesTreePopup.STEP_X_PADDING`.
  const STEP_X_PADDING = 2;
  // `applySearchPattern` and the `ide.popup.auto.delay` registry key.
  const SEARCH_DELAY = 100;
  const HOVER_DELAY = 500;

  interface ActionRow {
    id: string;
    label: string;
    shortcut?: string;
    run: (() => void) | undefined;
  }

  type Row = ActionRow | "separator" | { node: TreeNode; depth: number };

  let box: HTMLDivElement;
  let field: HTMLInputElement;
  let list = $state.raw<BranchList | null>(null);
  let sync = $state.raw<Record<string, BranchSync>>({});
  let text = $state("");
  let query = $state("");
  let selected = $state<string | null>(null);
  let expanded = $state.raw(new Set<string>());
  let open = $state.raw<{
    branch: BranchNode;
    x: number;
    y: number;
    flip: number;
    focused: boolean;
  } | null>(null);
  let settings = $state.raw<{ x: number; y: number } | null>(null);
  let fetching = $state(false);
  let shift = false;
  let placed = $state(false);
  let left = $state(0);
  let top = $state(0);
  let visibleCount = $state(MAX_VISIBLE_ROWS);
  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  let hoverTimer: ReturnType<typeof setTimeout> | undefined;
  let lastPointer = "";
  let readToken = 0;
  let syncToken = 0;
  const refs = refsTracker();

  const tree = $derived(
    list
      ? buildTree(list, query, {
          favorites: $favorites,
          grouping: $branchOptions.grouping,
          recent: $branchOptions.showRecent,
        })
      : [],
  );
  const actions = $derived<ActionRow[][]>([
    [
      {
        id: "action:update",
        // `AbstractCommonUpdateAction`: the dots say that it is going to ask.
        label: updatedialog ? "Update Project…" : "Update Project",
        shortcut: "⌘T",
        run: onupdateproject && (() => onupdateproject({ shift })),
      },
      { id: "action:commit", label: "Commit…", shortcut: "⌘K", run: oncommit },
      { id: "action:push", label: "Push…", shortcut: "⇧⌘K", run: onpush },
    ],
    [
      { id: "action:branch", label: "New Branch…", run: onnewbranch },
      { id: "action:ref", label: "Checkout Tag or Revision…", run: oncheckoutref },
    ],
  ]);
  const ongoingRows = $derived<ActionRow[]>(
    ongoing.flatMap((item) =>
      item && "label" in item
        ? [{ id: `ongoing:${item.label}`, label: item.label, run: item.disabled ? undefined : item.run }]
        : [],
    ),
  );
  const filtering = $derived(query.trim() !== "" && query.trim() !== "/");
  const noResults = $derived(list !== null && filtering && tree.length === 0);
  const rows = $derived<Row[]>(
    noResults || !list
      ? []
      : [
          ...ongoingRows,
          ...(ongoingRows.length > 0 ? (["separator"] as const) : []),
          ...actions[0],
          "separator",
          ...actions[1],
          "separator",
          ...visibleRows(tree, expanded),
        ],
  );
  const selectable = $derived(rows.filter((row) => row !== "separator"));

  const rowId = (row: Exclude<Row, "separator">) => ("node" in row ? row.node.id : row.id);
  const selectedRow = $derived(selectable.find((row) => rowId(row) === selected));
  const hasRemotes = $derived((list?.remote.length ?? 0) > 0);

  // A branch name can contain anything: it does not go into a selector.
  function rowElement(id: string) {
    const elements = box?.querySelectorAll<HTMLElement>("[data-id]") ?? [];
    return [...elements].find((element) => element.dataset.id === id);
  }

  function reveal(id: string | null) {
    if (!id) return;
    const path = pathTo(tree, id);
    if (path.some((group) => !expanded.has(group))) {
      expanded = new Set([...expanded, ...path]);
    }
    selected = id;
    tick().then(() => rowElement(id)?.scrollIntoView?.({ block: "nearest" }));
  }

  // `applySearchPattern`: what is open stays open, and the selection goes to the best one.
  function applyTree() {
    if (!list) return;
    expanded = new Set([...expanded, ...autoExpanded(tree, list, query)]);
    reveal(preferredId(tree, list, query) ?? nextLeaf(tree, null, 1)?.id ?? null);
  }

  // With large repositories it takes a while: the list does not wait for it, like IntelliJ's cached
  // information.
  async function readSync() {
    const token = ++syncToken;
    try {
      const state = await branchIncomingOutgoing();
      if (token === syncToken) sync = state;
    } catch {
      // Without this only the arrows are missing: the next signal reads again.
    }
  }

  async function read(first = false) {
    const token = ++readToken;
    readSync();
    try {
      await refs.note();
      const branches = await branchList();
      if (token !== readToken) return;
      list = branches;
      await tick();
      if (!first) {
        if (!selectedRow) applyTree();
        return;
      }
      const roots = tree.reduce(
        (count, node) =>
          count + 1 + (node.id === "recent" && node.kind !== "branch" ? node.children.length : 0),
        actions.flat().length + ongoingRows.length + (ongoingRows.length > 0 ? 1 : 0) + 2,
      );
      visibleCount = Math.min(roots, MAX_VISIBLE_ROWS);
      expanded = new Set($branchOptions.expanded);
      applyTree();
      await tick();
      place();
      placed = true;
    } catch (failure) {
      if (first) {
        notify({ kind: "error", title: errorText(failure) });
        onclose();
      }
    }
  }

  function place() {
    const bounds = box.getBoundingClientRect();
    const size = $branchOptions.popupSize;
    const [width, height] = [size?.width ?? bounds.width, size?.height ?? bounds.height];
    const x = anchor?.x ?? (window.innerWidth - width) / 2;
    const y = anchor?.y ?? (window.innerHeight - height) / 3;
    left = Math.max(MARGIN, Math.min(x, window.innerWidth - width - MARGIN));
    top = Math.max(MARGIN, Math.min(y, window.innerHeight - height - MARGIN));
  }

  function search() {
    // `onSpeedSearchPatternChanged`: a trailing space does not count.
    clearTimeout(searchTimer);
    open = null;
    searchTimer = setTimeout(() => {
      query = text.trim();
      tick().then(applyTree);
    }, SEARCH_DELAY);
  }

  function clearSearch() {
    text = "";
    clearTimeout(searchTimer);
    query = "";
    tick().then(applyTree);
  }

  function close() {
    onclose();
  }

  function saveState() {
    if (filtering || !list) return;
    // `GitBranchesPopupTreeStateHolder`: only the first two levels.
    const remembered = visibleRows(tree, expanded)
      .filter(({ node, depth }) => depth < 2 && node.kind !== "branch" && expanded.has(node.id))
      .map(({ node }) => node.id);
    setBranchOptions({ expanded: remembered });
  }

  function runAction(action: ActionRow) {
    if (!action.run) return;
    const run = action.run;
    close();
    tick().then(run);
  }

  function showSubmenu(branch: BranchNode, focused: boolean) {
    const bounds = box.getBoundingClientRect();
    const rowTop = rowElement(branch.id)?.getBoundingClientRect().top ?? bounds.top;
    open = {
      branch,
      x: bounds.right - STEP_X_PADDING,
      y: rowTop - MENU_PADDING,
      flip: bounds.left,
      focused,
    };
  }

  function closeSubmenu() {
    open = null;
    field.focus();
  }

  function toggleGroup(node: GroupNode) {
    const next = new Set(expanded);
    if (!next.delete(node.id)) next.add(node.id);
    expanded = next;
  }

  function activate(row: Exclude<Row, "separator">) {
    selected = rowId(row);
    if (!("node" in row)) runAction(row);
    else if (row.node.kind === "branch") showSubmenu(row.node, true);
    else toggleGroup(row.node);
  }

  function star(branch: BranchNode) {
    toggleFavorite(branch.name, refOf(branch).kind as "local" | "remote").catch((failure) =>
      notify({ kind: "error", title: errorText(failure) }),
    );
  }

  function move(step: number) {
    if (filtering) {
      reveal(nextLeaf(tree, selected, step)?.id ?? null);
      return;
    }
    if (selectable.length === 0) return;
    const position = selectable.findIndex((row) => rowId(row) === selected);
    const next = position < 0 && step < 0 ? selectable.length - 1 : position + step;
    reveal(rowId(selectable[(next + selectable.length) % selectable.length]));
  }

  function moveTo(end: "first" | "last") {
    const step = end === "first" ? 1 : -1;
    if (filtering) reveal(nextLeaf(tree, null, step)?.id ?? null);
    else if (selectable.length > 0) {
      reveal(rowId(selectable[end === "first" ? 0 : selectable.length - 1]));
    }
  }

  function right() {
    if (!selectedRow || !("node" in selectedRow)) return;
    const { node } = selectedRow;
    if (node.kind === "branch") showSubmenu(node, true);
    else if (!expanded.has(node.id)) toggleGroup(node);
    else if (node.children.length > 0) selected = node.children[0].id;
  }

  function leftKey() {
    if (!selectedRow || !("node" in selectedRow)) return;
    const { node } = selectedRow;
    if (node.kind !== "branch" && expanded.has(node.id)) toggleGroup(node);
    else {
      const parent = pathTo(tree, node.id).at(-1);
      if (parent) selected = parent;
    }
  }

  function keydown(event: KeyboardEvent) {
    event.stopPropagation();
    // `installShortcutActions`: the shortcuts of the rows above work here.
    const shortcut = matchShortcut(event);
    if (shortcut === "CheckinProject") runAction(actions[0][1]);
    else if (shortcut === "Vcs.UpdateProject") runAction(actions[0][0]);
    else if (shortcut === "Vcs.Push") runAction(actions[0][2]);
    else if (event.key === "ArrowDown") move(1);
    else if (event.key === "ArrowUp") move(-1);
    else if (event.key === "ArrowRight") right();
    else if (event.key === "ArrowLeft") leftKey();
    else if (event.key === "Home") moveTo("first");
    else if (event.key === "End") moveTo("last");
    else if (event.key === "Enter") {
      shift = event.shiftKey;
      if (selectedRow) activate(selectedRow);
    } else if (event.key === " ") {
      if (selectedRow && "node" in selectedRow && selectedRow.node.kind === "branch") {
        star(selectedRow.node);
      }
    } else if (event.key === "Escape") {
      if (text) clearSearch();
      else close();
    } else return;
    event.preventDefault();
  }

  function pointermove(event: PointerEvent, row: Exclude<Row, "separator">) {
    // `isMouseMoved`: the list moving under a still mouse does not count.
    const position = `${event.screenX},${event.screenY}`;
    if (position === lastPointer) return;
    lastPointer = position;
    const id = rowId(row);
    if (selected === id) return;
    selected = id;
    clearTimeout(hoverTimer);
    hoverTimer = setTimeout(() => {
      if (selected !== id) return;
      if ("node" in row && row.node.kind === "branch") showSubmenu(row.node, false);
      else open = null;
    }, HOVER_DELAY);
  }

  function pointerdown(event: PointerEvent) {
    if (!box.contains(event.target as Node)) close();
  }

  async function fetchRemotes() {
    if (!onfetch || fetching) return;
    fetching = true;
    try {
      await onfetch();
      // `forceUpdateBranches` with `updateBranchesWithIncoming(false)`: it does not ask the remotes.
      await read();
    } finally {
      fetching = false;
    }
  }

  function showSettings(event: MouseEvent) {
    const bounds = (event.currentTarget as HTMLElement).getBoundingClientRect();
    settings = { x: bounds.left, y: bounds.bottom };
  }

  function restoreSize() {
    setBranchOptions({ popupSize: null });
  }

  const settingsItems = (): MenuItem[] => [
    {
      label: "Group by Prefix",
      checked: $branchOptions.grouping,
      run: () => setBranchOptions({ grouping: !$branchOptions.grouping }),
    },
    {
      label: "Show Recent Branches",
      checked: $branchOptions.showRecent,
      run: () => setBranchOptions({ showRecent: !$branchOptions.showRecent }),
    },
    { label: "Restore Popup Size", disabled: !$branchOptions.popupSize, run: restoreSize },
  ];

  // `userResized`: the size only stays in the style if whoever drags has set it.
  function resized() {
    if (!box.style.width) return;
    const { width, height } = box.getBoundingClientRect();
    const known = $branchOptions.popupSize;
    if (known?.width !== width || known?.height !== height) {
      setBranchOptions({ popupSize: { width, height } });
    }
  }

  onMount(() => {
    const previous = document.activeElement;
    field.focus();
    untrack(() => read(true));
    const stops = [
      onRepoChanged(async () => {
        if (await refs.changed()) read();
      }),
      onBranchSyncChanged(readSync),
    ];
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(resized);
    observer?.observe(box);
    return () => {
      observer?.disconnect();
      untrack(saveState);
      if (previous instanceof HTMLElement && previous.isConnected) previous.focus();
      clearTimeout(searchTimer);
      clearTimeout(hoverTimer);
      stops.forEach((stop) => stop.then((unlisten) => unlisten()));
    };
  });

  const iconOf = (branch: BranchNode, active: boolean) => {
    if (active) return branch.favorite ? "favorite" : "notFavorite";
    if (branch.current) return branch.favorite ? "currentBranchFavorite" : "currentBranch";
    return branch.favorite ? "favorite" : "branchLabel";
  };

  const syncOf = (branch: BranchNode) => sync[branch.name];
  const hasSync = (branch: BranchNode) => branch.section !== "remote" && syncOf(branch);
</script>

<svelte:window onpointerdown={pointerdown} />

{#snippet branchText(label: string)}
  {#each highlighted(label, query) as part}
    {#if part.hit}<span class="hit">{part.text}</span>{:else}{part.text}{/if}
  {/each}
{/snippet}

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  bind:this={box}
  class="popup"
  style:left="{left}px"
  style:top="{top}px"
  style:opacity={placed ? null : 0}
  style:width={$branchOptions.popupSize ? `${$branchOptions.popupSize.width}px` : null}
  style:height={$branchOptions.popupSize ? `${$branchOptions.popupSize.height}px` : null}
  style:min-width="{MIN_WIDTH}px"
  style:min-height="{MIN_HEIGHT}px"
  onkeydown={keydown}
>
  <div class="header">
    <input
      bind:this={field}
      bind:value={text}
      class="search"
      aria-label="Search"
      placeholder="Search"
      spellcheck="false"
      oninput={search}
    />
    <button
      type="button"
      class="action"
      aria-label={fetching ? "Fetching…" : "Fetch"}
      title={fetching ? "Fetching…" : "Fetch"}
      disabled={!onfetch || fetching}
      onclick={fetchRemotes}
    >
      <Icon name="fetch" />
    </button>
    <button type="button" class="action" aria-label="Settings" title="Settings" onclick={showSettings}>
      <Icon name="settings" />
    </button>
  </div>

  <div
    class="tree"
    role="tree"
    aria-label="Branches Tree"
    style:height={$branchOptions.popupSize ? null : `${visibleCount * ROW_HEIGHT}px`}
  >
    {#if noResults}
      <p class="empty">Branch '{query}' not found</p>
    {/if}
    {#each rows as row, index (row === "separator" ? `separator${index}` : rowId(row))}
      {#if row === "separator"}
        <div class="separator" role="separator"></div>
      {:else if "node" in row}
        {@const { node, depth } = row}
        {@const active = selected === node.id}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <div
          class="row"
          class:selected={active}
          role="treeitem"
          tabindex="-1"
          aria-label={node.label}
          aria-selected={active}
          aria-expanded={node.kind === "branch" ? undefined : expanded.has(node.id)}
          data-id={node.id}
          title={node.kind === "branch" && hasSync(node)
            ? syncTooltip(syncOf(node).incoming, syncOf(node).outgoing)
            : undefined}
          style:padding-left="{8 + depth * 20}px"
          onpointermove={(event) => pointermove(event, row)}
          onclick={() => activate(row)}
        >
          {#if node.kind === "branch"}
            <button
              type="button"
              class="main-icon"
              tabindex="-1"
              aria-label="Mark/Unmark As Favorite"
              onclick={(event) => {
                event.stopPropagation();
                star(node);
              }}
            >
              <Icon name={iconOf(node, active)} />
            </button>
            <span class="label">{@render branchText(node.label)}</span>
            {#if hasSync(node)}
              <span class="sync">
                {#if syncOf(node).incoming}<Icon name="incoming" />{/if}
                {#if syncOf(node).outgoing}<Icon name="outgoing" />{/if}
              </span>
            {/if}
            {#if node.tracked}<span class="secondary">{node.tracked}</span>{/if}
            <span class="arrow"><Icon name="chevronRight" size={14} /></span>
          {:else}
            <span class="chevron">
              <Icon name={expanded.has(node.id) ? "chevronDown" : "chevronRight"} size={14} />
            </span>
            {#if node.kind === "folder"}<Icon name="folder" />{/if}
            <span class="label">{node.label}</span>
          {/if}
        </div>
      {:else}
        {@const active = selected === row.id}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <div
          class="row action-row"
          class:selected={active}
          class:disabled={!row.run}
          role="treeitem"
          tabindex="-1"
          aria-label={row.label}
          aria-selected={active}
          aria-disabled={!row.run}
          data-id={row.id}
          onpointermove={(event) => pointermove(event, row)}
          onclick={(event) => {
            shift = event.shiftKey;
            runAction(row);
          }}
        >
          <span class="label">{row.label}</span>
          {#if row.shortcut}<span class="secondary">{row.shortcut}</span>{/if}
        </div>
      {/if}
    {/each}
  </div>

  {#if open}
    {#key open}
      <Menu
        nested
        x={open.x}
        y={open.y}
        flip={open.flip}
        focused={open.focused}
        label={open.branch.name}
        items={branchActions(open.branch, {
          current: list?.current ?? null,
          remotes: hasRemotes,
          onvcs,
          protectedBranches: $protectedBranches,
          head,
        })}
        onclose={close}
        onback={closeSubmenu}
      />
    {/key}
  {/if}

  {#if settings}
    <Menu
      x={settings.x}
      y={settings.y}
      label="Settings"
      items={settingsItems()}
      onclose={() => (settings = null)}
    />
  {/if}
</div>

<style>
  .popup {
    position: fixed;
    z-index: 15;
    display: flex;
    flex-direction: column;
    box-sizing: border-box;
    width: fit-content;
    max-width: calc(100vw - 16px);
    max-height: calc(100vh - 16px);
    overflow: hidden;
    background: var(--popup-background);
    border: 1px solid var(--popup-border);
    border-radius: 8px;
    box-shadow: 0 6px 24px rgb(0 0 0 / 40%);
    resize: both;
  }

  .header {
    display: flex;
    flex: none;
    gap: 2px;
    align-items: center;
    height: 36px;
    padding: 0 8px 0 14px;
    border-bottom: 1px solid var(--menu-separator);
  }

  .search {
    flex: 1;
    min-width: 0;
    padding: 0;
    font: inherit;
    color: var(--text);
    background: none;
    border: 0;
    outline: none;
  }

  .search::placeholder {
    color: var(--muted);
  }

  .tree {
    flex: 1 1 auto;
    min-height: 0;
    padding: 5px 0;
    overflow: auto;
    outline: none;
  }

  .empty {
    padding: 12px 14px;
    margin: 0;
    color: var(--muted);
    text-align: center;
  }

  .separator {
    height: 1px;
    margin: 4px 14px;
    background: var(--menu-separator);
  }

  .row {
    display: flex;
    gap: 6px;
    align-items: center;
    box-sizing: border-box;
    height: 24px;
    padding-right: 8px;
    margin: 0 6px;
    color: var(--text);
    white-space: nowrap;
    border-radius: 4px;
    content-visibility: auto;
    contain-intrinsic-height: 24px;
    outline: none;
  }

  .row.selected {
    background: var(--selection);
  }

  .row.disabled {
    color: var(--disabled);
  }

  .action-row {
    padding-left: 28px;
  }

  .main-icon {
    display: inline-flex;
    flex: none;
    padding: 0;
    margin-left: 20px;
    background: none;
    border: 0;
  }

  .chevron,
  .arrow {
    display: inline-flex;
    flex: none;
    color: var(--muted);
  }

  .chevron {
    width: 14px;
  }

  .arrow {
    visibility: hidden;
  }

  .row.selected .arrow {
    visibility: visible;
  }

  .label {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .hit {
    color: var(--accent-text);
    background: var(--accent);
    border-radius: 2px;
  }

  .sync {
    display: inline-flex;
    flex: none;
    margin-left: 10px;
  }

  .secondary {
    flex: none;
    margin-left: auto;
    padding-left: 10px;
    color: var(--shortcut);
  }
</style>
