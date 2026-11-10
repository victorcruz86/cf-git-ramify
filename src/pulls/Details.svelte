<script lang="ts">
  import { onDestroy, onMount, tick } from "svelte";
  import ChangesTree from "../commit/ChangesTree.svelte";
  import { changeRoots, folderIds, pairOf, selectedFiles } from "../changes/files";
  import {
    clipboardWrite,
    diffWindowOpenRequest,
    githubPrChanges,
    githubPrCommits,
    githubPrDetails,
    githubPrStatus,
    openUrl,
    taskCancel,
    type AccountView,
    type ApiError,
    type ChangeWithSides,
    type CiJob,
    type Connection,
    type PullRequestChanges,
    type PullRequestCommit,
    type PullRequestDetails,
    type PullRequestStatus,
  } from "../lib/api";
  import Dialog from "../lib/Dialog.svelte";
  import Icon from "../lib/Icon.svelte";
  import { matchChangesBrowserShortcut, matchShortcut } from "../lib/keymap";
  import Menu, { type MenuItem } from "../lib/Menu.svelte";
  import { errorText } from "../lib/errors";
  import { notify } from "../lib/notifications";
  import { speedMatches } from "../lib/speedSearch";
  import { taskResult } from "../lib/tasks";
  import { loginFor } from "../hosting/accounts";
  import { CANNOT_OPEN } from "../hosting/links";
  import { prettyDateTime } from "../log/draw";
  import { openInViewer } from "../viewer/window";
  import {
    allCommitsText,
    commitsLink,
    commitsTitle,
    diffTitle,
    jobIcon,
    LOADING_STATUS,
    stateTag,
    statusLines,
    type StatusIcon,
  } from "./details";
  import {
    adText,
    checkoutLabel,
    pullRequestBranch,
    showInLogLabel,
    sourceText,
    watchCheckedOut,
    type RemoteAnswers,
  } from "./branch";
  import { openableUrl, pullsGeneration, refusedUrlText } from "./store";

  // A pull request's tab (P§6.2): GHPRDetailsComponentFactory without the timeline link and the
  // actions bar, with the description as plain text under the title (decision 16), the commits
  // dropdown, the selected commit, the changes tree computed with git after the two fetches
  // (decision 9) and the status lines without the access ones (decision 7). Every text from GitHub
  // is shown as text, and every URL it sent opens only on the connection's server.
  interface Props {
    connection: Connection;
    // The connection's account, for `Re-Login`.
    account: AccountView | null;
    number: number;
    // Who answers the questions of `Checkout '…'…`: the branch dialogs of the main window.
    answers?: RemoteAnswers | null;
  }

  let { connection, account, number, answers = null }: Props = $props();

  // `CTB:48`, `GB:73`, `GB:264`, `GB:328`, `GB:325`, `GB:210`, `GB:20`, `GB:2`, `CTB:76-77`,
  // `CTB:80`, `CTB:89`, `CTB:93`, `GB:215`, `GB:213`, `GB:216`; `Refresh` is ⌘R in the macOS keymap.
  const LOADING = "Loading…";
  const OPEN_ON_GITHUB = "Open on GitHub";
  const NO_DESCRIPTION = "No description provided";
  const CANNOT_LOAD_DETAILS = "Can't load details";
  const CANNOT_LOAD_CHANGES = "Can't load changes";
  const NO_CHANGES = "Pull request does not contain any changes";
  const RE_LOGIN = "Re-Login";
  const RETRY = "Retry";
  const HIDE_DETAILS = "Hide details";
  const SHOW_DETAILS = "Show details";
  const SEARCH_COMMITS = "Search commits";
  const CI_DETAILS = "Details…";
  const CI_POPUP_TITLE = "Open in Browser";
  const REFRESH = "Refresh Pull Request";
  const REFRESH_SHORTCUT = "⌘R";
  const OPEN_IN_BROWSER = "Open Pull Request in Browser";
  const COPY_URL = "Copy Pull Request URL";
  const SCOPE = "pull-request";
  const NO_CHECKS = new Set<string>();
  const ICON_LABELS: Record<StatusIcon, string> = {
    success: "Success",
    failed: "Failed",
    failedInProgress: "Failed, in progress",
    pending: "Pending",
    skipped: "Skipped",
    warning: "Warning",
  };

  let details = $state.raw<PullRequestDetails | null>(null);
  let detailsFailure = $state.raw<ApiError | null>(null);
  let status = $state.raw<PullRequestStatus | null>(null);
  let commits = $state.raw<PullRequestCommit[]>([]);
  // The chosen commit, `null` for all of them.
  let selection = $state<number | null>(null);
  let changes = $state.raw<ChangeWithSides[] | null>(null);
  let body = $state<string | null>(null);
  let changesFailure = $state.raw<ApiError | null>(null);
  let expanded = $state(false);
  let now = $state(Date.now());

  let collapsed = $state<Set<string>>(new Set());
  let selected = $state<Set<string>>(new Set());
  let lead = $state<string | null>(null);
  let byDirectory = $state(true);
  let menu = $state.raw<{
    x: number;
    y: number;
    label: string;
    items: MenuItem[];
    title?: string;
    ad?: string;
  } | null>(null);
  // P§7.6: the current branch is the one that tracks the head's branch.
  let checkedOut = $state(false);
  let chooser = $state.raw<{ x: number; y: number } | null>(null);
  let query = $state("");
  let activeRow = $state(0);
  let search = $state<HTMLInputElement>();

  // Bumped by every reload of the tab, and by every new ask for changes: an answer for an older
  // one is dropped, as is anything asked for a window that was forgotten since.
  let loads = 0;
  let changeAsks = 0;
  let destroyed = false;
  const running = new Set<number>();

  const roots = $derived(changeRoots(SCOPE, changes ?? [], byDirectory));
  const byPath = $derived(new Map((changes ?? []).map((change) => [change.path, change])));
  const lines = $derived(details ? statusLines(details, status) : null);
  const tag = $derived(details ? stateTag(details) : null);
  const chosen = $derived(selection === null ? null : (commits[selection] ?? null));

  const source = $derived(details ? sourceText(details) : "");

  // Asked once the details are loaded, and again on every `repo-changed`.
  let watch: ReturnType<typeof watchCheckedOut> | null = null;

  onMount(() => {
    watch = watchCheckedOut(number, (value) => (checkedOut = value));
    void reload();
  });

  onDestroy(() => {
    destroyed = true;
    watch?.stop();
    for (const id of running) void taskCancel(id).catch(() => {});
    running.clear();
  });

  // A task's value, unless the tab moved on before it started: then it is cancelled.
  async function run<T>(start: Promise<number>, stale: () => boolean): Promise<T> {
    const id = await start;
    if (stale()) {
      void taskCancel(id).catch(() => {});
      throw { kind: "cancelled", message: "The operation was cancelled" };
    }
    running.add(id);
    try {
      return await taskResult<T>(id);
    } finally {
      running.delete(id);
    }
  }

  const failureOf = (failed: unknown): ApiError =>
    ({ kind: (failed as Partial<ApiError> | null)?.kind ?? "failed", message: errorText(failed) }) as ApiError;

  // `GHPRInfoViewModel.refresh`: the details, then the status, the commits and the changes for
  // the head they name, with both fetches again.
  async function reload() {
    const load = ++loads;
    const asked = pullsGeneration();
    const stale = () => load !== loads || asked !== pullsGeneration() || destroyed;
    details = null;
    detailsFailure = null;
    status = null;
    commits = [];
    selection = null;
    changes = null;
    body = null;
    changesFailure = null;
    let loaded: PullRequestDetails;
    try {
      loaded = await run<PullRequestDetails>(githubPrDetails(number), stale);
    } catch (failed) {
      if (stale()) return;
      detailsFailure = failureOf(failed);
      return;
    }
    if (stale()) return;
    details = loaded;
    now = Date.now();
    watch?.refresh();
    void loadStatus(loaded, stale);
    void loadCommits(loaded, stale);
    void loadChanges(null);
  }

  // A status error is swallowed, as in 241: the loading line stays.
  async function loadStatus(shown: PullRequestDetails, stale: () => boolean) {
    try {
      const loaded = await run<PullRequestStatus>(githubPrStatus(number), stale);
      if (!stale() && loaded.headRefOid === shown.headRefOid) status = loaded;
    } catch (failed) {
      if (!stale()) console.warn("Could not load the pull request status:", errorText(failed));
    }
  }

  // A commits error is shown nowhere: the list stays empty (`GHPRChangesViewModel`).
  async function loadCommits(shown: PullRequestDetails, stale: () => boolean) {
    try {
      const loaded = await run<PullRequestCommit[]>(githubPrCommits(number), stale);
      if (stale() || loaded.at(-1)?.oid !== shown.headRefOid) return;
      commits = loaded;
      now = Date.now();
    } catch (failed) {
      if (!stale()) console.warn("Could not load the pull request commits:", errorText(failed));
    }
  }

  // The changes of the selection, for the head and base the details show.
  async function loadChanges(chosenIndex: number | null) {
    const shown = details;
    if (!shown) return;
    const ask = ++changeAsks;
    const load = loads;
    const asked = pullsGeneration();
    const stale = () =>
      ask !== changeAsks || load !== loads || asked !== pullsGeneration() || destroyed || details !== shown;
    changes = null;
    body = null;
    changesFailure = null;
    try {
      const loaded = await run<PullRequestChanges>(
        githubPrChanges(number, shown.baseRefName, shown.baseRefOid, shown.headRefOid, chosenIndex),
        stale,
      );
      if (stale()) return;
      changes = loaded.changes;
      body = loaded.body;
      collapsed = new Set();
      selected = new Set();
      lead = null;
    } catch (failed) {
      if (stale()) return;
      changesFailure = failureOf(failed);
    }
  }

  function choose(index: number | null) {
    if (index === selection) return;
    selection = index;
    void loadChanges(index);
  }

  const isAuthentication = (error: ApiError) =>
    error.kind === "authentication" || error.kind === "missing_token";

  // `GHApiLoadingErrorHandler`: a login of the connection's account, then a refresh.
  async function relogin() {
    const id = connection.account;
    const server = account?.server ?? id.slice(0, id.lastIndexOf("/"));
    const asked = pullsGeneration();
    const result = await loginFor(server, id);
    if (result && asked === pullsGeneration() && !destroyed) void reload();
  }

  // `Retry` of the changes: both fetches again.
  const retryChanges = () => void loadChanges(selection);

  // A URL GitHub sent, opened only on the connection's server.
  function openOnGithub(url: string) {
    if (!openableUrl(connection, url)) {
      notify({ kind: "error", title: CANNOT_OPEN, body: refusedUrlText(connection) });
      return;
    }
    openUrl(url).catch((failed) => notify({ kind: "error", title: CANNOT_OPEN, body: errorText(failed) }));
  }

  // An external CI's page, held as `new URL(...).href` (a Unicode host as punycode, the rest
  // percent-encoded): the dialog shows it and `openUrl` gets it, so both are the same string.
  // An external CI's page: `https://` only, and only after Ramify has shown the full address.
  let external = $state<string | null>(null);

  // What the dialog shows must be what opens: no control, format (bidi, zero width), separator (a plain space aside) or
  // unassigned character, which render differently from the bytes, and no credentials.
  const UNSAFE = /[\p{C}\p{Z}\s]/u;
  const isExternalHttps = (url: string) => {
    try {
      const parsed = new URL(url);
      return parsed.protocol === "https:" && parsed.username === "" && parsed.password === "" && !UNSAFE.test(url.replaceAll(" ", ""));
    } catch {
      return false;
    }
  };

  function openCheck(url: string) {
    if (openableUrl(connection, url)) openOnGithub(url);
    else if (isExternalHttps(url)) external = new URL(url).href;
  }

  function confirmExternal() {
    const url = external;
    external = null;
    if (url) openUrl(url).catch((failed) => notify({ kind: "error", title: CANNOT_OPEN, body: errorText(failed) }));
  }

  function copyUrl() {
    if (!details) return;
    clipboardWrite(details.url).catch((failed) => notify({ kind: "error", title: errorText(failed) }));
  }

  const chosenFiles = (): ChangeWithSides[] => selectedFiles(roots, selected, byPath);

  // `Show Diff`, double click and Enter: the chosen files in the diff window (decision 13).
  function showDiff() {
    const files = chosenFiles();
    if (files.length === 0) return;
    diffWindowOpenRequest({
      kind: "revisions",
      files: files.map((file) => pairOf(file.before, file.after, file)),
      title: diffTitle(number),
    }).catch((failed) => notify({ kind: "error", title: errorText(failed) }));
  }

  // `EditSource`: the files as they are on disk.
  function jumpToSource() {
    const targets = chosenFiles()
      .filter((file) => file.status[0] !== "D")
      .map((file) => ({ path: file.path, revision: null, line: null }));
    if (targets.length === 0) return;
    openInViewer(targets).catch((failed) => notify({ kind: "error", title: errorText(failed) }));
  }

  // `Github.PullRequest.Details.Popup`.
  function pullRequestItems(): MenuItem[] {
    return [
      { label: REFRESH, shortcut: REFRESH_SHORTCUT, run: () => void reload() },
      { label: OPEN_IN_BROWSER, disabled: !details, run: () => details && openOnGithub(details.url) },
      { label: COPY_URL, disabled: !details, run: copyUrl },
    ];
  }

  // `Github.PullRequest.Changes.Popup` (P§6.8), without `Mark as Viewed` and without `Open in
  // Separate Window`: Ramify's diff is always a window of its own.
  function changesItems(): MenuItem[] {
    const none = chosenFiles().length === 0;
    return [
      { label: "Show Diff", shortcut: "⌘D", disabled: none, run: showDiff },
      { label: "Jump to Source", shortcut: "⌘↓", disabled: none, run: jumpToSource },
      null,
      {
        label: "View Options",
        submenu: [
          { header: "Group By" },
          { label: "Directory", shortcut: "⌃P", checked: byDirectory, run: () => (byDirectory = !byDirectory) },
        ],
      },
      { label: "Expand All", shortcut: "⌘+", run: () => (collapsed = new Set()) },
      { label: "Collapse All", shortcut: "⌘-", run: () => (collapsed = new Set(folderIds(roots))) },
      null,
      ...pullRequestItems(),
    ];
  }

  // The branch's chooser (CodeReviewDetailsBranchComponentFactory): `Checkout '…'…` and `Show '…'
  // in Git Log`, with `target ← source` under them.
  function branchMenu(anchor: HTMLElement) {
    if (!details) return;
    const bounds = anchor.getBoundingClientRect();
    const text = sourceText(details);
    const flow = pullRequestBranch(answers);
    menu = {
      x: bounds.left,
      y: bounds.bottom + 2,
      label: "Branch",
      items: [
        { label: checkoutLabel(text), run: () => void flow.checkout(number) },
        { label: showInLogLabel(text), run: () => void flow.showInLog(number) },
      ],
      ad: adText(details.baseRefName, text),
    };
  }

  function panelMenu(event: MouseEvent) {
    // The changes tree has its own menu.
    if (event.defaultPrevented) return;
    event.preventDefault();
    menu = { x: event.clientX, y: event.clientY, label: "Pull request", items: pullRequestItems() };
  }

  // The tab's own shortcuts: ⌘R everywhere in it, and the changes tree's while it has the focus.
  function keydown(event: KeyboardEvent) {
    const inTree = (event.target as HTMLElement | null)?.closest?.('[aria-label="Changes"]') != null;
    if (event.key === "r" && event.metaKey) void reload();
    else if (inTree && matchShortcut(event) === "Diff.ShowDiff") showDiff();
    else if (inTree && matchShortcut(event) === "ExpandAll") collapsed = new Set();
    else if (inTree && matchShortcut(event) === "CollapseAll") collapsed = new Set(folderIds(roots));
    else if (inTree && matchShortcut(event) === "ChangesView.GroupBy.Directory") byDirectory = !byDirectory;
    else if (inTree && matchChangesBrowserShortcut(event) === "EditSource") jumpToSource();
    else return;
    event.preventDefault();
    event.stopPropagation();
  }

  // `Details…`: a chooser of the jobs, each opening its URL. A page on the connection's server
  // opens at once; an `https://` page elsewhere (an external CI) opens after a confirmation that
  // shows the full address (the user's decision of 2026-11-09); any other URL is disabled, and its
  // tooltip says why under its state: Ramify's own `refusedUrlText`, which 241 does not have.
  function ciMenu(anchor: HTMLElement, jobs: CiJob[]) {
    const bounds = anchor.getBoundingClientRect();
    const refused = (job: CiJob) => job.url !== null && !isExternalHttps(job.url) && !openableUrl(connection, job.url);
    menu = {
      x: bounds.left,
      y: bounds.bottom + 2,
      label: CI_POPUP_TITLE,
      title: CI_POPUP_TITLE,
      items: jobs.map((job) => ({
        label: job.name,
        title: refused(job)
          ? `${ICON_LABELS[jobIcon(job)]}\n${refusedUrlText(connection)}`
          : ICON_LABELS[jobIcon(job)],
        disabled: job.url === null || refused(job),
        run: () => job.url && openCheck(job.url),
      })),
    };
  }

  // The commits chooser: `All commits (n)`, then each commit, filtered by its headline.
  const choices = $derived.by(() => {
    const all = { index: null as number | null, text: allCommitsText(commits.length) };
    const rows = [all, ...commits.map((commit, index) => ({ index, text: commit.headline }))];
    return query === "" ? rows : rows.filter((row) => speedMatches(row.text, query));
  });

  async function openChooser(anchor: HTMLElement) {
    if (commits.length <= 1) return;
    const bounds = anchor.getBoundingClientRect();
    query = "";
    activeRow = 0;
    now = Date.now();
    chooser = { x: bounds.left, y: bounds.bottom + 2 };
    await tick();
    search?.focus();
  }

  // `showChooserPopup` gives nothing back when it closes without a choice, which 241 takes as
  // all commits.
  function closeChooser(index: number | null) {
    chooser = null;
    choose(index);
  }

  function chooserKeydown(event: KeyboardEvent) {
    if (event.key === "ArrowDown") {
      activeRow = Math.min(choices.length - 1, activeRow + 1);
    } else if (event.key === "ArrowUp") {
      activeRow = Math.max(0, activeRow - 1);
    } else if (event.key === "Enter") {
      const row = choices[activeRow];
      if (row) closeChooser(row.index);
    } else if (event.key === "Escape") {
      closeChooser(null);
    } else {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
  }

  const commitDate = (commit: PullRequestCommit) => {
    const time = Date.parse(commit.committedDate);
    return Number.isNaN(time) ? commit.committedDate : prettyDateTime(time, now);
  };
</script>

{#snippet mark(icon: StatusIcon)}
  <span class="mark {icon}" role="img" aria-label={ICON_LABELS[icon]}></span>
{/snippet}

{#snippet failure(title: string, error: ApiError, retry: () => void)}
  <div class="status-message error" role="alert">
    <strong>{title}</strong>
    <p>{error.message}</p>
    {#if isAuthentication(error)}
      <button type="button" class="link" onclick={() => void relogin()}>{RE_LOGIN}</button>
    {:else}
      <button type="button" class="link" onclick={retry}>{RETRY}</button>
    {/if}
  </div>
{/snippet}

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="details" onkeydown={keydown} oncontextmenu={panelMenu}>
  {#if detailsFailure}
    {@render failure(CANNOT_LOAD_DETAILS, detailsFailure, () => void reload())}
  {:else if !details}
    <p class="status-message" role="status">{LOADING}</p>
  {:else}
    <div class="title-row">
      <h2 class="title">
        <span>{details.title}</span>&nbsp;<button
          type="button"
          class="link number"
          title={OPEN_ON_GITHUB}
          onclick={() => details && openOnGithub(details.url)}>#{details.number}</button
        >
      </h2>
      {#if tag}
        <span class="tag">{tag}</span>
      {/if}
    </div>

    <div class="description" aria-label="Description">
      {#if details.body.trim() === ""}
        <p class="empty">{NO_DESCRIPTION}</p>
      {:else}
        <p class="body">{details.body}</p>
      {/if}
    </div>

    <div class="commits-row">
      <div class="commits">
        <strong>{commitsTitle(commits.length)}</strong>
        <button
          type="button"
          class="link dropdown"
          aria-haspopup="listbox"
          disabled={commits.length <= 1}
          onclick={(event) => void openChooser(event.currentTarget)}
        >
          {commitsLink(commits, selection)}
          <Icon name="chevronDown" size={12} />
        </button>
        {#if commits.length > 1 && selection !== null}
          <button
            type="button"
            class="action"
            aria-label="Next commit"
            disabled={selection === commits.length - 1}
            onclick={() => selection !== null && choose(selection + 1)}
          >
            <Icon name="down" size={14} />
          </button>
          <button
            type="button"
            class="action"
            aria-label="Previous commit"
            disabled={selection === 0}
            onclick={() => selection !== null && choose(selection - 1)}
          >
            <Icon name="up" size={14} />
          </button>
        {/if}
      </div>
      <span class="branch" aria-label="Branch" data-checked-out={checkedOut}>
        <button
          type="button"
          class="link branch-link"
          aria-haspopup="menu"
          onclick={(event) => branchMenu(event.currentTarget)}
        >
          <Icon name={checkedOut ? "currentBranch" : "branchLabel"} size={14} />
          {source}
        </button>
      </span>
    </div>

    {#if chosen}
      <div class="commit-info" aria-label="Commit">
        <div class="headline">
          <span>{chosen.headline}</span>
          <button type="button" class="link" onclick={() => (expanded = !expanded)}>
            {expanded ? HIDE_DETAILS : SHOW_DETAILS}
          </button>
        </div>
        {#if expanded}
          {#if body}
            <p class="body">{body}</p>
          {/if}
          <p class="meta">{chosen.author}, {commitDate(chosen)}</p>
        {/if}
      </div>
    {/if}

    <div class="changes">
      {#if changesFailure}
        {@render failure(CANNOT_LOAD_CHANGES, changesFailure, retryChanges)}
      {:else if changes === null}
        <p class="status-message" role="status">{LOADING}</p>
      {:else if changes.length === 0}
        <p class="status-message">{NO_CHANGES}</p>
      {:else}
        <ChangesTree
          {roots}
          included={NO_CHECKS}
          bind:collapsed
          bind:selected
          bind:lead
          ontoggle={() => {}}
          onopen={showDiff}
          onmenu={(x, y) => (menu = { x, y, label: "Changes", items: changesItems() })}
        />
      {/if}
    </div>

    <div class="statuses" aria-label="Status">
      {#if lines === null}
        <p class="line">{@render mark("pending")}{LOADING_STATUS}</p>
      {:else}
        {#each lines as line, at (at)}
          <p class="line">
            {@render mark(line.icon)}
            <span>{line.text}</span>
            {#if line.kind === "ci" && status}
              {@const jobs = status.jobs}
              <button type="button" class="link" onclick={(event) => ciMenu(event.currentTarget, jobs)}>
                {CI_DETAILS}
              </button>
            {/if}
          </p>
        {/each}
      {/if}
    </div>
  {/if}
</div>

{#if chooser}
  <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
  <div class="backdrop" onclick={() => closeChooser(null)}></div>
  <div class="chooser" style:left="{chooser.x}px" style:top="{chooser.y}px">
    <input
      bind:this={search}
      type="search"
      placeholder={SEARCH_COMMITS}
      aria-label={SEARCH_COMMITS}
      spellcheck="false"
      bind:value={query}
      oninput={() => (activeRow = 0)}
      onkeydown={chooserKeydown}
    />
    <ul role="listbox" aria-label="Commits">
      {#each choices as row, at (row.index ?? -1)}
        {@const commit = row.index === null ? null : commits[row.index]}
        <li
          role="option"
          class:all={row.index === null}
          class:active={at === activeRow}
          aria-selected={row.index === selection}
          onclick={() => closeChooser(row.index)}
          onkeydown={() => {}}
          onmouseenter={() => (activeRow = at)}
        >
          <span class="check">{row.index === selection ? "✓" : ""}</span>
          <span class="text">
            <span>{row.text}</span>
            {#if commit}
              <span class="meta">{commit.author}, {commitDate(commit)}</span>
            {/if}
          </span>
        </li>
      {/each}
    </ul>
  </div>
{/if}

{#if external}
  <Dialog title="Open External Link" width={520} onclose={() => (external = null)} onsubmit={confirmExternal}>
    <p class="leaves">This link leaves GitHub:</p>
    <p class="address">{external}</p>
    {#snippet footer()}
      <button type="button" class="button" onclick={() => (external = null)}>Cancel</button>
      <button class="button default" data-autofocus>Open</button>
    {/snippet}
  </Dialog>
{/if}

{#if menu}
  <Menu
    x={menu.x}
    y={menu.y}
    label={menu.label}
    title={menu.title}
    ad={menu.ad}
    items={menu.items}
    onclose={() => (menu = null)}
  />
{/if}

<style>
  .leaves {
    margin: 0 0 6px;
  }

  .address {
    margin: 0;
    font-family: var(--mono, monospace);
    overflow-wrap: anywhere;
    user-select: text;
  }

  .details {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    padding: 8px 12px;
    box-sizing: border-box;
    background: var(--panel);
    color: var(--text);
  }

  .title-row {
    display: flex;
    flex: none;
    gap: 8px;
    align-items: baseline;
  }

  .title {
    flex: 1;
    margin: 0;
    font-size: 1.25em;
    font-weight: bold;
    overflow-wrap: anywhere;
  }

  .number {
    font-weight: normal;
    color: var(--text-secondary);
  }

  .tag {
    flex: none;
    padding: 0 4px;
    font-size: 0.85em;
    color: var(--text-secondary);
    border: 1px solid var(--divider);
    border-radius: 3px;
  }

  .description {
    flex: none;
    max-height: 30%;
    overflow: auto;
    margin: 6px 0;
  }

  .body {
    margin: 0;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    user-select: text;
  }

  .empty {
    margin: 0;
    font-style: italic;
    color: var(--text-secondary);
  }

  .commits-row {
    display: flex;
    flex: none;
    align-items: center;
    gap: 8px;
    padding: 6px 0 5px;
  }

  .commits {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .branch {
    display: inline-flex;
    gap: 4px;
    align-items: center;
    margin-left: auto;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .branch-link {
    display: inline-flex;
    gap: 4px;
    align-items: center;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .commit-info {
    flex: none;
    padding-bottom: 6px;
  }

  .headline {
    display: flex;
    gap: 8px;
    white-space: nowrap;
  }

  .headline span {
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .meta {
    margin: 0;
    color: var(--text-secondary);
  }

  .changes {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-height: 80px;
  }

  .statuses {
    flex: none;
    max-height: 150px;
    overflow: auto;
    padding-top: 6px;
  }

  .line {
    display: flex;
    gap: 6px;
    align-items: center;
    margin: 0;
    padding: 2px 0;
  }

  .mark {
    flex: none;
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: var(--text-secondary);
  }

  .mark.success {
    background: var(--status-added);
  }

  .mark.failed {
    background: var(--error);
  }

  .mark.failedInProgress {
    background: linear-gradient(90deg, var(--error) 50%, var(--text-secondary) 50%);
  }

  .mark.warning {
    background: var(--editor-warning);
  }

  .mark.skipped {
    background: none;
    border: 1px solid var(--text-secondary);
    box-sizing: border-box;
  }

  .status-message {
    padding: 16px;
    margin: 0;
    color: var(--text-secondary);
    text-align: center;
  }

  .status-message.error {
    color: var(--text);
    white-space: pre-line;
    overflow-wrap: anywhere;
  }

  .status-message.error p {
    margin: 4px 0;
  }

  .link {
    padding: 0;
    font: inherit;
    color: var(--link);
    background: none;
    border: 0;
    cursor: pointer;
  }

  .link:disabled {
    color: var(--text);
    cursor: default;
  }

  .dropdown {
    display: inline-flex;
    gap: 2px;
    align-items: center;
  }

  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 9;
  }

  .chooser {
    position: fixed;
    z-index: 10;
    display: flex;
    flex-direction: column;
    width: 360px;
    max-height: 320px;
    background: var(--popup-background);
    border: 1px solid var(--popup-border);
    box-shadow: 0 4px 12px rgb(0 0 0 / 25%);
  }

  .chooser input {
    margin: 6px;
  }

  .chooser ul {
    margin: 0;
    padding: 0;
    overflow: auto;
    list-style: none;
  }

  .chooser li {
    display: flex;
    gap: 6px;
    padding: 4px 8px;
    cursor: default;
  }

  .chooser li.all {
    border-bottom: 1px solid var(--divider);
  }

  .chooser li.active {
    background: var(--selection);
  }

  .chooser .check {
    flex: none;
    width: 1em;
  }

  .chooser .text {
    display: flex;
    flex-direction: column;
    min-width: 0;
    overflow-wrap: anywhere;
  }
</style>
