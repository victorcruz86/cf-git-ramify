<script lang="ts">
  import Dialog from "../lib/Dialog.svelte";

  interface Props {
    title: string;
    lines: string[];
    onconfirm: () => void;
    onclose: () => void;
    // `Yes` and `No` unless the question has its own buttons (`Delete` and `Cancel`, say).
    confirmLabel?: string;
    cancelLabel?: string;
  }

  let { title, lines, onconfirm, onclose, confirmLabel = "Yes", cancelLabel = "No" }: Props = $props();
</script>

<Dialog {title} {onclose} onsubmit={onconfirm} width={420}>
  {#each lines as line}
    <p>{line}</p>
  {/each}
  {#snippet footer()}
    <button type="button" class="button" onclick={onclose}>{cancelLabel}</button>
    <button class="button default" data-autofocus>{confirmLabel}</button>
  {/snippet}
</Dialog>

<style>
  p {
    margin: 0 0 6px;
  }
</style>
