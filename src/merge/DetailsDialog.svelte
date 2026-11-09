<script lang="ts" module>
  import type { RangeCommit } from "../lib/api";

  // `MultipleCommitInfoDialog.setFilter`: the commits that pass, and the selection that stays, or
  // else the first row.
  export function filteredCommits(
    commits: RangeCommit[],
    onlyConflicted: boolean,
    selected: string[],
  ): { shown: RangeCommit[]; selected: string[] } {
    const shown = onlyConflicted ? commits.filter((commit) => commit.conflicted) : commits;
    const kept = shown.filter((commit) => selected.includes(commit.hash)).map((commit) => commit.hash);
    return { shown, selected: kept.length > 0 ? kept : shown.slice(0, 1).map((commit) => commit.hash) };
  }
</script>

<script lang="ts">
  import { onMount } from "svelte";
  import {
    commitDetails,
    mergeRangeCommits,
    taskCancel,
    type CommitDetails,
    type SideDetails,
  } from "../lib/api";
  import Dialog from "../lib/Dialog.svelte";
  import { errorText } from "../lib/errors";
  import { speedMatch } from "../lib/speedSearch";
  import { taskResult } from "../lib/tasks";
  import type { LogChange } from "../log/changes";
  import ChangesPanel from "../log/ChangesPanel.svelte";
  import { namePieces } from "../tree/model";
  import DetailsPanel from "../log/DetailsPanel.svelte";
  import { defaultOptions, type LogOptions } from "../log/options";

  // `GitDefaultMergeDialogCustomizer`: B:1291, B:1293; `close.action.name` of the single commit's
  // `ChangeListViewerDialog`.
  const FILTER = "Filter by conflicted file";
  const COLLECTING = "Collecting Commits Details…";
  const WIDTH = 600;
  const DIFF_WIDTH = 900;

  interface Props {
    // The side's title, which the dialog takes.
    title: string;
    details: SideDetails;
    // The conflicted file: preselected in a commit's changes, and what the range is filtered by.
    path: string;
    onclose: () => void;
  }

  let { title, details, path, onclose }: Props = $props();

  let options = $state<LogOptions>(defaultOptions());
  let failure = $state<string | null>(null);
  // A single commit: its message under its changes.
  let message = $state<CommitDetails | null>(null);
  // A range: its commits, the selected ones, and the filter.
  let commits = $state.raw<RangeCommit[] | null>(null);
  let selected = $state.raw<string[]>([]);
  let onlyConflicted = $state(true);
  // `ListSpeedSearch` on the subjects, and the change whose diff is open (a double click).
  let query = $state("");
  let diff = $state.raw<LogChange | null>(null);
  let task: number | null = null;

  const view = $derived(filteredCommits(commits ?? [], onlyConflicted, selected));

  function setFilter(on: boolean) {
    onlyConflicted = on;
    selected = filteredCommits(commits ?? [], on, selected).selected;
  }

  function select(hash: string, event: MouseEvent) {
    if (event.metaKey || event.ctrlKey) {
      selected = selected.includes(hash) ? selected.filter((other) => other !== hash) : [...selected, hash];
    } else if (event.shiftKey && selected.length > 0) {
      const hashes = view.shown.map((commit) => commit.hash);
      const [from, to] = [hashes.indexOf(selected[0]), hashes.indexOf(hash)];
      selected = hashes.slice(Math.min(from, to), Math.max(from, to) + 1);
    } else selected = [hash];
  }

  const matches = (commit: RangeCommit, text: string) => speedMatch(commit.subject, text.trim()) !== null;

  // `SpeedSearchBase.findElement`: the first subject that matches, from the selected one on.
  function search(text: string) {
    query = text;
    if (!text.trim()) return;
    const shown = view.shown;
    const start = Math.max(0, shown.findIndex((commit) => commit.hash === selected.at(-1)));
    const found = [...shown.slice(start), ...shown.slice(0, start)].find((commit) => matches(commit, text));
    if (found) selected = [found.hash];
  }

  function keydown(event: KeyboardEvent) {
    const shown = view.shown;
    const at = shown.findIndex((commit) => commit.hash === selected.at(-1));
    const plain = !event.metaKey && !event.ctrlKey && !event.altKey;
    if (query && event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      query = "";
      return;
    }
    if (query && plain && event.key === "Backspace") {
      event.preventDefault();
      search(query.slice(0, -1));
      return;
    }
    if (plain && event.key.length === 1 && (query || event.key !== " ")) {
      event.preventDefault();
      search(query + event.key);
      return;
    }
    const step = event.key === "ArrowDown" ? 1 : event.key === "ArrowUp" ? -1 : 0;
    if (step === 0) return;
    event.preventDefault();
    // While searching, the arrows go between the matches.
    const candidates = query ? shown.filter((commit) => matches(commit, query)) : shown;
    const ordered = step > 0 ? shown.slice(at + 1) : shown.slice(0, Math.max(0, at)).reverse();
    const next = ordered.find((commit) => candidates.includes(commit));
    if (next) selected = [next.hash];
  }

  onMount(() => {
    let closed = false;
    void (async () => {
      try {
        if (details.kind === "commit") {
          const [found] = await commitDetails([details.hash]);
          if (!closed) message = found ?? null;
          return;
        }
        task = await mergeRangeCommits(details.from, details.to, path);
        const found = await taskResult<RangeCommit[]>(task);
        task = null;
        if (closed) return;
        commits = found;
        selected = filteredCommits(found, onlyConflicted, []).selected;
      } catch (error) {
        task = null;
        if (!closed) failure = errorText(error);
      }
    })();
    return () => {
      closed = true;
      if (task !== null) taskCancel(task).catch(() => {});
    };
  });
</script>

<Dialog {title} width={WIDTH} {onclose} onsubmit={onclose}>
  {#if failure}
    <p class="status error" role="alert">{failure}</p>
  {:else if details.kind === "commit"}
    <div class="single">
      <div class="changes single-changes">
        <ChangesPanel
          hashes={[details.hash]}
          paths={[]}
          {options}
          onoptions={(next) => (options = { ...options, ...next })}
          onselect={() => {}}
          onopen={(change) => (diff = change)}
          preselect={path}
          modal
        />
      </div>
      <div class="message" aria-label="Commit message">
        {#if message}{message.subject}{#if message.body}{"\n\n"}{message.body}{/if}{/if}
      </div>
    </div>
  {:else if commits === null}
    <p class="status" role="status">{COLLECTING}</p>
  {:else}
    <div class="range">
      <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
      <div
        class="commits"
        role="listbox"
        aria-label="Commits"
        aria-multiselectable="true"
        tabindex="0"
        data-autofocus
        onkeydown={keydown}
        onblur={() => (query = "")}
      >
        {#if query}
          <div class="query" role="status">{query}</div>
        {/if}
        {#each view.shown as commit (commit.hash)}
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <div
            class="commit"
            class:current={selected.includes(commit.hash)}
            role="option"
            aria-selected={selected.includes(commit.hash)}
            tabindex="-1"
            onclick={(event) => select(commit.hash, event)}
          >
            {#each namePieces(commit.subject, query.trim()) as piece, index (index)}
              {#if piece.hit}<span class="hit">{piece.text}</span>{:else}{piece.text}{/if}
            {/each}
          </div>
        {/each}
      </div>
      <div class="side">
        <div class="changes">
          <ChangesPanel
            hashes={view.selected}
            paths={[]}
            {options}
            onoptions={(next) => (options = { ...options, ...next })}
            onselect={() => {}}
            onopen={(change) => (diff = change)}
            modal
          />
        </div>
        <div class="details">
          <DetailsPanel hashes={view.selected} generation={0} showBranches={false} onnavigate={() => {}} />
        </div>
      </div>
    </div>
    <label class="filter">
      <input type="checkbox" checked={onlyConflicted} onchange={(event) => setFilter(event.currentTarget.checked)} />
      {FILTER}
    </label>
  {/if}
  {#snippet footer()}
    {#if details.kind === "commit"}
      <button type="submit" class="button default">Close</button>
    {:else}
      <button type="submit" class="button default">OK</button>
    {/if}
  {/snippet}
</Dialog>

{#if diff}
  {@const shown = diff}
  <!-- The changes browser's diff of a double click, over the dialog. -->
  <Dialog title={shown.file.path} width={DIFF_WIDTH} onclose={() => (diff = null)} onsubmit={() => (diff = null)}>
    <div class="diff">
      {#await import("../diff/DiffView.svelte") then { default: DiffView }}
        <DiffView hash={shown.hash} file={shown.file} base={shown.base} />
      {/await}
    </div>
    {#snippet footer()}
      <button type="submit" class="button default">Close</button>
    {/snippet}
  </Dialog>
{/if}

<style>
  .single,
  .range {
    display: flex;
    height: 400px;
    border: 1px solid var(--divider);
  }

  .single {
    flex-direction: column;
  }

  .changes {
    display: flex;
    flex-direction: column;
    min-height: 0;
  }

  /* `LoadingCommittedChangeListPanel`: the changes take 0.8. */
  .single-changes {
    flex: 0.8;
  }

  .message {
    flex: 0.2;
    min-height: 0;
    padding: 6px 8px;
    overflow: auto;
    white-space: pre-wrap;
    border-top: 1px solid var(--divider);
  }

  /* `Vcs.Multiple.Commit.Info.Dialog.Changes.Splitter`: 0.5; the changes take 0.67 of the side. */
  .commits {
    position: relative;
    flex: 0.5;
    min-width: 0;
    padding-top: 10px;
    overflow: auto;
    outline: none;
    border-right: 1px solid var(--divider);
  }

  .commit {
    padding: 0 8px 0 20px;
    overflow: hidden;
    line-height: 24px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .commit.current {
    background: var(--selection-inactive);
  }

  .commits:focus .commit.current {
    background: var(--selection);
  }

  .side {
    display: flex;
    flex: 0.5;
    flex-direction: column;
    min-width: 0;
  }

  .side .changes {
    flex: 0.67;
  }

  .details {
    flex: 0.33;
    min-height: 0;
    overflow: auto;
    border-top: 1px solid var(--divider);
  }

  .query {
    position: absolute;
    top: 0;
    left: 8px;
    z-index: 1;
    padding: 0 6px;
    color: var(--text);
    background: var(--panel);
    border: 1px solid var(--focus);
    border-radius: 4px;
  }

  .hit {
    color: var(--accent-text);
    background: var(--accent);
    border-radius: 2px;
  }

  .diff {
    display: flex;
    height: 500px;
    background: var(--editor);
  }

  .filter {
    display: flex;
    gap: 6px;
    align-items: center;
    margin-top: 8px;
  }

  .status {
    margin: 0;
    color: var(--text-secondary);
  }

  .error {
    color: var(--error);
  }
</style>
