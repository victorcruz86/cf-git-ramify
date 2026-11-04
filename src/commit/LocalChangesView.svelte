<script lang="ts">
  import type { ChangeList, Changelists, FileChange, RepoInfo, StageComparison } from "../lib/api";
  import Icon from "../lib/Icon.svelte";
  import { matchShortcut } from "../lib/keymap";
  import { store, stored } from "../lib/storage";
  import FilePane from "../diff/FilePane.svelte";
  import { loadOptions } from "../diff/options";
  import ChangesPanel from "./ChangesPanel.svelte";
  import type { CheckinRequest } from "./checkin";

  // `VcsConfiguration.LOCAL_CHANGES_DETAILS_PREVIEW_SHOWN` (default true) and the proportion of
  // `PreviewDiffSplitterComponent` (0.3 for the tree), under IntelliJ's keys.
  const PREVIEW_KEY = "VcsConfiguration.LOCAL_CHANGES_DETAILS_PREVIEW_SHOWN";
  const PROPORTION_KEY = "ChangesViewManager.DETAILS_SPLITTER_PROPORTION";
  const PROPORTION = 0.3;
  const MIN_PROPORTION = 0.1;
  const MAX_PROPORTION = 0.9;

  interface Props {
    repo: RepoInfo;
    active: boolean;
    // `Commit…` and `Commit File…`: the Commit Changes dialog decides what goes in.
    oncheckin: (request: CheckinRequest) => void;
    onshowdiff?: (file: FileChange, comparison?: StageComparison) => void;
    // The `Resolve` link of the conflicts node: opens the Conflicts dialog.
    onresolveconflicts?: () => void;
    // Whether any file is in conflict: enables `Resolve Conflicts…` of the Git menu.
    onconflictschange?: (conflicts: boolean) => void;
  }

  let { repo, active, oncheckin, onshowdiff, onresolveconflicts, onconflictschange }: Props =
    $props();

  let panel = $state<ChangesPanel>();
  let lists = $state.raw<Changelists | null>(null);
  let previewShown = $state(stored(PREVIEW_KEY) !== "false");
  let proportion = $state(Number(stored(PROPORTION_KEY)) || PROPORTION);
  // The file under the cursor, which the preview shows.
  let previewPath = $state<string | null>(null);
  // Goes up with each refresh: the file shown may have changed on disk.
  let version = $state(0);
  let root: HTMLElement;
  const options = loadOptions();

  export function focusTree() {
    panel?.focusTree();
  }

  // After a commit of this list from the dialog: if it is not the active one and was left empty, its
  // removal is offered.
  export async function offerToRemoveEmpty(list: ChangeList) {
    if (list.id === lists?.active) return;
    await panel?.refresh();
    panel?.offerToRemoveEmpty([list]);
  }

  function checkinProject() {
    const selection = panel?.selectedChanges() ?? { paths: [], lists: [] };
    oncheckin({ kind: "project", ...selection });
  }

  function checkinFiles(paths: string[], chosen: string[]) {
    oncheckin({ kind: "files", paths, lists: chosen });
  }

  function showDiff(file: FileChange, comparison?: StageComparison) {
    onshowdiff?.(file, comparison);
  }

  // `ChangesView.SingleClickPreview`.
  function togglePreview() {
    previewShown = !previewShown;
    store(PREVIEW_KEY, String(previewShown));
  }

  // ⌘K from the tree. From the other tabs the application handles it.
  function keydown(event: KeyboardEvent) {
    if (!active || panel?.hasPopup()) return;
    if (matchShortcut(event) !== "CheckinProject") return;
    event.preventDefault();
    checkinProject();
  }

  function startResize(event: PointerEvent) {
    const handle = event.currentTarget as HTMLElement;
    handle.setPointerCapture(event.pointerId);
    const bounds = root.getBoundingClientRect();
    const move = (next: PointerEvent) => {
      if (bounds.width <= 0) return;
      const ratio = (next.clientX - bounds.left) / bounds.width;
      proportion = Math.max(MIN_PROPORTION, Math.min(MAX_PROPORTION, ratio));
    };
    const end = () => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", end);
      store(PROPORTION_KEY, String(proportion));
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", end);
  }
</script>

<svelte:window onkeydown={keydown} />

{#snippet commitAction()}
  <button
    type="button"
    class="action"
    title="Commit… (⌘K)"
    aria-label="Commit…"
    onclick={checkinProject}
  >
    <Icon name="commit" />
  </button>
{/snippet}

{#snippet previewAction()}
  <button
    type="button"
    class="action"
    class:toggled={previewShown}
    title="Preview Diff"
    aria-label="Preview Diff"
    aria-pressed={previewShown}
    onclick={togglePreview}
  >
    <Icon name="previewDetails" />
  </button>
{/snippet}

<section bind:this={root} class="view" aria-label="Local Changes">
  <div class="changes" style:flex={previewShown ? `0 0 ${proportion * 100}%` : "1"}>
    <ChangesPanel
      bind:this={panel}
      bind:lists
      {repo}
      {active}
      checkable={false}
      toolbar="vertical"
      onrefresh={() => version++}
      onopen={showDiff}
      onshowdiff={showDiff}
      onselection={(_paths, _lists, lead) => (previewPath = lead)}
      oncheckinfiles={checkinFiles}
      {onresolveconflicts}
      {onconflictschange}
      actions={commitAction}
      trailing={previewAction}
    />
  </div>

  {#if previewShown}
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <div
      class="splitter"
      role="separator"
      aria-orientation="vertical"
      onpointerdown={startResize}
    ></div>
    <!-- Nothing is read while it is hidden: the pane exists only while it is shown. -->
    <section class="preview" aria-label="Diff Preview">
      {#if previewPath}
        {#key previewPath}
          <FilePane
            path={previewPath}
            {version}
            {options}
            included={false}
            selection={[]}
            {lists}
            embedded
            readonly
            onselection={() => {}}
            onnotice={(kind, text, action) => panel?.notify(kind, text, action)}
          />
        {/key}
      {/if}
    </section>
  {/if}
</section>

<style>
  .view {
    position: relative;
    display: flex;
    flex: 1;
    min-width: 0;
    height: 100%;
    background: var(--panel);
  }

  .changes {
    display: flex;
    min-width: 0;
    min-height: 0;
  }

  .splitter {
    flex: none;
    width: 5px;
    margin: 0 -2px;
    cursor: col-resize;
    background: linear-gradient(
      90deg,
      transparent 2px,
      var(--border) 2px,
      var(--border) 3px,
      transparent 3px
    );
    position: relative;
    z-index: 2;
  }

  .preview {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }

  .action.toggled {
    background: var(--stripe-selected);
  }

  .action.toggled :global(.icon) {
    color: var(--accent-text);
  }
</style>
