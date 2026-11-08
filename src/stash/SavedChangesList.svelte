<script lang="ts">
  import Icon from "../lib/Icon.svelte";
  import { matchShortcut } from "../lib/keymap";
  import { speedMatch } from "../lib/speedSearch";
  import { changeKey, changeSections, fileName, shownChanges, type SavedChange } from "./savedPatches";

  // `SavedPatchesChangesBrowser`: the changes of the chosen patch, by tag and folder. `empty` is the
  // text while there are none (nothing chosen, loading, an error). `query` is the speed search of the
  // patch list, whose matches are marked here too (`CombinedSpeedSearch`).
  interface Props {
    changes: SavedChange[];
    empty: string;
    current: SavedChange | null;
    query?: string;
    onselect: (change: SavedChange) => void;
    onopen: (change: SavedChange) => void;
    oncontext?: (event: MouseEvent, change: SavedChange) => void;
  }

  let { changes, empty, current, query = "", onselect, onopen, oncontext }: Props = $props();

  const STATUS_COLORS: Record<string, string> = {
    A: "var(--status-added)",
    D: "var(--status-deleted)",
    M: "var(--status-modified)",
    R: "var(--status-modified)",
  };

  const sections = $derived(changeSections(changes));
  const ordered = $derived(shownChanges(changes));
  const isCurrent = (change: SavedChange) => current !== null && changeKey(current) === changeKey(change);

  function keydown(event: KeyboardEvent) {
    const index = current ? ordered.findIndex(isCurrent) : -1;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      const step = event.key === "ArrowDown" ? 1 : -1;
      const next = ordered[Math.min(Math.max(index + step, 0), ordered.length - 1)];
      if (next) onselect(next);
    } else if ((event.key === "Enter" || matchShortcut(event) === "Diff.ShowDiff") && current) {
      // `Show Diff` (⌘D) of `ChangesBrowserBase`, like Enter.
      onopen(current);
    } else {
      return;
    }
    event.preventDefault();
  }
</script>

<div class="tree" role="tree" aria-label="Changes" tabindex="0" onkeydown={keydown}>
  {#if changes.length === 0}
    <p class="status">{empty}</p>
  {/if}
  {#each sections as section (section.title)}
    {#if section.title}
      <p class="title">{section.title}</p>
    {/if}
    {#each section.folders as folder (folder.name)}
      {#if folder.name}
        <p class="folder" class:nested={section.title}><Icon name="folder" size={14} />{folder.name}</p>
      {/if}
      {#each folder.changes as change (changeKey(change))}
        {@const name = fileName(change.file.path)}
        <!-- The keys go to the tree, which holds the selection. -->
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <div
          role="treeitem"
          aria-label={name}
          aria-selected={isCurrent(change)}
          tabindex="-1"
          class="file"
          class:current={isCurrent(change)}
          class:nested={folder.name}
          class:grouped={section.title}
          class:matched={query !== "" && speedMatch(name, query) !== null}
          style:color={STATUS_COLORS[change.file.status[0]]}
          title={change.file.oldPath ? `${change.file.oldPath} → ${change.file.path}` : change.file.path}
          onclick={() => onselect(change)}
          ondblclick={() => {
            onselect(change);
            onopen(change);
          }}
          oncontextmenu={(event) => {
            event.preventDefault();
            onselect(change);
            oncontext?.(event, change);
          }}
        >
          {name}
        </div>
      {/each}
    {/each}
  {/each}
</div>

<style>
  .tree {
    flex: 1;
    min-height: 0;
    padding: 2px 0;
    overflow: auto;
    outline: none;
  }

  .status {
    padding: 16px;
    margin: 0;
    color: var(--text-secondary);
    text-align: center;
  }

  .title {
    padding: 2px 8px;
    margin: 0;
    color: var(--text-secondary);
  }

  .folder {
    display: flex;
    gap: 4px;
    align-items: center;
    padding: 2px 8px;
    margin: 0;
    overflow: hidden;
    color: var(--text-secondary);
    white-space: nowrap;
  }

  .folder.nested,
  .file.grouped {
    padding-left: 26px;
  }

  .file {
    padding: 2px 8px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .file.nested {
    padding-left: 26px;
  }

  .file.grouped.nested {
    padding-left: 44px;
  }

  .file.current {
    background: var(--selection);
  }

  .file.matched {
    text-decoration: underline;
  }
</style>
