<script lang="ts">
  import { onMount, tick } from "svelte";
  import { consoleLines, onConsoleLine, type ConsoleLine } from "../lib/api";
  import { errorText } from "../lib/errors";
  import { notify } from "../lib/notifications";

  let { active }: { active: boolean } = $props();

  // Like the backend's ring.
  const LIMIT = 2_000;

  let lines = $state.raw<ConsoleLine[]>([]);
  let log = $state<HTMLElement>();

  // Whatever arrives while the history loads is kept and appended afterwards.
  onMount(() => {
    let loaded = false;
    const early: ConsoleLine[] = [];
    const stop = onConsoleLine((line) => {
      if (loaded) lines = [...lines, line].slice(-LIMIT);
      else early.push(line);
    });
    consoleLines()
      .then((history) => {
        loaded = true;
        lines = [...history, ...early].slice(-LIMIT);
      })
      .catch((failure) => notify({ kind: "error", title: errorText(failure) }));
    return () => {
      stop.then((unlisten) => unlisten());
    };
  });

  // When a line arrives or the tab is shown, like `Vcs.ShowConsoleTab`, it scrolls to the end.
  $effect(() => {
    lines;
    if (!active) return;
    tick().then(() => {
      if (log) log.scrollTop = log.scrollHeight;
    });
  });
</script>

<div class="console" role="log" aria-label="Console" bind:this={log}>
  {#each lines as line}
    {@const text = line.command ? `${line.time}: ${line.text}` : line.text}
    <div class="line" class:command={line.command}>{text}</div>
  {/each}
</div>

<style>
  .console {
    flex: 1;
    min-height: 0;
    padding: 6px 12px;
    overflow: auto;
    font: 12px ui-monospace, SFMono-Regular, Menlo, monospace;
    user-select: text;
    background: var(--editor);
  }

  .line {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .command {
    color: var(--text-secondary);
  }
</style>
