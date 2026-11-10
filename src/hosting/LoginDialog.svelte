<script lang="ts">
  import {
    githubGenerateUrl,
    githubLogin,
    openUrl,
    taskCancel,
    type AccountView,
    type ApiError,
  } from "../lib/api";
  import Dialog from "../lib/Dialog.svelte";
  import { errorText } from "../lib/errors";
  import { taskResult } from "../lib/tasks";
  import { loginVariant } from "./accounts";

  interface Props {
    server: string | null;
    editing: string | null;
    add: boolean;
    // The account that was logged into, or `null` if the dialog was cancelled.
    // `maybeSaved`: it was cancelled after the login had started, so the backend may have saved it.
    ondone: (account: AccountView | null, maybeSaved?: boolean) => void;
  }

  let { server, editing, add, ondone }: Props = $props();

  // `GHSecurityUtil.DEFAULT_SCOPES`, as `CTB:174` shows them.
  const SCOPES =
    "The following scopes must be granted to the access token: [repo, gist, read:org, workflow, read:user, user:email]";

  const variant = loginVariant({ server, editing, add });
  const focusServer = variant.serverEditable && !variant.server;
  let serverText = $state(variant.server);
  // The token lives here and in the one `githubLogin` call of each Log In; it is emptied once that
  // is over, except when only the server was refused (no server was asked: the user fixes it).
  let token = $state("");
  let busy = $state(false);
  let serverError = $state<string | null>(null);
  let tokenError = $state<string | null>(null);
  let taskId: number | null = null;
  let cancelled = false;

  const generatable = $derived(!busy && serverText.trim() !== "");

  // The backend checks the server, then the token (the order of GithubLoginPanel.doValidateAll),
  // and its error says which field it belongs to. A re-login does not read the server.
  function show(failure: unknown) {
    const message = errorText(failure);
    if ((failure as ApiError | null)?.kind === "login_server") serverError = message;
    else tokenError = message;
  }

  async function logIn() {
    if (busy) return;
    serverError = tokenError = null;
    busy = true;
    cancelled = false;
    taskId = null;
    try {
      const id = await githubLogin(serverText.trim(), token, editing);
      taskId = id;
      if (cancelled) {
        void taskCancel(id).catch(() => {});
        return;
      }
      const account = await taskResult<AccountView>(id);
      token = "";
      if (!cancelled) ondone(account);
    } catch (failure) {
      if (cancelled || (failure as ApiError | null)?.kind !== "login_server") token = "";
      if (!cancelled) show(failure);
    } finally {
      busy = false;
    }
  }

  // Cancel stops the check that is running, if there is one.
  function cancel() {
    cancelled = true;
    token = "";
    if (taskId !== null) void taskCancel(taskId).catch(() => {});
    ondone(null, busy);
  }

  async function generate() {
    serverError = tokenError = null;
    try {
      await openUrl(await githubGenerateUrl(serverText.trim()));
    } catch (failure) {
      show(failure);
    }
  }

  function edited() {
    serverError = tokenError = null;
  }
</script>

<Dialog title={variant.title} width={480} onclose={cancel} onsubmit={logIn}>
  <div class="form">
    <label for="github-login-server">Server:</label>
    <input
      id="github-login-server"
      class="input"
      data-autofocus={focusServer ? "" : undefined}
      spellcheck="false"
      readonly={!variant.serverEditable}
      disabled={busy}
      bind:value={serverText}
      oninput={edited}
    />
    {#if serverError}
      <p class="field-error" role="alert">{serverError}</p>
    {/if}

    <label for="github-login-token">Token:</label>
    <div class="token">
      <input
        id="github-login-token"
        class="input"
        type="password"
        autocomplete="off"
        spellcheck="false"
        data-autofocus={focusServer ? undefined : ""}
        disabled={busy}
        bind:value={token}
        oninput={edited}
      />
      <button type="button" class="button" disabled={!generatable} onclick={generate}>
        Generate…
      </button>
    </div>
    {#if tokenError}
      <p class="field-error" role="alert">{tokenError}</p>
    {/if}
    <p class="comment">{SCOPES}</p>
  </div>

  {#snippet footer()}
    <button type="button" class="button" onclick={cancel}>Cancel</button>
    <button class="button default" disabled={busy}>{variant.ok}</button>
  {/snippet}
</Dialog>

<style>
  .form {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .token {
    display: flex;
    gap: 8px;
  }

  .token .input {
    flex: 1;
    min-width: 0;
  }

  .comment {
    margin: 0;
    color: var(--text-secondary);
  }

  .form .field-error {
    margin: 0;
  }
</style>
