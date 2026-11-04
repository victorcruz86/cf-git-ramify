<script lang="ts">
  import { tick, untrack } from "svelte";
  import Dialog from "../lib/Dialog.svelte";
  import Icon from "../lib/Icon.svelte";
  import Menu, { type MenuItem } from "../lib/Menu.svelte";
  import ChangesPanel from "../log/ChangesPanel.svelte";
  import DetailsPanel from "../log/DetailsPanel.svelte";
  import { defaultOptions, type LogOptions } from "../log/options";
  import GitCommandsDialog from "./GitCommandsDialog.svelte";
  import {
    RebaseModel,
    shortcutAction,
    type RebaseCommit,
    type RebasePlan,
    type RebaseRow,
    type ShortcutAction,
    type TableAction,
  } from "./model";

  // `GitInteractiveRebaseDialog`: on the left the table, from the oldest to the most recent, with its
  // toolbar (`ToolbarDecorator` with Up and Down, and the actions of `ChangeEntryStateActions`); on the
  // right the changes and the details of the chosen one (`FullCommitDetailsListPanel`). The message of
  // a `reword` is edited in the row itself (`CommitMessageCellEditor`).
  interface Props {
    commits: RebaseCommit[];
    onanswer: (plan: RebasePlan | null) => void;
  }

  let { commits, onanswer }: Props = $props();

  // `rebase.interactive.dialog.*` and the names of `GitRebaseEntry.Action`.
  const TITLE = "Rebasing Commits";
  const START = "Start Rebasing";
  // `rebase.interactive.dialog.reword.hint.text` with `CommonShortcuts.CTRL_ENTER` on macOS.
  const HINT = "Press ⌘⏎ to apply changes";
  // `DIALOG_WIDTH` and `DIALOG_HEIGHT`.
  const WIDTH = 1000;
  // `PaintParameters.ROW_HEIGHT`.
  const ROW_HEIGHT = 22;
  const NODE_X = 9;
  const NODE_RADIUS = 4;
  const FIXUP_WIDTH = 20;

  const model = new RebaseModel(untrack(() => commits));
  // Goes up with each change of the model: the rows are read again.
  let version = $state(0);
  let selection = $state.raw<number[]>([]);
  let anchor: number | null = null;
  let editing = $state.raw<{ row: number } | null>(null);
  let draft = $state("");
  let resetShown = $state(false);
  let menu = $state.raw<{ x: number; y: number } | null>(null);
  let squashMenu = $state.raw<{ x: number; y: number } | null>(null);
  let commands = $state(false);
  let confirming = $state(false);
  let options = $state.raw<LogOptions>(defaultOptions());
  let table = $state<HTMLDivElement>();
  let editor = $state<HTMLTextAreaElement>();

  const rows = $derived.by((): RebaseRow[] => {
    void version;
    return model.rows();
  });
  const sorted = $derived([...selection].sort((a, b) => a - b));
  const hashes = $derived(sorted.flatMap((row) => (rows[row] ? [rows[row].commit.full] : [])));
  const isEditing = $derived(editing !== null);
  const enabled = (action: TableAction) => {
    void version;
    return model.enabled(action, sorted, isEditing);
  };

  // `fireTableRowsUpdated`: Reset appears with any change.
  function changed(before: number) {
    version += 1;
    if (model.changes !== before) resetShown = true;
  }

  function act(change: () => number[] | void) {
    const before = model.changes;
    const next = change();
    if (next) selection = next;
    changed(before);
  }

  async function startEdit(row: number) {
    selection = [row];
    draft = model.message(row);
    editing = { row };
    await tick();
    editor?.focus();
    editor?.setSelectionRange(0, 0);
  }

  // `stopCellEditing`: what was typed goes to the model, without saving state.
  function applyEdit() {
    if (!editing) return;
    model.reword(editing.row, draft);
    editing = null;
    version += 1;
    table?.focus();
  }

  function cancelEdit() {
    editing = null;
    table?.focus();
  }

  function perform(action: ShortcutAction) {
    if (action === "undo" || action === "redo") {
      // `DisabledDuringRewordAction`.
      if (isEditing) return;
      act(() => (action === "undo" ? model.undo() : model.redo()));
      selection = selection.filter((row) => row < model.size);
      return;
    }
    if (!enabled(action)) return;
    switch (action) {
      case "moveUp":
      case "moveDown":
        applyEdit();
        act(() => (action === "moveUp" ? model.moveUp(sorted) : model.moveDown(sorted)));
        return;
      case "pick":
        return act(() => model.pick(sorted));
      case "edit":
        return act(() => model.edit(sorted));
      case "drop":
        return act(() => model.drop(sorted));
      case "fixup":
        return act(() => model.fixup(sorted));
      case "reword": {
        const [row] = sorted;
        act(() => model.rewordStarted());
        void startEdit(row);
        return;
      }
      case "squash": {
        let root: number | null = null;
        act(() => {
          root = model.squash(sorted);
        });
        if (root !== null) void startEdit(root);
      }
    }
  }

  function select(row: number, event: MouseEvent) {
    if (editing && editing.row !== row) applyEdit();
    if (event.metaKey) {
      selection = selection.includes(row)
        ? selection.filter((selected) => selected !== row)
        : [...selection, row];
      anchor = row;
    } else if (event.shiftKey && anchor !== null) {
      const [from, to] = anchor < row ? [anchor, row] : [row, anchor];
      selection = Array.from({ length: to - from + 1 }, (_, offset) => from + offset);
    } else {
      selection = [row];
      anchor = row;
    }
  }

  // `isCellEditable`: with two clicks and a single chosen row.
  function doubleClicked(row: number) {
    if (selection.length > 1 || !model.enabled("reword", [row])) return;
    void startEdit(row);
  }

  function opened(row: number, event: MouseEvent) {
    event.preventDefault();
    if (!selection.includes(row)) {
      if (editing) applyEdit();
      selection = [row];
      anchor = row;
    }
    menu = { x: event.clientX, y: event.clientY };
  }

  function tableKey(event: KeyboardEvent) {
    if (event.target !== table) return;
    const action = shortcutAction(event);
    if (action) {
      event.preventDefault();
      perform(action);
      return;
    }
    if (event.key !== "ArrowUp" && event.key !== "ArrowDown") return;
    event.preventDefault();
    if (model.size === 0) return;
    const step = event.key === "ArrowUp" ? -1 : 1;
    const current = selection.at(-1) ?? (step > 0 ? -1 : model.size);
    const row = Math.min(Math.max(current + step, 0), model.size - 1);
    if (event.shiftKey && anchor !== null) {
      const [from, to] = anchor < row ? [anchor, row] : [row, anchor];
      selection = Array.from({ length: to - from + 1 }, (_, offset) => from + offset).filter(
        (selected) => selected !== row,
      );
      selection = [...selection, row];
    } else {
      selection = [row];
      anchor = row;
    }
  }

  function editorKey(event: KeyboardEvent) {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      cancelEdit();
    } else if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
      event.preventDefault();
      event.stopPropagation();
      applyEdit();
    }
  }

  function reset() {
    editing = null;
    model.reset();
    version += 1;
    resetShown = false;
  }

  function cancel() {
    if (confirming || commands) return;
    // `doCancelAction`: with changes, it asks.
    if (model.changes > 0) confirming = true;
    else onanswer(null);
  }

  function submit() {
    if (isEditing) return;
    onanswer(model.entries());
  }

  const item = (label: string, action: TableAction): MenuItem => ({
    label,
    disabled: !enabled(action),
    run: () => perform(action),
  });

  // The rows' menu: the toolbar's actions and `View Git Commands`.
  function menuItems(): MenuItem[] {
    return [
      item("Pick", "pick"),
      item("Stop to Edit", "edit"),
      item("Reword", "reword"),
      item("Squash", "squash"),
      item("Fixup", "fixup"),
      item("Drop", "drop"),
      null,
      { label: "View Git Commands", run: () => (commands = true) },
    ];
  }

  function openSquashMenu(event: MouseEvent) {
    const box = (event.currentTarget as HTMLElement).getBoundingClientRect();
    squashMenu = { x: box.left, y: box.bottom };
  }

  const graph = "#87929a";
</script>

<Dialog title={TITLE} width={WIDTH} onclose={cancel} onsubmit={submit}>
  <div class="frame">
    <section class="left">
      <div class="toolbar">
        <button
          type="button"
          class="action"
          aria-label="Up"
          title="Up (⌥↑)"
          disabled={!enabled("moveUp")}
          onclick={() => perform("moveUp")}
        >
          <Icon name="up" />
        </button>
        <button
          type="button"
          class="action"
          aria-label="Down"
          title="Down (⌥↓)"
          disabled={!enabled("moveDown")}
          onclick={() => perform("moveDown")}
        >
          <Icon name="down" />
        </button>
        <button
          type="button"
          class="action"
          aria-label="Pick"
          title="Pick (⌥P)"
          disabled={!enabled("pick")}
          onclick={() => perform("pick")}
        >
          <Icon name="revert" />
        </button>
        <button
          type="button"
          class="action"
          aria-label="Stop to Edit"
          title="Stop to Edit (⌥E)"
          disabled={!enabled("edit")}
          onclick={() => perform("edit")}
        >
          <Icon name="pause" />
        </button>
        <span class="toolbar-separator"></span>
        <button
          type="button"
          class="button tool"
          title="Reword (⌥R)"
          disabled={!enabled("reword")}
          onclick={() => perform("reword")}>Reword</button
        >
        <span class="option">
          <button
            type="button"
            class="button tool main"
            title="Squash (⌥S)"
            disabled={!enabled("squash")}
            onclick={() => perform("squash")}>Squash</button
          >
          <button
            type="button"
            class="button tool arrow"
            aria-label="More squash options"
            disabled={!enabled("squash") && !enabled("fixup")}
            onclick={openSquashMenu}
          >
            <Icon name="chevronDown" size={12} />
          </button>
        </span>
        <button
          type="button"
          class="button tool"
          title="Drop (⌥D)"
          disabled={!enabled("drop")}
          onclick={() => perform("drop")}>Drop</button
        >
        <span class="spacer"></span>
        {#if resetShown}
          <button type="button" class="link" onclick={reset}>Reset</button>
        {/if}
      </div>
      <div
        bind:this={table}
        class="table"
        role="grid"
        aria-label="Commits"
        aria-multiselectable="true"
        tabindex="0"
        data-autofocus
        onkeydown={tableKey}
      >
        {#each rows as row, index (row.commit.hash)}
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <div
            class="row"
            class:selected={selection.includes(index)}
            class:editing={editing?.row === index}
            role="row"
            aria-selected={selection.includes(index)}
            tabindex="-1"
            onclick={(event) => select(index, event)}
            ondblclick={() => doubleClicked(index)}
            oncontextmenu={(event) => opened(index, event)}
          >
            <span class="node" role="gridcell">
              <svg width="26" height={ROW_HEIGHT} aria-hidden="true">
                {#if row.node !== "edit"}
                  <line x1={NODE_X} y1="0" x2={NODE_X} y2={ROW_HEIGHT / 2} stroke={graph} stroke-width="1.5" />
                  {#if index < rows.length - 1}
                    <line
                      x1={NODE_X}
                      y1={ROW_HEIGHT / 2}
                      x2={NODE_X}
                      y2={ROW_HEIGHT}
                      stroke={graph}
                      stroke-width="1.5"
                    />
                  {/if}
                {/if}
                {#if row.node === "simple"}
                  <circle cx={NODE_X} cy={ROW_HEIGHT / 2} r={NODE_RADIUS} fill={graph} />
                {:else if row.node === "double"}
                  <circle cx={NODE_X + NODE_RADIUS} cy={ROW_HEIGHT / 2} r={NODE_RADIUS} fill={graph} />
                  <circle cx={NODE_X} cy={ROW_HEIGHT / 2} r={NODE_RADIUS + 1} class="gap" />
                  <circle cx={NODE_X} cy={ROW_HEIGHT / 2} r={NODE_RADIUS} fill={graph} />
                {/if}
              </svg>
              {#if row.node === "edit"}
                <span class="pause"><Icon name="pause" /></span>
              {/if}
            </span>
            {#if editing?.row === index}
              <span class="cell-editor" role="gridcell">
                <textarea
                  bind:this={editor}
                  class="input"
                  aria-label="Commit message"
                  autocomplete="off"
                  autocapitalize="off"
                  spellcheck="false"
                  bind:value={draft}
                  onkeydown={editorKey}
                  onclick={(event) => event.stopPropagation()}
                  ondblclick={(event) => event.stopPropagation()}
                ></textarea>
                <span class="hint">{HINT}</span>
              </span>
            {:else}
              <span
                class="subject"
                class:dropped={row.dropped}
                class:reworded={row.reworded}
                class:child={row.child !== null}
                role="gridcell"
              >
                {#if row.child}
                  <svg class="fixup" width={FIXUP_WIDTH} height={ROW_HEIGHT} aria-hidden="true">
                    <line x1="5" y1={ROW_HEIGHT / 2} x2="16" y2={ROW_HEIGHT / 2} stroke={graph} stroke-width="1.5" />
                    <line
                      x1="5"
                      y1={ROW_HEIGHT / 2}
                      x2="5"
                      y2={row.child.first ? 4 : 0}
                      stroke={graph}
                      stroke-width="1.5"
                    />
                    {#if row.child.first}
                      <polygon points="5,0 1,4 9,4" fill={graph} />
                    {/if}
                    {#if !row.child.last}
                      <line x1="5" y1={ROW_HEIGHT / 2} x2="5" y2={ROW_HEIGHT} stroke={graph} stroke-width="1.5" />
                    {/if}
                  </svg>
                {/if}
                <span class="text">{row.subject}</span>
              </span>
            {/if}
          </div>
        {/each}
      </div>
    </section>
    <section class="right">
      <div class="changes">
        <ChangesPanel
          {hashes}
          paths={[]}
          {options}
          onoptions={(next) => (options = { ...options, ...next })}
          onselect={() => {}}
          onopen={() => {}}
        />
      </div>
      <div class="details">
        <DetailsPanel
          {hashes}
          generation={0}
          showBranches={false}
          onnavigate={(hash) => {
            const row = rows.findIndex(({ commit }) => commit.full === hash);
            if (row >= 0) selection = [row];
          }}
        />
      </div>
    </section>
  </div>
  {#snippet footer()}
    <button type="button" class="button" onclick={cancel}>Cancel</button>
    <button class="button default" disabled={isEditing}>{START}</button>
  {/snippet}
</Dialog>

{#if menu}
  <Menu {...menu} label="Rebase actions" items={menuItems()} onclose={() => (menu = null)} />
{/if}
{#if squashMenu}
  <Menu
    {...squashMenu}
    label="Squash options"
    items={[item("Fixup", "fixup")]}
    onclose={() => (squashMenu = null)}
  />
{/if}
{#if commands}
  <GitCommandsDialog entries={model.entries().entries} onclose={() => (commands = false)} />
{/if}
{#if confirming}
  <!-- `rebase.interactive.dialog.discard.modifications.*`: Discard is the default button. -->
  <Dialog
    title="Cancel Rebase"
    width={380}
    onclose={() => (confirming = false)}
    onsubmit={() => onanswer(null)}
  >
    <p class="question">Discard changes and cancel rebase?</p>
    {#snippet footer()}
      <button type="button" class="button" onclick={() => (confirming = false)}>Continue Rebasing</button>
      <button class="button default" data-autofocus>Discard</button>
    {/snippet}
  </Dialog>
{/if}

<style>
  .frame {
    display: flex;
    height: 450px;
    max-height: calc(100vh - 200px);
    border: 1px solid var(--divider);
  }

  .left,
  .right {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }

  .left {
    border-right: 1px solid var(--divider);
  }

  .toolbar {
    display: flex;
    flex: none;
    gap: 2px;
    align-items: center;
    height: 34px;
    padding: 0 6px;
    border-bottom: 1px solid var(--divider);
  }

  .tool {
    height: 26px;
    margin-left: 6px;
    padding: 0 10px;
  }

  .option {
    display: inline-flex;
  }

  .option .main {
    border-top-right-radius: 0;
    border-bottom-right-radius: 0;
  }

  .option .arrow {
    display: inline-flex;
    align-items: center;
    margin-left: 0;
    padding: 0 4px;
    border-left: 0;
    border-top-left-radius: 0;
    border-bottom-left-radius: 0;
  }

  .spacer {
    flex: 1;
  }

  .link {
    margin: 0 10px 0 5px;
    padding: 0;
    font: inherit;
    color: var(--link);
    background: none;
    border: 0;
  }

  .table {
    flex: 1;
    min-height: 0;
    overflow: auto;
    outline: none;
  }

  .row {
    display: flex;
    align-items: flex-start;
    min-height: 22px;
    cursor: default;
  }

  .row.selected {
    background: var(--selection-inactive);
  }

  .table:focus-within .row.selected {
    background: var(--selection);
  }

  .node {
    position: relative;
    flex: none;
    width: 26px;
    height: 22px;
  }

  .node svg {
    display: block;
  }

  .gap {
    fill: var(--panel);
  }

  .row.selected .gap {
    fill: var(--selection-inactive);
  }

  .table:focus-within .row.selected .gap {
    fill: var(--selection);
  }

  .pause {
    position: absolute;
    top: 3px;
    left: 1px;
  }

  .subject {
    display: flex;
    flex: 1;
    align-items: center;
    min-width: 0;
    height: 22px;
    white-space: nowrap;
  }

  .subject .text {
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .fixup {
    flex: none;
  }

  .dropped .text {
    text-decoration: line-through;
  }

  /* `JBColor.BLUE` in the dark theme (`DarculaColors.BLUE`). */
  .reworded .text {
    color: #589df6;
  }

  .cell-editor {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
    padding: 6px 6px 0 0;
  }

  .cell-editor textarea {
    height: 110px;
    padding: 4px 6px;
    resize: vertical;
  }

  .hint {
    height: 17px;
    padding: 0 6px;
    font-size: 11px;
    line-height: 17px;
    color: var(--muted);
  }

  .changes,
  .details {
    flex: 1;
    min-height: 0;
    overflow: auto;
  }

  .details {
    border-top: 1px solid var(--divider);
  }

  .question {
    margin: 0;
  }
</style>
