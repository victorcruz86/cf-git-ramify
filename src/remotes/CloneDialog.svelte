<script lang="ts">
  import Dialog from "../lib/Dialog.svelte";

  interface Props {
    onanswer: (answer: { url: string; directory: string } | null) => void;
  }

  let { onanswer }: Props = $props();

  let url = $state("");
  let directory = $state("");
  const valid = $derived(url.trim() !== "" && directory.trim() !== "");

  function submit() {
    if (valid) onanswer({ url: url.trim(), directory: directory.trim() });
  }
</script>

<Dialog title="Clone Repository" onclose={() => onanswer(null)} onsubmit={submit} width={520}>
  <label class="field">
    URL:
    <input class="input" data-autofocus bind:value={url} />
  </label>
  <label class="field">
    Directory:
    <input class="input" bind:value={directory} placeholder="~/path/to/new/directory" />
  </label>
  {#snippet footer()}
    <button type="button" class="button" onclick={() => onanswer(null)}>Cancel</button>
    <button class="button default" disabled={!valid}>Clone</button>
  {/snippet}
</Dialog>

<style>
  .field {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-bottom: 10px;
  }
</style>
