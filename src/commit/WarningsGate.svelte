<script lang="ts">
  import {
    commitFixAutocrlf,
    settingsGet,
    settingsSave,
    type CommitWarning,
    type Settings,
  } from "../lib/api";
  import Dialog from "../lib/Dialog.svelte";
  import Icon from "../lib/Icon.svelte";

  const AUTOCRLF_KEY = "core.autocrlf";
  const AUTOCRLF_VALUE = "input";
  const SUGGESTED_FIX = `git config --global ${AUTOCRLF_KEY} ${AUTOCRLF_VALUE}`;

  interface Asking {
    warning: CommitWarning;
    action: string;
    answer: (proceed: boolean) => void;
  }

  let { repo }: { repo: string } = $props();

  let asking = $state.raw<Asking | null>(null);
  let dontWarn = $state(false);

  function ask(warning: CommitWarning, action: string): Promise<boolean> {
    dontWarn = false;
    return new Promise((answer) => (asking = { warning, action, answer }));
  }

  // The dialogs of `GitCheckinHandlerFactory`, one by one. `action` is the text of the commit button.
  export async function confirm(warnings: CommitWarning[], action: string): Promise<boolean> {
    for (const warning of warnings) {
      if (!(await ask(warning, action))) return false;
    }
    return true;
  }

  function finish(proceed: boolean) {
    const answer = asking?.answer;
    asking = null;
    answer?.(proceed);
  }

  // As in IntelliJ, failing to remember it or to change the setting does not prevent the commit.
  async function silence(change: Partial<Settings>) {
    if (!dontWarn) return;
    try {
      await settingsSave({ ...(await settingsGet()), ...change });
    } catch {
      // The warning will show up again.
    }
  }

  async function fixAndCommit() {
    await commitFixAutocrlf().catch(() => {});
    finish(true);
  }

  async function commitAsIs() {
    await silence({ warnCrlf: false });
    finish(true);
  }

  async function proceed() {
    await silence({ warnDetached: false });
    finish(true);
  }
</script>

{#snippet dontWarnAgain()}
  <label class="checkbox-label">
    <input type="checkbox" bind:checked={dontWarn} />
    <span class="check" data-state={dontWarn ? "on" : "off"}></span>
    Don't warn again
  </label>
{/snippet}

{#if asking?.warning.kind === "crlf"}
  {@const paths = asking.warning.paths}
  <Dialog title="Line Separators Warning" width={560} onclose={() => finish(false)} onsubmit={fixAndCommit}>
    <div class="warning">
      <Icon name="warning" size={24} />
      <div>
        <p>
          You are about to commit CRLF line separators to the Git repository.<br />
          It is recommended to set the <code>{AUTOCRLF_KEY}</code> Git attribute to
          <code>{AUTOCRLF_VALUE}</code> to avoid line separator issues.
        </p>
        <p>
          If you choose <b>Fix and Commit</b>, <code>{SUGGESTED_FIX}</code> will be executed.<br />
          If you choose <b>Commit As Is</b>, the config value won't be changed.
        </p>
        <ul class="files">
          {#each paths as path (path)}
            <li>{path}</li>
          {/each}
        </ul>
        {@render dontWarnAgain()}
      </div>
    </div>
    {#snippet footer()}
      <button class="button">Fix and Commit</button>
      <button type="button" class="button" onclick={commitAsIs}>Commit As Is</button>
      <button type="button" class="button default" data-autofocus onclick={() => finish(false)}>
        Cancel
      </button>
    {/snippet}
  </Dialog>
{:else if asking}
  {@const rebasing = asking.warning.kind === "rebasing"}
  {@const action = asking.action}
  <Dialog
    title={rebasing ? "Unfinished Rebase Process" : "Commit in Detached HEAD"}
    width={520}
    onclose={() => finish(false)}
    onsubmit={proceed}
  >
    <div class="warning">
      <Icon name="warning" size={24} />
      <div>
        {#if rebasing}
          <p>
            The Git repository at the following path has an <b>unfinished rebase</b> process:<br />
            <b>{repo}</b><br />
            You probably want to <b>continue rebase</b> instead of committing.<br />
            Committing during rebase may lead to the commit loss.
          </p>
        {:else}
          <p>
            The Git repository at the following path is in the <b>detached HEAD</b> state:<br />
            <b>{repo}</b><br />
            You can look around, make experimental changes and commit them, but be sure to checkout a
            branch not to lose your work.<br />
            Otherwise you risk losing your changes.
          </p>
        {/if}
        {@render dontWarnAgain()}
      </div>
    </div>
    {#snippet footer()}
      <button type="button" class="button" onclick={() => finish(false)}>Cancel</button>
      <button class="button default" data-autofocus>{action}</button>
    {/snippet}
  </Dialog>
{/if}

<style>
  .warning {
    display: flex;
    gap: 12px;
    align-items: flex-start;
  }

  .warning > div {
    min-width: 0;
  }

  p {
    margin: 0 0 8px;
    overflow-wrap: anywhere;
  }

  code {
    font: 12px ui-monospace, Menlo, monospace;
  }

  .files {
    max-height: 96px;
    padding: 0;
    margin: 0 0 8px;
    overflow: auto;
    color: var(--text-secondary);
    list-style: none;
  }
</style>
