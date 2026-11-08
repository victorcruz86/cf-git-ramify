<script lang="ts">
  import { sideMessage, sideText } from "../diff/sides";
  import TextsDiff from "../diff/TextsDiff.svelte";
  import { errorText } from "../lib/errors";
  import type { SavedDiff } from "./savedPatches";

  // The diff of one saved change: the preview of the tab and the body of its diff dialogs. `load` changes
  // with the change; whatever arrives for an earlier one is dropped.
  interface Props {
    load: (() => Promise<SavedDiff>) | null;
    path: string;
  }

  let { load, path }: Props = $props();

  let diff = $state.raw<SavedDiff | null>(null);
  let notice = $state<string | null>(null);
  let request = 0;

  async function show(loader: (() => Promise<SavedDiff>) | null) {
    const token = ++request;
    diff = null;
    notice = null;
    if (!loader) return;
    try {
      const loaded = await loader();
      if (token !== request) return;
      notice = sideMessage(loaded.left) ?? sideMessage(loaded.right);
      diff = loaded;
    } catch (failure) {
      if (token === request) notice = errorText(failure);
    }
  }

  $effect(() => {
    void show(load);
  });
</script>

<div class="pane">
  {#if notice}
    <p class="notice">{notice}</p>
  {:else if diff}
    <TextsDiff
      oldText={sideText(diff.left)}
      newText={sideText(diff.right)}
      {path}
      titles={diff.titles}
    />
  {/if}
</div>

<style>
  .pane {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }

  .pane > :global(*) {
    flex: 1;
    min-height: 0;
  }

  .notice {
    flex: none;
    padding: 16px;
    margin: 0;
    color: var(--text-secondary);
  }
</style>
