<script lang="ts">
  import { untrack } from "svelte";
  import Dialog from "../lib/Dialog.svelte";
  import type { BranchChoice } from "./actions";
  import { branchProblem, cleanName, cleanOnTyping, overwriteEnabled, type BranchNames } from "./refName";

  interface Props {
    title: string;
    initialName: string;
    names: BranchNames;
    showCheckout: boolean;
    showReset: boolean;
    okLabel: string;
    onanswer: (choice: BranchChoice | null) => void;
  }

  let { title, initialName, names, showCheckout, showReset, okLabel, onanswer }: Props = $props();

  let value = $state(untrack(() => initialName));
  let checkout = $state(true);
  let overwrite = $state(false);
  let applied = $state(false);
  const canOverwrite = $derived(overwriteEnabled(value, names));
  const replaces = $derived(overwrite && canOverwrite);
  const problem = $derived(
    branchProblem(value, names, { onApply: applied, overwrite: replaces, showReset }),
  );

  function typed(event: Event & { currentTarget: HTMLInputElement }) {
    value = cleanOnTyping(event.currentTarget.value);
    event.currentTarget.value = value;
    applied = false;
  }

  function submit() {
    applied = true;
    const failed = branchProblem(value, names, { onApply: true, overwrite: replaces, showReset });
    if (!failed) onanswer({ name: cleanName(value), checkout, overwrite: replaces });
  }
</script>

<Dialog {title} onclose={() => onanswer(null)} onsubmit={submit} width={420}>
  <label class="field">
    New branch name:
    <input class="input" data-autofocus value={value} oninput={typed} onfocus={(event) => event.currentTarget.select()} />
  </label>
  {#if problem}
    <p class="field-error">{problem}</p>
  {/if}
  <div class="options">
    {#if showCheckout}
      <label class="checkbox-label">
        <input type="checkbox" bind:checked={checkout} />
        <span class="check" data-state={checkout ? "on" : "off"}></span>
        Checkout branch
      </label>
    {/if}
    {#if showReset}
      <label class="checkbox-label" class:disabled={!canOverwrite}>
        <input type="checkbox" bind:checked={overwrite} disabled={!canOverwrite} />
        <span class="check" data-state={replaces ? "on" : "off"}></span>
        Overwrite existing branch
      </label>
    {/if}
  </div>
  {#snippet footer()}
    <button type="button" class="button" onclick={() => onanswer(null)}>Cancel</button>
    <button class="button default" disabled={problem !== null}>{okLabel}</button>
  {/snippet}
</Dialog>

<style>
  .field {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .field-error {
    white-space: pre-line;
  }

  .options {
    display: flex;
    gap: 16px;
    margin-top: 10px;
  }
</style>
