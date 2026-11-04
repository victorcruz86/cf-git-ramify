<script lang="ts">
  import Dialog from "../lib/Dialog.svelte";

  // The app has no system folder picker: the path is typed.
  interface Props {
    initialDirectory: string;
    onanswer: (directory: string | null) => void;
  }

  let { initialDirectory, onanswer }: Props = $props();

  let directory = $state(initialDirectory);
  const valid = $derived(directory.trim() !== "");

  function submit() {
    if (valid) onanswer(directory.trim());
  }
</script>

<Dialog title="Create Git Repository" onclose={() => onanswer(null)} onsubmit={submit} width={520}>
  <p>Select directory where the new Git repository will be created.</p>
  <label class="field">
    Directory:
    <input class="input" data-autofocus bind:value={directory} />
  </label>
  {#snippet footer()}
    <button type="button" class="button" onclick={() => onanswer(null)}>Cancel</button>
    <button class="button default" disabled={!valid}>Create</button>
  {/snippet}
</Dialog>

<style>
  p {
    margin: 0 0 10px;
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-bottom: 10px;
  }
</style>
