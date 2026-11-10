import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { AccountView } from "../lib/api";
import { taskResult } from "../lib/tasks";
import LoginDialog from "./LoginDialog.svelte";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("../lib/tasks", () => ({ taskResult: vi.fn() }));

const OCTOCAT: AccountView = {
  id: "github.com/octocat",
  server: "github.com",
  login: "octocat",
  name: null,
  error: null,
  needsLogin: false,
  isDefault: false,
};

const GENERATE =
  "https://github.com/settings/tokens/new?description=rama%20GitHub%20integration%20plugin&scopes=repo%2Cgist%2Cread%3Aorg%2Cworkflow%2Cread%3Auser%2Cuser%3Aemail";

function mockCommands(commands: Record<string, (args: never) => unknown> = {}) {
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    const handler = commands[command];
    if (!handler) throw new Error(`unexpected command: ${command}`);
    return handler(args as never);
  });
}

const calls = (command: string) =>
  vi.mocked(invoke).mock.calls.filter(([name]) => name === command);

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(taskResult).mockReset();
});

afterEach(cleanup);

function open(
  props: {
    server?: string | null;
    editing?: string | null;
    add?: boolean;
  } = {},
) {
  const ondone = vi.fn();
  render(LoginDialog, {
    server: null,
    editing: null,
    add: false,
    ...props,
    ondone,
  });
  return ondone;
}

const server = () => screen.getByLabelText("Server:") as HTMLInputElement;
const token = () => screen.getByLabelText("Token:") as HTMLInputElement;
const ok = (name: string) => screen.getByRole("button", { name });

test("the_token_dialog_variants_have_241s_titles_and_buttons", () => {
  // Settings `+` > `Log In with Token…`: github.com, not editable.
  open({ server: "github.com", add: true });
  expect(
    screen.getByRole("dialog", { name: "Add GitHub Account" }),
  ).toBeTruthy();
  expect(ok("Add Account")).toBeTruthy();
  expect(server().value).toBe("github.com");
  expect(server().readOnly).toBe(true);
  cleanup();

  // `Log In to GitHub Enterprise…`: empty and editable.
  open({ add: true });
  expect(
    screen.getByRole("dialog", { name: "Add GitHub Account" }),
  ).toBeTruthy();
  expect(ok("Add Account")).toBeTruthy();
  expect(server().value).toBe("");
  expect(server().readOnly).toBe(false);
  cleanup();

  // A row's `Log in` link: the account's server, not editable.
  open({ server: "ghe.example.com", editing: "ghe.example.com/octocat" });
  expect(screen.getByRole("dialog", { name: "Log In to GitHub" })).toBeTruthy();
  expect(ok("Log In")).toBeTruthy();
  expect(server().value).toBe("ghe.example.com");
  expect(server().readOnly).toBe(true);
  cleanup();

  // A request with a server: fixed. Without one: editable, starting at github.com.
  open({ server: "ghe.example.com" });
  expect(screen.getByRole("dialog", { name: "Log In to GitHub" })).toBeTruthy();
  expect(server().readOnly).toBe(true);
  cleanup();
  open();
  expect(screen.getByRole("dialog", { name: "Log In to GitHub" })).toBeTruthy();
  expect(ok("Log In")).toBeTruthy();
  expect(server().value).toBe("github.com");
  expect(server().readOnly).toBe(false);
});

test("the_token_is_a_password_field_with_the_scopes_comment", () => {
  open();
  expect(token().type).toBe("password");
  expect(
    screen.getByText(
      "The following scopes must be granted to the access token: [repo, gist, read:org, workflow, read:user, user:email]",
    ),
  ).toBeTruthy();
  expect(ok("Generate…")).toBeTruthy();
});

test("the_backend_names_the_field_of_each_error", async () => {
  // The backend checks the server, then the token, and says where each failure belongs. The
  // dialog does not read the text.
  const rejections = [
    { kind: "login_server", message: "Server cannot be empty" },
    { kind: "login_server", message: "Server URL is not valid" },
    { kind: "login_token", message: "Token cannot be empty" },
  ];
  mockCommands({ github_login: () => Promise.reject(rejections.shift()) });
  open({ add: true });

  await fireEvent.click(ok("Add Account"));
  let alert = await screen.findByRole("alert");
  expect(alert.textContent).toBe("Server cannot be empty");
  expect(server().nextElementSibling).toBe(alert);

  await fireEvent.click(ok("Add Account"));
  await waitFor(() =>
    expect(screen.getByRole("alert").textContent).toBe(
      "Server URL is not valid",
    ),
  );
  expect(screen.getAllByRole("alert")).toHaveLength(1);

  await fireEvent.click(ok("Add Account"));
  await waitFor(() =>
    expect(screen.getByRole("alert").textContent).toBe("Token cannot be empty"),
  );
  alert = screen.getByRole("alert");
  expect(token().parentElement?.nextElementSibling).toBe(alert);
  expect(calls("github_login")).toHaveLength(3);
});

test("an_error_of_another_kind_with_the_text_of_a_server_error_stays_under_the_token", async () => {
  mockCommands({ github_login: () => 4 });
  vi.mocked(taskResult).mockRejectedValueOnce({
    kind: "failed",
    message: "Invalid server path",
  });
  open({ server: "github.com", add: true });
  await fireEvent.input(token(), { target: { value: "ghp_secret" } });
  await fireEvent.click(ok("Add Account"));
  const alert = await screen.findByRole("alert");
  expect(token().parentElement?.nextElementSibling).toBe(alert);
});

test("the_token_is_sent_once_and_cleared", async () => {
  mockCommands({ github_login: () => 7 });
  vi.mocked(taskResult).mockResolvedValueOnce(OCTOCAT);
  const ondone = open({ server: "github.com", add: true });

  await fireEvent.input(token(), { target: { value: "ghp_secret" } });
  await fireEvent.click(ok("Add Account"));

  await waitFor(() => expect(ondone).toHaveBeenCalledWith(OCTOCAT));
  expect(calls("github_login")).toHaveLength(1);
  expect(calls("github_login")[0][1]).toEqual({
    server: "github.com",
    token: "ghp_secret",
    editing: null,
  });
  expect(taskResult).toHaveBeenCalledWith(7);
  expect(token().value).toBe("");
});

test("a_failed_login_shows_the_error_under_its_field_clears_the_token_and_keeps_ok_enabled", async () => {
  mockCommands({ github_login: () => 8 });
  vi.mocked(taskResult).mockRejectedValueOnce({
    kind: "failed",
    message: "Incorrect credentials.\nRequest response: Bad credentials.",
  });
  const ondone = open({ server: "github.com", add: true });

  await fireEvent.input(token(), { target: { value: "ghp_wrong" } });
  await fireEvent.click(ok("Add Account"));

  const alert = await screen.findByRole("alert");
  expect(alert.textContent).toBe(
    "Incorrect credentials.\nRequest response: Bad credentials.",
  );
  expect(token().parentElement?.nextElementSibling).toBe(alert);
  expect(token().value).toBe("");
  expect((ok("Add Account") as HTMLButtonElement).disabled).toBe(false);
  expect(ondone).not.toHaveBeenCalled();
  expect(calls("github_login")).toHaveLength(1);
});

test("server_errors_of_the_backend_show_under_the_server_field", async () => {
  mockCommands({ github_login: () => 9 });
  vi.mocked(taskResult).mockRejectedValueOnce({
    kind: "login_server",
    message: "Invalid server path",
  });
  open({ server: "github.com", add: true });

  await fireEvent.input(token(), { target: { value: "ghp_secret" } });
  await fireEvent.click(ok("Add Account"));

  const alert = await screen.findByRole("alert");
  expect(alert.textContent).toBe("Invalid server path");
  expect(server().nextElementSibling).toBe(alert);
  // Only the server is wrong: the token stays for the next try.
  expect(token().value).toBe("ghp_secret");
});

// A server the backend refuses keeps the pasted token: no server was asked, and the user only
// fixes the server. The token still goes once per Log In, and success clears it.
test("a_mistyped_server_keeps_the_token_for_the_next_try", async () => {
  mockCommands({
    github_login: (args: { server: string }) =>
      args.server === "gihtub..com"
        ? Promise.reject({ kind: "login_server", message: "Server URL is not valid" })
        : 7,
  });
  vi.mocked(taskResult).mockResolvedValueOnce(OCTOCAT);
  const ondone = open({ add: true });

  await fireEvent.input(server(), { target: { value: "gihtub..com" } });
  await fireEvent.input(token(), { target: { value: "ghp_secret" } });
  await fireEvent.click(ok("Add Account"));

  const alert = await screen.findByRole("alert");
  expect(server().nextElementSibling).toBe(alert);
  expect(token().value).toBe("ghp_secret");

  await fireEvent.input(server(), { target: { value: "github.com" } });
  await fireEvent.click(ok("Add Account"));
  await waitFor(() => expect(ondone).toHaveBeenCalledWith(OCTOCAT));
  expect(calls("github_login").map(([, args]) => args)).toEqual([
    { server: "gihtub..com", token: "ghp_secret", editing: null },
    { server: "github.com", token: "ghp_secret", editing: null },
  ]);
  expect(token().value).toBe("");
});

test("a_relogin_sends_the_edited_id_and_does_not_parse_the_server", async () => {
  mockCommands({ github_login: () => 3 });
  vi.mocked(taskResult).mockResolvedValueOnce(OCTOCAT);
  const ondone = open({
    server: "https://odd server",
    editing: "github.com/octocat",
  });

  await fireEvent.input(token(), { target: { value: "ghp_new" } });
  await fireEvent.click(ok("Log In"));

  await waitFor(() => expect(ondone).toHaveBeenCalledWith(OCTOCAT));
  expect(calls("github_login")[0][1]).toEqual({
    server: "https://odd server",
    token: "ghp_new",
    editing: "github.com/octocat",
  });
});

test("while_busy_the_fields_are_disabled_and_cancel_cancels_the_task", async () => {
  mockCommands({ github_login: () => 5, task_cancel: () => undefined });
  vi.mocked(taskResult).mockReturnValueOnce(new Promise(() => {}));
  const ondone = open({ server: "github.com", add: true });

  await fireEvent.input(token(), { target: { value: "ghp_secret" } });
  await fireEvent.click(ok("Add Account"));

  await waitFor(() => expect(token().disabled).toBe(true));
  expect(server().disabled).toBe(true);
  await waitFor(() => expect(taskResult).toHaveBeenCalled());

  expect((ok("Generate…") as HTMLButtonElement).disabled).toBe(true);

  await fireEvent.click(ok("Cancel"));
  expect(calls("task_cancel")).toEqual([["task_cancel", { id: 5 }]]);
  // The backend may have saved the account already: the list is reloaded.
  expect(ondone).toHaveBeenCalledWith(null, true);
  expect(token().value).toBe("");
});

test("generate_opens_the_prefilled_page", async () => {
  mockCommands({
    github_generate_url: () => GENERATE,
    open_url: () => undefined,
  });
  open({ server: "github.com", add: true });

  await fireEvent.click(ok("Generate…"));

  await waitFor(() => expect(calls("open_url")).toHaveLength(1));
  expect(calls("github_generate_url")[0][1]).toEqual({ server: "github.com" });
  expect(calls("open_url")[0][1]).toEqual({ url: GENERATE });
});

test("generate_is_off_while_the_server_is_blank", async () => {
  open({ add: true });
  expect((ok("Generate…") as HTMLButtonElement).disabled).toBe(true);
  await fireEvent.input(server(), { target: { value: "ghe.example.com" } });
  expect((ok("Generate…") as HTMLButtonElement).disabled).toBe(false);
});

test("a_server_the_backend_refuses_for_generate_shows_under_the_server", async () => {
  mockCommands({
    github_generate_url: () =>
      Promise.reject({
        kind: "login_server",
        message: "Server URL is not valid",
      }),
  });
  open({ add: true });
  await fireEvent.input(server(), {
    target: { value: "ghe.example.com:port" },
  });
  await fireEvent.click(ok("Generate…"));
  const alert = await screen.findByRole("alert");
  expect(server().nextElementSibling).toBe(alert);
});

test("cancel_before_any_login_does_not_ask_for_a_reload", async () => {
  const ondone = open({ server: "github.com", add: true });
  await fireEvent.input(token(), { target: { value: "ghp_typed" } });
  await fireEvent.click(ok("Cancel"));
  expect(ondone).toHaveBeenCalledWith(null, false);
  expect(token().value).toBe("");
  expect(calls("github_login")).toHaveLength(0);
});
