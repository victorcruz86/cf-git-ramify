<script lang="ts">
  import { onDestroy, onMount, tick } from "svelte";
  import {
    clipboardWrite,
    githubPrChooser,
    githubPrEtag,
    githubPrPage,
    githubPrSearchSave,
    githubPrSearchSaved,
    openUrl,
    taskCancel,
    type AccountView,
    type ApiError,
    type ChooserKind,
    type Connection,
    type PullRequestPage,
    type PullRequestShort,
    type ReviewerState,
    type SavedSearch,
    type SearchValue,
  } from "../lib/api";
  import Icon from "../lib/Icon.svelte";
  import Menu, { type MenuItem } from "../lib/Menu.svelte";
  import { errorText } from "../lib/errors";
  import { notify } from "../lib/notifications";
  import { taskResult } from "../lib/tasks";
  import { loginFor } from "../hosting/accounts";
  import { CANNOT_OPEN } from "../hosting/links";
  import { formatPrettyDate } from "../log/draw";
  import { etagPoller } from "./poll";
  import {
    clearFiltersLabel,
    EMPTY_SEARCH,
    FILTERS,
    filterCount,
    filterText,
    initialSearch,
    quickFilters,
    REVIEWS,
    sameSearch,
    searchHistory,
    shortText,
    STATES,
    type FilterField,
  } from "./search";
  import { openableUrl, pullsGeneration, refusedUrlText } from "./store";

  // The pull request list (P§5): GHPRListPanelFactory, ReviewListSearchPanelFactory,
  // ReviewListCellRenderer, GHPRListComponentFactory, GHPRListPanelController and the list's popup
  // group (plugins/github and platform/collaboration-tools at idea/241.14494.240). Decision 7:
  // there is no unseen dot. Every text from GitHub is shown as text. `active` is whether the list
  // is on screen and `focused` whether the app has the focus: the update check polls only while
  // both are true (overview decision 16).
  interface Props {
    connection: Connection;
    // The connection's account, for `Re-Login`.
    account: AccountView | null;
    active: boolean;
    focused: boolean;
    onopen: (number: number) => void;
  }

  let { connection, account, active, focused, onopen }: Props = $props();

  // `CTB:48`, `GB:169-174`, `GB:201`, `GB:20`, `GB:2`, `CTB:51`, `GB:235`, `CTB:191`.
  const LOADING = "Loading…";
  const NO_MATCHES = "No pull requests matching filters.";
  const CLEAR_FILTERS = "Clear filters";
  const OUTDATED = "The list is outdated.";
  const REFRESH = "Refresh";
  const CANNOT_LOAD = "Can't load pull requests";
  const AUTHORIZATION = "User authorization failed";
  const RE_LOGIN = "Re-Login";
  const RETRY = "Retry";
  const QUICK_FILTERS = "Quick Filters";
  const CANNOT_MERGE = "Pull request cannot be merged";
  const SOMETHING_WRONG = "Something went wrong";
  // `GB:202-204`, `GB:213`, `GB:216`; `Refresh` is ⌘R in the macOS keymap.
  const VIEW = "View Pull Request";
  const VIEW_DESCRIPTION = "View pull request information and timeline";
  const OPEN_IN_BROWSER = "Open Pull Request in Browser";
  const COPY_URL = "Copy Pull Request URL";
  const REFRESH_LIST = "Refresh List";
  const REFRESH_SHORTCUT = "⌘R";
  // `BoundedRangeModelThresholdListener(model, 0.7f)`.
  const THRESHOLD = 0.7;
  const AVATARS = 2;

  // `GB:244`, `{0,choice,0#…|1#…|2#…}`.
  const commentsText = (count: number) =>
    count === 0
      ? "No unresolved comments"
      : count === 1
        ? `${count} unresolved comment`
        : `${count} unresolved comments`;
  // `{0, choice, 1#Label|2#Labels}`, and the same for assignees and reviewers (`GB:220-226`).
  const choice = (count: number, one: string, many: string) => (count >= 2 ? many : one);

  // `getStateText`: none for an open pull request that is not a draft, and a draft says so over
  // its state (`CTB:84-86`).
  function stateTag(pr: PullRequestShort): string | null {
    if (pr.isDraft) return "DRAFT";
    if (pr.state === "CLOSED") return "CLOSED";
    if (pr.state === "MERGED") return "MERGED";
    return null;
  }

  // `review.list.info.author` and `review.list.info` (`CTB:49-50`).
  function infoText(pr: PullRequestShort, now: number): string {
    const time = Date.parse(pr.createdAt);
    const date = Number.isNaN(time) ? pr.createdAt : formatPrettyDate(time, now);
    const info = `#${pr.number} · created ${date}`;
    return pr.author === null ? info : `${info}, by ${pr.author}`;
  }

  // Ramify draws no remote images: an avatar is the name's initials.
  function initials(name: string): string {
    const words = name.trim().split(/\s+/).filter(Boolean);
    return words
      .slice(0, 2)
      .map((word) => [...word][0]?.toUpperCase() ?? "")
      .join("");
  }

  const REVIEW_ORDER: ReviewerState[] = ["ACCEPTED", "WAIT_FOR_UPDATES", "NEED_REVIEW"];
  const reviewersOf = (pr: PullRequestShort) =>
    REVIEW_ORDER.flatMap((state) => pr.reviewers.filter((reviewer) => reviewer.state === state));

  const swatch = (color: string | null) => (color && /^[0-9a-fA-F]{6}$/.test(color) ? `#${color}` : undefined);

  const me = $derived(connection.viewerLogin);
  let saved = $state.raw<SavedSearch | null>(null);
  // `null` until the saved search is read: nothing is asked before.
  let search = $state.raw<SearchValue | null>(null);
  let draft = $state("");
  let items = $state.raw<PullRequestShort[]>([]);
  let cursor: string | null = null;
  let hasNext = true;
  let loading = $state(false);
  let failure = $state.raw<ApiError | null>(null);
  let outdated = $state(false);
  let selected = $state(-1);
  let now = $state(Date.now());
  let list = $state<HTMLElement>();
  let menu = $state.raw<{
    x: number;
    y: number;
    label: string;
    items: MenuItem[];
    title?: string;
    chooser?: ChooserKind;
  } | null>(null);
  let tip = $state.raw<{
    x: number;
    y: number;
    header: string;
    entries: { name: string; color?: string | null }[];
  } | null>(null);

  // Bumped by every reload: a page asked for before it is dropped, as is anything asked for a
  // window that was forgotten since (`pullsGeneration`).
  let loads = 0;
  let running: number | null = null;
  let destroyed = false;
  // `GHPRRepositoryDataService`: the chooser values, until `Refresh`, and the tasks still loading
  // them.
  let choosers: Partial<Record<ChooserKind, Promise<string[]>>> = {};
  const chooserTasks = new Map<ChooserKind, number | null>();

  const history = searchHistory(githubPrSearchSave, connection.viewerLogin, (value) => {
    if (!destroyed) saved = value;
  });

  // An authentication refusal of the poll stops it: the list shows 241's error panel with
  // `Re-Login`, whose reload starts it again. The answers of an old load are dropped.
  const poller = etagPoller(
    async () => {
      const asked = pullsGeneration();
      const run = loads;
      const stale = () => asked !== pullsGeneration() || destroyed;
      let etag: string | null;
      try {
        etag = await githubPrEtag();
      } catch (failed) {
        if (!stale() && run === loads) refused(failed);
        throw failed;
      }
      if (stale()) throw new Error("stale");
      return etag;
    },
    (value) => (outdated = value),
    undefined,
    (failed) => isAuthentication(failed as ApiError),
  );

  function refused(failed: unknown) {
    const error = failed as Partial<ApiError> | null;
    if (!error || !isAuthentication(error as ApiError) || failure) return;
    failure = { kind: error.kind, message: errorText(failed) } as ApiError;
  }

  $effect(() => {
    poller.update(active && focused && items.length > 0 && failure === null);
  });

  onMount(() => {
    const asked = pullsGeneration();
    githubPrSearchSaved().then(
      (value) => {
        if (asked !== pullsGeneration() || destroyed) return;
        saved = value;
        apply(initialSearch(value, me), false);
      },
      (failed) => {
        if (asked !== pullsGeneration() || destroyed) return;
        console.warn("Could not read the pull requests search:", errorText(failed));
        apply(initialSearch(null, me), false);
      },
    );
  });

  onDestroy(() => {
    destroyed = true;
    poller.stop();
    history.dispose();
    cancelRunning();
    cancelChoosers();
  });

  function cancelRunning() {
    if (running !== null) void taskCancel(running).catch(() => {});
    running = null;
  }

  // The search changed: it is saved, and the loader starts again (`GHPRListLoader.searchQuery`).
  function apply(next: SearchValue, persist = true) {
    const old = search;
    if (old && sameSearch(old, next)) return;
    search = next;
    draft = next.text ?? "";
    if (persist && old) history.changed(old, next);
    reload();
  }

  const update = (change: Partial<SearchValue>) => search && apply({ ...search, ...change });

  // `GHPRListLoader.reset`: the data goes, and page 1 is asked again.
  function reload() {
    loads++;
    cancelRunning();
    items = [];
    cursor = null;
    hasNext = true;
    loading = false;
    failure = null;
    selected = -1;
    poller.reset();
    void loadMore();
  }

  // `refresh()`: the loader and the chooser values.
  function refresh() {
    cancelChoosers();
    choosers = {};
    reload();
  }

  // `canLoadMore`: not while loading, not past the last page and not after an error.
  async function loadMore() {
    const value = search;
    if (!value || loading || !hasNext || failure) return;
    const run = loads;
    const asked = pullsGeneration();
    const stale = () => run !== loads || asked !== pullsGeneration() || destroyed;
    loading = true;
    let page: PullRequestPage;
    try {
      const id = await githubPrPage(value, cursor);
      if (stale()) {
        void taskCancel(id).catch(() => {});
        return;
      }
      running = id;
      page = await taskResult<PullRequestPage>(id);
    } catch (failed) {
      if (stale()) return;
      running = null;
      loading = false;
      const kind = (failed as Partial<ApiError> | null)?.kind;
      // A page that was cancelled is no error: scrolling asks for it again.
      if (kind === "cancelled") return;
      failure = { kind: kind ?? "failed", message: errorText(failed) } as ApiError;
      return;
    }
    if (stale()) return;
    running = null;
    loading = false;
    now = Date.now();
    // GitHub's search cursors are offsets: a pull request that moved between two pages comes
    // twice, and is shown once.
    const loaded = new Set(items.map((pr) => pr.number));
    const added = page.items.filter((pr) => !loaded.has(pr.number) && loaded.add(pr.number));
    items = [...items, ...added];
    cursor = page.endCursor;
    hasNext = page.hasNext;
    // The check is repeated once the rows are in: a short list fills itself.
    await tick();
    if (!stale()) checkScroll();
  }

  // `isAtThreshold`: `(extent + value) / maximum >= 0.7`, never with no height.
  function checkScroll() {
    if (!list) return;
    const maximum = list.scrollHeight;
    if (maximum === 0) return;
    if ((list.clientHeight + list.scrollTop) / maximum >= THRESHOLD) void loadMore();
  }

  // `GHApiLoadingErrorHandler`: a login of the connection's account, then a refresh.
  async function relogin() {
    const id = connection.account;
    const server = account?.server ?? id.slice(0, id.lastIndexOf("/"));
    const asked = pullsGeneration();
    const result = await loginFor(server, id);
    if (result && asked === pullsGeneration() && !destroyed) refresh();
  }

  function isAuthentication(error: ApiError | null): boolean {
    return error?.kind === "authentication" || error?.kind === "missing_token";
  }

  function applyText() {
    const text = draft === "" ? null : draft;
    if (search && text !== search.text) update({ text });
  }

  function textKeydown(event: KeyboardEvent) {
    if (event.key === "Enter") {
      event.preventDefault();
      applyText();
    }
  }

  function openMenu(anchor: HTMLElement, label: string, items: MenuItem[], extra: { title?: string; chooser?: ChooserKind } = {}) {
    const bounds = anchor.getBoundingClientRect();
    menu = { x: bounds.left, y: bounds.bottom + 2, label, items, ...extra };
  }

  function historyMenu(anchor: HTMLElement) {
    const entries = [...(saved?.history ?? [])].reverse();
    openMenu(
      anchor,
      "Search History",
      entries.map((value) => ({ label: shortText(value).trim(), run: () => apply(value) })),
    );
  }

  function quickMenu(anchor: HTMLElement) {
    if (!search) return;
    const current = search;
    const items: MenuItem[] = quickFilters(me).map(({ title, value }) => ({
      label: title,
      checked: sameSearch(current, value),
      run: () => apply(value),
    }));
    const count = filterCount(current);
    if (count > 0) {
      items.push(null, { label: clearFiltersLabel(count), run: () => apply(EMPTY_SEARCH) });
    }
    openMenu(anchor, QUICK_FILTERS, items, { title: QUICK_FILTERS });
  }

  // A chooser's values load in a task; the next opening asks again after a failure or a cancel.
  // `chooserTasks` holds the ones still loading: their task id, or `null` until it is known.
  function loadChooser(kind: ChooserKind): Promise<string[]> {
    const known = choosers[kind];
    if (known) return known;
    chooserTasks.set(kind, null);
    const loaded = (async () => {
      try {
        const id = await githubPrChooser(kind);
        if (choosers[kind] !== loaded) {
          void taskCancel(id).catch(() => {});
          throw { kind: "cancelled", message: "The operation was cancelled" };
        }
        chooserTasks.set(kind, id);
        return await taskResult<string[]>(id);
      } finally {
        if (choosers[kind] === loaded) chooserTasks.delete(kind);
      }
    })();
    choosers[kind] = loaded;
    loaded.catch(() => {
      if (choosers[kind] === loaded) delete choosers[kind];
    });
    return loaded;
  }

  // A chooser still loading is cancelled: its dropdown closed, or the list went.
  function cancelChooser(kind: ChooserKind) {
    if (!chooserTasks.has(kind)) return;
    const id = chooserTasks.get(kind);
    chooserTasks.delete(kind);
    delete choosers[kind];
    if (id !== null && id !== undefined) void taskCancel(id).catch(() => {});
  }

  function cancelChoosers() {
    for (const kind of [...chooserTasks.keys()]) cancelChooser(kind);
  }

  function closeMenu() {
    const kind = menu?.chooser;
    menu = null;
    if (kind) cancelChooser(kind);
  }

  // The load the open dropdown waits for: one closed and opened again waits for another, and an
  // answer of the old one (a late rejection of its cancel) is not the new dropdown's.
  let chooserShown: Promise<string[]> | null = null;

  // `ChooserPopupUtil.showAsyncChooserPopup`: the values once they arrive.
  function chooserMenu(anchor: HTMLElement, name: string, field: FilterField, kind: ChooserKind) {
    openMenu(anchor, name, [{ label: LOADING, disabled: true }], { chooser: kind });
    const asked = pullsGeneration();
    const loading = loadChooser(kind);
    chooserShown = loading;
    const current = () => menu?.chooser === kind && chooserShown === loading && asked === pullsGeneration();
    loading.then(
      (values) => {
        if (!current() || !menu) return;
        menu = {
          ...menu,
          items: values.map((value) => ({ label: value, run: () => update({ [field]: value }) })),
        };
      },
      (failed) => {
        if (!current() || !menu) return;
        menu = { ...menu, items: [{ label: errorText(failed) || SOMETHING_WRONG, disabled: true }] };
      },
    );
  }

  function filterMenu(anchor: HTMLElement, filter: (typeof FILTERS)[number]) {
    if (filter.field === "state") {
      openMenu(
        anchor,
        filter.name,
        STATES.map(({ value, label }) => ({ label, run: () => update({ state: value }) })),
      );
    } else if (filter.field === "review") {
      openMenu(
        anchor,
        filter.name,
        REVIEWS.map(({ value, full }) => ({ label: full, run: () => update({ review: value }) })),
      );
    } else if (filter.chooser) {
      chooserMenu(anchor, filter.name, filter.field, filter.chooser);
    }
  }

  function chipKeydown(event: KeyboardEvent, field: FilterField) {
    if ((event.key === "Delete" || event.key === "Backspace") && search?.[field] !== null) {
      event.preventDefault();
      update({ [field]: null });
    }
  }

  function open(pr: PullRequestShort) {
    onopen(pr.number);
  }

  // `GHPullRequestURLAction`: the URL GitHub gave, opened only on the connection's server.
  function openInBrowser(pr: PullRequestShort) {
    if (!openableUrl(connection, pr.url)) {
      notify({
        kind: "error",
        title: CANNOT_OPEN,
        body: refusedUrlText(connection),
      });
      return;
    }
    openUrl(pr.url).catch((failed) => notify({ kind: "error", title: CANNOT_OPEN, body: errorText(failed) }));
  }

  function copyUrl(pr: PullRequestShort) {
    clipboardWrite(pr.url).catch((failed) => notify({ kind: "error", title: errorText(failed) }));
  }

  function rowMenu(event: MouseEvent, index: number) {
    event.preventDefault();
    selected = index;
    const pr = items[index];
    menu = {
      x: event.clientX,
      y: event.clientY,
      label: "Pull request",
      items: [
        { label: VIEW, title: VIEW_DESCRIPTION, run: () => open(pr) },
        { label: OPEN_IN_BROWSER, run: () => openInBrowser(pr) },
        { label: COPY_URL, run: () => copyUrl(pr) },
        null,
        { label: REFRESH_LIST, shortcut: REFRESH_SHORTCUT, run: refresh },
      ],
    };
  }

  function listKeydown(event: KeyboardEvent) {
    if (event.key === "r" && event.metaKey) {
      event.preventDefault();
      event.stopPropagation();
      refresh();
      return;
    }
    if (items.length === 0) return;
    if (event.key === "ArrowDown") {
      event.preventDefault();
      selected = Math.min(items.length - 1, selected + 1);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      selected = Math.max(0, selected - 1);
    } else if (event.key === "Home") {
      event.preventDefault();
      selected = 0;
    } else if (event.key === "End") {
      event.preventDefault();
      selected = items.length - 1;
    } else if (event.key === "Enter" && selected >= 0) {
      event.preventDefault();
      open(items[selected]);
    } else {
      return;
    }
    list?.querySelectorAll<HTMLElement>('[role="option"]')[selected]?.scrollIntoView?.({ block: "nearest" });
  }

  function showTip(event: MouseEvent | FocusEvent, header: string, entries: { name: string; color?: string | null }[]) {
    const bounds = (event.currentTarget as HTMLElement).getBoundingClientRect();
    tip = { x: bounds.left, y: bounds.bottom + 4, header, entries };
  }

  const hideTip = () => (tip = null);

  const count = $derived(search ? filterCount(search) : 0);
  const showOutdated = $derived(outdated && !loading && failure === null);
</script>

<div class="panel">
  <div class="search">
    <button
      type="button"
      class="action"
      aria-label="Search History"
      title="Search History"
      onclick={(event) => historyMenu(event.currentTarget)}
    >
      <Icon name="history" size={14} />
    </button>
    <input
      type="search"
      aria-label="Search"
      spellcheck="false"
      bind:value={draft}
      onkeydown={textKeydown}
    />
  </div>

  <div class="filters" role="toolbar" aria-label="Pull request filters">
    <button
      type="button"
      class="action quick"
      class:set={count > 0}
      aria-label={QUICK_FILTERS}
      title={QUICK_FILTERS}
      onclick={(event) => quickMenu(event.currentTarget)}
    >
      <Icon name="filter" />
    </button>
    {#each FILTERS as filter (filter.field)}
      {@const text = search ? filterText(search, filter.field) : null}
      {@const label = text === null ? filter.name : `${filter.name}: ${text}`}
      <span class="chip" class:active={text !== null}>
        <button
          type="button"
          aria-label={label}
          onclick={(event) => filterMenu(event.currentTarget, filter)}
          onkeydown={(event) => chipKeydown(event, filter.field)}
        >
          {text ?? filter.name}
          {#if text === null}
            <Icon name="chevronDown" size={12} />
          {/if}
        </button>
        {#if text !== null}
          <button
            type="button"
            aria-label="Remove {filter.name} filter"
            onclick={() => update({ [filter.field]: null })}
          >
            <Icon name="close" size={12} />
          </button>
        {/if}
      </span>
    {/each}
  </div>

  {#if showOutdated}
    <div class="outdated" role="status">
      {OUTDATED}
      <button type="button" class="link" onclick={refresh}>{REFRESH}</button>
    </div>
  {/if}

  {#if loading}
    <div class="progress" role="progressbar" aria-label={LOADING}></div>
  {/if}

  {#if failure}
    <div class="status error" role="alert">
      <strong>{CANNOT_LOAD}</strong>
      <p>{failure.kind === "authentication" ? AUTHORIZATION : failure.message}</p>
      {#if isAuthentication(failure)}
        <button type="button" class="link" onclick={() => void relogin()}>{RE_LOGIN}</button>
      {:else}
        <button type="button" class="link" onclick={refresh}>{RETRY}</button>
      {/if}
    </div>
  {:else if items.length === 0}
    <div class="status">
      {#if loading || search === null}
        <p>{LOADING}</p>
      {:else if count === 0}
        <p>No pull requests in {connection.coordinates.name}</p>
      {:else}
        <p>{NO_MATCHES}</p>
        <button type="button" class="link" onclick={() => apply(EMPTY_SEARCH)}>{CLEAR_FILTERS}</button>
      {/if}
    </div>
  {:else}
    <ul
      bind:this={list}
      class="rows"
      role="listbox"
      aria-label="Pull requests"
      tabindex="0"
      aria-activedescendant={selected >= 0 ? `pull-request-${items[selected]?.number}` : undefined}
      onscroll={checkScroll}
      onkeydown={listKeydown}
    >
      {#each items as pr, index (pr.number)}
        {@const tag = stateTag(pr)}
        {@const reviewers = reviewersOf(pr)}
        <li
          id="pull-request-{pr.number}"
          class="row"
          role="option"
          aria-selected={index === selected}
          onclick={() => (selected = index)}
          ondblclick={() => {
            selected = index;
            open(pr);
          }}
          oncontextmenu={(event) => rowMenu(event, index)}
          onkeydown={() => {}}
        >
          <div class="line">
            <span class="title">{pr.title}</span>
            {#if pr.labels.length > 0}
              {@const header = choice(pr.labels.length, "Label", "Labels")}
              <span
                class="labels"
                role="img"
                aria-label={header}
                onmouseenter={(event) => showTip(event, header, pr.labels)}
                onmouseleave={hideTip}
              >
                <Icon name="branchLabel" size={14} />
              </span>
            {/if}
            <span class="end">
              {#if tag}
                <span class="tag">{tag}</span>
              {/if}
              {#if pr.mergeable === "CONFLICTING"}
                <span class="conflict" title={CANNOT_MERGE}>
                  <Icon name="nonMergeable" size={14} />
                </span>
              {/if}
              {#if pr.assignees.length > 0}
                {@const header = choice(pr.assignees.length, "Assignee", "Assignees")}
                <span
                  class="avatars"
                  role="group"
                  aria-label={header}
                  onmouseenter={(event) =>
                    showTip(event, header, pr.assignees.map((name) => ({ name })))}
                  onmouseleave={hideTip}
                >
                  {#each pr.assignees.slice(0, AVATARS) as name, at (at)}
                    <span class="avatar" role="img" aria-label={name}>{initials(name)}</span>
                  {/each}
                </span>
              {/if}
              {#if reviewers.length > 0}
                {@const header = choice(reviewers.length, "Reviewer", "Reviewers")}
                <span
                  class="avatars"
                  role="group"
                  aria-label={header}
                  onmouseenter={(event) =>
                    showTip(event, header, reviewers.map(({ name }) => ({ name })))}
                  onmouseleave={hideTip}
                >
                  {#each reviewers.slice(0, AVATARS) as reviewer, at (at)}
                    <span
                      class="avatar {reviewer.state.toLowerCase()}"
                      role="img"
                      aria-label={reviewer.name}>{initials(reviewer.name)}</span
                    >
                  {/each}
                </span>
              {/if}
              <span class="comments" title={commentsText(pr.unresolved)}>
                <Icon name="comment" size={14} />
                {pr.unresolved}
              </span>
            </span>
          </div>
          <div class="info">{infoText(pr, now)}</div>
        </li>
      {/each}
    </ul>
  {/if}
</div>

{#if tip}
  <div class="tip" role="tooltip" style:left="{tip.x}px" style:top="{tip.y}px">
    <div class="tip-header">{tip.header}</div>
    <ul>
      {#each tip.entries as entry, at (at)}
        <li>
          {#if entry.color !== undefined}
            <span class="swatch" style:background={swatch(entry.color)}></span>
          {/if}
          {entry.name}
        </li>
      {/each}
    </ul>
  </div>
{/if}

{#if menu}
  <Menu
    x={menu.x}
    y={menu.y}
    label={menu.label}
    title={menu.title}
    items={menu.items}
    onclose={closeMenu}
  />
{/if}

<style>
  .panel {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }

  .search {
    display: flex;
    align-items: center;
    height: 26px;
    margin: 6px 8px 0;
    padding-right: 2px;
    background: var(--input-background);
    border: 1px solid var(--control-border);
    border-radius: 6px;
  }

  .search:focus-within {
    border-color: var(--focus);
    box-shadow: 0 0 0 1px var(--focus);
  }

  .search input {
    flex: 1;
    min-width: 0;
    padding: 0 6px;
    font: inherit;
    color: var(--text);
    background: none;
    border: 0;
    outline: none;
    appearance: none;
  }

  .search input::-webkit-search-cancel-button {
    display: none;
  }

  .search .action {
    width: 22px;
    height: 22px;
  }

  .filters {
    display: flex;
    flex: none;
    gap: 4px;
    align-items: center;
    height: 34px;
    padding: 0 8px;
    overflow-x: auto;
    border-bottom: 1px solid var(--divider);
  }

  .quick {
    position: relative;
  }

  .quick.set::after {
    position: absolute;
    top: 3px;
    right: 3px;
    width: 6px;
    height: 6px;
    content: "";
    background: var(--accent);
    border-radius: 50%;
  }

  .chip {
    display: inline-flex;
    flex: none;
    align-items: center;
    max-width: 200px;
    height: 24px;
    border-radius: 6px;
  }

  .chip.active {
    background: var(--selection);
  }

  .chip button {
    display: inline-flex;
    gap: 2px;
    align-items: center;
    height: 100%;
    padding: 0 6px;
    overflow: hidden;
    font: inherit;
    color: var(--text);
    white-space: nowrap;
    text-overflow: ellipsis;
    background: none;
    border: 0;
    border-radius: 6px;
  }

  .chip button:hover {
    background: var(--hover);
  }

  .outdated {
    display: flex;
    gap: 6px;
    align-items: center;
    padding: 4px 8px;
    color: var(--text-secondary);
    border-bottom: 1px solid var(--divider);
  }

  .progress {
    flex: none;
    height: 2px;
    background: linear-gradient(90deg, transparent, var(--focus), transparent);
    background-size: 50% 100%;
    animation: stripe 1.2s linear infinite;
  }

  @keyframes stripe {
    from {
      background-position: -50% 0;
    }

    to {
      background-position: 150% 0;
    }
  }

  .status {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: 4px;
    align-items: center;
    justify-content: center;
    padding: 16px;
    color: var(--text-secondary);
    text-align: center;
  }

  .status p {
    margin: 0;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .status.error strong {
    color: var(--text);
    font-weight: normal;
  }

  .link {
    padding: 0;
    font: inherit;
    color: var(--link);
    cursor: pointer;
    background: none;
    border: 0;
  }

  .rows {
    flex: 1;
    min-height: 0;
    margin: 0;
    padding: 0;
    overflow-y: auto;
    list-style: none;
    outline: none;
  }

  .row {
    padding: 5px 10px;
    cursor: default;
  }

  .row[aria-selected="true"] {
    background: var(--selection-inactive);
  }

  .rows:focus .row[aria-selected="true"] {
    background: var(--selection);
  }

  .line {
    display: flex;
    gap: 6px;
    align-items: center;
    min-width: 0;
  }

  .title {
    overflow: hidden;
    color: var(--text);
    white-space: nowrap;
    text-overflow: ellipsis;
    unicode-bidi: isolate;
  }

  .labels {
    display: inline-flex;
    flex: none;
  }

  .end {
    display: inline-flex;
    flex: none;
    gap: 6px;
    align-items: center;
    margin-left: auto;
  }

  .tag {
    padding: 0 4px;
    font-size: 11px;
    color: var(--text-secondary);
    border: 1px solid var(--control-border);
    border-radius: 4px;
  }

  .avatars {
    display: inline-flex;
  }

  .avatar {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 18px;
    height: 18px;
    font-size: 9px;
    color: var(--text-strong);
    background: var(--pressed);
    border: 1px solid var(--panel);
    border-radius: 50%;
  }

  .avatar + .avatar {
    margin-left: -5px;
  }

  .avatar.accepted {
    border-color: var(--status-added);
  }

  .avatar.wait_for_updates {
    border-color: var(--status-deleted);
  }

  .avatar.need_review {
    border-color: var(--muted);
  }

  .comments {
    display: inline-flex;
    gap: 2px;
    align-items: center;
    color: var(--text-secondary);
  }

  .info {
    margin-top: 2px;
    overflow: hidden;
    font-size: 11px;
    color: var(--text-secondary);
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .tip {
    position: fixed;
    z-index: 20;
    max-width: 320px;
    padding: 6px 8px;
    color: var(--text);
    background: var(--popup-background);
    border: 1px solid var(--popup-border);
    border-radius: 6px;
  }

  .tip-header {
    margin-bottom: 4px;
    color: var(--text-secondary);
  }

  .tip ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .tip li {
    display: flex;
    gap: 6px;
    align-items: center;
    overflow-wrap: anywhere;
  }

  .swatch {
    flex: none;
    width: 10px;
    height: 10px;
    border: 1px solid var(--control-border);
    border-radius: 2px;
  }
</style>
