<script lang="ts">
  import type { MergeView } from "@codemirror/merge";
  import { onDestroy, untrack } from "svelte";
  import Menu, { type MenuItem } from "../lib/Menu.svelte";
  import { setWrap } from "./editor";
  import { inGutter, loadWrap, softWrapItem, storeWrap, type DiffPlace } from "./options";
  import { createTextsView } from "./textsView";

  // Two given texts, read only: the preview of a stash or shelf change and Apply Patch's
  // `Show Difference`. The texts are untrusted and are only shown as text.
  interface Props {
    oldText: string;
    newText: string;
    // Only for the syntax highlighting.
    path: string;
    titles?: [string, string];
    // Whose `Soft-Wrap` it remembers: by default the place SavedPatchesDiffPreview uses,
    // `SavedPatchesUiPlace` (off by default); Apply Patch's `Show Difference` is `Default`.
    place?: DiffPlace;
  }

  let { oldText, newText, path, titles, place = "SavedPatchesUiPlace" }: Props = $props();

  const PLACE = untrack(() => place);

  let host: HTMLDivElement;
  let view: MergeView | null = null;
  let wrap = loadWrap(PLACE);
  let menu = $state.raw<{ x: number; y: number; items: MenuItem[] } | null>(null);

  function clearView() {
    view?.destroy();
    view = null;
  }

  // Both sides wrap together, without being rebuilt; MergeView measures its own spacers.
  function toggleWrap(on: boolean) {
    wrap = on;
    storeWrap(PLACE, on);
    if (view) setWrap([view.a, view.b], on);
  }

  // The gutter popup of SetEditorSettingsActionGroup. The text has no menu here.
  function contextmenu(event: MouseEvent) {
    if (!view || !inGutter(event)) return;
    event.preventDefault();
    menu = { x: event.clientX, y: event.clientY, items: [softWrapItem(wrap, toggleWrap)] };
  }

  $effect(() => {
    const [before, after, file] = [oldText, newText, path];
    clearView();
    view = createTextsView(host, before, after, file, wrap);
  });

  onDestroy(clearView);
</script>

<div class="texts">
  {#if titles}
    <div class="titles">
      <span class="title">{titles[0]}</span>
      <span class="title">{titles[1]}</span>
    </div>
  {/if}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="host" bind:this={host} oncontextmenu={contextmenu}></div>
</div>

{#if menu}
  <Menu x={menu.x} y={menu.y} label="Diff" items={menu.items} onclose={() => (menu = null)} />
{/if}

<style>
  .texts {
    display: flex;
    flex-direction: column;
    height: 100%;
  }

  .titles {
    display: flex;
    flex: none;
    height: 28px;
    color: var(--text-secondary);
    background: var(--panel);
    border-bottom: 1px solid var(--divider);
  }

  .title {
    display: flex;
    flex: 1;
    align-items: center;
    min-width: 0;
    padding: 0 8px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .title + .title {
    border-left: 1px solid var(--divider);
  }

  .host {
    position: relative;
    flex: 1;
    min-height: 0;
    overflow: auto;
  }
</style>
