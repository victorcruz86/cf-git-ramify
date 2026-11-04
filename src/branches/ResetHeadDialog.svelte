<script lang="ts">
  import type { ResetMode } from "../lib/api";
  import Dialog from "../lib/Dialog.svelte";
  import type { ResetHeadChoice } from "./actions";

  // `GitResetDialog`, without the root selector: there is one per window.
  interface Props {
    branch: string;
    onanswer: (choice: ResetHeadChoice | null) => void;
  }

  let { branch, onanswer }: Props = $props();

  const MODES: { id: ResetMode; label: string }[] = [
    { id: "mixed", label: "Mixed" },
    { id: "soft", label: "Soft" },
    { id: "hard", label: "Hard" },
  ];
  const HEAD = "HEAD";

  let mode = $state<ResetMode>("mixed");
  let commit = $state(HEAD);
</script>

<Dialog
  title="Reset Head"
  onclose={() => onanswer(null)}
  onsubmit={() => onanswer({ mode, commit: commit.trim() || HEAD })}
  width={420}
>
  <div class="grid">
    <span>Current Branch:</span>
    <span>{branch}</span>
    <label for="reset-head-type">Reset Type:</label>
    <select id="reset-head-type" class="input" bind:value={mode}>
      {#each MODES as { id, label } (id)}
        <option value={id}>{label}</option>
      {/each}
    </select>
    <label for="reset-head-commit">To Commit:</label>
    <input id="reset-head-commit" class="input" data-autofocus bind:value={commit} />
  </div>
  {#snippet footer()}
    <button type="button" class="button" onclick={() => onanswer(null)}>Cancel</button>
    <button class="button default">Reset</button>
  {/snippet}
</Dialog>

<style>
  .grid {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 8px 12px;
    align-items: center;
  }
</style>
