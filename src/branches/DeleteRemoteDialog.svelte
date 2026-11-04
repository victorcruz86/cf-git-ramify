<script lang="ts">
  import Dialog from "../lib/Dialog.svelte";

  interface Props {
    branch: string;
    // The local branches that track it, except the current one.
    tracking: string[];
    onanswer: (answer: { alsoLocal: boolean } | null) => void;
  }

  let { branch, tracking, onanswer }: Props = $props();

  let alsoLocal = $state(false);
</script>

<Dialog
  title="Delete Remote Branch"
  onclose={() => onanswer(null)}
  onsubmit={() => onanswer({ alsoLocal })}
  width={420}
>
  <p>Delete remote branch {branch}?</p>
  {#if tracking.length > 0}
    <label class="checkbox-label">
      <input type="checkbox" bind:checked={alsoLocal} />
      <span class="check" data-state={alsoLocal ? "on" : "off"}></span>
      {#if tracking.length === 1}
        Delete tracking local branch {tracking[0]} as well
      {:else}
        Delete tracking local branches: {tracking.join(", ")}
      {/if}
    </label>
  {/if}
  {#snippet footer()}
    <button type="button" class="button" onclick={() => onanswer(null)}>Cancel</button>
    <button class="button default" data-autofocus>Delete</button>
  {/snippet}
</Dialog>

<style>
  p {
    margin: 0 0 10px;
  }
</style>
