<script lang="ts">
  import { onMount } from "svelte";
  import type { Remote } from "../lib/api";
  import Dialog from "../lib/Dialog.svelte";
  import { errorText } from "../lib/errors";
  import { notify } from "../lib/notifications";
  import ConfirmDialog from "./ConfirmDialog.svelte";
  import DefineRemoteDialog from "./DefineRemoteDialog.svelte";
  import {
    addRemote,
    editRemote,
    proposedName,
    removeMessage,
    removeRemote,
    REMOVE_TITLE,
    remotesApi,
    type RemotesApi,
  } from "./model";

  interface Props {
    api?: RemotesApi;
    onclose: () => void;
  }

  let { api = remotesApi, onclose }: Props = $props();

  let remotes = $state.raw<Remote[]>([]);
  let selected = $state<string | null>(null);
  // `remote` is the one being edited; without it, one is added.
  let defining = $state.raw<{ remote: Remote | null } | null>(null);
  let removing = $state.raw<Remote | null>(null);
  const current = $derived(remotes.find((remote) => remote.name === selected) ?? null);

  async function load() {
    try {
      remotes = await api.list();
    } catch (failure) {
      notify({ kind: "error", title: "Cannot load remotes", body: errorText(failure) });
    }
  }

  async function defined(answer: { name: string; url: string } | null) {
    const target = defining?.remote ?? null;
    defining = null;
    if (!answer) return;
    const done = target
      ? await editRemote(api, target, answer.name, answer.url)
      : await addRemote(api, answer.name, answer.url);
    await load();
    if (done) selected = answer.name;
  }

  async function confirmed(yes: boolean) {
    const target = removing;
    removing = null;
    if (!yes || !target) return;
    await removeRemote(api, target);
    await load();
    selected = null;
  }

  // Move with the arrows, as in IntelliJ's table.
  function keydown(event: KeyboardEvent, name: string, index: number) {
    const step = event.key === "ArrowDown" ? 1 : event.key === "ArrowUp" ? -1 : 0;
    if (step !== 0) {
      const next = remotes[index + step];
      if (next) selected = next.name;
    } else if (event.key === "Enter" && current) {
      defining = { remote: current };
    } else if (event.key === " ") {
      selected = name;
    } else return;
    event.preventDefault();
    event.stopPropagation();
  }

  onMount(load);
</script>

<Dialog title="Git Remotes" {onclose} onsubmit={onclose} width={620}>
  <div class="layout">
    <table>
      <thead>
        <tr><th>Name</th><th>URL</th></tr>
      </thead>
      <tbody>
        {#each remotes as remote, index (remote.name)}
          <tr
            tabindex="0"
            aria-selected={selected === remote.name}
            onclick={() => (selected = remote.name)}
            ondblclick={() => (defining = { remote })}
            onkeydown={(event) => keydown(event, remote.name, index)}
          >
            <td class="name">{remote.name}</td>
            <td>{remote.url}</td>
          </tr>
        {/each}
      </tbody>
    </table>
    <div class="buttons">
      <button type="button" class="button" onclick={() => (defining = { remote: null })}>Add</button>
      <button type="button" class="button" disabled={!current} onclick={() => (removing = current)}>
        Remove
      </button>
      <button
        type="button"
        class="button"
        disabled={!current}
        onclick={() => (defining = { remote: current })}>Edit</button>
    </div>
  </div>
  {#snippet footer()}
    <button class="button default">OK</button>
  {/snippet}
</Dialog>

{#if defining}
  <DefineRemoteDialog
    initialName={defining.remote?.name ?? proposedName(remotes)}
    initialUrl={defining.remote?.url ?? ""}
    taken={remotes.map((remote) => remote.name)}
    test={(url, name) => api.testUrl(url, name)}
    onanswer={defined}
  />
{/if}
{#if removing}
  <ConfirmDialog title={REMOVE_TITLE} message={removeMessage(removing)} onanswer={confirmed} />
{/if}

<style>
  .layout {
    display: flex;
    gap: 12px;
    align-items: flex-start;
  }

  table {
    flex: 1;
    min-width: 0;
    border-collapse: collapse;
    border: 1px solid var(--border);
  }

  th {
    padding: 4px 8px;
    font-weight: 400;
    color: var(--text-secondary);
    text-align: left;
    border-bottom: 1px solid var(--border);
  }

  td {
    padding: 4px 8px;
    user-select: text;
    word-break: break-all;
  }

  .name {
    font-weight: 600;
    white-space: nowrap;
  }

  tbody tr {
    outline: none;
  }

  tbody tr:hover {
    background: var(--hover);
  }

  tbody tr[aria-selected="true"] {
    background: var(--selection, var(--stripe-selected));
  }

  .buttons {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
</style>
