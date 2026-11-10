<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { effectiveProtected, protectedBranches } from "../branches/state";
  import AccountsList from "../hosting/AccountsList.svelte";
  import {
    cleanTreeSet,
    gitVersion,
    protectedBranchesSet,
    protectedPatternsCheck,
    repoSettingsGet,
    settingsSave,
    type ApiError,
    type CleanTree,
    type Settings,
  } from "../lib/api";
  import Dialog from "../lib/Dialog.svelte";
  import { errorText } from "../lib/errors";

  interface Props {
    settings: Settings;
    onclose: () => void;
  }

  // `spinner(1..100000, 1)` of the large files row.
  const LIMIT_RANGE = { min: 1, max: 100000 };
  // `Connection timeout:` is shown in seconds and stored in milliseconds (`GithubSettingsConfigurable`).
  const TIMEOUT_RANGE = { min: 0, max: 60 };

  let { settings, onclose }: Props = $props();

  let draft = $state({ ...untrack(() => settings) });
  // One path per line, like the log's path filter.
  let updatePaths = $state(untrack(() => settings.updatePaths.join("\n")));
  let timeoutSeconds = $state<number | null>(untrack(() => settings.githubConnectionTimeoutMs / 1000));
  // Of the open repository, separated by `;` as in `ParametersListUtil.COLON_LINE_PARSER`. `null`:
  // there is no repository, or they could not be read, and then they are left untouched.
  let protectedText = $state<string | null>(null);
  // `Clean working tree using:` of the open repository, and what it had. `null`: there is no
  // repository, or it could not be read; the radio is then greyed out on the default.
  let cleanTree = $state<CleanTree | null>(null);
  let savedCleanTree: CleanTree | null = null;
  // Until it is read nothing is checked: the default would be wrong for a repository with Stash.
  let cleanTreeRead = $state(false);
  // `Load branch protection rules from GitHub`, and the rules loaded on the last fetches.
  let loadGithub = $state(true);
  let githubProtected: string[] = [];
  // `ExpandableTextFieldWithReadOnlyText.readOnlyText`: GitHub's rules joined with `;`, only when
  // the checkbox was on as the page opened. It is shown after the user's text and never saved.
  let readOnlyText = "";
  // The text the grey suffix was painted for, and the suffix. It shows while the field still has
  // that text, as `setTextToTriggerStatus` does, and follows the text on focus loss.
  let readOnly = $state<{ text: string; suffix: string } | null>(null);
  // The error of the first pattern that does not compile, while it is typed. It does not block Save.
  let protectedError = $state<string | null>(null);
  let checkRun = 0;
  let checkTimer: ReturnType<typeof setTimeout> | undefined;
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

  // The user's patterns first, then the read-only ones.
  async function checkProtected(text: string) {
    const run = ++checkRun;
    try {
      let error = await protectedPatternsCheck(text);
      if (error === null && readOnlyText) error = await protectedPatternsCheck(readOnlyText);
      if (run === checkRun) protectedError = error;
    } catch {
      if (run === checkRun) protectedError = null;
    }
  }

  // The same pause for typing as the rest of the app uses for text fields: none, so 300 ms.
  function protectedEdited() {
    clearTimeout(checkTimer);
    checkTimer = setTimeout(() => void checkProtected(protectedText ?? ""), 300);
  }

  // `updateReadOnlyText`.
  function paintReadOnly(text: string) {
    if (!readOnlyText.trim()) return;
    readOnly = { text, suffix: text.trim() ? `;${readOnlyText}` : readOnlyText };
  }

  async function loadProtected() {
    try {
      const repoSettings = await repoSettingsGet();
      const { protectedBranches: stored, cleanTree: saver } = repoSettings;
      cleanTree = savedCleanTree = saver;
      cleanTreeRead = true;
      loadGithub = repoSettings.loadGithubProtection ?? true;
      githubProtected = repoSettings.githubProtected ?? [];
      readOnlyText = loadGithub ? githubProtected.join(";") : "";
      protectedText = stored.join(";");
      // `setText` paints the suffix only for a text that is not blank.
      if (protectedText.trim()) paintReadOnly(protectedText);
      await checkProtected(protectedText);
    } catch {
      protectedText = null;
      cleanTree = savedCleanTree = null;
      cleanTreeRead = true;
    }
  }

  async function saveCleanTree() {
    if (cleanTree === null || cleanTree === savedCleanTree) return;
    await cleanTreeSet(cleanTree);
    savedCleanTree = cleanTree;
  }

  async function saveProtected() {
    if (protectedText === null) return;
    const list = patterns(protectedText);
    await protectedBranchesSet(list, loadGithub);
    protectedBranches.set(
      effectiveProtected({
        protectedBranches: list,
        loadGithubProtection: loadGithub,
        githubProtected,
      }),
    );
  }

  // The spinner of IntelliJ keeps the value inside its range; an empty field goes back to what was saved.
  function validLimit(value: number | null): number {
    const whole = Math.trunc(Number(value));
    if (!value || Number.isNaN(whole)) return settings.largeFilesLimitMb;
    return Math.min(LIMIT_RANGE.max, Math.max(LIMIT_RANGE.min, whole));
  }

  // Zero is a valid timeout; an empty field goes back to what was saved.
  function validTimeout(value: number | null): number {
    const whole = Math.trunc(Number(value));
    if (value === null || Number.isNaN(whole)) return settings.githubConnectionTimeoutMs;
    return Math.min(TIMEOUT_RANGE.max, Math.max(TIMEOUT_RANGE.min, whole)) * 1000;
  }

  async function save() {
    if (saving) return;
    saving = true;
    problem = null;
    try {
      await saveProtected();
      await saveCleanTree();
      const paths = updatePaths
        .split("\n")
        .map((path) => path.trim())
        .filter((path, index, all) => path && all.indexOf(path) === index);
      await settingsSave({
        ...$state.snapshot(draft),
        largeFilesLimitMb: validLimit(draft.largeFilesLimitMb),
        githubConnectionTimeoutMs: validTimeout(timeoutSeconds),
        updatePaths: paths,
      });
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
    return () => clearTimeout(checkTimer);
  });
</script>

<Dialog title="Settings" width={600} {onclose} onsubmit={save}>
  <h3>Commit</h3>
  <div class="options">
    <label class="checkbox-label">
      <input type="checkbox" bind:checked={draft.nonModalCommit} />
      <span class="check" data-state={draft.nonModalCommit ? "on" : "off"}></span>
      Use non-modal commit interface
    </label>
    <p class="comment nested">Applies to projects under Git and Mercurial</p>
    <label class="checkbox-label">
      <input type="checkbox" bind:checked={draft.clearInitialCommitMessage} />
      <span class="check" data-state={draft.clearInitialCommitMessage ? "on" : "off"}></span>
      Clear initial commit message
    </label>
  </div>

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
    <div class="large-files">
      <label class="checkbox-label">
        <input type="checkbox" bind:checked={draft.warnLargeFiles} />
        <span class="check" data-state={draft.warnLargeFiles ? "on" : "off"}></span>
        Warn when committing files larger than
      </label>
      <input
        type="number"
        class="input limit"
        aria-label="Large file limit in MB"
        min={LIMIT_RANGE.min}
        max={LIMIT_RANGE.max}
        step="1"
        disabled={!draft.warnLargeFiles}
        bind:value={draft.largeFilesLimitMb}
      />
      <span class="unit" class:disabled={!draft.warnLargeFiles}>MB</span>
    </div>
    <label class="checkbox-label">
      <input type="checkbox" bind:checked={draft.warnBadFileNames} />
      <span class="check" data-state={draft.warnBadFileNames ? "on" : "off"}></span>
      Warn when committing files with names that might cause issues on other systems
    </label>
    <label class="checkbox-label">
      <input type="checkbox" bind:checked={draft.cherryPickSuffix} />
      <span class="check" data-state={draft.cherryPickSuffix ? "on" : "off"}></span>
      Add the 'cherry-picked from &lt;hash&gt;' suffix when picking commits pushed to protected branches
    </label>
    <!-- Greyed out while the commit interface is modal; the saved value is left as it is. -->
    <label class="checkbox-label" class:disabled={!draft.nonModalCommit}>
      <input type="checkbox" disabled={!draft.nonModalCommit} bind:checked={draft.stagingArea} />
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
        <div class="protected-field">
          <input
            id="settings-protected-branches"
            class="input"
            spellcheck="false"
            bind:value={protectedText}
            oninput={protectedEdited}
            onblur={() => paintReadOnly(protectedText ?? "")}
          />
          {#if readOnly && readOnly.text === protectedText}
            <span class="read-only" aria-hidden="true"
              ><span class="typed">{readOnly.text}</span><span class="read-only-suffix"
                >{readOnly.suffix}</span
              ></span
            >
          {/if}
        </div>
      </div>
      {#if protectedError}
        <p class="field-error" role="alert">{protectedError}</p>
      {/if}
      <label class="checkbox-label nested">
        <input type="checkbox" bind:checked={loadGithub} />
        <span class="check" data-state={loadGithub ? "on" : "off"}></span>
        Load branch protection rules from GitHub
      </label>
      <p class="comment nested-checkbox">
        GitHub rules are added to the local rules and synced on every fetch
      </p>
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
      {#if cleanTree === null}
        <label class="choice">
          <input type="radio" disabled />
          Stash
        </label>
        <label class="choice">
          <input type="radio" checked={cleanTreeRead} disabled />
          Shelve
        </label>
      {:else}
        <label class="choice">
          <input type="radio" value="stash" bind:group={cleanTree} />
          Stash
        </label>
        <label class="choice">
          <input type="radio" value="shelve" bind:group={cleanTree} />
          Shelve
        </label>
      {/if}
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

  <!-- VcsGeneralSettingsConfigurable: the Confirmation group, with `GitLogCommitEditingConfigurable`. -->
  <h3>Confirmation</h3>
  <div class="options">
    <label class="checkbox-label">
      <input type="checkbox" bind:checked={draft.showDropCommitDialog} />
      <span class="check" data-state={draft.showDropCommitDialog ? "on" : "off"}></span>
      Ask for confirmation to drop commits
    </label>
    <p class="comment nested">Applicable to Git</p>
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

  <!-- GithubSettingsConfigurable. The accounts act at once: only the timeout goes with Save. -->
  <h3>GitHub</h3>
  <AccountsList />
  <div class="timeout">
    <label for="settings-github-timeout">Connection timeout:</label>
    <input
      id="settings-github-timeout"
      type="number"
      class="input limit"
      min={TIMEOUT_RANGE.min}
      max={TIMEOUT_RANGE.max}
      step="1"
      bind:value={timeoutSeconds}
    />
    <span>seconds</span>
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

  .protected-field {
    position: relative;
    display: flex;
    flex: 1;
    min-width: 0;
  }

  .protected input {
    flex: 1;
    min-width: 0;
  }

  /* Painted over the field as its empty text is: the typed text takes its place unseen, and the
     rules from GitHub follow it in grey. */
  .read-only {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    padding: 0 9px;
    overflow: hidden;
    white-space: pre;
    pointer-events: none;
  }

  .read-only .typed {
    visibility: hidden;
  }

  .read-only-suffix {
    color: var(--text-secondary);
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

  .comment.nested-checkbox {
    margin: -6px 0 0 40px;
  }

  .signature {
    margin: 6px 0 0;
  }

  .timeout {
    display: flex;
    gap: 8px;
    align-items: center;
    margin-top: 10px;
  }

  .large-files {
    display: flex;
    gap: 8px;
    align-items: center;
  }

  .limit {
    width: 80px;
  }

  .limit:disabled {
    color: var(--disabled);
  }

  .unit.disabled {
    color: var(--disabled);
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
