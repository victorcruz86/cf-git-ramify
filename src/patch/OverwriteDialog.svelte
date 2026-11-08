<script lang="ts">
  import { untrack } from "svelte";
  import Dialog from "../lib/Dialog.svelte";
  import Icon from "../lib/Icon.svelte";
  import { joinPath, OVERWRITE } from "./flow";

  // `Overwrite Existing Files` (`PathsVerifier.DelayedPrecheckContext.doDelayed`,
  // `AbstractVcsHelperImpl.selectFilePathsToProcess`): one file is a question with its path, several
  // a list where each is chosen, all at first. The answer is the paths to write over; Cancel is none.
  interface Props {
    repo: string;
    paths: string[];
    onanswer: (paths: string[]) => void;
  }

  let { repo, paths, onanswer }: Props = $props();

  const TITLE = "Overwrite Existing Files"; // V:367
  // V:368-369.
  const SEVERAL =
    "The following files should be created by patch, but they already exist.\nDo you want to overwrite them?";
  const one = (path: string) =>
    `The following file should be created by patch, but it already exists.\nDo you want to overwrite it?\n${path}`;

  let chosen = $state<Set<string>>(untrack(() => new Set(paths)));

  function flip(path: string) {
    const next = new Set(chosen);
    if (next.has(path)) next.delete(path);
    else next.add(path);
    chosen = next;
  }

  function submit() {
    onanswer(paths.length === 1 ? [...paths] : paths.filter((path) => chosen.has(path)));
  }
</script>

<Dialog title={TITLE} width={480} onclose={() => onanswer([])} onsubmit={submit}>
  {#if paths.length === 1}
    <div class="message">
      <Icon name="info" size={24} />
      <p>{one(joinPath(repo, paths[0]))}</p>
    </div>
  {:else}
    <p>{SEVERAL}</p>
    <div class="list">
      {#each paths as path (path)}
        <label class="checkbox-label">
          <input type="checkbox" aria-label={path} checked={chosen.has(path)} onchange={() => flip(path)} />
          <span class="check" data-state={chosen.has(path) ? "on" : "off"}></span>
          {path}
        </label>
      {/each}
    </div>
  {/if}
  {#snippet footer()}
    <button type="button" class="button" onclick={() => onanswer([])}>Cancel</button>
    <button class="button default" data-autofocus>{OVERWRITE}</button>
  {/snippet}
</Dialog>

<style>
  .message {
    display: flex;
    gap: 12px;
    align-items: flex-start;
  }

  p {
    margin: 0 0 8px;
    white-space: pre-line;
    overflow-wrap: anywhere;
    user-select: text;
  }

  .list {
    display: flex;
    flex-direction: column;
    gap: 4px;
    max-height: 220px;
    padding: 6px 8px;
    overflow: auto;
    background: var(--editor);
    border: 1px solid var(--control-border);
    border-radius: 6px;
  }
</style>
