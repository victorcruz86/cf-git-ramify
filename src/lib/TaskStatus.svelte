<script lang="ts">
  import { onMount } from "svelte";
  import { taskCancel } from "./api";
  import Icon from "./Icon.svelte";
  import { tasks, trackTasks } from "./tasks";

  onMount(() => {
    const stop = trackTasks();
    return () => {
      stop.then((unlisten) => unlisten());
    };
  });
</script>

{#if $tasks.length > 0}
  <footer class="task-status">
    {#each $tasks as task (task.id)}
      <div class="task">
        <span class="title">{task.title}</span>
        {#if task.text}
          <span class="text">{task.text}</span>
        {/if}
        {#if task.fraction !== null}
          <div
            class="progress"
            role="progressbar"
            aria-label={task.title}
            aria-valuemin="0"
            aria-valuemax="100"
            aria-valuenow={Math.round(task.fraction * 100)}
          >
            <div class="bar" style:width="{task.fraction * 100}%"></div>
          </div>
        {/if}
        <button
          type="button"
          class="cancel"
          aria-label="Cancel"
          title="Cancel"
          onclick={() => taskCancel(task.id)}
        >
          <Icon name="close" />
        </button>
      </div>
    {/each}
  </footer>
{/if}

<style>
  .task-status {
    display: flex;
    flex: none;
    gap: 16px;
    align-items: center;
    height: 28px;
    box-sizing: border-box;
    padding: 0 8px 0 12px;
    background: var(--panel);
    border-top: 1px solid var(--border);
  }

  .task {
    display: flex;
    gap: 8px;
    align-items: center;
    min-width: 0;
  }

  .title {
    white-space: nowrap;
  }

  .text {
    min-width: 0;
    overflow: hidden;
    color: var(--muted);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .progress {
    flex: none;
    width: 120px;
    height: 4px;
    overflow: hidden;
    background: var(--divider);
    border-radius: 2px;
  }

  .bar {
    height: 100%;
    background: var(--accent);
  }

  .cancel {
    display: inline-flex;
    flex: none;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    padding: 0;
    background: none;
    border: 0;
    border-radius: 4px;
  }

  .cancel:hover {
    background: var(--hover);
  }
</style>
