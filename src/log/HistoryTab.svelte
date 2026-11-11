<script lang="ts">
  import { untrack } from "svelte";
  import {
    clipboardWrite,
    logHistoryChange,
    logHistoryEntries,
    type HistoryChange,
    type LinkTarget,
    type LogRef,
    type LogSummary,
    type RevisionPair,
  } from "../lib/api";
  import { CANNOT_OPEN, hostingItems, OPEN_ON_GITHUB } from "../hosting/links";
  import { errorText } from "../lib/errors";
  import Icon from "../lib/Icon.svelte";
  import type { ActionId } from "../lib/keymap";
  import Menu, { type MenuItem } from "../lib/Menu.svelte";
  import { notify } from "../lib/notifications";
  import { getRevision } from "../changes/get";
  import { compareWithLocal, showAllAffected, showHistoryDiff } from "../compare/actions";
  import RevisionsPreview from "../diff/RevisionsPreview.svelte";
  import { openInViewer } from "../viewer/window";
  import type { LogChange } from "./changes";
  import { revisionsText, type VcsAction } from "./contextMenu";
  import DetailsPanel from "./DetailsPanel.svelte";
  import FilterBar from "./FilterBar.svelte";
  import { branchValues, type FilterState } from "./filters";
  import { HISTORY_EMPTY, HISTORY_ERROR, REFRESH, RESET_FILTERS, historyPair } from "./history";
  import { historyMenu, showDiffLabel, type HistoryCommit, type HistoryHandlers } from "./historyMenu";
  import LogView from "./LogView.svelte";
  import { toggleColumn, type LogColumn, type LogOptions, type Splits } from "./options";
  import { SIGNATURE_COLUMN } from "./signatures";
  import type { HistoryTarget } from "./tabKinds";

  const MIN_SPLIT = 0.1;
  const COLUMNS: [LogColumn, string][] = [
    ["author", "Author"],
    ["date", "Date"],
    ["hash", "Hash"],
    // `column.name.commit.signature` (`GitB:1084`): external-status columns come after the others.
    ["signature", SIGNATURE_COLUMN],
  ];
  const LOCATIONS: [LogOptions["diffPreviewAt"], string][] = [
    ["bottom", "Bottom"],
    ["right", "Right"],
  ];

  // `FileHistoryPanel`: the toolbar on top, the table over the details, and the diff preview.
  interface Props {
    view: LogSummary | null;
    generation: number;
    options: LogOptions;
    history: HistoryTarget;
    filters: FilterState;
    refs: LogRef[];
    failure: { message: string; reset: boolean } | null;
    busy: boolean;
    // The table of the tab: the Log keeps its selection when it reloads.
    logView?: LogView;
    onready?: () => void;
    onoptions: (change: Partial<LogOptions>) => void;
    onbranches: (branches: string[]) => void;
    onrefresh: () => void;
    // `FileHistoryFilterUi.resetFiltersToDefault`: the Branch filter back to `HEAD`.
    onresetfilters: () => void;
    onshowinlog: (hash: string) => void;
    onvcs?: (action: VcsAction) => void;
    oncreatepatch?: (changes: LogChange[]) => void;
  }

  let {
    view,
    generation,
    options,
    history,
    filters,
    refs,
    failure,
    busy,
    logView = $bindable(),
    onready,
    onoptions,
    onbranches,
    onrefresh,
    onresetfilters,
    onshowinlog,
    onvcs,
    oncreatepatch,
  }: Props = $props();

  let selection = $state.raw<string[]>([]);
  let pair = $state.raw<RevisionPair | null>(null);
  let menu = $state.raw<{ x: number; y: number; label: string; items: MenuItem[] } | null>(null);
  let previewRequest = 0;
  let menuRequest = 0;

  const showDiffPreview = $derived(options.diffPreview && selection.length > 0);

  const failed = (failure: unknown) => notify({ kind: "error", title: errorText(failure) });
  const failedTitle = (failure: unknown) =>
    notify({ kind: "error", title: CANNOT_OPEN, body: errorText(failure) });
  const commitTarget = (hash: string): LinkTarget => ({ kind: "commit", hash });

  // The toolbar's `Open on GitHub` of the first selected row (`VcsHistoryActionsGroup.Toolbar`):
  // present only while a GitHub repository answers for it.
  let hosting = $state.raw<MenuItem[]>([]);
  let hostingRequest = 0;
  $effect(() => {
    const hash = selection[0];
    const token = ++hostingRequest;
    if (!hash) {
      hosting = [];
      return;
    }
    void hostingItems(commitTarget(hash), "open", OPEN_ON_GITHUB, failedTitle).then((items) => {
      if (token === hostingRequest) hosting = items;
    });
  });

  // One repository opens at once; several drop down, with no arrow on the button otherwise.
  function openOnGithub(anchor: HTMLElement) {
    const [item] = hosting;
    if (!item || !("label" in item)) return;
    if (!item.submenu) {
      item.run?.();
      return;
    }
    const bounds = anchor.getBoundingClientRect();
    menu = { x: bounds.left, y: bounds.bottom, label: OPEN_ON_GITHUB, items: item.submenu };
  }

  // The change of the selection, for the preview and the actions on it.
  const changeOf = (hashes: string[]): Promise<HistoryChange | null> =>
    view && hashes.length > 0 ? logHistoryChange(view.viewId, hashes) : Promise.resolve(null);

  async function preview(hashes: string[]) {
    const token = ++previewRequest;
    if (!options.diffPreview || hashes.length === 0) {
      pair = null;
      return;
    }
    try {
      const change = await changeOf(hashes);
      if (token === previewRequest) pair = change ? historyPair(change, false) : null;
    } catch {
      if (token === previewRequest) pair = null;
    }
  }

  function select(hashes: string[]) {
    selection = hashes;
    void preview(hashes);
  }

  $effect(() => {
    if (options.diffPreview) untrack(() => void preview(selection));
  });

  async function selectedCommits(): Promise<HistoryCommit[]> {
    const rows = (await logView?.selectedCommits()) ?? [];
    if (!view || rows.length === 0) return [];
    const entries = await logHistoryEntries(
      view.viewId,
      rows.map((row) => row.hash),
    );
    return rows.map((row, index) => ({
      hash: row.hash,
      subject: row.subject,
      parentCount: row.parentCount,
      entry: entries[index] ?? null,
    }));
  }

  // `CompareRevisionsFromFileHistoryActionProvider`: the full hashes in the diff window.
  async function showDiff() {
    try {
      const change = await changeOf(selection);
      if (change) await showHistoryDiff(historyPair(change, true), history.path);
    } catch (error) {
      failed(error);
    }
  }

  // `Create Patch...` of `SELECTED_CHANGES`: the history's own change, which a merge does not have.
  function patchChange(change: HistoryChange | null): LogChange | null {
    if (!change || change.before.length > 1) return null;
    const before = change.before[0] ?? null;
    const path = change.after?.path ?? before?.path ?? change.path;
    const oldPath = before && before.path !== path ? before.path : null;
    const status = !before ? "A" : !change.after ? "D" : oldPath ? "R" : "M";
    return { file: { path, oldPath, status }, hash: change.hash, base: before?.hash ?? null };
  }

  const run = (work: () => Promise<unknown>) => () => void work().catch(failed);

  function handlers(change: HistoryChange | null, hosting: MenuItem[] = []): HistoryHandlers {
    const patch = patchChange(change);
    return {
      copy: (text) => void clipboardWrite(text).catch(failed),
      createPatch: oncreatepatch && patch ? () => oncreatepatch([patch]) : undefined,
      // `GetVersionFromHistoryAction`: the revision goes to the history's path.
      get: (commit) =>
        void getRevision([
          {
            path: history.path,
            revision: commit.hash,
            oldPath: commit.entry && commit.entry.path !== history.path ? commit.entry.path : null,
          },
        ]),
      showFileAtRevision: (commit) =>
        run(() =>
          openInViewer({ path: commit.entry?.path ?? history.path, revision: commit.hash, line: null }),
        )(),
      compareWithLocal: (commit) =>
        run(() =>
          compareWithLocal(commit.hash, [
            {
              path: history.path,
              revisionPath: commit.entry?.deleted ? null : (commit.entry?.path ?? history.path),
            },
          ]),
        )(),
      showDiff: () => void showDiff(),
      showAllAffected: (commit) =>
        run(() => showAllAffected(commit.hash, commit.entry?.path ?? history.path))(),
      // `AnnotateRevisionFromHistoryAction`: the file at that revision in the viewer, annotated.
      annotate: (commit) =>
        run(() =>
          openInViewer({
            path: commit.entry?.path ?? history.path,
            revision: commit.hash,
            line: null,
            annotate: true,
          }),
        )(),
      showInGitLog: (commit) => onshowinlog(commit.hash),
      revert: onvcs
        ? (commits) =>
            onvcs({
              kind: "revert",
              commits: commits.map(({ hash, subject }) => ({ hash, subject })),
            })
        : undefined,
      newTag: onvcs ? (commit) => onvcs({ kind: "newTag", hash: commit.hash }) : undefined,
      hosting,
    };
  }

  async function contextMenu(x: number, y: number) {
    const token = ++menuRequest;
    try {
      const commits = await selectedCommits();
      const change = await changeOf(commits.map((commit) => commit.hash));
      const hosting =
        commits.length > 0 ? await hostingItems(commitTarget(commits[0].hash), "open", OPEN_ON_GITHUB, failedTitle) : [];
      if (token !== menuRequest) return;
      const items = historyMenu(commits, handlers(change, hosting));
      if (items.length > 0) menu = { x, y, label: "File History", items };
    } catch (error) {
      failed(error);
    }
  }

  async function allAffected() {
    if (selection.length !== 1) return;
    const [commit] = await selectedCommits().catch(() => []);
    const path = commit?.entry?.path ?? history.path;
    await showAllAffected(selection[0], path).catch(failed);
  }

  async function copyRevisions() {
    const commits = await selectedCommits();
    if (commits.length > 0) handlers(null).copy(revisionsText(commits.map((commit) => commit.hash)));
  }

  // The keys of `Vcs.FileHistory.Toolbar` and the menu: `true` when this tab took the key.
  export function keydown(_event: KeyboardEvent, action: ActionId | null): boolean {
    if (action === "Diff.ShowDiff") {
      void showDiff();
      return true;
    }
    if (action === "Vcs.Log.ShowAllAffected") {
      void allAffected();
      return true;
    }
    if (action === "Vcs.CopyRevisionNumberAction") {
      void copyRevisions();
      return true;
    }
    return false;
  }

  function viewMenu(anchor: HTMLElement) {
    const bounds = anchor.getBoundingClientRect();
    // `Vcs.FileHistory.PresentationSettings` (`LX:246-253`).
    const items: MenuItem[] = [
      {
        label: "Prefer Commit Timestamp",
        checked: options.preferCommitDate,
        run: () => onoptions({ preferCommitDate: !options.preferCommitDate }),
      },
      {
        label: "Columns",
        submenu: COLUMNS.map(([column, label]) => ({
          label,
          checked: options.columns.includes(column),
          run: () => onoptions({ columns: toggleColumn(options.columns, column) }),
        })),
      },
      { header: "Layout" },
      {
        label: "Show Details",
        checked: options.showDetails,
        run: () => onoptions({ showDetails: !options.showDetails }),
      },
      {
        label: "Show Diff Preview",
        checked: options.diffPreview,
        run: () => onoptions({ diffPreview: !options.diffPreview }),
      },
      {
        label: "Diff Preview Location",
        submenu: LOCATIONS.map(([location, label]) => ({
          label,
          checked: options.diffPreviewAt === location,
          run: () => onoptions({ diffPreviewAt: location }),
        })),
      },
    ];
    menu = { x: bounds.left, y: bounds.bottom, label: "View Options", items };
  }

  function drag(event: PointerEvent, key: keyof Splits, vertical: boolean) {
    const divider = event.currentTarget as HTMLElement;
    const bounds = divider.parentElement!.getBoundingClientRect();
    divider.setPointerCapture?.(event.pointerId);
    const move = (moved: PointerEvent) => {
      const part = vertical
        ? (moved.clientY - bounds.top) / bounds.height
        : (moved.clientX - bounds.left) / bounds.width;
      const value = Math.min(1 - MIN_SPLIT, Math.max(MIN_SPLIT, part));
      onoptions({ splits: { ...options.splits, [key]: value } });
    };
    const finish = () => {
      divider.removeEventListener("pointermove", move);
      divider.removeEventListener("pointerup", finish);
    };
    divider.addEventListener("pointermove", move);
    divider.addEventListener("pointerup", finish);
  }
</script>

<section class="history" aria-label="File History">
  <div class="toolbar" role="toolbar" aria-label="File History Toolbar">
    {#if !history.revision}
      <FilterBar
        branchOnly
        {filters}
        {refs}
        users={[]}
        onchange={(changed) => onbranches(branchValues(changed))}
      />
      <span class="separator"></span>
    {/if}
    <button type="button" class="action" aria-label="Refresh" title="Refresh (⌘R)" onclick={onrefresh}>
      <Icon name="refresh" />
    </button>
    <button
      type="button"
      class="action"
      aria-label={showDiffLabel(selection.length)}
      title="{showDiffLabel(selection.length)} (⌘D)"
      disabled={selection.length === 0}
      onclick={showDiff}
    >
      <Icon name="diff" />
    </button>
    <button
      type="button"
      class="action"
      aria-label="Show All Affected Files"
      title="Show All Affected Files (⌃⌘A)"
      disabled={selection.length !== 1}
      onclick={allAffected}
    >
      <Icon name="changes" />
    </button>
    <span class="separator"></span>
    <button
      type="button"
      class="action"
      aria-label="View Options"
      title="View Options"
      onclick={(event) => viewMenu(event.currentTarget)}
    >
      <Icon name="groupBy" />
    </button>
    {#if hosting.length > 0}
      <span class="separator"></span>
      <button
        type="button"
        class="action"
        aria-label={OPEN_ON_GITHUB}
        title={OPEN_ON_GITHUB}
        onclick={(event) => openOnGithub(event.currentTarget)}
      >
        <Icon name="github" />
      </button>
    {/if}
    {#if busy}
      <span class="busy" role="status" aria-label="Loading commits…"></span>
    {/if}
  </div>

  <div class="frame" data-location={options.diffPreviewAt}>
    <div class="main" style:flex={showDiffPreview ? options.splits.diff : 1}>
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="rows"
        style:flex={options.showDetails ? options.splits.details : 1}
        ondblclick={showDiff}
      >
        {#if view}
          <LogView
            bind:this={logView}
            viewId={view.viewId}
            count={view.count}
            {generation}
            {options}
            onselect={select}
            {onready}
            oncontext={(x, y) => void contextMenu(x, y)}
            onwidths={(columnWidths) => onoptions({ columnWidths })}
          />
        {/if}
        {#if failure}
          <div class="empty" role="alert">
            <p>{HISTORY_ERROR}</p>
            <p class="detail">{failure.message}</p>
            {#if failure.reset}
              <button type="button" class="link" onclick={onresetfilters}>{RESET_FILTERS}</button>
            {:else}
              <button type="button" class="link" onclick={onrefresh}>{REFRESH}</button>
            {/if}
          </div>
        {:else if view && view.count === 0}
          <div class="empty"><p>{HISTORY_EMPTY}</p></div>
        {/if}
      </div>
      {#if options.showDetails}
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
          class="divider rows-split"
          role="separator"
          aria-orientation="horizontal"
          onpointerdown={(event) => drag(event, "details", true)}
        ></div>
        <div class="details" style:flex={1 - options.splits.details}>
          <DetailsPanel hashes={selection} {generation} onnavigate={onshowinlog} />
        </div>
      {/if}
    </div>
    {#if showDiffPreview}
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="divider"
        class:columns={options.diffPreviewAt === "right"}
        class:rows-split={options.diffPreviewAt === "bottom"}
        role="separator"
        onpointerdown={(event) => drag(event, "diff", options.diffPreviewAt === "bottom")}
      ></div>
      <section class="preview" style:flex={1 - options.splits.diff} aria-label="Diff Preview">
        {#if pair}
          {#key pair}
            <RevisionsPreview {pair} place="VcsLogView" />
          {/key}
        {/if}
      </section>
    {/if}
  </div>
</section>

{#if menu}
  <Menu x={menu.x} y={menu.y} label={menu.label} items={menu.items} onclose={() => (menu = null)} />
{/if}

<style>
  .history {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }

  .toolbar {
    display: flex;
    flex: none;
    gap: 4px;
    align-items: center;
    height: 36px;
    padding: 0 8px;
    background: var(--panel);
    border-bottom: 1px solid var(--divider);
  }

  .separator {
    flex: none;
    width: 1px;
    height: 18px;
    margin: 0 4px;
    background: var(--divider);
  }

  .busy {
    width: 12px;
    height: 12px;
    margin-left: auto;
    border: 2px solid var(--divider);
    border-top-color: var(--focus);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .frame {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }

  .frame[data-location="right"] {
    flex-direction: row;
  }

  .main {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }

  .rows {
    position: relative;
    min-height: 0;
  }

  .details {
    min-height: 0;
    overflow: auto;
  }

  .empty {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
    align-items: center;
    justify-content: center;
    color: var(--text-secondary);
  }

  .empty p {
    margin: 0;
  }

  .empty .detail {
    max-width: 80%;
    overflow-wrap: anywhere;
  }

  .link {
    padding: 0;
    font: inherit;
    color: var(--link);
    background: none;
    border: 0;
  }

  .divider {
    flex: none;
    background: var(--divider);
  }

  .divider.columns {
    width: 1px;
    cursor: col-resize;
    touch-action: none;
  }

  .divider.rows-split {
    height: 1px;
    cursor: row-resize;
    touch-action: none;
  }

  .preview {
    position: relative;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
    background: var(--editor);
  }
</style>
