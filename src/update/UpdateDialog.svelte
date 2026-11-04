<script lang="ts">
  import { untrack } from "svelte";
  import type { UpdateMethod } from "../lib/api";
  import Dialog from "../lib/Dialog.svelte";
  import { UPDATE_METHODS } from "./methods";
  import type { UpdateChoice } from "./flow";

  // With `hidden` it was opened with ⇧: the checkbox comes out checked, and unchecking it shows the
  // dialog again.
  interface Props {
    method: UpdateMethod;
    hidden: boolean;
    onanswer: (choice: UpdateChoice | null) => void;
  }

  let { method, hidden, onanswer }: Props = $props();

  let chosen = $state(untrack(() => method));
  let hide = $state(untrack(() => hidden));
</script>

<Dialog
  title="Update Project"
  onclose={() => onanswer(null)}
  onsubmit={() => onanswer({ method: chosen, hide })}
  width={440}
>
  <div class="methods" role="radiogroup" aria-label="Update method">
    {#each UPDATE_METHODS as { id, label } (id)}
      <label class="method">
        <input type="radio" name="update-method" value={id} bind:group={chosen} data-autofocus={id === chosen || undefined} />
        {label}
      </label>
    {/each}
  </div>
  {#snippet footer()}
    <label class="checkbox-label dont-show">
      <input type="checkbox" bind:checked={hide} />
      <span class="check" data-state={hide ? "on" : "off"}></span>
      Don't show again
    </label>
    <button type="button" class="button" onclick={() => onanswer(null)}>Cancel</button>
    <button class="button default">OK</button>
  {/snippet}
</Dialog>

<style>
  .methods {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .method {
    display: flex;
    gap: 6px;
    align-items: center;
  }

  .dont-show {
    margin-right: auto;
  }
</style>
