<script lang="ts">
  import type { UpdateMethod } from "../lib/api";
  import Dialog from "../lib/Dialog.svelte";

  // `GitRebaseOverMergeProblem`: `Merge` is the default button and focus goes to `Rebase`.
  interface Props {
    onanswer: (method: UpdateMethod | null) => void;
  }

  let { onanswer }: Props = $props();
</script>

<Dialog
  title="Rebasing Merge Commits"
  onclose={() => onanswer(null)}
  onsubmit={() => onanswer("merge")}
  width={440}
>
  <p>You are about to rebase a merge commit with conflicts.</p>
  <p>
    Choose 'Merge' if you don't want to resolve conflicts again, or you still can rebase if you want
    to linearize the history.
  </p>
  {#snippet footer()}
    <button type="button" class="button" onclick={() => onanswer(null)}>Cancel</button>
    <button type="button" class="button" data-autofocus onclick={() => onanswer("rebase")}>
      Rebase
    </button>
    <button class="button default">Merge</button>
  {/snippet}
</Dialog>

<style>
  p {
    margin: 0 0 10px;
  }
</style>
