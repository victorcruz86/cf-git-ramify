<script lang="ts">
  // `GitMergeRebaseWidget`: icon buttons after the branch widget, one for each action of
  // `GitMergeRebaseWidgetGroup` that applies to the operation in progress. Resolve and Continue have the
  // run widget's running background, the aborts its stop background (`ColoredActionButtonLook`).
  import Icon from "../lib/Icon.svelte";
  import type { RepoState } from "../lib/api";
  import type { MenuAction } from "../lib/menu";
  import { ongoingActions, RESOLVE_ID, type OngoingHandlers } from "./operation";

  interface Button {
    id: string;
    label: string;
    icon: string;
    stop: boolean;
    run: () => void;
  }

  let {
    state,
    conflicts,
    handlers,
    onresolve,
  }: {
    state: RepoState;
    // `GitResolveConflictsAction`: any conflict in the changelist.
    conflicts: boolean;
    handlers: OngoingHandlers;
    onresolve: () => void;
  } = $props();

  const CONTINUE = "Continue Rebase";
  const RESOLVE = "Resolve Conflicts…";

  // The order of `GitMergeRebaseWidgetGroup`: Resolve, Continue, then the aborts. `Skip Commit` is not in it.
  const buttons = $derived.by<Button[]>(() => {
    const actions = ongoingActions(state, handlers).filter(
      (item): item is MenuAction => item !== null && "label" in item,
    );
    const button = (item: MenuAction): Button => {
      const stop = item.label.startsWith("Abort");
      return {
        id: item.label,
        label: item.label,
        icon: stop ? "abort" : "continue",
        stop,
        run: () => item.run?.(),
      };
    };
    const resume = actions.filter((item) => item.label === CONTINUE).map(button);
    const aborts = actions.filter((item) => item.label.startsWith("Abort")).map(button);
    // `GitResolveConflictsAction.update`: in the widget it is hidden while rebasing, where Continue Rebase is.
    const resolve: Button[] = conflicts && state.operation !== "rebasing"
      ? [{ id: RESOLVE_ID, label: RESOLVE, icon: "resolveConflicts", stop: false, run: onresolve }]
      : [];
    return [...resolve, ...resume, ...aborts];
  });
</script>

{#if buttons.length > 0}
  <div class="widget" role="toolbar" aria-label="Merge and rebase">
    {#each buttons as { id, label, icon, stop, run } (id)}
      <button
        type="button"
        class="button"
        aria-label={label}
        title={label}
        style:--button-background={stop ? "var(--run-stop)" : "var(--run-running)"}
        onclick={run}
      >
        <Icon name={icon} />
      </button>
    {/each}
  </div>
{/if}

<style>
  .widget {
    display: inline-flex;
    flex: none;
    gap: 2px;
    align-items: center;
    margin-right: 8px;
  }

  .button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    padding: 0;
    --icon: var(--run-icon);
    background: var(--button-background);
    border: 0;
    border-radius: 6px;
  }

  .button:hover {
    background: linear-gradient(var(--run-hover), var(--run-hover)), var(--button-background);
  }

  .button:active {
    background: linear-gradient(var(--run-pressed), var(--run-pressed)), var(--button-background);
  }
</style>
