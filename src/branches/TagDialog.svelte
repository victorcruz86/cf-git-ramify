<script lang="ts">
  import Dialog from "../lib/Dialog.svelte";
  import type { TagChoice } from "./actions";

  // With `hash`, the question from the log; without it, the `Tag` dialog of the Git menu.
  interface Props {
    hash: string | null;
    onanswer: (choice: TagChoice | null) => void;
  }

  let { hash, onanswer }: Props = $props();

  let name = $state("");
  let commit = $state("");
  let message = $state("");
  let force = $state(false);
  const valid = $derived(name !== "" && !/\s/.test(name));

  function submit() {
    if (!valid) return;
    onanswer({ name, target: hash ?? (commit.trim() || "HEAD"), message, force });
  }
</script>

<Dialog
  title={hash ? `Create New Tag On ${hash}` : "Tag"}
  onclose={() => onanswer(null)}
  onsubmit={submit}
  width={hash ? 520 : 460}
>
  <label class="field">
    {hash ? "Enter the name of new tag" : "Tag Name:"}
    <input class="input" data-autofocus bind:value={name} />
  </label>
  {#if !hash}
    <label class="checkbox-label">
      <input type="checkbox" bind:checked={force} />
      <span class="check" data-state={force ? "on" : "off"}></span>
      Force
    </label>
    <label class="field">
      Commit:
      <input class="input" bind:value={commit} placeholder="HEAD" />
    </label>
    <label class="field">
      Message:
      <textarea class="input" rows="5" bind:value={message}></textarea>
    </label>
  {/if}
  {#snippet footer()}
    <button type="button" class="button" onclick={() => onanswer(null)}>Cancel</button>
    <button class="button default" disabled={!valid}>{hash ? "OK" : "Create Tag"}</button>
  {/snippet}
</Dialog>

<style>
  .field {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-bottom: 10px;
  }

  textarea {
    height: auto;
    resize: vertical;
  }
</style>
