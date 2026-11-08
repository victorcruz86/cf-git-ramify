<script lang="ts">
  import Dialog from "../lib/Dialog.svelte";
  import Icon from "../lib/Icon.svelte";
  import type { DropAnswer } from "./editing";

  // `GitDropLogAction`: `MessageDialogBuilder.okCancel(...).asWarning().doNotAsk(...)`. OK is the default
  // button; Escape and Cancel answer no, and the checkbox only counts with OK.
  interface Props {
    count: number;
    onanswer: (answer: DropAnswer) => void;
  }

  let { count, onanswer }: Props = $props();

  // `dialog.options.do.not.ask`, without the mnemonic.
  const DO_NOT_ASK = "Don't ask again";
  let doNotAsk = $state(false);
</script>

<Dialog
  title={count === 1 ? "Drop Commit" : "Drop Commits"}
  width={380}
  onclose={() => onanswer({ ok: false, doNotAsk: false })}
  onsubmit={() => onanswer({ ok: true, doNotAsk })}
>
  <div class="warning">
    <Icon name="warning" size={24} />
    <div>
      <p>Do you want to drop {count} {count === 1 ? "commit" : "commits"}?</p>
      <label class="checkbox-label">
        <input type="checkbox" aria-label={DO_NOT_ASK} bind:checked={doNotAsk} />
        <span class="check" data-state={doNotAsk ? "on" : "off"}></span>
        <span>{DO_NOT_ASK}</span>
      </label>
    </div>
  </div>
  {#snippet footer()}
    <button type="button" class="button" onclick={() => onanswer({ ok: false, doNotAsk: false })}>
      Cancel
    </button>
    <button class="button default" data-autofocus>OK</button>
  {/snippet}
</Dialog>

<style>
  .warning {
    display: flex;
    gap: 12px;
    align-items: flex-start;
  }

  .warning > div {
    min-width: 0;
  }

  p {
    margin: 0 0 10px;
  }
</style>
