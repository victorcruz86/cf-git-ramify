<script lang="ts">
  import Dialog from "../lib/Dialog.svelte";

  interface Props {
    onanswer: (reference: string | null) => void;
  }

  let { onanswer }: Props = $props();

  let value = $state("");
  const reference = $derived(value.trim());

  function submit() {
    if (reference) onanswer(reference);
  }
</script>

<Dialog title="Checkout" onclose={() => onanswer(null)} onsubmit={submit} width={420}>
  <label class="field">
    Enter reference (branch, tag) name or commit hash:
    <input class="input" data-autofocus bind:value />
  </label>
  {#snippet footer()}
    <button type="button" class="button" onclick={() => onanswer(null)}>Cancel</button>
    <button class="button default" disabled={!reference}>OK</button>
  {/snippet}
</Dialog>

<style>
  .field {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
</style>
