<script lang="ts">
  import { onMount } from "svelte";

  // `ShowBaseRevisionAction.NotificationPanel` in a `JBPopup`: the text of `V:611`, line by line, in
  // the middle of the window (`showCenteredInCurrentWindow`). A click outside or Escape closes it.
  // The text comes from the repository: it is shown as text only.
  interface Props {
    lines: string[];
    onclose: () => void;
  }

  let { lines, onclose }: Props = $props();

  let box: HTMLDivElement;

  function keydown(event: KeyboardEvent) {
    if (event.key !== "Escape") return;
    event.preventDefault();
    event.stopPropagation();
    onclose();
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
  aria-label="Current Revision"
  tabindex="-1"
  onkeydown={keydown}
>
  {#each lines as line, index (index)}
    <p class:empty={line === ""}>{line}</p>
  {/each}
</div>

<style>
  .popup {
    position: fixed;
    top: 50%;
    left: 50%;
    z-index: 30;
    max-width: min(560px, calc(100vw - 32px));
    max-height: min(480px, calc(100vh - 32px));
    padding: 15px;
    overflow: auto;
    color: var(--text);
    background: var(--popup-background);
    border: 1px solid var(--popup-border);
    border-radius: 8px;
    outline: none;
    box-shadow: 0 6px 24px rgb(0 0 0 / 40%);
    transform: translate(-50%, -50%);
    user-select: text;
  }

  p {
    margin: 0;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  p.empty {
    height: 1lh;
  }
</style>
