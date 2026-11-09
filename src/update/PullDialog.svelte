<script lang="ts">
  import { untrack } from "svelte";
  import type { PullInfo, PullOption } from "../lib/api";
  import Dialog from "../lib/Dialog.svelte";
  import { notify } from "../lib/notifications";
  import type { PullChoice } from "./flow";
  import {
    PULL_OPTIONS,
    branchesOf,
    chooseBranch,
    chooseRemote,
    incompatible,
    pullTitle,
    selectionProblem,
  } from "./pullOptions";

  // `GitPullDialog`, without the repository selector: there is one per window. `reload` fetches the
  // remote and brings the new branches; `null` if it failed.
  interface Props {
    info: PullInfo;
    options: PullOption[];
    reload: (remote: string) => Promise<PullInfo | null>;
    onanswer: (choice: PullChoice | null) => void;
  }

  let { info, options, reload, onanswer }: Props = $props();

  const first = untrack(() => info);
  const firstRemote = chooseRemote(first) ?? "";
  let current = $state.raw(first);
  let remote = $state(firstRemote);
  let branch = $state(chooseBranch(first, firstRemote || null, null));
  let chosen = $state<PullOption[]>(untrack(() => [...options]));
  let showOptions = $state(false);
  let submitted = $state(false);
  let reloading = false;

  const names = $derived(branchesOf(current, remote || null));
  const problem = $derived(selectionProblem(current, remote || null, branch));
  const grey = $derived(incompatible(chosen));
  const flags = Object.fromEntries(PULL_OPTIONS.map(({ id, flag }) => [id, flag]));

  function remoteChanged(event: Event & { currentTarget: HTMLSelectElement }) {
    remote = event.currentTarget.value;
    branch = chooseBranch(current, remote || null, branch);
  }

  function toggle(id: PullOption) {
    if (chosen.includes(id)) chosen = chosen.filter((option) => option !== id);
    else if (!grey.has(id)) chosen = [...chosen, id];
  }

  async function refresh() {
    if (!remote) {
      notify({
        kind: "error",
        title: "Cannot load remote branches",
        body: "Select a local and a remote repository",
      });
      return;
    }
    if (reloading) return;
    reloading = true;
    try {
      const fresh = await reload(remote);
      if (fresh) {
        current = fresh;
        branch = chooseBranch(fresh, remote, branch);
      }
    } finally {
      reloading = false;
    }
  }

  function branchKey(event: KeyboardEvent) {
    if (event.metaKey && event.key.toLowerCase() === "r") {
      event.preventDefault();
      refresh();
    }
  }

  function submit() {
    submitted = true;
    if (problem) return;
    const options = PULL_OPTIONS.map(({ id }) => id).filter((id) => chosen.includes(id));
    onanswer({ remote, branch, options });
  }
</script>

<Dialog title={pullTitle(current.branch)} onclose={() => onanswer(null)} onsubmit={submit} width={560}>
  <div class="command">
    <span class="label">git pull</span>
    <select class="input remote" aria-label="Remote" value={remote} onchange={remoteChanged}>
      {#if current.remotes.length === 0}
        <option value="">None</option>
      {/if}
      {#each current.remotes as name (name)}
        <option value={name}>{name}</option>
      {/each}
    </select>
    <input
      class="input branch"
      aria-label="Branch"
      list="pull-branches"
      placeholder="Specify branch"
      data-autofocus
      value={branch}
      oninput={(event) => (branch = event.currentTarget.value)}
      onkeydown={branchKey}
    />
    <datalist id="pull-branches">
      {#each names as name (name)}
        <option value={name}></option>
      {/each}
    </datalist>
  </div>
  <p class="hint">Press ⌘R to update branches</p>
  {#if submitted && problem}
    <p class="field-error" role="alert">{problem.text}</p>
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
    <div class="menu" role="group" aria-label="Add Pull Options">
      <h3>Add Pull Options</h3>
      {#each PULL_OPTIONS as { id, flag, description } (id)}
        <label class="checkbox-label" class:disabled={grey.has(id)}>
          <input
            type="checkbox"
            aria-label={flag}
            checked={chosen.includes(id)}
            disabled={grey.has(id)}
            onchange={() => toggle(id)}
          />
          <span class="check" data-state={chosen.includes(id) ? "on" : "off"}></span>
          <span class="flag">{flag}</span>
          <span class="description">{description}</span>
        </label>
      {/each}
    </div>
  {/if}
  {#snippet footer()}
    <button type="button" class="modify" onclick={() => (showOptions = !showOptions)}>
      Modify options
    </button>
    <button type="button" class="button" onclick={() => onanswer(null)}>Cancel</button>
    <button class="button default">Pull</button>
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

  .remote {
    flex: none;
    width: 110px;
    margin-right: 6px;
  }

  .branch {
    flex: 1;
    min-width: 0;
  }

  .hint {
    margin: 4px 0 0;
    font-size: 11px;
    color: var(--text-secondary);
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

  .modify {
    margin-right: auto;
    font: inherit;
    color: var(--link, var(--focus));
    cursor: pointer;
    background: none;
    border: 0;
  }
</style>
