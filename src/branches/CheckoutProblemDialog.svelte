<script lang="ts">
  import type { CheckoutMode, CleanTree } from "../lib/api";
  import Dialog from "../lib/Dialog.svelte";

  // `operation` is lowercase, like `checkout.operation.name`: the title and the buttons already carry
  // it capitalized. Without `force` there is no button on the left, like Smart Merge.
  interface Props {
    operation: string;
    title: string;
    smart: string;
    force: string | null;
    cancel: string;
    files: string[];
    // The repository's saver (`smart.operation.dialog.*.stash` or `.shelf`, G:377-380).
    saver?: CleanTree;
    onanswer: (how: CheckoutMode | null) => void;
  }

  let { operation, title, smart, force, cancel, files, saver = "shelve", onanswer }: Props =
    $props();

  // G:377 and G:378 without the product name, as the rest of the app; G:379 and G:380 are the
  // description of the smart button.
  const verbs = $derived(saver === "stash" ? ["stash", "unstash"] : ["shelve", "unshelve"]);
  const description = $derived(
    `${saver === "stash" ? "Stash" : "Shelve"} local changes, ${operation}, ${verbs[1]}`,
  );
</script>

<!-- Focus goes to `cancel`, like `GitSmartOperationDialog`: Enter must not touch anything by surprise. -->
<Dialog {title} onclose={() => onanswer(null)} onsubmit={() => onanswer("smart")} width={520}>
  <p>Your local changes to the following files would be overwritten by {operation}.</p>
  <p>Can {verbs[0]} the changes, {operation} and {verbs[1]} them after that.</p>
  <ul>
    {#each files as file}
      <li>{file}</li>
    {/each}
  </ul>
  {#snippet footer()}
    {#if force}
      <button type="button" class="button left" onclick={() => onanswer("force")}>{force}</button>
    {/if}
    <button type="button" class="button" data-autofocus onclick={() => onanswer(null)}>
      {cancel}
    </button>
    <button class="button default" title={description}>{smart}</button>
  {/snippet}
</Dialog>

<style>
  p {
    margin: 0 0 6px;
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

  .left {
    margin-right: auto;
  }
</style>
