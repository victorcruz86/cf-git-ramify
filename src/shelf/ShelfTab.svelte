<script lang="ts">
  import { tick, untrack } from "svelte";
  import type { ListFiles, ShelfEntry, ShelfSettings, ShelfSides } from "../lib/api";
  import Icon from "../lib/Icon.svelte";
  import { matchShelfShortcut, matchShortcut } from "../lib/keymap";
  import Menu from "../lib/Menu.svelte";
  import type { MenuItem } from "../lib/menu";
  import { store, stored } from "../lib/storage";
  import SavedDiffDialog from "../stash/SavedDiffDialog.svelte";
  import SavedDiffPane from "../stash/SavedDiffPane.svelte";
  import type { SavedChange, SavedDiff } from "../stash/savedPatches";
  import { IMPORT_IDEA, clearShelfEdit, shelfEditRequest } from "./flow";
  import {
    TAG_ID,
    diffChain,
    expandableIds,
    fileName,
    greyed,
    listId,
    listText,
    sameFile,
    selectionOf,
    treeRows,
    type ShelfRow,
    type ShelfSelection,
  } from "./shelfTree";

  // The Shelf tab (`ShelvedChangesViewManager.ShelfToolWindowPanel`): the vertical toolbar of
  // `ShelvedChangesToolbar`, the tree of lists and files with several selected at once, and the diff
  // preview on the right. Everything shown comes from the store and is text.
  interface Props {
    entries: ShelfEntry[];
    settings: ShelfSettings;
    // In front: the inline rename that Shelve Silently asks for only starts then (`isShelfContentActive`).
    active: boolean;
    now?: () => number;
    api: { sides(name: string, file: ShelfEntry["changes"][number]["file"], withLocal: boolean): Promise<ShelfSides> };
    // `dialog`: `Unshelve...`; otherwise `Unshelve Silently`.
    onunshelve: (selection: ShelfSelection, dialog: boolean) => unknown;
    ondelete: (selection: ShelfSelection) => unknown;
    onrestore: (names: string[]) => unknown;
    onrename: (name: string, description: string) => unknown;
    onimport: () => unknown;
    onimportidea: () => unknown;
    oncreatepatch: (files: ListFiles[]) => unknown;
    oncopypatch: (files: ListFiles[]) => unknown;
    onsettings: (settings: ShelfSettings) => unknown;
  }

  let {
    entries,
    settings,
    active,
    now = Date.now,
    api,
    onunshelve,
    ondelete,
    onrestore,
    onrename,
    onimport,
    onimportidea,
    oncreatepatch,
    oncopypatch,
    onsettings,
  }: Props = $props();

  // `ShelvedChangesViewManager.DETAILS_SPLITTER_PROPORTION`.
  const PREVIEW_SPLITTER = "ShelvedChangesViewManager.DETAILS_SPLITTER_PROPORTION";
  const PROPORTION = 0.5;
  const MIN_PROPORTION = 0.1;
  const MAX_PROPORTION = 0.9;

  let tree = $state<HTMLDivElement>();
  let split = $state<HTMLDivElement>();
  let expanded = $state.raw(new Set<string>());
  let selected = $state.raw(new Set<string>());
  let lead = $state<string | null>(null);
  let anchor: string | null = null;
  let editing = $state<{ name: string; value: string } | null>(null);
  let pendingEdit: string | null = null;
  // A list asked for before the tab read it (`activateView` right after a task made it).
  let pendingSelect: string | null = null;
  let menu = $state.raw<{ x: number; y: number; items: MenuItem[]; label: string } | null>(null);
  let dialog = $state.raw<{ chain: { list: string; change: SavedChange }[]; start: number } | null>(null);
  let proportion = $state(
    untrack(() => {
      const value = Number(stored(PREVIEW_SPLITTER));
      return value >= MIN_PROPORTION && value <= MAX_PROPORTION ? value : PROPORTION;
    }),
  );

  const rows = $derived(
    treeRows(entries, {
      showRecycled: settings.showRecycled,
      groupByDirectory: settings.groupByDirectory,
      expanded,
    }),
  );
  const selection = $derived(selectionOf(entries, rows, selected));
  const leadRow = $derived(rows.find((row) => row.id === lead) ?? null);
  const nowMs = $derived.by(() => {
    void entries;
    return now();
  });

  // `TreeUtil.selectFirstNode` and, when the selection is gone, the first row again.
  $effect(() => {
    const shown = rows;
    untrack(() => {
      const kept = new Set([...selected].filter((id) => shown.some((row) => row.id === id)));
      if (kept.size === 0 && shown.length > 0) {
        selected = new Set([shown[0].id]);
        lead = shown[0].id;
      } else if (kept.size !== selected.size) {
        selected = kept;
        if (!lead || !kept.has(lead)) lead = [...kept][0] ?? null;
      }
    });
  });

  export function focus() {
    tree?.focus();
  }

  // `selectShelvedList`: the list's row, and its place in the tree opened.
  export function select(name: string): boolean {
    const entry = entries.find((item) => item.list.name === name);
    if (!entry) {
      pendingSelect = name;
      return false;
    }
    pendingSelect = null;
    if (entry.list.deleted && !expanded.has(TAG_ID)) expanded = new Set([...expanded, TAG_ID]);
    selected = new Set([listId(name)]);
    lead = listId(name);
    anchor = lead;
    return true;
  }

  // `startEditing`: the list selected and its name in a field.
  export async function startEditing(name: string) {
    if (!select(name)) {
      pendingEdit = name;
      return;
    }
    const entry = entries.find((item) => item.list.name === name);
    editing = { name, value: entry?.list.description ?? "" };
    await tick();
    const field = document.querySelector<HTMLInputElement>(".rename-field");
    field?.focus();
    field?.select();
  }

  // Shelve Silently's request: taken while the tab is in front, otherwise dropped.
  $effect(() =>
    shelfEditRequest.subscribe((name) => {
      if (name === null) return;
      untrack(() => {
        clearShelfEdit();
        if (active) void startEditing(name);
      });
    }),
  );

  // A list the request named that the tree did not have yet.
  $effect(() => {
    const shown = entries;
    untrack(() => {
      const wanted = pendingSelect;
      if (wanted && shown.some((entry) => entry.list.name === wanted)) select(wanted);
      const name = pendingEdit;
      if (name && shown.some((entry) => entry.list.name === name)) {
        pendingEdit = null;
        void startEditing(name);
      }
    });
  });

  function commitRename() {
    const current = editing;
    editing = null;
    if (current) onrename(current.name, current.value);
    tree?.focus();
  }

  function cancelRename() {
    editing = null;
    tree?.focus();
  }

  function toggle(id: string) {
    const next = new Set(expanded);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    expanded = next;
  }

  const expandable = (row: ShelfRow) => row.kind === "list" || row.kind === "folder" || row.kind === "tag";

  function click(row: ShelfRow, event: MouseEvent) {
    if (event.metaKey || event.ctrlKey) {
      const next = new Set(selected);
      if (next.has(row.id)) next.delete(row.id);
      else next.add(row.id);
      selected = next;
      anchor = row.id;
    } else if (event.shiftKey && anchor) {
      const from = rows.findIndex((item) => item.id === anchor);
      const to = rows.findIndex((item) => item.id === row.id);
      const [low, high] = from < to ? [from, to] : [to, from];
      selected = new Set(rows.slice(low, high + 1).map((item) => item.id));
    } else {
      selected = new Set([row.id]);
      anchor = row.id;
    }
    lead = row.id;
    tree?.focus();
  }

  // `setDoubleClickHandler` / `setEnterKeyHandler`: the diff of the selected files, otherwise the
  // node opens or closes (`getToggleClickCount` is 2).
  function open(row: ShelfRow) {
    if (selection.files.length > 0 && row.kind === "file") void showDiff(false);
    else if (expandable(row)) toggle(row.id);
  }

  function move(step: number, extend: boolean) {
    if (rows.length === 0) return;
    const index = rows.findIndex((row) => row.id === lead);
    const next = rows[Math.min(Math.max(index + step, 0), rows.length - 1)];
    if (extend) selected = new Set([...selected, next.id]);
    else {
      selected = new Set([next.id]);
      anchor = next.id;
    }
    lead = next.id;
  }

  const canRename = () => selection.lists.length === 1;
  const deletedNames = () => selection.lists.filter((entry) => entry.list.deleted).map((entry) => entry.list.name);
  // `VcsTreeModelData.selected`: a selected list counts with all its files.
  const chosenFiles = (): ListFiles[] => {
    const exact = selection.exact.map((entry) => ({
      name: entry.list.name,
      files: entry.changes.map((change) => change.file),
    }));
    const names = new Set(exact.map((files) => files.name));
    return [...exact, ...selection.files.filter((files) => !names.has(files.name))];
  };
  const hasChanges = () => chosenFiles().some((files) => files.files.length > 0);

  function rename() {
    if (!canRename()) return;
    void startEditing(selection.lists[0].list.name);
  }

  function remove() {
    if (selection.lists.length > 0) ondelete(selection);
  }

  function unshelve(withDialog: boolean) {
    if (selection.lists.length > 0) onunshelve(selection, withDialog);
  }

  const asSaved = (list: string, change: ShelfEntry["changes"][number]): SavedChange => ({
    file: { path: change.path, oldPath: change.oldPath, status: change.status },
    group: null,
    source: list,
  });

  // `Show Diff` and `Compare with Local` (`getSelectedChangesOrAll`).
  async function showDiff(withLocal: boolean) {
    const chain = diffChain(selection);
    if (chain.length === 0) return;
    const first = leadRow?.kind === "file" ? leadRow : null;
    const start = first
      ? Math.max(0, chain.findIndex((item) => item.list === first.list && sameFile(item.change.file, first.change.file)))
      : 0;
    dialog = {
      chain: chain.map((item) => ({ list: item.list, change: asSaved(item.list, item.change) })),
      start,
    };
    diffWithLocal = withLocal;
  }

  let diffWithLocal = false;

  function loadDiff(change: SavedChange): Promise<SavedDiff> {
    const entry = entries.find((item) => item.list.name === change.source);
    const shelved = entry?.changes.find((item) => item.path === change.file.path && item.oldPath === change.file.oldPath);
    if (!shelved) return Promise.reject(new Error(`Cannot show diff for '${change.file.path}'`));
    return api.sides(change.source, shelved.file, diffWithLocal);
  }

  // The preview follows the lead file.
  const preview = $derived.by(() => {
    const row = leadRow;
    if (row?.kind !== "file") return null;
    const file = row.change.file;
    const name = row.list;
    return () => api.sides(name, file, false);
  });

  function expandAll() {
    expanded = expandableIds(entries);
  }

  function collapseAll() {
    expanded = new Set();
    const first = rows[0];
    if (first) {
      selected = new Set([first.id]);
      lead = first.id;
    }
  }

  function keydown(event: KeyboardEvent) {
    if (editing) return;
    const shelfAction = matchShelfShortcut(event);
    const action = matchShortcut(event);
    const plain = !event.metaKey && !event.ctrlKey && !event.altKey;
    if (shelfAction === "ChangesView.UnshelveSilently") unshelve(false);
    else if (shelfAction === "ShelveChanges.UnshelveWithDialog") unshelve(true);
    else if (shelfAction === "ShelvedChanges.Rename") rename();
    else if (action === "Diff.ShowDiff") void showDiff(false);
    else if (action === "ExpandAll") expandAll();
    else if (action === "CollapseAll") collapseAll();
    else if ((event.key === "Delete" || event.key === "Backspace") && !event.ctrlKey && !event.altKey) remove();
    else if (plain && (event.key === "ArrowDown" || event.key === "ArrowUp")) {
      move(event.key === "ArrowDown" ? 1 : -1, event.shiftKey);
    } else if (plain && event.key === "ArrowRight" && leadRow && expandable(leadRow)) {
      if (!expanded.has(leadRow.id)) toggle(leadRow.id);
      else move(1, false);
    } else if (plain && event.key === "ArrowLeft" && leadRow) {
      if (expandable(leadRow) && expanded.has(leadRow.id)) toggle(leadRow.id);
    } else if (plain && event.key === "Enter" && leadRow) open(leadRow);
    else return;
    event.preventDefault();
    event.stopPropagation();
  }

  // `ShelvedChangesPopupMenu`.
  function popupItems(): MenuItem[] {
    const lists = selection.lists.length > 0;
    const changes = hasChanges();
    return [
      { label: "Unshelve...", shortcut: "⇧⌘U", disabled: !lists, run: () => unshelve(true) },
      { label: "Restore", disabled: deletedNames().length === 0, run: () => onrestore(deletedNames()) },
      { label: "Show Diff", shortcut: "⌘D", disabled: !changes, run: () => void showDiff(false) },
      { label: "Show Diff in a New Tab or Window", disabled: !changes, run: () => void showDiff(false) },
      { label: "Compare with Local", disabled: !changes, run: () => void showDiff(true) },
      { label: "Create Patch...", disabled: !changes, run: () => oncreatepatch(chosenFiles()) },
      { label: "Copy as Patch to Clipboard", disabled: !changes, run: () => oncopypatch(chosenFiles()) },
      { label: "Import Patches…", run: () => onimport() },
      { label: IMPORT_IDEA, run: () => onimportidea() },
      null,
      { label: "Rename...", shortcut: "F2", disabled: !canRename(), run: rename },
      { label: "Delete", shortcut: "⌫", disabled: !lists, run: remove },
    ];
  }

  function contextMenu(event: MouseEvent, row: ShelfRow) {
    event.preventDefault();
    if (!selected.has(row.id)) {
      selected = new Set([row.id]);
      lead = row.id;
      anchor = row.id;
    }
    menu = { x: event.clientX, y: event.clientY, items: popupItems(), label: "Shelf" };
  }

  function toolbarMenu(event: MouseEvent, label: string, items: MenuItem[]) {
    const bounds = (event.currentTarget as HTMLElement).getBoundingClientRect();
    menu = { x: bounds.right + 2, y: bounds.top, items, label };
  }

  // `ShelvedChangesToolbar.ViewOptions`: `Group By` ▸ Directory, then `Show` with `Already Unshelved`.
  function viewOptions(event: MouseEvent) {
    toolbarMenu(event, "View Options", [
      {
        label: "Group By",
        submenu: [
          { header: "Group By" },
          {
            label: "Directory",
            checked: settings.groupByDirectory,
            run: () => onsettings({ ...settings, groupByDirectory: !settings.groupByDirectory }),
          },
        ],
      },
      { header: "Show" },
      {
        label: "Already Unshelved",
        checked: settings.showRecycled,
        run: () => onsettings({ ...settings, showRecycled: !settings.showRecycled }),
      },
    ]);
  }

  function resize(event: PointerEvent) {
    const divider = event.currentTarget as HTMLElement;
    if (!split) return;
    divider.setPointerCapture?.(event.pointerId);
    const bounds = split.getBoundingClientRect();
    const moved = (next: PointerEvent) => {
      const part = (next.clientX - bounds.left) / bounds.width;
      if (!Number.isFinite(part)) return;
      proportion = Math.round(Math.min(MAX_PROPORTION, Math.max(MIN_PROPORTION, part)) * 1000) / 1000;
    };
    const finish = () => {
      divider.removeEventListener("pointermove", moved);
      divider.removeEventListener("pointerup", finish);
      divider.removeEventListener("pointercancel", finish);
      store(PREVIEW_SPLITTER, String(proportion));
    };
    divider.addEventListener("pointermove", moved);
    divider.addEventListener("pointerup", finish);
    divider.addEventListener("pointercancel", finish);
  }

  const STATUS_COLORS: Record<string, string> = {
    A: "var(--status-added)",
    D: "var(--status-deleted)",
    M: "var(--status-modified)",
    R: "var(--status-modified)",
  };

  // `ShelvedChangeNode.render`: the name, then its folder (`<project root>` at the root).
  const folderOf = (path: string) => path.slice(0, Math.max(0, path.lastIndexOf("/"))) || "<project root>";
  const rowLabel = (row: ShelfRow): string => {
    if (row.kind === "tag") return row.text;
    if (row.kind === "folder") return row.name;
    if (row.kind === "file") {
      const name = fileName(row.change.path);
      return settings.groupByDirectory || !row.change.path.includes("/") ? name : `${name} ${folderOf(row.change.path)}`;
    }
    const text = listText(row.entry, nowMs);
    return `${text.name}${text.count}${text.date}${row.entry.error ? ` ${row.entry.error}` : ""}`;
  };
</script>

<div class="shelf" bind:this={split}>
  <div class="side" style:flex="{proportion} 1 0">
    <div class="toolbar" role="toolbar" aria-label="Shelf" aria-orientation="vertical">
      <button
        type="button"
        class="action"
        title="Unshelve Silently (⌥⌘U)"
        aria-label="Unshelve Silently"
        disabled={selection.lists.length === 0}
        onclick={() => unshelve(false)}
      >
        <Icon name="unshelve" />
      </button>
      <button
        type="button"
        class="action"
        title="Show Diff (⌘D)"
        aria-label="Show Diff"
        disabled={!hasChanges()}
        onclick={() => void showDiff(false)}
      >
        <Icon name="diff" />
      </button>
      <button type="button" class="action" title="View Options" aria-label="View Options" onclick={viewOptions}>
        <Icon name="show" />
      </button>
      <button type="button" class="action" title="Expand All (⌘+)" aria-label="Expand All" onclick={expandAll}>
        <Icon name="expandAll" />
      </button>
      <button type="button" class="action" title="Collapse All (⌘-)" aria-label="Collapse All" onclick={collapseAll}>
        <Icon name="collapseAll" />
      </button>
    </div>
    <div
      bind:this={tree}
      class="tree"
      role="tree"
      aria-label="Shelf"
      aria-multiselectable="true"
      tabindex="0"
      onkeydown={keydown}
    >
      {#each rows as row (row.id)}
        <!-- The keys go to the tree, which holds the selection. -->
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <div
          role="treeitem"
          tabindex="-1"
          aria-label={rowLabel(row)}
          aria-selected={selected.has(row.id)}
          aria-expanded={expandable(row) ? expanded.has(row.id) : undefined}
          class="row {row.kind}"
          class:current={selected.has(row.id)}
          class:greyed={row.kind === "list" && greyed(row.entry)}
          style:padding-left="{6 + row.depth * 18}px"
          onclick={(event) => click(row, event)}
          ondblclick={() => {
            if (!selected.has(row.id)) {
              selected = new Set([row.id]);
              anchor = row.id;
            }
            lead = row.id;
            open(row);
          }}
          oncontextmenu={(event) => contextMenu(event, row)}
        >
          {#if expandable(row)}
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <span
              class="chevron"
              onclick={(event) => {
                event.stopPropagation();
                toggle(row.id);
              }}
            >
              <Icon name={expanded.has(row.id) ? "chevronDown" : "chevronRight"} size={12} />
            </span>
          {:else}
            <span class="chevron"></span>
          {/if}
          {#if row.kind === "list"}
            {@const text = listText(row.entry, nowMs)}
            {#if editing && editing.name === row.entry.list.name}
              <input
                class="input rename-field"
                aria-label="Rename"
                value={editing.value}
                oninput={(event) => editing && (editing = { ...editing, value: event.currentTarget.value })}
                onkeydown={(event) => {
                  event.stopPropagation();
                  if (event.key === "Enter") {
                    event.preventDefault();
                    commitRename();
                  } else if (event.key === "Escape") {
                    event.preventDefault();
                    cancelRename();
                  }
                }}
                onblur={cancelRename}
                onclick={(event) => event.stopPropagation()}
              />
            {:else}
              <span class="name">{text.name}</span>
            {/if}
            <span class="grey">{text.count}{text.date}</span>
            {#if row.entry.error}
              <span class="error"> {row.entry.error}</span>
            {/if}
          {:else if row.kind === "tag"}
            <span class="name">{row.text}</span>
          {:else if row.kind === "folder"}
            <Icon name="folder" size={14} />
            <span class="name">{row.name}</span>
          {:else}
            <span class="name" style:color={STATUS_COLORS[row.change.status[0]]}>{fileName(row.change.path)}</span>
            {#if !settings.groupByDirectory && row.change.path.includes("/")}
              <span class="grey">{folderOf(row.change.path)}</span>
            {/if}
          {/if}
        </div>
      {/each}
    </div>
  </div>
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div
    class="splitter"
    role="separator"
    aria-orientation="vertical"
    aria-valuenow={Math.round(proportion * 100)}
    onpointerdown={resize}
  ></div>
  <section class="preview" aria-label="Diff Preview" style:flex="{1 - proportion} 1 0">
    {#if leadRow?.kind === "file"}
      <SavedDiffPane load={preview} path={leadRow.change.path} />
    {/if}
  </section>
</div>

{#if menu}
  <Menu x={menu.x} y={menu.y} label={menu.label} items={menu.items} onclose={() => (menu = null)} />
{/if}

{#if dialog}
  {@const shown = dialog}
  <SavedDiffDialog
    changes={shown.chain.map((item) => item.change)}
    start={shown.start}
    titleOf={(change) => `Shelved: ${fileName(change.file.path)}`}
    load={loadDiff}
    onclose={() => (dialog = null)}
  />
{/if}

<style>
  .shelf {
    display: flex;
    flex: 1;
    min-width: 0;
    min-height: 0;
    background: var(--panel);
  }

  .side {
    display: flex;
    min-width: 0;
  }

  .toolbar {
    display: flex;
    flex: none;
    flex-direction: column;
    gap: 2px;
    padding: 4px 3px;
    border-right: 1px solid var(--divider);
  }

  .tree {
    flex: 1;
    min-width: 0;
    padding: 2px 0;
    overflow: auto;
    outline: none;
  }

  .row {
    display: flex;
    gap: 4px;
    align-items: center;
    padding: 2px 8px;
    white-space: nowrap;
  }

  .row.current {
    background: var(--selection);
  }

  .row.greyed .name {
    font-weight: 600;
    color: var(--text-secondary);
  }

  .chevron {
    display: inline-flex;
    flex: none;
    width: 12px;
    color: var(--text-secondary);
  }

  .name {
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .grey {
    color: var(--text-secondary);
  }

  .error {
    color: var(--error);
  }

  .rename-field {
    min-width: 160px;
    padding: 0 4px;
  }

  .splitter {
    position: relative;
    z-index: 2;
    flex: none;
    width: 5px;
    margin: 0 -2px;
    cursor: col-resize;
    background: linear-gradient(90deg, transparent 2px, var(--border) 2px, var(--border) 3px, transparent 3px);
  }

  .preview {
    display: flex;
    min-width: 0;
    min-height: 0;
  }
</style>
