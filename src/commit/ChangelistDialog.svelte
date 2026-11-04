<script lang="ts">
  import { untrack } from "svelte";
  import Dialog from "../lib/Dialog.svelte";
  import { nameProblem } from "./changelists";

  interface Props {
    title: string;
    name?: string;
    // The names of the other lists, which this one cannot repeat.
    taken: string[];
    canSetActive: boolean;
    onconfirm: (name: string, setActive: boolean) => void;
    onclose: () => void;
  }

  let { title, name = "", taken, canSetActive, onconfirm, onclose }: Props = $props();

  let value = $state(untrack(() => name));
  let setActive = $state(false);
  const problem = $derived(nameProblem(value, taken));

  function submit() {
    if (!problem) onconfirm(value.trim(), setActive);
  }
</script>

<Dialog {title} {onclose} onsubmit={submit} width={420}>
  <label class="row">
    Name:
    <input class="input" data-autofocus bind:value />
  </label>
  {#if canSetActive}
    <label class="checkbox-label active">
      <input type="checkbox" bind:checked={setActive} />
      <span class="check" data-state={setActive ? "on" : "off"}></span>
      Set active
    </label>
  {/if}
  {#if value && problem}
    <p class="field-error">{problem}</p>
  {/if}
  {#snippet footer()}
    <button type="button" class="button" onclick={onclose}>Cancel</button>
    <button class="button default" disabled={problem !== null}>OK</button>
  {/snippet}
</Dialog>

<style>
  .row {
    display: flex;
    gap: 8px;
    align-items: center;
  }

  .row input {
    flex: 1;
  }

  .active {
    margin-top: 10px;
  }
</style>
