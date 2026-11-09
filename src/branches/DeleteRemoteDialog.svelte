<script lang="ts">
  import Dialog from "../lib/Dialog.svelte";

  // `GitBranchUiHandlerImpl.confirmRemoteBranchDeletion` (`B:1253-1257`): one or several remote
  // branches.
  interface Props {
    branches: string[];
    // The local branches that track any of them, except the current one.
    tracking: string[];
    onanswer: (answer: { alsoLocal: boolean } | null) => void;
  }

  let { branches, tracking, onanswer }: Props = $props();

  const several = $derived(branches.length > 1);

  let alsoLocal = $state(false);
</script>

<Dialog
  title={several ? "Delete Remote Branches" : "Delete Remote Branch"}
  onclose={() => onanswer(null)}
  onsubmit={() => onanswer({ alsoLocal })}
  width={420}
>
  <p>
    {several ? "Delete remote branches" : "Delete remote branch"}
    {branches.join(", ")}?
  </p>
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
    <button class="button default" data-autofocus>{several ? "Delete All" : "Delete"}</button>
  {/snippet}
</Dialog>

<style>
  p {
    margin: 0 0 10px;
  }
</style>
