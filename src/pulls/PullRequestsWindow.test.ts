import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { tick } from "svelte";
import { get } from "svelte/store";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { AccountView, Known } from "../lib/api";
import { taskResult } from "../lib/tasks";
import { loginFor } from "../hosting/accounts";
import { dismiss, notifications } from "../lib/notifications";
import { forgetToolWindows, registeredToolWindows } from "../shell/toolWindows";
import { account, connection, mapping } from "./fixtures";
import PullRequestsWindow from "./PullRequestsWindow.svelte";
import {
  checkPullRequestsWindow,
  showPullRequestsWindow,
  closePullRequestTab,
  connected,
  currentConnection,
  openPullRequestTab,
  pullsStore,
  selectPullsTab,
} from "./store";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({ label: "main" }),
}));
vi.mock("../lib/tasks", () => ({ taskResult: vi.fn() }));
vi.mock("../hosting/accounts", async (original) => ({
  ...(await original<typeof import("../hosting/accounts")>()),
  loginFor: vi.fn(),
}));

let known: Known;
const LIST_PAGE = 9;

const calls = (command: string) => vi.mocked(invoke).mock.calls.filter(([name]) => name === command);

beforeEach(() => {
  forgetToolWindows();
  vi.mocked(invoke).mockReset();
  vi.mocked(taskResult).mockReset();
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "github_known") return known;
    if (command === "github_forget_connection") return undefined;
    if (command === "github_connect") return 3;
    // The list tab asks for its search and its first page.
    if (command === "github_pr_search_saved") return { last: null, history: [] };
    if (command === "github_pr_page") return LIST_PAGE;
    throw new Error(`unexpected command: ${command}`);
  });
  vi.mocked(taskResult).mockImplementation(async (id) =>
    id === LIST_PAGE ? { items: [], endCursor: null, hasNext: false } : connection("origin", "github.com/octocat"),
  );
});

afterEach(cleanup);

// Two repositories and one account: nothing connects by itself, so the gear is offered.
const TWO: () => Known = () => ({
  mappings: [mapping("acme/r", "upstream"), mapping("o/r", "origin")],
  accounts: [account("octocat")],
  saved: null,
});

async function connectedWindow(value: Known) {
  known = value;
  await checkPullRequestsWindow();
  connected(connection("origin", "github.com/octocat"));
  showPullRequestsWindow();
  render(PullRequestsWindow);
}

test("the_window_shows_only_with_a_github_remote", async () => {
  known = { mappings: [], accounts: [], saved: null };
  await checkPullRequestsWindow();
  expect(get(registeredToolWindows)).toEqual([]);

  // github.com with no account: the window is there (the backend only maps a GitHub Enterprise
  // remote when an account on its server exists, decision 4).
  known = { mappings: [mapping("o/r", "origin")], accounts: [], saved: null };
  await checkPullRequestsWindow();
  expect(get(registeredToolWindows)).toEqual(["pulls"]);
  expect(get(pullsStore).known).toEqual(known);

  // The remote goes: so does the window, with its connection and tabs.
  connected(connection("origin", "github.com/octocat"));
  openPullRequestTab(4);
  known = { mappings: [], accounts: [], saved: null };
  await checkPullRequestsWindow();
  expect(get(registeredToolWindows)).toEqual([]);
  expect(get(pullsStore)).toMatchObject({ known: null, connection: null, tabs: [] });
});

test("an_answer_for_another_repository_is_dropped", async () => {
  known = { mappings: [mapping("o/r", "origin")], accounts: [], saved: null };
  await checkPullRequestsWindow(() => false);
  expect(get(registeredToolWindows)).toEqual([]);
});

test("a_connection_whose_account_or_remote_is_gone_is_closed", async () => {
  known = TWO();
  await checkPullRequestsWindow();
  connected(connection("origin", "github.com/octocat"));
  expect(currentConnection()?.account).toBe("github.com/octocat");
  await checkPullRequestsWindow();
  expect(currentConnection()).not.toBeNull();

  known = { ...TWO(), accounts: [] };
  await checkPullRequestsWindow();
  expect(currentConnection()).toBeNull();
});

test("the_selector_shows_until_a_connection_opens", async () => {
  known = TWO();
  await checkPullRequestsWindow();
  showPullRequestsWindow();
  render(PullRequestsWindow);
  expect(screen.queryByRole("tablist")).toBeNull();
  await fireEvent.click(screen.getByRole("button", { name: "View Pull Requests" }));
  const tabs = await screen.findByRole("tablist", { name: "Pull Requests tabs" });
  expect(within(tabs).getAllByRole("tab").map((tab) => tab.textContent?.trim())).toEqual(["r"]);
  expect(calls("github_connect")[0][1]).toEqual({ remote: "upstream", account: "github.com/octocat" });
});

test("pr_tabs_open_once_per_number_and_close", async () => {
  await connectedWindow(TWO());
  const strip = () => screen.getByRole("tablist", { name: "Pull Requests tabs" });
  const names = () => within(strip()).getAllByRole("tab").map((tab) => tab.textContent?.trim());
  // The list tab is fixed: it has no close button.
  expect(names()).toEqual(["r"]);
  expect(within(strip()).queryByRole("button", { name: "Close Tab" })).toBeNull();
  // No `+`: new tabs come from the list. The gear is the strip's only other button.
  const others = within(strip())
    .getAllByRole("button")
    .filter((button) => button.getAttribute("role") !== "tab")
    .map((button) => button.getAttribute("aria-label"));
  expect(others).toEqual(["Options"]);

  openPullRequestTab(12);
  openPullRequestTab(7);
  openPullRequestTab(12);
  await waitFor(() => expect(names()).toEqual(["r", "#12", "#7"]));
  expect(get(pullsStore).tabs).toEqual([12, 7]);
  expect(get(pullsStore).selected).toBe(12);
  const twelve = within(strip()).getByRole("tab", { name: "#12" });
  expect(twelve.getAttribute("aria-selected")).toBe("true");
  // `<projectName>: <description>`, and a PR tab's description is its name.
  expect(twelve.title).toBe("r: #12");

  await fireEvent.click(within(strip()).getByRole("tab", { name: "#7" }));
  expect(get(pullsStore).selected).toBe(7);
  // Closing the selected tab shows the one before it.
  closePullRequestTab(12);
  expect(get(pullsStore).selected).toBe(7);
  const closes = () => within(strip()).getAllByRole("button", { name: "Close Tab" });
  await waitFor(() => expect(closes()).toHaveLength(1));
  await fireEvent.click(closes()[0]);
  await waitFor(() => expect(names()).toEqual(["r"]));
  expect(get(pullsStore)).toMatchObject({ tabs: [], selected: 0 });
});

test("change_repository_or_account_forgets_the_choice", async () => {
  await connectedWindow(TWO());
  openPullRequestTab(3);
  await fireEvent.click(await screen.findByRole("button", { name: "Options" }));
  await fireEvent.click(screen.getByRole("menuitem", { name: "Change Repository or Account…" }));
  await waitFor(() => expect(calls("github_forget_connection")).toHaveLength(1));
  expect(await screen.findByRole("button", { name: "View Pull Requests" })).toBeTruthy();
  expect(screen.queryByRole("tablist")).toBeNull();
  expect(get(pullsStore)).toMatchObject({ connection: null, tabs: [] });
  // The selector does not reconnect by itself afterwards.
  expect(calls("github_connect")).toHaveLength(0);
});

test("the_automatic_pair_has_no_gear", async () => {
  await connectedWindow({ mappings: [mapping("o/r", "origin")], accounts: [account("octocat")], saved: null });
  expect(await screen.findByRole("tablist", { name: "Pull Requests tabs" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Options" })).toBeNull();
});

// A connect that finishes after another repository was opened belongs to the old one: dropped.
test("a_connect_that_finishes_after_another_repository_opened_is_dropped", async () => {
  let finish: (value: unknown) => void = () => {};
  vi.mocked(taskResult).mockImplementationOnce(() => new Promise((resolve) => (finish = resolve)));
  known = TWO();
  await checkPullRequestsWindow();
  showPullRequestsWindow();
  render(PullRequestsWindow);
  await fireEvent.click(screen.getByRole("button", { name: "View Pull Requests" }));
  await waitFor(() => expect(calls("github_connect")).toHaveLength(1));

  // Another repository opens, with a GitHub remote of its own.
  forgetToolWindows();
  known = { mappings: [mapping("b/other", "origin")], accounts: [account("octocat"), account("hubot")], saved: null };
  await checkPullRequestsWindow();
  finish(connection("upstream", "github.com/octocat"));
  await tick();
  await tick();
  expect(currentConnection()).toBeNull();
  expect(screen.queryByRole("tablist")).toBeNull();
});

// A login left open across a repository switch connects nothing when it finishes.
test("a_login_that_finishes_after_another_repository_opened_connects_nothing", async () => {
  let finish: (value: AccountView | null) => void = () => {};
  vi.mocked(loginFor).mockImplementationOnce(() => new Promise((resolve) => (finish = resolve)));
  known = { mappings: [mapping("o/r", "origin")], accounts: [], saved: null };
  await checkPullRequestsWindow();
  showPullRequestsWindow();
  render(PullRequestsWindow);
  await fireEvent.click(await screen.findByRole("button", { name: "Log In with Token…" }));

  forgetToolWindows();
  known = TWO();
  await checkPullRequestsWindow();
  finish(account("octocat"));
  await tick();
  await tick();
  await tick();
  expect(calls("github_connect")).toHaveLength(0);
});

test("a_failure_to_forget_says_it_once", async () => {
  await connectedWindow(TWO());
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "github_forget_connection") {
      throw { kind: "failed", message: "Could not forget the pull requests connection: denied" };
    }
    if (command === "github_known") return known;
    if (command === "github_pr_search_saved") return { last: null, history: [] };
    if (command === "github_pr_page") return LIST_PAGE;
    throw new Error(`unexpected command: ${command}`);
  });
  await fireEvent.click(await screen.findByRole("button", { name: "Options" }));
  await fireEvent.click(screen.getByRole("menuitem", { name: "Change Repository or Account…" }));
  await waitFor(() => expect(get(notifications)).toHaveLength(1));
  const [shown] = get(notifications);
  expect(shown.title).toBe("Could not forget the pull requests connection: denied");
  expect(shown.body).toBeUndefined();
  dismiss(shown.id);
});

test("nothing_connects_until_the_window_is_shown", async () => {
  known = { mappings: [mapping("o/r", "origin")], accounts: [account("octocat")], saved: null };
  await checkPullRequestsWindow();
  render(PullRequestsWindow);
  await tick();
  expect(screen.queryByRole("button", { name: "View Pull Requests" })).toBeNull();
  expect(calls("github_connect")).toHaveLength(0);
  showPullRequestsWindow();
  await waitFor(() => expect(calls("github_connect")).toHaveLength(1));
  // Another repository: hidden again until it is shown.
  forgetToolWindows();
  await checkPullRequestsWindow();
  expect(get(pullsStore).shown).toBe(false);
});

test("the_list_tab_shows_the_list_and_keeps_it_behind_a_pull_request_tab", async () => {
  await connectedWindow(TWO());
  const region = await screen.findByRole("region", { name: "r" });
  await waitFor(() => expect(within(region).getByText("No pull requests matching filters.")).toBeTruthy());
  expect(calls("github_pr_page")).toHaveLength(1);
  openPullRequestTab(5);
  await tick();
  // Still there, hidden, and not asked again.
  expect(region.classList.contains("hidden")).toBe(true);
  selectPullsTab(0);
  await tick();
  expect(region.classList.contains("hidden")).toBe(false);
  expect(calls("github_pr_page")).toHaveLength(1);
});

// A pull request's tab is its details, kept while another tab is in front.
test("a_pull_request_tab_shows_its_details_and_keeps_them_behind_another_tab", async () => {
  await connectedWindow(TWO());
  const failing = vi.mocked(invoke).getMockImplementation()!;
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "github_pr_details") throw { kind: "failed", message: "Server is unreachable" };
    return failing(command, args);
  });
  openPullRequestTab(5);

  const region = await screen.findByRole("region", { name: "#5" });
  await waitFor(() => expect(within(region).getByRole("alert").textContent).toContain("Can't load details"));
  expect(calls("github_pr_details").map(([, args]) => args)).toEqual([{ number: 5 }]);
  selectPullsTab(0);
  await tick();
  expect(region.classList.contains("hidden")).toBe(true);
  selectPullsTab(5);
  await tick();
  expect(region.classList.contains("hidden")).toBe(false);
  expect(calls("github_pr_details")).toHaveLength(1);
});
