<script lang="ts">
  import type { Snippet } from "svelte";
  import Icon from "../lib/Icon.svelte";

  // The closable tabs of a tool window: the Log's, and the Pull Requests window's. `label` and
  // `closeLabel` name the strip and the close buttons; `title` is a tab's tooltip. Without
  // `newTab` there is no `+`; `trailing` goes after the tabs (the gear of the Pull Requests window).
  interface Props {
    tabs: { id: number; name: string; closable: boolean; title?: string }[];
    current: number;
    label?: string;
    closeLabel?: string;
    newTab?: string;
    onselect: (id: number) => void;
    onclose: (id: number) => void;
    onnew?: () => void;
    trailing?: Snippet;
  }

  let {
    tabs,
    current,
    label = "Log tabs",
    closeLabel = "Close Log Tab",
    newTab,
    onselect,
    onclose,
    onnew,
    trailing,
  }: Props = $props();
</script>

<div class="tabs" role="tablist" aria-label={label}>
  {#each tabs as tab (tab.id)}
    <span class="tab" class:selected={tab.id === current}>
      <button
        type="button"
        role="tab"
        aria-selected={tab.id === current}
        title={tab.title}
        onclick={() => tab.id !== current && onselect(tab.id)}
      >
        {tab.name}
      </button>
      {#if tab.closable}
        <button type="button" class="close" aria-label={closeLabel} onclick={() => onclose(tab.id)}>
          <Icon name="close" size={12} />
        </button>
      {/if}
    </span>
  {/each}
  {#if newTab}
    <button type="button" class="action" aria-label={newTab} title={newTab} onclick={() => onnew?.()}>
      <Icon name="add" />
    </button>
  {/if}
  {@render trailing?.()}
</div>

<style>
  .tabs {
    display: flex;
    flex: none;
    gap: 2px;
    align-items: center;
    height: 30px;
    padding: 0 8px;
    box-sizing: border-box;
    background: var(--panel);
    border-bottom: 1px solid var(--divider);
  }

  .tab {
    display: inline-flex;
    align-items: center;
    max-width: 260px;
    height: 100%;
    border-bottom: 2px solid transparent;
  }

  .tab.selected {
    border-bottom-color: var(--focus);
  }

  .tab button {
    padding: 0 8px;
    overflow: hidden;
    font: inherit;
    color: var(--text);
    text-overflow: ellipsis;
    white-space: nowrap;
    background: none;
    border: 0;
  }

  .tab .close {
    display: inline-flex;
    padding: 0 4px 0 0;
  }
</style>
