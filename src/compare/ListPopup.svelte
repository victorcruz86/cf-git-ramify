<script lang="ts">
  import { onMount, tick } from "svelte";
  import { speedMatches } from "../lib/speedSearch";

  // `DvcsCompareWithAction.createPopup`: a list chooser shown in the middle of the window, with a
  // title, filtered by typing (`setNamerForFiltering`). Enter or a click chooses, Escape closes.
  interface Props {
    title: string;
    items: string[];
    onchoose: (item: string) => void;
    onclose: () => void;
  }

  let { title, items, onchoose, onclose }: Props = $props();

  let box: HTMLDivElement;
  let query = $state("");
  let current = $state(0);

  const shown = $derived(items.filter((item) => speedMatches(item, query)));

  function choose(item: string | undefined) {
    if (item === undefined) return;
    onclose();
    onchoose(item);
  }

  async function move(step: number) {
    if (shown.length === 0) return;
    current = (current + step + shown.length) % shown.length;
    await tick();
    const row = box.querySelectorAll<HTMLElement>('[role="option"]')[current];
    row?.scrollIntoView?.({ block: "nearest" });
  }

  function keydown(event: KeyboardEvent) {
    event.stopPropagation();
    if (event.key === "Escape") {
      event.preventDefault();
      if (query) query = "";
      else onclose();
    } else if (event.key === "Enter") {
      event.preventDefault();
      choose(shown[current]);
    } else if (event.key === "ArrowDown") {
      event.preventDefault();
      move(1);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      move(-1);
    } else if (event.key === "Backspace") {
      event.preventDefault();
      query = query.slice(0, -1);
      current = 0;
    } else if (event.key.length === 1 && !event.metaKey && !event.ctrlKey) {
      event.preventDefault();
      query += event.key;
      current = 0;
    }
  }

  function pointerdown(event: PointerEvent) {
    if (!box.contains(event.target as Node)) onclose();
  }

  onMount(() => box.focus());
</script>

<svelte:window onpointerdown={pointerdown} />

<div
  bind:this={box}
  class="popup"
  role="dialog"
  aria-label={title}
  tabindex="-1"
  onkeydown={keydown}
>
  <div class="title">{title}</div>
  {#if query}
    <div class="query">{query}</div>
  {/if}
  <div class="list" role="listbox" aria-label={title}>
    {#each shown as item, index (item)}
      <div
        role="option"
        tabindex="-1"
        aria-selected={index === current}
        class:current={index === current}
        onpointerenter={() => (current = index)}
        onclick={() => choose(item)}
        onkeydown={() => {}}
      >
        {item}
      </div>
    {/each}
  </div>
</div>

<style>
  .popup {
    position: fixed;
    top: 50%;
    left: 50%;
    z-index: 30;
    display: flex;
    flex-direction: column;
    width: 350px;
    max-height: min(480px, calc(100vh - 32px));
    padding-bottom: 6px;
    background: var(--popup-background);
    border: 1px solid var(--popup-border);
    border-radius: 8px;
    outline: none;
    box-shadow: 0 6px 24px rgb(0 0 0 / 40%);
    transform: translate(-50%, -50%);
  }

  .title {
    flex: none;
    padding: 8px 14px 6px;
    font-weight: 600;
    color: var(--text);
  }

  .query {
    flex: none;
    height: 24px;
    margin: 0 14px 4px;
    overflow: hidden;
    line-height: 24px;
    color: var(--text);
    text-overflow: ellipsis;
    white-space: nowrap;
    border-bottom: 1px solid var(--menu-separator);
  }

  .list {
    flex: 1;
    min-height: 0;
    overflow: auto;
  }

  [role="option"] {
    height: 28px;
    margin: 0 6px;
    padding: 0 8px;
    overflow: hidden;
    line-height: 28px;
    color: var(--text);
    text-overflow: ellipsis;
    white-space: nowrap;
    border-radius: 4px;
  }

  [role="option"].current {
    background: var(--selection);
  }
</style>
