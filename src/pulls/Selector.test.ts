import { tick } from "svelte";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { Known } from "../lib/api";
import { announceLogin, loginFor } from "../hosting/accounts";
import { taskResult } from "../lib/tasks";
import { forgetToolWindows, registerToolWindow } from "../shell/toolWindows";
import { account, connection, mapping } from "./fixtures";
import Selector from "./Selector.svelte";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("../lib/tasks", () => ({ taskResult: vi.fn() }));
vi.mock("../hosting/accounts", async (original) => ({
  ...(await original<typeof import("../hosting/accounts")>()),
  loginFor: vi.fn(),
}));

const connects = () =>
  vi.mocked(invoke).mock.calls.filter(([name]) => name === "github_connect").map(([, args]) => args);

let known: Known;
let onconnected: ReturnType<typeof vi.fn>;
let onreload: ReturnType<typeof vi.fn>;

beforeEach(() => {
  // The window's state, the accounts found without a token included, starts empty.
  registerToolWindow("pulls");
  forgetToolWindows();
  vi.mocked(invoke).mockReset();
  vi.mocked(taskResult).mockReset();
  vi.mocked(loginFor).mockReset();
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "github_connect") return 5;
    throw new Error(`unexpected command: ${command}`);
  });
  vi.mocked(taskResult).mockImplementation(async () => {
    const [args] = connects().slice(-1) as [{ remote: string; account: string }];
    return connection(args.remote, args.account);
  });
  onconnected = vi.fn();
  onreload = vi.fn(async () => {
    view.rerender({ known, onconnected, onreload });
  });
});

afterEach(cleanup);

let view: ReturnType<typeof render>;

function show(value: Known) {
  known = value;
  view = render(Selector, { known, onconnected, onreload });
}

const combo = () => screen.getByRole("button", { name: "Repository" });
const accountButton = () => screen.getByRole("button", { name: "Account" });

test("one_mapping_and_one_account_connect_by_themselves", async () => {
  // Another account on another server does not count (`URIUtil.equalWithoutSchema`).
  show({
    mappings: [mapping("o/r", "origin")],
    accounts: [account("octocat"), account("hubot", { server: "ghe.example.com" })],
    saved: null,
  });
  await waitFor(() => expect(onconnected).toHaveBeenCalledTimes(1));
  expect(connects()).toEqual([{ remote: "origin", account: "github.com/octocat" }]);
  expect(taskResult).toHaveBeenCalledWith(5);
  expect(onconnected.mock.calls[0][0].account).toBe("github.com/octocat");
});

test("a_saved_choice_reconnects", async () => {
  const fork = mapping("me/r", "origin");
  show({
    mappings: [mapping("acme/r", "upstream"), fork],
    accounts: [account("alice"), account("bob")],
    saved: { url: fork.url, account: "github.com/bob" },
  });
  await waitFor(() => expect(onconnected).toHaveBeenCalledTimes(1));
  expect(connects()).toEqual([{ remote: "origin", account: "github.com/bob" }]);
});

test("a_saved_choice_that_is_gone_is_not_used", async () => {
  show({
    mappings: [mapping("acme/r", "upstream"), mapping("me/r", "origin")],
    accounts: [account("alice"), account("bob")],
    saved: { url: "https://github.com/gone/r.git", account: "github.com/bob" },
  });
  expect(await screen.findByRole("button", { name: "View Pull Requests" })).toBeTruthy();
  expect(connects()).toEqual([]);
});

test("otherwise_the_first_repo_and_the_first_account_by_name_are_preselected", async () => {
  show({
    mappings: [mapping("acme/r", "upstream"), mapping("me/r", "origin")],
    accounts: [account("alice"), account("bob")],
    saved: null,
  });
  expect(combo().textContent).toContain("acme/r");
  // The remote's name, in grey after the repository.
  expect(combo().querySelector(".secondary")?.textContent).toBe("upstream");
  expect(accountButton().textContent).toContain("alice");
  expect(connects()).toEqual([]);
  expect(screen.getByText("Change repository or account later under the gear icon")).toBeTruthy();

  // Another repository and another account.
  await fireEvent.click(combo());
  const items = screen.getAllByRole("menuitem").map((item) => item.getAttribute("aria-label"));
  expect(items).toEqual(["acme/r", "me/r"]);
  await fireEvent.click(screen.getByRole("menuitem", { name: "me/r" }));
  await fireEvent.click(accountButton());
  expect(screen.getAllByRole("menuitem").map((item) => item.getAttribute("aria-label"))).toEqual([
    "alice",
    "bob",
    "Log In with Token…",
  ]);
  await fireEvent.click(screen.getByRole("menuitem", { name: "bob" }));

  await fireEvent.click(screen.getByRole("button", { name: "View Pull Requests" }));
  await waitFor(() => expect(onconnected).toHaveBeenCalledTimes(1));
  expect(connects()).toEqual([{ remote: "origin", account: "github.com/bob" }]);
});

const MISSING = { kind: "missing_token", message: "Missing access token" };

// The tokens are read only when connecting: the connect says one is missing.
test("a_missing_token_says_could_not_connect_and_missing_access_token", async () => {
  vi.mocked(taskResult).mockRejectedValueOnce(MISSING);
  show({
    mappings: [mapping("o/r", "origin")],
    accounts: [account("octocat")],
    saved: null,
  });
  const alert = await screen.findByRole("alert");
  expect(connects()).toHaveLength(1);
  expect(alert.textContent).toContain("Could not connect to repository");
  expect(alert.textContent).toContain("Missing access token");
  expect(screen.queryByText("Log in again…")).toBeNull();
  expect(screen.queryByRole("button", { name: "View Pull Requests" })).toBeNull();

  // `Log In with Token…` logs the account in again, then connects.
  vi.mocked(loginFor).mockResolvedValueOnce(account("octocat"));
  await fireEvent.click(screen.getByRole("button", { name: "Log In with Token…" }));
  await waitFor(() => expect(onconnected).toHaveBeenCalledTimes(1));
  expect(loginFor).toHaveBeenCalledWith("github.com", "github.com/octocat");
  expect(onreload).toHaveBeenCalled();
  expect(connects()).toHaveLength(2);
  expect(screen.queryByRole("alert")).toBeNull();
});

test("without_an_account_the_selector_offers_a_login_and_connects_after_it", async () => {
  show({ mappings: [mapping("o/r", "origin")], accounts: [], saved: null });
  expect(screen.queryByRole("alert")).toBeNull();
  expect(screen.getByRole("button", { name: "Account" }).title).toBe("Choose an account");
  vi.mocked(loginFor).mockImplementation(async () => {
    known = { ...known, accounts: [account("octocat")] };
    return account("octocat");
  });
  await fireEvent.click(screen.getByRole("button", { name: "Log In with Token…" }));
  await waitFor(() => expect(onconnected).toHaveBeenCalledTimes(1));
  expect(loginFor).toHaveBeenCalledWith("github.com");
  expect(connects()).toEqual([{ remote: "origin", account: "github.com/octocat" }]);
});

test("a_github_enterprise_repository_logs_in_to_github_enterprise", async () => {
  const work = mapping("acme/tool", "work", "ghe.example.com");
  vi.mocked(taskResult).mockRejectedValueOnce(MISSING);
  show({
    mappings: [work],
    accounts: [account("hubot", { server: "ghe.example.com" })],
    saved: null,
  });
  expect(await screen.findByRole("button", { name: "Log In to GitHub Enterprise…" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Log In with Token…" })).toBeNull();
});

test("a_failed_connect_names_repo_and_account_and_offers_log_in_again", async () => {
  vi.mocked(taskResult).mockRejectedValueOnce({
    kind: "failed",
    message: "Request response: Bad credentials.",
  });
  show({ mappings: [mapping("o/r", "origin")], accounts: [account("octocat")], saved: null });
  const alert = await screen.findByRole("alert");
  expect(alert.textContent).toContain(
    "Could not connect to repository github.com/o/r with account github.com/octocat",
  );
  expect(alert.textContent).toContain("Request response: Bad credentials.");
  expect(onconnected).not.toHaveBeenCalled();

  vi.mocked(loginFor).mockResolvedValueOnce(account("octocat"));
  await fireEvent.click(screen.getByRole("button", { name: "Log in again…" }));
  await waitFor(() => expect(onconnected).toHaveBeenCalledTimes(1));
  expect(loginFor).toHaveBeenCalledWith("github.com", "github.com/octocat");
  expect(connects()).toHaveLength(2);
  expect(screen.queryByRole("alert")).toBeNull();
});

test("a_cancelled_login_does_not_connect", async () => {
  vi.mocked(taskResult).mockRejectedValueOnce({ kind: "failed", message: "Server is unreachable" });
  show({ mappings: [mapping("o/r", "origin")], accounts: [account("octocat")], saved: null });
  await screen.findByRole("alert");
  vi.mocked(loginFor).mockResolvedValueOnce(null);
  await fireEvent.click(screen.getByRole("button", { name: "Log in again…" }));
  await waitFor(() => expect(loginFor).toHaveBeenCalled());
  expect(connects()).toHaveLength(1);
});

test("a_cancelled_connect_shows_no_error", async () => {
  vi.mocked(taskResult).mockRejectedValueOnce({ kind: "cancelled", message: "The operation was cancelled" });
  show({ mappings: [mapping("o/r", "origin")], accounts: [account("octocat")], saved: null });
  await waitFor(() => expect(connects()).toHaveLength(1));
  expect(await screen.findByRole("button", { name: "View Pull Requests" })).toBeTruthy();
  expect(screen.queryByRole("alert")).toBeNull();
  expect(onconnected).not.toHaveBeenCalled();
});

// `singleRepoAndAccountState.collect` keeps watching: a pair that forms later (a login in
// Settings) connects by itself, and so does a saved choice that becomes valid.
test("a_pair_that_forms_later_connects_by_itself", async () => {
  show({ mappings: [mapping("o/r", "origin")], accounts: [], saved: null });
  await screen.findByRole("button", { name: "Log In with Token…" });
  expect(connects()).toEqual([]);
  known = { ...known, accounts: [account("octocat")] };
  view.rerender({ known, onconnected, onreload });
  await waitFor(() => expect(onconnected).toHaveBeenCalledTimes(1));
  expect(connects()).toEqual([{ remote: "origin", account: "github.com/octocat" }]);
  // The same pair again does not connect twice.
  known = { ...known };
  view.rerender({ known, onconnected, onreload });
  await tick();
  expect(connects()).toHaveLength(1);
});

// The accounts a connect found without a token live in the window's state, not in the selector:
// the selector shown again (the gear's `Change Repository or Account…`) still offers the login and
// connects nothing.
test("an_account_found_without_a_token_stays_so_when_the_selector_comes_back", async () => {
  vi.mocked(taskResult).mockRejectedValueOnce(MISSING);
  const value = { mappings: [mapping("o/r", "origin")], accounts: [account("octocat")], saved: null };
  show(value);
  expect(await screen.findByRole("button", { name: "Log In with Token…" })).toBeTruthy();
  expect(connects()).toHaveLength(1);
  cleanup();

  show(value);
  await tick();
  expect(screen.getByRole("button", { name: "Log In with Token…" })).toBeTruthy();
  expect(screen.getByRole("alert").textContent).toContain("Missing access token");
  expect(screen.queryByRole("button", { name: "View Pull Requests" })).toBeNull();
  expect(connects()).toHaveLength(1);
});

// A login of that account anywhere, Settings included, forgets that it had no token: the selector
// offers `View Pull Requests` again.
test("a_login_in_settings_forgets_that_the_account_had_no_token", async () => {
  vi.mocked(taskResult).mockRejectedValueOnce(MISSING);
  show({ mappings: [mapping("o/r", "origin")], accounts: [account("octocat")], saved: null });
  expect(await screen.findByRole("button", { name: "Log In with Token…" })).toBeTruthy();

  announceLogin(account("octocat"));

  expect(await screen.findByRole("button", { name: "View Pull Requests" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Log In with Token…" })).toBeNull();
  expect(screen.queryByRole("alert")).toBeNull();
});
