<script lang="ts">
  import { untrack } from "svelte";
  import ChangesTree from "../commit/ChangesTree.svelte";
  import { fileNodes, withoutCheck, type TreeNode } from "../commit/tree";
  import {
    diffWindowOpenRequest,
    LOCAL_REVISION,
    type ChangedFile,
    type FileChange,
    type FileState,
    type GetItem,
    type PatchEntry,
    type RevisionPair,
  } from "../lib/api";
  import { errorText } from "../lib/errors";
  import Icon from "../lib/Icon.svelte";
  import { matchShortcut } from "../lib/keymap";
  import Menu, { type MenuItem } from "../lib/Menu.svelte";
  import { notify } from "../lib/notifications";
  import { patches } from "../patch/flow";
  import { getRevision } from "./get";
  import {
    BASE_VERSION,
    changesTab,
    emptyChanges,
    hasLocalSide,
    refreshChanges,
    reloadBranchChanges,
    swapBranchSides,
    YOUR_VERSION,
    type ChangesTab,
  } from "./tab";

  // `DB:31`, `V:1035`, `V:298`, `DB:91`, `DB:94`.
  const NO_DIFFERENCES = "No differences";
  const CANNOT_LOAD = "Cannot load changes";
  const LOADING = "Loading...";
  const diffTitle = (name: string) => `Changes: ${name}`;
  // `DvcsBundle:91-95`.
  const GET_FROM_BRANCH = "Get from Branch";
  const getFromBranchText = (branch: string) =>
    `Replace file content with its version from branch ${branch}`;
  const getFromBranchTitle = (branch: string) => `Get from Branch '${branch}'`;
  const CANNOT_COPY = "Cannot Copy Changes";
  const LOADING_FROM_BRANCH = "Loading Content from Branch…";
  const NO_CHECKS = new Set<string>();
  const SCOPE = "changes";

  interface Props {
    repo: string;
    active: boolean;
  }

  let { repo, active }: Props = $props();

  let collapsed = $state<Set<string>>(new Set());
  let selected = $state<Set<string>>(new Set());
  let lead = $state<string | null>(null);
  // `ChangesTree`: `Directory` grouping is on by default.
  let byDirectory = $state(true);
  let loading = $state(false);
  // A Get in progress: another one waits for it.
  let getting = $state(false);
  let failure = $state<string | null>(null);
  let menu = $state.raw<{ x: number; y: number; label: string; items: MenuItem[] } | null>(null);
  let tree = $state<ChangesTree>();
  let shownId: number | null = null;

  const tab = $derived($changesTab);
  // Show Diff with Working Tree has its own panel (`CompareBranchesDiffPanel`): no Refresh and no
  // Get of `CompareWithLocalDialog`, but Get from Branch, whichever side the working tree is on.
  const branch = $derived(tab?.branch ?? null);
  const local = $derived(tab && !branch ? hasLocalSide(tab) : false);
  const swapped = $derived(tab?.right === LOCAL_REVISION);

  // `git diff --name-status` letters as the states the changes tree colours.
  const STATES: Record<string, FileState> = {
    A: "added",
    C: "copied",
    D: "deleted",
    R: "renamed",
  };
  const stateOf = (file: ChangedFile): FileState => STATES[file.status[0]] ?? "modified";
  const asChange = (file: ChangedFile): FileChange => ({
    path: file.path,
    oldPath: file.oldPath,
    state: stateOf(file),
    staged: false,
    unstaged: false,
  });

  const byPath = $derived(new Map((tab?.files ?? []).map((file) => [file.path, file])));
  const roots = $derived(
    fileNodes(SCOPE, new Set(), (tab?.files ?? []).map(asChange), byDirectory).map(withoutCheck),
  );

  // A new tab starts without selection or folded folders; a refresh keeps them (`KEEP_NON_EMPTY`).
  $effect(() => {
    const id = tab?.id ?? null;
    untrack(() => {
      if (id === shownId) return;
      shownId = id;
      collapsed = new Set();
      // `selectFile(toSelect)`: the file asked for, when the list has it.
      const chosen = tab?.select;
      const chosenId = chosen !== undefined && byPath.has(chosen) ? `file:${chosen}` : null;
      selected = new Set(chosenId ? [chosenId] : []);
      lead = chosenId;
      failure = null;
    });
  });

  $effect(() => {
    if (active && tab) untrack(() => tree?.focus());
  });

  function nodesById(nodes: TreeNode[], into = new Map<string, TreeNode>()) {
    for (const node of nodes) {
      into.set(node.id, node);
      nodesById(node.children, into);
    }
    return into;
  }

  // `VcsTreeModelData.selected`: the files of the chosen rows, a folder giving all of its own, in
  // the order of the tree.
  function selectedFiles(): ChangedFile[] {
    const nodes = nodesById(roots);
    const paths = new Set<string>();
    const visit = (node: TreeNode) => {
      if (node.change) paths.add(node.change.path);
      node.children.forEach(visit);
    };
    for (const id of selected) {
      const node = nodes.get(id);
      if (node) visit(node);
    }
    const files: ChangedFile[] = [];
    const ordered = (nodes: TreeNode[]) => {
      for (const node of nodes) {
        const file = node.change && byPath.get(node.change.path);
        if (file && paths.has(file.path)) files.push(file);
        ordered(node.children);
      }
    };
    ordered(roots);
    return files;
  }

  // `ChangeDiffRequestProducer.createSimpleRequest`: the revision number of each side, `Base
  // version` where the old file is not and `Your version` for the local file or none.
  // The local side of Show Diff with Working Tree is on the left: a `CurrentContentRevision`, whose
  // empty number gives the left default, `Base version`.
  function pairOf(current: ChangesTab, file: ChangedFile): RevisionPair {
    const kind = file.status[0];
    const leftTitle = current.left === LOCAL_REVISION ? BASE_VERSION : current.left;
    const rightTitle = current.right === LOCAL_REVISION ? YOUR_VERSION : current.right;
    return {
      left:
        kind === "A"
          ? { revision: null, path: file.path, title: BASE_VERSION }
          : { revision: current.left, path: file.oldPath ?? file.path, title: leftTitle },
      right:
        kind === "D"
          ? { revision: null, path: file.path, title: YOUR_VERSION }
          : { revision: current.right, path: file.path, title: rightTitle },
    };
  }

  const fileName = (path: string) => path.slice(path.lastIndexOf("/") + 1);

  // `Show Diff` (⌘D), double click and Enter: the chosen files in the diff window.
  function showDiff() {
    const current = tab;
    const files = selectedFiles();
    if (!current || files.length === 0) return;
    diffWindowOpenRequest({
      kind: "revisions",
      files: files.map((file) => pairOf(current, file)),
      title: diffTitle(fileName(files[0].path)),
    }).catch((error) => notify({ kind: "error", title: errorText(error) }));
  }

  // `ChangesView.CreatePatchFromChanges`: each file between the two sides of the tab.
  function createPatch() {
    const current = tab;
    const files = selectedFiles();
    if (!current || files.length === 0) return;
    const entries: PatchEntry[] = files.map((file) => ({
      path: file.path,
      oldPath: file.oldPath,
      before:
        file.status[0] === "A"
          ? null
          : current.left === LOCAL_REVISION
            ? { kind: "disk" }
            : { kind: "revision", hash: current.left },
      after:
        file.status[0] === "D"
          ? null
          : current.right === LOCAL_REVISION
            ? { kind: "disk" }
            : { kind: "revision", hash: current.right },
    }));
    void patches.createPatch({ repo, entries }, null);
  }

  async function refresh() {
    if (!tab || !local) return;
    loading = true;
    failure = null;
    try {
      await refreshChanges();
    } catch (error) {
      failure = errorText(error) || CANNOT_LOAD;
      emptyChanges();
    } finally {
      loading = false;
    }
  }

  // `GetVersionActionProvider`: the left revision of each chosen file at the path it had there,
  // then the list is read again.
  async function get() {
    const current = tab;
    const files = selectedFiles();
    if (!current || !local || getting || files.length === 0) return;
    const items: GetItem[] = files.map((file) => ({
      path: file.path,
      revision: current.left,
      oldPath: file.oldPath,
    }));
    getting = true;
    try {
      await getRevision(items, { expectedHead: current.head });
      // What was written and what was left, after a failure too: the list is read again.
      await refresh();
    } finally {
      getting = false;
    }
  }

  // `GitLocalCommitCompareInfo.copyChangesFromBranch`: the branch's side of each change over the
  // working tree's (a missing one deleted, a rename put back at the branch's path), then the list
  // is read again (`reloadTotalDiff`). Task 5's Get writes it, after its confirmation.
  async function getFromBranch() {
    const current = tab;
    const files = selectedFiles();
    const comparison = current?.branch;
    if (!current || !comparison || getting || files.length === 0) return;
    const localOnLeft = current.left === LOCAL_REVISION;
    const items: GetItem[] = files.map((file) => {
      const kind = file.status[0];
      const before = kind === "A" ? null : (file.oldPath ?? file.path);
      const after = kind === "D" ? null : file.path;
      const [here, there] = localOnLeft ? [before, after] : [after, before];
      const path = (here ?? there) as string;
      return { path, revision: comparison.hash, oldPath: here && there && here !== there ? there : null };
    });
    getting = true;
    try {
      await getRevision(items, {
        expectedHead: current.head,
        titles: {
          confirm: getFromBranchTitle(comparison.branch),
          progress: LOADING_FROM_BRANCH,
          error: CANNOT_COPY,
        },
      });
      loading = true;
      await reloadBranchChanges().catch((error) => {
        failure = errorText(error) || CANNOT_LOAD;
        emptyChanges();
      });
    } finally {
      loading = false;
      getting = false;
    }
  }

  // `CompareWithLocalDialog.MyChangesBrowser.createPopupMenuActions`, without `Show Diff in a New
  // Tab or Window` (decision 8); `Get` only exists with a local side. Show Diff with Working Tree:
  // `ChangesBrowserBase`'s Show Diff, then Get from Branch (`CompareBranchesDiffPanel`).
  function popupItems(): MenuItem[] {
    const none = selectedFiles().length === 0;
    const diff: MenuItem = { label: "Show Diff", shortcut: "⌘D", disabled: none, run: showDiff };
    if (branch) {
      return [
        diff,
        {
          label: GET_FROM_BRANCH,
          title: getFromBranchText(branch.branch),
          disabled: none || getting,
          run: () => void getFromBranch(),
        },
      ];
    }
    return [
      diff,
      { label: "Create Patch...", disabled: none, run: createPatch },
      ...(local ? [{ label: "Get", disabled: none || getting, run: () => void get() }] : []),
    ];
  }

  function groupByItems(): MenuItem[] {
    return [
      { header: "Group By" },
      {
        label: "Directory",
        shortcut: "⌃P",
        checked: byDirectory,
        run: () => (byDirectory = !byDirectory),
      },
    ];
  }

  function openGroupBy(event: MouseEvent) {
    const bounds = (event.currentTarget as HTMLElement).getBoundingClientRect();
    menu = { x: bounds.left, y: bounds.bottom + 2, label: "Group By", items: groupByItems() };
  }

  // The window's own shortcuts: ⌘R and ⌘D act on this tab and go no further.
  function keydown(event: KeyboardEvent) {
    const action = matchShortcut(event);
    if (action === "Diff.ShowDiff") showDiff();
    else if (action === "Vcs.Log.Refresh" && local) void refresh();
    else if (action === "ChangesView.GroupBy.Directory") byDirectory = !byDirectory;
    else return;
    event.preventDefault();
    event.stopPropagation();
  }

  const nothingChosen = $derived(selected.size === 0);
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<section class="changes" onkeydown={keydown}>
  {#if tab}
    <div class="tabs" role="tablist" aria-label="Changes">
      <span class="tab selected">
        <span role="tab" aria-selected="true" tabindex="-1" title={tab.title}>{tab.title}</span>
      </span>
    </div>
    <div class="toolbar" role="toolbar" aria-label="Changes">
      {#if local}
        <button
          type="button"
          class="action"
          title="Refresh (⌘R)"
          aria-label="Refresh"
          disabled={loading}
          onclick={() => void refresh()}
        >
          <Icon name="refresh" />
        </button>
      {/if}
      <button
        type="button"
        class="action"
        title="Show Diff (⌘D)"
        aria-label="Show Diff"
        disabled={nothingChosen}
        onclick={showDiff}
      >
        <Icon name="diff" />
      </button>
      {#if local}
        <button
          type="button"
          class="action"
          title="Get"
          aria-label="Get"
          disabled={nothingChosen || getting}
          onclick={() => void get()}
        >
          <Icon name="download" />
        </button>
      {:else if branch}
        <button
          type="button"
          class="action"
          title={getFromBranchText(branch.branch)}
          aria-label={GET_FROM_BRANCH}
          disabled={nothingChosen || getting}
          onclick={() => void getFromBranch()}
        >
          <Icon name="download" />
        </button>
      {/if}
      <span class="toolbar-separator"></span>
      <button type="button" class="action" title="Group By" aria-label="Group By" onclick={openGroupBy}>
        <Icon name="groupBy" />
      </button>
    </div>
    {#if branch}
      <!-- `CompareBranchesDiffPanel.updateLabelText` (`DvcsBundle:88-90`): bold code names, then
           the link that swaps the sides, off while the list loads. -->
      <p class="direction">
        {#if swapped}
          Difference between files in <code><b>{branch.branch}</b></code> and current working tree on
          <code><b>{branch.current}</b></code>:
        {:else}
          Difference between current working tree on <code><b>{branch.current}</b></code> and files
          in <code><b>{branch.branch}</b></code>:
        {/if}
        <button type="button" class="link" disabled={loading || getting} onclick={swapBranchSides}>
          Swap branches
        </button>
      </p>
    {/if}
    <div class="body">
      {#if failure}
        <p class="status error">{failure}</p>
      {:else if loading}
        <p class="status loading" role="status">{LOADING}</p>
      {:else if tab.files.length === 0}
        <p class="status">{NO_DIFFERENCES}</p>
      {:else}
        <ChangesTree
          bind:this={tree}
          {roots}
          included={NO_CHECKS}
          bind:collapsed
          bind:selected
          bind:lead
          ontoggle={() => {}}
          onopen={showDiff}
          onmenu={(x, y) => (menu = { x, y, label: "Changes", items: popupItems() })}
        />
      {/if}
    </div>
    {#if tab.message}
      <!-- Show All Affected Files: the commit message under the tree, as text. -->
      <div class="message" aria-label="Commit message">{tab.message}</div>
    {/if}
  {/if}
</section>

{#if menu}
  <Menu x={menu.x} y={menu.y} label={menu.label} items={menu.items} onclose={() => (menu = null)} />
{/if}

<style>
  .changes {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    background: var(--panel);
  }

  .tabs {
    display: flex;
    flex: none;
    align-items: center;
    height: 30px;
    padding: 0 8px;
    box-sizing: border-box;
    border-bottom: 1px solid var(--divider);
  }

  .tab {
    display: inline-flex;
    align-items: center;
    max-width: 100%;
    height: 100%;
    padding: 0 8px;
    overflow: hidden;
    color: var(--text);
    text-overflow: ellipsis;
    white-space: nowrap;
    border-bottom: 2px solid transparent;
  }

  .tab.selected {
    border-bottom-color: var(--focus);
  }

  .tab [role="tab"] {
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .toolbar {
    display: flex;
    flex: none;
    gap: 4px;
    align-items: center;
    height: 30px;
    padding: 0 8px;
  }

  .direction {
    flex: none;
    padding: 4px 12px;
    margin: 0;
    color: var(--text);
  }

  .direction code {
    font-family: var(--mono, monospace);
  }

  .direction .link {
    padding: 0;
    margin-left: 1em;
    font: inherit;
    color: var(--link);
    background: none;
    border: 0;
  }

  .direction .link:disabled {
    color: var(--disabled);
  }

  .body {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-height: 0;
  }

  .status {
    padding: 16px;
    margin: 0;
    color: var(--text-secondary);
    text-align: center;
  }

  .status.error {
    color: var(--text);
    white-space: pre-line;
    overflow-wrap: anywhere;
  }

  /* Normally it takes less time than a blink takes to be noticed. */
  .loading {
    opacity: 0;
    animation: appear 0s linear 300ms forwards;
  }

  @keyframes appear {
    to {
      opacity: 1;
    }
  }

  .message {
    flex: none;
    max-height: 30%;
    padding: 8px 12px;
    overflow: auto;
    color: var(--text);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    user-select: text;
    border-top: 1px solid var(--divider);
  }
</style>
