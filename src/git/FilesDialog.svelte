<script lang="ts">
  import Dialog from "../lib/Dialog.svelte";

  // `GitUtil.showPathsInDialog`: a description and the list of files, with only OK. The names come
  // from the repository: they go as text.
  interface Props {
    title: string;
    message: string;
    files: string[];
    onanswer: () => void;
  }

  let { title, message, files, onanswer }: Props = $props();
</script>

<Dialog {title} onclose={onanswer} onsubmit={onanswer} width={480}>
  <p>{message}</p>
  <ul>
    {#each files as file (file)}
      <li>{file}</li>
    {/each}
  </ul>
  {#snippet footer()}
    <button class="button default" data-autofocus>OK</button>
  {/snippet}
</Dialog>

<style>
  p {
    margin: 0 0 6px;
    white-space: pre-line;
  }

  ul {
    max-height: 220px;
    margin: 8px 0 0;
    padding: 6px 8px;
    overflow: auto;
    list-style: none;
    background: var(--background);
    border: 1px solid var(--popup-border);
    border-radius: 4px;
  }

  li {
    line-height: 20px;
    overflow-wrap: anywhere;
  }
</style>
