<script lang="ts">
  import { onMount, type Snippet } from "svelte";

  interface Props {
    title: string;
    width?: number;
    // Opens as a dropdown anchored to this bottom-right corner, without dimming what is underneath.
    popup?: { right: number; bottom: number };
    onclose: () => void;
    onsubmit?: () => void;
    children: Snippet;
    footer?: Snippet;
  }

  let { title, width = 460, popup, onclose, onsubmit, children, footer }: Props = $props();

  let box: HTMLFormElement;
  const FOCUSABLE =
    "input:not(:disabled), textarea:not(:disabled), button:not(:disabled), [tabindex='0']";

  function focusable(): HTMLElement[] {
    return [...box.querySelectorAll<HTMLElement>(FOCUSABLE)];
  }

  // Focus does not leave the dialog and keys do not reach the window underneath.
  function keydown(event: KeyboardEvent) {
    event.stopPropagation();
    if (event.key === "Escape") {
      event.preventDefault();
      onclose();
    } else if (event.key === "Tab") {
      const items = focusable();
      const edge = event.shiftKey ? items[0] : items[items.length - 1];
      if (document.activeElement === edge) {
        event.preventDefault();
        (event.shiftKey ? items[items.length - 1] : items[0])?.focus();
      }
    }
  }

  function submit(event: SubmitEvent) {
    event.preventDefault();
    onsubmit?.();
  }

  onMount(() => {
    const previous = document.activeElement;
    const first = box.querySelector<HTMLElement>("[data-autofocus]") ?? focusable()[0];
    first?.focus();
    return () => {
      if (previous instanceof HTMLElement) previous.focus();
    };
  });
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="backdrop"
  class:popup
  style:padding-right={popup ? `${popup.right}px` : undefined}
  style:padding-bottom={popup ? `${popup.bottom}px` : undefined}
  onpointerdown={(event) => {
    if (popup && event.target === event.currentTarget) onclose();
  }}
>
  <div
    class="dialog"
    role="dialog"
    aria-modal="true"
    aria-label={title}
    tabindex="-1"
    style:width="{width}px"
    onkeydown={keydown}
  >
    <form bind:this={box} class="form" onsubmit={submit}>
      {#if !popup}
        <h2>{title}</h2>
      {/if}
      <div class="body">
        {@render children()}
      </div>
      {#if footer}
        <div class="footer">
          {@render footer()}
        </div>
      {/if}
    </form>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 30;
    display: flex;
    align-items: center;
    justify-content: center;
    background: var(--backdrop);
  }

  .backdrop.popup {
    box-sizing: border-box;
    align-items: flex-end;
    justify-content: flex-end;
    background: transparent;
  }

  .dialog {
    display: flex;
    flex-direction: column;
    max-width: calc(100vw - 80px);
    max-height: calc(100vh - 80px);
    background: var(--panel);
    border: 1px solid var(--popup-border);
    border-radius: 8px;
    box-shadow: 0 8px 32px rgb(0 0 0 / 45%);
  }

  .form {
    display: flex;
    flex-direction: column;
    min-height: 0;
  }

  h2 {
    margin: 0;
    padding: 12px 16px 8px;
    font-size: 13px;
    font-weight: 600;
    color: var(--text);
    text-align: center;
  }

  .body {
    flex: 1;
    min-height: 0;
    padding: 12px 16px 8px;
    overflow: auto;
  }

  .footer {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
    padding: 12px 16px;
  }
</style>
