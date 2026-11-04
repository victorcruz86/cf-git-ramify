<script lang="ts">
  import { untrack } from "svelte";
  import type { UpdateMethod } from "../lib/api";
  import Dialog from "../lib/Dialog.svelte";
  import { UPDATE_METHODS } from "./methods";
  import type { TrackedChoice } from "./flow";

  // `FixTrackedBranchDialog`, without the repository selector: there is one per window.
  interface Props {
    branch: string;
    remoteBranches: string[];
    method: UpdateMethod;
    onanswer: (choice: TrackedChoice | null) => void;
  }

  let { branch, remoteBranches, method, onanswer }: Props = $props();

  const split = (name: string): [string, string] => {
    const slash = name.indexOf("/");
    return [name.slice(0, slash), name.slice(slash + 1)];
  };
  const pairs = $derived(remoteBranches.map(split));
  const remotes = $derived([...new Set(pairs.map(([remote]) => remote))]);
  // `getBranchMatchingLocal`.
  const matching = untrack(() => remoteBranches.map(split).find(([, name]) => name === branch));

  let remote = $state(untrack(() => matching?.[0] ?? split(remoteBranches[0] ?? "/")[0]));
  let name = $state(matching?.[1] ?? "");
  let setUpstream = $state(false);
  let chosen = $state(untrack(() => method));
  const names = $derived(pairs.filter(([owner]) => owner === remote).map(([, name]) => name));

  function submit() {
    const trimmed = name.trim();
    if (trimmed) onanswer({ target: `${remote}/${trimmed}`, setUpstream, method: chosen });
  }
</script>

<Dialog title="Update" onclose={() => onanswer(null)} onsubmit={submit} width={460}>
  <div class="row">
    <select class="input" aria-label="Remote" bind:value={remote}>
      {#each remotes as option (option)}
        <option value={option}>{option}</option>
      {/each}
    </select>
    <input
      class="input branch"
      aria-label="Branch"
      list="fix-tracked-branches"
      placeholder="Specify branch"
      data-autofocus
      bind:value={name}
    />
    <datalist id="fix-tracked-branches">
      {#each names as option (option)}
        <option value={option}></option>
      {/each}
    </datalist>
  </div>
  <label class="checkbox-label tracked">
    <input type="checkbox" bind:checked={setUpstream} />
    <span class="check" data-state={setUpstream ? "on" : "off"}></span>
    Set as tracked branch
  </label>
  <div class="methods" role="radiogroup" aria-label="Update method">
    {#each UPDATE_METHODS as { id, label } (id)}
      <label class="method">
        <input type="radio" name="fix-tracked-method" value={id} bind:group={chosen} />
        {label}
      </label>
    {/each}
  </div>
  {#snippet footer()}
    <button type="button" class="button" onclick={() => onanswer(null)}>Cancel</button>
    <button class="button default">Update</button>
  {/snippet}
</Dialog>

<style>
  .row {
    display: flex;
    gap: 8px;
  }

  .branch {
    flex: 1;
    min-width: 0;
  }

  .tracked {
    margin-top: 10px;
  }

  .methods {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-top: 10px;
  }

  .method {
    display: flex;
    gap: 6px;
    align-items: center;
  }
</style>
