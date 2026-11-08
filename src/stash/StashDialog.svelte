<script lang="ts">
  import Dialog from "../lib/Dialog.svelte";
  import type { StashChoice } from "./flow";

  // `GitStashDialog` and its `.form`. There is one root per window: the `Git Root:` combo shows it
  // without choices.
  interface Props {
    root: string;
    branch: string;
    onanswer: (choice: StashChoice | null) => void;
  }

  let { root, branch, onanswer }: Props = $props();

  let message = $state("");
  let keepIndex = $state(false);
</script>

<Dialog
  title="Stash"
  width={500}
  onclose={() => onanswer(null)}
  onsubmit={() => onanswer({ message, keepIndex })}
>
  <div class="rows">
    <span>Git Root:</span>
    <span class="value" title="Select Git VCS root">{root}</span>
    <span>Current Branch:</span>
    <span class="value" title="The currently checked-out branch.">{branch}</span>
    <label for="stash-message">Message:</label>
    <input
      id="stash-message"
      class="input"
      title="Enter stash message here."
      data-autofocus
      bind:value={message}
    />
    <span></span>
    <label
      class="checkbox-label"
      title="If this checkbox is selected, indexed changes are kept in the index."
    >
      <input type="checkbox" bind:checked={keepIndex} />
      <span class="check" data-state={keepIndex ? "on" : "off"}></span>
      Keep index
    </label>
  </div>
  {#snippet footer()}
    <button type="button" class="button" onclick={() => onanswer(null)}>Cancel</button>
    <button class="button default">Create Stash</button>
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
    overflow-wrap: anywhere;
    user-select: text;
  }
</style>
