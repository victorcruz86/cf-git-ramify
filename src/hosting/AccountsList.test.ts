import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { get } from "svelte/store";
import { invoke } from "@tauri-apps/api/core";
import type { AccountView } from "../lib/api";
import { githubAccountsStore, loginFor, loginRequest } from "./accounts";
import AccountsList from "./AccountsList.svelte";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const account = (
  login: string,
  extra: Partial<AccountView> = {},
): AccountView => ({
  id: `github.com/${login}`,
  server: "github.com",
  login,
  name: null,
  error: null,
  needsLogin: false,
  isDefault: false,
  ...extra,
});

let accounts: AccountView[] = [];

function mockCommands(commands: Record<string, (args: never) => unknown> = {}) {
  // The list gives the rows at once; the details come one row at a time.
  const bare = (row: AccountView): AccountView => ({
    ...row,
    name: null,
    error: null,
  });
  const all: Record<string, (args: never) => unknown> = {
    github_accounts: () => accounts.map(bare),
    github_account_details: (args) =>
      accounts.find((row) => row.id === (args as unknown as { id: string }).id),
    ...commands,
  };
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    const handler = all[command];
    if (!handler) throw new Error(`unexpected command: ${command}`);
    return handler(args as never);
  });
}

const calls = (command: string) =>
  vi.mocked(invoke).mock.calls.filter(([name]) => name === command);

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  accounts = [];
  loginRequest.set(null);
  githubAccountsStore.reset();
});

afterEach(cleanup);

async function show(list: AccountView[]) {
  accounts = list;
  mockCommands();
  render(AccountsList);
  await waitFor(() =>
    expect(calls("github_accounts").length).toBeGreaterThan(0),
  );
}

test("the_accounts_list_shows_default_in_bold_and_log_in_on_auth_errors", async () => {
  await show([
    account("octocat", { name: "The Octocat", isDefault: true }),
    account("hubot", {
      error: "Missing access token",
      needsLogin: true,
      server: "ghe.example.com",
    }),
    account("mona", { error: "Server is unreachable" }),
  ]);

  const rows = await screen.findAllByRole("option");
  expect(rows).toHaveLength(3);
  await screen.findByText("The Octocat");
  await screen.findByText("Server is unreachable");

  // The full name is bold for the default, with the login beside it; the server is below.
  expect(rows[0].querySelector(".name")?.textContent).toBe("The Octocat");
  expect(rows[0].querySelector(".name")?.classList.contains("bold")).toBe(true);
  expect(rows[0].querySelector(".login")?.textContent).toBe("octocat");
  expect(rows[0].querySelector(".server")?.textContent).toBe("github.com");
  expect(rows[0].querySelector("button")).toBeNull();

  // Without a full name the login is the name; the error is in red with `Log in` when it needs one.
  expect(rows[1].querySelector(".name")?.textContent).toBe("hubot");
  expect(rows[1].querySelector(".name")?.classList.contains("bold")).toBe(
    false,
  );
  expect(rows[1].querySelector(".error")?.textContent).toBe(
    "Missing access token",
  );
  expect(rows[1].querySelector(".server")?.textContent).toBe("ghe.example.com");
  expect(rows[1].textContent).toContain("Log in");

  // An error that is not about the login has no link.
  expect(rows[2].querySelector(".error")?.textContent).toBe(
    "Server is unreachable",
  );
  expect(rows[2].querySelector("button")).toBeNull();
});

test("the_rows_show_at_once_and_a_slow_row_does_not_hold_the_others", async () => {
  accounts = [
    account("octocat", { name: "The Octocat" }),
    account("hubot", { name: "The Hubot" }),
  ];
  let release: (row: AccountView) => void = () => {};
  mockCommands({
    github_account_details: (args) => {
      const id = (args as unknown as { id: string }).id;
      return id === "github.com/hubot"
        ? new Promise((resolve) => (release = resolve))
        : accounts[0];
    },
  });
  render(AccountsList);

  // Both rows are there with their logins; only the quick one has its details.
  expect(await screen.findAllByRole("option")).toHaveLength(2);
  expect(await screen.findByText("The Octocat")).toBeTruthy();
  expect(screen.queryByText("The Hubot")).toBeNull();
  expect(screen.getByText("hubot")).toBeTruthy();
  expect(calls("github_account_details").map(([, args]) => args)).toEqual([
    { id: "github.com/octocat" },
    { id: "github.com/hubot" },
  ]);

  release(accounts[1]);
  expect(await screen.findByText("The Hubot")).toBeTruthy();
});

test("a_row_whose_details_fail_shows_the_error_in_that_row_only", async () => {
  accounts = [
    account("octocat", { name: "The Octocat" }),
    account("hubot", {
      error: "Request response: Bad credentials.",
      needsLogin: true,
    }),
    account("mona"),
  ];
  mockCommands({
    github_account_details: (args) => {
      const id = (args as unknown as { id: string }).id;
      if (id === "github.com/mona") {
        return Promise.reject({
          kind: "failed",
          message: "The account is gone",
        });
      }
      return accounts.find((row) => row.id === id);
    },
  });
  render(AccountsList);
  const rows = await screen.findAllByRole("option");
  await waitFor(() =>
    expect(rows[1].querySelector(".error")?.textContent).toBe(
      "Request response: Bad credentials.",
    ),
  );
  expect(rows[1].textContent).toContain("Log in");
  await waitFor(() =>
    expect(rows[2].querySelector(".error")?.textContent).toBe(
      "The account is gone",
    ),
  );
  expect(rows[2].textContent).not.toContain("Log in");
  expect(rows[0].querySelector(".error")).toBeNull();
});

test("a_failed_list_shows_its_error_and_not_a_blank_list", async () => {
  mockCommands({
    github_accounts: () =>
      Promise.reject({
        kind: "failed",
        message: "Could not read the GitHub accounts: denied",
      }),
  });
  render(AccountsList);
  const alert = await screen.findByRole("alert");
  expect(alert.textContent).toBe("Could not read the GitHub accounts: denied");
  expect(screen.queryByText(/No accounts added/)).toBeNull();
});

test("log_in_opens_the_dialog_for_that_account_with_its_server", async () => {
  await show([
    account("hubot", { needsLogin: true, error: "Missing access token" }),
  ]);
  await fireEvent.click(await screen.findByRole("button", { name: "Log in" }));

  let request: {
    server: string | null;
    editing: string | null;
    add: boolean;
  } | null = null;
  loginRequest.subscribe((value) => (request = value))();
  expect(request).toMatchObject({
    server: "github.com",
    editing: "github.com/hubot",
    add: false,
  });
});

test("set_as_default_is_off_for_the_default_and_minus_needs_a_row", async () => {
  await show([account("octocat", { isDefault: true }), account("hubot")]);
  const setDefault = () =>
    screen.getByRole("button", { name: "Set as Default" }) as HTMLButtonElement;
  const minus = () =>
    screen.getByRole("button", { name: "Remove" }) as HTMLButtonElement;

  // No row selected: both are off.
  expect(setDefault().disabled).toBe(true);
  expect(minus().disabled).toBe(true);

  const rows = await screen.findAllByRole("option");
  await fireEvent.click(rows[0]);
  expect(setDefault().disabled).toBe(true);
  expect(minus().disabled).toBe(false);

  await fireEvent.click(rows[1]);
  expect(setDefault().disabled).toBe(false);

  accounts = [account("octocat"), account("hubot", { isDefault: true })];
  mockCommands({ github_set_default: () => undefined });
  await fireEvent.click(setDefault());
  await waitFor(() =>
    expect(calls("github_set_default")).toEqual([
      ["github_set_default", { id: "github.com/hubot" }],
    ]),
  );
  await waitFor(() => expect(setDefault().disabled).toBe(true));
});

test("minus_removes_the_selected_account_without_asking", async () => {
  await show([account("octocat"), account("hubot")]);
  mockCommands({
    github_remove: () => undefined,
    github_accounts: () => [account("octocat")],
  });
  await fireEvent.click((await screen.findAllByRole("option"))[1]);
  await fireEvent.click(screen.getByRole("button", { name: "Remove" }));
  await waitFor(() =>
    expect(calls("github_remove")).toEqual([
      ["github_remove", { id: "github.com/hubot" }],
    ]),
  );
  await waitFor(() => expect(screen.getAllByRole("option")).toHaveLength(1));
});

test("the_plus_popup_offers_token_then_a_separator_then_enterprise", async () => {
  await show([account("octocat")]);
  await fireEvent.click(screen.getByRole("button", { name: "Add" }));
  const menu = await screen.findByRole("menu");
  expect(menu.querySelectorAll("[role=menuitem]")).toHaveLength(2);
  expect(
    [...menu.children].map(
      (item) => item.getAttribute("aria-label") ?? item.getAttribute("role"),
    ),
  ).toEqual([
    "Log In with Token…",
    "separator",
    "Log In to GitHub Enterprise…",
  ]);

  await fireEvent.click(
    screen.getByRole("menuitem", { name: "Log In to GitHub Enterprise…" }),
  );
  let request: { server: string | null; add: boolean } | null = null;
  loginRequest.subscribe((value) => (request = value))();
  expect(request).toMatchObject({ server: null, add: true });
});

test("an_empty_list_says_so_and_add_account_opens_the_popup", async () => {
  await show([]);
  expect(await screen.findByText(/No accounts added/)).toBeTruthy();
  expect(screen.getByText("(⌘N)")).toBeTruthy();
  await fireEvent.click(screen.getByRole("button", { name: "Add account…" }));
  expect(await screen.findByRole("menu")).toBeTruthy();
});

test("the_store_keeps_the_last_load", async () => {
  accounts = [account("octocat")];
  mockCommands();
  await githubAccountsStore.load();
  await waitFor(() => expect(calls("github_account_details")).toHaveLength(1));
  let seen: AccountView[] | null = null;
  githubAccountsStore.subscribe((state) => (seen = state.accounts))();
  expect(seen).toEqual([account("octocat")]);
});

test("a_default_answer_for_another_repository_is_dropped_and_errors_go_to_the_console", async () => {
  const { checkDefaultAccount } = await import("./accounts");
  const { notifications, dismiss } = await import("../lib/notifications");
  const { get } = await import("svelte/store");
  for (const { id } of get(notifications)) dismiss(id);
  accounts = [account("octocat")];
  mockCommands({
    repo_settings_get: () => ({ githubDefaultAccount: "github.com/gone" }),
  });

  // Another repository was opened while the accounts were read.
  await checkDefaultAccount(vi.fn(), () => false);
  expect(get(notifications)).toEqual([]);

  const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
  mockCommands({
    repo_settings_get: () => Promise.reject(new Error("no repository")),
  });
  await checkDefaultAccount(vi.fn(), () => true);
  expect(warn).toHaveBeenCalledOnce();
  warn.mockRestore();
});

test("a_stored_default_that_is_not_an_account_warns_with_configure", async () => {
  const { checkDefaultAccount } = await import("./accounts");
  const { notifications, dismiss } = await import("../lib/notifications");
  const { get } = await import("svelte/store");
  for (const { id } of get(notifications)) dismiss(id);
  accounts = [account("octocat")];
  mockCommands({
    repo_settings_get: () => ({ githubDefaultAccount: "github.com/gone" }),
  });
  const configure = vi.fn();

  await checkDefaultAccount(configure, () => true);

  const [shown] = get(notifications);
  expect(shown.title).toBe("Missing default GitHub account");
  expect(shown.kind).toBe("warning");
  shown.actions?.[0].run();
  expect(shown.actions?.[0].label).toBe("Configure…");
  expect(configure).toHaveBeenCalledOnce();

  // A default that exists, or none stored, says nothing.
  dismiss(shown.id);
  mockCommands({
    repo_settings_get: () => ({ githubDefaultAccount: "github.com/octocat" }),
  });
  await checkDefaultAccount(configure, () => true);
  mockCommands({ repo_settings_get: () => ({}) });
  await checkDefaultAccount(configure, () => true);
  expect(get(notifications)).toEqual([]);
  // Every repository open makes this check: it reads no token (an unsigned build would prompt).
  expect(calls("github_accounts").map(([, args]) => args)).toEqual([{ tokens: false }, { tokens: false }]);
});

// Every login that ends with an account is announced, wherever it was asked: the Pull Requests
// window forgets then that the account had no token. A cancelled one says nothing.
test("a_login_is_announced_to_whoever_listens", async () => {
  const { onLogin } = await import("./accounts");
  mockCommands();
  const heard = vi.fn();
  const stop = onLogin(heard);

  const first = loginFor("github.com", "github.com/octocat");
  get(loginRequest)?.finish(account("octocat"));
  expect(await first).toEqual(account("octocat"));
  const second = loginFor("github.com");
  get(loginRequest)?.finish(null);
  expect(await second).toBeNull();

  expect(heard.mock.calls).toEqual([[account("octocat")]]);
  stop();
});
