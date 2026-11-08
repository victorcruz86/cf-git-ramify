<script lang="ts">
  import type { StashEntry } from "../lib/api";
  import type { MenuItem } from "../lib/menu";
  import type { VcsAction } from "../log/contextMenu";
  import { refColor } from "../log/labels";
  import { shortDate, shortTime } from "../log/draw";
  import SavedPatchesView from "./SavedPatchesView.svelte";
  import type { SavedChange } from "./savedPatches";
  import {
    stashChangeList,
    stashChangesBackend,
    stashComparable,
    stashDiff,
    stashDiffTitle,
    stashPathsSelection,
    type StashChangesApi,
  } from "./stashChanges";

  // The `Stash` tab (`GitStashProvider` in `SavedPatchesUi`). Only exists while there are stashes; the
  // list comes from whoever reads `git stash list` again when the repository changes. Apply and Pop
  // go through the flow of `Unstash` (`GitStashOperations.unstash`): the same commands, locks and
  // recoverable copy.
  interface Props {
    stashes: StashEntry[];
    // `git stash list` failed: the tab shows the error as its node.
    error?: string | null;
    currentBranch: string | null;
    onunstash: (stash: StashEntry, pop: boolean) => unknown;
    onunstashas: (stash: StashEntry) => unknown;
    ondrop: (stash: StashEntry) => unknown;
    // `Cherry-Pick Selected Changes`, which the log's `apply_paths` runs. Without it, disabled.
    onvcs?: (action: VcsAction) => unknown;
    api?: StashChangesApi;
  }

  let {
    stashes,
    error = null,
    currentBranch,
    onunstash,
    onunstashas,
    ondrop,
    onvcs,
    api = stashChangesBackend,
  }: Props = $props();

  let view = $state<SavedPatchesView<StashEntry>>();

  export function focus() {
    view?.focus();
  }

  const label = (stash: StashEntry) => `stash@{${stash.index}}`;

  // `saved.patch.created.on.date.at.time.tooltip` with `DateFormatUtil.formatDate` and `formatTime`.
  const tooltip = (stash: StashEntry) =>
    `${label(stash)} created on ${shortDate(stash.time)} at ${shortTime(stash.time)}`;

  // `Git.Stash.Operations.ContextMenu`: `Pop`, `Apply`, `Unstash…` and `Drop` (`GitBundle`
  // `action.Git.Stash.*.text`); Drop has the shortcuts of `$Delete`.
  const operations = (stash: StashEntry): MenuItem[] => [
    { label: "Pop", run: () => void onunstash(stash, true) },
    { label: "Apply", run: () => void onunstash(stash, false) },
    { label: "Unstash…", run: () => void onunstashas(stash) },
    { label: "Drop", shortcut: "⌫", run: () => void ondrop(stash) },
  ];

  // `Git.Stash.ChangesBrowser.ContextMenu` after `Compare Before with Local`: `Edit Source` and
  // `Open Repository Version` are not in rama; `Vcs.ApplySelectedChanges` is.
  const changeOperations = (stash: StashEntry, change: SavedChange): MenuItem[] => [
    {
      label: "Cherry-Pick Selected Changes",
      disabled: !onvcs,
      run: () =>
        void onvcs?.({
          kind: "applyPaths",
          reverse: false,
          selection: [stashPathsSelection(stash, change)],
          hashes: [change.source],
        }),
    },
  ];
</script>

<SavedPatchesView
  bind:this={view}
  items={stashes}
  {error}
  label="Stashes"
  menuLabel="Stash"
  keyOf={(stash) => stash.sha}
  textOf={(stash) => stash.subject}
  tooltipOf={tooltip}
  loadChanges={async (stash) => stashChangeList(stash, await api.changes(stash.sha))}
  loadDiff={(stash, change, mode) => stashDiff(api, stash, change, mode)}
  diffTitle={stashDiffTitle}
  {operations}
  compareBefore
  {changeOperations}
  comparable={(_stash, change, mode) => stashComparable(api, change, mode)}
  onapply={(stash) => void onunstash(stash, false)}
  onpop={(stash) => void onunstash(stash, true)}
  ondelete={(stash) => void ondrop(stash)}
>
  {#snippet badge(stash: StashEntry)}
    <!-- `GitStashBranchComponent`: the current branch takes the color of HEAD. -->
    {#if stash.branch}
      {@const current = stash.branch === currentBranch}
      <span class="branch" class:current>
        <span class="flag" style:background={refColor(current ? "head" : "local")}></span>
        {stash.branch}
      </span>
    {/if}
  {/snippet}
</SavedPatchesView>

<style>
  .branch {
    display: inline-flex;
    flex: none;
    gap: 4px;
    align-items: center;
    color: var(--text-secondary);
  }

  .flag {
    width: 6px;
    height: 6px;
    border-radius: 1px;
  }
</style>
