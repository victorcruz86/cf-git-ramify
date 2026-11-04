<script lang="ts">
  import { onMount } from "svelte";
  import type { LogRef } from "../lib/api";
  import { completions } from "./filters";

  // `GoToHashOrRefPopup`: centered on the table, and a click outside cancels it. `chosen` is the
  // reference chosen from the list: the text is no longer interpreted.
  interface Props {
    x: number;
    y: number;
    refs: LogRef[];
    onsubmit: (text: string, chosen: string | null) => void;
    onclose: () => void;
  }

  let { x, y, refs, onsubmit, onclose }: Props = $props();

  const LABEL = "Enter hash or branch/tag name:";
  const MARGIN = 8;

  let box: HTMLDivElement;
  let field: HTMLInputElement;
  let text = $state("");
  let active = $state(-1);
  let left = $state(0);
  let top = $state(0);
  const names = $derived([...new Set(refs.map(({ name }) => name))]);
  const found = $derived(completions(names, text.trim()));

  function input() {
    active = -1;
  }

  function move(step: number) {
    const size = found.length;
    if (size > 0) active = ((active < 0 && step < 0 ? 0 : active) + step + size) % size;
  }

  function keydown(event: KeyboardEvent) {
    event.stopPropagation();
    if (event.key === "ArrowDown") move(1);
    else if (event.key === "ArrowUp") move(-1);
    else if (event.key === "Enter") {
      const chosen = found[active] ?? null;
      onsubmit(chosen ?? text, chosen);
    } else if (event.key === "Escape") onclose();
    else return;
    event.preventDefault();
  }

  function pointerdown(event: PointerEvent) {
    if (!box.contains(event.target as Node)) onclose();
  }

  onMount(() => {
    const bounds = box.getBoundingClientRect();
    left = Math.max(MARGIN, Math.min(x - bounds.width / 2, window.innerWidth - bounds.width - MARGIN));
    top = Math.max(MARGIN, y - bounds.height / 2);
    const previous = document.activeElement;
    field.focus();
    return () => {
      if (previous instanceof HTMLElement && previous.isConnected) previous.focus();
    };
  });
</script>

<svelte:window onpointerdown={pointerdown} />

<div bind:this={box} class="popup" style:left="{left}px" style:top="{top}px">
  <label>
    <span>{LABEL}</span>
    <input
      bind:this={field}
      bind:value={text}
      aria-label={LABEL}
      spellcheck="false"
      oninput={input}
      onkeydown={keydown}
    />
  </label>
  {#if found.length > 0}
    <ul role="listbox" aria-label="Completions">
      {#each found as name, index (name)}
        <li
          role="option"
          aria-selected={index === active}
          class:active={index === active}
          onpointerdown={(event) => {
            event.preventDefault();
            onsubmit(name, name);
          }}
        >
          {name}
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .popup {
    position: fixed;
    z-index: 20;
    box-sizing: border-box;
    width: 360px;
    padding: 6px;
    background: var(--popup-background);
    border: 1px solid var(--popup-border);
    border-radius: 8px;
    box-shadow: 0 6px 24px rgb(0 0 0 / 40%);
  }

  label {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  span {
    font-weight: 600;
    color: var(--text);
  }

  input {
    box-sizing: border-box;
    width: 100%;
    height: 28px;
    padding: 0 6px;
    font: inherit;
    color: var(--text);
    background: var(--editor);
    border: 1px solid var(--divider);
    border-radius: 4px;
    outline: none;
  }

  input:focus {
    border-color: var(--focus);
  }

  ul {
    max-height: 200px;
    padding: 0;
    margin: 4px 0 0;
    overflow: auto;
    list-style: none;
  }

  li {
    height: 24px;
    padding: 0 6px;
    overflow: hidden;
    line-height: 24px;
    color: var(--text);
    text-overflow: ellipsis;
    white-space: nowrap;
    border-radius: 4px;
  }

  li.active {
    background: var(--selection);
  }
</style>
