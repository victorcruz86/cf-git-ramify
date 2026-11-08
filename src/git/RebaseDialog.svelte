<script lang="ts">
  import { untrack } from "svelte";
  import type { RebaseFlag } from "../lib/api";
  import Dialog from "../lib/Dialog.svelte";
  import type { RebaseChoice } from "./flow";
  import {
    REBASE_OPTIONS,
    rebaseFlagText,
    rebaseOptionsCompatible,
    type GitVersion,
    type RebaseOption,
  } from "./options";

  // `GitRebaseDialog`, without the repository selector. `upstream` is the remembered base if git still
  // resolves it. `validate` says which of these revisions git resolves: it is only asked on accepting.
  interface Props {
    branches: string[];
    refs: string[];
    upstream: string;
    options: RebaseFlag[];
    version: GitVersion | null;
    validate: (revisions: string[]) => Promise<boolean[]>;
    onanswer: (choice: RebaseChoice | null) => void;
  }

  let { branches, refs, upstream: initial, options, version, validate, onanswer }: Props = $props();

  // `REBASE_FLAGS`: what comes out as a button. The rest of the options have their own field.
  const FLAGS: RebaseOption[] = [
    "rebaseMerges",
    "keepEmpty",
    "root",
    "interactive",
    "updateRefs",
  ];

  let chosen = $state<RebaseOption[]>(untrack(() => [...options]));
  let upstream = $state(untrack(() => initial));
  let onto = $state("");
  let source = $state("");
  let showOptions = $state(false);
  let touched = $state(false);
  let submitted = $state(false);
  let checking = $state(false);
  let badRevisions = $state<("upstream" | "onto")[]>([]);

  const has = (option: RebaseOption) => chosen.includes(option);
  const compatible = $derived(rebaseOptionsCompatible(chosen, version));
  const shown = $derived(chosen.filter((option) => FLAGS.includes(option)));

  // `validateUpstream` and `validateOnto`: without them it cannot be accepted.
  const required = $derived.by(() => {
    const list: { field: "upstream" | "onto"; text: string }[] = [];
    if (has("root")) return list;
    if (!upstream.trim()) {
      list.push({
        field: "upstream",
        text: has("onto") ? "Select an upstream" : "Select a new base",
      });
    }
    if (has("onto") && !onto.trim()) list.push({ field: "onto", text: "Select a new base" });
    return list;
  });

  // `validateBranch`.
  const branchProblem = $derived.by(() => {
    if (!has("switchBranch")) return null;
    if (!source) return "Select a branch to switch to";
    return branches.includes(source) ? null : `Branch '${source}' does not exist`;
  });

  const problems = $derived([
    ...(touched ? required.map(({ text }) => text) : []),
    ...(submitted && branchProblem ? [branchProblem] : []),
    ...badRevisions.map(() => "There is no such branch or tag"),
  ]);

  function toggle(id: RebaseOption) {
    touched = true;
    if (has(id)) {
      chosen = chosen.filter((option) => option !== id);
      // `moveNewBaseValue`: what was typed moves from one field to the other.
      if (id === "onto") {
        upstream = onto;
        onto = "";
      }
    } else if (compatible.has(id)) {
      chosen = [...chosen, id];
      if (id === "onto") {
        onto = upstream;
        upstream = "";
      }
    }
    badRevisions = [];
  }

  function typed(setter: (value: string) => void) {
    return (event: Event & { currentTarget: HTMLInputElement }) => {
      touched = true;
      badRevisions = [];
      setter(event.currentTarget.value);
    };
  }

  async function missing(): Promise<("upstream" | "onto")[]> {
    const asked: ["upstream" | "onto", string][] = [];
    if (!has("root") && upstream.trim()) asked.push(["upstream", upstream.trim()]);
    if (has("onto") && onto.trim()) asked.push(["onto", onto.trim()]);
    if (asked.length === 0) return [];
    try {
      const valid = await validate(asked.map(([, name]) => name));
      return asked.filter((_, index) => !valid[index]).map(([field]) => field);
    } catch {
      // If it cannot be asked, git decides when launching the rebase.
      return [];
    }
  }

  async function submit() {
    submitted = true;
    touched = true;
    if (required.length > 0 || branchProblem || checking) return;
    checking = true;
    try {
      badRevisions = await missing();
      if (badRevisions.length > 0) return;
    } finally {
      checking = false;
    }
    onanswer({
      params: {
        upstream: has("root") ? null : upstream.trim(),
        onto: has("onto") ? onto.trim() || null : null,
        branch: has("switchBranch") ? source : null,
        interactive: has("interactive"),
        rebaseMerges: has("rebaseMerges"),
        keepEmpty: has("keepEmpty"),
        root: has("root"),
        updateRefs: has("updateRefs"),
      },
      // `saveSettings`: the new base if there is `--onto`, and otherwise the upstream, even if `--root` turns it off.
      newBase: (has("onto") ? onto : upstream).trim(),
    });
  }
</script>

<Dialog title="Rebase" onclose={() => onanswer(null)} onsubmit={submit} width={560}>
  <div class="command">
    <span class="label">git rebase</span>
    {#if has("onto")}
      <span class="label onto">--onto</span>
      <input
        class="input field"
        aria-label="New base"
        list="rebase-refs"
        placeholder="new base (branch or hash)"
        value={onto}
        oninput={typed((value) => (onto = value))}
      />
    {/if}
    <input
      class="input field"
      aria-label="Upstream"
      list="rebase-refs"
      placeholder={has("onto") ? "old base (branch or hash)" : "branch or hash"}
      data-autofocus
      disabled={has("root")}
      value={upstream}
      oninput={typed((value) => (upstream = value))}
    />
    {#if has("switchBranch")}
      <input
        class="input field"
        aria-label="Source branch"
        list="rebase-branches"
        placeholder="source branch"
        value={source}
        oninput={typed((value) => (source = value))}
      />
    {/if}
    <datalist id="rebase-refs">
      {#each refs as name (name)}
        <option value={name}></option>
      {/each}
    </datalist>
    <datalist id="rebase-branches">
      {#each branches as name (name)}
        <option value={name}></option>
      {/each}
    </datalist>
  </div>
  {#each problems as text}
    <p class="field-error" role="alert">{text}</p>
  {/each}
  {#if shown.length > 0}
    <div class="chosen">
      {#each shown as id (id)}
        <span class="chip">
          {rebaseFlagText(id, version)}
          <button
            type="button"
            aria-label="Remove {rebaseFlagText(id, version)}"
            onclick={() => toggle(id)}>×</button
          >
        </span>
      {/each}
    </div>
  {/if}
  {#if showOptions}
    <div class="menu" role="group" aria-label="Add Rebase Options">
      <h3>Add Rebase Options</h3>
      {#each REBASE_OPTIONS as { id, description, separatorAbove } (id)}
        {@const flag = rebaseFlagText(id, version)}
        {#if separatorAbove}
          <hr />
        {/if}
        <label class="checkbox-label" class:disabled={!compatible.has(id)}>
          <input
            type="checkbox"
            aria-label={flag || description}
            checked={has(id)}
            disabled={!compatible.has(id)}
            onchange={() => toggle(id)}
          />
          <span class="check" data-state={has(id) ? "on" : "off"}></span>
          {#if flag}
            <span class="flag">{flag}</span>
          {/if}
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
    <button class="button default" disabled={required.length > 0 || checking}>Rebase</button>
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

  .onto {
    padding-left: 12px;
  }

  .field {
    flex: 1;
    min-width: 0;
    margin-right: 6px;
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

  .menu hr {
    width: 100%;
    margin: 2px 0;
    border: 0;
    border-top: 1px solid var(--border);
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
