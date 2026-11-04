<script lang="ts">
  import Dialog from "../lib/Dialog.svelte";

  // `GitRebaseProcess.askIfShouldRebasePublishedCommit`: `Cancel` is the default button and the one
  // that has focus, so Enter does not rebase.
  interface Props {
    onanswer: (rebase: boolean) => void;
  }

  let { onanswer }: Props = $props();
</script>

<Dialog
  title="Rebasing Published Commit"
  onclose={() => onanswer(false)}
  onsubmit={() => onanswer(false)}
  width={460}
>
  <p>You are trying to rebase some commits already pushed to a protected branch.</p>
  <p>Rebasing them would duplicate commits, which is not recommended and most likely unwanted.</p>
  {#snippet footer()}
    <button type="button" class="button" onclick={() => onanswer(true)}>Rebase Anyway</button>
    <button class="button default" data-autofocus>Cancel</button>
  {/snippet}
</Dialog>

<style>
  p {
    margin: 0 0 6px;
  }
</style>
