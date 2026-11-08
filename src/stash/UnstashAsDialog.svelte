<script lang="ts">
  import type { StashEntry } from "../lib/api";
  import type { BranchNames } from "../branches/refName";
  import Dialog from "../lib/Dialog.svelte";
  import { unstashBranchProblem, type UnstashChoice } from "./flow";

  // `GitUnstashAsDialog`, with the 2024.1 bug fixed (decision 11): there both checkboxes are bound to
  // `popStash`, so `Reinstate index` never reached git. Here each one is its own option.
  interface Props {
    // The root's name.
    root: string;
    branch: string;
    entry: StashEntry;
    names: BranchNames;
    onanswer: (choice: UnstashChoice | null) => void;
  }

  let { root, branch: current, entry, names, onanswer }: Props = $props();

  let branch = $state("");
  let pop = $state(false);
  let reinstateIndex = $state(false);

  const hasBranch = $derived(branch.trim() !== "");
  // `validationOnInput`: a blank name is no branch at all.
  const problem = $derived(hasBranch ? unstashBranchProblem(branch, names) : null);
  const ok = $derived(hasBranch ? "Branch" : pop ? "Pop Stash" : "Apply Stash");

  function submit() {
    if (problem) return;
    onanswer({ entry, branch, pop, reinstateIndex });
  }
</script>

<Dialog title="Unstash Changes in {root}" width={420} onclose={() => onanswer(null)} onsubmit={submit}>
  <div class="rows">
    <span>Current branch:</span>
    <span class="value">{current}</span>
    <label for="unstash-as-branch">As new branch:</label>
    <input
      id="unstash-as-branch"
      class="input"
      data-autofocus
      bind:value={branch}
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
    <button type="button" class="button" onclick={() => onanswer(null)}>Cancel</button>
    <button class="button default" disabled={problem !== null}>{ok}</button>
  {/snippet}
</Dialog>

<style>
  .rows {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 8px 10px;
    align-items: center;
  }

  .value {
    user-select: text;
  }

  .options {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-top: 10px;
  }
</style>
