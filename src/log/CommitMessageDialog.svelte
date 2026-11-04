<script lang="ts">
  import { onMount, untrack } from "svelte";
  import CommitMessage from "../commit/CommitMessage.svelte";
  import Dialog from "../lib/Dialog.svelte";
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

  async function submit() {
    if (checking) return;
    checking = true;
    try {
      problem = await request.validate();
    } finally {
      checking = false;
    }
    if (problem === null) onanswer(message);
  }

  // ⌘⏎ accepts, as in IntelliJ's editor.
  function keydown(event: KeyboardEvent) {
    if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
      event.preventDefault();
      void submit();
    }
  }

  onMount(() => editor?.focusStart());
</script>

<Dialog title={request.title} width={600} onclose={() => onanswer(null)} onsubmit={submit}>
  <p class="label">{request.label}</p>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="message" onkeydown={keydown} oninput={() => (problem = null)}>
    <CommitMessage bind:this={editor} bind:message editorOnly />
  </div>
  {#if problem}
    <p class="field-error">{problem}</p>
  {/if}
  {#snippet footer()}
    <button type="button" class="button" onclick={() => onanswer(null)}>Cancel</button>
    <button class="button default" disabled={checking}>OK</button>
  {/snippet}
</Dialog>

<style>
  .label {
    margin: 0 0 8px;
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
