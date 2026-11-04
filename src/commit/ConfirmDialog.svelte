<script lang="ts">
  import Dialog from "../lib/Dialog.svelte";

  interface Props {
    title: string;
    lines: string[];
    onconfirm: () => void;
    onclose: () => void;
  }

  let { title, lines, onconfirm, onclose }: Props = $props();
</script>

<Dialog {title} {onclose} onsubmit={onconfirm} width={420}>
  {#each lines as line}
    <p>{line}</p>
  {/each}
  {#snippet footer()}
    <button type="button" class="button" onclick={onclose}>No</button>
    <button class="button default" data-autofocus>Yes</button>
  {/snippet}
</Dialog>

<style>
  p {
    margin: 0 0 6px;
  }
</style>
