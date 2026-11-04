<script lang="ts">
  import { onMount, tick } from "svelte";
  import { completions } from "./filters";

  // `MultilinePopupBuilder`: opens below the filter, can be resized and a click outside
  // cancels it. Only ⌘⏎ accepts.
  interface Props {
    x: number;
    y: number;
    label: string;
    value: string;
    hint: string;
    variants?: string[];
    prefixAt?: (text: string, offset: number) => string;
    onok: (text: string) => void;
    onclose: () => void;
  }

  let { x, y, label, value, hint, variants, prefixAt, onok, onclose }: Props = $props();

  const MARGIN = 8;

  let box: HTMLDivElement;
  let field: HTMLTextAreaElement;
  let text = $derived(value);
  let found = $state<string[]>([]);
  let active = $state(0);
  let left = $state(0);
  let top = $state(0);

  function input() {
    text = field.value;
    active = 0;
    found =
      variants && prefixAt ? completions(variants, prefixAt(text, field.selectionStart)) : [];
  }

  async function accept(variant: string) {
    const offset = field.selectionStart;
    const start = offset - (prefixAt?.(text, offset).length ?? 0);
    text = text.slice(0, start) + variant + text.slice(offset);
    found = [];
    await tick();
    field.selectionStart = field.selectionEnd = start + variant.length;
    field.focus();
  }

  function completing(event: KeyboardEvent): boolean {
    if (found.length === 0) return false;
    if (event.key === "ArrowDown") active = (active + 1) % found.length;
    else if (event.key === "ArrowUp") active = (active - 1 + found.length) % found.length;
    else if (event.key === "Enter" || event.key === "Tab") accept(found[active]);
    else if (event.key === "Escape") found = [];
    else return false;
    return true;
  }

  function keydown(event: KeyboardEvent) {
    event.stopPropagation();
    if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) onok(text);
    else if (!completing(event)) {
      if (event.key !== "Escape") return;
      onclose();
    }
    event.preventDefault();
  }

  function pointerdown(event: PointerEvent) {
    if (!box.contains(event.target as Node)) onclose();
  }

  onMount(() => {
    const bounds = box.getBoundingClientRect();
    left = Math.max(MARGIN, Math.min(x, window.innerWidth - bounds.width - MARGIN));
    top = Math.max(MARGIN, Math.min(y, window.innerHeight - bounds.height - MARGIN));
    const previous = document.activeElement;
    field.focus();
    field.selectionStart = field.selectionEnd = text.length;
    return () => {
      if (previous instanceof HTMLElement && previous.isConnected) previous.focus();
    };
  });
</script>

<svelte:window onpointerdown={pointerdown} />

<div bind:this={box} class="popup" style:left="{left}px" style:top="{top}px">
  <textarea
    bind:this={field}
    aria-label={label}
    spellcheck="false"
    value={text}
    oninput={input}
    onkeydown={keydown}
  ></textarea>
  {#if found.length > 0}
    <ul role="listbox" aria-label="Completions">
      {#each found as variant, index}
        <li
          role="option"
          aria-selected={index === active}
          class:active={index === active}
          onpointerdown={(event) => {
            event.preventDefault();
            accept(variant);
          }}
        >
          {variant}
        </li>
      {/each}
    </ul>
  {/if}
  <p>{hint}</p>
</div>

<style>
  .popup {
    position: fixed;
    z-index: 20;
    display: flex;
    flex-direction: column;
    box-sizing: border-box;
    width: 340px;
    min-width: 200px;
    height: 150px;
    min-height: 90px;
    overflow: hidden;
    background: var(--popup-background);
    border: 1px solid var(--popup-border);
    border-radius: 8px;
    box-shadow: 0 6px 24px rgb(0 0 0 / 40%);
    resize: both;
  }

  textarea {
    flex: 1;
    min-height: 0;
    padding: 6px 8px;
    font: inherit;
    color: var(--text);
    background: var(--input-background);
    border: 0;
    outline: none;
    resize: none;
  }

  ul {
    flex: none;
    max-height: 84px;
    margin: 0;
    padding: 2px 0;
    overflow: auto;
    list-style: none;
    border-top: 1px solid var(--divider);
  }

  li {
    padding: 2px 8px;
    color: var(--text);
    white-space: nowrap;
  }

  li.active {
    background: var(--selection);
  }

  p {
    flex: none;
    margin: 0;
    padding: 4px 8px;
    font-size: 12px;
    color: var(--muted);
    border-top: 1px solid var(--divider);
  }
</style>
