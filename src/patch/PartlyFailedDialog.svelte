<script lang="ts">
  import Dialog from "../lib/Dialog.svelte";
  import Icon from "../lib/Icon.svelte";

  // `UndoApplyPatchDialog`: the files that failed, and whether to roll back what was applied.
  interface Props {
    failed: string[];
    // Some file of the patch is binary.
    binaries: boolean;
    onanswer: (rollback: boolean) => void;
  }

  let { failed, binaries, onanswer }: Props = $props();

  // V:374-378.
  const TITLE = "Patch Applying Partly Failed";
  const prompt = (count: number) =>
    `Failed to apply ${count === 0 ? "patch." : count === 1 ? "patch to file:" : "patch to files:"}`;
  const BOTTOM = "Other changes from the patch have been applied successfully, would you like to rollback them?";
  const BINARIES = "Rollback will not affect binaries";
</script>

<Dialog title={TITLE} width={520} onclose={() => onanswer(false)} onsubmit={() => onanswer(true)}>
  <p>{prompt(failed.length)}</p>
  {#if failed.length > 0}
    <ul class="files">
      {#each failed as path (path)}
        <li>{path}</li>
      {/each}
    </ul>
  {/if}
  <p>{BOTTOM}</p>
  {#if binaries}
    <div class="warning">
      <Icon name="warning" />
      <p>{BINARIES}</p>
    </div>
  {/if}
  {#snippet footer()}
    <button type="button" class="button" onclick={() => onanswer(false)}>Cancel</button>
    <button class="button default" data-autofocus>Rollback</button>
  {/snippet}
</Dialog>

<style>
  p {
    margin: 0 0 8px;
    overflow-wrap: anywhere;
  }

  .files {
    max-height: 200px;
    margin: 0 0 8px;
    padding: 6px 8px;
    overflow: auto;
    list-style: none;
    background: var(--editor);
    border: 1px solid var(--control-border);
    border-radius: 6px;
    user-select: text;
  }

  .warning {
    display: flex;
    gap: 6px;
    align-items: center;
  }

  .warning p {
    margin: 0;
  }
</style>
