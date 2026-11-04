<script lang="ts">
  import { onMount, tick, untrack } from "svelte";
  import type { CommitSummary, PushInfo, PushTags } from "../lib/api";
  import Dialog from "../lib/Dialog.svelte";
  import { errorText } from "../lib/errors";
  import Icon from "../lib/Icon.svelte";
  import Menu, { type MenuItem } from "../lib/Menu.svelte";
  import type { LogChange } from "../log/changes";
  import ChangesPanel from "../log/ChangesPanel.svelte";
  import DetailsPanel from "../log/DetailsPanel.svelte";
  import { defaultOptions, type LogOptions } from "../log/options";
  import { forcePushProhibited, isProtectedBranch, silentTarget } from "./force";
  import ForcePushDialog from "./ForcePushDialog.svelte";
  import {
    FIRST_COMMITS,
    commitTooltip,
    pushTitle,
    shownCommits,
    targetIsNew,
    targetNames,
    targetProblem,
    type PushChoice,
    type PushSource,
  } from "./model";

  // `VcsPushDialog` with `PushLog`, without the repository selector: there is one per window. With
  // `preview`, `GitPushAfterCommitDialog`.
  interface Props {
    info: PushInfo;
    source: PushSource;
    tags: PushTags | null;
    outgoing: (remote: string, target: string) => Promise<CommitSummary[]>;
    defineRemote?: () => Promise<PushInfo | null>;
    preview?: boolean | null;
    onanswer: (choice: PushChoice | null) => void;
  }

  let { info, source, tags, outgoing, defineRemote, preview = null, onanswer }: Props = $props();

  const REPOSITORY_ROW = "repository";
  const WIDTH = 800;
  const WIDE = 1200;
  const HOOKS_TOOLTIP = "If unchecked, Git hooks will be skipped with the '--no-verify' parameter";
  const PREVIEW_LABEL =
    "For Commit and Push to non-protected branches, preview commits before push";
  const TAG_MODES: { id: PushTags; label: string }[] = [
    { id: "all", label: "All" },
    { id: "currentBranch", label: "Current Branch" },
  ];

  const first = untrack(() => info);
  const remembered = untrack(() => tags);
  let current = $state.raw(first);
  let remote = $state(first.target.remote);
  let branch = $state(first.target.branch);
  // While it is not `null` the target is being edited, and pushing is not possible.
  let draft = $state<string | null>(null);
  let problem = $state<string | null>(null);
  let commits = $state.raw<CommitSummary[]>([]);
  let limit = $state(FIRST_COMMITS);
  let loading = $state(false);
  let failure = $state<string | null>(null);
  let selected = $state.raw<string[]>([]);
  let anchor: string | null = null;
  let pushTags = $state(remembered !== null);
  let tagMode = $state<PushTags>(remembered ?? "all");
  let runHooks = $state(true);
  let menu = $state.raw<{ x: number; y: number } | null>(null);
  let forceMenu = $state.raw<{ x: number; y: number } | null>(null);
  let confirming = $state(false);
  let previewAgain = $state(untrack(() => preview) ?? true);
  let options = $state.raw<LogOptions>(defaultOptions());
  let change = $state.raw<LogChange | null>(null);
  let diffOpen = $state(false);
  let tree = $state<HTMLDivElement>();
  let request = 0;

  const visible = $derived(shownCommits(commits, limit));
  const rowIds = $derived([REPOSITORY_ROW, ...visible.shown.map(({ hash }) => hash)]);
  const repositorySelected = $derived(selected.includes(REPOSITORY_ROW));
  // `collectSelectedCommitNodes`: the branch row stands for all its commits.
  const hashes = $derived(
    rowIds.filter(
      (id) => id !== REPOSITORY_ROW && (repositorySelected || selected.includes(id)),
    ),
  );
  const details = $derived(hashes.length === 1 && !repositorySelected ? hashes : []);
  const names = $derived(targetNames(current, remote));
  const isNew = $derived(remote !== null && targetIsNew(current, remote, branch));
  const canPush = $derived(
    remote !== null && current.head !== null && draft === null && failure === null,
  );
  const showDiff = $derived((options.diffPreview || diffOpen) && change !== null);
  const prohibited = $derived(isProtectedBranch(current.protectedBranches, branch));
  // `createOptionsPanel`: it is decided on open, and with a protected target it does not even appear.
  const showPreview =
    untrack(() => preview) !== null &&
    !isProtectedBranch(first.protectedBranches, first.target.branch);

  async function load() {
    const token = ++request;
    failure = null;
    if (remote === null || current.head === null) return;
    loading = true;
    try {
      const found = await outgoing(remote, branch);
      if (token !== request) return;
      commits = found;
      limit = FIRST_COMMITS;
      selected = found.length > 0 ? [REPOSITORY_ROW] : [];
    } catch (error) {
      if (token !== request) return;
      commits = [];
      selected = [];
      failure = errorText(error);
    } finally {
      if (token === request) loading = false;
    }
  }

  onMount(load);

  function select(id: string, event: MouseEvent) {
    if (event.shiftKey && anchor !== null) {
      const ends = [rowIds.indexOf(anchor), rowIds.indexOf(id)].sort((a, b) => a - b);
      selected = rowIds.slice(ends[0], ends[1] + 1);
      return;
    }
    anchor = id;
    if (!event.metaKey) selected = [id];
    else if (selected.includes(id)) selected = selected.filter((other) => other !== id);
    else selected = [...selected, id];
  }

  function move(step: number) {
    const at = rowIds.indexOf(selected[selected.length - 1]);
    const next = rowIds[Math.min(Math.max(at + step, 0), rowIds.length - 1)];
    anchor = next;
    selected = [next];
  }

  function editTarget() {
    if (remote !== null) draft = branch;
  }

  function closeEditor() {
    draft = null;
    problem = null;
    tick().then(() => tree?.focus());
  }

  function applyTarget(name: string) {
    closeEditor();
    if (name === branch) return;
    branch = name;
    load();
  }

  // `stopEditing`: with Enter, what is not valid stays to be corrected.
  function targetKey(event: KeyboardEvent) {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      closeEditor();
    } else if (event.key === "Enter") {
      event.preventDefault();
      problem = targetProblem(draft ?? "");
      if (!problem) applyTarget(draft ?? "");
    }
  }

  // `stopEditing` with `setInvokesStopCellEditing`: what is not valid is neither accepted nor swapped
  // for the previous target. Otherwise, the click on Push that leaves the field would push elsewhere.
  function targetBlur() {
    if (draft === null) return;
    problem = targetProblem(draft);
    if (!problem) applyTarget(draft);
  }

  function treeKey(event: KeyboardEvent) {
    if ((event.target as HTMLElement).closest("button, input")) return;
    if (event.key === "F2") editTarget();
    else if (event.key === "ArrowDown") move(1);
    else if (event.key === "ArrowUp") move(-1);
    else if (event.key === "Enter") submit();
    else return;
    event.preventDefault();
  }

  function chooseRemote(name: string) {
    if (name === remote) return;
    remote = name;
    load();
  }

  async function define() {
    const fresh = await defineRemote?.();
    if (!fresh) return;
    current = fresh;
    remote = fresh.target.remote;
    branch = fresh.target.branch;
    await load();
  }

  function openRemotes(event: MouseEvent) {
    const bounds = (event.currentTarget as HTMLElement).getBoundingClientRect();
    menu = { x: bounds.left, y: bounds.bottom };
  }

  // `getPopupItems`.
  const remoteItems = (): MenuItem[] => [
    ...current.remotes.map((name) => ({ label: name, run: () => chooseRemote(name) })),
    null,
    { label: "Define remote", disabled: !defineRemote, run: define },
  ];

  function answer(force: boolean, rememberTarget: boolean) {
    if (!canPush || remote === null) return;
    onanswer({
      remote,
      target: branch,
      tags: pushTags ? tagMode : null,
      noVerify: current.prePushHook && !runHooks,
      force,
      preview: !showPreview || previewAgain,
      rememberTarget,
    });
  }

  const submit = () => answer(false, false);

  // `ForcePushAction`. The core is what rules: this only avoids offering what it would reject.
  function forcePush() {
    if (!canPush || remote === null || prohibited) return;
    const silent = current.forcePushTargets.includes(silentTarget(remote, branch));
    if (silent) answer(true, false);
    else confirming = true;
  }

  function openForceMenu(event: MouseEvent) {
    const bounds = (event.currentTarget as HTMLElement).getBoundingClientRect();
    forceMenu = { x: bounds.left, y: bounds.bottom };
  }

  const forceItems = (): MenuItem[] => [
    {
      label: "Force Push",
      disabled: prohibited,
      title: prohibited ? forcePushProhibited(branch) : undefined,
      run: forcePush,
    },
  ];
</script>

{#snippet target()}
  {#if current.head === null}
    <span class="error">Empty repository</span>
  {:else if remote === null}
    <button
      type="button"
      class="link"
      disabled={!defineRemote}
      onclick={(event) => {
        event.stopPropagation();
        define();
      }}
    >Define remote</button>
  {:else}
    <button
      type="button"
      class="link"
      aria-label="Remote"
      onclick={(event) => {
        event.stopPropagation();
        openRemotes(event);
      }}
    >{remote}</button>
    <span> : </span>
    {#if draft === null}
      <button
        type="button"
        class="link"
        aria-label="Target branch"
        onclick={(event) => {
          event.stopPropagation();
          editTarget();
        }}
      >{branch}</button>
      {#if isNew}
        <span class="new">New</span>
      {/if}
    {:else}
      <!-- svelte-ignore a11y_autofocus -->
      <input
        class="input target"
        aria-label="Target branch"
        list="push-targets"
        autofocus
        value={draft}
        oninput={(event) => {
          draft = event.currentTarget.value;
          problem = null;
        }}
        onkeydown={targetKey}
        onblur={targetBlur}
        onclick={(event) => event.stopPropagation()}
      />
      <datalist id="push-targets">
        {#each names as name (name)}
          <option value={name}></option>
        {/each}
      </datalist>
    {/if}
  {/if}
{/snippet}

<Dialog
  title={pushTitle(current.repository)}
  width={showDiff && options.diffPreviewAt === "right" ? WIDE : WIDTH}
  onclose={() => onanswer(null)}
  onsubmit={submit}
>
  <div class="frame" class:with-diff={showDiff} data-location={options.diffPreviewAt}>
    <div class="main">
      <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
      <div
        bind:this={tree}
        class="tree"
        role="tree"
        aria-label="Commits to push"
        aria-multiselectable="true"
        tabindex="0"
        data-autofocus
        onkeydown={treeKey}
      >
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <div
          role="treeitem"
          aria-label={source.label}
          aria-selected={repositorySelected}
          tabindex="-1"
          class="row"
          class:current={repositorySelected}
          onclick={(event) => select(REPOSITORY_ROW, event)}
        >
          <span>{source.label}</span>
          <span> → </span>
          {@render target()}
        </div>
        {#if problem}
          <p class="status error" role="alert">{problem}</p>
        {/if}
        {#if failure}
          <p class="status error">{failure}</p>
        {:else if loading && commits.length === 0}
          <p class="status" role="status">Loading…</p>
        {/if}
        {#each visible.shown as commit (commit.hash)}
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <div
            role="treeitem"
            aria-label={commit.subject}
            aria-selected={selected.includes(commit.hash)}
            tabindex="-1"
            class="row commit"
            class:current={selected.includes(commit.hash)}
            title={commitTooltip(commit)}
            onclick={(event) => select(commit.hash, event)}
          >
            {commit.subject}
          </div>
        {/each}
        {#if visible.more}
          <button type="button" class="link more" onclick={() => (limit *= 2)}>...</button>
        {/if}
      </div>
      <div class="side">
        <div class="pane changes">
          <ChangesPanel
            {hashes}
            paths={[]}
            {options}
            onoptions={(next) => (options = { ...options, ...next })}
            onselect={(next) => (change = next)}
            onopen={() => (diffOpen = true)}
          />
        </div>
        {#if options.showDetails && details.length === 1}
          <div class="pane details">
            <DetailsPanel
              hashes={details}
              generation={0}
              showBranches={false}
              onnavigate={(hash) => {
                if (rowIds.includes(hash)) selected = [hash];
              }}
            />
          </div>
        {/if}
      </div>
    </div>
    {#if showDiff && change}
      <section class="preview" aria-label="Diff Preview">
        {#if diffOpen && !options.diffPreview}
          <button
            type="button"
            class="action close-diff"
            aria-label="Close"
            onclick={() => (diffOpen = false)}
          >
            <Icon name="close" size={14} />
          </button>
        {/if}
        {#await import("../diff/DiffView.svelte") then { default: DiffView }}
          <DiffView hash={change.hash} file={change.file} base={change.base} />
        {/await}
      </section>
    {/if}
  </div>
  {#if menu}
    <Menu {...menu} label="Remote" items={remoteItems()} onclose={() => (menu = null)} />
  {/if}
  {#if forceMenu}
    <Menu {...forceMenu} label="Push" items={forceItems()} onclose={() => (forceMenu = null)} />
  {/if}
  {#snippet footer()}
    <div class="options">
      <label class="checkbox-label">
        <input type="checkbox" aria-label="Push tags:" bind:checked={pushTags} />
        <span class="check" data-state={pushTags ? "on" : "off"}></span>
        <span>Push tags:</span>
      </label>
      <select class="input" aria-label="Push tags mode" disabled={!pushTags} bind:value={tagMode}>
        {#each TAG_MODES as { id, label } (id)}
          <option value={id}>{label}</option>
        {/each}
      </select>
      {#if current.prePushHook}
        <label class="checkbox-label" title={HOOKS_TOOLTIP}>
          <input type="checkbox" aria-label="Run Git hooks" bind:checked={runHooks} />
          <span class="check" data-state={runHooks ? "on" : "off"}></span>
          <span>Run Git hooks</span>
        </label>
      {/if}
      {#if showPreview}
        <label class="checkbox-label">
          <input type="checkbox" aria-label={PREVIEW_LABEL} bind:checked={previewAgain} />
          <span class="check" data-state={previewAgain ? "on" : "off"}></span>
          <span>{PREVIEW_LABEL}</span>
        </label>
      {/if}
    </div>
    <button type="button" class="button" onclick={() => onanswer(null)}>Cancel</button>
    <div class="split">
      <button class="button default" disabled={!canPush}>Push</button>
      <button
        type="button"
        class="button default more"
        aria-label="More push options"
        disabled={!canPush}
        onclick={openForceMenu}
      >
        <Icon name="chevronDown" size={12} />
      </button>
    </div>
  {/snippet}
</Dialog>

{#if confirming}
  <ForcePushDialog
    {branch}
    onanswer={(confirmed) => {
      confirming = false;
      if (confirmed) answer(true, confirmed.remember);
    }}
  />
{/if}

<style>
  .frame {
    display: flex;
    flex-direction: column;
    height: 450px;
    max-height: calc(100vh - 200px);
  }

  .frame.with-diff {
    height: 700px;
  }

  .frame.with-diff[data-location="right"] {
    flex-direction: row;
    height: 450px;
  }

  .main {
    display: flex;
    flex: 1;
    min-width: 0;
    min-height: 0;
    border: 1px solid var(--divider);
  }

  .tree {
    flex: 1;
    min-width: 0;
    padding: 4px 0;
    overflow: auto;
    outline: none;
    border-right: 1px solid var(--divider);
  }

  .row {
    display: flex;
    gap: 4px;
    align-items: center;
    min-height: 24px;
    padding: 0 8px;
    white-space: nowrap;
  }

  .row.current {
    background: var(--selection);
  }

  .commit {
    display: block;
    padding-left: 24px;
    overflow: hidden;
    line-height: 24px;
    text-overflow: ellipsis;
  }

  .link {
    padding: 0;
    font: inherit;
    color: var(--link);
    background: none;
    border: 0;
  }

  .link:disabled {
    color: var(--disabled);
  }

  .more {
    padding-left: 24px;
  }

  .new {
    padding: 0 4px;
    font-size: 10px;
    color: var(--status-added);
    border: 1px solid var(--status-added);
    border-radius: 4px;
  }

  .target {
    flex: 1;
    min-width: 80px;
    height: 22px;
  }

  .status {
    padding: 4px 24px;
    margin: 0;
    color: var(--text-secondary);
  }

  .error {
    color: var(--status-deleted);
  }

  .side {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
  }

  .pane {
    min-height: 0;
  }

  .changes {
    flex: 0.67;
  }

  .details {
    flex: 0.33;
    border-top: 1px solid var(--divider);
  }

  .preview {
    position: relative;
    display: flex;
    flex: 1;
    min-width: 0;
    min-height: 0;
    overflow: auto;
  }

  .close-diff {
    position: absolute;
    top: 4px;
    right: 4px;
    z-index: 1;
  }

  .options {
    display: flex;
    gap: 8px;
    align-items: center;
    margin-right: auto;
  }

  .split {
    display: flex;
    gap: 1px;
  }

  .split .button:first-child {
    border-top-right-radius: 0;
    border-bottom-right-radius: 0;
  }

  .more {
    display: inline-flex;
    align-items: center;
    padding: 0 6px;
    border-top-left-radius: 0;
    border-bottom-left-radius: 0;
  }
</style>
