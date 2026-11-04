<script lang="ts">
  import { onMount } from "svelte";
  import { snapshotsList, type Snapshot } from "../lib/api";
  import Dialog from "../lib/Dialog.svelte";
  import { errorText } from "../lib/errors";

  interface Props {
    onconfirm: (id: string) => void;
    onclose: () => void;
  }

  let { onconfirm, onclose }: Props = $props();

  const dateFormat = new Intl.DateTimeFormat(undefined, {
    dateStyle: "short",
    timeStyle: "short",
  });

  let snapshots = $state.raw<Snapshot[] | null>(null);
  let failure = $state<string | null>(null);
  let chosen = $state<string | null>(null);

  function fileCount(snapshot: Snapshot): string {
    return snapshot.entries.length === 1 ? "1 file" : `${snapshot.entries.length} files`;
  }

  function submit() {
    if (chosen) onconfirm(chosen);
  }

  onMount(async () => {
    try {
      snapshots = await snapshotsList();
    } catch (error) {
      failure = errorText(error);
    }
  });
</script>

<Dialog title="Saved Snapshots" {onclose} onsubmit={submit} width={520}>
  <p class="hint">Snapshots are saved before a rollback, a reset, a force checkout or before losing something from the stage, and are kept for 30 days.</p>
  {#if failure}
    <p class="field-error">{failure}</p>
  {:else if snapshots?.length === 0}
    <p class="hint">No saved snapshots.</p>
  {:else if snapshots}
    <div class="list" role="listbox" aria-label="Saved Snapshots">
      {#each snapshots as snapshot (snapshot.id)}
        <button
          type="button"
          role="option"
          aria-selected={chosen === snapshot.id}
          class:selected={chosen === snapshot.id}
          onclick={() => (chosen = snapshot.id)}
        >
          <span>{dateFormat.format(snapshot.createdMs)}</span>
          <span class="reason">{snapshot.reason}</span>
          <span class="count">{fileCount(snapshot)}</span>
        </button>
      {/each}
    </div>
  {/if}
  {#snippet footer()}
    <button type="button" class="button" onclick={onclose}>Close</button>
    <button class="button default" disabled={!chosen}>Restore</button>
  {/snippet}
</Dialog>

<style>
  .hint {
    margin: 0 0 8px;
    color: var(--text-secondary);
  }

  .list {
    display: flex;
    flex-direction: column;
    max-height: 280px;
    padding: 4px;
    overflow: auto;
    background: var(--editor);
    border: 1px solid var(--control-border);
    border-radius: 6px;
  }

  .list button {
    display: flex;
    flex: none;
    gap: 12px;
    align-items: center;
    height: 24px;
    padding: 0 8px;
    font: inherit;
    color: var(--text);
    white-space: nowrap;
    background: none;
    border: 0;
    border-radius: 6px;
  }

  .list button.selected {
    background: var(--selection);
  }

  .reason {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    text-align: left;
  }

  .count {
    color: var(--muted);
  }
</style>
