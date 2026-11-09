<script lang="ts">
  import { onMount, tick, untrack } from "svelte";
  import { repoStatus, treeList, type TreeEntry } from "../lib/api";
  import Dialog from "../lib/Dialog.svelte";
  import { errorText } from "../lib/errors";
  import { notify } from "../lib/notifications";
  import {
    ROOT,
    checkState,
    parentOf,
    stateOf,
    statusIndex,
    toggleChecked,
    visibleRows,
    type StatusIndex,
  } from "../tree/model";
  import { filterPath, pathFilterValues } from "./filters";
  import { TreeNavigation } from "../tree/navigation.svelte";
  import TreeView from "../tree/TreeView.svelte";

  // `VcsStructureChooser.MAX_FOLDERS`.
  const MAX_SELECTED = 100;
  // `vcs.log.select.folder.dialog.title` (L:219) and `vcs.log.filters.structure.*` (L:220-221).
  const TITLE = "Select Paths to Filter by";
  const LIMIT_TEXT = `(You have added ${MAX_SELECTED} elements. No more is allowed.)`;

  interface Props {
    // The repository's folder: the root of the tree.
    root: string;
    // The values of the Paths filter now: those that exist start checked.
    paths: string[];
    onok: (values: string[]) => void;
    onclose: () => void;
  }

  let { root, paths, onok, onclose }: Props = $props();

  let listed = $state.raw(new Map<string, TreeEntry[]>());
  let expanded = $state.raw(new Set<string>([ROOT]));
  let selected = $state.raw<ReadonlySet<string>>(new Set());
  let status: StatusIndex = statusIndex([]);
  let view = $state<TreeView>();
  // The folders are read once the status says which entries to leave out.
  let ready = $state(false);
  let disposed = false;
  const loading = new Set<string>();
  // Whether each checked path is a folder, for the filter's values.
  const dirs = new Map<string, boolean>([[ROOT, true]]);

  const parts = $derived(root.split("/").filter(Boolean));
  const rootName = $derived(parts.at(-1) ?? root);
  // `WithModulesListCellRenderer.putParentPathImpl`: a root with a parent folder shows its own path,
  // in grey and between parentheses.
  const rootPath = $derived(parts.length > 0 ? `(${root})` : "");
  const rows = $derived(visibleRows(rootName, listed, expanded));
  const indexOf = $derived(new Map(rows.map((row, index) => [row.entry.path, index])));
  const nav = new TreeNavigation(
    {
      rows: () => rows,
      indexOf: () => indexOf,
      reveal: (index) => view?.reveal(index),
      pageSize: () => view?.pageSize() ?? 1,
    },
    ROOT,
  );
  const full = $derived(selected.size >= MAX_SELECTED);

  // `isFileVisible`: neither ignored nor unversioned. Folders first, then by path without case.
  function shown(entries: TreeEntry[]): TreeEntry[] {
    const kept = entries.filter((entry) => {
      const state = stateOf(status, entry.path);
      return state !== "ignored" && state !== "untracked";
    });
    const key = (entry: TreeEntry) => entry.path.toLowerCase();
    return kept.sort((a, b) => Number(b.dir) - Number(a.dir) || (key(a) < key(b) ? -1 : key(a) > key(b) ? 1 : 0));
  }

  async function load(dir: string) {
    loading.add(dir);
    try {
      const entries = shown(await treeList(dir));
      if (disposed) return;
      entries.forEach((entry) => dirs.set(entry.path, entry.dir));
      listed = new Map(listed).set(dir, entries);
    } catch (failure) {
      if (!disposed) notify({ kind: "error", title: errorText(failure) });
    } finally {
      loading.delete(dir);
    }
  }

  // `getVirtualFile` of each path of the filter: what is not on disk is left out.
  async function existing(values: string[]): Promise<Set<string>> {
    const wanted = [...new Set(values.map(filterPath))];
    const parents = [...new Set(wanted.flatMap((path) => parentOf(path) ?? []))];
    const lists = await Promise.all(parents.map((dir) => treeList(dir).catch(() => [])));
    const found = new Map(lists.flat().map((entry) => [entry.path, entry.dir]));
    found.forEach((dir, path) => dirs.set(path, dir));
    return new Set(wanted.filter((path) => path === ROOT || found.has(path)));
  }

  onMount(() => {
    void (async () => {
      try {
        status = statusIndex(await repoStatus(true));
      } catch {
        // Without the status every entry shows.
      }
      const initial = await existing(paths);
      if (disposed) return;
      selected = initial;
      ready = true;
      await tick();
      view?.focus();
    })();
    return () => {
      disposed = true;
    };
  });

  // An expanded folder that shows without a listing is read.
  $effect(() => {
    if (!ready) return;
    const unread = rows.filter(
      (row) => row.entry.dir && expanded.has(row.entry.path) && !listed.has(row.entry.path),
    );
    untrack(() => {
      for (const { entry } of unread) if (!loading.has(entry.path)) void load(entry.path);
    });
  });

  function setOpen(path: string, open: boolean) {
    if (open === expanded.has(path)) return;
    const next = new Set(expanded);
    if (open) next.add(path);
    else {
      next.delete(path);
      nav.collapsed(path);
    }
    expanded = next;
  }

  const toggle = (path: string) => (selected = toggleChecked(selected, path, MAX_SELECTED));

  function keydown(event: KeyboardEvent) {
    const plain = !event.metaKey && !event.ctrlKey && !event.altKey;
    // Space toggles every selected row.
    if (plain && event.key === " " && nav.query === "") {
      event.preventDefault();
      for (const row of nav.selectedRows()) toggle(row.entry.path);
      return;
    }
    const used = nav.keydown(event, {
      isOpen: (path) => expanded.has(path),
      setOpen,
      enter: () => {
        if (selected.size > 0) finish();
      },
    });
    if (used) event.preventDefault();
  }

  function finish() {
    if (selected.size === 0) return;
    onok(pathFilterValues(selected, (path) => dirs.get(path) ?? false));
  }
</script>

<Dialog title={TITLE} width={400} {onclose} onsubmit={finish}>
  <div class="tree">
    <TreeView
      bind:this={view}
      {rows}
      {nav}
      label={TITLE}
      idPrefix="structure"
      isOpen={(path) => expanded.has(path)}
      isEmpty={(path) => listed.get(path)?.length === 0}
      onkeydown={keydown}
      ondblclick={(row) => row.entry.dir && setOpen(row.entry.path, !expanded.has(row.entry.path))}
      oncontextmenu={() => {}}
      ontoggle={(entry) => setOpen(entry.path, !expanded.has(entry.path))}
    >
      {#snippet before(entry)}
        {@const state = checkState(selected, entry.path)}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <span
          class="check"
          class:on={state !== "clear"}
          role="checkbox"
          tabindex="-1"
          aria-label={entry.name}
          aria-checked={state !== "clear"}
          aria-disabled={state === "above" || state === "below"}
          onclick={() => toggle(entry.path)}
        ></span>
      {/snippet}
      {#snippet after(entry)}
        {#if entry.path === ROOT && rootPath}
          <span class="parent">{rootPath}</span>
        {/if}
      {/snippet}
    </TreeView>
  </div>
  <p class="selected">
    {#if full}Selected: <span class="error">{LIMIT_TEXT}</span>{/if}
  </p>
  {#snippet footer()}
    <button type="button" class="button" onclick={onclose}>Cancel</button>
    <button type="submit" class="button default" disabled={selected.size === 0}>OK</button>
  {/snippet}
</Dialog>

<style>
  .tree {
    display: flex;
    height: 300px;
    border: 1px solid var(--divider);
  }

  .check {
    display: inline-block;
    flex: none;
    width: 14px;
    height: 14px;
    border: 1px solid var(--control-border);
    border-radius: 3px;
  }

  .check.on {
    background: var(--focus);
    border-color: var(--focus);
  }

  .check.on::after {
    display: block;
    width: 4px;
    height: 8px;
    margin: 1px 0 0 4px;
    content: "";
    border: solid var(--accent-text);
    border-width: 0 2px 2px 0;
    transform: rotate(45deg);
  }

  .check[aria-disabled="true"] {
    opacity: 0.5;
  }

  .parent {
    color: var(--text-secondary);
  }

  .selected {
    min-height: 18px;
    margin: 2px 0 0;
  }

  .error {
    color: var(--error);
  }
</style>
