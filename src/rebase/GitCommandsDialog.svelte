<script lang="ts">
  import Dialog from "../lib/Dialog.svelte";
  import type { RebaseEntry } from "./model";

  // `GitRebaseCommandsDialog`: the list that will be written to `git-rebase-todo`, read-only.
  interface Props {
    entries: RebaseEntry[];
    onclose: () => void;
  }

  let { entries, onclose }: Props = $props();
</script>

<Dialog title="Git Commands" width={600} {onclose} onsubmit={onclose}>
  <div class="frame">
    <table>
      <thead>
        <tr>
          <th>Action</th>
          <th>Hash</th>
          <th>Subject</th>
        </tr>
      </thead>
      <tbody>
        {#each entries as entry, index (index)}
          <tr>
            <td class="fit">{entry.action}</td>
            <td class="fit">{entry.hash}</td>
            <td>{entry.subject}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
  {#snippet footer()}
    <button class="button default" data-autofocus>OK</button>
  {/snippet}
</Dialog>

<style>
  .frame {
    height: 300px;
    overflow: auto;
    border: 1px solid var(--divider);
  }

  table {
    width: 100%;
    border-collapse: collapse;
  }

  th,
  td {
    height: 22px;
    padding: 0 8px;
    text-align: left;
    white-space: nowrap;
  }

  th {
    position: sticky;
    top: 0;
    font-weight: normal;
    color: var(--text-secondary);
    background: var(--panel);
    border-bottom: 1px solid var(--divider);
  }

  td:not(.fit) {
    width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>
