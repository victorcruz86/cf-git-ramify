<script lang="ts">
  import Dialog from "../lib/Dialog.svelte";

  // `ReopenProjectAction` for a recent project whose path no longer exists: OK, the default,
  // keeps it in the list (it may be on a drive that is not connected), and Remove From List
  // takes it out. Escape is OK.
  interface Props {
    path: string;
    onanswer: (remove: boolean) => void;
  }

  let { path, onanswer }: Props = $props();
</script>

<Dialog
  title="Reopen Project"
  onclose={() => onanswer(false)}
  onsubmit={() => onanswer(false)}
  width={420}
>
  <p>
    The path {path} does not exist.
    {"\n"}If it is on a removable or network drive, please make sure that the drive is connected.
  </p>
  {#snippet footer()}
    <button type="button" class="button" onclick={() => onanswer(true)}>Remove From List</button>
    <button class="button default" data-autofocus>OK</button>
  {/snippet}
</Dialog>

<style>
  p {
    margin: 0;
    white-space: pre-line;
    overflow-wrap: anywhere;
  }
</style>
