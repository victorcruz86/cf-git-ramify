<script lang="ts">
  import Icon from "../lib/Icon.svelte";

  interface Props {
    tabs: { id: number; name: string; closable: boolean }[];
    current: number;
    newTab: string;
    onselect: (id: number) => void;
    onclose: (id: number) => void;
    onnew: () => void;
  }

  let { tabs, current, newTab, onselect, onclose, onnew }: Props = $props();
</script>

<div class="tabs" role="tablist" aria-label="Log tabs">
  {#each tabs as tab (tab.id)}
    <span class="tab" class:selected={tab.id === current}>
      <button
        type="button"
        role="tab"
        aria-selected={tab.id === current}
        onclick={() => tab.id !== current && onselect(tab.id)}
      >
        {tab.name}
      </button>
      {#if tab.closable}
        <button type="button" class="close" aria-label="Close Log Tab" onclick={() => onclose(tab.id)}>
          <Icon name="close" size={12} />
        </button>
      {/if}
    </span>
  {/each}
  <button type="button" class="action" aria-label={newTab} title={newTab} onclick={onnew}>
    <Icon name="add" />
  </button>
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
