<script lang="ts">
  import type { ChangeList } from "../lib/api";
  import Icon from "../lib/Icon.svelte";

  // `MultipleLocalChangeListsBrowser.ChangeListChooser`: every list, the active one in bold, names cut
  // to 35 characters, and disabled while there is only one list.
  const MAX_NAME_LENGTH = 35;

  interface Props {
    lists: ChangeList[];
    active: string;
    selected: string;
    onchange: (id: string) => void;
  }

  let { lists, active, selected, onchange }: Props = $props();

  let open = $state(false);
  let current = $state(0);
  let root: HTMLDivElement;
  let button: HTMLButtonElement;

  const chosen = $derived(lists.find((list) => list.id === selected) ?? lists[0]);
  const disabled = $derived(lists.length < 2);

  // `StringUtil.shortenTextWithEllipsis(name.trim(), 35, 0)`: without `useEllipsisSymbol` the symbol
  // is three dots, and the result is 35 characters long with them.
  const SYMBOL = "...";

  function shortName(name: string): string {
    const trimmed = name.trim();
    if (trimmed.length <= MAX_NAME_LENGTH) return trimmed;
    return `${trimmed.slice(0, MAX_NAME_LENGTH - SYMBOL.length)}${SYMBOL}`;
  }

  function show() {
    if (disabled) return;
    current = Math.max(0, lists.findIndex((list) => list.id === selected));
    open = true;
  }

  function pick(id: string) {
    open = false;
    button.focus();
    if (id !== selected) onchange(id);
  }

  function keydown(event: KeyboardEvent) {
    if (disabled) return;
    if (!open) {
      if (!["ArrowDown", "ArrowUp", "Enter", " "].includes(event.key)) return;
      show();
    } else if (event.key === "Escape") {
      // The popup closes, not the dialog.
      event.stopPropagation();
      open = false;
    } else if (event.key === "ArrowDown") {
      current = Math.min(lists.length - 1, current + 1);
    } else if (event.key === "ArrowUp") {
      current = Math.max(0, current - 1);
    } else if (event.key === "Enter" || event.key === " ") {
      pick(lists[current].id);
    } else if (event.key === "Tab") {
      open = false;
      return;
    } else {
      return;
    }
    event.preventDefault();
  }

  function outside(event: PointerEvent) {
    if (open && !root.contains(event.target as Node)) open = false;
  }
</script>

<svelte:window onpointerdown={outside} />

<div bind:this={root} class="chooser">
  <span id="changelist-label" class="label">Changelist:</span>
  <button
    bind:this={button}
    type="button"
    class="input combo"
    role="combobox"
    aria-labelledby="changelist-label"
    aria-expanded={open}
    aria-controls="changelist-options"
    title={chosen?.name}
    {disabled}
    onclick={() => (open ? (open = false) : show())}
    onkeydown={keydown}
  >
    <span class="name" class:bold={chosen?.id === active}>{chosen ? shortName(chosen.name) : ""}</span>
    <Icon name="chevronDown" />
  </button>
  {#if open}
    <div id="changelist-options" class="options" role="listbox" aria-label="Changelist">
      {#each lists as list, index (list.id)}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <div
          class="option"
          class:current={index === current}
          role="option"
          tabindex="-1"
          aria-selected={list.id === selected}
          onpointerenter={() => (current = index)}
          onclick={() => pick(list.id)}
        >
          {#if list.id === active}
            <strong>{shortName(list.name)}</strong>
          {:else}
            {shortName(list.name)}
          {/if}
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .chooser {
    position: relative;
    display: flex;
    flex: none;
    gap: 4px;
    align-items: center;
    padding: 4px 8px 4px 6px;
  }

  .label {
    white-space: nowrap;
  }

  .combo {
    display: flex;
    flex: 1;
    gap: 6px;
    align-items: center;
    min-width: 0;
    text-align: left;
  }

  .combo:disabled {
    opacity: 0.6;
  }

  .name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .bold {
    font-weight: 600;
  }

  .options {
    position: absolute;
    top: 100%;
    right: 8px;
    left: 78px;
    z-index: 5;
    max-height: 240px;
    padding: 4px;
    overflow: auto;
    background: var(--popup-background);
    border: 1px solid var(--popup-border);
    border-radius: 6px;
    box-shadow: 0 4px 16px rgb(0 0 0 / 35%);
  }

  .option {
    padding: 3px 8px;
    border-radius: 4px;
    white-space: nowrap;
  }

  .option.current {
    background: var(--selection);
  }

  strong {
    font-weight: 600;
  }
</style>
