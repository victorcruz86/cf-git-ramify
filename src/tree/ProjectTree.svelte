<script lang="ts">
  import { onMount, tick, untrack } from "svelte";
  import { onRepoChanged, repoStatus, treeList, type TreeEntry } from "../lib/api";
  import { CANNOT_OPEN, hostingItems, OPEN_IN_GITHUB } from "../hosting/links";
  import { errorText } from "../lib/errors";
  import { matchProjectViewShortcut, matchShortcut } from "../lib/keymap";
  import Menu from "../lib/Menu.svelte";
  import type { MenuAction, MenuItem } from "../lib/menu";
  import type { FileTarget } from "../files/gitFileMenu";
  import { notify } from "../lib/notifications";
  import { openInViewer } from "../viewer/window";
  import {
    ROOT,
    collapsedAll,
    entryColor,
    fileTargets,
    hasLocalChanges,
    limiter,
    openFolders,
    parentOf,
    provideTreeSelection,
    stateOf,
    statusIndex,
    visibleRows,
    type StatusIndex,
    type TreeRow,
  } from "./model";
  import { TreeNavigation, type TreeKeyActions } from "./navigation.svelte";
  import TreeView from "./TreeView.svelte";

  // Folders read at once, whoever asks: an expand, Expand All or a change on disk.
  const LISTINGS_AT_ONCE = 16;

  interface Props {
    // The repository's folder: the root of the tree.
    root: string;
    // The Project window is in front: the tree takes the focus.
    active?: boolean;
    // `Git ▸` of the context menu for the selected entries (`gitSubmenu` of the Git file menu).
    gitMenu?: (files: FileTarget[]) => MenuAction;
    // The selection and the changes, each time either changes: what `Git > Current File` acts on.
    onfiles?: (files: FileTarget[], hasChanges: boolean) => void;
  }

  const NO_GIT_MENU = (): MenuAction => ({ label: "Git", submenu: [], disabled: true });

  let { root, active = false, gitMenu = NO_GIT_MENU, onfiles }: Props = $props();

  // Each folder as it was last read, the expanded ones, and the colours of the last `git status`.
  let listed = $state.raw(new Map<string, TreeEntry[]>());
  let expanded = $state.raw(new Set<string>([ROOT]));
  let status = $state.raw<StatusIndex>(statusIndex([]));
  let menu = $state.raw<{ x: number; y: number } | null>(null);
  let view = $state<TreeView>();
  let disposed = false;
  // Collapse All stops an Expand All that is still reading.
  let expandRun = 0;

  const rootName = $derived(root.split("/").filter(Boolean).pop() ?? root);
  const rows = $derived(visibleRows(rootName, listed, expanded));
  const indexOf = $derived(new Map(rows.map((row, index) => [row.entry.path, index])));
  // `TreeUtil.promiseSelectFirst`: the root is selected when the tree opens.
  const nav = new TreeNavigation(
    {
      rows: () => rows,
      indexOf: () => indexOf,
      reveal: (index) => view?.reveal(index),
      pageSize: () => view?.pageSize() ?? 1,
    },
    ROOT,
  );

  // What is no longer in sight (a collapsed folder, an entry deleted on disk) leaves the selection;
  // the lead goes up to the nearest folder that still shows.
  $effect(() => {
    const visible = indexOf;
    untrack(() => {
      const kept = [...nav.selected].filter((path) => visible.has(path));
      if (kept.length !== nav.selected.size) nav.selected = new Set(kept);
      if (nav.lead === null || visible.has(nav.lead)) return;
      let up = parentOf(nav.lead);
      while (up !== null && !visible.has(up)) up = parentOf(up);
      nav.lead = up;
      nav.anchor = up;
      if (up !== null && nav.selected.size === 0) nav.selected = new Set([up]);
    });
  });

  $effect(() => {
    if (active) void tick().then(() => view?.focus());
  });

  $effect(() => {
    const files = fileTargets(
      rows.filter((row) => nav.selected.has(row.entry.path)).map((row) => row.entry),
      status,
    );
    const changed = hasLocalChanges(status);
    untrack(() => onfiles?.(files, changed));
  });

  onMount(() => {
    const stopSelection = provideTreeSelection(() => nav.selectedRows().map((row) => row.entry.path));
    const stopListening = onRepoChanged(refresh);
    void load(ROOT);
    void readStatus();
    return () => {
      disposed = true;
      stopSelection();
      void stopListening.then((unlisten) => unlisten());
    };
  });

  function showError(failure: unknown) {
    notify({ kind: "error", title: errorText(failure) });
  }

  const limit = limiter(LISTINGS_AT_ONCE);
  // A folder that cannot be read is reported once, until it can be read again.
  const failing = new Set<string>();
  // The reads of an expand in flight, so that a folder is read once however many ask for it.
  const loading = new Map<string, Promise<TreeEntry[] | null>>();

  async function list(dir: string): Promise<TreeEntry[] | null> {
    try {
      const entries = await limit(() => treeList(dir));
      failing.delete(dir);
      return entries;
    } catch (failure) {
      if (!disposed && !failing.has(dir)) showError(failure);
      failing.add(dir);
      return null;
    }
  }

  function read(dir: string): Promise<TreeEntry[] | null> {
    let reading = loading.get(dir);
    if (!reading) {
      reading = list(dir).finally(() => loading.delete(dir));
      loading.set(dir, reading);
    }
    return reading;
  }

  async function load(dir: string) {
    const entries = await read(dir);
    if (entries && !disposed) listed = new Map(listed).set(dir, entries);
  }

  // An expanded folder that shows without a listing (first expand, or one dropped by a change on
  // disk while it was out of sight) is read.
  $effect(() => {
    const unread = rows.filter(
      (row) => row.entry.dir && expanded.has(row.entry.path) && !listed.has(row.entry.path),
    );
    untrack(() => unread.forEach((row) => void load(row.entry.path)));
  });

  // The colours stay as they were if git cannot say: the next change asks again.
  async function readStatus() {
    try {
      const changes = await repoStatus(true);
      if (!disposed) status = statusIndex(changes);
    } catch {
      // Nothing to show: the Commit view reports a repository git cannot read.
    }
  }

  // `repo-changed` says nothing about which paths: the colours and every folder that shows are
  // read again, at most `LISTINGS_AT_ONCE` at a time. The listings out of sight are dropped, so a
  // folder expanded later is read as it is then. The changes that arrive while that runs make it
  // run once more, all together.
  let refreshing = false;
  let refreshAgain = false;

  async function refresh() {
    if (refreshing) {
      refreshAgain = true;
      return;
    }
    refreshing = true;
    try {
      do {
        refreshAgain = false;
        const before = listed;
        const folders = openFolders(before, expanded);
        const [, lists] = await Promise.all([readStatus(), Promise.all(folders.map(list))]);
        if (disposed) return;
        const next = new Map<string, TreeEntry[]>();
        // What an expand read meanwhile is newer than this pass.
        listed.forEach((entries, dir) => {
          if (before.get(dir) !== entries) next.set(dir, entries);
        });
        folders.forEach((dir, index) => {
          // A folder that cannot be read keeps what it showed.
          const entries = lists[index] ?? listed.get(dir);
          if (entries) next.set(dir, entries);
        });
        listed = next;
      } while (refreshAgain);
    } finally {
      refreshing = false;
    }
  }

  const isUnder = (path: string, folder: string) =>
    folder === ROOT ? path !== ROOT : path.startsWith(`${folder}/`);

  function setOpen(path: string, open: boolean) {
    if (open === expanded.has(path)) return;
    const next = new Set(expanded);
    if (open) {
      next.add(path);
    } else {
      next.delete(path);
      // A lead that gets hidden leaves the selection on the folder that is collapsed.
      if (nav.lead !== null && isUnder(nav.lead, path)) nav.selectOnly(path);
    }
    expanded = next;
  }

  const toggle = (entry: TreeEntry) => setOpen(entry.path, !expanded.has(entry.path));

  // `TreeUtil.promiseExpandAll`: every folder, each read as it opens, a level at a time. A change on
  // disk meanwhile can drop a listing this used: the folder is expanded by then, and read again.
  async function expandAll() {
    const run = ++expandRun;
    let level = [ROOT];
    while (level.length > 0) {
      const known = level.map((dir) => listed.get(dir));
      const lists = await Promise.all(level.map((dir, index) => known[index] ?? read(dir)));
      if (disposed || run !== expandRun) return;
      const nextListed = new Map(listed);
      level.forEach((dir, index) => {
        if (!known[index]) nextListed.set(dir, lists[index] ?? []);
      });
      listed = nextListed;
      expanded = new Set([...expanded, ...level]);
      level = lists.flatMap((entries) =>
        (entries ?? []).filter((entry) => entry.dir).map((entry) => entry.path),
      );
    }
  }

  function collapseAll() {
    expandRun++;
    const next = collapsedAll(nav.lead);
    expanded = next.expanded;
    if (next.lead !== null) {
      nav.selectOnly(next.lead);
      revealPath(next.lead);
    }
  }

  const selectedEntries = () => nav.selectedRows().map((row) => row.entry);

  // `ProjectViewEditSource` and the double click: the files open in the viewer, the folders do not.
  function openFiles(entries: TreeEntry[]) {
    const targets = entries
      .filter((entry) => !entry.dir)
      .map((entry) => ({ path: entry.path, revision: null, line: null }));
    if (targets.length > 0) openInViewer(targets).catch(showError);
  }

  function revealPath(path: string) {
    void tick().then(() => view?.reveal(indexOf.get(path) ?? -1));
  }

  // `EditSourceOnDoubleClickHandler`: a file opens, a folder expands or collapses.
  function doubleClick(row: TreeRow) {
    if (row.entry.dir) toggle(row.entry);
    else openFiles([row.entry]);
  }

  // `Open In ▸ GitHub` of the first selected entry (`RevealGroup`): none while no GitHub repository
  // answers for it, as for the repository itself, an untracked file or an ignored one.
  let hosting = $state.raw<MenuItem[]>([]);

  async function contextMenu(event: MouseEvent, row: TreeRow | null) {
    event.preventDefault();
    if (row && !nav.selected.has(row.entry.path)) nav.selectOnly(row.entry.path);
    const [x, y] = [event.clientX, event.clientY];
    const [first] = selectedEntries();
    hosting = first
      ? await hostingItems({ kind: "file", path: first.path }, "open", OPEN_IN_GITHUB, (failure) =>
          notify({ kind: "error", title: CANNOT_OPEN, body: errorText(failure) }),
        )
      : [];
    menu = { x, y };
  }

  // `ProjectViewPopupMenu`, with what Ramify has of it: `Edit Source` (`LangActions.xml:844`) and,
  // in `VersionControlsGroup`, `Git ▸`.
  function menuItems(): MenuItem[] {
    const entries = selectedEntries();
    return [
      {
        label: "Edit Source",
        shortcut: "⌘↓",
        disabled: !entries.some((entry) => !entry.dir),
        run: () => openFiles(entries),
      },
      null,
      // `RevealGroup`, in `RunContextPopupGroup`, comes before the version control group.
      ...(hosting.length > 0 ? [{ label: "Open In", submenu: hosting }] : []),
      gitMenu(fileTargets(entries, status)),
    ];
  }

  function closeMenu() {
    menu = null;
    view?.focus();
  }

  const keyActions: TreeKeyActions = {
    isOpen: (path) => expanded.has(path),
    setOpen,
    // `EditSourceOnEnterKeyHandler`: a folder expands or collapses, the files open.
    enter: (row) => {
      if (row?.entry.dir) toggle(row.entry);
      else openFiles(selectedEntries());
    },
  };

  function keydown(event: KeyboardEvent) {
    const action = matchShortcut(event);
    if (matchProjectViewShortcut(event) === "ProjectViewEditSource") {
      openFiles(selectedEntries());
    } else if (action === "ExpandAll") {
      void expandAll();
    } else if (action === "CollapseAll") {
      collapseAll();
    } else if (!nav.keydown(event, keyActions)) {
      return;
    }
    event.preventDefault();
  }
</script>

<TreeView
  bind:this={view}
  {rows}
  {nav}
  label="Project"
  idPrefix="project"
  isOpen={(path) => expanded.has(path)}
  isEmpty={(path) => listed.get(path)?.length === 0}
  color={(entry) => entryColor(stateOf(status, entry.path))}
  strong={(entry) => entry.path === ROOT}
  onkeydown={keydown}
  ondblclick={doubleClick}
  oncontextmenu={contextMenu}
  ontoggle={toggle}
>
  {#snippet after(entry)}
    {#if entry.path === ROOT}
      <span class="muted">{root}</span>
    {/if}
  {/snippet}
</TreeView>

{#if menu}
  <Menu x={menu.x} y={menu.y} label="Project" items={menuItems()} onclose={closeMenu} />
{/if}

<style>
  .muted {
    min-width: 0;
    margin-left: 4px;
    overflow: hidden;
    color: var(--muted);
    text-overflow: ellipsis;
  }
</style>
