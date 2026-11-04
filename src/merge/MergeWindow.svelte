<script lang="ts">
  import { onMount, tick } from "svelte";
  import {
    conflictResolve,
    conflictSides,
    conflictsAccept,
    mergeBlocks,
    mergeWindowDone,
    mergeWindowPath,
    mergeWindowReady,
    mergeLeaveReceived,
    mergeLeft,
    mergeWorkingFile,
    onMergeClose,
    onMergeLeave,
    onRepoChanged,
    onSettingsChanged,
    settingsGet,
    settingsSave,
    showWindow,
    type ConflictSide,
    type ConflictSides,
    type ConflictVersion,
    type MergeResult,
    type Settings,
    type Whitespace,
  } from "../lib/api";
  import Dialog from "../lib/Dialog.svelte";
  import { errorText, isOutdated } from "../lib/errors";
  import Icon from "../lib/Icon.svelte";
  import Menu, { type MenuItem } from "../lib/Menu.svelte";
  import { taskResult, trackTasks } from "../lib/tasks";
  import { MergeEditors } from "./editors";
  import { normalized, statusText, unprocessedMessage, withSeparator } from "./model";

  // Ported from MergeRequestProcessor, MergeThreesideViewer.getResolveAction, MergeUtil and
  // BinaryMergeTool: the window around the three-pane editor.

  interface Question {
    title: string;
    message: string;
    yes: string;
    no: string;
    answer: (yes: boolean) => void;
  }

  const CONTINUE = "Continue Merge";
  const WHITESPACE: { value: Whitespace; label: string }[] = [
    { value: "default", label: "Do not ignore" },
    { value: "trim", label: "Trim whitespaces" },
    { value: "ignore", label: "Ignore whitespaces" },
  ];

  let path: string | null = null;
  let sides = $state.raw<ConflictSides | null>(null);
  let texts: { left: string; base: string; right: string } | null = null;
  let separator = "\n";
  let settings: Settings | null = null;
  let editors: MergeEditors | null = null;
  let host = $state<HTMLElement>();
  // Changes with every change of the editor: what is derived from it is read again.
  let version = $state(0);
  let loaded = $state(false);
  let failure = $state<string | null>(null);
  let error = $state<string | null>(null);
  let outdated = $state(false);
  let processed = $state(false);
  let busy = $state(false);
  let whitespace = $state<Whitespace>("default");
  let question = $state.raw<Question | null>(null);
  let menu = $state.raw<{ x: number; y: number; items: MenuItem[] } | null>(null);

  const binary = $derived(
    sides !== null && [sides.base, sides.yours, sides.theirs].some((side) => side.kind === "binary"),
  );
  const text = $derived(loaded && !binary && failure === null);
  const counts = $derived.by(() => {
    void version;
    return editors?.counts() ?? { changes: 0, conflicts: 0 };
  });
  const enabled = $derived.by(() => {
    void version;
    return {
      left: editors?.hasNonConflicting("left") ?? false,
      all: editors?.hasNonConflicting("all") ?? false,
      right: editors?.hasNonConflicting("right") ?? false,
      resolvable: editors?.hasResolvable() ?? false,
    };
  });

  const contentOf = (version: ConflictVersion) => (version.kind === "text" ? version.text : "");

  function ask(details: Omit<Question, "answer">): Promise<boolean> {
    return new Promise((resolve) => {
      question = {
        ...details,
        answer: (yes) => {
          question = null;
          resolve(yes);
        },
      };
    });
  }

  // MergeUtil.showConfirmDiscardChangesDialog.
  function discard(action: string, modified: boolean): Promise<boolean> {
    if (!modified) return Promise.resolve(true);
    return ask({
      title: action,
      message: `There are unsaved changes in the result file. Discard changes and ${action.toLowerCase()} anyway?`,
      yes: `Discard Changes and ${action}`,
      no: CONTINUE,
    });
  }

  async function finish(result: MergeResult) {
    busy = true;
    await mergeWindowDone(result);
  }

  // A command that writes. If the file changed, the red bar; if something else fails, the error.
  async function write(work: () => Promise<void>) {
    busy = true;
    error = null;
    try {
      await work();
    } catch (failed) {
      busy = false;
      if (isOutdated(failed)) outdated = true;
      else error = errorText(failed);
    }
  }

  async function accept(side: "left" | "right") {
    if (busy || question || outdated || !path) return;
    const action = side === "left" ? "Accept Left" : "Accept Right";
    if (!(await discard(action, editors?.modified() ?? false))) return;
    // R1: the side as the user sees it; the backend inverts it in a rebase.
    const chosen: ConflictSide = side === "left" ? "yours" : "theirs";
    const target = path;
    await write(async () => {
      await taskResult(await conflictsAccept([target], chosen));
      await finish(chosen);
    });
  }

  // doFinishMerge(RESOLVED): without asking.
  async function save() {
    if (busy || outdated || !editors || !sides || !path) return;
    const content = withSeparator(editors.result(), separator);
    const target = path;
    const hash = sides.hash;
    await write(async () => {
      await taskResult(await conflictResolve(target, content, hash));
      await finish("resolved");
    });
  }

  async function apply() {
    if (busy || question || outdated || !editors) return;
    const pending = editors.counts();
    if (pending.changes + pending.conflicts > 0) {
      const yes = await ask({
        title: "Apply Changes",
        message: unprocessedMessage(pending),
        yes: "Apply Changes and Mark Resolved",
        no: CONTINUE,
      });
      if (!yes) return;
    }
    await save();
  }

  // MergeUtil.showExitWithoutApplyingChangesDialog. A binary never asks.
  async function cancel() {
    if (busy || question) return;
    if (!(await discard("Cancel Merge", editors?.modified() ?? false))) return;
    await finish("cancelled");
  }

  function autoApply() {
    if (settings?.mergeAutoApply && editors?.hasNonConflicting("all")) {
      editors.applyNonConflicting("all");
    }
  }

  // restartMergeResolveIfNeeded: another policy starts the merge over.
  async function changeWhitespace(policy: Whitespace) {
    if (!editors || !texts || policy === whitespace || busy) return;
    if (editors.modified()) {
      const yes = await ask({
        title: "Update Highlighting Settings",
        message:
          "Changing highlighting requires the file merge restart. Discard unsaved changes and restart merge anyway?",
        yes: "Discard Changes and Restart Merge",
        no: CONTINUE,
      });
      if (!yes) return;
    }
    try {
      const blocks = await mergeBlocks(texts.left, texts.base, texts.right, policy);
      editors.setWhitespace(policy, blocks);
      whitespace = policy;
      processed = false;
      autoApply();
      version++;
      if (settings) settings = await settingsSave({ ...settings, mergeWhitespace: policy });
    } catch (failed) {
      error = errorText(failed);
    }
  }

  function openWhitespace(event: MouseEvent) {
    const bounds = (event.currentTarget as HTMLElement).getBoundingClientRect();
    menu = {
      x: bounds.left,
      y: bounds.bottom + 2,
      items: WHITESPACE.map((choice) => ({
        label: choice.label,
        checked: choice.value === whitespace,
        run: () => changeWhitespace(choice.value),
      })),
    };
  }

  // Another command would throw away the merge: it asks as in Cancel, without closing; whoever asked decides.
  async function leaveRequested() {
    await mergeLeaveReceived();
    if (busy || question) {
      await mergeLeft(false);
      return;
    }
    await mergeLeft(await discard("Cancel Merge", editors?.modified() ?? false));
  }

  // MergeCallback.checkIsValid: the working file is no longer the one that was read, or it is no longer in
  // conflict because someone marked it resolved outside.
  async function check() {
    if (!path || !sides || outdated || busy) return;
    try {
      const file = await mergeWorkingFile(path);
      if (file.hash !== sides.hash || !file.conflicted) outdated = true;
    } catch {
      // If it cannot be read, Apply will say so.
    }
  }

  function changed() {
    version++;
    if (editors && (counts.changes > 0 || counts.conflicts > 0)) processed = false;
  }

  // CTRL_ENTER goes to the default button, before the editor, which with ⌘⏎ would add a line.
  function keydown(event: KeyboardEvent) {
    if (question || menu || !text) return;
    const command = event.metaKey || event.ctrlKey;
    if (event.key === "Enter" && command && !event.shiftKey && !event.altKey) {
      event.preventDefault();
      event.stopPropagation();
      apply();
    } else if (event.key === "F7" && !command && !event.altKey) {
      event.preventDefault();
      editors?.navigate(event.shiftKey ? -1 : 1);
    }
  }

  async function load() {
    settings = await settingsGet();
    whitespace = settings.mergeWhitespace;
    path = await mergeWindowPath();
    if (!path) throw new Error("There is no file to resolve");
    const read = await taskResult<ConflictSides>(await conflictSides(path));
    const file = await mergeWorkingFile(path);
    separator = file.separator;
    outdated = file.hash !== read.hash || !file.conflicted;
    sides = read;
    if (binary) return;
    const raw = { left: contentOf(read.yours), base: contentOf(read.base), right: contentOf(read.theirs) };
    texts = {
      left: normalized(raw.left),
      base: normalized(raw.base),
      right: normalized(raw.right),
    };
    // The blocks of `sides` are those of the text read with the default policy.
    const same = texts.left === raw.left && texts.base === raw.base && texts.right === raw.right;
    const blocks =
      same && whitespace === "default"
        ? read.blocks
        : await mergeBlocks(texts.left, texts.base, texts.right, whitespace);
    await tick();
    if (!host) return;
    editors = new MergeEditors(
      host,
      { ...texts, blocks, titles: read.titles, path: read.path },
      { onchange: changed, onallprocessed: () => (processed = true) },
    );
    autoApply();
    version++;
  }

  onMount(() => {
    const stops = [
      trackTasks(),
      onRepoChanged(check),
      onMergeClose(cancel),
      onMergeLeave(leaveRequested),
      onSettingsChanged((next) => (settings = next)),
    ];
    (async () => {
      try {
        await Promise.all(stops);
        await mergeWindowReady();
        await load();
      } catch (failed) {
        failure = `Error loading revisions to merge: ${errorText(failed)}`;
      } finally {
        loaded = true;
        await showWindow();
      }
    })();
    return () => {
      editors?.destroy();
      stops.forEach((stop) => stop.then((unlisten) => unlisten()));
    };
  });
</script>

<svelte:window onkeydowncapture={keydown} />

<main>
  {#if text}
    <div class="toolbar" role="toolbar" aria-label="Merge">
      <button type="button" class="choice" onclick={openWhitespace} disabled={busy}>
        {WHITESPACE.find((choice) => choice.value === whitespace)?.label}
        <Icon name="chevronDown" size={12} />
      </button>
      <span class="toolbar-separator"></span>
      <span class="label">Apply non-conflicting changes:</span>
      <button
        type="button"
        class="choice"
        disabled={!enabled.left}
        onclick={() => editors?.applyNonConflicting("left")}>Left</button
      >
      <button
        type="button"
        class="choice"
        disabled={!enabled.all}
        onclick={() => editors?.applyNonConflicting("all")}>All</button
      >
      <button
        type="button"
        class="choice"
        disabled={!enabled.right}
        onclick={() => editors?.applyNonConflicting("right")}>Right</button
      >
      <button
        type="button"
        class="choice"
        disabled={!enabled.resolvable}
        onclick={() => editors?.resolveAutomatically()}>Resolve Automatically</button
      >
      <span class="toolbar-separator"></span>
      <button
        type="button"
        class="action"
        title="Previous Difference (⇧F7)"
        aria-label="Previous Difference"
        onclick={() => editors?.navigate(-1)}
      >
        <Icon name="up" />
      </button>
      <button
        type="button"
        class="action"
        title="Next Difference (F7)"
        aria-label="Next Difference"
        onclick={() => editors?.navigate(1)}
      >
        <Icon name="down" />
      </button>
      <span class="status">{statusText(counts)}</span>
    </div>
  {/if}

  {#if outdated}
    <div class="invalid" role="alert">
      <span>Conflict is not valid and no longer can be resolved.</span>
      <button type="button" class="link" onclick={() => finish("cancelled")}>Abort Resolve</button>
    </div>
  {/if}
  {#if error}
    <div class="invalid" role="alert"><span>{error}</span></div>
  {/if}

  <div class="content">
    {#if failure}
      <p class="message">{failure}</p>
    {:else if binary && sides}
      <div class="binary">
        {#each sides.titles as title, index (index)}
          <section>
            <header>{title}</header>
          </section>
        {/each}
      </div>
    {/if}
    <div class="host" bind:this={host}></div>
    {#if processed}
      <div class="processed" role="status">
        <p>All changes have been processed.</p>
        <button type="button" class="link" onclick={save}>Save changes and finish merging</button>
      </div>
    {/if}
  </div>

  {#if loaded}
    <div class="buttons">
      <div class="left">
        {#if !failure}
          <button
            type="button"
            class="button"
            disabled={busy || outdated}
            onclick={() => accept("left")}
            >Accept Left</button
          >
          <button
            type="button"
            class="button"
            disabled={busy || outdated}
            onclick={() => accept("right")}
            >Accept Right</button
          >
        {/if}
      </div>
      <div class="right">
        <button type="button" class="button" disabled={busy} onclick={cancel}>Cancel</button>
        {#if text}
          <button
            type="button"
            class="button default"
            title="Apply (⌘⏎)"
            disabled={busy || outdated}
            onclick={apply}>Apply</button
          >
        {/if}
      </div>
    </div>
  {/if}
</main>

{#if question}
  {@const current = question}
  <Dialog
    title={current.title}
    width={420}
    onclose={() => current.answer(false)}
    onsubmit={() => current.answer(true)}
  >
    {#each current.message.split("\n") as line, index (index)}
      <p>{line}</p>
    {/each}
    {#snippet footer()}
      <button type="button" class="button" onclick={() => current.answer(false)}>{current.no}</button>
      <button class="button default" data-autofocus>{current.yes}</button>
    {/snippet}
  </Dialog>
{/if}

{#if menu}
  <Menu
    x={menu.x}
    y={menu.y}
    label="Ignore Changes"
    items={menu.items}
    onclose={() => (menu = null)}
  />
{/if}

<style>
  main {
    position: relative;
    display: flex;
    flex-direction: column;
    height: 100%;
    background: var(--editor);
  }

  .toolbar {
    display: flex;
    flex: none;
    gap: 2px;
    align-items: center;
    height: 40px;
    box-sizing: border-box;
    padding: 0 8px;
    background: var(--panel);
    border-bottom: 1px solid var(--border);
  }

  .label {
    padding: 0 4px;
    color: var(--text-secondary);
    white-space: nowrap;
  }

  .choice {
    display: inline-flex;
    gap: 4px;
    align-items: center;
    height: 26px;
    padding: 0 6px;
    font: inherit;
    color: var(--text);
    white-space: nowrap;
    background: none;
    border: 0;
    border-radius: 6px;
  }

  .choice:hover:not(:disabled) {
    background: var(--hover);
  }

  .choice:disabled {
    color: var(--disabled);
  }

  .status {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    color: var(--text-secondary);
    text-align: right;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* EditorNotificationPanel with LightColors.RED. */
  .invalid {
    display: flex;
    flex: none;
    gap: 12px;
    align-items: center;
    padding: 6px 12px;
    color: var(--text);
    background: var(--notice-error);
    border-bottom: 1px solid var(--notice-error-border);
  }

  .link {
    padding: 0;
    font: inherit;
    color: var(--link);
    white-space: nowrap;
    background: none;
    border: 0;
  }

  .content {
    position: relative;
    display: flex;
    flex: 1;
    flex-direction: column;
    min-height: 0;
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

  .host:empty {
    display: none;
  }

  .host :global(.rm-merge) {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }

  .host :global(.rm-merge-titles),
  .binary {
    display: flex;
    flex: none;
    height: 28px;
    color: var(--text-secondary);
    background: var(--panel);
    border-bottom: 1px solid var(--divider);
  }

  .host :global(.rm-merge-title),
  .binary header {
    display: flex;
    flex: 1;
    align-items: center;
    min-width: 0;
    padding: 0 8px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .host :global(.rm-merge-title-gap) {
    flex: none;
    width: 30px;
  }

  .binary section {
    display: flex;
    flex: 1;
    min-width: 0;
  }

  .binary section + section {
    border-left: 1px solid var(--divider);
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

  .host :global(.rm-editor .cm-editor) {
    height: 100%;
  }

  .host :global(.rm-divider) {
    flex: none;
    width: 30px;
    background: var(--panel);
  }

  .host :global(.rm-inserted-band) {
    fill: var(--diff-inserted);
    stroke: var(--diff-inserted-stripe);
  }

  .host :global(.rm-deleted-band) {
    fill: var(--diff-deleted);
    stroke: var(--diff-deleted-stripe);
  }

  .host :global(.rm-modified-band) {
    fill: var(--diff-modified);
    stroke: var(--diff-modified-stripe);
  }

  .host :global(.rm-conflict-band) {
    fill: var(--diff-conflict);
    stroke: var(--diff-conflict-stripe);
  }

  /* What is resolved keeps only the border, like DiffDrawUtil with `withResolved`. */
  .host :global(.rm-divider path) {
    stroke-width: 0;
  }

  .host :global(.rm-divider path.rm-resolved-band) {
    fill: none;
    stroke-width: 1;
  }

  /* DiffUtil.showSuccessPopup, at the top center of the result. */
  .processed {
    position: absolute;
    top: 34px;
    left: 50%;
    z-index: 5;
    padding: 8px 12px;
    color: var(--notification-text);
    text-align: center;
    background: var(--notification-background);
    border: 1px solid var(--notification-border);
    border-radius: 8px;
    box-shadow: 0 4px 16px rgb(0 0 0 / 35%);
    transform: translateX(-50%);
  }

  .processed p {
    margin: 0 0 2px;
  }

  .buttons {
    display: flex;
    flex: none;
    gap: 8px;
    align-items: center;
    justify-content: space-between;
    padding: 12px 16px;
    background: var(--panel);
    border-top: 1px solid var(--border);
  }

  .left,
  .right {
    display: flex;
    gap: 8px;
  }

  p {
    margin: 0 0 6px;
  }
</style>
