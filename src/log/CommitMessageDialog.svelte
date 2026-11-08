<script lang="ts">
  import { onMount, untrack } from "svelte";
  import CommitMessage from "../commit/CommitMessage.svelte";
  import { historyMenu } from "../commit/messageHistory";
  import Dialog from "../lib/Dialog.svelte";
  import Icon from "../lib/Icon.svelte";
  import { matchShortcut } from "../lib/keymap";
  import Menu, { type MenuItem } from "../lib/Menu.svelte";
  import { messagesList, messagesPush } from "../lib/api";
  import type { CommitMessageRequest } from "./editing";

  // `GitNewCommitMessageActionDialog`: the Commit view's message editor, with the label
  // on top. OK first checks that the repository did not change (`doValidate`).
  interface Props {
    request: CommitMessageRequest;
    onanswer: (message: string | null) => void;
  }

  let { request, onanswer }: Props = $props();

  let message = $state(untrack(() => request.message));
  let problem = $state<string | null>(null);
  let checking = $state(false);
  let editor = $state<CommitMessage>();
  let history = $state.raw<string[]>([]);
  let historyButton = $state<HTMLButtonElement>();
  let menu = $state.raw<{ x: number; y: number; items: MenuItem[] } | null>(null);

  // `dispose`: every close saves a changed message to the history (trimmed at the end, as
  // `CommitMessage.getComment`), except an OK that validation refused. The failure of the save is ignored.
  function close(answer: string | null) {
    const comment = message.trimEnd();
    if (comment !== request.message) messagesPush(comment).catch(() => {});
    onanswer(answer);
  }

  function showHistory() {
    if (!historyButton || history.length === 0) return;
    const bounds = historyButton.getBoundingClientRect();
    menu = {
      x: bounds.left,
      y: bounds.bottom + 2,
      items: historyMenu(history, (text) => {
        message = text;
        editor?.focus();
      }),
    };
  }

  async function submit() {
    if (checking) return;
    checking = true;
    try {
      problem = await request.validate();
    } finally {
      checking = false;
    }
    if (problem === null) close(message);
  }

  // ⌘⏎ accepts, as in IntelliJ's editor.
  function keydown(event: KeyboardEvent) {
    if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
      event.preventDefault();
      void submit();
    } else if (matchShortcut(event) === "Vcs.ShowMessageHistory") {
      event.preventDefault();
      showHistory();
    }
  }

  onMount(() => {
    editor?.focusStart();
    messagesList()
      .then((messages) => (history = Array.isArray(messages) ? messages : []))
      .catch(() => {});
  });
</script>

<Dialog title={request.title} width={600} onclose={() => close(null)} onsubmit={submit}>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div onkeydown={keydown}>
    <div class="top">
      <p class="label">{request.label}</p>
      <button
        bind:this={historyButton}
        type="button"
        class="action"
        title="Commit Message History (⌃M)"
        aria-label="Commit Message History"
        disabled={history.length === 0}
        onclick={showHistory}
      >
        <Icon name="history" />
      </button>
    </div>
    <div class="message" oninput={() => (problem = null)}>
      <CommitMessage bind:this={editor} bind:message editorOnly />
    </div>
  </div>
  {#if problem}
    <p class="field-error">{problem}</p>
  {/if}
  {#snippet footer()}
    <button type="button" class="button" onclick={() => close(null)}>Cancel</button>
    <button class="button default" disabled={checking}>OK</button>
  {/snippet}
</Dialog>

{#if menu}
  <Menu {...menu} label="Commit Message History" onclose={() => (menu = null)} />
{/if}

<style>
  .top {
    display: flex;
    gap: 8px;
    align-items: center;
    justify-content: space-between;
    margin: 0 0 8px;
  }

  .label {
    margin: 0;
    overflow-wrap: anywhere;
  }

  .message {
    display: flex;
    flex-direction: column;
    height: 200px;
    border: 1px solid var(--popup-border);
    border-radius: 4px;
    overflow: hidden;
  }
</style>
