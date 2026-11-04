<script lang="ts">
  import { untrack } from "svelte";
  import type { ResetMode } from "../lib/api";
  import Dialog from "../lib/Dialog.svelte";
  import { shortHash } from "../log/draw";
  import type { ResetTarget } from "./actions";

  interface Props {
    source: string;
    target: ResetTarget;
    mode: ResetMode;
    onanswer: (mode: ResetMode | null) => void;
  }

  let { source, target, mode, onanswer }: Props = $props();

  const MODES: { id: ResetMode; label: string; lines: string[] }[] = [
    { id: "soft", label: "Soft", lines: ["Files won't change, differences will be staged for commit."] },
    { id: "mixed", label: "Mixed", lines: ["Files won't change, differences won't be staged."] },
    {
      id: "hard",
      label: "Hard",
      lines: [
        "Files will be reverted to the state of the selected commit.",
        "Warning: any local changes will be lost.",
      ],
    },
    {
      id: "keep",
      label: "Keep",
      lines: [
        "Files will be reverted to the state of the selected commit,",
        "but local changes will be kept intact.",
      ],
    },
  ];
  const MAX_TEXT = 40;

  // `StringUtil.shortenTextWithEllipsis`.
  const short = (text: string) =>
    text.length > MAX_TEXT ? `${text.slice(0, MAX_TEXT - 1)}…` : text;

  let chosen = $state(untrack(() => mode));
</script>

<Dialog title="Git Reset" onclose={() => onanswer(null)} onsubmit={() => onanswer(chosen)} width={460}>
  <p>
    <b>{source}</b> -&gt; <b>{shortHash(target.hash)}</b> "{short(target.subject)}" by
    <code>{short(target.author)}</code>
  </p>
  <p>
    This will reset the current branch head to the selected commit,<br />
    and update the working tree and the index according to the selected mode:
  </p>
  <div class="modes" role="radiogroup" aria-label="Reset mode">
    {#each MODES as { id, label, lines } (id)}
      <label class="mode">
        <input type="radio" name="reset-mode" value={id} bind:group={chosen} data-autofocus={id === chosen || undefined} />
        <span>
          {label}
          {#each lines as line}
            <span class="line">{line}</span>
          {/each}
        </span>
      </label>
    {/each}
  </div>
  {#snippet footer()}
    <button type="button" class="button" onclick={() => onanswer(null)}>Cancel</button>
    <button class="button default">Reset</button>
  {/snippet}
</Dialog>

<style>
  p {
    margin: 0 0 8px;
  }

  .modes {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .mode {
    display: flex;
    gap: 8px;
    align-items: flex-start;
  }

  .line {
    display: block;
    color: var(--disabled);
  }
</style>
