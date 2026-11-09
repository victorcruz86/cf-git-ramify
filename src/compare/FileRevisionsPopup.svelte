<script lang="ts">
  import { onMount, tick } from "svelte";
  import type { FileRevision } from "../lib/api";
  import { prettyDateTime } from "../log/draw";
  import { speedMatch } from "../lib/speedSearch";

  // `CompareWithSelectedRevisionAction.showListPopup`: a table without a header (`Revision`, the
  // date and `Author`), speed search that selects the first match, and the commit message of the
  // selected row below (`V:115`). A click selects; Enter or a double click chooses.
  interface Props {
    revisions: FileRevision[];
    now: number;
    onchoose: (revision: FileRevision) => void;
    onclose: () => void;
  }

  let { revisions, now, onchoose, onclose }: Props = $props();

  const TITLE = "File Revisions";
  const MESSAGE_TITLE = "Commit message";

  let box: HTMLDivElement;
  let query = $state("");
  let current = $state(0);

  const selected = $derived(revisions[current] ?? null);

  // `getElementText`: the revision and the author (git revisions have no branch).
  const searchText = (revision: FileRevision) => `${revision.hash} ${revision.author}`;

  function choose() {
    if (!selected) return;
    const revision = selected;
    onclose();
    onchoose(revision);
  }

  async function select(index: number) {
    if (index < 0 || index >= revisions.length) return;
    current = index;
    await tick();
    const row = box.querySelectorAll<HTMLElement>('[role="row"]')[current];
    row?.scrollIntoView?.({ block: "nearest" });
  }

  function search(next: string) {
    query = next;
    if (!next) return;
    const found = revisions.findIndex((revision) => speedMatch(searchText(revision), next));
    if (found >= 0) select(found);
  }

  function keydown(event: KeyboardEvent) {
    event.stopPropagation();
    if (event.key === "Escape") {
      event.preventDefault();
      if (query) query = "";
      else onclose();
    } else if (event.key === "Enter") {
      event.preventDefault();
      choose();
    } else if (event.key === "ArrowDown") {
      event.preventDefault();
      select(current + 1);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      select(current - 1);
    } else if (event.key === "Backspace") {
      event.preventDefault();
      search(query.slice(0, -1));
    } else if (event.key.length === 1 && !event.metaKey && !event.ctrlKey) {
      event.preventDefault();
      search(query + event.key);
    }
  }

  function pointerdown(event: PointerEvent) {
    if (!box.contains(event.target as Node)) onclose();
  }

  onMount(() => box.focus());
</script>

<svelte:window onpointerdown={pointerdown} />

<div
  bind:this={box}
  class="popup"
  role="dialog"
  aria-label={TITLE}
  tabindex="-1"
  onkeydown={keydown}
>
  <div class="title">{TITLE}</div>
  {#if query}
    <div class="query">{query}</div>
  {/if}
  <div class="table" role="grid" aria-label={TITLE}>
    {#each revisions as revision, index (revision.hash)}
      <div
        role="row"
        tabindex="-1"
        aria-selected={index === current}
        class:current={index === current}
        onclick={() => select(index)}
        ondblclick={choose}
        onkeydown={() => {}}
      >
        <span role="gridcell" class="hash">{revision.hash}</span>
        <span role="gridcell">{prettyDateTime(revision.commitTime * 1000, now)}</span>
        <span role="gridcell">{revision.author}</span>
      </div>
    {/each}
  </div>
  <div class="message" role="group" aria-label={MESSAGE_TITLE}>
    <div class="message-title">{MESSAGE_TITLE}</div>
    <textarea readonly rows="5" aria-label={MESSAGE_TITLE} value={selected?.message ?? ""}
    ></textarea>
  </div>
</div>

<style>
  .popup {
    position: fixed;
    top: 50%;
    left: 50%;
    z-index: 30;
    display: flex;
    flex-direction: column;
    width: min(760px, calc(100vw - 32px));
    min-height: 300px;
    max-height: min(560px, calc(100vh - 32px));
    background: var(--popup-background);
    border: 1px solid var(--popup-border);
    border-radius: 8px;
    outline: none;
    box-shadow: 0 6px 24px rgb(0 0 0 / 40%);
    transform: translate(-50%, -50%);
  }

  .title {
    flex: none;
    padding: 8px 14px 6px;
    font-weight: 600;
    color: var(--text);
  }

  .query {
    flex: none;
    height: 24px;
    margin: 0 14px 4px;
    overflow: hidden;
    line-height: 24px;
    color: var(--text);
    text-overflow: ellipsis;
    white-space: nowrap;
    border-bottom: 1px solid var(--menu-separator);
  }

  .table {
    flex: 1;
    min-height: 50px;
    overflow: auto;
  }

  [role="row"] {
    display: grid;
    grid-template-columns: auto 160px minmax(80px, 1fr);
    gap: 16px;
    height: 24px;
    padding: 0 14px;
    line-height: 24px;
    color: var(--text);
    white-space: nowrap;
  }

  [role="row"].current {
    background: var(--selection);
  }

  [role="gridcell"] {
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .hash {
    font-family: ui-monospace, Menlo, monospace;
  }

  .message {
    display: flex;
    flex: none;
    flex-direction: column;
    gap: 4px;
    padding: 8px 14px 12px;
    border-top: 1px solid var(--divider);
  }

  .message-title {
    color: var(--text-secondary);
  }

  textarea {
    font: inherit;
    color: var(--text);
    resize: none;
    background: var(--editor);
    border: 1px solid var(--border);
    border-radius: 4px;
  }
</style>
