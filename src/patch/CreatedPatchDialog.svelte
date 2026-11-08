<script lang="ts">
  import Dialog from "../lib/Dialog.svelte";
  import Icon from "../lib/Icon.svelte";
  import { CLOSE, CREATED_TITLE, DONT_ASK, REVEAL, createdText, type CreatedAnswer } from "./flow";

  // `CreatePatchCommitExecutor.showDialog`: `MessageDialogBuilder.okCancel` with the information icon,
  // `Reveal in Finder` as the default button and `Don't ask again`, which counts with either button
  // and with closing the dialog (`shouldSaveOptionsOnCancel`).
  interface Props {
    path: string;
    onanswer: (answer: CreatedAnswer) => void;
  }

  let { path, onanswer }: Props = $props();

  let dontAsk = $state(false);
</script>

<Dialog
  title={CREATED_TITLE}
  width={440}
  onclose={() => onanswer({ reveal: false, dontAsk })}
  onsubmit={() => onanswer({ reveal: true, dontAsk })}
>
  <div class="message">
    <Icon name="info" size={24} />
    <div>
      <p>{createdText(path)}</p>
      <label class="checkbox-label">
        <input type="checkbox" aria-label={DONT_ASK} bind:checked={dontAsk} />
        <span class="check" data-state={dontAsk ? "on" : "off"}></span>
        <span>{DONT_ASK}</span>
      </label>
    </div>
  </div>
  {#snippet footer()}
    <button type="button" class="button" onclick={() => onanswer({ reveal: false, dontAsk })}>
      {CLOSE}
    </button>
    <button class="button default" data-autofocus>{REVEAL}</button>
  {/snippet}
</Dialog>

<style>
  .message {
    display: flex;
    gap: 12px;
    align-items: flex-start;
  }

  .message > div {
    min-width: 0;
  }

  p {
    margin: 0 0 10px;
    overflow-wrap: anywhere;
    user-select: text;
  }
</style>
