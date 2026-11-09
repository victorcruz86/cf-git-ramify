<script lang="ts">
  import { tick, untrack } from "svelte";
  import {
    diffWindowOpenRequest,
    LOCAL_REVISION,
    treeListAt,
    type RevisionPair,
    type TreeEntry,
  } from "../lib/api";
  import { errorText } from "../lib/errors";
  import { matchChangesBrowserShortcut } from "../lib/keymap";
  import Menu from "../lib/Menu.svelte";
  import type { MenuItem } from "../lib/menu";
  import { notify } from "../lib/notifications";
  import { ROOT, visibleRows, type TreeRow } from "../tree/model";
  import { TreeNavigation, type TreeKeyActions } from "../tree/navigation.svelte";
  import TreeView from "../tree/TreeView.svelte";
  import { openInViewer } from "../viewer/window";
  import { repositoryTab, repositoryTabTitle } from "./tab";

  // `ChangeDiffRequestProducer.getYourVersion`, the title of a `CurrentContentRevision`.
  const YOUR_VERSION = "Your version";
  // `DiffRequestFactoryImpl.DIFF_TITLE_RENAME_SEPARATOR`.
  const SEPARATOR = " -> ";

  interface Props {
    // The repository's folder: the local side of `Compare with Local`, and the tab's root name.
    repo: string;
    // The Repositories window is in front: the tree takes the focus.
    active?: boolean;
  }

  let { repo, active = false }: Props = $props();

  const tab = $derived($repositoryTab);

  // Each folder of the revision as it was read (it never changes) and the expanded ones.
  let listed = $state.raw(new Map<string, TreeEntry[]>());
  let expanded = $state.raw(new Set<string>([ROOT]));
  let menu = $state.raw<{ x: number; y: number } | null>(null);
  let view = $state<TreeView>();
  let disposed = false;
  // The tab the tree shows: a read for another one is dropped when it arrives.
  let shownId: number | null = null;
  // The reads in flight and the ones that failed, by tab and folder: each is read, and reported, once.
  const loading = new Set<string>();
  const failing = new Set<string>();

  // `FileTreeModel` with one root that is not shown: the rows start with its entries.
  const allRows = $derived(visibleRows("", listed, expanded));
  const rows = $derived(allRows.slice(1).map((row) => ({ entry: row.entry, depth: row.depth - 1 })));
  const indexOf = $derived(new Map(rows.map((row, index) => [row.entry.path, index])));
  // `RepositoryBrowserPanel` selects nothing when it opens.
  const nav = new TreeNavigation({
    rows: () => rows,
    indexOf: () => indexOf,
    reveal: (index) => view?.reveal(index),
    pageSize: () => view?.pageSize() ?? 1,
  });

  // A new tab starts as `RepositoryBrowserPanel` does: the root's entries, nothing expanded or selected.
  $effect(() => {
    const current = tab;
    untrack(() => {
      if (!current || current.id === shownId) return;
      shownId = current.id;
      listed = new Map();
      expanded = new Set([ROOT]);
      nav.clear();
      menu = null;
      view?.scrollToTop();
    });
  });

  // An expanded folder that shows without its entries is read (`GitDirectoryVirtualFile.cachedChildren`).
  $effect(() => {
    const unread = allRows.filter(
      (row) => row.entry.dir && expanded.has(row.entry.path) && !listed.has(row.entry.path),
    );
    untrack(() => unread.forEach((row) => void load(row.entry.path)));
  });

  // `repoToolWindow.activate`: each time the window or a tab comes to the front.
  $effect(() => {
    void tab?.id;
    if (active) void tick().then(() => view?.focus());
  });

  $effect(() => () => {
    disposed = true;
  });

  async function load(dir: string) {
    const current = tab;
    if (!current || current.id !== shownId) return;
    const key = `${current.id}:${dir}`;
    if (loading.has(key)) return;
    loading.add(key);
    try {
      const entries = await treeListAt(current.hash, dir);
      failing.delete(key);
      if (!disposed && shownId === current.id) listed = new Map(listed).set(dir, entries);
    } catch (failure) {
      if (!disposed && shownId === current.id && !failing.has(key)) {
        notify({ kind: "error", title: errorText(failure) });
      }
      failing.add(key);
    } finally {
      loading.delete(key);
    }
  }

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

  const toggle = (entry: TreeEntry) => setOpen(entry.path, !expanded.has(entry.path));

  // `getSelectedFiles`: in the order they were selected, the files only.
  const selectedFiles = () => nav.selectedInOrder().filter((entry) => !entry.dir);

  // The `OkAction` and `EditSource` (`OpenFileDescriptor` of each selected file): every selected file
  // at the revision, read-only. A folder opens nothing.
  function openFiles() {
    const hash = tab?.hash;
    const targets = selectedFiles().map((entry) => ({ path: entry.path, revision: hash ?? null, line: null }));
    if (hash && targets.length > 0) {
      openInViewer(targets).catch((failure) => notify({ kind: "error", title: errorText(failure) }));
    }
  }

  // `ChangeDiffRequestProducer.getRequestTitle` of the change from `RemoteFilePath(path)` (relative to
  // the repository, no parent at its root) to the local file (`DiffRequestFactoryImpl.getTitle`).
  function compareTitle(path: string): string {
    const slash = path.lastIndexOf("/");
    const name = path.slice(slash + 1);
    const local = `${repo}/${path}`;
    if (slash < 0) return `${path}${SEPARATOR}${local}`;
    return `${name} (${path.slice(0, slash)}${SEPARATOR}${local.slice(0, local.lastIndexOf("/"))})`;
  }

  // `DiffRepoWithLocalAction`: a change from each selected file at the revision to the local file,
  // all in one diff, the first one shown.
  function compareWithLocal() {
    const hash = tab?.hash;
    const files = selectedFiles();
    if (!hash || files.length === 0) return;
    const pairs: RevisionPair[] = files.map(({ path }) => ({
      left: { revision: hash, path, title: hash },
      right: { revision: LOCAL_REVISION, path, title: YOUR_VERSION },
    }));
    diffWindowOpenRequest({ kind: "revisions", files: pairs, title: compareTitle(files[0].path) }).catch(
      (failure) => notify({ kind: "error", title: errorText(failure) }),
    );
  }

  // `FileSystemTreeImpl`'s double click (`performEnterAction(false)`) opens the files; on a folder
  // only the tree's own double click acts, which expands or collapses it.
  function doubleClick(row: TreeRow) {
    if (row.entry.dir) {
      toggle(row.entry);
      return;
    }
    if (!nav.selected.has(row.entry.path)) nav.selectOnly(row.entry.path);
    openFiles();
  }

  function contextMenu(event: MouseEvent, row: TreeRow | null) {
    event.preventDefault();
    if (row && !nav.selected.has(row.entry.path)) nav.selectOnly(row.entry.path);
    menu = { x: event.clientX, y: event.clientY };
  }

  // `RepositoryBrowserPanel`'s popup: `EditSource` (`A:553`) and `Vcs.ShowDiffWithLocal` (`A:1636`),
  // each enabled with a file selected.
  function menuItems(): MenuItem[] {
    const none = selectedFiles().length === 0;
    return [
      { label: "Jump to Source", shortcut: "⌘↓", disabled: none, run: openFiles },
      { label: "Compare with Local", disabled: none, run: compareWithLocal },
    ];
  }

  function closeMenu() {
    menu = null;
    view?.focus();
  }

  const keyActions: TreeKeyActions = {
    isOpen: (path) => expanded.has(path),
    setOpen,
    // `performEnterAction(true)` acts on `getSelectionPath()`, the first selected path: a folder
    // expands or collapses, a file opens the selected files.
    enter: () => {
      const [first] = nav.selectedInOrder();
      if (first?.dir) toggle(first);
      else if (first) openFiles();
    },
  };

  function keydown(event: KeyboardEvent) {
    if (matchChangesBrowserShortcut(event) === "EditSource") openFiles();
    else if (!nav.keydown(event, keyActions)) return;
    event.preventDefault();
  }
</script>

<section class="repositories">
  {#if tab}
    {@const title = repositoryTabTitle(repo, tab.hash)}
    <div class="tabs" role="tablist" aria-label="Repositories">
      <span class="tab selected">
        <span role="tab" aria-selected="true" tabindex="-1" {title}>{title}</span>
      </span>
    </div>
    <TreeView
      bind:this={view}
      {rows}
      {nav}
      label="Repositories"
      idPrefix="repositories"
      isOpen={(path) => expanded.has(path)}
      isEmpty={(path) => listed.get(path)?.length === 0}
      onkeydown={keydown}
      ondblclick={doubleClick}
      oncontextmenu={contextMenu}
      ontoggle={toggle}
    />
  {/if}
</section>

{#if menu}
  <Menu x={menu.x} y={menu.y} label="Repositories" items={menuItems()} onclose={closeMenu} />
{/if}

<style>
  .repositories {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    background: var(--panel);
  }

  .tabs {
    display: flex;
    flex: none;
    align-items: center;
    height: 30px;
    padding: 0 8px;
    box-sizing: border-box;
    border-bottom: 1px solid var(--divider);
  }

  .tab {
    display: inline-flex;
    align-items: center;
    max-width: 100%;
    height: 100%;
    padding: 0 8px;
    overflow: hidden;
    color: var(--text);
    text-overflow: ellipsis;
    white-space: nowrap;
    border-bottom: 2px solid transparent;
  }

  .tab.selected {
    border-bottom-color: var(--focus);
  }

  .tab [role="tab"] {
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>
