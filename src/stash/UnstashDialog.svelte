<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import type { StashEntry } from "../lib/api";
  import type { BranchNames } from "../branches/refName";
  import Dialog from "../lib/Dialog.svelte";
  import ConfirmDialog from "../remotes/ConfirmDialog.svelte";
  import type { SavedChange } from "./savedPatches";
  import StashViewDialog from "./StashViewDialog.svelte";
  import {
    clearConfirmation,
    dropConfirmation,
    unstashBranchProblem,
    type UnstashActions,
    type UnstashChoice,
  } from "./flow";

  // `GitUnstashDialog` and its `.form`, which `Unstash Changes…` opens when the Stash tab has nothing
  // to show. Drop and Clear ask first (yes/no) and reload the list; OK runs the unstash,
  // which it runs itself and which closes it only once completed. Stash messages are the repository's:
  // they are shown as text.
  interface Props extends UnstashActions {
    root: string;
    branch: string;
    names: BranchNames;
    onanswer: (choice: UnstashChoice | null) => void;
  }

  let {
    root,
    branch: current,
    names,
    load,
    drop,
    clear,
    view: viewOf,
    viewDiff,
    unstash,
    onanswer,
  }: Props = $props();

  let stashes = $state.raw<StashEntry[]>([]);
  let selected = $state<number>(-1);
  let branch = $state("");
  let pop = $state(false);
  let reinstateIndex = $state(false);
  let busy = $state(false);
  let confirm = $state.raw<{ title: string; message: string; run: () => Promise<void> } | null>(
    null,
  );
  let viewing = $state.raw<{ title: string; entry: StashEntry; changes: SavedChange[] } | null>(null);

  const hasBranch = $derived(branch !== "");
  const entry = $derived(stashes[selected] ?? null);
  const problem = $derived(hasBranch ? unstashBranchProblem(branch, names) : null);
  const ok = $derived(hasBranch ? "Branch" : pop ? "Pop Stash" : "Apply Stash");
  const label = (stash: StashEntry) => `stash@{${stash.index}}`;

  // `refreshStashList`: the first one is selected.
  async function refresh() {
    busy = true;
    try {
      stashes = await load();
      selected = stashes.length > 0 ? 0 : -1;
    } finally {
      busy = false;
    }
  }

  // The Conflicts dialog of an unstash takes its place: what it answers afterwards is nobody's.
  let destroyed = false;

  onMount(() => {
    void refresh();
  });

  onDestroy(() => (destroyed = true));

  // `updateDialogState`: with a branch both options are on and fixed, and they go back off without it.
  function typed(event: Event & { currentTarget: HTMLInputElement }) {
    const had = hasBranch;
    branch = event.currentTarget.value;
    if (branch !== "") {
      pop = true;
      reinstateIndex = true;
    } else if (had) {
      pop = false;
      reinstateIndex = false;
    }
  }

  function ask(texts: { title: string; message: string }, run: () => Promise<void>) {
    confirm = { ...texts, run };
  }

  async function answered(yes: boolean) {
    const asked = confirm;
    confirm = null;
    if (!yes || !asked) return;
    await asked.run();
    await refresh();
  }

  // `unstash.view.dialog.title`. A load error has already been notified: nothing opens.
  async function view() {
    if (!entry) return;
    const shown = entry;
    const changes = await viewOf(shown);
    if (changes && !destroyed) {
      viewing = { title: `Paths Affected by ${label(shown)}`, entry: shown, changes };
    }
  }

  function keydown(event: KeyboardEvent) {
    if (stashes.length === 0) return;
    if (event.key === "ArrowDown") selected = Math.min(selected + 1, stashes.length - 1);
    else if (event.key === "ArrowUp") selected = Math.max(selected - 1, 0);
    else return;
    event.preventDefault();
  }

  // `doOKAction`: it closes only if the unstash completed; otherwise the choice stays as it was.
  async function submit() {
    if (!entry || problem || busy) return;
    const choice = { entry, branch, pop, reinstateIndex };
    busy = true;
    try {
      if ((await unstash(choice)) && !destroyed) onanswer(choice);
    } finally {
      busy = false;
    }
  }
</script>

<Dialog title="Unstash Changes" width={560} onclose={() => onanswer(null)} onsubmit={submit}>
  <div class="rows">
    <span>Git Root:</span>
    <span class="value" title="Select Git VCS root">{root}</span>
    <span>Current Branch:</span>
    <span class="value" title="The currently checked-out branch.">{current}</span>
  </div>
  <span id="unstash-stashes" class="caption">Stashes:</span>
  <div class="stashes">
    <div
      class="list"
      role="listbox"
      aria-labelledby="unstash-stashes"
      tabindex="0"
      data-autofocus
      onkeydown={keydown}
    >
      {#each stashes as stash, index (stash.sha)}
        <div
          class="row"
          role="option"
          tabindex="-1"
          aria-selected={index === selected}
          onclick={() => (selected = index)}
          onkeydown={keydown}
        >
          <b><tt>{label(stash)}</tt></b>:
          {#if stash.prefix !== null}<i>{stash.prefix}</i>:{/if}
          {stash.message}
        </div>
      {/each}
    </div>
    <div class="buttons">
      <button
        type="button"
        class="button"
        title="View selected stash"
        disabled={!entry}
        onclick={view}>View</button
      >
      <button
        type="button"
        class="button"
        title="Delete selected stash"
        disabled={!entry}
        onclick={() => entry && ask(dropConfirmation(entry), () => drop(entry))}>Drop</button
      >
      <button
        type="button"
        class="button"
        title="Delete all stashes in the repository."
        disabled={stashes.length === 0}
        onclick={() => ask(clearConfirmation(), clear)}>Clear</button
      >
    </div>
  </div>
  <div class="rows">
    <label for="unstash-branch">As new branch:</label>
    <input
      id="unstash-branch"
      class="input"
      title="If non-empty name is entered, the stash is checked out as a new branch."
      value={branch}
      oninput={typed}
    />
  </div>
  {#if problem}
    <p class="field-error">{problem}</p>
  {/if}
  <div class="options">
    <label
      class="checkbox-label"
      class:disabled={hasBranch}
      title="If selected the stash is dropped after it is applied."
    >
      <input type="checkbox" disabled={hasBranch} bind:checked={pop} />
      <span class="check" data-state={pop ? "on" : "off"}></span>
      Pop stash
    </label>
    <label
      class="checkbox-label"
      class:disabled={hasBranch}
      title="Attempt to reinstate index as well as working tree changes."
    >
      <input type="checkbox" disabled={hasBranch} bind:checked={reinstateIndex} />
      <span class="check" data-state={reinstateIndex ? "on" : "off"}></span>
      Reinstate index
    </label>
  </div>
  {#snippet footer()}
    <button type="button" class="button" onclick={() => onanswer(null)}>Close</button>
    <button class="button default" disabled={!entry || problem !== null || busy}>{ok}</button>
  {/snippet}
</Dialog>

{#if confirm}
  <ConfirmDialog title={confirm.title} message={confirm.message} onanswer={answered} />
{/if}
{#if viewing}
  {@const shown = viewing}
  <StashViewDialog
    title={shown.title}
    changes={shown.changes}
    load={(change) => viewDiff(shown.entry, change)}
    onclose={() => (viewing = null)}
  />
{/if}

<style>
  .rows {
    display: grid;
    grid-template-columns: 110px 1fr;
    gap: 8px 10px;
    align-items: center;
    margin-bottom: 10px;
  }

  .value {
    overflow-wrap: anywhere;
    user-select: text;
  }

  .caption {
    display: block;
    margin-bottom: 6px;
  }

  .stashes {
    display: flex;
    gap: 8px;
    margin-bottom: 10px;
  }

  .list {
    flex: 1;
    height: 160px;
    overflow: auto;
    background: var(--background);
    border: 1px solid var(--popup-border);
    border-radius: 4px;
  }

  .row {
    padding: 0 6px;
    line-height: 22px;
    white-space: nowrap;
    cursor: default;
  }

  .row[aria-selected="true"] {
    background: var(--selection);
  }

  .buttons {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .options {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
</style>
