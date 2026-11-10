<script lang="ts">
  import { tick, untrack } from "svelte";
  import {
    githubConnect,
    type AccountView,
    type ApiError,
    type Connection,
    type Known,
    type MappingView,
  } from "../lib/api";
  import { errorText } from "../lib/errors";
  import Icon from "../lib/Icon.svelte";
  import Menu, { type MenuItem } from "../lib/Menu.svelte";
  import { loginFor } from "../hosting/accounts";
  import { taskResult } from "../lib/tasks";
  import {
    accountsOn,
    forgetTokenless,
    isGithubCom,
    markTokenless,
    pullsGeneration,
    pullsStore,
    singlePair,
  } from "./store";

  // `GHRepositoryAndAccountSelectorComponentFactory` (P§3.5): the repository combo, the account
  // selector, `View Pull Requests` or the login buttons, the error and the footer note.
  interface Props {
    known: Known;
    // `asked` is the store's generation when the connect started.
    onconnected: (connection: Connection, asked: number) => void;
    // Reads the repositories and accounts again, after a login.
    onreload: () => Promise<void>;
  }

  let { known, onconnected, onreload }: Props = $props();

  // `GB:167`, `GB:14`, `GB:15`, `CTB:6`, `CTB:46`, `CTB:47`, `CTB:8`, `CTB:25`, `CTB:53`.
  const VIEW = "View Pull Requests";
  const LOG_IN_WITH_TOKEN = "Log In with Token…";
  const LOG_IN_TO_ENTERPRISE = "Log In to GitHub Enterprise…";
  const CHOOSE_ACCOUNT = "Choose an account";
  const CONNECTION_FAILED = "Could not connect to repository";
  const connectionFailed = (repository: string, account: string) =>
    `Could not connect to repository ${repository} with account ${account}`;
  const MISSING_TOKEN = "Missing access token";
  const LOG_IN_AGAIN = "Log in again…";
  const NOTE = "Change repository or account later under the gear icon";

  interface SubmissionError {
    repository: string;
    account: AccountView;
    message: string;
  }

  let remote = $state<string | null>(null);
  let accountId = $state<string | null>(null);
  let submitting = $state(false);
  let logging = $state(false);
  let submission = $state.raw<SubmissionError | null>(null);
  // The accounts a connect found without a token (`MissingCredentials`), kept by the window's
  // store so that the selector shown again still knows.
  const tokenless = $derived($pullsStore.tokenless);
  // The automatic choice last acted on, so that the same one is not submitted twice.
  let lastAutomatic: string | null = null;
  // How many connects have started: a login whose new pair already connected by itself does not
  // connect a second time.
  let submits = 0;
  let menu = $state.raw<{ x: number; y: number; label: string; items: MenuItem[] } | null>(null);

  const mapping = $derived(known.mappings.find((candidate) => candidate.remote === remote) ?? null);
  const here = $derived(mapping ? accountsOn(known, mapping) : []);
  const account = $derived(here.find((candidate) => candidate.id === accountId) ?? null);
  const missing = $derived(account !== null && tokenless.has(account.id));
  const busy = $derived(submitting || logging);
  const submitAvailable = $derived(mapping !== null && account !== null && !missing);
  const loginAvailable = $derived(mapping !== null && (account === null || missing));
  const dotCom = $derived(mapping !== null && isGithubCom(mapping.server));
  const loginLabel = $derived(dotCom ? LOG_IN_WITH_TOKEN : LOG_IN_TO_ENTERPRISE);

  // `account.server/login`: 241's `GithubAccount.toString`.
  const accountText = (row: AccountView) => `${row.server}/${row.login}`;

  function select(next: MappingView | null, nextAccount: AccountView | null) {
    remote = next?.remote ?? null;
    accountId = nextAccount?.id ?? null;
  }

  // The combo pre-selects its first item, and the account selector the first account of that server.
  function preselect() {
    const first = known.mappings[0] ?? null;
    select(first, first ? (accountsOn(known, first)[0] ?? null) : null);
  }

  // The saved choice, if its remote and account are still there, else the automatic pair.
  function automatic(value: Known): { mapping: MappingView; account: AccountView } | null {
    const saved = value.saved;
    const savedMapping = saved && value.mappings.find((candidate) => candidate.url === saved.url);
    const savedAccount = saved && value.accounts.find((candidate) => candidate.id === saved.account);
    if (savedMapping && savedAccount) return { mapping: savedMapping, account: savedAccount };
    return singlePair(value);
  }

  // As 241's collectors, on mount and whenever the repositories or the accounts change: a new
  // saved choice or automatic pair is chosen and submitted, unless its account has no token. Else a
  // repository or account that is gone leaves the selection for the first one again, and the user
  // picks.
  $effect(() => {
    const value = known;
    untrack(() => {
      const chosen = automatic(value);
      const key = chosen && `${chosen.mapping.remote}\n${chosen.account.id}`;
      const fresh = chosen !== null && key !== lastAutomatic;
      lastAutomatic = key;
      if (fresh && chosen) {
        select(chosen.mapping, chosen.account);
        if (!tokenless.has(chosen.account.id)) void submit();
      } else if (!mapping) preselect();
      else if (accountId !== null && !account) accountId = here[0]?.id ?? null;
    });
  });

  async function submit() {
    const chosenMapping = mapping;
    const chosenAccount = account;
    if (!chosenMapping || !chosenAccount || submitting) return;
    const asked = pullsGeneration();
    submits++;
    submitting = true;
    try {
      const connection = await taskResult<Connection>(
        await githubConnect(chosenMapping.remote, chosenAccount.id),
      );
      // Another repository opened while it was asked: the answer is not this window's.
      if (asked !== pullsGeneration()) return;
      submission = null;
      onconnected(connection, asked);
    } catch (failure) {
      if (asked !== pullsGeneration()) return;
      const kind = (failure as Partial<ApiError> | null)?.kind;
      // A cancelled connect is not an error (241 rethrows the CancellationException).
      if (kind === "cancelled") return;
      if (kind === "missing_token") {
        markTokenless(chosenAccount.id);
        submission = null;
        return;
      }
      submission = {
        repository: chosenMapping.repository,
        account: chosenAccount,
        message: errorText(failure),
      };
    } finally {
      submitting = false;
    }
  }

  // A login, then the lists read again with the account it gave chosen.
  async function logIn(login: () => Promise<AccountView | null>): Promise<boolean> {
    const asked = pullsGeneration();
    logging = true;
    try {
      const result = await login();
      if (!result || asked !== pullsGeneration()) return false;
      await onreload();
      await tick();
      if (asked !== pullsGeneration()) return false;
      forgetTokenless(result.id);
      accountId = result.id;
      return true;
    } finally {
      logging = false;
    }
  }

  // `Log In with Token…` or `Log In to GitHub Enterprise…` under the combo: a new account, or the
  // chosen one again when it has no token, then the connect.
  async function logInAndSubmit() {
    const target = mapping;
    if (!target) return;
    const relogin = account && missing ? account : null;
    const before = submits;
    const done = await logIn(() =>
      relogin ? loginFor(relogin.server, relogin.id) : loginFor(target.server),
    );
    if (done && submits === before) void submit();
  }

  // `Log in again…`: the failed account logs in again, and the choice is submitted again.
  async function logInAgain() {
    const failed = submission?.account;
    if (!failed) return;
    const before = submits;
    const done = await logIn(() => loginFor(failed.server, failed.id));
    if (done && submits === before) void submit();
  }

  function popup(event: MouseEvent, label: string, items: MenuItem[]) {
    const bounds = (event.currentTarget as HTMLElement).getBoundingClientRect();
    menu = { x: bounds.left, y: bounds.bottom + 2, label, items };
  }

  function chooseRepository(event: MouseEvent) {
    popup(
      event,
      "Repository",
      known.mappings.map((candidate) => ({
        label: candidate.display,
        shortcut: candidate.remote,
        checked: candidate.remote === remote,
        run: () => {
          const keep = here.find((row) => row.id === accountId && accountsOn(known, candidate).includes(row));
          select(candidate, keep ?? accountsOn(known, candidate)[0] ?? null);
        },
      })),
    );
  }

  // The accounts of the repository's server, by login, then the login that always adds one
  // (`forceNew`). Ramify has no OAuth (decision 14): github.com offers only the token.
  function chooseAccount(event: MouseEvent) {
    const target = mapping;
    if (!target) return;
    popup(event, "Account", [
      ...here.map((row) => ({
        label: row.login,
        checked: row.id === accountId,
        run: () => (accountId = row.id),
      })),
      ...(here.length > 0 ? [null] : []),
      { label: loginLabel, run: () => void logIn(() => loginFor(target.server)) },
    ]);
  }

  const shownError = $derived(
    missing
      ? { title: CONNECTION_FAILED, description: MISSING_TOKEN, action: false }
      : submission
        ? {
            title: connectionFailed(submission.repository, accountText(submission.account)),
            description: submission.message,
            action: true,
          }
        : null,
  );
</script>

<div class="selector">
  <div class="row">
    <button
      type="button"
      class="button combo"
      aria-label="Repository"
      disabled={busy}
      onclick={chooseRepository}
    >
      {#if mapping}
        <span class="name">{mapping.display}</span>
        <span class="secondary">{mapping.remote}</span>
      {/if}
      <Icon name="chevronDown" size={12} />
    </button>
    <button
      type="button"
      class="button account"
      aria-label="Account"
      title={account ? undefined : CHOOSE_ACCOUNT}
      disabled={busy || !mapping}
      onclick={chooseAccount}
    >
      {account?.login ?? ""}
      <Icon name="chevronDown" size={12} />
    </button>
  </div>

  <div class="row actions">
    {#if submitAvailable}
      <button type="button" class="button default" disabled={busy} onclick={() => void submit()}>{VIEW}</button>
    {/if}
    {#if loginAvailable}
      <button type="button" class="button default" disabled={busy} onclick={() => void logInAndSubmit()}>
        {loginLabel}
      </button>
    {/if}
    {#if busy}
      <span class="busy" role="status" aria-label="Loading"></span>
    {/if}
  </div>

  {#if shownError}
    <div class="error" role="alert">
      <Icon name="error" />
      <p>
        {shownError.title}<br />
        {shownError.description}
        {#if shownError.action}
          <button type="button" class="link" disabled={busy} onclick={() => void logInAgain()}>
            {LOG_IN_AGAIN}
          </button>
        {/if}
      </p>
    </div>
  {/if}

  <p class="note">{NOTE}</p>
</div>

{#if menu}
  <Menu x={menu.x} y={menu.y} label={menu.label} items={menu.items} onclose={() => (menu = null)} />
{/if}

<style>
  .selector {
    display: flex;
    flex-direction: column;
    gap: 16px;
    padding: 30px 16px;
  }

  .row {
    display: flex;
    gap: 10px;
    align-items: center;
  }

  .actions {
    gap: 16px;
  }

  .combo {
    display: flex;
    flex: 1;
    gap: 6px;
    align-items: center;
    min-width: 0;
    text-align: left;
  }

  .combo .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .combo :global(.icon),
  .account :global(.icon) {
    margin-left: auto;
  }

  .secondary {
    color: var(--text-secondary);
  }

  .account {
    display: flex;
    gap: 6px;
    align-items: center;
    min-width: 80px;
  }

  .error {
    display: flex;
    gap: 6px;
    align-items: flex-start;
  }

  .error p {
    margin: 0;
    white-space: pre-wrap;
  }

  .link {
    padding: 0;
    font: inherit;
    color: var(--link);
    cursor: pointer;
    background: none;
    border: 0;
  }

  .note {
    margin: 0;
    color: var(--text-secondary);
  }

  .busy {
    width: 14px;
    height: 14px;
    border: 2px solid var(--divider);
    border-top-color: var(--focus);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
