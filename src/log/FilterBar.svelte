<script lang="ts">
  import { onMount } from "svelte";
  import {
    logRecentFilters,
    logRememberFilter,
    type LogRef,
    type Person,
    type RecentFilters,
  } from "../lib/api";
  import Dialog from "../lib/Dialog.svelte";
  import Icon from "../lib/Icon.svelte";
  import Menu, { type MenuAction, type MenuItem } from "../lib/Menu.svelte";
  import { favorites, toggleFavorite } from "../branches/state";
  import {
    branchCompletionPrefix,
    branchMenu,
    branchValues,
    collectUsers,
    completionPrefix,
    dateText,
    displayText,
    emptyFilters,
    lastDay,
    lastWeek,
    localizedUsers,
    ME,
    parseUsers,
    parseValues,
    pathsText,
    splitBranchValues,
    userLabel,
    type FilterState,
  } from "./filters";
  import ValuesPopup from "./ValuesPopup.svelte";

  type ValueFilter = "branches" | "users" | "paths";

  interface Props {
    filters?: FilterState;
    refs: LogRef[];
    users: Person[];
    // A result is on its way: the log keeps showing the previous view.
    busy?: boolean;
    onchange: (filters: FilterState) => void;
    onrefresh?: () => void;
    onviewoptions?: (anchor: HTMLElement) => void;
    ongoto?: () => void;
  }

  let {
    filters = $bindable(emptyFilters()),
    refs,
    users,
    busy = false,
    onchange,
    onrefresh,
    onviewoptions,
    ongoto,
  }: Props = $props();

  const SELECT = "Select…";
  const RECENT = "Recent";
  const TEXT_FILTER = "Filter by Commit Message or Hashes";
  const TEXT_HISTORY = "Text";
  const DOUBLE_LENGTH = 40;
  const hint = (separators: string) =>
    `Select one or more values separated with ${separators}, use ⌘⏎ to finish`;

  let bar: HTMLDivElement;
  let textField: HTMLInputElement;
  let draft = $derived(filters.text);
  let recents = $state.raw<RecentFilters>({});
  let menu = $state.raw<{
    x: number;
    y: number;
    label: string;
    items: MenuItem[];
    typing?: MenuItem[];
  } | null>(null);
  let editing = $state.raw<{
    x: number;
    y: number;
    name: string;
    filter: ValueFilter;
    value: string;
  } | null>(null);
  let period = $state<{ after: string; before: string } | null>(null);
  let anchor: HTMLElement;

  // Recents are a help: without them, the filter works the same.
  function remember(filter: string, values: string[]) {
    logRememberFilter(filter, values).then(
      (recent) => (recents = recent),
      () => {},
    );
  }

  onMount(() => {
    logRecentFilters().then(
      (recent) => (recents = recent),
      () => {},
    );
  });

  function update(change: Partial<FilterState>) {
    filters = { ...filters, ...change };
    onchange(filters);
  }

  // `TextFilterField.applyFilter`. With `isSameFilterAs`, two blank texts are the same.
  function applyText(addToHistory = false) {
    const text = draft.trim() ? draft : "";
    if (text !== filters.text) update({ text });
    if (addToHistory && text) remember(TEXT_HISTORY, [text]);
  }

  function toggleOption(option: "regex" | "matchCase") {
    const change = { [option]: !filters[option] };
    if (filters.text) update(change);
    else filters = { ...filters, ...change };
  }

  function showMenu(label: string, content: { items: MenuItem[]; typing?: MenuItem[] }) {
    const bounds = anchor.getBoundingClientRect();
    menu = { x: bounds.left, y: bounds.bottom, label, ...content };
  }

  function chooseValues(name: string, filter: ValueFilter, values: string[], addToRecent = true) {
    update(filter === "branches" ? splitBranchValues(values) : { [filter]: values });
    if (addToRecent) remember(name, values);
  }

  function valueItem(
    name: string,
    filter: ValueFilter,
    values: string[],
    label = displayText(localizedUsers(values), DOUBLE_LENGTH),
    addToRecent = true,
  ): MenuAction {
    return { label, run: () => chooseValues(name, filter, values, addToRecent) };
  }

  function selectItem(name: string, filter: ValueFilter, value: () => string[]): MenuAction {
    return {
      label: SELECT,
      run: () => {
        const bounds = anchor.getBoundingClientRect();
        editing = {
          x: bounds.left,
          y: bounds.bottom,
          name,
          filter,
          value: value().join("\n"),
        };
      },
    };
  }

  // `BranchLogSpeedSearchPopup`: when typing, the groups open up and stop being groups.
  function branchItems() {
    const menu = branchMenu(refs, recents.Branch ?? [], $favorites);
    const { recent, groups } = menu;
    const item = (values: string[], label?: string, addToRecent = true) =>
      valueItem("Branch", "branches", values, label, addToRecent);
    // `BranchFilterAction`: a branch carries a star; the rest of the menu does not.
    const branch = (name: string): MenuAction => {
      const ref = refs.find((other) => other.name === name);
      if (ref?.kind !== "local" && ref?.kind !== "remote") return item([name], name);
      const { kind } = ref;
      const star = {
        on: menu.favorites.includes(name),
        toggle: () => toggleFavorite(name, kind).catch(() => {}),
      };
      return { ...item([name], name), star };
    };
    const top: MenuItem[] = [selectItem("Branch", "branches", () => branchValues(filters))];
    const direct: MenuItem[] = [];
    if (menu.favorites.length > 1) direct.push(item(menu.favorites, "Favorites", false));
    direct.push(...menu.favorites.map(branch));
    const nested = groups.map(({ label, values }) => ({
      label,
      submenu: values.map(branch),
    }));
    const recentGroup = { label: RECENT, submenu: recent.map((values) => item(values)) };
    const opened = recent.length > 0 ? [recentGroup, ...nested] : nested;
    return {
      items: [
        ...top,
        ...(recent.length > 0 ? [recentGroup] : []),
        ...direct,
        ...(nested.length > 0 ? [null] : []),
        ...nested,
      ],
      typing: [
        ...top,
        ...direct,
        ...opened.flatMap(({ label, submenu }) => [{ header: label }, ...submenu]),
      ],
    };
  }

  // `UserLogSpeedSearchPopup`: users only appear when typing, and then the single-value recents are
  // redundant, since they are already in the list.
  function userItems() {
    const item = (values: string[], label?: string) => valueItem("User", "users", values, label);
    const select = selectItem("User", "users", () => localizedUsers(filters.users));
    const recent = recents.User ?? [];
    const several = recent.filter((values) => values.length > 1);
    const header = { header: RECENT };
    return {
      items: [
        select,
        item([ME]),
        ...(recent.length > 0 ? [header, ...recent.map((values) => item(values))] : []),
      ],
      typing: [
        select,
        ...(several.length > 0 ? [header, ...several.map((values) => item(values))] : []),
        item([ME]),
        null,
        ...collectUsers(users).map((name) => item([name], name)),
      ],
    };
  }

  const day = (seconds: number | null) => {
    if (seconds === null) return "";
    const date = new Date(seconds * 1000);
    const month = String(date.getMonth() + 1).padStart(2, "0");
    return `${date.getFullYear()}-${month}-${String(date.getDate()).padStart(2, "0")}`;
  };

  const dayStart = (text: string) => {
    if (!text) return null;
    const [year, month, date] = text.split("-").map(Number);
    return new Date(year, month - 1, date).getTime() / 1000;
  };

  function dateItems() {
    const since = (value: number) => () => update({ since: value, until: null });
    const now = new Date();
    return {
      items: [
        {
          label: SELECT,
          run: () => (period = { after: day(filters.since), before: day(filters.until) }),
        },
        { label: "Last 24 hours", run: since(lastDay(now)) },
        { label: "Last 7 days", run: since(lastWeek(now)) },
      ],
    };
  }

  function pathItems() {
    const recent = recents.Paths ?? [];
    const items: MenuItem[] = [selectItem("Paths", "paths", () => filters.paths)];
    if (recent.length > 0) {
      items.push({ header: RECENT });
      items.push(...recent.map((values) => valueItem("Paths", "paths", values, pathsText(values))));
    }
    return { items };
  }

  const gearItems = () => ({
    items: [
      { label: "Regex", checked: filters.regex, run: () => toggleOption("regex") },
      { label: "Match Case", checked: filters.matchCase, run: () => toggleOption("matchCase") },
    ],
  });

  function historyItems() {
    const items = (recents[TEXT_HISTORY] ?? []).map(([text]) => ({
      label: text,
      run: () => {
        draft = text;
        applyText();
      },
    }));
    return { items };
  }

  const chips = $derived([
    {
      name: "Branch",
      text: displayText(branchValues(filters)),
      items: branchItems,
      reset: { branches: [], revisions: [], ranges: [] },
    },
    {
      name: "User",
      text: displayText(localizedUsers(filters.users)),
      items: userItems,
      reset: { users: [] },
    },
    {
      name: "Date",
      text: dateText(filters.since, filters.until),
      items: dateItems,
      reset: { since: null, until: null },
    },
    {
      name: "Paths",
      text: pathsText(filters.paths),
      items: pathItems,
      reset: { paths: [] },
    },
  ]);

  function open(element: EventTarget | null, label: string, items: () => { items: MenuItem[] }) {
    anchor = element as HTMLElement;
    const content = items();
    if (content.items.length > 0) showMenu(label, content);
  }

  function textKeydown(event: KeyboardEvent) {
    if (event.key === "Enter") applyText(true);
    else if (event.key === "ArrowDown") {
      event.preventDefault();
      open(event.currentTarget, "Search History", historyItems);
    }
  }

  // `MultipleValueFilterPopupComponent.SelectMultipleValuesAction`.
  function finishEditing(text: string) {
    if (!editing) return;
    const { name, filter } = editing;
    const paths = filter === "paths";
    const parsed = parseValues(text, paths ? ["\n"] : undefined);
    const values = filter === "users" ? parseUsers(parsed) : parsed;
    editing = null;
    if (values.length > 0) chooseValues(name, filter, values);
    else update(filter === "branches" ? splitBranchValues([]) : { [filter]: [] });
  }

  // `getAllValues`. Paths are not completed: `EditPathsAction` uses a field without completion.
  const variantsOf = (filter: ValueFilter) => {
    if (filter === "users") return [userLabel(ME), ...collectUsers(users)];
    if (filter === "paths") return undefined;
    return refs.filter((ref) => ref.kind !== "tag").map((ref) => ref.name);
  };

  const prefixOf = (filter: ValueFilter) =>
    filter === "branches" ? branchCompletionPrefix : completionPrefix;

  function finishPeriod() {
    if (!period) return;
    const { after, before } = period;
    period = null;
    update({ since: dayStart(after), until: dayStart(before) });
  }

  export function focusText() {
    textField.focus();
  }

  export const hasFocus = () => bar.contains(document.activeElement);
</script>

<div bind:this={bar} class="bar" role="toolbar" aria-label="Log filters">
  <div class="search">
    <button
      type="button"
      class="action"
      aria-label="Search History"
      onclick={(event) => open(event.currentTarget, "Search History", historyItems)}
    >
      <Icon name="history" size={14} />
    </button>
    <input
      bind:this={textField}
      type="search"
      aria-label={TEXT_FILTER}
      title={TEXT_FILTER}
      placeholder="Text or hash"
      spellcheck="false"
      bind:value={draft}
      onkeydown={textKeydown}
      onblur={() => applyText()}
    />
    {#if draft}
      <button
        type="button"
        class="action"
        aria-label="Clear"
        onclick={() => {
          draft = "";
          applyText();
        }}
      >
        <Icon name="close" size={14} />
      </button>
    {/if}
    <button
      type="button"
      class="action"
      aria-label="Text filter settings"
      onclick={(event) => open(event.currentTarget, "Text filter settings", gearItems)}
    >
      <Icon name="settings" size={14} />
    </button>
  </div>

  {#each chips as chip (chip.name)}
    {@const label = chip.text ? `${chip.name}: ${chip.text}` : chip.name}
    <span class="chip" class:active={chip.text}>
      <button
        type="button"
        aria-label={label}
        title={chip.text || undefined}
        onclick={(event) => open(event.currentTarget, chip.name, chip.items)}
      >
        {label}
        {#if !chip.text}
          <Icon name="chevronDown" size={12} />
        {/if}
      </button>
      {#if chip.text}
        <button
          type="button"
          aria-label="Remove {chip.name} filter"
          onclick={() => update(chip.reset)}
        >
          <Icon name="close" size={12} />
        </button>
      {/if}
    </span>
  {/each}

  <span class="end">
    {#if busy}
      <span class="busy" role="status" aria-label="Loading commits…"></span>
    {/if}
    <button type="button" class="action" aria-label="Refresh" onclick={() => onrefresh?.()}>
      <Icon name="refresh" />
    </button>
    <button
      type="button"
      class="action"
      aria-label="View Options"
      onclick={(event) => onviewoptions?.(event.currentTarget)}
    >
      <Icon name="show" />
    </button>
    <button
      type="button"
      class="action"
      aria-label="Go to Hash/Branch/Tag"
      onclick={() => ongoto?.()}
    >
      <Icon name="right" />
    </button>
  </span>
</div>

{#if menu}
  <Menu {...menu} onclose={() => (menu = null)} />
{/if}

{#if editing}
  <ValuesPopup
    x={editing.x}
    y={editing.y}
    label={editing.name}
    value={editing.value}
    hint={hint(editing.filter === "paths" ? "new lines" : "| or new lines")}
    variants={variantsOf(editing.filter)}
    prefixAt={prefixOf(editing.filter)}
    onok={finishEditing}
    onclose={() => (editing = null)}
  />
{/if}

{#if period}
  <Dialog title="Select Period" onclose={() => (period = null)} onsubmit={finishPeriod}>
    <label class="field">After: <input class="input" type="date" bind:value={period.after} /></label>
    <label class="field">
      Before: <input class="input" type="date" bind:value={period.before} />
    </label>
    {#snippet footer()}
      <button type="button" class="button" onclick={() => (period = null)}>Cancel</button>
      <button type="submit" class="button default">OK</button>
    {/snippet}
  </Dialog>
{/if}

<style>
  .bar {
    display: flex;
    gap: 6px;
    align-items: center;
    height: 36px;
    padding: 0 8px;
    overflow: hidden;
    background: var(--panel);
    border-bottom: 1px solid var(--divider);
  }

  .search {
    display: flex;
    flex: 0 1 260px;
    align-items: center;
    min-width: 120px;
    height: 26px;
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

  .chip {
    display: inline-flex;
    flex: none;
    align-items: center;
    max-width: 240px;
    height: 26px;
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
    background: none;
    border: 0;
    border-radius: 6px;
  }

  .chip button:hover {
    background: var(--divider);
  }

  .end {
    display: inline-flex;
    gap: 2px;
    align-items: center;
    margin-left: auto;
  }

  .busy {
    width: 12px;
    height: 12px;
    margin-right: 6px;
    border: 2px solid var(--control-border);
    border-top-color: var(--focus);
    border-radius: 50%;
    animation: spin 0.9s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .field {
    display: flex;
    gap: 8px;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 8px;
  }
</style>
