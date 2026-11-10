<script lang="ts">
  import { onMount } from "svelte";
  import type { AccountView } from "../lib/api";
  import { errorText } from "../lib/errors";
  import Icon from "../lib/Icon.svelte";
  import Menu, { type MenuItem } from "../lib/Menu.svelte";
  import { GITHUB_COM, githubAccountsStore, loginFor } from "./accounts";

  let selected = $state<string | null>(null);
  let menu = $state.raw<{ x: number; y: number; items: MenuItem[] } | null>(null);
  let problem = $state<string | null>(null);

  const accounts = $derived($githubAccountsStore.accounts ?? []);
  const loaded = $derived($githubAccountsStore.accounts !== null);
  const failure = $derived($githubAccountsStore.failure);
  const current = $derived(accounts.find((account) => account.id === selected) ?? null);

  // `AccountsPanelFactory`: `+` opens this popup, and so does `Add account…`. OAuth is not offered.
  const addItems: MenuItem[] = [
    { label: "Log In with Token…", run: () => void loginFor(GITHUB_COM, undefined, { add: true }) },
    null,
    { label: "Log In to GitHub Enterprise…", run: () => void loginFor(null, undefined, { add: true }) },
  ];

  function openMenu(target: HTMLElement) {
    const bounds = target.getBoundingClientRect();
    menu = { x: bounds.left, y: bounds.bottom, items: addItems };
  }

  async function act(work: () => Promise<void>) {
    problem = null;
    try {
      await work();
    } catch (failure) {
      problem = errorText(failure);
    }
  }

  const remove = () => current && act(() => githubAccountsStore.remove(current.id));
  const setDefault = () => current && act(() => githubAccountsStore.setDefault(current.id));

  // The `Log in` link: the server stays the account's.
  const logIn = (account: AccountView) => void loginFor(account.server, account.id);

  // The empty list shows `(⌘N)` after `Add account…`.
  function keydown(event: KeyboardEvent) {
    if (event.metaKey && event.key.toLowerCase() === "n" && accounts.length === 0) {
      event.preventDefault();
      const anchor = (event.currentTarget as HTMLElement).querySelector<HTMLElement>(".list");
      if (anchor) openMenu(anchor);
    }
  }

  onMount(() => {
    void githubAccountsStore.load();
  });
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="accounts" onkeydown={keydown}>
  <div class="toolbar" role="toolbar" aria-label="Accounts">
    <button
      type="button"
      class="tool"
      aria-label="Add"
      title="Add"
      aria-haspopup="menu"
      onclick={(event) => openMenu(event.currentTarget)}
    >
      <Icon name="add" />
    </button>
    <button
      type="button"
      class="tool"
      aria-label="Remove"
      title="Remove"
      disabled={!current}
      onclick={remove}
    >
      <Icon name="remove" />
    </button>
    <button type="button" class="button" disabled={!current || current.isDefault} onclick={setDefault}>
      Set as Default
    </button>
  </div>

  <div class="list" role="listbox" aria-label="GitHub accounts" tabindex="0">
    {#if loaded && accounts.length === 0 && !failure}
      <p class="empty">
        No accounts added
        <button type="button" class="link" onclick={(event) => openMenu(event.currentTarget)}>
          Add account…
        </button>
        <span class="shortcut">(⌘N)</span>
      </p>
    {/if}
    {#each accounts as account (account.id)}
      <div
        class="row"
        role="option"
        aria-selected={selected === account.id}
        tabindex="-1"
        data-default={account.isDefault}
        onclick={() => (selected = account.id)}
        onkeydown={(event) => {
          if (event.key === " ") {
            event.preventDefault();
            selected = account.id;
          }
        }}
      >
        <div class="line">
          {#if account.name}
            <span class="name" class:bold={account.isDefault}>{account.name}</span>
            <span class="login">{account.login}</span>
          {:else}
            <span class="name" class:bold={account.isDefault}>{account.login}</span>
          {/if}
          {#if account.error}
            <span class="error">{account.error}</span>
          {/if}
          {#if account.needsLogin}
            <button type="button" class="link" onclick={() => logIn(account)}>Log in</button>
          {/if}
        </div>
        <div class="server">{account.server}</div>
      </div>
    {/each}
  </div>
  {#if failure}
    <p class="field-error" role="alert">{errorText(failure)}</p>
  {/if}
  {#if problem}
    <p class="field-error" role="alert">{problem}</p>
  {/if}
</div>

{#if menu}
  <Menu {...menu} label="Add account" onclose={() => (menu = null)} />
{/if}

<style>
  .accounts {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .toolbar {
    display: flex;
    gap: 6px;
    align-items: center;
  }

  .tool {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    padding: 0;
    color: var(--text);
    background: transparent;
    border: 1px solid transparent;
    border-radius: 6px;
  }

  .tool:hover:not(:disabled) {
    background: var(--hover);
  }

  .tool:disabled {
    color: var(--disabled);
  }

  .list {
    min-height: 96px;
    max-height: 180px;
    overflow-y: auto;
    border: 1px solid var(--control-border);
    border-radius: 6px;
  }

  .empty {
    margin: 0;
    padding: 12px;
    color: var(--text-secondary);
    text-align: center;
  }

  .row {
    padding: 6px 10px;
  }

  .row[aria-selected="true"] {
    background: var(--selection);
  }

  .line {
    display: flex;
    gap: 8px;
    align-items: baseline;
  }

  .name.bold {
    font-weight: 600;
  }

  .login,
  .server {
    color: var(--text-secondary);
  }

  .error {
    color: var(--error);
  }

  .link {
    padding: 0;
    font: inherit;
    color: var(--link, var(--accent));
    background: none;
    border: none;
    cursor: pointer;
  }

  .shortcut {
    color: var(--text-secondary);
  }
</style>
