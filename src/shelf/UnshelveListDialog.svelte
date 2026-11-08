<script lang="ts">
  import { untrack } from "svelte";
  import type { Changelists } from "../lib/api";
  import Dialog from "../lib/Dialog.svelte";
  import Icon from "../lib/Icon.svelte";
  import Menu from "../lib/Menu.svelte";
  import type { MenuItem } from "../lib/menu";
  import { EMPTY_NAME, renamedList, unshelveList, type ListDraft } from "../patch/applyModel";
  import type { ListChoice } from "../patch/flow";
  import { UNSHELVE_TO_CHANGELIST } from "./flow";
  import { saveShelfSetting } from "./settings";

  // `ChangeListChooser` titled `Unshelve Changes to Changelist` (V:536), with the Remove checkbox in
  // the "do not ask" place (`UnshelveWithDialogAction.unshelveMultipleShelveChangeLists`). The panel is
  // `ChangeListChooserPanel`: `Name:` with the lists and a `New` tag, `Comment:`, `Set active`.
  interface Props {
    lists: Changelists;
    names: string[];
    removeApplied: boolean;
    onanswer: (answer: { list: ListChoice; removeApplied: boolean } | null) => void;
  }

  let { lists, names, removeApplied, onanswer }: Props = $props();

  let draft = $state.raw<ListDraft>(untrack(() => unshelveList(lists, names)));
  let setActive = $state(false);
  let remove = $state(untrack(() => removeApplied));
  let menu = $state.raw<{ x: number; y: number; items: MenuItem[] } | null>(null);

  const error = $derived(draft.name.trim() ? null : EMPTY_NAME);
  const isNew = $derived(draft.name.trim() !== "" && !lists.lists.some((list) => list.name === draft.name));

  function openLists(event: MouseEvent) {
    const bounds = (event.currentTarget as HTMLElement).getBoundingClientRect();
    menu = {
      x: bounds.left,
      y: bounds.bottom + 2,
      items: lists.lists.map((list) => ({
        label: list.name,
        checked: list.name === draft.name,
        run: () => (draft = renamedList(draft, lists, list.name)),
      })),
    };
  }

  function submit() {
    if (error) return;
    onanswer({ list: { name: draft.name.trim(), comment: draft.comment, setActive }, removeApplied: remove });
  }
</script>

<Dialog title={UNSHELVE_TO_CHANGELIST} width={480} onclose={() => onanswer(null)} onsubmit={submit}>
  <div class="chooser">
    <label for="unshelve-list-name">Name:</label>
    <div class="field">
      <input
        id="unshelve-list-name"
        class="input"
        role="combobox"
        aria-expanded={menu !== null}
        aria-controls="unshelve-list-options"
        data-autofocus
        value={draft.name}
        oninput={(event) => (draft = renamedList(draft, lists, event.currentTarget.value))}
      />
      {#if isNew}
        <span class="tag">New</span>
      {/if}
      <button type="button" class="button browse" aria-label="Changelists" onclick={openLists}>
        <Icon name="chevronDown" />
      </button>
    </div>
    <label for="unshelve-list-comment">Comment:</label>
    <textarea
      id="unshelve-list-comment"
      class="input"
      rows="3"
      value={draft.comment}
      oninput={(event) => (draft = { ...draft, comment: event.currentTarget.value, typed: true })}
    ></textarea>
    <span></span>
    <label class="checkbox-label">
      <input type="checkbox" bind:checked={setActive} />
      <span class="check" data-state={setActive ? "on" : "off"}></span>
      Set active
    </label>
    {#if error}
      <span></span>
      <p class="field-error" role="status">{error}</p>
    {/if}
  </div>
  {#snippet footer()}
    <label class="checkbox-label left">
      <!-- `createRemoveFilesStrategyCheckbox`: saved as it changes, even if the dialog is cancelled. -->
      <input
        type="checkbox"
        checked={remove}
        onchange={(event) => {
          remove = event.currentTarget.checked;
          void saveShelfSetting({ removeStrategy: remove }).catch(() => {});
        }}
      />
      <span class="check" data-state={remove ? "on" : "off"}></span>
      Remove successfully applied files from the shelf
    </label>
    <button type="button" class="button" onclick={() => onanswer(null)}>Cancel</button>
    <button class="button default" disabled={error !== null}>OK</button>
  {/snippet}
</Dialog>

{#if menu}
  <Menu x={menu.x} y={menu.y} label="Changelists" items={menu.items} onclose={() => (menu = null)} />
{/if}

<style>
  .chooser {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 6px 8px;
    align-items: center;
  }

  .field {
    display: flex;
    gap: 6px;
    align-items: center;
    min-width: 0;
  }

  .field .input {
    flex: 1;
    min-width: 0;
  }

  .tag {
    color: var(--text-secondary);
  }

  .field-error {
    margin: 0;
    color: var(--error);
  }

  .left {
    margin-right: auto;
  }
</style>
