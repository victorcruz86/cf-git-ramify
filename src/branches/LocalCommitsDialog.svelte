<script lang="ts">
  import type { LostCommit } from "../lib/api";
  import Dialog from "../lib/Dialog.svelte";
  import { shortHash } from "../log/draw";

  // Two questions with the same shape: what to do with commits that only the local branch has and,
  // with `commits`, whether to bring back the deleted branch that had them.
  interface Props {
    title: string;
    message: string;
    commits?: LostCommit[];
    rebase?: boolean;
    onanswer: (answer: "rebase" | "drop" | "restore" | null) => void;
  }

  let { title, message, commits, rebase = false, onanswer }: Props = $props();
</script>

<Dialog {title} onclose={() => onanswer(null)} width={commits ? 620 : 460}>
  <p>{message}</p>
  {#if commits}
    <ul aria-label="Commits">
      {#each commits as commit (commit.hash)}
        <li>
          <span class="hash">{shortHash(commit.hash)}</span>
          <span class="subject">{commit.subject}</span>
          <span class="author">{commit.author}</span>
        </li>
      {/each}
    </ul>
  {/if}
  {#snippet footer()}
    {#if commits}
      <button type="button" class="button" data-autofocus onclick={() => onanswer(null)}>Cancel</button>
      <button type="button" class="button" onclick={() => onanswer("restore")}>Restore</button>
    {:else}
      {#if rebase}
        <button type="button" class="button" onclick={() => onanswer("rebase")}>
          Rebase onto Remote
        </button>
      {/if}
      <button type="button" class="button" onclick={() => onanswer("drop")}>
        Drop Local Commits
      </button>
      <button type="button" class="button default" data-autofocus onclick={() => onanswer(null)}>
        Cancel
      </button>
    {/if}
  {/snippet}
</Dialog>

<style>
  p {
    margin: 0 0 8px;
  }

  ul {
    max-height: 280px;
    margin: 0;
    padding: 4px 8px;
    overflow: auto;
    list-style: none;
    background: var(--background);
    border: 1px solid var(--popup-border);
    border-radius: 4px;
  }

  li {
    display: flex;
    gap: 10px;
    line-height: 22px;
  }

  .hash,
  .author {
    flex: none;
    color: var(--disabled);
  }

  .subject {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
