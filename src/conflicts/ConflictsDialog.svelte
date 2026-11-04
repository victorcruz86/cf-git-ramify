<script lang="ts">
  import { onDestroy, onMount, untrack } from "svelte";
  import { rangeIds, reconcileSelection, sameIds, toggleId } from "../commit/selection";
  import { fileNodes, flattenTree, type TreeRow } from "../commit/tree";
  import {
    conflictsAccept,
    conflictsSession,
    onRepoChanged,
    type ApiError,
    type Conflict,
    type ConflictSession,
    type ConflictSide,
    type FileChange,
    type SideStatus,
  } from "../lib/api";
  import Dialog from "../lib/Dialog.svelte";
  import { errorText } from "../lib/errors";
  import Icon from "../lib/Icon.svelte";
  import { taskResult } from "../lib/tasks";
  import { mergeFiles } from "./flow";

  // `MultipleFileMergeDialog`: the files in conflict with the state of each side and three buttons.
  // The session already comes from the user's side (in a rebase, `Yours` is the branch being rebased): the
  // dialog inverts nothing, and `conflicts_accept` is the one that does.
  interface Props {
    session: ConflictSession;
    // It closes: with `Close`, with Escape, or only when no conflict is left.
    onanswer: () => void;
  }

  let { session: initial, onanswer }: Props = $props();

  // `VcsConfiguration.GROUP_MULTIFILE_MERGE_BY_DIRECTORY`.
  const GROUP_KEY = "conflicts.groupByDirectory";
  const STATUS: Record<SideStatus, string> = {
    modified: "Modified",
    deleted: "Deleted",
  };
  const SCOPE = "conflicts";

  function stored(): boolean {
    try {
      return localStorage.getItem(GROUP_KEY) === "true";
    } catch {
      return false;
    }
  }

  let session = $state.raw(untrack(() => initial));
  let byDirectory = $state(stored());
  let selected = $state.raw(new Set<string>());
  let lead = $state<string | null>(null);
  let anchor: string | null = null;
  let leadIndex = 0;
  let busy = $state(false);
  let merging = $state(false);
  let failure = $state<string | null>(null);
  let destroyed = false;
  let reads = 0;
  const abort = new AbortController();

  const changes = $derived(
    session.conflicts.map(
      ({ path }): FileChange => ({
        path,
        oldPath: null,
        state: "unmerged",
        staged: false,
        unstaged: true,
      }),
    ),
  );
  const rows = $derived(flattenTree(fileNodes(SCOPE, new Set(), changes, byDirectory), new Set()));
  const byPath = $derived(new Map(session.conflicts.map((entry) => [entry.path, entry])));
  // What is chosen, and what hangs from a chosen folder (`VcsTreeModelData.selected`).
  const paths = $derived([
    ...new Set(rows.filter((row) => selected.has(row.node.id)).flatMap((row) => row.node.paths)),
  ]);
  const bothDeleted = (entry: Conflict | undefined) =>
    entry?.yours === "deleted" && entry.theirs === "deleted";
  // `GitMergeHandler.canResolveConflict`: with neither of the two sides there is nothing to merge.
  const mergeable = $derived(
    paths.length > 0 && !paths.some((path) => bothDeleted(byPath.get(path))),
  );

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

  // `selectFirstFile`.
  onMount(() => {
    const first = untrack(() => rows[0]);
    if (first && !lead) pick(first);
    const stop = onRepoChanged(() => void reload());
    return () => void stop.then((unlisten) => unlisten());
  });

  onDestroy(() => {
    destroyed = true;
    abort.abort();
  });

  function pick(row: TreeRow) {
    selected = new Set([row.node.id]);
    lead = row.node.id;
    anchor = row.node.id;
    leadIndex = rows.indexOf(row);
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
    } else {
      pick(row);
      return;
    }
    leadIndex = rows.indexOf(row);
  }

  function moveTo(index: number, extend: boolean) {
    const row = rows[Math.max(0, Math.min(index, rows.length - 1))];
    if (!row) return;
    if (extend) {
      selected = new Set(rangeIds(rows, anchor ?? lead, row.node.id));
      lead = row.node.id;
      leadIndex = rows.indexOf(row);
    } else pick(row);
  }

  function keydown(event: KeyboardEvent) {
    if (event.metaKey || event.ctrlKey || event.altKey) return;
    const index = rows.findIndex((row) => row.node.id === lead);
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
      // The default button: a table does not submit the form with Enter.
      case "Enter":
        void merge();
        break;
      default:
        return;
    }
    event.preventDefault();
  }

  function toggleGroup(event: Event & { currentTarget: HTMLInputElement }) {
    byDirectory = event.currentTarget.checked;
    try {
      localStorage.setItem(GROUP_KEY, String(byDirectory));
    } catch {
      // Without storage, only the memory between sessions is lost.
    }
  }

  // `updateModelFromFiles`: with no conflicts the dialog closes by itself; with them, the selection follows
  // the row that takes its place. A read that arrives late, behind another one, is discarded.
  async function reload() {
    if (destroyed) return;
    const token = ++reads;
    let next: ConflictSession;
    try {
      next = await taskResult<ConflictSession>(await conflictsSession());
    } catch (error) {
      if (!destroyed && token === reads && (error as ApiError).kind !== "cancelled") {
        failure = errorText(error);
      }
      return;
    }
    if (destroyed || token !== reads) return;
    session = next;
    if (next.conflicts.length === 0) onanswer();
  }

  async function accept(side: ConflictSide) {
    if (paths.length === 0 || busy || merging) return;
    busy = true;
    failure = null;
    try {
      await taskResult<void>(await conflictsAccept(paths, side));
    } catch (error) {
      if ((error as ApiError).kind !== "cancelled") {
        failure = `Error saving merged data: ${errorText(error)}`;
      }
    } finally {
      busy = false;
    }
    await reload();
  }

  // A single merge window (decision 3): the chosen files are opened one at a time, and when
  // each one finishes (`merge-done`) the session is read again.
  async function merge() {
    if (!mergeable || busy || merging) return;
    merging = true;
    failure = null;
    try {
      await mergeFiles(paths, {
        signal: abort.signal,
        skip: (path) => !byPath.has(path),
        ondone: reload,
      });
    } catch (error) {
      if (!destroyed) failure = `Error loading revisions to merge: ${errorText(error)}`;
      merging = false;
      await reload();
      return;
    }
    merging = false;
  }

  function open(row: TreeRow) {
    if (row.node.kind !== "file") return;
    if (!selected.has(row.node.id)) pick(row);
    void merge();
  }
</script>

<Dialog title="Conflicts" width={760} onclose={onanswer} onsubmit={merge}>
  <p class="description">{session.description}</p>
  <div class="layout">
    <div
      class="table"
      role="treegrid"
      tabindex="0"
      data-autofocus
      aria-label="Conflicts"
      aria-multiselectable="true"
      aria-activedescendant={lead ? `conflict-${lead}` : undefined}
      onkeydown={keydown}
    >
      <div class="head" role="row">
        <span class="cell name" role="columnheader">Name</span>
        <span class="cell status" role="columnheader">{session.yoursColumn}</span>
        <span class="cell status" role="columnheader">{session.theirsColumn}</span>
      </div>
      {#each rows as row (row.node.id)}
        {@const node = row.node}
        {@const entry = node.change ? byPath.get(node.change.path) : undefined}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <div
          id="conflict-{node.id}"
          class="row"
          class:selected={selected.has(node.id)}
          role="row"
          tabindex="-1"
          aria-level={row.depth + 1}
          aria-selected={selected.has(node.id)}
          onclick={(event) => click(event, row)}
          ondblclick={() => open(row)}
        >
          <span class="cell name" role="gridcell" style:padding-left="{row.depth * 16}px">
            <Icon name={node.kind === "directory" ? "folder" : "text"} />
            <span class="file">{node.name}</span>
            {#if node.location}
              <span class="muted">{node.location}</span>
            {/if}
          </span>
          <span class="cell status" role="gridcell">
            {entry ? STATUS[entry.yours] : ""}
          </span>
          <span class="cell status" role="gridcell">
            {entry ? STATUS[entry.theirs] : ""}
          </span>
        </div>
      {/each}
    </div>
    <div class="buttons">
      <button
        type="button"
        class="button"
        disabled={paths.length === 0 || busy || merging}
        onclick={() => accept("yours")}
      >
        Accept Yours
      </button>
      <button
        type="button"
        class="button"
        disabled={paths.length === 0 || busy || merging}
        onclick={() => accept("theirs")}
      >
        Accept Theirs
      </button>
      <button class="button default" disabled={!mergeable || busy || merging}>Merge…</button>
    </div>
  </div>
  <label class="group">
    <input type="checkbox" checked={byDirectory} onchange={toggleGroup} />
    Group files by directory
  </label>
  {#if failure}
    <p class="failure" role="alert">{failure}</p>
  {/if}
  {#snippet footer()}
    <button type="button" class="button" onclick={onanswer}>Close</button>
  {/snippet}
</Dialog>

<style>
  .description {
    margin: 0 0 8px;
    overflow-wrap: anywhere;
  }

  .layout {
    display: flex;
    gap: 12px;
    align-items: flex-start;
  }

  .table {
    flex: 1;
    min-width: 0;
    height: 300px;
    overflow: auto;
    outline: none;
    user-select: none;
    background: var(--editor);
    border: 1px solid var(--popup-border);
    border-radius: 4px;
  }

  .head,
  .row {
    display: flex;
    align-items: center;
    height: 24px;
    white-space: nowrap;
  }

  .head {
    position: sticky;
    top: 0;
    z-index: 1;
    color: var(--muted);
    background: var(--panel);
    border-bottom: 1px solid var(--border);
  }

  .row {
    border-radius: 4px;
  }

  .row.selected {
    background: var(--selection-inactive);
  }

  .table:focus .row.selected {
    background: var(--selection);
  }

  .cell {
    display: flex;
    gap: 4px;
    align-items: center;
    min-width: 0;
    padding: 0 8px;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .cell.name {
    flex: 1;
  }

  .cell.status {
    flex: none;
    width: 110px;
  }

  .file {
    flex: none;
  }

  .muted {
    margin-left: 4px;
    overflow: hidden;
    color: var(--muted);
    text-overflow: ellipsis;
  }

  .buttons {
    display: flex;
    flex: none;
    flex-direction: column;
    gap: 8px;
    width: 116px;
  }

  .group {
    display: flex;
    gap: 6px;
    align-items: center;
    margin-top: 10px;
  }

  .failure {
    margin: 8px 0 0;
    color: var(--error);
    overflow-wrap: anywhere;
  }
</style>
