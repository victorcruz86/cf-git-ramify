<script lang="ts">
  import { tick } from "svelte";
  import { protectedBranches } from "../branches/state";
  import {
    clipboardWrite,
    logAdjacentRows,
    logRowOf,
    type ApiError,
    type JumpTarget,
    type LogRef,
    type RowOf,
  } from "../lib/api";
  import { hostingItems, OPEN_ON_GITHUB } from "../hosting/links";
  import { errorText } from "../lib/errors";
  import { matchLogShortcut, type ActionId } from "../lib/keymap";
  import Menu, { type MenuItem } from "../lib/Menu.svelte";
  import { notify } from "../lib/notifications";
  import type { LogChange } from "./changes";
  import { contextMenu, revisionsText, type VcsAction } from "./contextMenu";
  import { menuChecks, rewordFromKeyboard } from "./editing";
  import { adjacentLabel, jumpFailure, resolveTarget, tooltipRefs, type Target } from "./goto";
  import GoToRef from "./GoToRef.svelte";
  import { refColor } from "./labels";
  import type LogView from "./LogView.svelte";

  // What the user launches on the table: go to a commit, copy, the tooltip and the menus.
  interface Props {
    logView: LogView | undefined;
    viewId: number;
    refs: LogRef[];
    currentBranch: string | null;
    hashes: string[];
    filtered: boolean;
    onresetfilters: () => Promise<void>;
    onnewtab: () => Promise<void>;
    onvcs?: (action: VcsAction) => void | Promise<void>;
    // The Paths filter, which Compare Versions and Compare with Local are limited to.
    paths?: string[];
    // `Create Patch...` of the chosen commits' changes.
    oncreatepatch?: (changes: LogChange[]) => void;
    // What the changes panel has loaded for the chosen commits (`VcsDataKeys.CHANGES` is
    // `myChangesBrowser.getDirectChanges()`, `MainFrame.java:267-268`): the menu never waits for
    // git, and while they load Create Patch... is off, as `VcsLogCreatePatchActionProvider.update`
    // leaves it with no changes.
    changes?: LogChange[];
  }

  let {
    logView,
    viewId,
    refs,
    currentBranch,
    hashes,
    filtered,
    onresetfilters,
    onnewtab,
    onvcs,
    paths = [],
    oncreatepatch,
    changes = [],
  }: Props = $props();

  const COMMIT_MENU = "Commit";
  const SELECT_PARENT = "Select Parent to Navigate";
  const SELECT_CHILD = "Select Child to Navigate";

  let goToAt = $state.raw<{ x: number; y: number } | null>(null);
  let menu = $state.raw<{
    x: number;
    y: number;
    label: string;
    items: MenuItem[];
    numbered?: boolean;
  } | null>(null);
  let tooltip = $state.raw<{ x: number; y: number; refs: LogRef[] } | null>(null);
  const shownRefs = $derived(tooltip ? tooltipRefs(tooltip.refs) : null);

  function showError(failure: unknown) {
    notify({ kind: "error", title: errorText(failure as ApiError) });
  }

  // What `VcsLogUiImpl.handleCommitNotFound` offers if the filters are to blame.
  function warn(target: Target, found: RowOf) {
    const hidden = found.hash !== null && filtered;
    notify({
      kind: "warning",
      title: jumpFailure(target, found),
      actions: hidden
        ? [
            { label: "View and reset filters", run: () => jumpAfter(onresetfilters, target) },
            { label: "View in new tab", run: () => jumpAfter(onnewtab, target) },
          ]
        : undefined,
    });
  }

  async function jumpAfter(change: () => Promise<void>, target: Target) {
    await change();
    await tick();
    await jump(target);
  }

  async function jump(target: Target) {
    if (target.kind === "invalid") {
      warn(target, { row: null, hash: null });
      return;
    }
    try {
      const found = await logRowOf(viewId, target as JumpTarget);
      if (found.row === null) warn(target, found);
      else logView?.jumpToRow(found.row, true);
    } catch (failure) {
      showError(failure);
    }
  }

  // `jumpToBranch`, from the branches panel.
  export const goToBranch = (name: string) => jump({ kind: "ref", value: name });

  export function goToRef() {
    if (logView) goToAt = logView.center();
  }

  async function submit(text: string, chosen: string | null) {
    const wanted = text.trim();
    goToAt = null;
    if (!wanted) return;
    // When the popup closes it returns the focus, and jumping takes it to the table.
    await tick();
    await jump(chosen === null ? resolveTarget(wanted, refs) : { kind: "ref", value: chosen });
  }

  function adjacent(row: number, parent: boolean): Promise<number[]> {
    return logAdjacentRows(viewId, row, parent);
  }

  // `GoToParentOrChildAction`.
  async function goToAdjacent(parent: boolean) {
    const view = logView;
    const row = view?.selectedRow() ?? null;
    if (!view || row === null) return;
    try {
      const rows = await adjacent(row, parent);
      if (rows.length === 1) view.jumpToRow(rows[0], true);
      else if (rows.length > 1) await chooseAdjacent(view, row, rows, parent);
    } catch (failure) {
      showError(failure);
    }
  }

  async function chooseAdjacent(view: LogView, row: number, rows: number[], parent: boolean) {
    const label = parent ? SELECT_PARENT : SELECT_CHILD;
    const commits = await Promise.all(rows.map((target) => view.commitAt(target)));
    const items = rows.flatMap((target, index) => {
      const commit = commits[index];
      return commit
        ? [{ label: adjacentLabel(commit), run: () => view.jumpToRow(target, true) }]
        : [];
    });
    menu = { ...view.belowRow(row), label, numbered: true, items: [{ header: label }, ...items] };
  }

  function copy(text: string) {
    clipboardWrite(text).catch(showError);
  }

  export async function showContextMenu(x: number, y: number) {
    const view = logView;
    if (!view) return;
    const row = view.selectedRow();
    try {
      const [commits, parents, children] = await Promise.all([
        view.selectedCommits(),
        row === null ? [] : adjacent(row, true),
        row === null ? [] : adjacent(row, false),
      ]);
      // Without knowing it nothing is turned off: the core checks it again on click.
      const editing =
        commits.length > 0 && onvcs
          ? await menuChecks(commits.map(({ hash }) => hash)).catch(() => undefined)
          : undefined;
      const loaded = changes;
      const hosting =
        commits.length > 0
          ? await hostingItems({ kind: "commit", hash: commits[0].hash }, "open", OPEN_ON_GITHUB)
          : [];
      const items = contextMenu(
        { commits, parents, children },
        { currentBranch },
        {
          copy,
          goToChild: () => goToAdjacent(false),
          goToParent: () => goToAdjacent(true),
          vcs: onvcs,
          protectedBranches: $protectedBranches,
          editing,
          paths,
          hosting,
          createPatch:
            oncreatepatch && loaded.length > 0 ? () => oncreatepatch(loaded) : undefined,
        },
      );
      if (items.length > 0) menu = { x, y, items, label: COMMIT_MENU };
    } catch (failure) {
      showError(failure);
    }
  }

  // `ShowCommitTooltipAction`: the one shown when hovering the labels, with their references.
  async function showTooltip() {
    const view = logView;
    const row = view?.selectedRow() ?? null;
    if (!view || row === null || !view.isVisible(row)) return;
    const commit = await view.commitAt(row);
    if (commit && commit.refs.length > 0) tooltip = { ...view.tooltipPoint(row), refs: commit.refs };
  }

  // `Git.Reword.Commit` with F2 or ⇧F6, like the menu entry.
  async function rewordSelected() {
    const view = logView;
    if (!view) return;
    try {
      await rewordFromKeyboard(await view.selectedCommits(), menuChecks, onvcs);
    } catch (failure) {
      showError(failure);
    }
  }

  export function keydown(event: KeyboardEvent, action: ActionId | null) {
    const onTable = event.target === logView?.scrollElement();
    if (onTable && matchLogShortcut(event) === "Git.Reword.Commit") void rewordSelected();
    else if (action === "Vcs.Log.GoToRef") goToRef();
    else if (action === "Vcs.Log.ShowTooltip") showTooltip();
    else if (action === "Vcs.CopyRevisionNumberAction" && hashes.length > 0) {
      copy(revisionsText(hashes));
    } else if (action === "Vcs.Log.GoToParent" && onTable) goToAdjacent(true);
    else if (action === "Vcs.Log.GoToChild" && onTable) goToAdjacent(false);
    else return;
    event.preventDefault();
  }

  const hideTooltip = () => (tooltip = null);
</script>

<svelte:window
  onkeydowncapture={hideTooltip}
  onpointerdowncapture={hideTooltip}
  onwheelcapture={hideTooltip}
/>

{#if goToAt}
  <GoToRef {...goToAt} {refs} onsubmit={submit} onclose={() => (goToAt = null)} />
{/if}

{#if menu}
  <Menu {...menu} onclose={() => (menu = null)} />
{/if}

{#if tooltip && shownRefs}
  <div class="tooltip" role="tooltip" style:left="{tooltip.x}px" style:top="{tooltip.y}px">
    {#each shownRefs.rows as { text, kind, first }}
      <div class="ref" class:first style:--ref={refColor(kind)}>{text}</div>
    {/each}
    {#if shownRefs.rest}
      <div class="rest">{shownRefs.rest}</div>
    {/if}
  </div>
{/if}

<style>
  .tooltip {
    position: fixed;
    z-index: 20;
    padding: 6px 10px;
    color: var(--text);
    transform: translateX(-50%);
    background: var(--popup-background);
    border: 1px solid var(--popup-border);
    border-radius: 6px;
    box-shadow: 0 6px 24px rgb(0 0 0 / 40%);
  }

  .ref {
    position: relative;
    padding-left: 14px;
    line-height: 20px;
    white-space: nowrap;
  }

  .ref.first::before {
    position: absolute;
    top: 6px;
    left: 0;
    width: 8px;
    height: 8px;
    content: "";
    background: var(--ref);
    border-radius: 2px;
  }

  .rest {
    padding-left: 14px;
    line-height: 20px;
    color: var(--disabled);
  }
</style>
