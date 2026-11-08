<script lang="ts">
  import Dialog from "../lib/Dialog.svelte";
  import Icon from "../lib/Icon.svelte";
  import ApplyPatchDialog from "./ApplyPatchDialog.svelte";
  import CreatedPatchDialog from "./CreatedPatchDialog.svelte";
  import { APPLY_TITLE, OVERWRITE, SAVE_TITLE, answerPatchDialog, overwriteText, patchDialog } from "./flow";
  import OverwriteDialog from "./OverwriteDialog.svelte";
  import PartlyFailedDialog from "./PartlyFailedDialog.svelte";
  import PatchSettingsDialog from "./PatchSettingsDialog.svelte";

  // The dialogs of `patches` and `applies` (src/patch/flow.ts). The application mounts this after
  // the Create Patch dialog, so that Patch File Settings opens on top of it.
  const request = $derived($patchDialog?.request);
</script>

{#if request?.kind === "settings"}
  {#key request}
    <PatchSettingsDialog {...request} onanswer={answerPatchDialog} />
  {/key}
{:else if request?.kind === "overwrite"}
  <!-- `checkIsFileValid`: `Messages.showYesNoDialog` with the warning icon, Overwrite and Cancel. -->
  <Dialog
    title={SAVE_TITLE}
    width={440}
    onclose={() => answerPatchDialog(false)}
    onsubmit={() => answerPatchDialog(true)}
  >
    <div class="message">
      <Icon name="warning" size={24} />
      <p>{overwriteText(request.name, request.parent)}</p>
    </div>
    {#snippet footer()}
      <button type="button" class="button" onclick={() => answerPatchDialog(false)}>Cancel</button>
      <button class="button default" data-autofocus>{OVERWRITE}</button>
    {/snippet}
  </Dialog>
{:else if request?.kind === "created"}
  {#key request}
    <CreatedPatchDialog path={request.path} onanswer={answerPatchDialog} />
  {/key}
{:else if request?.kind === "apply"}
  {#key request}
    <ApplyPatchDialog {...request} onanswer={answerPatchDialog} />
  {/key}
{:else if request?.kind === "overwriteExisting"}
  {#key request}
    <OverwriteDialog repo={request.repo} paths={request.paths} onanswer={answerPatchDialog} />
  {/key}
{:else if request?.kind === "partlyFailed"}
  {#key request}
    <PartlyFailedDialog failed={request.failed} binaries={request.binaries} onanswer={answerPatchDialog} />
  {/key}
{:else if request?.kind === "applyErrors"}
  <!-- `PatchApplier.showError`: the pre-check's errors in one `Apply Patch` error dialog. -->
  <Dialog
    title={APPLY_TITLE}
    width={480}
    onclose={() => answerPatchDialog(true)}
    onsubmit={() => answerPatchDialog(true)}
  >
    <div class="message">
      <Icon name="error" size={24} />
      <p>{request.messages.join("\n")}</p>
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
