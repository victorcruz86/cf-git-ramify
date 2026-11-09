<script lang="ts">
  import Icon from "../lib/Icon.svelte";
  import Menu, { type MenuItem } from "../lib/Menu.svelte";
  import { contentTitle, type ContentId, type ToolWindowId } from "../lib/tabs";

  // The strip of the `Git` window, or of the `Commit` one in non-modal mode: `label` is the window's name.
  interface Props {
    toolWindow: ToolWindowId;
    label: string;
    contents: ContentId[];
    selected: ContentId;
    // `focus`: the content takes the focus, as on a click. Moving along the strip with the arrows
    // keeps it on the strip.
    onselect: (id: ContentId, focus: boolean) => void;
    // The gear of the title bar (`Options`): the selected content's gear actions, such as
    // `LocalChangesView.GearActions`. Without them there is no gear.
    options?: () => MenuItem[];
  }

  let { toolWindow, label, contents, selected, onselect, options }: Props = $props();

  let menu = $state.raw<{ x: number; y: number; items: MenuItem[] } | null>(null);

  function openOptions(event: MouseEvent) {
    if (!options) return;
    const bounds = (event.currentTarget as HTMLElement).getBoundingClientRect();
    menu = { x: bounds.left, y: bounds.bottom + 2, items: options() };
  }

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

<div class="tabs" role="tablist" aria-label={label}>
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
        {contentTitle(id, toolWindow)}
      </button>
    </span>
  {/each}
  {#if options}
    <button
      type="button"
      class="action options"
      title="Options"
      aria-label="Options"
      onclick={openOptions}
    >
      <Icon name="settings" />
    </button>
  {/if}
</div>

{#if menu}
  <Menu x={menu.x} y={menu.y} label="Options" items={menu.items} onclose={() => (menu = null)} />
{/if}

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

  .options {
    margin-left: auto;
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
