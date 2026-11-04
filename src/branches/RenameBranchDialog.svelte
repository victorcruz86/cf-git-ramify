<script lang="ts">
  import { untrack } from "svelte";
  import Dialog from "../lib/Dialog.svelte";
  import { branchProblem, cleanName, cleanOnTyping, type BranchNames } from "./refName";

  interface Props {
    branch: string;
    names: BranchNames;
    onanswer: (name: string | null) => void;
  }

  let { branch, names, onanswer }: Props = $props();

  const check = { overwrite: false, showReset: false };
  let value = $state(untrack(() => branch));
  let applied = $state(false);
  const problem = $derived(branchProblem(value, names, { ...check, onApply: applied }));

  function typed(event: Event & { currentTarget: HTMLInputElement }) {
    value = cleanOnTyping(event.currentTarget.value);
    event.currentTarget.value = value;
    applied = false;
  }

  function submit() {
    applied = true;
    if (!branchProblem(value, names, { ...check, onApply: true })) onanswer(cleanName(value));
  }
</script>

<Dialog title="Rename Branch {branch}" onclose={() => onanswer(null)} onsubmit={submit} width={420}>
  <label class="field">
    New branch name:
    <input class="input" data-autofocus value={value} oninput={typed} onfocus={(event) => event.currentTarget.select()} />
  </label>
  {#if problem}
    <p class="field-error">{problem}</p>
  {/if}
  {#snippet footer()}
    <button type="button" class="button" onclick={() => onanswer(null)}>Cancel</button>
    <button class="button default" disabled={problem !== null}>Rename</button>
  {/snippet}
</Dialog>

<style>
  .field {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
</style>
