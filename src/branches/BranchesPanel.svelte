<script lang="ts">
  import { onMount, tick } from "svelte";
  import {
    branchIncomingOutgoing,
    branchList,
    onBranchSyncChanged,
    onRepoChanged,
    repoState,
    type BranchList,
    type BranchSync,
  } from "../lib/api";
  import Icon from "../lib/Icon.svelte";
  import { fetching } from "../update/flow";
  import { matchBranchPanelShortcut, matchShortcut } from "../lib/keymap";
  import Menu, { type MenuItem } from "../lib/Menu.svelte";
  import { isProtected, type VcsAction } from "../log/contextMenu";
  import {
    branchActions,
    branchesOf,
    buildTree,
    highlighted,
    refOf,
    syncTooltip,
    visibleRows,
    type BranchNode,
    type GroupNode,
    type TreeNode,
  } from "./model";
  import { compareBranches, showBranchesDiff } from "../compare/actions";
  import { errorText } from "../lib/errors";
  import { copyBranchName } from "./copyName";
  import { ramifyOptions } from "../lib/ramifyOptions";
  import { notify } from "../lib/notifications";
  import {
    branchOptions,
    favorites,
    protectedBranches,
    refsTracker,
    setBranchOptions,
    toggleFavorite,
  } from "./state";

  // The log's branches panel: `BranchesDashboardUi`, without `Show My Branches`.
  interface Props {
    active: boolean;
    onfilter: (branches: string[]) => void;
    onnavigate: (branch: string) => void;
    onhide: () => void;
    // What it returns is awaited when several branches go one after the other.
    onvcs?: (action: VcsAction) => void | Promise<void>;
    onfetch?: () => Promise<unknown>;
  }

  let { active, onfilter, onnavigate, onhide, onvcs, onfetch }: Props = $props();

  const HEAD_ID = "head";
  const HEAD_LABEL = "HEAD (Current Branch)";
  const HEAD_FILTER = "HEAD";
  // `initDefaultTreeExpandState`: open everything, unless it is too much.
  const MANY_NODES = 30000;

  let list = $state.raw<BranchList | null>(null);
  // HEAD's hash, empty before the first commit (`Repository.isFresh`), which turns the
  // comparisons off.
  let head = $state<string | null>(null);
  let sync = $state.raw<Record<string, BranchSync>>({});
  let text = $state("");
  let selected = $state.raw<string[]>([]);
  let expanded = $state.raw(new Set<string>());
  let firstRead = true;
  let menu = $state.raw<{ x: number; y: number; label: string; items: MenuItem[] } | null>(null);
  let readToken = 0;
  let syncToken = 0;
  const refs = refsTracker();

  const tree = $derived(
    list
      ? buildTree(list, text, {
          favorites: $favorites,
          grouping: $branchOptions.grouping,
          recent: false,
          panel: true,
        })
      : [],
  );
  const searching = $derived(text.trim() !== "");
  const allGroups = (nodes: TreeNode[]): string[] =>
    nodes.flatMap((node) => (node.kind === "branch" ? [] : [node.id, ...allGroups(node.children)]));
  const rows = $derived(
    visibleRows(tree, searching ? new Set(allGroups(tree)) : expanded),
  );
  const branchRows = $derived(
    rows.flatMap(({ node }) => (node.kind === "branch" ? [node] : [])),
  );
  const selectedBranches = $derived(branchRows.filter(({ id }) => selected.includes(id)));
  // `getSelectedBranchFilters`: in the order they are seen.
  const selectedFilters = $derived([
    ...(selected.includes(HEAD_ID) ? [HEAD_FILTER] : []),
    ...selectedBranches.map(({ name }) => name),
  ]);
  const single = $derived(selectedBranches.length === 1 ? selectedBranches[0] : null);
  const showHead = $derived(!searching && tree.length > 0);

  async function readSync() {
    const token = ++syncToken;
    try {
      const state = await branchIncomingOutgoing();
      if (token === syncToken) sync = state;
    } catch {
      // Without this only the arrows are missing: the next signal reads again.
    }
  }

  async function read() {
    const token = ++readToken;
    readSync();
    try {
      await refs.note();
      const [branches, state] = await Promise.all([branchList(), repoState().catch(() => null)]);
      if (token !== readToken) return;
      list = branches;
      head = state?.head ?? null;
      if (firstRead) {
        firstRead = false;
        const many = branches.local.length + branches.remote.length > MANY_NODES;
        expanded = new Set(many ? [] : allGroups(tree));
      }
    } catch {
      // If git fails now, the next signal reads again.
    }
  }

  // `treeSelectionListener`.
  async function selectionChanged() {
    await tick();
    if ($branchOptions.filterOnSelect) onfilter(selectedFilters);
    else if ($branchOptions.navigateOnSelect) {
      if (selectedFilters.length === 1) onnavigate(selectedFilters[0]);
    }
  }

  function select(event: MouseEvent, id: string) {
    if (event.metaKey) {
      selected = selected.includes(id)
        ? selected.filter((other) => other !== id)
        : [...selected, id];
    } else {
      selected = [id];
    }
    selectionChanged();
  }

  function toggleGroup(node: GroupNode) {
    const next = new Set(expanded);
    if (!next.delete(node.id)) next.add(node.id);
    expanded = next;
  }

  // `BranchesTreeComponent`: it collapses with a double click or with the arrow. A single click only selects.
  function clickChevron(event: MouseEvent, node: GroupNode) {
    event.stopPropagation();
    toggleGroup(node);
  }

  // `UpdateBranchFilterInLogAction`: with no branches chosen it is disabled.
  function updateFilter() {
    if (selectedFilters.length > 0) onfilter(selectedFilters);
  }

  // A single row chosen with the arrows, like a click.
  function step(delta: number) {
    const ids = [...(showHead ? [HEAD_ID] : []), ...rows.map(({ node }) => node.id)];
    if (ids.length === 0) return;
    const position = ids.indexOf(selected[selected.length - 1]);
    const next = position < 0 ? 0 : Math.max(0, Math.min(ids.length - 1, position + delta));
    selected = [ids[next]];
    selectionChanged();
  }

  const selectedRow = () => rows.find(({ node }) => node.id === selected[selected.length - 1]);

  function fold(open: boolean) {
    const row = selectedRow();
    if (row?.node.kind === "branch" || !row) {
      if (!open) step(-1);
    } else if (expanded.has(row.node.id) !== open) toggleGroup(row.node);
    else step(open ? 1 : -1);
  }

  function keydown(event: KeyboardEvent) {
    if (event.key === "Enter") updateFilter();
    else if (event.key === "ArrowDown") step(1);
    else if (event.key === "ArrowUp") step(-1);
    else if (event.key === "ArrowRight") fold(true);
    else if (event.key === "ArrowLeft") fold(false);
    else return;
    event.preventDefault();
  }

  // Same as the bar: `RenameLocalBranch.update` only accepts a local branch.
  function shortcut(event: KeyboardEvent) {
    const action = matchBranchPanelShortcut(event);
    if (action === "Git.Rename.Local.Branch") {
      if (single?.section !== "remote") run("rename");
    } else if (action === "Git.New.Branch.In.Log") run("newBranch");
    else if (action === "Git.Log.Branches.Navigate.Log.To.Selected.Branch") navigate();
    else if (matchShortcut(event) === "Diff.ShowDiff") compareWithCurrent();
    else return false;
    event.preventDefault();
    event.stopPropagation();
    return true;
  }

  // What is typed here is not for the log table; global shortcuts go on their way.
  function contain(event: KeyboardEvent) {
    if (shortcut(event)) return;
    if (!event.metaKey && !event.ctrlKey) event.stopPropagation();
  }

  // `guessRepo.currentBranch`: the current branch whatever is folded or filtered in the tree.
  function currentBranchNode(): BranchNode | null {
    const name = list?.current ?? null;
    if (name === null) return null;
    const found = branchesOf(tree).find((node) => node.current && node.section === "local");
    if (found) return found;
    const tracked = list?.local.find((local) => local.name === name)?.tracked ?? null;
    return {
      kind: "branch",
      id: `local:${name}`,
      section: "local",
      name,
      label: name,
      current: true,
      favorite: false,
      tracked,
      match: null,
    };
  }

  // `branch` is `null` on the HEAD row.
  function contextmenu(event: MouseEvent, branch: BranchNode | null) {
    event.preventDefault();
    // A right click inside the selection keeps it (`BranchesTreeComponent`); outside it chooses
    // only that row.
    const id = branch?.id ?? HEAD_ID;
    if (!selected.includes(id)) selected = [id];
    // `BranchActionsBuilder.build` (`BranchesDashboardActions.kt:128-170`): the branches chosen,
    // HEAD not counted; several give their own menu; HEAD chosen, alone or with one branch, gives
    // the current branch's menu (`CurrentBranchActions`, `:165-167`).
    if (selectedBranches.length > 1) {
      menu = {
        x: event.clientX,
        y: event.clientY,
        label: "Branches",
        items: severalItems(selectedBranches),
      };
      return;
    }
    const shown = selected.includes(HEAD_ID) ? currentBranchNode() : branch;
    if (!shown) return;
    menu = {
      x: event.clientX,
      y: event.clientY,
      label: shown.name,
      items: branchActions(shown, {
        current: list?.current ?? null,
        remotes: (list?.remote.length ?? 0) > 0,
        onvcs,
        protectedBranches: $protectedBranches,
        head,
        oncopy: $ramifyOptions.copyBranchName ? copyBranchName : undefined,
      }),
    };
  }

  function anchored(event: MouseEvent, label: string, items: MenuItem[]) {
    const bounds = (event.currentTarget as HTMLElement).getBoundingClientRect();
    menu = { x: bounds.right, y: bounds.top, label, items };
  }

  // `ChangeBranchFilterAction` and `NavigateLogToBranchAction`: turning one on turns the other off.
  function paneSettings(event: MouseEvent) {
    const { filterOnSelect, navigateOnSelect } = $branchOptions;
    anchored(event, "Branches Pane Settings", [
      { header: "On Single Click" },
      {
        label: "Update Branch Filter",
        checked: filterOnSelect,
        run: () => setBranchOptions({ filterOnSelect: !filterOnSelect, navigateOnSelect: false }),
      },
      {
        label: "Navigate Log to Branch Head",
        checked: navigateOnSelect && !filterOnSelect,
        run: () => setBranchOptions({ navigateOnSelect: !navigateOnSelect, filterOnSelect: false }),
      },
    ]);
  }

  function groupBy(event: MouseEvent) {
    const { grouping } = $branchOptions;
    anchored(event, "Group By", [
      { header: "Group By" },
      {
        label: "Directory",
        checked: grouping,
        run: () => setBranchOptions({ grouping: !grouping }),
      },
    ]);
  }

  function run(operation: "newBranch" | "delete" | "update" | "rename") {
    if (single) onvcs?.({ kind: "ref", operation, ref: refOf(single) });
  }

  const failed = (failure: unknown) => notify({ kind: "error", title: errorText(failure) });

  // `ShowBranchDiffAction` (`Compare with Current`, `B:604`, the toolbar's diff button and ⌘D):
  // each chosen branch that is not the current one against the current one.
  const others = $derived(selectedBranches.filter((branch) => !branch.current));

  function compareWithCurrent() {
    const start = list?.current ?? "HEAD";
    for (const branch of others) compareBranches(branch.name, start).catch(failed);
  }

  // `DeleteBranchAction.delete` (`BranchesDashboardActions.kt:303-321`): the local branches first,
  // each as its own deletion, then all the remote ones with one question
  // (`GitDeleteRemoteBranchOperation`).
  async function deleteSeveral(branches: BranchNode[]) {
    const locals = branches.filter((branch) => branch.section !== "remote");
    const remotes = branches.filter((branch) => branch.section === "remote");
    for (const branch of locals) await onvcs?.({ kind: "ref", operation: "delete", ref: refOf(branch) });
    if (remotes.length > 0) {
      await onvcs?.({ kind: "deleteRemotes", names: remotes.map((branch) => branch.name) });
    }
  }

  // `MultipleLocalBranchActions` (`BranchesDashboardActions.kt:68-71`), the menu of several chosen
  // branches: `ShowArbitraryBranchesDiffAction` (`Compare Branches`, `B:605`) and
  // `ShowArbitraryBranchesFileDiffAction` (`Show Files Diff`, `B:607`), only with exactly two
  // (`BranchesPairActionBase`, `:350-371`), then `UpdateSelectedBranchAction` (only with remotes, on
  // when one of them tracks a branch, `:224-262`) and `DeleteBranchAction` (`Delete branches`, off
  // with the current branch or a protected remote one, `:264-296`).
  function severalItems(branches: BranchNode[]): MenuItem[] {
    const [one, two] = branches;
    const pair: MenuItem[] =
      branches.length === 2
        ? [
            {
              label: "Compare Branches",
              title: "Compare selected branches with each other",
              run: () => void compareBranches(one.name, two.name).catch(failed),
            },
            {
              label: "Show Files Diff",
              title: "Show diff between the selected branch heads",
              run: () => void showBranchesDiff(one.name, two.name).catch(failed),
            },
          ]
        : [];
    const tracking = branches.filter((branch) => branch.section !== "remote" && branch.tracked !== null);
    const update: MenuItem[] =
      (list?.remote.length ?? 0) > 0
        ? [
            {
              label: "Update Selected",
              disabled: !onvcs || tracking.length === 0 || $fetching,
              run: () =>
                void onvcs?.({ kind: "updateBranches", names: tracking.map((branch) => branch.name) }),
            },
          ]
        : [];
    const undeletable = branches.some(
      (branch) =>
        branch.current ||
        (branch.section === "remote" && isProtected(refOf(branch), $protectedBranches)),
    );
    return [
      ...pair,
      ...update,
      {
        label: "Delete branches",
        disabled: !onvcs || undeletable,
        run: () => void deleteSeveral(branches),
      },
    ];
  }

  function navigate() {
    if (selectedFilters.length === 1) onnavigate(selectedFilters[0]);
  }

  async function fetchRemotes() {
    // The shared flag is raised by the fetch itself (`src/update/flow.ts`).
    if (!onfetch || $fetching) return;
    await onfetch();
  }

  function toggleFavorites() {
    for (const branch of selectedBranches) {
      toggleFavorite(branch.name, refOf(branch).kind as "local" | "remote").catch(() => {});
    }
  }

  $effect(() => {
    if (active) read();
  });

  onMount(() => {
    const stops = [
      onRepoChanged(async () => {
        if (active && (await refs.changed())) read();
      }),
      onBranchSyncChanged(() => {
        if (active) readSync();
      }),
    ];
    return () => stops.forEach((stop) => stop.then((unlisten) => unlisten()));
  });

  const iconOf = (branch: BranchNode) => {
    if (branch.current) return branch.favorite ? "currentBranchFavorite" : "currentBranch";
    return branch.favorite ? "favorite" : "branchLabel";
  };

  const hasSync = (branch: BranchNode) => branch.section !== "remote" && sync[branch.name];
</script>

{#snippet toolbarButton(label: string, icon: string, onclick: (event: MouseEvent) => void, disabled = false)}
  <button type="button" class="action" aria-label={label} title={label} {disabled} {onclick}>
    <Icon name={icon} />
  </button>
{/snippet}

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="panel" onkeydown={contain}>
  <div class="toolbar" role="toolbar" aria-label="Branches" aria-orientation="vertical">
    {@render toolbarButton("Hide Git Branches", "left", onhide)}
    <span class="gap"></span>
    {@render toolbarButton("New Branch", "add", () => run("newBranch"), !single || !onvcs)}
    {@render toolbarButton(
      "Update Selected",
      "down",
      () => run("update"),
      !single || !onvcs || single.section === "remote" || single.tracked === null,
    )}
    {@render toolbarButton(
      "Delete branch",
      "delete",
      () => run("delete"),
      !single ||
        !onvcs ||
        single.current ||
        (single.section === "remote" && isProtected(refOf(single), $protectedBranches)),
    )}
    {@render toolbarButton("Compare with Current", "diff", compareWithCurrent, others.length === 0)}
    {@render toolbarButton("Fetch All Remotes", "fetch", fetchRemotes, !onfetch || $fetching)}
    {@render toolbarButton(
      "Mark/Unmark As Favorite",
      "favorite",
      toggleFavorites,
      selectedBranches.length === 0,
    )}
    {@render toolbarButton(
      "Navigate Log to Selected Branch Head",
      "locate",
      navigate,
      selectedFilters.length !== 1,
    )}
    <span class="gap"></span>
    {@render toolbarButton("Branches Pane Settings", "settings", paneSettings)}
    {@render toolbarButton("Group By", "groupBy", groupBy)}
    {@render toolbarButton("Expand All", "expandAll", () => (expanded = new Set(allGroups(tree))))}
    {@render toolbarButton("Collapse All", "collapseAll", () => (expanded = new Set()))}
  </div>

  <div class="content">
    <input
      class="search"
      aria-label="Search"
      placeholder="Search"
      spellcheck="false"
      bind:value={text}
    />
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <div
      class="tree"
      role="tree"
      aria-label="Branches"
      tabindex="0"
      onkeydown={keydown}
    >
      {#if showHead}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <div
          class="row"
          class:selected={selected.includes(HEAD_ID)}
          role="treeitem"
          tabindex="-1"
          aria-label={HEAD_LABEL}
          aria-selected={selected.includes(HEAD_ID)}
          style:padding-left="24px"
          onclick={(event) => select(event, HEAD_ID)}
          ondblclick={updateFilter}
          oncontextmenu={(event) => contextmenu(event, null)}
        >
          <span class="label">{HEAD_LABEL}</span>
        </div>
      {/if}
      {#each rows as { node, depth } (node.id)}
        {#if node.kind === "branch"}
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <div
            class="row"
            class:selected={selected.includes(node.id)}
            role="treeitem"
            tabindex="-1"
            aria-label={node.label}
            aria-selected={selected.includes(node.id)}
            title={hasSync(node)
              ? syncTooltip(sync[node.name].incoming, sync[node.name].outgoing)
              : undefined}
            style:padding-left="{24 + depth * 18}px"
            onclick={(event) => select(event, node.id)}
            ondblclick={updateFilter}
            oncontextmenu={(event) => contextmenu(event, node)}
          >
            <Icon name={iconOf(node)} />
            <span class="label">
              {#each highlighted(node.label, text) as part}
                {#if part.hit}<span class="hit">{part.text}</span>{:else}{part.text}{/if}
              {/each}
            </span>
            {#if hasSync(node)}
              <span class="sync">
                {#if sync[node.name].incoming}<Icon name="incoming" />{/if}
                {#if sync[node.name].outgoing}<Icon name="outgoing" />{/if}
              </span>
            {/if}
          </div>
        {:else}
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <div
            class="row"
            class:selected={selected.includes(node.id)}
            role="treeitem"
            tabindex="-1"
            aria-label={node.label}
            aria-selected={selected.includes(node.id)}
            aria-expanded={searching || expanded.has(node.id)}
            style:padding-left="{6 + depth * 18}px"
            onclick={(event) => select(event, node.id)}
            ondblclick={() => toggleGroup(node)}
          >
            <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
            <span
              class="chevron"
              onclick={(event) => clickChevron(event, node)}
              ondblclick={(event) => event.stopPropagation()}
            >
              <Icon
                name={searching || expanded.has(node.id) ? "chevronDown" : "chevronRight"}
                size={14}
              />
            </span>
            {#if node.kind === "folder"}<Icon name="folder" />{/if}
            <span class="label">{node.label}</span>
          </div>
        {/if}
      {/each}
    </div>
  </div>
</div>

{#if menu}
  <Menu {...menu} onclose={() => (menu = null)} />
{/if}

<style>
  .panel {
    display: flex;
    width: 100%;
    height: 100%;
    min-width: 0;
    background: var(--panel);
  }

  .toolbar {
    display: flex;
    flex: none;
    flex-direction: column;
    gap: 2px;
    align-items: center;
    width: 30px;
    padding-top: 4px;
    border-right: 1px solid var(--divider);
  }

  .gap {
    flex: none;
    width: 16px;
    height: 1px;
    margin: 3px 0;
    background: var(--divider);
  }

  .content {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
  }

  .search {
    flex: none;
    height: 30px;
    box-sizing: border-box;
    padding: 0 10px;
    font: inherit;
    color: var(--text);
    background: none;
    border: 0;
    border-bottom: 1px solid var(--divider);
    outline: none;
  }

  .search::placeholder {
    color: var(--muted);
  }

  .tree {
    flex: 1;
    min-height: 0;
    padding: 2px 0;
    overflow: auto;
    outline: none;
  }

  .row {
    display: flex;
    gap: 6px;
    align-items: center;
    box-sizing: border-box;
    height: 24px;
    padding-right: 8px;
    color: var(--text);
    white-space: nowrap;
    outline: none;
    content-visibility: auto;
    contain-intrinsic-height: 24px;
  }

  .row.selected {
    background: var(--selection-inactive);
  }

  .tree:focus-within .row.selected {
    background: var(--selection);
  }

  .chevron {
    display: inline-flex;
    flex: none;
    width: 14px;
    color: var(--muted);
  }

  .label {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .hit {
    color: var(--accent-text);
    background: var(--accent);
    border-radius: 2px;
  }

  .sync {
    display: inline-flex;
    flex: none;
    margin-left: 6px;
  }
</style>
