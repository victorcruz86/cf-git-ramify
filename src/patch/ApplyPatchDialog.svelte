<script lang="ts">
  import { onDestroy, untrack } from "svelte";
  import type { ApplyPlan, PatchInput, PathMapping, PlannedFile } from "../lib/api";
  import Dialog from "../lib/Dialog.svelte";
  import { errorText } from "../lib/errors";
  import Icon from "../lib/Icon.svelte";
  import { matchShortcut } from "../lib/keymap";
  import Menu, { type MenuItem } from "../lib/Menu.svelte";
  import {
    EMPTY_NAME,
    NEW_BASE_DETECTED,
    SELECT_MISSING_BASE,
    applyRows,
    baseChanged,
    baseTooltip,
    canStrip,
    initialIncluded,
    initialList,
    isValid,
    keptIncluded,
    legendText,
    mapBase,
    missingBase,
    patchFileName,
    preselectedIncluded,
    relativeTo,
    renamedList,
    rowText,
    statusText,
    strip,
    strippedText,
    unshelveList,
    withOverwrites,
    type ApplyRow,
    type ListDraft,
    type StripAction,
  } from "./applyModel";
  import ApplyPatchDiff from "./ApplyPatchDiff.svelte";
  import { OVERWRITE, SELECT_PATCH_TITLE, type ApplyAnswer, type ApplyDialogRequest } from "./flow";

  // `ApplyPatchDifferentiatedDialog` in its APPLY, APPLY_PATCH_IN_MEMORY and UNSHELVE modes
  // (`MyUnshelveDialog`). The patch text and its paths are untrusted: shown as text only.
  interface Props extends ApplyDialogRequest {
    onanswer: (answer: ApplyAnswer | null) => void;
  }

  let {
    mode,
    repo,
    source: given,
    plan: first,
    lists,
    onTheFly,
    services,
    removeApplied = false,
    preselected = null,
    listNames = [],
    onanswer,
  }: Props = $props();

  // `patch.apply.cannot.read.patch` (V:392) and rama's own refusal of a folder outside the repository.
  const cannotRead = (name: string, reason: string) => `Cannot read patch ${name}: ${reason}`;
  const OUTSIDE = "The folder must be inside the repository";
  // `MergingUpdateQueue("ApplyPatchDifferentiatedDialog", 500, …)`.
  const RELOAD_DELAY_MS = 500;
  const STRIP_ACTIONS: { action: StripAction; label: string }[] = [
    { action: "up", label: "Remove Leading Directory" },
    { action: "all", label: "Remove All Leading Directories" },
    { action: "down", label: "Restore Leading Directory" },
    { action: "reset", label: "Restore All Leading Directories" },
  ];

  let source = $state.raw<PatchInput>(untrack(() => given));
  let fileField = $state(untrack(() => (given.kind === "file" ? given.path : "")));
  let plan = $state.raw<ApplyPlan>(untrack(() => first));
  let mapping = $state.raw<PathMapping>({ files: [] });
  let included = $state.raw<Set<number>>(untrack(() => preselectedIncluded(first, preselected)));
  // The binary files chosen with `Overwrite`, by path: a new plan keeps the choice.
  let overwriting = $state.raw<Set<string>>(new Set());
  let selected = $state.raw<Set<string>>(new Set());
  let lead = $state<string | null>(null);
  let loading = $state(false);
  let failure = $state<string | null>(null);
  let menu = $state.raw<{ x: number; y: number; items: MenuItem[] } | null>(null);
  let diffStart = $state<number | null>(null);
  let detect = $state(untrack(() => onTheFly));
  let remove = $state(untrack(() => removeApplied));
  let draft = $state.raw<ListDraft | null>(
    untrack(() =>
      !lists ? null : mode === "unshelve" ? unshelveList(lists, listNames) : initialList(first, lists, given),
    ),
  );
  let draftTouched = false;
  let setActive = $state(false);
  let timer: ReturnType<typeof setTimeout> | undefined;
  // A typed `Patch file name:` waits for its reload: until then OK would apply the old patch.
  let pending = $state(false);
  let request = 0;
  let destroyed = false;
  let treeElement = $state<HTMLDivElement>();

  // `unshelve.changes.dialog.title` (V:639) and `unshelve.changes.action` (V:481) for the OK button.
  const title = $derived(mode === "unshelve" ? "Unshelve Changes" : "Apply Patch");
  const okText = $derived(mode === "unshelve" ? "Unshelve Changes" : "OK");
  const rows = $derived(applyRows(plan.files));
  const legend = $derived(legendText(plan.files, included));
  const readError = $derived(plan.readError ? cannotRead(patchFileName(source), plan.readError) : null);
  const chosen = $derived(selectedFiles(selected));
  // `MyShowDiff.update`: with files and some whose base exists or is added.
  const canDiff = $derived(plan.files.length > 0 && plan.files.some(isValid));
  const listError = $derived(draft && !draft.name.trim() ? EMPTY_NAME : null);
  const isNew = $derived(
    draft !== null && lists !== null && draft.name.trim() !== "" && !lists.lists.some((list) => list.name === draft?.name),
  );
  const canApply = $derived(!loading && !pending && included.size > 0 && listError === null);

  onDestroy(() => {
    destroyed = true;
    clearTimeout(timer);
  });

  function selectedFiles(ids: Set<string>): PlannedFile[] {
    const indexes = new Set(rows.filter((row) => ids.has(row.id)).flatMap((row) => row.indexes));
    return plan.files.filter((file) => indexes.has(file.index));
  }

  // `reloadPatchFromFile` (a new file: everything valid included again) and `updateTree(false)`
  // (a new mapping: what was included stays).
  async function reload(fresh: boolean, after?: (loaded: ApplyPlan) => void) {
    const token = ++request;
    loading = true;
    failure = null;
    try {
      const loaded = await services.load(source, mapping);
      if (destroyed || token !== request) return;
      plan = withOverwrites(loaded, overwriting);
      included = fresh ? initialIncluded(loaded) : keptIncluded(loaded, included);
      if (fresh && lists && !draftTouched && mode !== "unshelve") draft = initialList(loaded, lists, source);
      after?.(loaded);
    } catch (error) {
      if (!destroyed && token === request) failure = errorText(error);
    } finally {
      if (!destroyed && token === request) loading = false;
    }
  }

  function fileTyped(value: string) {
    fileField = value;
    clearTimeout(timer);
    pending = true;
    timer = setTimeout(() => {
      pending = false;
      source = { kind: "file", path: fileField.trim() };
      mapping = { files: [] };
      void reload(true);
    }, RELOAD_DELAY_MS);
  }

  async function browse() {
    const path = await services.chooseFile({ title: SELECT_PATCH_TITLE, defaultPath: fileField || repo });
    if (!path) return;
    clearTimeout(timer);
    pending = false;
    fileField = path;
    source = { kind: "file", path };
    mapping = { files: [] };
    await reload(true);
  }

  // The file as typed now, even before its reload.
  function refresh() {
    clearTimeout(timer);
    if (pending) {
      source = { kind: "file", path: fileField.trim() };
      mapping = { files: [] };
    }
    pending = false;
    void reload(true);
  }

  function toggle(row: ApplyRow) {
    const valid = row.indexes.filter((index) => plan.files.some((file) => file.index === index && isValid(file)));
    if (valid.length === 0) return;
    const next = new Set(included);
    const on = valid.every((index) => next.has(index));
    for (const index of valid) {
      if (on) next.delete(index);
      else next.add(index);
    }
    included = next;
  }

  function checkState(row: ApplyRow): "true" | "false" | "mixed" {
    const valid = row.indexes.filter((index) => plan.files.some((file) => file.index === index && isValid(file)));
    const on = valid.filter((index) => included.has(index)).length;
    if (on === 0) return "false";
    return on === valid.length ? "true" : "mixed";
  }

  const checkable = (row: ApplyRow) =>
    row.indexes.some((index) => plan.files.some((file) => file.index === index && isValid(file)));

  function select(row: ApplyRow, event?: MouseEvent) {
    if (event && (event.metaKey || event.ctrlKey)) {
      const next = new Set(selected);
      if (next.has(row.id)) next.delete(row.id);
      else next.add(row.id);
      selected = next;
    } else {
      selected = new Set([row.id]);
    }
    lead = row.id;
  }

  // rama's own: the shelved version is written over what HEAD brought, after the copy.
  function overwriteRow(file: PlannedFile) {
    overwriting = new Set([...overwriting, file.path]);
    plan = withOverwrites(plan, overwriting);
    included = new Set([...included, file.index]);
  }

  // `NewBaseSelector(false)`: a file for a target that is not there; its folder becomes the base.
  async function selectMissingBase(file: PlannedFile) {
    const chosenPath = await services.chooseFile({ title: "Select File Base", defaultPath: repo });
    if (!chosenPath) return;
    const parent = chosenPath.slice(0, Math.max(0, chosenPath.lastIndexOf("/")));
    const base = relativeTo(repo, parent);
    if (base === null) {
      failure = OUTSIDE;
      return;
    }
    mapping = missingBase(mapping, file, base);
    await reload(false, (loaded) => {
      const now = loaded.files.find((item) => item.index === file.index);
      if (now && isValid(now)) included = new Set([...included, now.index]);
    });
  }

  // `MapDirectory` with `NewBaseSelector(true)`.
  async function mapDirectory(files: PlannedFile[]) {
    const folder = await services.chooseFolder({ title: "Select Directory Base", defaultPath: repo });
    if (!folder) return;
    const base = relativeTo(repo, folder);
    if (base === null) {
      failure = OUTSIDE;
      return;
    }
    mapping = mapBase(mapping, files, base);
    await reload(false);
  }

  function openPaths(event: MouseEvent) {
    const bounds = (event.currentTarget as HTMLElement).getBoundingClientRect();
    const files = chosen;
    const run = (action: StripAction) => () => {
      mapping = strip(mapping, files, action);
      void reload(false);
    };
    menu = {
      x: bounds.left,
      y: bounds.bottom + 2,
      items: [
        { label: "Map Base Directory…", disabled: files.length === 0, run: () => void mapDirectory(files) },
        ...STRIP_ACTIONS.map(({ action, label }) => ({
          label,
          disabled: !canStrip(action, files),
          run: run(action),
        })),
      ],
    };
  }

  // `MyShowDiff.showDiff`: the chain of every file, from the selected one.
  function showDiff() {
    if (!canDiff) return;
    const ordered = diffFiles();
    const start = chosen[0] ? ordered.findIndex((file) => file.index === chosen[0].index) : 0;
    diffStart = Math.max(0, start);
  }

  const diffFiles = () => [...plan.files].sort((left, right) => (left.path < right.path ? -1 : left.path > right.path ? 1 : 0));

  // The double-click and Enter handler: an invalid file asks for its base, any other shows the diff.
  function open(row: ApplyRow) {
    const file = row.file;
    if (file && file.status === "missing") void selectMissingBase(file);
    else if (file) showDiff();
  }

  // `MyShowDiff.registerCustomShortcutSet(…, getRootPane())`: ⌘D anywhere in the dialog.
  function dialogKeydown(event: KeyboardEvent) {
    if (matchShortcut(event) !== "Diff.ShowDiff") return;
    event.preventDefault();
    event.stopPropagation();
    showDiff();
  }

  // The tree's own keys; in the fields they stay the fields'.
  function treeKeydown(event: KeyboardEvent) {
    if (event.metaKey || event.ctrlKey || event.altKey) return;
    const index = rows.findIndex((row) => row.id === lead);
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      const next = rows[Math.min(Math.max(index + (event.key === "ArrowDown" ? 1 : -1), 0), rows.length - 1)];
      if (next) select(next);
    } else if (event.key === " ") {
      for (const row of rows.filter((item) => selected.has(item.id))) toggle(row);
    } else if (event.key === "Enter" && index >= 0) {
      open(rows[index]);
    } else {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
  }

  function nameTyped(value: string) {
    if (!draft || !lists) return;
    draftTouched = true;
    draft = renamedList(draft, lists, value);
  }

  function commentTyped(value: string) {
    if (!draft) return;
    draftTouched = true;
    draft = { ...draft, comment: value, typed: true };
  }

  function openLists(event: MouseEvent) {
    if (!lists || !draft) return;
    const bounds = (event.currentTarget as HTMLElement).getBoundingClientRect();
    const current = lists;
    menu = {
      x: bounds.left,
      y: bounds.bottom + 2,
      items: current.lists.map((list) => ({
        label: list.name,
        checked: list.name === draft?.name,
        run: () => nameTyped(list.name),
      })),
    };
  }

  function detectChanged(on: boolean) {
    detect = on;
    void services.setOnTheFly(on).catch(() => {});
  }

  function removeChanged(on: boolean) {
    remove = on;
    void services.setRemoveApplied(on).catch(() => {});
  }

  // OK, or `Import to Shelf` (`runExecutor`) with the same included files.
  function submit(importToShelf = false) {
    if (!canApply) return;
    const order = plan.files.map((file) => file.index).filter((index) => included.has(index));
    const list = draft ? { name: draft.name.trim(), comment: draft.comment, setActive } : null;
    const extra = mode === "unshelve" ? { removeApplied: remove } : importToShelf ? { importToShelf } : {};
    const overwrite = plan.files
      .filter((file) => file.overwritable && overwriting.has(file.path) && included.has(file.index))
      .map((file) => file.path);
    onanswer({ source, mapping, plan, selected: order, list, ...extra, ...(overwrite.length > 0 ? { overwrite } : {}) });
  }
</script>

<Dialog {title} width={720} onclose={() => onanswer(null)} onsubmit={() => submit()}>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="body" onkeydown={dialogKeydown}>
    {#if mode === "apply"}
      <label for="patch-file-name">Patch file name:</label>
      <div class="field">
        <input
          id="patch-file-name"
          class="input"
          value={fileField}
          oninput={(event) => fileTyped(event.currentTarget.value)}
        />
        <button type="button" class="button browse" aria-label="Select Patch File" title={SELECT_PATCH_TITLE} onclick={browse}
          >…</button
        >
      </div>
    {/if}
    <div class="toolbar">
      <button
        type="button"
        class="action"
        aria-label="Show Difference"
        title="Show Difference (⌘D)"
        disabled={!canDiff}
        onclick={showDiff}
      >
        <Icon name="diff" />
      </button>
      <button
        type="button"
        class="action"
        aria-label="Change Directory Paths"
        title="Change Directory Paths"
        aria-haspopup="menu"
        onclick={openPaths}
      >
        <Icon name="folder" />
      </button>
      {#if mode === "apply"}
        <button type="button" class="action" aria-label="Refresh" title="Refresh" onclick={refresh}>
          <Icon name="refresh" />
        </button>
      {/if}
    </div>
    <div class="tree-box">
      {#if readError || failure}
        <p class="error" role="alert">{failure ?? readError}</p>
      {/if}
      <div
        bind:this={treeElement}
        class="tree"
        role="tree"
        aria-label="Changes"
        aria-multiselectable="true"
        aria-busy={loading}
        tabindex="0"
        onkeydown={treeKeydown}
      >
        {#if !loading && plan.files.length === 0}
          <p class="empty">No changes detected</p>
        {/if}
        {#each rows as row (row.id)}
          {@const file = row.file}
          <!-- The keys belong to the tree, which holds the selection. -->
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <div
            class="row"
            class:selected={selected.has(row.id)}
            role="treeitem"
            tabindex="-1"
            aria-selected={selected.has(row.id)}
            aria-label={file ? rowText(file) : row.name}
            title={file && isValid(file) && baseChanged(file)
              ? baseTooltip(file, repo, mapping.files[file.index]?.base ?? "")
              : undefined}
            style:padding-left="{8 + row.depth * 20}px"
            onclick={(event) => {
              select(row, event);
              treeElement?.focus();
            }}
            ondblclick={() => open(row)}
          >
            {#if checkable(row)}
              <!-- svelte-ignore a11y_interactive_supports_focus -->
              <span
                class="check"
                role="checkbox"
                aria-checked={checkState(row)}
                aria-label={row.name}
                data-state={checkState(row) === "true" ? "on" : checkState(row) === "mixed" ? "mixed" : "off"}
                onclick={(event) => {
                  event.stopPropagation();
                  toggle(row);
                }}
              ></span>
            {:else}
              <span class="no-check"></span>
            {/if}
            <Icon name={row.kind === "folder" ? "folder" : "text"} />
            <span class="name">{row.name}</span>
            {#if file}
              {#if file.strip > 0}
                <span class="stripped">{strippedText(file.strip)}</span>
              {/if}
              <span class="status">{statusText(file)}</span>
              {#if file.status === "missing"}
                <button
                  type="button"
                  class="link"
                  onclick={(event) => {
                    event.stopPropagation();
                    void selectMissingBase(file);
                  }}>{SELECT_MISSING_BASE}</button
                >
              {:else if file.status === "invalid"}
                <span class="invalid">{file.error}</span>
                {#if file.overwritable}
                  <button
                    type="button"
                    class="link"
                    onclick={(event) => {
                      event.stopPropagation();
                      overwriteRow(file);
                    }}>{OVERWRITE}</button
                  >
                {/if}
              {:else if baseChanged(file)}
                <span class="detected">{NEW_BASE_DETECTED}</span>
              {/if}
            {/if}
          </div>
        {/each}
      </div>
    </div>
    <p class="legend">{legend}</p>
    {#if draft && lists}
      <div class="chooser">
        <label for="apply-list-name">Name:</label>
        <div class="field">
          <input
            id="apply-list-name"
            class="input"
            role="combobox"
            aria-expanded={menu !== null}
            aria-controls="apply-list-options"
            value={draft.name}
            oninput={(event) => nameTyped(event.currentTarget.value)}
          />
          {#if isNew}
            <span class="tag">New</span>
          {/if}
          <button type="button" class="button browse" aria-label="Changelists" onclick={openLists}>
            <Icon name="chevronDown" />
          </button>
        </div>
        <label for="apply-list-comment">Comment:</label>
        <textarea
          id="apply-list-comment"
          class="input comment"
          rows="3"
          value={draft.comment}
          oninput={(event) => commentTyped(event.currentTarget.value)}
        ></textarea>
        <span></span>
        <label class="checkbox-label">
          <input type="checkbox" bind:checked={setActive} />
          <span class="check" data-state={setActive ? "on" : "off"}></span>
          Set active
        </label>
        {#if listError}
          <span></span>
          <p class="field-error" role="status">{listError}</p>
        {/if}
      </div>
    {/if}
  </div>
  {#snippet footer()}
    {#if mode === "clipboard"}
      <label class="checkbox-label left">
        <input type="checkbox" checked={detect} onchange={(event) => detectChanged(event.currentTarget.checked)} />
        <span class="check" data-state={detect ? "on" : "off"}></span>
        Analyze and apply patch from clipboard on the fly
      </label>
    {/if}
    {#if mode === "unshelve"}
      <label class="checkbox-label left">
        <input type="checkbox" checked={remove} onchange={(event) => removeChanged(event.currentTarget.checked)} />
        <span class="check" data-state={remove ? "on" : "off"}></span>
        Remove successfully applied files from the shelf
      </label>
    {/if}
    {#if mode === "apply"}
      <button type="button" class="button" disabled={!canApply} onclick={() => submit(true)}>Import to Shelf</button>
    {/if}
    <button type="button" class="button" onclick={() => onanswer(null)}>Cancel</button>
    <button class="button default" disabled={!canApply}>{okText}</button>
  {/snippet}
</Dialog>

{#if menu}
  <Menu x={menu.x} y={menu.y} label="Change Directory Paths" items={menu.items} onclose={() => (menu = null)} />
{/if}

{#if diffStart !== null}
  <ApplyPatchDiff
    files={diffFiles()}
    start={diffStart}
    {repo}
    load={(file) => services.preview(source, mapping, file.index)}
    onclose={() => (diffStart = null)}
  />
{/if}

<style>
  .body {
    display: flex;
    flex-direction: column;
    gap: 6px;
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

  .toolbar {
    display: flex;
    gap: 2px;
    align-items: center;
  }

  .tree-box {
    display: flex;
    flex-direction: column;
    height: 260px;
    background: var(--editor);
    border: 1px solid var(--control-border);
    border-radius: 6px;
  }

  .error {
    margin: 0;
    padding: 6px 8px;
    color: var(--error);
    overflow-wrap: anywhere;
  }

  .tree {
    flex: 1;
    min-height: 0;
    padding: 4px 0;
    overflow: auto;
    outline: none;
  }

  .empty {
    margin: 0;
    padding: 24px 0;
    color: var(--text-secondary);
    text-align: center;
  }

  .row {
    display: flex;
    gap: 6px;
    align-items: center;
    height: 24px;
    padding-right: 8px;
    white-space: nowrap;
    cursor: default;
  }

  .row.selected {
    background: var(--selection);
  }

  .no-check {
    width: 14px;
  }

  .stripped {
    margin-left: -6px;
    font-style: italic;
    color: var(--text-secondary);
  }

  .status {
    margin-left: 12px;
    color: var(--text-secondary);
  }

  .detected {
    margin-left: 4px;
    font-style: italic;
    color: var(--text-secondary);
  }

  .invalid {
    margin-left: 4px;
    color: var(--error);
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .link {
    margin-left: 4px;
    padding: 0;
    font: inherit;
    color: var(--link);
    background: none;
    border: 0;
    cursor: pointer;
  }

  .legend {
    min-height: 16px;
    margin: 0;
    white-space: pre;
  }

  .chooser {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 8px 10px;
    align-items: center;
    margin-top: 6px;
  }

  .comment {
    resize: vertical;
  }

  .tag {
    padding: 1px 6px;
    color: var(--status-added);
    background: var(--hover);
    border-radius: 4px;
  }

  .field-error {
    margin: 0;
    color: var(--error);
  }

  .left {
    margin-right: auto;
  }
</style>
