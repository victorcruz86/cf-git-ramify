<script lang="ts">
  import Icon from "../lib/Icon.svelte";
  import Menu, { type MenuItem } from "../lib/Menu.svelte";
  import { errorText } from "../lib/errors";
  import { notify } from "../lib/notifications";
  import LogTabs from "../log/LogTabs.svelte";
  import type { RemoteAnswers } from "./branch";
  import Details from "./Details.svelte";
  import List from "./List.svelte";
  import Selector from "./Selector.svelte";
  import {
    changeRepositoryOrAccount,
    checkPullRequestsWindow,
    closePullRequestTab,
    connected,
    LIST_TAB,
    openPullRequestTab,
    pullsStore,
    selectPullsTab,
    singlePair,
  } from "./store";

  // The Pull Requests tool window (P§3.3): the selector until a connection opens, then the list tab,
  // named after the repository, and a closable `#<number>` tab per pull request opened. The gear
  // takes the strip's `.action` style from `controls.css`, which is global.

  // `active`: the window is the one in front; `focused`: the app has the focus. The list polls
  // for updates only while both are true and its tab is selected (overview decision 16).
  // `answers`: the main window's branch dialogs, for the questions of a pull request's checkout.
  let {
    active = false,
    focused = false,
    answers = null,
  }: { active?: boolean; focused?: boolean; answers?: RemoteAnswers | null } = $props();

  // `GB:168`.
  const CHANGE_REPOSITORY_OR_ACCOUNT = "Change Repository or Account…";

  let menu = $state.raw<{ x: number; y: number; items: MenuItem[] } | null>(null);

  const current = $derived($pullsStore);
  const connection = $derived(current.connection);
  const account = $derived(
    (connection && current.known?.accounts.find((candidate) => candidate.id === connection.account)) ?? null,
  );
  // `canResetRemoteOrAccount`: connected, and not the automatic pair.
  const gear = $derived(connection !== null && current.known !== null && singlePair(current.known) === null);
  // `ReviewToolwindowTabsManager`: a PR tab's tooltip is `<projectName>: <description>`, and its
  // description is its name.
  const tabs = $derived(
    connection
      ? [
          { id: LIST_TAB, name: connection.title, closable: false },
          ...current.tabs.map((number) => ({
            id: number,
            name: `#${number}`,
            closable: true,
            title: `${connection.title}: #${number}`,
          })),
        ]
      : [],
  );

  function openOptions(event: MouseEvent) {
    const bounds = (event.currentTarget as HTMLElement).getBoundingClientRect();
    menu = {
      x: bounds.left,
      y: bounds.bottom + 2,
      items: [{ label: CHANGE_REPOSITORY_OR_ACCOUNT, run: () => void change() }],
    };
  }

  async function change() {
    try {
      await changeRepositoryOrAccount();
    } catch (failure) {
      // The backend's message already says what could not be done.
      notify({ kind: "error", title: errorText(failure) });
    }
  }
</script>

<!-- Nothing is built, and nothing connects, until the window first comes to the front. -->
{#if current.shown && connection}
  <div class="window">
    <LogTabs
      {tabs}
      current={current.selected}
      label="Pull Requests tabs"
      closeLabel="Close Tab"
      onselect={selectPullsTab}
      onclose={closePullRequestTab}
    >
      {#snippet trailing()}
        {#if gear}
          <button type="button" class="action options" title="Options" aria-label="Options" onclick={openOptions}>
            <Icon name="settings" />
          </button>
        {/if}
      {/snippet}
    </LogTabs>
    <!-- Every tab keeps its content while another one is in front, as in 241. -->
    <div class="content" class:hidden={current.selected !== LIST_TAB} role="region" aria-label={connection.title}>
      {#key connection}
        <List
          {connection}
          {account}
          active={active && current.selected === LIST_TAB}
          {focused}
          onopen={openPullRequestTab}
        />
      {/key}
    </div>
    {#each current.tabs as number (number)}
      <div class="content" class:hidden={current.selected !== number} role="region" aria-label={`#${number}`}>
        <Details {connection} {account} {number} {answers} />
      </div>
    {/each}
  </div>
{:else if current.shown && current.known}
  <Selector known={current.known} onconnected={connected} onreload={() => checkPullRequestsWindow()} />
{/if}

{#if menu}
  <Menu x={menu.x} y={menu.y} label="Options" items={menu.items} onclose={() => (menu = null)} />
{/if}

<style>
  .window {
    display: flex;
    flex-direction: column;
    height: 100%;
  }

  .content {
    flex: 1;
    min-height: 0;
  }

  .hidden {
    display: none;
  }

  .options {
    margin-left: auto;
  }
</style>
