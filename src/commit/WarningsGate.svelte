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
  const BYTES_PER_MB = 1024 * 1024;

  interface Asking {
    warning: CommitWarning;
    action: string;
    answer: (proceed: boolean) => void;
  }

  // `ReturnResult.CLOSE_WINDOW`: the handlers that answer it on Cancel also close the commit window. The CRLF and
  // large file ones answer `CANCEL` and leave it open.
  const CLOSES_THE_WINDOW = new Set<CommitWarning["kind"]>(["detachedHead", "rebasing", "badFileNames"]);

  // `onclosewindow` is for the modal dialog; the non-modal view has no window to close and just stops.
  let { repo, onclosewindow }: { repo: string; onclosewindow?: () => void } = $props();

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
    if (!proceed && asking && CLOSES_THE_WINDOW.has(asking.warning.kind)) onclosewindow?.();
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

  // `GitBundle` `commit.check.warning.title.large.file`: `{0,choice,1#file|2#files}`, and the size cut to whole MB.
  // The numbers are formatted as `MessageFormat` does, with a separator for the thousands.
  function largeFilesText(files: [string, number][]): string {
    const count = files.length;
    const megabytes = Math.floor(files.reduce((sum, [, size]) => sum + size, 0) / BYTES_PER_MB);
    const number = (value: number) => value.toLocaleString("en-US");
    return `You are about to commit a large file into the repository: ${number(count)} ${count === 1 ? "file" : "files"} with the total size of ${number(megabytes)}MB.`;
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
{:else if asking?.warning.kind === "largeFiles"}
  {@const files = asking.warning.paths}
  {@const action = asking.action}
  <!-- `CommitProblem.showModalSolution`: Yes is the commit, No is `Cancel`. -->
  <Dialog
    title="Commit Check Failed"
    width={520}
    onclose={() => finish(false)}
    onsubmit={() => finish(true)}
  >
    <div class="warning">
      <Icon name="warning" size={24} />
      <p>
        {largeFilesText(files)}<br />
        It is not recommended to store large files in a Git repository. Consider using Git LFS.
      </p>
    </div>
    {#snippet footer()}
      <button type="button" class="button" onclick={() => finish(false)}>Cancel</button>
      <button class="button default" data-autofocus>{action}</button>
    {/snippet}
  </Dialog>
{:else if asking?.warning.kind === "badFileNames"}
  <!-- `GitFileNameCommitProblem`: only the first of the problems is told, as IntelliJ does. -->
  {@const first = asking.warning.paths[0]}
  {@const action = asking.action}
  <Dialog
    title="File Name Is Not Allowed on Windows"
    width={520}
    onclose={() => finish(false)}
    onsubmit={() => finish(true)}
  >
    <div class="warning">
      <Icon name="warning" size={24} />
      <p>
        {#if first?.problem === "reserved"}
          Some files use reserved names that are not allowed on Windows: '{first.name}'.<br />
          If you commit, it will not be possible to check out the Git repository on Windows.
        {:else}
          Some of the file names contain characters that are not allowed on Windows: '{first?.name}'.<br />
          If you commit, it might not be possible to check out the Git repository on Windows.
        {/if}
      </p>
    </div>
    {#snippet footer()}
      <button type="button" class="button" onclick={() => finish(false)}>Cancel</button>
      <button class="button default" data-autofocus>{action}</button>
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
