<script lang="ts">
  import { onMount } from "svelte";
  import {
    editorAnswer,
    onEditorDismissed,
    onEditorRequest,
    type EditorRequest,
  } from "../lib/api";
  import Dialog from "../lib/Dialog.svelte";
  import InteractiveRebaseDialog from "../rebase/InteractiveRebaseDialog.svelte";
  import type { RebaseCommit } from "../rebase/model";
  import { EDITOR_OK, EDITOR_TITLE, NOOP_TEXT, NOOP_TITLE, rootLabel } from "./editor";

  // `GitUnstructuredEditor`: git asks for a message (a `reword`) or a rebase list that has no
  // window of its own, and what is typed here goes back to the file git is waiting for. A list that is
  // understood goes to the `Rebasing Commits` dialog, and a `noop`, to its question
  // (`GitInteractiveRebaseEditorHandler`). Each request is handled in turn.
  interface Props {
    // The root of the window's repository, which is that of every task.
    root: string;
  }

  let { root }: Props = $props();

  let queue = $state.raw<EditorRequest[]>([]);
  let text = $state("");

  const current = $derived(queue[0] ?? null);

  // The list that the core sends; if it cannot be read, there is no dialog to show and it is cancelled.
  function listOf(text: string): RebaseCommit[] | null {
    try {
      const parsed: unknown = JSON.parse(text);
      return Array.isArray(parsed) ? (parsed as RebaseCommit[]) : null;
    } catch {
      return null;
    }
  }

  function advance() {
    queue = queue.slice(1);
    text = queue[0]?.text ?? "";
  }

  function reply(answer: string | null) {
    if (current) editorAnswer(current.id, answer);
    advance();
  }

  const commits = $derived(current?.form === "commits" ? listOf(current.text) : null);

  function asked(request: EditorRequest) {
    if (request.form === "commits" && !listOf(request.text)) {
      editorAnswer(request.id, null);
      return;
    }
    if (queue.length === 0) text = request.text;
    queue = [...queue, request];
  }

  // Git's editor no longer waits: what was typed has nowhere to go.
  function dismissed(id: number) {
    if (current?.id === id) advance();
    else queue = queue.filter((request) => request.id !== id);
  }

  onMount(() => {
    const stops = [onEditorRequest(asked), onEditorDismissed(dismissed)];
    return () => {
      stops.forEach((stop) => stop.then((unlisten) => unlisten()));
    };
  });
</script>

{#if current?.form === "commits" && commits}
  {#key current.id}
    <InteractiveRebaseDialog {commits} onanswer={(plan) => reply(plan ? JSON.stringify(plan) : null)} />
  {/key}
{:else if current?.form === "noop"}
  {#key current.id}
    <Dialog title={NOOP_TITLE} width={480} onclose={() => reply(null)} onsubmit={() => reply("")}>
      <p class="question">{NOOP_TEXT}</p>
      {#snippet footer()}
        <button type="button" class="button" onclick={() => reply(null)}>Cancel</button>
        <button class="button default" data-autofocus>OK</button>
      {/snippet}
    </Dialog>
  {/key}
{:else if current}
  {#key current.id}
    <Dialog title={EDITOR_TITLE} width={640} onclose={() => reply(null)} onsubmit={() => reply(text)}>
      <p class="root">{rootLabel(root)}</p>
      <textarea
        class="input"
        rows="14"
        data-autofocus
        autocomplete="off"
        autocapitalize="off"
        spellcheck="false"
        aria-label="Commit message"
        bind:value={text}
      ></textarea>

      {#snippet footer()}
        <button type="button" class="button" onclick={() => reply(null)}>Cancel</button>
        <button class="button default">{EDITOR_OK}</button>
      {/snippet}
    </Dialog>
  {/key}
{/if}

<style>
  .question {
    margin: 0;
    white-space: pre-line;
  }

  .root {
    margin: 0 0 8px;
    overflow-wrap: anywhere;
  }

  textarea {
    box-sizing: border-box;
    width: 100%;
    height: auto;
    padding: 6px 8px;
    font: 13px/20px ui-monospace, Menlo, monospace;
    resize: vertical;
  }
</style>
