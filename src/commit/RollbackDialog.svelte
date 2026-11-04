<script lang="ts">
  import { untrack } from "svelte";
  import type { FileChange } from "../lib/api";
  import Dialog from "../lib/Dialog.svelte";
  import Icon from "../lib/Icon.svelte";
  import { legend } from "./request";
  import { statusColor } from "./tree";

  const LEGEND_COLORS = {
    added: "var(--status-added)",
    modified: "var(--status-modified)",
    deleted: "var(--status-deleted)",
  };

  interface Props {
    files: FileChange[];
    checked: string[];
    onconfirm: (paths: string[], deleteAdded: boolean) => void;
    onclose: () => void;
  }

  let { files, checked, onconfirm, onclose }: Props = $props();

  let chosen = $state.raw(new Set(untrack(() => checked)));
  let deleteAdded = $state(false);

  const picked = $derived(files.filter((file) => chosen.has(file.path)));
  const hasNewFiles = $derived(picked.some((file) => file.state === "added"));
  const items = $derived(legend(files, chosen));

  function toggle(path: string) {
    const next = new Set(chosen);
    if (!next.delete(path)) next.add(path);
    chosen = next;
  }

  function submit() {
    if (picked.length > 0) {
      onconfirm(
        picked.map((file) => file.path),
        hasNewFiles && deleteAdded,
      );
    }
  }
</script>

<Dialog title="Rollback Changes" {onclose} onsubmit={submit} width={560}>
  <div class="files">
    {#each files as file (file.path)}
      <label class="file">
        <input
          type="checkbox"
          aria-label={file.path}
          checked={chosen.has(file.path)}
          onchange={() => toggle(file.path)}
        />
        <span class="check" data-state={chosen.has(file.path) ? "on" : "off"}></span>
        <Icon name="text" />
        <span class="name" style:color={statusColor(file.state)}>{file.path}</span>
      </label>
    {/each}
  </div>
  <div class="legend">
    {#each items as item (item.key)}
      <span style:color={LEGEND_COLORS[item.key]}>{item.text}</span>
    {/each}
  </div>
  <label class="checkbox-label" class:disabled={!hasNewFiles}>
    <input type="checkbox" bind:checked={deleteAdded} disabled={!hasNewFiles} />
    <span class="check" data-state={deleteAdded ? "on" : "off"}></span>
    Delete local copies of added files
  </label>
  {#snippet footer()}
    <button type="button" class="button" onclick={onclose}>Close</button>
    <button class="button default" disabled={picked.length === 0} data-autofocus>Rollback</button>
  {/snippet}
</Dialog>

<style>
  .files {
    max-height: 280px;
    min-height: 96px;
    padding: 4px;
    overflow: auto;
    background: var(--editor);
    border: 1px solid var(--control-border);
    border-radius: 6px;
  }

  .file {
    position: relative;
    display: flex;
    gap: 6px;
    align-items: center;
    height: 24px;
    padding: 0 6px;
    white-space: nowrap;
  }

  .file input {
    position: absolute;
    opacity: 0;
  }

  .file input:focus-visible + .check {
    outline: 2px solid var(--focus);
    outline-offset: 1px;
  }

  .name {
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .legend {
    display: flex;
    gap: 12px;
    min-height: 18px;
    margin: 8px 0;
  }
</style>
