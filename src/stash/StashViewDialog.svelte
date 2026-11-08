<script lang="ts">
  import { untrack } from "svelte";
  import Dialog from "../lib/Dialog.svelte";
  import SavedChangesList from "./SavedChangesList.svelte";
  import SavedDiffPane from "./SavedDiffPane.svelte";
  import { shownChanges, type SavedChange, type SavedDiff } from "./savedPatches";

  // `ChangeListViewerDialog` with `viewStash(…, compareWithLocal = true)`: the changes of the stash, each
  // with the local file before the stash's version, and a `Close` button. The diff is shown next to
  // the list instead of in its own window.
  interface Props {
    title: string;
    changes: SavedChange[];
    load: (change: SavedChange) => Promise<SavedDiff>;
    onclose: () => void;
  }

  let { title, changes, load, onclose }: Props = $props();

  let current = $state.raw<SavedChange | null>(untrack(() => shownChanges(changes)[0] ?? null));
  const loader = $derived.by(() => {
    const change = current;
    return change ? () => load(change) : null;
  });
</script>

<Dialog {title} width={900} {onclose} onsubmit={onclose}>
  <div class="body">
    <div class="changes">
      <SavedChangesList
        {changes}
        empty=""
        {current}
        onselect={(change) => (current = change)}
        onopen={(change) => (current = change)}
      />
    </div>
    <div class="diff">
      {#if current}
        <SavedDiffPane load={loader} path={current.file.path} />
      {/if}
    </div>
  </div>
  {#snippet footer()}
    <button type="submit" class="button default">Close</button>
  {/snippet}
</Dialog>

<style>
  .body {
    display: flex;
    height: 55vh;
    min-height: 240px;
    border: 1px solid var(--divider);
  }

  .changes {
    display: flex;
    flex: 0 0 260px;
    min-width: 0;
    border-right: 1px solid var(--divider);
  }

  .diff {
    display: flex;
    flex: 1;
    min-width: 0;
  }
</style>
