<script lang="ts">
  import { suggestWhileTyping } from "../lib/suggest";
  import { onMount, untrack } from "svelte";
  import type { MergeOption } from "../lib/api";
  import Dialog from "../lib/Dialog.svelte";
  import type { MergeChoice } from "./flow";
  import { mergeOptionsAvailable, mergeOptionsCompatible, type GitVersion } from "./options";

  // `GitMergeDialog`, without the repository selector: there is one per window. `unmerged` arrives separately, as
  // in IntelliJ, and `null` means it could not be known: then there is no warning about what is already merged.
  interface Props {
    current: string | null;
    branches: string[];
    branch: string;
    options: MergeOption[];
    version: GitVersion | null;
    unmerged: Promise<string[] | null>;
    onanswer: (choice: MergeChoice | null) => void;
  }

  let { current, branches, branch: initial, options, version, unmerged, onanswer }: Props = $props();

  let branch = $state(untrack(() => initial));
  let chosen = $state<MergeOption[]>(untrack(() => [...options]));
  let message = $state("");
  let showOptions = $state(false);
  let submitted = $state(false);
  let notMerged = $state.raw<Set<string> | null>(null);

  onMount(() => {
    let alive = true;
    unmerged.then((list) => {
      if (alive) notMerged = list && new Set(list);
    });
    return () => (alive = false);
  });

  const available = $derived(mergeOptionsAvailable(version));
  const compatible = $derived(mergeOptionsCompatible(chosen));
  const flags = $derived(Object.fromEntries(available.map(({ id, flag }) => [id, flag])));
  const title = $derived(current ? `Merge into ${current}` : "Merge Changes");

  // `validateBranchField`.
  const problem = $derived.by(() => {
    if (!branch) return "The merge branch should be selected.";
    if (!branches.includes(branch)) return "There is no matching branch.";
    if (notMerged && !notMerged.has(branch)) return `Branch '${branch}' is already merged`;
    return null;
  });

  function toggle(id: MergeOption) {
    if (chosen.includes(id)) {
      chosen = chosen.filter((option) => option !== id);
      // `updateCommitMessagePanel`: without `-m` the text is cleared.
      if (id === "message") message = "";
    } else if (compatible.has(id)) {
      chosen = [...chosen, id];
    }
  }

  function submit() {
    submitted = true;
    if (problem) return;
    onanswer({ branch, options: [...chosen], message: chosen.includes("message") ? message : "" });
  }
</script>

<Dialog {title} onclose={() => onanswer(null)} onsubmit={submit} width={560}>
  <div class="command">
    <span class="label">git merge</span>
    <input
      class="input branch"
      aria-label="Branch"
      use:suggestWhileTyping={"merge-branches"}
      placeholder="Specify branch"
      data-autofocus
      value={branch}
      oninput={(event) => (branch = event.currentTarget.value)}
    />
    <datalist id="merge-branches">
      {#each branches as name (name)}
        <option value={name}></option>
      {/each}
    </datalist>
  </div>
  {#if submitted && problem}
    <p class="field-error" role="alert">{problem}</p>
  {/if}
  {#if chosen.length > 0}
    <div class="chosen">
      {#each chosen as id (id)}
        <span class="chip">
          {flags[id]}
          <button type="button" aria-label="Remove {flags[id]}" onclick={() => toggle(id)}>×</button>
        </span>
      {/each}
    </div>
  {/if}
  {#if showOptions}
    <div class="menu" role="group" aria-label="Add Merge Options">
      <h3>Add Merge Options</h3>
      {#each available as { id, flag, description } (id)}
        <label class="checkbox-label" class:disabled={!compatible.has(id)}>
          <input
            type="checkbox"
            aria-label={flag}
            checked={chosen.includes(id)}
            disabled={!compatible.has(id)}
            onchange={() => toggle(id)}
          />
          <span class="check" data-state={chosen.includes(id) ? "on" : "off"}></span>
          <span class="flag">{flag}</span>
          <span class="description">{description}</span>
        </label>
      {/each}
    </div>
  {/if}
  {#if chosen.includes("message")}
    <label class="message">
      Commit message:
      <textarea class="input" rows="4" bind:value={message}></textarea>
    </label>
  {/if}
  {#snippet footer()}
    <button type="button" class="modify" onclick={() => (showOptions = !showOptions)}>
      Modify options
    </button>
    <button type="button" class="button" onclick={() => onanswer(null)}>Cancel</button>
    <button class="button default">Merge</button>
  {/snippet}
</Dialog>

<style>
  .command {
    display: flex;
    align-items: center;
  }

  .label {
    padding: 0 8px;
    font-family: var(--mono, monospace);
    color: var(--text-secondary);
    white-space: nowrap;
  }

  .branch {
    flex: 1;
    min-width: 0;
  }

  .chosen {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 10px;
  }

  .chip {
    display: inline-flex;
    gap: 4px;
    align-items: center;
    padding: 1px 6px;
    font-family: var(--mono, monospace);
    border: 1px solid var(--border);
    border-radius: 4px;
  }

  .chip button {
    padding: 0 2px;
    font: inherit;
    color: var(--text-secondary);
    cursor: pointer;
    background: none;
    border: 0;
  }

  .menu {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 8px 10px;
    margin-top: 10px;
    border: 1px solid var(--border);
    border-radius: 4px;
  }

  .menu h3 {
    margin: 0 0 2px;
    font-size: 12px;
    font-weight: 600;
  }

  .flag {
    font-family: var(--mono, monospace);
  }

  .description {
    color: var(--text-secondary);
  }

  .message {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-top: 10px;
  }

  textarea {
    resize: vertical;
    font: inherit;
  }

  .modify {
    margin-right: auto;
    font: inherit;
    color: var(--link, var(--focus));
    cursor: pointer;
    background: none;
    border: 0;
  }
</style>
