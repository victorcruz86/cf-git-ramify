<script lang="ts">
  import Dialog from "../lib/Dialog.svelte";
  import Icon from "../lib/Icon.svelte";
  import { forcePushWarning } from "./force";

  // `ForcePushAction.confirmForcePush`.
  interface Props {
    branch: string;
    onanswer: (answer: { remember: boolean } | null) => void;
  }

  let { branch, onanswer }: Props = $props();

  let remember = $state(false);
</script>

<Dialog
  title="Force Push"
  width={440}
  onclose={() => onanswer(null)}
  onsubmit={() => onanswer({ remember })}
>
  <div class="warning">
    <Icon name="warning" size={24} />
    <p>{forcePushWarning(branch)}</p>
  </div>
  {#snippet footer()}
    <label class="checkbox-label remember">
      <input type="checkbox" aria-label="Don't warn about this target" bind:checked={remember} />
      <span class="check" data-state={remember ? "on" : "off"}></span>
      <span>Don't warn about this target</span>
    </label>
    <button type="button" class="button" onclick={() => onanswer(null)}>Cancel</button>
    <button class="button default" data-autofocus>Force Push</button>
  {/snippet}
</Dialog>

<style>
  .warning {
    display: flex;
    gap: 12px;
    align-items: flex-start;
  }

  p {
    margin: 0;
    overflow-wrap: anywhere;
  }

  .remember {
    margin-right: auto;
  }
</style>
