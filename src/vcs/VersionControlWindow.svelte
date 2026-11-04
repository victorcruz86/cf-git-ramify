<script lang="ts">
  import { CONTENT_TITLE, type ContentId } from "../lib/tabs";

  interface Props {
    contents: ContentId[];
    selected: ContentId;
    // `focus`: the content takes the focus, as on a click. Moving along the strip with the arrows
    // keeps it on the strip.
    onselect: (id: ContentId, focus: boolean) => void;
  }

  let { contents, selected, onselect }: Props = $props();

  // The tabs of the `Version Control` window are not closable: the closable ones are those the user adds
  // (extra log tabs, file history), which are inside their own content.
  function move(event: KeyboardEvent, index: number) {
    const step = event.key === "ArrowRight" ? 1 : event.key === "ArrowLeft" ? -1 : 0;
    if (!step) return;
    const next = contents[(index + step + contents.length) % contents.length];
    event.preventDefault();
    onselect(next, false);
    const strip = (event.currentTarget as HTMLElement).closest('[role="tablist"]');
    const buttons = strip?.querySelectorAll("button");
    buttons?.[contents.indexOf(next)]?.focus();
  }
</script>

<div class="tabs" role="tablist" aria-label="Git">
  {#each contents as id, index (id)}
    <span class="tab" class:selected={id === selected}>
      <button
        type="button"
        role="tab"
        aria-selected={id === selected}
        tabindex={id === selected ? 0 : -1}
        onclick={() => id !== selected && onselect(id, true)}
        onkeydown={(event) => move(event, index)}
      >
        {CONTENT_TITLE[id]}
      </button>
    </span>
  {/each}
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
    height: 100%;
    border-bottom: 2px solid transparent;
  }

  .tab.selected {
    border-bottom-color: var(--focus);
  }

  .tab button {
    padding: 0 8px;
    font: inherit;
    color: var(--text);
    white-space: nowrap;
    background: none;
    border: 0;
  }
</style>
