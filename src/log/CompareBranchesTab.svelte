<script lang="ts" module>
  import type { LogSummary } from "../lib/api";
  import type { FilterState } from "./filters";

  // One of the two logs: its view over `start..end` and the filters it has besides the range.
  export interface CompareHalf {
    start: string;
    end: string;
    view: LogSummary | null;
    generation: number;
    filters: FilterState;
  }

  export type HalfName = "upper" | "lower";
</script>

<script lang="ts">
  import type { LogRef, Person } from "../lib/api";
  import Icon from "../lib/Icon.svelte";
  import type { ActionId } from "../lib/keymap";
  import type { LogChange } from "./changes";
  import ChangesPanel from "./ChangesPanel.svelte";
  import type { VcsAction } from "./contextMenu";
  import DetailsPanel from "./DetailsPanel.svelte";
  import FilterBar from "./FilterBar.svelte";
  import LogActions from "./LogActions.svelte";
  import LogView from "./LogView.svelte";
  import type { LogOptions } from "./options";

  // `VcsLogClassicFilterUi`'s generic empty text, `Reset filters` its link.
  const NO_MATCHES = "No commits matching filters";
  const RESET_FILTERS = "Reset filters";
  const NAMES: HalfName[] = ["upper", "lower"];

  // `GitCompareBranchesUi`: two logs one over the other, `start..end` above and reversed below.
  // Each is a `VcsLogPanel` of its own: the filters minus Branch (kept in memory only), the banner,
  // the table, the changes and details of its selection and its diff preview. There is no swap.
  interface Props {
    upper: CompareHalf;
    lower: CompareHalf;
    options: LogOptions;
    refs: LogRef[];
    users: Person[];
    currentBranch: string | null;
    busy: boolean;
    onfilters: (half: HalfName, filters: FilterState) => void;
    onoptions: (change: Partial<LogOptions>) => void;
    onrefresh: () => void;
    onnewtab: () => Promise<void>;
    onready?: () => void;
    onvcs?: (action: VcsAction) => void | Promise<void>;
    oncreatepatch?: (changes: LogChange[]) => void;
    // The repository's folder, for the Paths filter's `Select in Tree…`.
    root?: string;
  }

  let {
    upper,
    lower,
    options,
    refs,
    users,
    currentBranch,
    busy,
    onfilters,
    onoptions,
    onrefresh,
    onnewtab,
    onready,
    onvcs,
    oncreatepatch,
    root = "",
  }: Props = $props();

  const halves = $derived<[HalfName, CompareHalf][]>([
    ["upper", upper],
    ["lower", lower],
  ]);

  const none = <T,>(): Record<HalfName, T | undefined> => ({ upper: undefined, lower: undefined });
  let views = $state(none<LogView>());
  let actions = $state(none<LogActions>());
  let filterBars = $state(none<FilterBar>());
  let sections = $state(none<HTMLElement>());
  let selections = $state.raw<Record<HalfName, string[]>>({ upper: [], lower: [] });
  let changes = $state.raw<Record<HalfName, LogChange | null>>({ upper: null, lower: null });
  // Each changes panel's own changes, for its half's Create Patch....
  let direct = $state.raw<Record<HalfName, LogChange[]>>({ upper: [], lower: [] });
  // `Show Diff` (⌘D or a double click in the changes) opens the preview while it is off.
  let diffOpen = $state<Record<HalfName, boolean>>({ upper: false, lower: false });

  const showDiff = (name: HalfName) =>
    (options.diffPreview || diffOpen[name]) && selections[name].length > 0;

  // `MyFilterUi.setCustomEmptyText`: any filter other than the range gives the generic text.
  function otherFilters({ filters }: CompareHalf): boolean {
    const { text, users, since, until, paths } = filters;
    return Boolean(text.trim()) || users.length > 0 || since !== null || until !== null || paths.length > 0;
  }

  // `MyFilterUi.setFilters` with an empty collection: the other filters go, the range stays.
  function reset(name: HalfName, half: CompareHalf) {
    onfilters(name, {
      ...half.filters,
      text: "",
      users: [],
      since: null,
      until: null,
      paths: [],
    });
  }

  // `B:1118`.
  const containsAll = ({ start, end }: CompareHalf) => `${start} contains all commits from ${end}`;
  // `B:1119`, as plain text: the name of the half.
  const explanation = ({ start, end }: CompareHalf) =>
    `Commits that exist in ${end} but don't exist in ${start}`;

  function select(name: HalfName, hashes: string[]) {
    const same =
      hashes.length === selections[name].length &&
      hashes.every((hash, index) => hash === selections[name][index]);
    if (same) return;
    selections = { ...selections, [name]: hashes };
    if (hashes.length === 0) diffOpen[name] = false;
  }

  // The file of the half's diff preview. The changes panel says it again on each of its runs:
  // only a different one is a change.
  function chooseChange(name: HalfName, change: LogChange | null) {
    if (changes[name] !== change) changes = { ...changes, [name]: change };
  }

  function keepChanges(name: HalfName, loaded: LogChange[]) {
    if (direct[name] !== loaded) direct = { ...direct, [name]: loaded };
  }

  function closeDiff(name: HalfName) {
    diffOpen[name] = false;
    views[name]?.focus();
  }

  // The keys of the log go to the half that has the focus: `FocusTextFilterAction` (⌘L) and
  // `Diff.ShowDiff` (⌘D) as in the Log, the rest to its table's actions.
  export function keydown(event: KeyboardEvent, action: ActionId | null): boolean {
    const target = event.target as Node | null;
    const name = NAMES.find((half) => sections[half]?.contains(target));
    if (!name) return false;
    if (action === "Vcs.Log.FocusTextFilter") {
      if (filterBars[name]?.hasFocus()) views[name]?.focus();
      else filterBars[name]?.focusText();
      event.preventDefault();
    } else if (action === "Diff.ShowDiff") {
      if (selections[name].length > 0) diffOpen[name] = true;
      event.preventDefault();
    } else {
      actions[name]?.keydown(event, action);
    }
    return true;
  }

  // The table of a half, which the Log keeps the selection of when it reloads.
  export const table = (name: HalfName) => views[name];

  export function focus() {
    views.upper?.focus();
  }
</script>

<div class="compare">
  {#each halves as [name, half] (name)}
    <section bind:this={sections[name]} class="half" aria-label={explanation(half)}>
      <p class="banner">
        Commits that exist in <code><b>{half.end}</b></code> but don't exist in
        <code><b>{half.start}</b></code>
      </p>
      <div class="frame" data-location={options.diffPreviewAt}>
        <div class="main" style:flex={showDiff(name) ? options.splits.diff : 1}>
          <div class="table" style:flex={options.splits.table}>
            <FilterBar
              bind:this={filterBars[name]}
              withoutBranch
              filters={half.filters}
              {root}
              {refs}
              {users}
              {busy}
              onchange={(filters) => onfilters(name, filters)}
              {onrefresh}
              ongoto={() => actions[name]?.goToRef()}
            />
            <div class="rows">
              {#if half.view}
                <LogView
                  bind:this={views[name]}
                  viewId={half.view.viewId}
                  count={half.view.count}
                  generation={half.generation}
                  {options}
                  onselect={(hashes) => select(name, hashes)}
                  {onready}
                  oncontext={(x, y) => actions[name]?.showContextMenu(x, y)}
                  onwidths={(columnWidths) => onoptions({ columnWidths })}
                />
                {#if half.view.count === 0}
                  <div class="empty">
                    {#if otherFilters(half)}
                      <p>{NO_MATCHES}</p>
                      <button type="button" class="link" onclick={() => reset(name, half)}>
                        {RESET_FILTERS}
                      </button>
                    {:else}
                      <p>{containsAll(half)}</p>
                    {/if}
                  </div>
                {/if}
              {/if}
            </div>
          </div>
          <div class="divider columns" role="separator" aria-orientation="vertical"></div>
          <div class="side" style:flex={1 - options.splits.table}>
            <div class="pane" style:flex={options.showDetails ? options.splits.details : 1}>
              <ChangesPanel
                hashes={selections[name]}
                paths={half.filters.paths}
                {options}
                {onoptions}
                onselect={(change) => chooseChange(name, change)}
                onopen={() => (diffOpen[name] = true)}
                onchanges={(loaded) => keepChanges(name, loaded)}
                {onvcs}
                {oncreatepatch}
              />
            </div>
            {#if options.showDetails}
              <div class="divider rows-split" role="separator" aria-orientation="horizontal"></div>
              <div class="pane" style:flex={1 - options.splits.details}>
                <DetailsPanel
                  hashes={selections[name]}
                  generation={half.generation}
                  onnavigate={(hash) => views[name]?.selectHash(hash)}
                />
              </div>
            {/if}
          </div>
        </div>
        {#if showDiff(name)}
          <div
            class="divider"
            class:columns={options.diffPreviewAt === "right"}
            class:rows-split={options.diffPreviewAt === "bottom"}
            role="separator"
          ></div>
          <section class="preview" style:flex={1 - options.splits.diff} aria-label="Diff Preview">
            {#if diffOpen[name] && !options.diffPreview}
              <button
                type="button"
                class="action close-diff"
                aria-label="Close"
                onclick={() => closeDiff(name)}
              >
                <Icon name="close" size={14} />
              </button>
            {/if}
            {#if changes[name]}
              {@const change = changes[name]}
              {#await import("../diff/DiffView.svelte") then { default: DiffView }}
                <DiffView hash={change.hash} file={change.file} base={change.base} />
              {/await}
            {/if}
          </section>
        {/if}
      </div>
    </section>
    {#if half.view}
      <LogActions
        bind:this={actions[name]}
        logView={views[name]}
        viewId={half.view.viewId}
        {refs}
        {currentBranch}
        hashes={selections[name]}
        filtered={otherFilters(half)}
        onresetfilters={async () => reset(name, half)}
        {onnewtab}
        {onvcs}
        paths={half.filters.paths}
        {oncreatepatch}
        changes={direct[name]}
      />
    {/if}
    {#if name === "upper"}
      <div class="divider rows-split" role="separator" aria-orientation="horizontal"></div>
    {/if}
  {/each}
</div>

<style>
  .compare {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }

  .half {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }

  .banner {
    flex: none;
    padding: 4px 8px;
    margin: 0;
    color: var(--text);
    border-bottom: 1px solid var(--divider);
  }

  .banner code {
    font-family: var(--mono, monospace);
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
    min-width: 0;
    min-height: 0;
  }

  .table,
  .side {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }

  .rows {
    position: relative;
    flex: 1;
    min-height: 0;
  }

  .pane {
    min-height: 0;
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
  }

  .divider.rows-split {
    height: 1px;
  }

  .preview {
    position: relative;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
    background: var(--editor);
  }

  .close-diff {
    position: absolute;
    top: 4px;
    right: 12px;
    z-index: 2;
  }
</style>
