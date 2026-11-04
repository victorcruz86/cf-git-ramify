<script lang="ts">
  import { onDestroy } from "svelte";
  import {
    commitChangesOf,
    commitChangesStart,
    taskCancel,
    type ApiError,
    type SelectedChanges,
  } from "../lib/api";
  import { errorText } from "../lib/errors";
  import Icon from "../lib/Icon.svelte";
  import Menu, { type MenuItem } from "../lib/Menu.svelte";
  import { taskResult } from "../lib/tasks";
  import {
    affectsPaths,
    sectionOf,
    shortName,
    sizeText,
    pathsSelection,
    zipChanges,
    type ChangeSection,
    type LogChange,
  } from "./changes";
  import type { VcsAction } from "./contextMenu";
  import type { DiffPreviewAt, LogOptions } from "./options";

  const SHORT_HASH = 8;
  const SUBJECT_LENGTH = 50;
  const SELECT_COMMITS = "Select commit to view changes";
  const NO_MERGED = "No merged conflicts";
  const NO_AFFECTED = "No changes that affect selected paths.";
  const LOADING = "Loading Commit Changes";
  // `vcs.log.max.changes.shown` from the registry of the installed app.
  const MAX_CHANGES = 50_000;

  const STATUS_COLORS: Record<string, string> = {
    A: "var(--status-added)",
    D: "var(--status-deleted)",
    M: "var(--status-modified)",
    R: "var(--status-modified)",
  };

  interface Props {
    hashes: string[];
    // Those of the path filter: `Show Only Affected Changes` exists only with them.
    paths: string[];
    options: LogOptions;
    onoptions: (change: Partial<LogOptions>) => void;
    onselect: (change: LogChange | null) => void;
    onopen: (change: LogChange) => void;
    // The selection has already been read and has no file to show.
    onempty?: () => void;
    // Cherry-Pick Selected Changes and Revert Selected Changes. Without it they come out disabled.
    onvcs?: (action: VcsAction) => void;
  }

  let { hashes, paths, options, onoptions, onselect, onopen, onempty, onvcs }: Props = $props();

  let own = $state.raw<LogChange[]>([]);
  let parents = $state.raw<ChangeSection[]>([]);
  let merge = $state(false);
  let loaded = $state(false);
  let failure = $state<string | null>(null);
  let current = $state.raw<LogChange | null>(null);
  let menu = $state.raw<{ x: number; y: number } | null>(null);
  // The chosen files (`current` is one of them) and the files' context menu.
  let picked = $state.raw<Set<string>>(new Set());
  let fileMenu = $state.raw<{ x: number; y: number; changes: LogChange[] } | null>(null);
  let tooMany = $state.raw<{ commits: number; size: number } | null>(null);
  // The selection for which `Show anyway` was pressed.
  let anyway = $state.raw<string[] | null>(null);
  let request = 0;
  let running: number | null = null;

  // A prop is re-read on any change in the parent, and this launches git.
  const showParents = $derived(options.showChangesToParents);
  const unlimited = $derived(anyway === hashes);

  const onlyAffected = $derived(paths.length > 0 && options.showOnlyAffected);
  const keep = (changes: LogChange[]) =>
    onlyAffected ? changes.filter(({ file }) => affectsPaths(file, paths)) : changes;
  const shownOwn = $derived(keep(own));
  const shownParents = $derived(
    showParents
      ? parents.flatMap((section) => {
          const changes = keep(section.folders.flatMap((folder) => folder.changes));
          return changes.length > 0 ? [sectionOf(section.title, changes)] : [];
        })
      : [],
  );
  const sections = $derived([
    ...(shownOwn.length > 0 ? [sectionOf(null, shownOwn)] : []),
    ...shownParents,
  ]);
  const visible = $derived(
    sections.flatMap((section) => section.folders.flatMap((folder) => folder.changes)),
  );

  const same = (a: LogChange | null, b: LogChange) =>
    a?.file.path === b.file.path && a.hash === b.hash && a.base === b.base;

  const keyOf = (change: LogChange) => `${change.file.path}\0${change.base}\0${change.hash}`;
  const chosen = (changes: LogChange[]) => (picked = new Set(changes.map(keyOf)));

  function choose(change: LogChange | null) {
    current = change;
    onselect(change);
  }

  $effect(() => {
    const first = visible[0] ?? null;
    if (!visible.some((change) => same(current, change))) {
      choose(first);
      chosen(first ? [first] : []);
    } else if ([...picked].some((key) => !visible.some((change) => keyOf(change) === key))) {
      // What is no longer visible (another selection, a filter) stops being chosen.
      chosen(visible.filter((change) => picked.has(keyOf(change))));
    }
    if (loaded && !first) onempty?.();
  });

  // Shift chooses the range from the current file; Cmd or Ctrl adds or removes one.
  function click(event: MouseEvent, change: LogChange) {
    if (event.shiftKey && current) {
      const [from, to] = [current, change].map((item) =>
        visible.findIndex((candidate) => same(item, candidate)),
      );
      chosen(visible.slice(Math.min(from, to), Math.max(from, to) + 1));
      return;
    }
    if (event.metaKey || event.ctrlKey) {
      const key = keyOf(change);
      const next = new Set(picked);
      if (next.delete(key)) {
        picked = next;
        return;
      }
      picked = next.add(key);
    } else {
      chosen([change]);
    }
    choose(change);
  }

  // `Vcs.RepositoryChangesBrowserMenu`: on a file that is not chosen, it chooses only that one; on a
  // folder, it chooses its files.
  function contextMenu(event: MouseEvent, changes: LogChange[]) {
    event.preventDefault();
    event.stopPropagation();
    if (!changes.every((change) => picked.has(keyOf(change)))) {
      chosen(changes);
      if (!changes.some((change) => same(current, change))) choose(changes[0]);
    }
    fileMenu = {
      x: event.clientX,
      y: event.clientY,
      changes: visible.filter((change) => picked.has(keyOf(change))),
    };
  }

  // `RevertSelectedChangesAction.Revert` and `.Apply`.
  const fileItems = (changes: LogChange[]): MenuItem[] =>
    [
      ["Revert Selected Changes", true],
      ["Cherry-Pick Selected Changes", false],
    ].map(([label, reverse]) => ({
      label: label as string,
      disabled: !onvcs || changes.length === 0,
      run: () =>
        onvcs?.({
          kind: "applyPaths",
          reverse: reverse as boolean,
          selection: pathsSelection(changes),
          hashes,
        }),
    }));

  async function single(token: number, hash: string, withParents: boolean, all: boolean) {
    const [group] = await commitChangesOf(hash, "merged");
    if (token !== request) return;
    if (!all && group.size > MAX_CHANGES) {
      tooMany = { commits: 1, size: group.size };
      return;
    }
    own = group.files.map((file) => ({ file, hash, base: group.base }));
    merge = group.merge;
    if (!group.merge || !withParents) return;
    const groups = await commitChangesOf(hash, "parents");
    if (token !== request) return;
    parents = groups.map((parent) => {
      const title = [
        `Changes to ${parent.parent?.slice(0, SHORT_HASH)}`,
        parent.subject && shortened(parent.subject),
      ];
      const changes = parent.files.map((file) => ({ file, hash, base: parent.base }));
      return sectionOf(title.filter(Boolean).join(" "), changes);
    });
  }

  // `StringUtil.shortenTextWithEllipsis`, with the three dots inside the limit.
  const shortened = (text: string) =>
    text.length <= SUBJECT_LENGTH ? text : `${text.slice(0, SUBJECT_LENGTH - 3)}...`;

  function cancelRunning(id = running) {
    if (id !== null) taskCancel(id).catch(() => {});
    running = null;
  }

  async function several(token: number, selected: string[], all: boolean) {
    const id = await commitChangesStart(selected, all ? null : MAX_CHANGES);
    if (token !== request) {
      cancelRunning(id);
      return;
    }
    running = id;
    const { commits, maxSize } = await taskResult<SelectedChanges>(id);
    if (token !== request) return;
    running = null;
    if (!all && maxSize > MAX_CHANGES) tooMany = { commits: selected.length, size: maxSize };
    // `VcsLogUtil.collectChanges`: from oldest to newest.
    else own = zipChanges([...commits].reverse());
  }

  async function load(selected: string[], withParents: boolean, all: boolean) {
    const token = ++request;
    cancelRunning();
    failure = null;
    tooMany = null;
    loaded = false;
    // `setEmpty`: the previous selection does not stay in view while the new one arrives.
    own = [];
    parents = [];
    merge = false;
    if (selected.length === 0) return;
    try {
      if (selected.length === 1) await single(token, selected[0], withParents, all);
      else await several(token, selected, all);
      if (token === request) loaded = true;
    } catch (error) {
      if (token !== request) return;
      loaded = true;
      if ((error as ApiError)?.kind !== "cancelled") failure = errorText(error);
    }
  }

  $effect(() => {
    load(hashes, showParents, unlimited);
  });

  onDestroy(() => cancelRunning());

  function keydown(event: KeyboardEvent) {
    if (event.key !== "Enter" || !current) return;
    event.preventDefault();
    onopen(current);
  }

  function openMenu(event: MouseEvent) {
    const bounds = (event.currentTarget as HTMLElement).getBoundingClientRect();
    menu = { x: bounds.left, y: bounds.bottom };
  }

  const locations: [DiffPreviewAt, string][] = [
    ["bottom", "Bottom"],
    ["right", "Right"],
  ];

  // `Vcs.Log.LayoutConfiguration`, and `Vcs.Log.ShowChangesFromParents`.
  const menuItems = (): MenuItem[] => [
    {
      label: "Show Changes to Parents",
      checked: options.showChangesToParents,
      run: () => onoptions({ showChangesToParents: !options.showChangesToParents }),
    },
    { header: "Layout" },
    {
      label: "Show Details",
      checked: options.showDetails,
      run: () => onoptions({ showDetails: !options.showDetails }),
    },
    {
      label: "Show Diff Preview",
      checked: options.diffPreview,
      run: () => onoptions({ diffPreview: !options.diffPreview }),
    },
    {
      label: "Diff Preview Location",
      submenu: locations.map(([location, label]) => ({
        label,
        checked: options.diffPreviewAt === location,
        run: () => onoptions({ diffPreviewAt: location }),
      })),
    },
  ];
</script>

<section class="changes" aria-label="Changes">
  <div class="toolbar">
    {#if paths.length > 0}
      <button
        type="button"
        class="action"
        class:on={options.showOnlyAffected}
        aria-label="Show Only Affected Changes"
        aria-pressed={options.showOnlyAffected}
        title="Show Only Affected Changes"
        onclick={() => onoptions({ showOnlyAffected: !options.showOnlyAffected })}
      >
        <Icon name="show" />
      </button>
    {/if}
    <button type="button" class="action" aria-label="Changes options" onclick={openMenu}>
      <Icon name="settings" />
    </button>
  </div>

  <div
    class="tree"
    role="tree"
    aria-label="Changes"
    tabindex="0"
    onkeydown={keydown}
  >
    {#if failure}
      <p class="status">{failure}</p>
    {:else if hashes.length === 0}
      <p class="status">{SELECT_COMMITS}</p>
    {:else if tooMany}
      <p class="status">
        {tooMany.commits === 1 ? "This commit" : "One of the selected commits"} has
        {sizeText(tooMany.size)} changes
        <button type="button" class="link" onclick={() => (anyway = hashes)}>Show anyway</button>
      </p>
    {:else if !loaded}
      <p class="status loading" role="status" aria-label={LOADING}>{LOADING}…</p>
    {:else if own.length === 0 && merge && !showParents}
      <p class="status">
        {NO_MERGED}.
        <button
          type="button"
          class="link"
          onclick={() => onoptions({ showChangesToParents: true })}
        >Show changes to parents</button>
      </p>
    {:else if sections.length === 0 && onlyAffected}
      <p class="status">
        {NO_AFFECTED}
        <button
          type="button"
          class="link"
          onclick={() => onoptions({ showOnlyAffected: false })}
        >Show changes for all paths</button>
      </p>
    {:else}
      {#if merge && showParents && own.length === 0}
        <p class="title">{NO_MERGED}</p>
      {/if}
      {#each sections as section}
        {#if section.title}
          <p class="title">{section.title}</p>
        {/if}
        {#each section.folders as folder (folder.name)}
          {#if folder.name}
            <!-- svelte-ignore a11y_no_static_element_interactions -->
          <p class="folder" oncontextmenu={(event) => contextMenu(event, folder.changes)}>
            <Icon name="folder" size={14} />{folder.name}
          </p>
          {/if}
          {#each folder.changes as change (change.file.path + change.base)}
            {@const name = shortName(change.file.path)}
            <div
              role="treeitem"
              aria-label={name}
              aria-selected={picked.has(keyOf(change))}
              tabindex="-1"
              class="file"
              class:nested={folder.name}
              class:current={picked.has(keyOf(change))}
              style:color={STATUS_COLORS[change.file.status[0]]}
              title={change.file.path}
              onclick={(event) => click(event, change)}
              oncontextmenu={(event) => contextMenu(event, [change])}
              ondblclick={() => {
                chosen([change]);
                choose(change);
                onopen(change);
              }}
              onkeydown={keydown}
            >
              {name}
            </div>
          {/each}
        {/each}
      {/each}
    {/if}
  </div>
</section>

{#if menu}
  <Menu {...menu} label="Changes options" items={menuItems()} onclose={() => (menu = null)} />
{/if}

{#if fileMenu}
  <Menu
    x={fileMenu.x}
    y={fileMenu.y}
    label="Changes"
    items={fileItems(fileMenu.changes)}
    onclose={() => (fileMenu = null)}
  />
{/if}

<style>
  .changes {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: var(--panel);
  }

  .toolbar {
    display: flex;
    flex: none;
    gap: 2px;
    align-items: center;
    justify-content: flex-end;
    height: 36px;
    padding: 0 6px;
    box-sizing: border-box;
    border-bottom: 1px solid var(--divider);
  }

  .action.on {
    background: var(--selection);
  }

  .tree {
    flex: 1;
    min-height: 0;
    padding: 2px 0;
    overflow: auto;
    outline: none;
  }

  .status {
    padding: 16px;
    margin: 0;
    color: var(--text-secondary);
    text-align: center;
  }

  /* Normally it takes less time than a blink takes to be noticed. */
  .loading {
    opacity: 0;
    animation: appear 0s linear 300ms forwards;
  }

  @keyframes appear {
    to {
      opacity: 1;
    }
  }

  .title {
    padding: 4px 8px;
    margin: 0;
    color: var(--text-secondary);
  }

  .folder {
    display: flex;
    gap: 4px;
    align-items: center;
    padding: 2px 8px;
    margin: 0;
    overflow: hidden;
    color: var(--text-secondary);
    white-space: nowrap;
  }

  .file {
    padding: 2px 8px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .file.nested {
    padding-left: 26px;
  }

  .file.current {
    background: var(--selection);
  }

  .link {
    display: block;
    width: 100%;
    padding: 4px 0 0;
    font: inherit;
    color: var(--link);
    background: none;
    border: 0;
  }
</style>
