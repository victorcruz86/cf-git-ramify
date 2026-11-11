<script lang="ts">
  import { tick } from "svelte";
  import { projectFocusOpen, projectsList, type ProjectRow, type ProjectsList } from "../lib/api";
  import { errorText } from "../lib/errors";
  import { notify } from "../lib/notifications";
  import { homeRelative, projectName } from "./open";
  import { PROJECT_PALETTE, projectColorIndex, projectInitials } from "./projectIcon";

  // IntelliJ's `ProjectToolbarWidgetAction`: the project's icon and name in the toolbar, and a popup
  // with the widget's actions (`ProjectWidget.Actions`: Open… and Get from Version Control…, which
  // IntelliJ IDEA 2024.1 adds), `Open Projects` and `Recent Projects`.
  interface Props {
    // The repository open in this window; without one the widget is not shown.
    path: string | null;
    // The home folder, for the tooltip.
    home: string | null;
    // A recent project (and an open one whose window is gone): the open flow.
    onopen: (path: string) => Promise<void> | void;
    // A recent project whose folder is gone: IntelliJ's Reopen Project dialog.
    onmissing: (path: string) => void;
    // Open…: the chooser of the toolbar's Open button.
    onchoose: () => void;
    // Get from Version Control…: the Clone dialog.
    onclone: () => void;
  }

  let { path, home, onopen, onmissing, onchoose, onclone }: Props = $props();

  let button = $state<HTMLButtonElement>();
  let box = $state<HTMLDivElement>();
  let shown = $state(false);
  let list = $state.raw<ProjectsList | null>(null);
  let left = $state(0);
  let top = $state(0);
  let token = 0;

  const name = $derived(path ? projectName(path) : "");

  function close() {
    shown = false;
    token++;
    button?.focus();
  }

  async function toggle() {
    if (shown) {
      close();
      return;
    }
    const bounds = button?.getBoundingClientRect();
    left = bounds?.left ?? 0;
    top = bounds?.bottom ?? 0;
    shown = true;
    list = null;
    const mine = ++token;
    try {
      const loaded = await projectsList();
      if (mine !== token) return;
      list = loaded;
    } catch (failure) {
      if (mine !== token) return;
      notify({ kind: "error", title: errorText(failure) });
      list = { open: [], recent: [] };
    }
    await tick();
    items()[0]?.focus();
  }

  const items = () =>
    Array.from(box?.querySelectorAll<HTMLElement>("[role=menuitem]") ?? []);

  async function choose(run: () => void | Promise<void>) {
    shown = false;
    token++;
    try {
      await run();
    } catch (failure) {
      notify({ kind: "error", title: errorText(failure) });
    }
  }

  // `ReopenProjectAction` for a project that is open: its window comes to the front.
  async function focusOpen(row: ProjectRow) {
    if (!(await projectFocusOpen(row.path))) await onopen(row.path);
  }

  function keydown(event: KeyboardEvent) {
    const all = items();
    const at = all.indexOf(document.activeElement as HTMLElement);
    if (event.key === "Escape") close();
    else if (event.key === "ArrowDown") all[(at + 1) % all.length]?.focus();
    else if (event.key === "ArrowUp") all[(at - 1 + all.length) % all.length]?.focus();
    else return;
    event.preventDefault();
  }

  // Escape closes the popup wherever the focus is, also on the widget button it opened from.
  function windowKeydown(event: KeyboardEvent) {
    if (shown && event.key === "Escape" && !event.defaultPrevented) {
      close();
      event.preventDefault();
    }
  }

  function pointerdown(event: PointerEvent) {
    if (shown && !box?.contains(event.target as Node)) close();
  }
</script>

<svelte:window onpointerdown={pointerdown} onkeydown={windowKeydown} />

{#snippet icon(project: string, size: number)}
  {@const [start, end] = PROJECT_PALETTE[projectColorIndex(project)]}
  <span
    class="project-icon"
    aria-hidden="true"
    style:--icon-start={start}
    style:--icon-end={end}
    style:width="{size}px"
    style:height="{size}px"
    style:font-size="{Math.round(size * 0.5)}px"
  >
    {projectInitials(projectName(project))}
  </span>
{/snippet}

{#snippet row(project: ProjectRow, run: () => void | Promise<void>)}
  <button type="button" role="menuitem" class="row project" onclick={() => choose(run)}>
    {@render icon(project.path, 20)}
    <span class="texts">
      <span class="name">{project.name}</span>
      <span class="path">{project.homePath}</span>
    </span>
  </button>
{/snippet}

{#if path}
  <button
    bind:this={button}
    type="button"
    class="widget"
    title={homeRelative(path, home)}
    aria-haspopup="menu"
    aria-expanded={shown}
    onpointerdown={(event) => event.stopPropagation()}
    onclick={toggle}
  >
    {@render icon(path, 18)}
    <span class="widget-name">{name}</span>
  </button>
{/if}

{#if shown}
  <!-- svelte-ignore a11y_interactive_supports_focus -->
  <div
    bind:this={box}
    class="popup"
    role="menu"
    aria-label="Projects"
    style:left="{left}px"
    style:top="{top}px"
    onkeydown={keydown}
  >
    <button type="button" role="menuitem" class="row action" onclick={() => choose(onchoose)}>
      Open…
    </button>
    <button type="button" role="menuitem" class="row action" onclick={() => choose(onclone)}>
      Get from Version Control…
    </button>
    {#if list && list.open.length > 0}
      <div class="section" role="presentation">Open Projects</div>
      {#each list.open as project (project.path)}
        {@render row(project, () => focusOpen(project))}
      {/each}
    {/if}
    {#if list && list.recent.length > 0}
      <div class="section" role="presentation">Recent Projects</div>
      {#each list.recent as project (project.path)}
        {@render row(project, () =>
          project.exists ? onopen(project.path) : onmissing(project.path),
        )}
      {/each}
    {/if}
  </div>
{/if}

<style>
  .widget {
    display: inline-flex;
    gap: 6px;
    align-items: center;
    height: 28px;
    max-width: 320px;
    padding: 0 8px;
    margin-right: 4px;
    font: inherit;
    color: var(--text);
    white-space: nowrap;
    background: none;
    border: 0;
    border-radius: 6px;
  }

  .widget:hover {
    background: var(--hover);
  }

  .widget-name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* `AvatarIcon`: a rounded square (arc ratio 0.4) with a gradient and the initials in white. */
  .project-icon {
    display: inline-flex;
    flex: none;
    align-items: center;
    justify-content: center;
    box-sizing: border-box;
    font-weight: 600;
    line-height: 1;
    color: #fff;
    background: linear-gradient(45deg, var(--icon-end), var(--icon-start));
    border-radius: 40%;
  }

  .popup {
    position: fixed;
    z-index: 15;
    display: flex;
    flex-direction: column;
    box-sizing: border-box;
    min-width: 280px;
    max-width: calc(100vw - 16px);
    max-height: calc(100vh - 56px);
    padding: 6px 0;
    overflow: auto;
    background: var(--popup-background);
    border: 1px solid var(--popup-border);
    border-radius: 8px;
    box-shadow: 0 6px 24px rgb(0 0 0 / 40%);
  }

  .row {
    display: flex;
    gap: 8px;
    align-items: center;
    box-sizing: border-box;
    padding: 0 8px;
    margin: 0 6px;
    font: inherit;
    color: var(--text);
    text-align: left;
    white-space: nowrap;
    background: none;
    border: 0;
    border-radius: 4px;
    outline: none;
  }

  .row:hover,
  .row:focus-visible {
    background: var(--selection);
  }

  .action {
    height: 28px;
    padding-left: 12px;
  }

  .project {
    padding-top: 6px;
    padding-bottom: 6px;
  }

  .texts {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .name,
  .path {
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .path {
    font-size: 11px;
    color: var(--muted);
  }

  .section {
    padding: 8px 14px 4px;
    font-size: 11px;
    color: var(--muted);
    border-top: 1px solid var(--menu-separator);
    margin-top: 6px;
  }
</style>
