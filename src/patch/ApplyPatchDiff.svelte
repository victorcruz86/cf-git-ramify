<script lang="ts">
  import { untrack } from "svelte";
  import TextsDiff from "../diff/TextsDiff.svelte";
  import type { PatchPreview, PlannedFile } from "../lib/api";
  import Dialog from "../lib/Dialog.svelte";
  import { errorText } from "../lib/errors";
  import Icon from "../lib/Icon.svelte";

  // `Show Difference` of Apply Patch: the chain of the patch's files (`ChangeDiffRequestChain`),
  // starting at `start`, each one titled with its path. A file whose target is not there says so
  // (`createBaseNotFoundErrorRequest`).
  interface Props {
    files: PlannedFile[];
    start: number;
    repo: string;
    load: (file: PlannedFile) => Promise<PatchPreview>;
    onclose: () => void;
  }

  let { files, start, repo, load, onclose }: Props = $props();

  let index = $state(untrack(() => start));
  let preview = $state.raw<PatchPreview | null>(null);
  let notice = $state<string | null>(null);
  let request = 0;

  const file = $derived(files[index]);
  // `changes.error.cannot.find.base.for.path` (V:1048).
  const noBase = (shown: PlannedFile) => `Cannot find base for '${shown.before ?? shown.after ?? shown.path}'`;

  async function show(shown: PlannedFile | undefined) {
    const token = ++request;
    preview = null;
    notice = null;
    if (!shown) return;
    if (shown.status === "missing") {
      notice = noBase(shown);
      return;
    }
    try {
      const loaded = await load(shown);
      if (token === request) preview = loaded;
    } catch (failure) {
      if (token === request) notice = errorText(failure);
    }
  }

  $effect(() => {
    void show(file);
  });
</script>

<Dialog title={file ? `${repo}/${file.path}` : ""} width={960} {onclose}>
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
      disabled={index >= files.length - 1}
      onclick={() => (index += 1)}
    >
      <Icon name="right" />
    </button>
    <span class="count">{index + 1}/{files.length}</span>
  </div>
  <div class="body">
    {#if notice}
      <p class="notice">{notice}</p>
    {:else if preview && file}
      {#key preview}
        <TextsDiff
          oldText={preview.before}
          newText={preview.after}
          path={file.path}
          titles={preview.titles}
          place="Default"
        />
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
    flex-direction: column;
    height: 60vh;
    min-height: 240px;
    border: 1px solid var(--divider);
  }

  .notice {
    margin: auto;
    color: var(--text-secondary);
    overflow-wrap: anywhere;
  }
</style>
