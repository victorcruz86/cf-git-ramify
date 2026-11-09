<script lang="ts">
  import Icon from "../lib/Icon.svelte";
  import type { LegendKey } from "./request";
  import { highlightMessage, validateMessage } from "./validate";

  const LEGEND_COLORS: Record<LegendKey, string> = {
    added: "var(--status-added)",
    modified: "var(--status-modified)",
    deleted: "var(--status-deleted)",
  };

  // `editorOnly` is the editor alone, without the Amend bar or the buttons: the one of
  // `GitNewCommitMessageActionDialog` (`CommitMessage` without a toolbar).
  interface Props {
    message: string;
    editorOnly?: boolean;
    amend?: boolean;
    amendAvailable?: boolean;
    legend?: { key: LegendKey; text: string }[];
    problem?: string | null;
    busy?: boolean;
    // The blue button indicates that ⌘⏎ presses it: only with the focus in the view.
    primary?: boolean;
    ontoggleamend?: () => void;
    onhistory?: (x: number, y: number) => void;
    onoptions?: () => void;
    oncommit?: () => void;
    // Without it, `Commit and Push…` is shown disabled.
    onpush?: () => void;
    // `Commit and Rebase…` (Fixup… and Squash Into…): without it, the button does not exist.
    onrebase?: () => void;
    oninput?: () => void;
  }

  const nothing = () => {};

  let {
    message = $bindable(),
    editorOnly = false,
    amend = false,
    amendAvailable = false,
    legend = [],
    problem = null,
    busy = false,
    primary = false,
    ontoggleamend = nothing,
    onhistory = nothing,
    onoptions = nothing,
    oncommit = nothing,
    onpush,
    onrebase,
    oninput = nothing,
  }: Props = $props();

  let field: HTMLTextAreaElement;
  let backdrop: HTMLDivElement;
  let historyButton = $state<HTMLButtonElement>();

  const issues = $derived(validateMessage(message).filter((issue) => issue.code !== "empty"));
  const segments = $derived(highlightMessage(message, issues));
  const commitLabel = $derived(amend ? "Amend Commit" : "Commit");
  const pushLabel = $derived(amend ? "Amend Commit and Push…" : "Commit and Push…");

  export function focus() {
    field.focus();
  }

  // `setCaretPosition(0)`: the beginning of the message in view.
  export function focusStart() {
    field.focus();
    field.setSelectionRange(0, 0);
    field.scrollTop = 0;
  }

  export function showHistory() {
    if (busy || !historyButton) return;
    const bounds = historyButton.getBoundingClientRect();
    onhistory(bounds.left, bounds.top);
  }

  function syncScroll() {
    backdrop.scrollTop = field.scrollTop;
  }
</script>

{#if !editorOnly}
  <div class="status">
    <div class="tools">
      <label class="checkbox-label" class:disabled={!amendAvailable}>
        <input
          type="checkbox"
          checked={amend}
          disabled={!amendAvailable || busy}
          onchange={ontoggleamend}
        />
        <span class="check" data-state={amend ? "on" : "off"}></span>
        Amend
      </label>
      <button
        bind:this={historyButton}
        type="button"
        class="action"
        title="Commit Message History (⌃M)"
        aria-label="Commit Message History"
        disabled={busy}
        onclick={showHistory}
      >
        <Icon name="history" />
      </button>
    </div>
    <div class="legend">
      {#each legend as item (item.key)}
        <span style:color={LEGEND_COLORS[item.key]}>{item.text}</span>
      {/each}
    </div>
  </div>
{/if}

<div class="commit-box">
  <div class="editor">
    <div bind:this={backdrop} class="backdrop" aria-hidden="true">
      <div class="margin"></div>
      <div class="mirror">{#each segments as segment}{#if segment.marked}<mark data-testid="message-issue">{segment.text}</mark>{:else}{segment.text}{/if}{/each}</div>
    </div>
    <textarea
      bind:this={field}
      bind:value={message}
      readonly={busy}
      aria-label="Commit Message"
      placeholder="Commit Message"
      title={issues.map((issue) => issue.text).join("\n") || undefined}
      spellcheck="false"
      oninput={oninput}
      onscroll={syncScroll}
    ></textarea>
  </div>
  {#if problem}
    <p class="problem"><Icon name="error" />{problem}</p>
  {/if}
  {#if !editorOnly}
    <div class="buttons">
      <button type="button" class="button" class:default={primary} disabled={busy} onclick={oncommit}>
        {commitLabel}
      </button>
      <button
        type="button"
        class="button"
        title="{pushLabel} (⌥⌘K)"
        disabled={busy || !onpush}
        onclick={onpush}
      >
        {pushLabel}
      </button>
      {#if onrebase}
        <button type="button" class="button" disabled={busy} onclick={onrebase}>
          Commit and Rebase…
        </button>
      {/if}
      <button
        type="button"
        class="action options"
        title="Show Commit Options"
        aria-label="Show Commit Options"
        onclick={onoptions}
      >
        <Icon name="settings" />
      </button>
    </div>
  {/if}
</div>

<style>
  .status {
    display: flex;
    flex: none;
    align-items: center;
    justify-content: space-between;
    height: 30px;
    padding: 0 6px 0 12px;
    background: var(--panel);
  }

  .tools {
    display: flex;
    gap: 8px;
    align-items: center;
  }

  .legend {
    display: flex;
    gap: 12px;
    white-space: nowrap;
  }

  .commit-box {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-height: 0;
    background: var(--editor);
  }

  .editor {
    position: relative;
    flex: 1;
    min-height: 0;
  }

  .backdrop,
  textarea {
    position: absolute;
    inset: 0;
    box-sizing: border-box;
    width: 100%;
    height: 100%;
    margin: 0;
    padding: 6px 6px 6px 12px;
    font: 13px/20px ui-monospace, Menlo, monospace;
    border: 0;
  }

  .mirror,
  textarea {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .backdrop {
    overflow: hidden;
    color: transparent;
    pointer-events: none;
  }

  .margin {
    position: absolute;
    top: 0;
    bottom: 0;
    left: calc(12px + 72ch);
    width: 1px;
    background: var(--right-margin);
  }

  .mirror {
    position: relative;
  }

  mark {
    color: transparent;
    background: none;
    text-decoration: underline wavy var(--editor-warning) 1px;
    text-decoration-skip-ink: none;
    text-underline-offset: 2px;
  }

  textarea {
    overflow: auto;
    color: var(--editor-text);
    caret-color: var(--caret);
    resize: none;
    background: transparent;
    outline: none;
  }

  textarea::placeholder {
    color: var(--muted);
  }

  .problem {
    display: flex;
    flex: none;
    gap: 6px;
    align-items: center;
    margin: 0;
    padding: 4px 12px 6px;
    color: var(--text);
  }

  .buttons {
    display: flex;
    flex: none;
    gap: 8px;
    align-items: center;
    padding: 0 6px 6px 12px;
  }

  .options {
    margin-left: auto;
  }
</style>
