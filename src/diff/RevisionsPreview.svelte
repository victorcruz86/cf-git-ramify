<script lang="ts">
  import type { MergeView } from "@codemirror/merge";
  import { onDestroy, untrack } from "svelte";
  import { compareSides, mergeBlocks, type ApiError, type RevisionPair } from "../lib/api";
  import Menu, { type MenuItem } from "../lib/Menu.svelte";
  import { setWrap } from "./editor";
  import { inGutter, loadWrap, softWrapItem, storeWrap, type DiffPlace } from "./options";
  import { revisionsMessage, sideText, unchangedText } from "./sides";
  import { createTextsView } from "./textsView";
  import { VersionEditors } from "./ThreeVersionsPane.svelte";

  // Two revisions of a file read-only, or three for a merge of the file history
  // (`MergedChangeDiffRequestProvider`: first parent, the merge, the other parent). The titles are
  // those of the pair.
  interface Props {
    pair: RevisionPair;
    place: DiffPlace;
  }

  let { pair, place }: Props = $props();

  let notice = $state<string | null>(null);
  let host = $state<HTMLDivElement>();
  let menu = $state.raw<{ x: number; y: number; items: MenuItem[] } | null>(null);
  let two: MergeView | null = null;
  let three: VersionEditors | null = null;
  let request = 0;
  // `Soft-Wrap` of the place where it is shown, read once.
  let wrap = untrack(() => loadWrap(place));

  const titles = $derived(
    pair.center ? null : ([pair.left.title, pair.right.title] as [string, string]),
  );

  function clear() {
    two?.destroy();
    three?.destroy();
    two = null;
    three = null;
  }

  async function openTwo(target: HTMLElement, token: number) {
    const sides = await compareSides(pair.left, pair.right);
    if (token !== request) return;
    notice = revisionsMessage(sides) ?? unchangedText(sides, null);
    if (notice) return;
    two = createTextsView(target, sideText(sides.old), sideText(sides.new), pair.right.path, wrap);
  }

  async function openThree(target: HTMLElement, token: number) {
    const center = pair.center!;
    const [left, right] = await Promise.all([
      compareSides(pair.left, center),
      compareSides(pair.right, center),
    ]);
    if (token !== request) return;
    notice = revisionsMessage(left) ?? revisionsMessage(right);
    if (notice) return;
    const texts = [sideText(left.old), sideText(left.new), sideText(right.old)];
    const blocks = await mergeBlocks(texts[0], texts[1], texts[2], "default");
    if (token !== request) return;
    three = new VersionEditors(target, {
      path: center.path,
      sides: { head: texts[0], staged: texts[1], local: texts[2], stagedHash: "", localHash: "" },
      blocks,
      synced: () => true,
      wrap,
      onedit: () => {},
      onaction: () => {},
      readOnly: true,
      titles: [pair.left.title, center.title, pair.right.title],
    });
  }

  async function open(target: HTMLElement) {
    const token = ++request;
    clear();
    notice = null;
    try {
      await (pair.center ? openThree(target, token) : openTwo(target, token));
    } catch (failure) {
      if (token === request) notice = (failure as ApiError).message ?? String(failure);
    }
  }

  function toggleWrap(on: boolean) {
    wrap = on;
    storeWrap(place, on);
    if (two) setWrap([two.a, two.b], on);
    three?.setWrap(on);
  }

  function contextmenu(event: MouseEvent) {
    if ((!two && !three) || !inGutter(event)) return;
    event.preventDefault();
    menu = { x: event.clientX, y: event.clientY, items: [softWrapItem(wrap, toggleWrap)] };
  }

  $effect(() => {
    if (host) void open(host);
  });

  onDestroy(clear);
</script>

<div class="preview">
  {#if titles}
    <div class="titles">
      <span>{titles[0]}</span>
      <span>{titles[1]}</span>
    </div>
  {/if}
  {#if notice}
    <p>{notice}</p>
  {/if}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="host" bind:this={host} oncontextmenu={contextmenu}></div>
</div>

{#if menu}
  <Menu x={menu.x} y={menu.y} label="Diff" items={menu.items} onclose={() => (menu = null)} />
{/if}

<style>
  .preview {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }

  .titles {
    display: flex;
    flex: none;
    height: 28px;
    color: var(--text-secondary);
    background: var(--panel);
    border-bottom: 1px solid var(--divider);
  }

  .titles span {
    flex: 1;
    min-width: 0;
    padding: 6px 8px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  p {
    padding: 16px;
    margin: 0;
    color: var(--text-secondary);
  }

  .host {
    display: flex;
    flex: 1;
    min-height: 0;
    overflow: auto;
  }

  .host :global(.rm-merge) {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }

  .host :global(.rm-merge-titles) {
    display: flex;
    flex: none;
    height: 28px;
    color: var(--text-secondary);
    background: var(--panel);
    border-bottom: 1px solid var(--divider);
  }

  .host :global(.rm-merge-title) {
    display: flex;
    flex: 1;
    align-items: center;
    min-width: 0;
    padding: 0 8px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .host :global(.rm-merge-title-gap),
  .host :global(.rm-divider) {
    flex: none;
    width: 30px;
  }

  .host :global(.rm-divider) {
    background: var(--panel);
  }

  .host :global(.rm-editors) {
    display: flex;
    flex: 1;
    min-width: 0;
    min-height: 0;
  }

  .host :global(.rm-editor) {
    flex: 1;
    min-width: 0;
    min-height: 0;
  }

  .host :global(.rm-editor .cm-editor) {
    height: 100%;
  }

  .host :global(.rm-inserted-band) {
    fill: var(--diff-inserted);
  }

  .host :global(.rm-deleted-band) {
    fill: var(--diff-deleted);
  }

  .host :global(.rm-modified-band) {
    fill: var(--diff-modified);
  }
</style>
