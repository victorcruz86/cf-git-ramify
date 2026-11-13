<script lang="ts">
  import { onMount, tick } from "svelte";
  import Dialog from "../lib/Dialog.svelte";
  import Icon from "../lib/Icon.svelte";
  import Menu, { type MenuItem } from "../lib/Menu.svelte";
  import { onTerminalExited, terminalBusy, terminalReset } from "../lib/api";
  import TerminalPane from "./TerminalPane.svelte";
  import {
    dividers,
    focusAfterClosing,
    nextTitle,
    panes,
    ratioAt,
    rects,
    remove,
    setRatio,
    split,
    type Direction,
    type Divider,
    type Tab,
  } from "./layout";

  let { active }: { active: boolean } = $props();

  let tabs = $state.raw<Tab[]>([]);
  let current = $state<number | null>(null);
  let started = $state(false);
  let menu = $state.raw<{ x: number; y: number; items: MenuItem[] } | null>(null);
  let confirm = $state.raw<{ run: () => void } | null>(null);
  let renaming = $state<number | null>(null);
  let renameText = $state("");
  const areas: Record<number, HTMLDivElement> = {};
  // The shell session of each pane, once it has started.
  const sessions = new Map<number, number>();
  let nextId = 1;

  function newTab() {
    const pane = nextId++;
    const tab: Tab = {
      id: nextId++,
      title: nextTitle(tabs.map((each) => each.title)),
      layout: { kind: "pane", id: pane },
      focused: pane,
    };
    tabs = [...tabs, tab];
    current = tab.id;
  }

  // The sessions start the first time the tab is shown, after the ones a previous page left
  // behind have ended.
  $effect(() => {
    if (!active || started) return;
    started = true;
    void terminalReset()
      .catch(() => {})
      .then(() => {
        if (tabs.length === 0) newTab();
      });
  });

  // `exit` in a shell closes its pane, as in IntelliJ.
  onMount(() => {
    const stop = onTerminalExited((session) => {
      for (const [pane, each] of sessions) if (each === session) dropPane(pane);
    });
    return () => {
      void stop.then((unlisten) => unlisten());
    };
  });

  function changeTab(id: number, change: (tab: Tab) => Tab | null) {
    tabs = tabs.flatMap((tab) => {
      if (tab.id !== id) return [tab];
      const next = change(tab);
      return next ? [next] : [];
    });
    if (!tabs.some((tab) => tab.id === current)) current = tabs.at(-1)?.id ?? null;
  }

  function dropPane(pane: number) {
    sessions.delete(pane);
    const tab = tabs.find((each) => panes(each.layout).includes(pane));
    if (!tab) return;
    changeTab(tab.id, (each) => {
      const layout = remove(each.layout, pane);
      if (!layout) return null;
      const focused = each.focused === pane ? (focusAfterClosing(each.layout, pane) ?? pane) : each.focused;
      return { ...each, layout, focused };
    });
  }

  function splitPane(tabId: number, pane: number, direction: Direction) {
    const added = nextId++;
    const splitId = nextId++;
    changeTab(tabId, (tab) => ({
      ...tab,
      layout: split(tab.layout, pane, direction, added, splitId),
      focused: added,
    }));
  }

  async function busy(paneIds: number[]): Promise<boolean> {
    const answers = await Promise.all(
      paneIds.map((pane) => {
        const session = sessions.get(pane);
        return session === undefined ? false : terminalBusy(session).catch(() => false);
      }),
    );
    return answers.some(Boolean);
  }

  // A running program is not ended without asking.
  async function closeWithConfirmation(paneIds: number[], run: () => void) {
    if (await busy(paneIds)) confirm = { run };
    else run();
  }

  function closePane(pane: number) {
    void closeWithConfirmation([pane], () => dropPane(pane));
  }

  function closeTab(tabId: number) {
    const tab = tabs.find((each) => each.id === tabId);
    if (!tab) return;
    const ids = panes(tab.layout);
    void closeWithConfirmation(ids, () => {
      ids.forEach((id) => sessions.delete(id));
      changeTab(tabId, () => null);
    });
  }

  async function startRename(tab: Tab) {
    renaming = tab.id;
    renameText = tab.title;
    await tick();
    const input = document.querySelector<HTMLInputElement>(".rename");
    input?.select();
  }

  function finishRename(save: boolean) {
    const id = renaming;
    renaming = null;
    const title = renameText.trim();
    if (save && id !== null && title) changeTab(id, (tab) => ({ ...tab, title }));
  }

  function openMenu(event: MouseEvent, tab: Tab, pane = tab.focused) {
    event.preventDefault();
    current = tab.id;
    menu = {
      x: event.clientX,
      y: event.clientY,
      items: [
        { label: "New Tab", run: newTab },
        { label: "Rename Session…", run: () => void startRename(tab) },
        null,
        { label: "Split Right", run: () => splitPane(tab.id, pane, "right") },
        { label: "Split Down", run: () => splitPane(tab.id, pane, "down") },
        null,
        { label: "Close Session", run: () => closePane(pane) },
        { label: "Close Tab", run: () => closeTab(tab.id) },
      ],
    };
  }

  function drag(event: PointerEvent, tabId: number, divider: Divider) {
    const handle = event.currentTarget as HTMLElement;
    const area = areas[tabId]?.getBoundingClientRect();
    if (!area || area.width <= 0 || area.height <= 0) return;
    handle.setPointerCapture(event.pointerId);
    const move = (next: PointerEvent) => {
      const position =
        divider.direction === "right"
          ? (next.clientX - area.left) / area.width
          : (next.clientY - area.top) / area.height;
      changeTab(tabId, (tab) => ({ ...tab, layout: setRatio(tab.layout, divider.split, ratioAt(divider, position)) }));
    };
    const end = () => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", end);
      handle.removeEventListener("pointercancel", end);
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", end);
    handle.addEventListener("pointercancel", end);
  }

  const percent = (fraction: number) => `${fraction * 100}%`;
</script>

<section class="terminal" aria-label="Terminal">
  <div class="strip" role="tablist" aria-label="Terminal sessions">
    {#each tabs as tab (tab.id)}
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <div
        class="tab"
        class:selected={tab.id === current}
        role="tab"
        tabindex="-1"
        aria-selected={tab.id === current}
        onclick={() => (current = tab.id)}
        ondblclick={() => void startRename(tab)}
        oncontextmenu={(event) => openMenu(event, tab)}
      >
        {#if renaming === tab.id}
          <input
            class="rename"
            aria-label="Session name"
            bind:value={renameText}
            onkeydown={(event) => {
              event.stopPropagation();
              if (event.key === "Enter") finishRename(true);
              else if (event.key === "Escape") finishRename(false);
            }}
            onblur={() => finishRename(true)}
          />
        {:else}
          <span>{tab.title}</span>
        {/if}
        <button
          type="button"
          class="close"
          aria-label="Close Tab"
          title="Close Tab"
          onclick={(event) => {
            event.stopPropagation();
            closeTab(tab.id);
          }}
        >
          <Icon name="close" size={12} />
        </button>
      </div>
    {/each}
    <button type="button" class="action" aria-label="New Tab" title="New Tab" onclick={newTab}>
      <Icon name="add" />
    </button>
  </div>

  <div class="body">
    {#each tabs as tab (tab.id)}
      <div class="area" class:hidden={tab.id !== current} bind:this={areas[tab.id]}>
        {#each rects(tab.layout) as { id, rect } (id)}
          <div
            class="slot"
            style:left={percent(rect.x)}
            style:top={percent(rect.y)}
            style:width={percent(rect.width)}
            style:height={percent(rect.height)}
          >
            <TerminalPane
              focused={tab.focused === id}
              visible={active && tab.id === current}
              outlined={panes(tab.layout).length > 1}
              onfocus={() => {
                if (tab.focused !== id) changeTab(tab.id, (each) => ({ ...each, focused: id }));
              }}
              onsession={(session) => sessions.set(id, session)}
              oncontextmenu={(event) => openMenu(event, tab, id)}
            />
          </div>
        {/each}
        {#each dividers(tab.layout) as divider (divider.split)}
          <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
          <div
            class="divider"
            class:down={divider.direction === "down"}
            role="separator"
            aria-orientation={divider.direction === "right" ? "vertical" : "horizontal"}
            style:left={divider.direction === "right" ? percent(divider.at) : percent(divider.area.x)}
            style:top={divider.direction === "down" ? percent(divider.at) : percent(divider.area.y)}
            style:width={divider.direction === "down" ? percent(divider.area.width) : null}
            style:height={divider.direction === "right" ? percent(divider.area.height) : null}
            onpointerdown={(event) => drag(event, tab.id, divider)}
          ></div>
        {/each}
      </div>
    {/each}
    {#if started && tabs.length === 0}
      <div class="empty">
        <button type="button" class="button" onclick={newTab}>New Tab</button>
      </div>
    {/if}
  </div>
</section>

{#if menu}
  <Menu {...menu} label="Terminal" onclose={() => (menu = null)} />
{/if}

{#if confirm}
  <Dialog
    title="Terminate Process"
    width={400}
    onclose={() => (confirm = null)}
    onsubmit={() => {
      const run = confirm?.run;
      confirm = null;
      run?.();
    }}
  >
    <p>A program is running in this session. Closing it terminates the program.</p>
    {#snippet footer()}
      <button type="button" class="button" onclick={() => (confirm = null)}>Cancel</button>
      <button class="button default" data-autofocus>Terminate</button>
    {/snippet}
  </Dialog>
{/if}

<style>
  .terminal {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-height: 0;
    background: var(--editor);
  }

  .strip {
    display: flex;
    flex: none;
    align-items: center;
    gap: 2px;
    padding: 3px 6px;
    background: var(--panel);
    border-bottom: 1px solid var(--divider);
  }

  .tab {
    display: flex;
    align-items: center;
    gap: 4px;
    height: 24px;
    padding: 0 4px 0 10px;
    border-radius: 4px;
    color: var(--text-secondary);
    cursor: default;
  }

  .tab:hover {
    background: var(--hover);
  }

  .tab.selected {
    color: var(--text);
    background: var(--selection);
  }

  .rename {
    width: 110px;
    height: 18px;
    font: inherit;
  }

  .close,
  .action {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    padding: 2px;
    color: var(--icon);
    background: none;
    border: 0;
    border-radius: 4px;
  }

  .close:hover,
  .action:hover {
    background: var(--hover);
  }

  .body {
    position: relative;
    flex: 1;
    min-height: 0;
  }

  .area {
    position: absolute;
    inset: 0;
  }

  .area.hidden {
    display: none;
  }

  .slot {
    position: absolute;
  }

  .divider {
    position: absolute;
    z-index: 2;
    width: 5px;
    margin-left: -2px;
    cursor: col-resize;
    background: linear-gradient(90deg, transparent 2px, var(--divider) 2px, var(--divider) 3px, transparent 3px);
  }

  .divider.down {
    width: auto;
    height: 5px;
    margin: -2px 0 0;
    cursor: row-resize;
    background: linear-gradient(180deg, transparent 2px, var(--divider) 2px, var(--divider) 3px, transparent 3px);
  }

  .empty {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 100%;
  }
</style>
