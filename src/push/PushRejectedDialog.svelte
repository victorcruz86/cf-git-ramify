<script lang="ts">
  import type { UpdateMethod } from "../lib/api";
  import Dialog from "../lib/Dialog.svelte";
  import Icon from "../lib/Icon.svelte";
  import type { RejectedAnswer } from "./model";

  // `GitRejectedPushUpdateDialog`. With unpushed merges, `Merge` is the default button and focus goes
  // to `Cancel`; otherwise, the default button is the saved method and focus goes to the other one.
  interface Props {
    branch: string;
    method: UpdateMethod;
    merges: boolean;
    onanswer: (answer: RejectedAnswer | null) => void;
  }

  let { branch, method, merges, onanswer }: Props = $props();

  const REMEMBER =
    "Remember the update method and update silently in the future. Change this later in Settings | Version Control.";

  let remember = $state(false);

  const chosen = $derived<UpdateMethod>(merges ? "merge" : method);
  const answer = (picked: UpdateMethod) => onanswer({ method: picked, remember });
</script>

{#snippet choice(picked: UpdateMethod, label: string)}
  {#if picked === chosen}
    <button class="button default">{label}</button>
  {:else}
    <button
      type="button"
      class="button"
      data-autofocus={!merges || undefined}
      onclick={() => answer(picked)}
    >
      {label}
    </button>
  {/if}
{/snippet}

<Dialog
  title="Push Rejected"
  width={520}
  onclose={() => onanswer(null)}
  onsubmit={() => answer(chosen)}
>
  <div class="warning">
    <Icon name="warning" size={24} />
    <div>
      <p>
        Push of the current branch "{branch}" was rejected. Remote changes need to be merged before
        pushing.
      </p>
      {#if merges}
        <p>
          In this case <b>merge is highly recommended</b>, because there are non-pushed merge
          commits.<br />
          Rebasing them can lead to problems.
        </p>
      {/if}
      <label class="checkbox-label">
        <input type="checkbox" aria-label={REMEMBER} bind:checked={remember} />
        <span class="check" data-state={remember ? "on" : "off"}></span>
        <span>{REMEMBER}</span>
      </label>
    </div>
  </div>
  {#snippet footer()}
    {@render choice("merge", "Merge")}
    {@render choice("rebase", merges ? "Rebase Anyway" : "Rebase")}
    <button
      type="button"
      class="button"
      data-autofocus={merges || undefined}
      onclick={() => onanswer(null)}
    >
      Cancel
    </button>
  {/snippet}
</Dialog>

<style>
  .warning {
    display: flex;
    gap: 12px;
    align-items: flex-start;
  }

  .warning > div {
    min-width: 0;
  }

  p {
    margin: 0 0 10px;
    overflow-wrap: anywhere;
  }
</style>
