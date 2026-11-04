<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { protectedBranches } from "../branches/state";
  import {
    gitVersion,
    protectedBranchesSet,
    repoSettingsGet,
    settingsSave,
    type ApiError,
    type Settings,
  } from "../lib/api";
  import Dialog from "../lib/Dialog.svelte";
  import { errorText } from "../lib/errors";
  import { notify } from "../lib/notifications";

  interface Props {
    settings: Settings;
    onclose: () => void;
  }

  let { settings, onclose }: Props = $props();

  let draft = $state({ ...untrack(() => settings) });
  // One path per line, like the log's path filter.
  let updatePaths = $state(untrack(() => settings.updatePaths.join("\n")));
  // Of the open repository, separated by `;` as in `ParametersListUtil.COLON_LINE_PARSER`. `null`:
  // there is no repository, or they could not be read, and then they are left untouched.
  let protectedText = $state<string | null>(null);
  let tested = $state<{ ok: boolean; text: string } | null>(null);
  let problem = $state<string | null>(null);
  let saving = $state(false);
  let testRun = 0;

  const signature = $derived(
    draft.signCommits
      ? `Commits are signed with GPG key: ${draft.signingKey || "default"}`
      : "Commits signing with GPG key is not configured",
  );

  function failureText(path: string | null, failure: unknown): string {
    const notFound = (failure as ApiError).kind === "git_not_found";
    return notFound && path ? `No such file: ${path}` : errorText(failure);
  }

  async function testPath() {
    const run = ++testRun;
    const path = draft.gitPath?.trim() || null;
    try {
      const version = await gitVersion(path);
      if (run === testRun) tested = { ok: true, text: `Git version is ${version}` };
    } catch (failure) {
      if (run === testRun) tested = { ok: false, text: failureText(path, failure) };
    }
  }

  function pathEdited() {
    testRun++;
    tested = null;
  }

  const patterns = (text: string) =>
    text
      .split(";")
      .map((pattern) => pattern.trim())
      .filter(Boolean);

  async function loadProtected() {
    try {
      const { protectedBranches: stored } = await repoSettingsGet();
      protectedText = stored.join(";");
    } catch {
      protectedText = null;
    }
  }

  async function saveProtected() {
    if (protectedText === null) return;
    const list = patterns(protectedText);
    const invalid = await protectedBranchesSet(list);
    protectedBranches.set(list);
    if (invalid.length > 0) {
      notify({
        kind: "warning",
        title: "Invalid protected branch pattern",
        body: invalid.join("\n"),
      });
    }
  }

  async function save() {
    if (saving) return;
    saving = true;
    problem = null;
    try {
      await saveProtected();
      const paths = updatePaths
        .split("\n")
        .map((path) => path.trim())
        .filter((path, index, all) => path && all.indexOf(path) === index);
      await settingsSave({ ...$state.snapshot(draft), updatePaths: paths });
      onclose();
    } catch (failure) {
      problem = errorText(failure);
    } finally {
      saving = false;
    }
  }

  onMount(() => {
    testPath();
    loadProtected();
  });
</script>

<Dialog title="Settings" width={600} {onclose} onsubmit={save}>
  <h3>Git</h3>
  <div class="path">
    <label for="settings-git-path">Path to Git executable:</label>
    <input
      id="settings-git-path"
      class="input"
      data-autofocus
      placeholder="Auto-detected"
      spellcheck="false"
      bind:value={draft.gitPath}
      oninput={pathEdited}
    />
    <button type="button" class="button" onclick={testPath}>Test</button>
  </div>
  {#if tested?.ok}
    <p class="comment">{tested.text}</p>
  {:else if tested}
    <p class="field-error" role="alert">{tested.text}</p>
  {/if}
  <div class="options helper">
    <label class="checkbox-label">
      <input type="checkbox" bind:checked={draft.useCredentialHelper} />
      <span class="check" data-state={draft.useCredentialHelper ? "on" : "off"}></span>
      Use credential helper
    </label>
    <div class="incoming">
      <label for="settings-incoming-check">Explicitly check for incoming commits on remotes:</label>
      <select id="settings-incoming-check" class="input" bind:value={draft.incomingCheck}>
        <option value="auto">Auto</option>
        <option value="always">Always</option>
        <option value="never">Never</option>
      </select>
    </div>
  </div>

  <h3>Commit</h3>
  <div class="options">
    <label class="checkbox-label">
      <input type="checkbox" bind:checked={draft.warnCrlf} />
      <span class="check" data-state={draft.warnCrlf ? "on" : "off"}></span>
      Warn if CRLF line separators are about to be committed
    </label>
    <label class="checkbox-label">
      <input type="checkbox" bind:checked={draft.warnDetached} />
      <span class="check" data-state={draft.warnDetached ? "on" : "off"}></span>
      Warn when committing in detached HEAD or during rebase
    </label>
    <label class="checkbox-label">
      <input type="checkbox" bind:checked={draft.cherryPickSuffix} />
      <span class="check" data-state={draft.cherryPickSuffix ? "on" : "off"}></span>
      Add the 'cherry-picked from &lt;hash&gt;' suffix when picking commits pushed to protected branches
    </label>
    <label class="checkbox-label">
      <input type="checkbox" bind:checked={draft.stagingArea} />
      <span class="check" data-state={draft.stagingArea ? "on" : "off"}></span>
      Enable staging area
    </label>
    <p class="comment nested">
      This will disable changelists support. Only for non-modal commit interface.
    </p>
    <p class="signature">{signature}</p>
    <div class="key">
      <label class="checkbox-label">
        <input type="checkbox" bind:checked={draft.signCommits} />
        <span class="check" data-state={draft.signCommits ? "on" : "off"}></span>
        Sign commits with GPG key:
      </label>
      {#if draft.signCommits}
        <input
          class="input"
          aria-label="GPG key"
          placeholder="default"
          spellcheck="false"
          bind:value={draft.signingKey}
        />
      {/if}
    </div>
  </div>

  <h3>Push</h3>
  <div class="options">
    <label class="checkbox-label">
      <input type="checkbox" bind:checked={draft.pushAutoUpdate} />
      <span class="check" data-state={draft.pushAutoUpdate ? "on" : "off"}></span>
      Auto-update if push of the current branch was rejected
    </label>
    <label class="checkbox-label">
      <input type="checkbox" bind:checked={draft.previewPushOnCommit} />
      <span class="check" data-state={draft.previewPushOnCommit ? "on" : "off"}></span>
      Show Push dialog for Commit and Push
    </label>
    <label class="checkbox-label nested" class:disabled={!draft.previewPushOnCommit}>
      <input
        type="checkbox"
        disabled={!draft.previewPushOnCommit}
        bind:checked={draft.previewPushProtectedOnly}
      />
      <span class="check" data-state={draft.previewPushProtectedOnly ? "on" : "off"}></span>
      Show Push dialog only when committing to protected branches
    </label>
    {#if protectedText !== null}
      <div class="protected">
        <label for="settings-protected-branches">Protected branches:</label>
        <input
          id="settings-protected-branches"
          class="input"
          spellcheck="false"
          bind:value={protectedText}
        />
      </div>
    {/if}
  </div>

  <h3>Update</h3>
  <div class="options">
    <div class="choices" role="group" aria-labelledby="settings-update-method">
      <span id="settings-update-method">Update method:</span>
      <label class="choice">
        <input type="radio" value="merge" bind:group={draft.updateMethod} />
        Merge
      </label>
      <label class="choice">
        <input type="radio" value="rebase" bind:group={draft.updateMethod} />
        Rebase
      </label>
    </div>
    <div class="choices" role="group" aria-labelledby="settings-clean-tree">
      <span id="settings-clean-tree">Clean working tree using:</span>
      <label class="choice">
        <input type="radio" checked disabled />
        Stash
      </label>
    </div>
    <div class="update-paths">
      <label for="settings-update-paths">Filter "Update Project" information by paths:</label>
      <textarea
        id="settings-update-paths"
        class="input"
        rows="3"
        placeholder="All"
        spellcheck="false"
        bind:value={updatePaths}
      ></textarea>
    </div>
  </div>

  <!-- DiffSettingsConfigurable: the Merge group of Diff & Merge. -->
  <h3>Merge</h3>
  <div class="options">
    <label class="checkbox-label">
      <input type="checkbox" bind:checked={draft.mergeAutoApply} />
      <span class="check" data-state={draft.mergeAutoApply ? "on" : "off"}></span>
      Automatically apply non-conflicting changes
    </label>
  </div>

  {#if problem}
    <p class="field-error" role="alert">{problem}</p>
  {/if}

  {#snippet footer()}
    <button type="button" class="button" onclick={onclose}>Cancel</button>
    <button class="button default" disabled={saving}>OK</button>
  {/snippet}
</Dialog>

<style>
  h3 {
    margin: 0 0 10px;
    font-size: 13px;
    font-weight: 600;
    color: var(--text);
  }

  .path {
    display: flex;
    gap: 8px;
    align-items: center;
  }

  .path label {
    white-space: nowrap;
  }

  .path input {
    flex: 1;
    min-width: 0;
  }

  .options {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding-bottom: 8px;
  }

  .options.helper {
    padding: 10px 0 0;
  }

  .update-paths {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .nested {
    margin-left: 20px;
  }

  .protected,
  .choices,
  .choice {
    display: flex;
    gap: 8px;
    align-items: center;
  }

  .choices {
    gap: 12px;
  }

  .protected label {
    white-space: nowrap;
  }

  .protected input {
    flex: 1;
    min-width: 0;
  }

  .update-paths textarea {
    resize: vertical;
  }

  .incoming {
    display: flex;
    gap: 8px;
    align-items: center;
  }

  h3:not(:first-child) {
    margin-top: 18px;
  }

  .comment {
    margin: 6px 0 0;
    color: var(--text-secondary);
  }

  .comment.nested {
    margin: -6px 0 0 20px;
  }

  .signature {
    margin: 6px 0 0;
  }

  .key {
    display: flex;
    gap: 8px;
    align-items: center;
    min-height: 28px;
  }

  .key .input {
    flex: 1;
    min-width: 0;
  }
</style>
