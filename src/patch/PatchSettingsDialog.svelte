<script lang="ts">
  import { onDestroy, untrack } from "svelte";
  import type { PatchPaths } from "../lib/api";
  import { chooseFolder, chooseSaveFile } from "../lib/choosers";
  import Dialog from "../lib/Dialog.svelte";
  import {
    SAVE_TO_TITLE,
    SETTINGS_TITLE,
    settingsProblem,
    settingsWarning,
    type PatchProblem,
    type PatchSettingsAnswer,
    type PatchSettingsRequest,
  } from "./flow";

  // `CreatePatchConfigurationPanel` in `SessionDialog`: OK stays disabled while a field is wrong
  // (`canExecute`), and the red label warns about an existing file. `Encoding:` is UTF-8 only
  // (decision 6). The Help button is not copied (decision 10).
  interface Props extends PatchSettingsRequest {
    onanswer: (answer: PatchSettingsAnswer | null) => void;
  }

  let {
    file: initialFile,
    toClipboard: initialClipboard,
    base: initialBase,
    commonParent,
    check,
    onanswer,
  }: Props = $props();

  let file = $state(untrack(() => initialFile));
  let toClipboard = $state(untrack(() => initialClipboard));
  let base = $state(untrack(() => initialBase));
  let reverse = $state(false);
  let checked = $state.raw<PatchPaths | null>(null);
  let failure = $state<string | null>(null);
  let request = 0;
  let destroyed = false;

  const problem = $derived.by<PatchProblem | null>(() => {
    if (failure) return { field: "file", text: failure };
    return checked ? settingsProblem(checked, commonParent, toClipboard) : null;
  });
  const warning = $derived(checked ? settingsWarning(checked) : null);

  onDestroy(() => (destroyed = true));

  // The latest one wins: what was typed meanwhile is checked again.
  async function validate(): Promise<PatchPaths | null> {
    const token = ++request;
    try {
      const paths = await check(file, base);
      if (destroyed || token !== request) return null;
      failure = null;
      checked = paths;
      return paths;
    } catch (error) {
      if (destroyed || token !== request) return null;
      failure = String((error as { message?: string })?.message ?? error);
      return null;
    }
  }

  $effect(() => {
    void [file, base];
    void validate();
  });

  async function submit() {
    const paths = await validate();
    if (!paths || settingsProblem(paths, commonParent, toClipboard)) return;
    onanswer({ file: paths.file, toClipboard, base: paths.base, reverse });
  }

  // `selectFileName`: the save panel in the folder of the current file, with its name.
  async function browseFile() {
    const chosen = await chooseSaveFile({ title: SAVE_TO_TITLE, defaultPath: file });
    if (chosen && !destroyed) file = chosen;
  }

  async function browseBase() {
    const chosen = await chooseFolder({ defaultPath: base });
    if (chosen && !destroyed) base = chosen;
  }
</script>

<Dialog title={SETTINGS_TITLE} width={600} onclose={() => onanswer(null)} onsubmit={submit}>
  <div class="rows">
    <label class="choice">
      <input
        type="radio"
        name="patch-target"
        checked={!toClipboard}
        onchange={() => (toClipboard = false)}
      />
      To file:
    </label>
    <div class="field">
      <input
        class="input"
        aria-label="To file:"
        aria-invalid={problem?.field === "file"}
        disabled={toClipboard}
        data-autofocus
        bind:value={file}
      />
      <button
        type="button"
        class="button browse"
        aria-label={SAVE_TO_TITLE}
        disabled={toClipboard}
        onclick={browseFile}
      >
        …
      </button>
    </div>
    {#if problem?.field === "file"}
      <span></span>
      <p class="field-error" role="alert">{problem.text}</p>
    {/if}
    <label class="choice span">
      <input
        type="radio"
        name="patch-target"
        checked={toClipboard}
        onchange={() => (toClipboard = true)}
      />
      To clipboard
    </label>
    <label for="patch-base">Base path:</label>
    <div class="field">
      <input
        id="patch-base"
        class="input"
        aria-invalid={problem?.field === "base"}
        bind:value={base}
      />
      <button
        type="button"
        class="button browse"
        aria-label="Choose the base path"
        onclick={browseBase}
      >
        …
      </button>
    </div>
    {#if problem?.field === "base"}
      <span></span>
      <p class="field-error" role="alert">{problem.text}</p>
    {/if}
    <label class="checkbox-label span">
      <input type="checkbox" bind:checked={reverse} />
      <span class="check" data-state={reverse ? "on" : "off"}></span>
      Reverse patch
    </label>
    <label for="patch-encoding">Encoding:</label>
    <select id="patch-encoding" class="input encoding" disabled>
      <option>UTF-8</option>
    </select>
    {#if warning}
      <p class="warning span" role="status">{warning}</p>
    {/if}
  </div>
  {#snippet footer()}
    <button type="button" class="button" onclick={() => onanswer(null)}>Cancel</button>
    <button class="button default" disabled={problem !== null}>OK</button>
  {/snippet}
</Dialog>

<style>
  .rows {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 8px 10px;
    align-items: center;
  }

  .span {
    grid-column: 1 / -1;
  }

  .choice {
    display: flex;
    gap: 6px;
    align-items: center;
    white-space: nowrap;
  }

  .field {
    display: flex;
    gap: 6px;
    min-width: 0;
  }

  .field .input {
    flex: 1;
    min-width: 0;
  }

  .field-error {
    margin: 0;
  }

  .encoding {
    justify-self: start;
  }

  .warning {
    margin: 0;
    color: var(--error);
    overflow-wrap: anywhere;
  }
</style>
