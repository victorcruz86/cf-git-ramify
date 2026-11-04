<script lang="ts">
  import Dialog from "../lib/Dialog.svelte";

  interface Props {
    title: string;
    message: string;
    onanswer: (yes: boolean) => void;
  }

  let { title, message, onanswer }: Props = $props();
</script>

<Dialog {title} onclose={() => onanswer(false)} onsubmit={() => onanswer(true)} width={440}>
  <p>{message}</p>
  {#snippet footer()}
    <button type="button" class="button" onclick={() => onanswer(false)}>No</button>
    <button class="button default" data-autofocus>Yes</button>
  {/snippet}
</Dialog>

<style>
  p {
    margin: 0 0 10px;
    white-space: pre-line;
    user-select: text;
  }
</style>
