<script lang="ts">
  import { onMount, tick } from "svelte";
  import { gpgConfig, gpgConfigSet, gpgKeys, type GpgKey, type SigningConfig } from "../lib/api";
  import Dialog from "../lib/Dialog.svelte";
  import { errorText } from "../lib/errors";
  import { taskResult } from "../lib/tasks";

  interface Props {
    // `changed` is false when OK had nothing to write.
    onclose: (changed: boolean) => void;
  }

  let { onclose }: Props = $props();

  let loading = $state(true);
  let initial = $state.raw<SigningConfig | null>(null);
  let keys = $state.raw<GpgKey[]>([]);
  let sign = $state(false);
  let selected = $state<string | null>(null);
  let listOpen = $state(false);
  let problem = $state<string | null>(null);
  // Shown instead of `Cannot find suitable private key`: an empty list says nothing then.
  let keysProblem = $state<string | null>(null);
  let writing = $state(false);
  let checkbox: HTMLInputElement | undefined = $state();

  // `GitGpgConfigDialog.initComboBox`: a configured key that is not in the list is still shown.
  const items = $derived.by(() => {
    const configured = initial?.key;
    return configured && !keys.some((key) => key.id === configured)
      ? [{ id: configured, userId: null }, ...keys]
      : keys;
  });
  const current = $derived(items.find((item) => item.id === selected) ?? null);
  const noKeys = $derived(!loading && keysProblem === null && keys.length === 0);
  const canWrite = $derived(!loading && !writing && (items.length > 0 || !sign));

  function check(on: boolean) {
    sign = on;
    if (on && selected === null) selected = items[0]?.id ?? null;
  }

  // A config that cannot be read shows its error, and the dialog starts unchecked: OK then writes
  // what the user chose.
  async function load() {
    try {
      const config = await gpgConfig();
      initial = config;
      sign = config.sign && config.key !== null;
      selected = config.key;
    } catch (failure) {
      problem = errorText(failure);
    }
    try {
      keys = await gpgKeys();
    } catch (failure) {
      keysProblem = errorText(failure);
    }
    if (sign && selected === null) selected = items[0]?.id ?? null;
    loading = false;
    // It was disabled while loading, so it could not take the initial focus.
    await tick();
    checkbox?.focus();
  }

  function pick(id: string) {
    selected = id;
    listOpen = false;
  }

  // OK writes at once, like 241's dialog; it is not part of the settings `Save`.
  async function submit() {
    if (!canWrite) return;
    const wasSigning = initial !== null && initial.sign && initial.key !== null;
    if (initial !== null && sign === wasSigning && (!sign || selected === initial.key)) {
      onclose(false);
      return;
    }
    writing = true;
    problem = null;
    try {
      await taskResult<null>(await gpgConfigSet(sign ? selected : null));
      onclose(true);
    } catch (failure) {
      problem = errorText(failure);
    } finally {
      writing = false;
    }
  }

  onMount(() => {
    void load();
  });
</script>

<Dialog title="Configure GPG Key" width={460} onclose={() => onclose(false)} onsubmit={submit}>
  <label class="checkbox-label">
    <input
      type="checkbox"
      bind:this={checkbox}
      data-autofocus
      disabled={loading}
      checked={sign}
      onchange={(event) => check(event.currentTarget.checked)}
    />
    <span class="check" data-state={sign ? "on" : "off"}></span>
    Sign commits with GPG key:
  </label>
  <div class="combo-row">
    <div class="combo">
      <button
        type="button"
        class="input combo-button"
        role="combobox"
        aria-label="GPG key"
        aria-expanded={listOpen}
        aria-controls="gpg-key-list"
        disabled={loading || !sign}
        onclick={() => (listOpen = !listOpen)}
      >
        {#if current}
          <span>{current.id}</span>
          {#if current.userId}<span class="uid">{current.userId}</span>{/if}
        {/if}
      </button>
      {#if listOpen}
        <ul id="gpg-key-list" class="list" role="listbox">
          {#each items as item (item.id)}
            <li role="presentation">
              <button
                type="button"
                role="option"
                aria-selected={item.id === selected}
                onclick={() => pick(item.id)}
              >
                <span>{item.id}</span>
                {#if item.userId}<span class="uid">{item.userId}</span>{/if}
              </button>
            </li>
          {/each}
        </ul>
      {/if}
    </div>
    {#if loading}<span class="spinner" role="status" aria-label="Loading"></span>{/if}
  </div>
  {#if noKeys}
    <p class="field-error" role="alert">Cannot find suitable private key</p>
  {/if}
  {#if keysProblem}
    <p class="field-error" role="alert">{keysProblem}</p>
  {/if}
  {#if problem}
    <p class="field-error" role="alert">{problem}</p>
  {/if}

  {#snippet footer()}
    <span class="synced">Synced with gitconfig</span>
    <button type="button" class="button" onclick={() => onclose(false)}>Cancel</button>
    <button class="button default" disabled={!canWrite}>OK</button>
  {/snippet}
</Dialog>

<style>
  .combo-row {
    display: flex;
    gap: 8px;
    align-items: center;
    margin: 8px 0 0 20px;
  }

  .combo {
    position: relative;
    flex: 1;
    min-width: 0;
  }

  .combo-button {
    display: flex;
    gap: 8px;
    align-items: center;
    width: 100%;
    text-align: left;
  }

  .uid {
    overflow: hidden;
    color: var(--text-secondary);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .list {
    position: absolute;
    top: 100%;
    right: 0;
    left: 0;
    z-index: 1;
    max-height: 160px;
    padding: 4px 0;
    margin: 2px 0 0;
    overflow: auto;
    list-style: none;
    background: var(--panel);
    border: 1px solid var(--popup-border);
    border-radius: 6px;
  }

  .list button {
    display: flex;
    gap: 8px;
    width: 100%;
    padding: 4px 8px;
    font: inherit;
    color: var(--text);
    text-align: left;
    background: none;
    border: 0;
  }

  .spinner {
    width: 14px;
    height: 14px;
    border: 2px solid var(--text-secondary);
    border-top-color: transparent;
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .synced {
    margin-right: auto;
    align-self: center;
    color: var(--text-secondary);
  }
</style>
