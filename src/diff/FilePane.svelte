<script lang="ts">
  import { onDestroy, untrack } from "svelte";
  import {
    changelistsCreate,
    changelistsMoveHunks,
    changelistsSetActive,
    fileWrite,
    linesRollback,
    notifyRepoChanged,
    snapshotsRestore,
    stagingPreview,
    stagingSides,
    stagingStageHunks,
    stagingUnstageHunks,
    worktreePreview,
    worktreeSides,
    type ApiError,
    type Changelists,
    type Comparison,
    type Hunk,
    type LineSelection,
    type SelectionChange,
    type WorktreeSides,
  } from "../lib/api";
  import Dialog from "../lib/Dialog.svelte";
  import { errorText, lostRollbackText, lostStageText } from "../lib/errors";
  import Menu, { type MenuItem } from "../lib/Menu.svelte";
  import { findList } from "../commit/changelists";
  import ListChooserDialog from "../commit/ListChooserDialog.svelte";
  import { visibleHunks, wholeFileHunk } from "./blocks";
  import { DiffEditors, type DiffData, type LineRange } from "./editor";
  import {
    effectiveSelection,
    fileCheck,
    hunkPlace,
    hunksAt,
    linesSelection,
    normalizeSelection,
    toggleHunk,
    toggleLine,
  } from "./gutter";
  import checkOff from "./icons/gutterCheckBox.svg?raw";
  import checkPartial from "./icons/gutterCheckBoxIndeterminate.svg?raw";
  import checkOn from "./icons/gutterCheckBoxSelected.svg?raw";
  import type { DiffOptions } from "./options";
  import { sameSides, sideMessage, sideText, unchangedText } from "./sides";

  const PREVIEW_DELAY_MS = 150;

  export type NoticeKind = "info" | "warning" | "error";

  interface Props {
    path: string;
    // Goes up when something may have changed on disk.
    version: number;
    options: DiffOptions;
    included: boolean;
    selection: LineSelection[];
    lists: Changelists | null;
    // With the staging area: what is compared, and the margin moves to the stage or takes out of it.
    comparison?: Comparison | null;
    embedded?: boolean;
    // The `Preview Diff` of Local Changes: the text cannot be edited and there are no line checkboxes.
    readonly?: boolean;
    // Nobody keeps the line checkboxes (the diff of Local Changes in modal mode): the text is still editable.
    checkboxes?: boolean;
    onselection: (change: SelectionChange) => void;
    onnotice: (kind: NoticeKind, text: string, action?: { label: string; run: () => void }) => void;
    onactive?: () => void;
    // Only in measurement mode: timings of the first time the editors are created.
    ondrawn?: (timing: { sidesMs: number; buildMs: number; builtAtEpochMs: number }) => void;
  }

  let {
    path,
    version,
    options,
    included,
    selection,
    lists,
    comparison = null,
    embedded = false,
    readonly = false,
    checkboxes = true,
    onselection,
    onnotice,
    onactive,
    ondrawn,
  }: Props = $props();

  let sides = $state.raw<WorktreeSides | null>(null);
  let failure = $state<string | null>(null);
  let dirty = $state(false);
  // The file changed on disk with unsaved changes here: saving would fail.
  let conflict = $state(false);
  let host = $state<HTMLDivElement>();
  let menu = $state.raw<{ x: number; y: number; items: MenuItem[] } | null>(null);
  let moving = $state.raw<Hunk[] | null>(null);
  let showing = $state.raw<Hunk[] | null>(null);
  let editors: DiffEditors | null = null;
  let editorsUnified = false;
  let shown = $state.raw<DiffData | null>(null);
  let request = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  // Saves go one at a time: each one needs the hash left by the previous one.
  let saving: Promise<boolean> = Promise.resolve(true);
  let writing = false;
  let sidesMs: number | null = null;

  const message = $derived(sides && (sideMessage(sides.old) ?? sideMessage(sides.new)));
  // With a conflict there is no stage to compare or to change.
  const shownComparison = $derived(sides?.conflicted ? null : comparison);
  const unchanged = $derived(
    sides && !message
      ? unchangedText(sides, shownComparison === "local" ? "the stage" : "HEAD")
      : null,
  );
  const oldTitle = $derived.by(() => {
    const absent = sides?.old.kind === "absent";
    if (shownComparison === "local") return absent ? "No version in the stage" : "Staged";
    return absent ? "No previous version" : "HEAD";
  });
  const newTitle = $derived.by(() => {
    if (sides?.new.kind === "absent") return "Deleted";
    if (shownComparison === "staged") return "Staged";
    return shownComparison === "local" ? "Local" : "Your version";
  });
  const checkLabel = $derived.by(() => {
    if (shownComparison === "staged") return "Unstage";
    return shownComparison === "local" ? "Stage" : "Include into commit";
  });
  const fileList = $derived(lists ? (lists.assignments[path] ?? lists.active) : null);
  const ownHunks = $derived(
    sides?.conflicted || readonly || !checkboxes ? [] : (sides?.hunks ?? []).filter(isOwn),
  );
  // What was chosen from hunks that no longer exist is handled by the Commit view, which reports it.
  const ownIds = $derived(new Set(ownHunks.map((hunk) => hunk.id)));
  const chosen = $derived(selection.filter((entry) => ownIds.has(entry.hunk)));
  const effective = $derived(effectiveSelection(ownHunks, chosen, included));
  const allCheck = $derived(fileCheck(ownHunks, effective));
  const hiddenCount = $derived(
    (sides?.hunks.length ?? 0) - (shown?.hunks.filter((hunk) => hunk.id !== "").length ?? 0),
  );

  function isOwn(hunk: Hunk): boolean {
    const assigned = lists?.hunkAssignments[path]?.[hunk.id];
    return !assigned || assigned === fileList;
  }

  function dataFor(current: WorktreeSides): DiffData {
    const oldText = sideText(current.old);
    const newText = sideText(current.new);
    const hunks =
      options.ignore === "none"
        ? current.hunks
        : visibleHunks(current.hunks, oldText.split("\n"), newText.split("\n"), options.ignore);
    const whole = current.hunks.length === 0 ? wholeFileHunk(oldText, newText) : null;
    return {
      oldText,
      newText,
      hunks: whole ? [whole] : hunks,
      selectable: current.conflicted || readonly || !checkboxes ? [] : hunks.filter(isOwn),
      selection: effective,
    };
  }

  function sameHunks(left: Hunk[], right: Hunk[]): boolean {
    return (
      left.length === right.length && left.every((hunk, index) => hunk.id === right[index].id)
    );
  }

  function destroyEditors() {
    editors?.destroy();
    editors = null;
    shown = null;
  }

  // Follows what has to be shown without rebuilding the editors: a change of selection or options
  // must not lose the cursor or the scroll position.
  function draw(current: WorktreeSides | null) {
    if (!current || !host || sideMessage(current.old) || sideMessage(current.new)) {
      destroyEditors();
      return;
    }
    const data = dataFor(current);
    const editable = current.editable && !readonly;
    const fits = editors?.fits(data, editable) ?? false;
    // `load` does not change `sides` with unsaved text: what was typed always fits in its editors.
    if (editors?.dirty && !fits) return;
    if (!editors || !fits || editorsUnified !== options.unified) {
      const draft = editors?.dirty ? editors.content() : null;
      const buildStart = sidesMs === null ? 0 : performance.now();
      editors?.destroy();
      editorsUnified = options.unified;
      editors = new DiffEditors(host, {
        path,
        data,
        draft,
        editable,
        unified: options.unified,
        embedded,
        options,
        handlers: { ontoggle: toggle, onedit: edited, onfocus: () => onactive?.() },
      });
      if (!embedded) {
        editors.showFirstChange();
        editors.focus();
      }
      if (draft !== null) edited();
      if (sidesMs !== null) {
        const buildMs = performance.now() - buildStart;
        ondrawn?.({ sidesMs, buildMs, builtAtEpochMs: Date.now() });
        sidesMs = null;
      }
    } else {
      editors.setOptions(options);
      const same =
        shown !== null &&
        shown.oldText === data.oldText &&
        shown.newText === data.newText &&
        sameHunks(shown.hunks, data.hunks) &&
        sameHunks(shown.selectable, data.selectable);
      if (same) editors.setSelection(data.selection);
      else if (editors.dirty) return;
      else editors.setData(data);
    }
    shown = data;
  }

  async function load() {
    const token = ++request;
    const measured = ondrawn !== undefined && sides === null;
    const start = measured ? performance.now() : 0;
    try {
      const fresh = await (comparison ? stagingSides(path, comparison) : worktreeSides(path));
      if (token !== request || writing) return;
      if (measured) sidesMs = performance.now() - start;
      failure = null;
      if (editors?.dirty) {
        conflict = fresh.hash !== sides?.hash;
        return;
      }
      conflict = false;
      if (!sides || !sameSides(sides, fresh)) sides = fresh;
    } catch (error) {
      if (token === request) failure = errorText(error);
    }
  }

  function edited() {
    dirty = editors?.dirty ?? false;
    clearTimeout(timer);
    timer = setTimeout(preview, PREVIEW_DELAY_MS);
  }

  async function preview() {
    if (!editors) return;
    if (!editors.dirty) {
      // It was undone back to what is saved. If what is saved is new, it will arrive with its hunks.
      if (shown && editors.content() === shown.newText) editors.setData(shown);
      return;
    }
    const content = editors.content();
    try {
      const hunks = await (comparison
        ? stagingPreview(path, content)
        : worktreePreview(path, content));
      if (editors?.dirty && editors.content() === content) editors.setDraftHunks(hunks);
    } catch (error) {
      onnotice("error", errorText(error));
    }
  }

  export function isDirty(): boolean {
    return dirty;
  }

  export function save(): Promise<boolean> {
    saving = saving.then(writeDraft);
    return saving;
  }

  // What is marked as saved is what was sent: what was typed in the meantime is still unsaved, which
  // is why the disk is not read again then. False if it could not be saved.
  async function writeDraft(): Promise<boolean> {
    const target = editors;
    if (!target?.dirty) return true;
    if (!sides?.hash) {
      onnotice("error", `"${path}" cannot be saved from here`);
      return false;
    }
    const sent = target.snapshot();
    writing = true;
    try {
      const hash = await fileWrite(path, target.textOf(sent), sides.hash);
      target.markSaved(sent);
      sides = { ...sides, hash };
      conflict = false;
      // What was read before writing is no longer valid.
      request++;
      writing = false;
      dirty = editors?.dirty ?? false;
      if (dirty) edited();
      else await load();
      return true;
    } catch (error) {
      writing = false;
      onnotice("error", errorText(error));
      // A file changed from outside is reported in the notice with the option to reload.
      await load();
      return false;
    } finally {
      writing = false;
    }
  }

  // One pass: what was typed during this write is picked up by the next one of `saveAll`.
  export async function flush(): Promise<boolean> {
    if (editors?.dirty && !(await save())) return false;
    await saving;
    return true;
  }

  function discard() {
    dirty = false;
    conflict = false;
    destroyEditors();
    sides = null;
    load();
  }

  export function navigate(direction: 1 | -1): boolean {
    return editors?.navigate(direction) ?? false;
  }

  function places(chosen: LineSelection[]): Record<string, string> {
    const known = sides?.hunks ?? [];
    return Object.fromEntries(
      chosen.flatMap((entry) => {
        const hunk = known.find((candidate) => candidate.id === entry.hunk);
        return hunk ? [[hunk.id, hunkPlace(hunk)]] : [];
      }),
    );
  }

  function publish(next: LineSelection[]) {
    const normal = normalizeSelection(ownHunks, next);
    onselection({ path, ...normal, places: places(next) });
  }

  function toggle(hunk: Hunk, line: number | null) {
    if (comparison) {
      changeStage([{ hunk: hunk.id, lines: line === null ? null : [line] }]);
      return;
    }
    publish(line === null ? toggleHunk(effective, hunk) : toggleLine(effective, hunk, line));
  }

  function toggleAll() {
    if (comparison) {
      changeStage(effectiveSelection(ownHunks, [], true));
      return;
    }
    publish(allCheck === "on" ? [] : effectiveSelection(ownHunks, [], true));
  }

  // What is "local" goes to the stage and what is "staged" comes out of it. The hunks are those of
  // what is shown.
  async function changeStage(chosen: LineSelection[]) {
    if (dirty || chosen.length === 0) return;
    try {
      const copy = await (comparison === "staged"
        ? stagingUnstageHunks(path, chosen)
        : stagingStageHunks(path, chosen));
      if (copy) {
        onnotice("info", "A snapshot was saved of what existed only in the stage.", {
          label: "Restore",
          run: () => restore(copy),
        });
      }
      await notifyRepoChanged();
      await load();
    } catch (error) {
      const lost = (error as ApiError).details;
      if (lost) {
        onnotice("warning", lostStageText(lost));
        await load();
      } else {
        onnotice("error", errorText(error));
      }
    }
  }

  function linesOf(range: LineRange): Hunk[] {
    return hunksAt(sides?.hunks ?? [], range.side, range.from, range.to);
  }

  async function rollback(range: LineRange) {
    const chosen = linesSelection(sides?.hunks ?? [], range.side, range.from, range.to, range.exact);
    if (chosen.length === 0) return;
    try {
      const snapshot = await linesRollback(path, chosen);
      const touched = new Set(chosen.map((entry) => entry.hunk));
      if (selection.some((entry) => touched.has(entry.hunk))) {
        publish(effective.filter((entry) => !touched.has(entry.hunk)));
      }
      onnotice("info", "Rollback of lines. A snapshot was saved.", {
        label: "Restore",
        run: () => restore(snapshot),
      });
      await load();
    } catch (error) {
      const lost = (error as ApiError).details;
      if (lost) {
        onnotice("warning", lostRollbackText(lost));
        await load();
      } else {
        onnotice("error", errorText(error));
      }
    }
  }

  async function restore(id: string) {
    try {
      const previous = await snapshotsRestore(id);
      onnotice("info", "Snapshot restored.", {
        label: "Undo",
        run: () => restore(previous),
      });
      await load();
    } catch (error) {
      onnotice("error", errorText(error));
    }
  }

  async function move(hunks: Hunk[], name: string, makeActive: boolean) {
    moving = null;
    if (!lists) return;
    try {
      const target = findList(lists, name) ?? findList(await changelistsCreate(name), name);
      if (!target) return;
      const ids = new Set(hunks.map((hunk) => hunk.id));
      await changelistsMoveHunks(path, [...ids], target.id);
      unchoose(ids);
      if (makeActive) await changelistsSetActive(target.id);
      await notifyRepoChanged();
    } catch (error) {
      onnotice("error", errorText(error));
    }
  }

  // What moves to another list stops being chosen in this one.
  function unchoose(ids: Set<string>) {
    if (!chosen.some((entry) => ids.has(entry.hunk))) return;
    const left = effective.filter((entry) => !ids.has(entry.hunk));
    const rest = ownHunks.filter((hunk) => !ids.has(hunk.id));
    onselection({ path, ...normalizeSelection(rest, left), places: places(left) });
  }

  function range(event?: MouseEvent): LineRange | null {
    if (!editors) return null;
    return event ? editors.linesAt(event) : editors.selectedLines();
  }

  // For ⌥⌘Z and ⌘⇧M: the lines of the cursor or of the selection.
  export function rollbackLines() {
    const lines = range();
    if (lines && !dirty && !comparison) rollback(lines);
  }

  export function moveLines() {
    if (comparison) return;
    const lines = range();
    const hunks = lines ? linesOf(lines) : [];
    if (hunks.length > 0) moving = hunks;
  }

  function contextmenu(event: MouseEvent) {
    const lines = range(event);
    if (!lines) return;
    event.preventDefault();
    const hunks = linesOf(lines);
    const none = hunks.length === 0;
    const changeItems: MenuItem[] = comparison
      ? [
          {
            label: comparison === "staged" ? "Unstage" : "Stage",
            disabled: none || dirty || sides?.conflicted,
            run: () => stageLines(lines),
          },
        ]
      : [
          {
            label: "Rollback Lines",
            shortcut: "⌥⌘Z",
            disabled: none || dirty,
            run: () => rollback(lines),
          },
          {
            label: "Move Lines to Another Changelist...",
            shortcut: "⇧⌘M",
            disabled: none || !lists,
            run: () => (moving = hunks),
          },
        ];
    menu = {
      x: event.clientX,
      y: event.clientY,
      items: [
        ...changeItems,
        { label: "Show Diff for Lines", disabled: none, run: () => (showing = hunks) },
      ],
    };
  }

  function stageLines(lines: LineRange) {
    const known = sides?.hunks ?? [];
    changeStage(linesSelection(known, lines.side, lines.from, lines.to, lines.exact));
  }

  function linesText(text: string, start: number, count: number): string[] {
    return text.split("\n").slice(start, start + count);
  }

  $effect(() => {
    void [version, comparison];
    untrack(() => load());
  });

  $effect(() => {
    void [sides, options, selection, included, lists, host, checkboxes];
    const current = sides;
    untrack(() => draw(current));
  });

  onDestroy(() => {
    clearTimeout(timer);
    editors?.destroy();
  });
</script>

<div class="pane" class:embedded>
  <div class="titles">
    {#if !options.unified}
      <div class="title">{oldTitle}</div>
    {/if}
    <div class="title">
      {#if ownHunks.length > 0 && !dirty}
        <button
          type="button"
          class="all"
          role="checkbox"
          aria-checked={allCheck === "partial" ? "mixed" : allCheck === "on"}
          aria-label={checkLabel}
          title={checkLabel}
          onclick={toggleAll}
        >
          {@html (allCheck === "on" ? checkOn : allCheck === "off" ? checkOff : checkPartial).replace(
            /<!--.*?-->\s*/s,
            "",
          )}
        </button>
      {/if}
      <span>{newTitle}{dirty ? " *" : ""}</span>
      {#if sides?.note}
        <span class="note">{sides.note}</span>
      {/if}
      {#if hiddenCount > 0}
        <span class="note">
          {hiddenCount === 1 ? "1 change hidden" : `${hiddenCount} changes hidden`} by the whitespace option
        </span>
      {/if}
    </div>
  </div>

  {#if conflict}
    <div class="banner" role="alert">
      <span>The file has changed on disk and there are unsaved changes here.</span>
      <button type="button" class="link" onclick={discard}>Reload and Discard Edits</button>
    </div>
  {/if}

  {#if failure}
    <p class="message" role="alert">{failure}</p>
  {:else if message}
    <p class="message">{message}</p>
  {:else if unchanged}
    <p class="message">{unchanged}</p>
  {/if}

  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="host" bind:this={host} oncontextmenu={contextmenu}></div>
</div>

{#if menu}
  <Menu
    x={menu.x}
    y={menu.y}
    label="Diff"
    items={menu.items}
    onclose={() => (menu = null)}
  />
{/if}

{#if moving && lists}
  {@const hunks = moving}
  <ListChooserDialog
    title="Move Lines to Another Changelist"
    lists={lists.lists}
    initial={lists.lists.find((list) => list.id !== fileList)?.name ?? ""}
    allowNew
    onconfirm={(name, makeActive) => move(hunks, name, makeActive)}
    onclose={() => (moving = null)}
  />
{/if}

{#if showing && sides}
  {@const oldText = sideText(sides.old)}
  {@const newText = sideText(sides.new)}
  <Dialog title="Show Diff for Lines" width={640} onclose={() => (showing = null)}>
    {#each showing as hunk (hunk.id)}
      <pre class="lines">{#each linesText(oldText, hunk.oldStart, hunk.oldLines) as line}<span class="old">{line}
</span>{/each}{#each linesText(newText, hunk.newStart, hunk.newLines) as line}<span class="new">{line}
</span>{/each}</pre>
    {/each}
    {#snippet footer()}
      <button type="button" class="button" onclick={() => (showing = null)}>Close</button>
    {/snippet}
  </Dialog>
{/if}

<style>
  .pane {
    position: relative;
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    background: var(--editor);
  }

  .titles {
    display: flex;
    flex: none;
    height: 28px;
    color: var(--text-secondary);
    background: var(--panel);
    border-bottom: 1px solid var(--divider);
  }

  .title {
    display: flex;
    flex: 1;
    gap: 8px;
    align-items: center;
    min-width: 0;
    padding: 0 8px;
    overflow: hidden;
    white-space: nowrap;
  }

  .title + .title {
    border-left: 1px solid var(--divider);
  }

  .note {
    overflow: hidden;
    color: var(--muted);
    text-overflow: ellipsis;
  }

  .all {
    display: inline-flex;
    flex: none;
    padding: 0;
    color: var(--icon);
    background: none;
    border: 0;
  }

  .banner {
    display: flex;
    flex: none;
    gap: 12px;
    align-items: center;
    padding: 6px 12px;
    background: var(--notice-warning);
    border-bottom: 1px solid var(--notice-warning-border);
  }

  .link {
    padding: 0;
    font: inherit;
    color: var(--link);
    background: none;
    border: 0;
  }

  .message {
    padding: 16px;
    margin: 0;
    color: var(--text-secondary);
  }

  .host {
    display: flex;
    flex: 1;
    min-height: 0;
  }

  .embedded .host {
    flex: none;
  }

  .host :global(.rm-editors) {
    display: flex;
    flex: 1;
    min-width: 0;
    min-height: 0;
  }

  .host :global(.rm-editor) {
    flex: 1;
    min-width: 0;
    min-height: 0;
  }

  .host :global(.rm-editor + .rm-editor) {
    border-left: 1px solid var(--divider);
  }

  .host :global(.rm-editor .cm-editor) {
    height: 100%;
  }

  .host :global(.rm-embedded .rm-editor .cm-editor) {
    height: auto;
  }

  .host :global(.rm-divider) {
    flex: none;
    width: 30px;
    background: var(--panel);
  }

  .host :global(.rm-inserted-band) {
    fill: var(--diff-inserted);
  }

  .host :global(.rm-deleted-band) {
    fill: var(--diff-deleted);
  }

  .host :global(.rm-modified-band) {
    fill: var(--diff-modified);
  }

  .lines {
    max-height: 320px;
    padding: 4px 0;
    margin: 0 0 8px;
    overflow: auto;
    font: 12px/18px ui-monospace, Menlo, monospace;
    color: var(--editor-text);
    background: var(--editor);
  }

  .lines span {
    display: block;
    padding: 0 8px;
  }

  .lines .old {
    background: var(--diff-deleted);
  }

  .lines .new {
    background: var(--diff-inserted);
  }
</style>
