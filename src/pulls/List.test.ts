import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { tick } from "svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { PullRequestPage, PullRequestShort, SavedSearch, SearchValue } from "../lib/api";
import { taskResult } from "../lib/tasks";
import { loginFor } from "../hosting/accounts";
import { dismiss, notifications } from "../lib/notifications";
import { get } from "svelte/store";
import { account, connection } from "./fixtures";
import List from "./List.svelte";
import { POLL_INTERVAL } from "./poll";
import { EMPTY_SEARCH } from "./search";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("../lib/tasks", () => ({ taskResult: vi.fn() }));
vi.mock("../hosting/accounts", async (original) => ({
  ...(await original<typeof import("../hosting/accounts")>()),
  loginFor: vi.fn(),
}));

const search = (value: Partial<SearchValue>): SearchValue => ({ ...EMPTY_SEARCH, ...value });

function pr(number: number, extra: Partial<PullRequestShort> = {}): PullRequestShort {
  return {
    number,
    title: `Pull request ${number}`,
    url: `https://github.com/o/r/pull/${number}`,
    state: "OPEN",
    isDraft: false,
    mergeable: "MERGEABLE",
    author: "hubot",
    createdAt: new Date(2020, 0, 2, 10, 0, 0).toISOString(),
    labels: [],
    assignees: [],
    reviewers: [],
    unresolved: 0,
    ...extra,
  };
}

const page = (items: PullRequestShort[], endCursor: string | null = null): PullRequestPage => ({
  items,
  endCursor,
  hasNext: endCursor !== null,
});

let saved: SavedSearch;
// What each page request answers, in order: a page, or an error.
let pages: (PullRequestPage | { kind: string; message: string })[];
let etags: (string | null | { kind: string; message: string })[];
let ids = 0;
const answers = new Map<number, PullRequestPage | string[] | { kind: string; message: string }>();
// A chooser task that answers only when the test says so: `held` keeps how to reject it.
let chooserHeld = false;
const held = new Map<number, (error: unknown) => void>();

const calls = (command: string) => vi.mocked(invoke).mock.calls.filter(([name]) => name === command);
const asked = () => calls("github_pr_page").map(([, args]) => args as { value: SearchValue; cursor: string | null });

beforeEach(() => {
  saved = { last: null, history: [] };
  pages = [];
  etags = [];
  ids = 0;
  answers.clear();
  chooserHeld = false;
  held.clear();
  vi.mocked(invoke).mockReset();
  vi.mocked(taskResult).mockReset();
  vi.mocked(loginFor).mockReset();
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    switch (command) {
      case "github_pr_search_saved":
        return saved;
      case "github_pr_search_save": {
        const { value, remember } = args as { value: SearchValue; remember: boolean };
        saved = { last: value, history: remember ? [...saved.history, value] : saved.history };
        return saved;
      }
      case "github_pr_page": {
        const id = ++ids;
        answers.set(id, pages.shift() ?? page([]));
        return id;
      }
      case "github_pr_etag": {
        const answer = etags.shift() ?? null;
        if (answer !== null && typeof answer === "object") throw answer;
        return answer;
      }
      case "github_pr_chooser": {
        const id = ++ids;
        answers.set(id, ["alice", "bob"]);
        if (chooserHeld) held.set(id, () => {});
        return id;
      }
      case "task_cancel":
      case "open_url":
      case "clipboard_write":
        return undefined;
      default:
        throw new Error(`unexpected command: ${command}`);
    }
  });
  vi.mocked(taskResult).mockImplementation(async (id) => {
    if (held.has(id)) return new Promise((_, reject) => held.set(id, reject));
    const answer = answers.get(id);
    if (answer && "kind" in answer) throw answer;
    return answer;
  });
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  for (const shown of get(notifications)) dismiss(shown.id);
});

function show(props: Partial<{ active: boolean; focused: boolean; onopen: (number: number) => void }> = {}) {
  return render(List, {
    connection: connection("origin", "github.com/octocat"),
    account: account("octocat"),
    active: true,
    focused: true,
    onopen: () => {},
    ...props,
  });
}

const rows = () => screen.queryAllByRole("option");
const labels = (menu: HTMLElement) =>
  within(menu)
    .getAllByRole("menuitem")
    .map((item) => item.getAttribute("aria-label"));
const filters = () => screen.getByRole("toolbar", { name: "Pull request filters" });

test("the_default_search_is_assigned_to_you_unless_one_was_saved", async () => {
  show();
  await waitFor(() => expect(asked()).toHaveLength(1));
  expect(asked()[0]).toEqual({ value: search({ state: "OPEN", assignee: "octocat" }), cursor: null });
  // The dropdowns show the values.
  expect(within(filters()).getByRole("button", { name: "State: Open" })).toBeTruthy();
  expect(within(filters()).getByRole("button", { name: "Assignee: octocat" })).toBeTruthy();
  cleanup();

  saved = { last: search({ author: "bob" }), history: [] };
  vi.mocked(invoke).mockClear();
  show();
  await waitFor(() => expect(asked()).toHaveLength(1));
  expect(asked()[0].value).toEqual(search({ author: "bob" }));
});

test("clear_filters_sets_the_empty_search", async () => {
  show();
  await waitFor(() => expect(asked()).toHaveLength(1));
  const quick = within(filters()).getByRole("button", { name: "Quick Filters" });
  // The badge says filters are set.
  expect(quick.classList.contains("set")).toBe(true);
  await fireEvent.click(quick);
  const menu = screen.getByRole("menu", { name: "Quick Filters" });
  expect(labels(menu)).toEqual(["Open", "Your pull requests", "Assigned to you", "Review requests", "Clear 2 Filters"]);
  // The current search is checked.
  const ticks = within(menu)
    .getAllByRole("menuitem")
    .map((item) => item.querySelector(".tick")?.textContent ?? null);
  expect(ticks).toEqual(["", "", "✓", "", null]);
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Clear 2 Filters" }));
  await waitFor(() => expect(asked()).toHaveLength(2));
  expect(asked()[1].value).toEqual(EMPTY_SEARCH);
  expect(calls("github_pr_search_save").at(-1)?.[1]).toEqual({ value: EMPTY_SEARCH, remember: false });

  // With no filter there is nothing to clear.
  await fireEvent.click(within(filters()).getByRole("button", { name: "Quick Filters" }));
  const again = screen.getByRole("menu", { name: "Quick Filters" });
  expect(within(again).queryByRole("menuitem", { name: /Clear/ })).toBeNull();
  await fireEvent.click(within(again).getByRole("menuitem", { name: "Review requests" }));
  await waitFor(() => expect(asked()).toHaveLength(3));
  expect(asked()[2].value).toEqual(search({ state: "OPEN", review: "AWAITING_REVIEW" }));
});

test("the_filter_dropdowns_are_in_241s_order_with_their_values", async () => {
  saved = { last: EMPTY_SEARCH, history: [] };
  show();
  await waitFor(() => expect(asked()).toHaveLength(1));
  const names = within(filters())
    .getAllByRole("button")
    .map((button) => button.getAttribute("aria-label"));
  expect(names).toEqual(["Quick Filters", "State", "Author", "Label", "Assignee", "Review"]);

  await fireEvent.click(within(filters()).getByRole("button", { name: "State" }));
  let menu = screen.getByRole("menu", { name: "State" });
  expect(labels(menu)).toEqual(["Open", "Closed", "Merged"]);
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Merged" }));
  await waitFor(() => expect(asked()).toHaveLength(2));
  expect(asked()[1].value).toEqual(search({ state: "MERGED" }));

  // The popup has the full texts, the dropdown the short one.
  await fireEvent.click(within(filters()).getByRole("button", { name: "Review" }));
  menu = screen.getByRole("menu", { name: "Review" });
  expect(labels(menu)).toEqual([
    "No reviews",
    "Review required",
    "Approved review",
    "Changes requested",
    "Reviewed by you",
    "Not reviewed by you",
    "Awaiting review from you",
  ]);
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Reviewed by you" }));
  expect(await within(filters()).findByRole("button", { name: "Review: Reviewed" })).toBeTruthy();

  // An async chooser: the logins of the collaborators.
  await fireEvent.click(within(filters()).getByRole("button", { name: "Author" }));
  menu = await screen.findByRole("menu", { name: "Author" });
  await waitFor(() =>
    expect(labels(menu)).toEqual(["alice", "bob"]),
  );
  expect(calls("github_pr_chooser")[0][1]).toEqual({ kind: "collaborators" });
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "bob" }));
  await waitFor(() => expect(asked().at(-1)?.value).toEqual(search({ state: "MERGED", review: "REVIEWED_BY_ME", author: "bob" })));

  // The reset ×, and Delete on a set dropdown.
  await fireEvent.click(within(filters()).getByRole("button", { name: "Remove Author filter" }));
  await waitFor(() => expect(asked().at(-1)?.value.author).toBeNull());
  await fireEvent.keyDown(within(filters()).getByRole("button", { name: "State: Merged" }), { key: "Delete" });
  await waitFor(() => expect(asked().at(-1)?.value).toEqual(search({ review: "REVIEWED_BY_ME" })));
});

test("the_search_field_applies_on_enter_and_has_its_history", async () => {
  saved = {
    last: EMPTY_SEARCH,
    history: [search({ text: "old" }), search({ state: "CLOSED", label: "bug" })],
  };
  show();
  await waitFor(() => expect(asked()).toHaveLength(1));
  const field = screen.getByRole("searchbox", { name: "Search" });
  await fireEvent.input(field, { target: { value: "crash" } });
  await tick();
  expect(asked()).toHaveLength(1);
  await fireEvent.keyDown(field, { key: "Enter" });
  await waitFor(() => expect(asked()).toHaveLength(2));
  expect(asked()[1].value).toEqual(search({ text: "crash" }));
  // A new text goes into the history at once.
  expect(calls("github_pr_search_save").at(-1)?.[1]).toEqual({ value: search({ text: "crash" }), remember: true });

  // The history, newest first, in 241's short text.
  await fireEvent.click(screen.getByRole("button", { name: "Search History" }));
  const menu = screen.getByRole("menu", { name: "Search History" });
  expect(labels(menu)).toEqual([
    '"crash"',
    'state:"Closed" label:bug',
    '"old"',
  ]);
  await fireEvent.click(within(menu).getByRole("menuitem", { name: '"old"' }));
  await waitFor(() => expect(asked()).toHaveLength(3));
  expect(asked()[2].value).toEqual(search({ text: "old" }));
  expect((field as HTMLInputElement).value).toBe("old");

  // An empty text is no text.
  await fireEvent.input(field, { target: { value: "" } });
  await fireEvent.keyDown(field, { key: "Enter" });
  await waitFor(() => expect(asked()).toHaveLength(4));
  expect(asked()[3].value).toEqual(EMPTY_SEARCH);
});

test("rows_show_state_tags_conflict_icon_and_comment_count_even_at_zero", async () => {
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(new Date(2026, 10, 9, 15, 0, 0));
  pages = [
    page([
      pr(1, { createdAt: new Date(2026, 10, 9, 9, 0, 0).toISOString() }),
      pr(2, { state: "CLOSED", isDraft: true, mergeable: "CONFLICTING", unresolved: 3, author: null,
        createdAt: new Date(2026, 10, 8, 9, 0, 0).toISOString() }),
      pr(3, {
        state: "MERGED",
        unresolved: 1,
        labels: [{ name: "bug", color: "d73a4a" }, { name: "docs", color: null }],
        assignees: ["Alice A", "bob", "carol"],
        reviewers: [
          { name: "Need", state: "NEED_REVIEW" },
          { name: "Waits", state: "WAIT_FOR_UPDATES" },
          { name: "Okay", state: "ACCEPTED" },
        ],
      }),
      pr(4, { state: "CLOSED" }),
    ]),
  ];
  show();
  await waitFor(() => expect(rows()).toHaveLength(4));
  const [open, draft, merged, closed] = rows();

  // An open PR has no tag; a draft says DRAFT over its state.
  expect(within(open).queryByText(/^(OPEN|CLOSED|MERGED|DRAFT)$/)).toBeNull();
  expect(within(draft).getByText("DRAFT")).toBeTruthy();
  expect(within(merged).getByText("MERGED")).toBeTruthy();
  expect(within(closed).getByText("CLOSED")).toBeTruthy();

  expect(within(draft).getByTitle("Pull request cannot be merged")).toBeTruthy();
  expect(within(open).queryByTitle("Pull request cannot be merged")).toBeNull();

  // The comment counter is always there.
  expect(within(open).getByTitle("No unresolved comments").textContent?.trim()).toBe("0");
  expect(within(merged).getByTitle("1 unresolved comment").textContent?.trim()).toBe("1");
  expect(within(draft).getByTitle("3 unresolved comments").textContent?.trim()).toBe("3");

  // Line 2.
  expect(within(open).getByText("#1 · created Today, by hubot")).toBeTruthy();
  expect(within(draft).getByText("#2 · created Yesterday")).toBeTruthy();

  // Two avatars of each group at most, initials; the reviewers in 241's order.
  const assignees = within(merged).getByRole("group", { name: "Assignees" });
  expect(within(assignees).getAllByRole("img").map((avatar) => avatar.textContent)).toEqual(["AA", "B"]);
  const reviewers = within(merged).getByRole("group", { name: "Reviewers" });
  expect(within(reviewers).getAllByRole("img").map((avatar) => avatar.getAttribute("aria-label"))).toEqual([
    "Okay",
    "Waits",
  ]);
  await fireEvent.mouseEnter(assignees);
  let tip = screen.getByRole("tooltip");
  expect(within(tip).getAllByRole("listitem").map((item) => item.textContent?.trim())).toEqual([
    "Alice A",
    "bob",
    "carol",
  ]);
  expect(within(tip).getByText("Assignees")).toBeTruthy();
  await fireEvent.mouseLeave(assignees);
  await fireEvent.mouseEnter(reviewers);
  tip = screen.getByRole("tooltip");
  expect(within(tip).getByText("Reviewers")).toBeTruthy();
  expect(within(tip).getAllByRole("listitem").map((item) => item.textContent?.trim())).toEqual([
    "Okay",
    "Waits",
    "Need",
  ]);
});

test("a_title_with_html_is_text_and_a_bad_label_colour_is_dropped", async () => {
  const title = '<img src=x onerror="window.hacked = 1">';
  pages = [
    page([
      pr(5, {
        title,
        labels: [
          { name: "<b>bold</b>", color: "d73a4a" },
          // The backend drops a bad colour; one that came through anyway is not used either.
          { name: "evil", color: "red;background:url(x)" },
        ],
      }),
    ]),
  ];
  show();
  await waitFor(() => expect(rows()).toHaveLength(1));
  const [row] = rows();
  expect(row.querySelector("img")).toBeNull();
  expect(within(row).getByText(title)).toBeTruthy();
  const labels = within(row).getByRole("img", { name: "Labels" });
  await fireEvent.mouseEnter(labels);
  const tip = screen.getByRole("tooltip");
  expect(within(tip).getByText("Labels")).toBeTruthy();
  const [bold, evil] = within(tip).getAllByRole("listitem");
  expect(bold.textContent?.trim()).toBe("<b>bold</b>");
  expect(bold.querySelector("b")).toBeNull();
  expect((bold.querySelector(".swatch") as HTMLElement).style.background).toBe("rgb(215, 58, 74)");
  expect((evil.querySelector(".swatch") as HTMLElement).getAttribute("style") ?? "").toBe("");
});

function scrollable(list: HTMLElement, height: number, client: number, top: number) {
  Object.defineProperty(list, "scrollHeight", { configurable: true, value: height });
  Object.defineProperty(list, "clientHeight", { configurable: true, value: client });
  Object.defineProperty(list, "scrollTop", { configurable: true, writable: true, value: top });
}

test("pages_load_at_seventy_percent_and_stop_after_an_error", async () => {
  pages = [
    page([pr(1), pr(2)], "C1"),
    page([pr(3)], "C2"),
    { kind: "failed", message: "502 Bad Gateway - oops" },
  ];
  show();
  await waitFor(() => expect(rows()).toHaveLength(2));
  const list = screen.getByRole("listbox", { name: "Pull requests" });
  // 60% scrolled: not yet.
  scrollable(list, 1000, 400, 200);
  await fireEvent.scroll(list);
  await tick();
  expect(asked()).toHaveLength(1);
  // 70%: the next page, with the cursor.
  scrollable(list, 1000, 400, 300);
  await fireEvent.scroll(list);
  await waitFor(() => expect(rows()).toHaveLength(3));
  expect(asked()[1].cursor).toBe("C1");
  // Still at the end after it is added: the next one at once, which fails.
  await waitFor(() => expect(asked()).toHaveLength(3));
  expect(asked()[2].cursor).toBe("C2");
  expect(await screen.findByText("Can't load pull requests")).toBeTruthy();
  // No more after an error, until Retry.
  await fireEvent.scroll(list);
  await tick();
  expect(asked()).toHaveLength(3);
  pages = [page([pr(9)])];
  await fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  await waitFor(() => expect(rows()).toHaveLength(1));
  expect(asked()[3].cursor).toBeNull();
});

test("errors_show_cant_load_with_re_login_for_auth_and_retry_otherwise", async () => {
  pages = [{ kind: "authentication", message: "Request response: Bad credentials." }];
  show();
  expect(await screen.findByText("Can't load pull requests")).toBeTruthy();
  expect(screen.getByText("User authorization failed")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
  // Re-Login logs the connection's account in again, then reloads.
  vi.mocked(loginFor).mockResolvedValueOnce(account("octocat"));
  pages = [{ kind: "failed", message: "Could not resolve to a Repository with the name 'o/r'. Either…" }];
  await fireEvent.click(screen.getByRole("button", { name: "Re-Login" }));
  expect(loginFor).toHaveBeenCalledWith("github.com", "github.com/octocat");
  expect(await screen.findByText("Could not resolve to a Repository with the name 'o/r'. Either…")).toBeTruthy();
  expect(asked()).toHaveLength(2);
  // Anything else: Retry.
  expect(screen.queryByRole("button", { name: "Re-Login" })).toBeNull();
  pages = [page([pr(1)])];
  await fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  await waitFor(() => expect(rows()).toHaveLength(1));

  // A cancelled login reloads nothing.
  cleanup();
  vi.mocked(invoke).mockClear();
  pages = [{ kind: "authentication", message: "Request response: Bad credentials." }];
  show();
  await fireEvent.click(await screen.findByRole("button", { name: "Re-Login" }));
  await tick();
  expect(asked()).toHaveLength(1);
});

test("empty_says_no_pull_requests_in_repo_or_matching_filters", async () => {
  show();
  expect(await screen.findByText("No pull requests matching filters.")).toBeTruthy();
  await fireEvent.click(screen.getByRole("button", { name: "Clear filters" }));
  await waitFor(() => expect(asked()).toHaveLength(2));
  expect(asked()[1].value).toEqual(EMPTY_SEARCH);
  // The repository's name alone.
  expect(await screen.findByText("No pull requests in r")).toBeTruthy();
});

test("the_list_is_outdated_only_after_a_changed_etag_and_polls_only_while_shown_and_focused", async () => {
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  pages = [page([pr(1)])];
  etags = ["a", "b"];
  const view = show({ active: false });
  await vi.waitFor(() => expect(rows()).toHaveLength(1));
  // Not shown: no poll.
  await vi.advanceTimersByTimeAsync(POLL_INTERVAL * 2);
  expect(calls("github_pr_etag")).toHaveLength(0);
  await view.rerender({ active: true, focused: false });
  await vi.advanceTimersByTimeAsync(POLL_INTERVAL * 2);
  expect(calls("github_pr_etag")).toHaveLength(0);

  await view.rerender({ active: true, focused: true });
  await vi.advanceTimersByTimeAsync(POLL_INTERVAL);
  expect(calls("github_pr_etag")).toHaveLength(1);
  expect(screen.queryByText("The list is outdated.")).toBeNull();
  await vi.advanceTimersByTimeAsync(POLL_INTERVAL);
  expect(calls("github_pr_etag")).toHaveLength(2);
  expect(screen.getByText("The list is outdated.")).toBeTruthy();

  // Refresh reloads the list and forgets the baseline.
  pages = [page([pr(1), pr(2)])];
  await fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
  await vi.waitFor(() => expect(rows()).toHaveLength(2));
  expect(screen.queryByText("The list is outdated.")).toBeNull();
  expect(asked()).toHaveLength(2);
});

test("enter_or_a_double_click_opens_and_the_context_menu_is_241s", async () => {
  const onopen = vi.fn();
  pages = [page([pr(1), pr(2, { url: "javascript:alert(1)" })])];
  show({ onopen });
  await waitFor(() => expect(rows()).toHaveLength(2));
  const [first, second] = rows();
  await fireEvent.dblClick(first);
  expect(onopen).toHaveBeenCalledWith(1);
  const list = screen.getByRole("listbox", { name: "Pull requests" });
  await fireEvent.keyDown(list, { key: "ArrowDown" });
  await fireEvent.keyDown(list, { key: "Enter" });
  expect(onopen).toHaveBeenLastCalledWith(2);

  await fireEvent.contextMenu(first);
  let menu = screen.getByRole("menu", { name: "Pull request" });
  expect(labels(menu)).toEqual([
    "View Pull Request",
    "Open Pull Request in Browser",
    "Copy Pull Request URL",
    "Refresh List",
  ]);
  expect(menu.querySelectorAll('[role="separator"]')).toHaveLength(1);
  expect(within(menu).getByRole("menuitem", { name: "Refresh List" }).textContent).toContain("⌘R");
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Open Pull Request in Browser" }));
  expect(calls("open_url")[0][1]).toEqual({ url: "https://github.com/o/r/pull/1" });

  await fireEvent.contextMenu(first);
  menu = screen.getByRole("menu", { name: "Pull request" });
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Copy Pull Request URL" }));
  expect(calls("clipboard_write")[0][1]).toEqual({ text: "https://github.com/o/r/pull/1" });

  // A URL that is not the connection's server is not opened.
  await fireEvent.contextMenu(second);
  menu = screen.getByRole("menu", { name: "Pull request" });
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Open Pull Request in Browser" }));
  expect(calls("open_url")).toHaveLength(1);
  await waitFor(() => expect(get(notifications)).toHaveLength(1));
  expect(get(notifications)[0].title).toBe("Cannot open in browser");

  await fireEvent.contextMenu(first);
  menu = screen.getByRole("menu", { name: "Pull request" });
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "View Pull Request" }));
  expect(onopen).toHaveBeenLastCalledWith(1);
  await fireEvent.contextMenu(first);
  pages = [page([])];
  await fireEvent.click(within(screen.getByRole("menu", { name: "Pull request" })).getByRole("menuitem", { name: "Refresh List" }));
  await waitFor(() => expect(asked()).toHaveLength(2));
});

test("a_chooser_loads_in_a_task_that_closing_its_dropdown_cancels", async () => {
  saved = { last: EMPTY_SEARCH, history: [] };
  chooserHeld = true;
  show();
  await waitFor(() => expect(asked()).toHaveLength(1));
  await fireEvent.click(within(filters()).getByRole("button", { name: "Label" }));
  const menu = screen.getByRole("menu", { name: "Label" });
  await waitFor(() => expect(calls("github_pr_chooser")).toHaveLength(1));
  expect(calls("github_pr_chooser")[0][1]).toEqual({ kind: "labels" });
  expect(labels(menu)).toEqual(["Loading…"]);
  await fireEvent.keyDown(menu, { key: "Escape" });
  await waitFor(() => expect(screen.queryByRole("menu", { name: "Label" })).toBeNull());
  const id = vi.mocked(taskResult).mock.calls.at(-1)?.[0];
  expect(calls("task_cancel").map(([, args]) => args)).toContainEqual({ id });

  // Opened again, it asks again, and its values are kept until Refresh.
  chooserHeld = false;
  await fireEvent.click(within(filters()).getByRole("button", { name: "Label" }));
  let again = screen.getByRole("menu", { name: "Label" });
  await waitFor(() => expect(labels(again)).toEqual(["alice", "bob"]));
  await fireEvent.keyDown(again, { key: "Escape" });
  await fireEvent.click(within(filters()).getByRole("button", { name: "Label" }));
  again = screen.getByRole("menu", { name: "Label" });
  expect(labels(again)).toEqual(["alice", "bob"]);
  expect(calls("github_pr_chooser")).toHaveLength(2);
});

test("a_pull_request_on_two_pages_is_shown_once", async () => {
  pages = [page([pr(1), pr(2)], "C1"), page([pr(2, { title: "Moved" }), pr(3), pr(3)])];
  show();
  await waitFor(() => expect(rows()).toHaveLength(2));
  const list = screen.getByRole("listbox", { name: "Pull requests" });
  scrollable(list, 1000, 400, 600);
  await fireEvent.scroll(list);
  await waitFor(() => expect(asked()).toHaveLength(2));
  await waitFor(() => expect(rows()).toHaveLength(3));
  expect(rows().map((row) => row.id)).toEqual(["pull-request-1", "pull-request-2", "pull-request-3"]);
  expect(screen.queryByText("Moved")).toBeNull();
});

// After GitHub refuses the token, the poll stops (each poll would read the Keychain again) and the
// list shows 241's error panel with `Re-Login`; the re-login reloads it, and polling starts again.
test("a_refused_poll_stops_polling_and_offers_re_login", async () => {
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  pages = [page([pr(1)])];
  etags = ["a", { kind: "authentication", message: "Request response: Bad credentials." }];
  show();
  await vi.waitFor(() => expect(rows()).toHaveLength(1));
  await vi.advanceTimersByTimeAsync(POLL_INTERVAL * 2);
  expect(calls("github_pr_etag")).toHaveLength(2);
  await vi.waitFor(() => expect(screen.getByText("User authorization failed")).toBeTruthy());
  expect(screen.getByText("Can't load pull requests")).toBeTruthy();

  await vi.advanceTimersByTimeAsync(POLL_INTERVAL * 4);
  expect(calls("github_pr_etag")).toHaveLength(2);

  vi.mocked(loginFor).mockResolvedValueOnce(account("octocat"));
  pages = [page([pr(1), pr(2)])];
  etags = ["c"];
  await fireEvent.click(screen.getByRole("button", { name: "Re-Login" }));
  await vi.waitFor(() => expect(rows()).toHaveLength(2));
  await vi.advanceTimersByTimeAsync(POLL_INTERVAL);
  expect(calls("github_pr_etag")).toHaveLength(3);
});

// A missing token refuses the same way. Any other failure is ignored, as in 241.
test("a_poll_that_fails_otherwise_goes_on_and_shows_nothing", async () => {
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  pages = [page([pr(1)])];
  etags = ["a", { kind: "failed", message: "Server is unreachable" }, "a"];
  show();
  await vi.waitFor(() => expect(rows()).toHaveLength(1));
  await vi.advanceTimersByTimeAsync(POLL_INTERVAL * 3);
  expect(calls("github_pr_etag")).toHaveLength(3);
  expect(screen.queryByRole("alert")).toBeNull();
  expect(rows()).toHaveLength(1);
});

// A chooser closed while loading is cancelled; its late rejection must not reach the dropdown
// opened again since, which shows its own values.
test("a_cancelled_chooser_that_rejects_late_leaves_the_reopened_dropdown_alone", async () => {
  saved = { last: EMPTY_SEARCH, history: [] };
  chooserHeld = true;
  show();
  await waitFor(() => expect(asked()).toHaveLength(1));
  await fireEvent.click(within(filters()).getByRole("button", { name: "Label" }));
  await waitFor(() => expect(calls("github_pr_chooser")).toHaveLength(1));
  const [first] = [...held.keys()];
  await waitFor(() => expect(taskResult).toHaveBeenCalledWith(first));
  await fireEvent.keyDown(screen.getByRole("menu", { name: "Label" }), { key: "Escape" });
  await waitFor(() => expect(screen.queryByRole("menu", { name: "Label" })).toBeNull());

  chooserHeld = false;
  await fireEvent.click(within(filters()).getByRole("button", { name: "Label" }));
  const again = screen.getByRole("menu", { name: "Label" });
  await waitFor(() => expect(labels(again)).toEqual(["alice", "bob"]));

  held.get(first)?.({ kind: "cancelled", message: "The operation was cancelled" });
  // Every promise of the old load settles before a new macrotask.
  await new Promise((resolve) => setTimeout(resolve, 0));
  await tick();
  expect(labels(screen.getByRole("menu", { name: "Label" }))).toEqual(["alice", "bob"]);
});
