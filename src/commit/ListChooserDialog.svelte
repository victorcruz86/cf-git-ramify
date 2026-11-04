<script lang="ts">
  import { untrack } from "svelte";
  import type { ChangeList } from "../lib/api";
  import Dialog from "../lib/Dialog.svelte";

  interface Props {
    title: string;
    prompt?: string;
    lists: ChangeList[];
    initial: string;
    // With this, the name of a new list can be typed.
    allowNew?: boolean;
    onconfirm: (name: string, setActive: boolean) => void;
    onclose: () => void;
  }

  let { title, prompt, lists, initial, allowNew = false, onconfirm, onclose }: Props = $props();

  let name = $state(untrack(() => initial));
  let setActive = $state(false);
  const trimmed = $derived(name.trim());
  const existing = $derived(
    lists.some((list) => list.name.toLowerCase() === trimmed.toLowerCase()),
  );
  const valid = $derived(trimmed !== "" && (allowNew || existing));

  function submit() {
    if (valid) onconfirm(trimmed, setActive);
  }
</script>

<Dialog {title} {onclose} onsubmit={submit} width={420}>
  {#if prompt}
    <p class="prompt">{prompt}</p>
  {/if}
  {#if allowNew}
    <label class="row">
      Name:
      <input class="input" data-autofocus bind:value={name} />
      {#if trimmed && !existing}
        <span class="tag">New</span>
      {/if}
    </label>
  {/if}
  <div class="list" role="listbox" aria-label="Changelists">
    {#each lists as list (list.id)}
      <button
        type="button"
        role="option"
        aria-selected={list.name === trimmed}
        class:selected={list.name === trimmed}
        onclick={() => (name = list.name)}
        ondblclick={submit}
      >
        {list.name}
      </button>
    {/each}
  </div>
  {#if allowNew}
    <label class="checkbox-label active">
      <input type="checkbox" bind:checked={setActive} />
      <span class="check" data-state={setActive ? "on" : "off"}></span>
      Set active
    </label>
  {/if}
  {#snippet footer()}
    <button type="button" class="button" onclick={onclose}>Cancel</button>
    <button class="button default" disabled={!valid}>OK</button>
  {/snippet}
</Dialog>

<style>
  .prompt {
    margin: 0 0 8px;
  }

  .row {
    display: flex;
    gap: 8px;
    align-items: center;
    margin-bottom: 8px;
  }

  .row input {
    flex: 1;
  }

  .tag {
    padding: 1px 6px;
    color: var(--text-secondary);
    background: var(--hover);
    border-radius: 4px;
  }

  .list {
    display: flex;
    flex-direction: column;
    max-height: 200px;
    padding: 4px;
    overflow: auto;
    background: var(--editor);
    border: 1px solid var(--control-border);
    border-radius: 6px;
  }

  .list button {
    flex: none;
    height: 24px;
    padding: 0 8px;
    font: inherit;
    color: var(--text);
    text-align: left;
    background: none;
    border: 0;
    border-radius: 6px;
  }

  .list button.selected {
    background: var(--selection);
  }

  .active {
    margin-top: 10px;
  }
</style>
