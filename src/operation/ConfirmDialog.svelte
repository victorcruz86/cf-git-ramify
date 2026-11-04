<script lang="ts">
  import Dialog from "../lib/Dialog.svelte";
  import type { ConfirmRequest } from "./operation";

  // `DialogManager.showOkCancelDialog` with the question icon: Enter accepts and Escape cancels.
  interface Props extends Omit<ConfirmRequest, "kind"> {
    onanswer: (confirmed: boolean) => void;
  }

  let { title, message, ok, onanswer }: Props = $props();
</script>

<Dialog {title} onclose={() => onanswer(false)} onsubmit={() => onanswer(true)} width={380}>
  <p>{message}</p>
  {#snippet footer()}
    <button type="button" class="button" onclick={() => onanswer(false)}>Cancel</button>
    <button class="button default" data-autofocus>{ok}</button>
  {/snippet}
</Dialog>

<style>
  p {
    margin: 0;
  }
</style>
