<script lang="ts">
  import Dialog from "../lib/Dialog.svelte";

  interface Props {
    author: string;
    signOff: boolean;
    runHooks: boolean;
    onclose: () => void;
  }

  let {
    author = $bindable(),
    signOff = $bindable(),
    runHooks = $bindable(),
    onclose,
  }: Props = $props();
</script>

<Dialog title="Commit Options" width={340} popup={{ right: 12, bottom: 44 }} {onclose}>
  <div class="options">
    <label class="author">
      Author:
      <input class="input" data-autofocus bind:value={author} />
    </label>
    <label class="checkbox-label">
      <input type="checkbox" bind:checked={signOff} />
      <span class="check" data-state={signOff ? "on" : "off"}></span>
      Sign-off commit
    </label>
    <label class="checkbox-label" title="If unchecked, Git hooks will be skipped with the '--no-verify' parameter">
      <input type="checkbox" bind:checked={runHooks} />
      <span class="check" data-state={runHooks ? "on" : "off"}></span>
      Run Git hooks
    </label>
  </div>
</Dialog>

<style>
  .options {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding-bottom: 8px;
  }

  .author {
    display: flex;
    gap: 8px;
    align-items: center;
  }

  .author input {
    flex: 1;
  }
</style>
