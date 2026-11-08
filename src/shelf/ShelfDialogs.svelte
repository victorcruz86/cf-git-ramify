<script lang="ts">
  import Dialog from "../lib/Dialog.svelte";
  import Icon from "../lib/Icon.svelte";
  import { answerShelfDialog, shelfDialog } from "./dialogs";
  import UnshelveListDialog from "./UnshelveListDialog.svelte";

  // The questions of the Shelf tab (`src/shelf/dialogs.ts`).
  const request = $derived($shelfDialog?.request);
</script>

{#if request?.kind === "confirm"}
  <!-- `Messages.showYesNoDialog` with the question icon. -->
  <Dialog
    title={request.title}
    width={440}
    onclose={() => answerShelfDialog(false)}
    onsubmit={() => answerShelfDialog(true)}
  >
    <div class="message">
      <Icon name="info" size={24} />
      <p>{request.message}</p>
    </div>
    {#snippet footer()}
      <button type="button" class="button" onclick={() => answerShelfDialog(false)}>No</button>
      <button class="button default" data-autofocus>Yes</button>
    {/snippet}
  </Dialog>
{:else if request?.kind === "chooseList"}
  {#key request}
    <UnshelveListDialog
      lists={request.lists}
      names={request.names}
      removeApplied={request.removeApplied}
      onanswer={answerShelfDialog}
    />
  {/key}
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
