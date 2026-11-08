<script lang="ts">
  import { untrack } from "svelte";
  import Dialog from "../lib/Dialog.svelte";
  import Icon from "../lib/Icon.svelte";
  import SavedDiffPane from "./SavedDiffPane.svelte";
  import type { SavedChange, SavedDiff } from "./savedPatches";

  // `ChangesBrowserBase.showStandaloneDiff` of a saved patch: the chain of its changes, starting at
  // `start`, with the previous and next file.
  interface Props {
    changes: SavedChange[];
    start: number;
    titleOf: (change: SavedChange) => string;
    load: (change: SavedChange) => Promise<SavedDiff>;
    onclose: () => void;
  }

  let { changes, start, titleOf, load, onclose }: Props = $props();

  let index = $state(untrack(() => start));
  const change = $derived(changes[index]);
  const loader = $derived(change ? () => load(change) : null);
</script>

<Dialog title={change ? titleOf(change) : ""} width={960} {onclose}>
  <div class="bar">
    <button
      type="button"
      class="action"
      aria-label="Previous File"
      title="Previous File"
      disabled={index <= 0}
      onclick={() => (index -= 1)}
    >
      <Icon name="left" />
    </button>
    <button
      type="button"
      class="action"
      aria-label="Next File"
      title="Next File"
      disabled={index >= changes.length - 1}
      onclick={() => (index += 1)}
    >
      <Icon name="right" />
    </button>
    <span class="count">{index + 1}/{changes.length}</span>
  </div>
  <div class="body">
    {#if change}
      {#key change}
        <SavedDiffPane load={loader} path={change.file.path} />
      {/key}
    {/if}
  </div>
</Dialog>

<style>
  .bar {
    display: flex;
    gap: 2px;
    align-items: center;
    margin-bottom: 6px;
  }

  .count {
    margin-left: 6px;
    color: var(--text-secondary);
  }

  .body {
    display: flex;
    height: 60vh;
    min-height: 240px;
    border: 1px solid var(--divider);
  }
</style>
