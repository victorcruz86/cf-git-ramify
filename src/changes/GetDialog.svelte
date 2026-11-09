<script lang="ts">
  import Dialog from "../lib/Dialog.svelte";
  import Icon from "../lib/Icon.svelte";
  import { GET_CONTENT_TITLE, GET_TITLE, answerGetQuestion, getQuestion } from "./get";

  // The questions of `getRevision` (src/changes/get.ts), wherever the Get started: the Changes
  // window, file history or the log. The application mounts this once.
  const request = $derived($getQuestion?.request);
</script>

{#if request?.kind === "confirm"}
  <!-- `ReplaceFileConfirmationDialog`: `showOkCancelDialog` with the warning icon. -->
  <Dialog
    title={request.title ?? GET_TITLE}
    width={420}
    onclose={() => answerGetQuestion(false)}
    onsubmit={() => answerGetQuestion(true)}
  >
    <div class="message">
      <Icon name="warning" size={24} />
      <p>{request.message}</p>
    </div>
    {#snippet footer()}
      <button type="button" class="button" onclick={() => answerGetQuestion(false)}>Cancel</button>
      <button class="button default" data-autofocus>{request.ok}</button>
    {/snippet}
  </Dialog>
{:else if request?.kind === "error"}
  <!-- `MyWriteVersionTask.run`: `showMessageDialog` titled `Get Revision Content`. -->
  <Dialog
    title={request.title ?? GET_CONTENT_TITLE}
    width={440}
    onclose={() => answerGetQuestion(true)}
    onsubmit={() => answerGetQuestion(true)}
  >
    <div class="message">
      <Icon name={request.icon === "error" ? "error" : "info"} size={24} />
      <p>{request.message}</p>
    </div>
    {#snippet footer()}
      <button class="button default" data-autofocus>OK</button>
    {/snippet}
  </Dialog>
{/if}

<style>
  .message {
    display: flex;
    gap: 12px;
    align-items: flex-start;
  }

  p {
    margin: 0;
    white-space: pre-line;
    overflow-wrap: anywhere;
    user-select: text;
  }
</style>
