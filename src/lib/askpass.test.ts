import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { AskpassPrompt, PromptKind } from "./api";
import AskpassDialog from "./AskpassDialog.svelte";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({ label: "main" }),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
  emitTo: vi.fn(async () => {}),
}));

const handlers = new Map<string, (event: { payload: unknown }) => void>();
let nextId = 1;

beforeEach(() => {
  handlers.clear();
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockResolvedValue(undefined);
  vi.mocked(listen).mockImplementation(async (name, handler) => {
    handlers.set(name, handler as (event: { payload: unknown }) => void);
    return () => {};
  });
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

async function open() {
  render(AskpassDialog);
  await waitFor(() => expect(handlers.has("askpass-prompt")).toBe(true));
}

async function ask(kind: PromptKind): Promise<number> {
  const id = nextId++;
  handlers.get("askpass-prompt")!({ payload: { id, kind } satisfies AskpassPrompt });
  await Promise.resolve();
  return id;
}

async function dismiss(id: number) {
  handlers.get("askpass-dismissed")!({ payload: id });
  await Promise.resolve();
}

const answers = () =>
  vi
    .mocked(invoke)
    .mock.calls.filter(([command]) => command === "askpass_answer")
    .map(([, args]) => args);

const USERNAME: PromptKind = { type: "username", url: "https://github.com" };
const PASSWORD: PromptKind = { type: "password", url: "https://github.com", user: "me" };

const dialog = (name: string) => screen.findByRole("dialog", { name });
const usernameField = () => screen.getByLabelText("Username:") as HTMLInputElement;
const passwordField = () => screen.getByLabelText("Password:") as HTMLInputElement;
const button = (name: string) => screen.getByRole("button", { name });

test("login_dialog_has_username_and_password", async () => {
  await open();
  await ask(USERNAME);

  await dialog("Log In to https://github.com");

  expect(screen.getByText("Enter credentials:")).toBeTruthy();
  expect(usernameField().value).toBe("");
  expect(passwordField().type).toBe("password");
  expect(passwordField().autocomplete).toBe("off");
  expect(button("Log In")).toBeTruthy();
  expect(screen.queryByRole("checkbox")).toBeNull();
});

test("the login answers username and password with a single dialog", async () => {
  await open();
  const first = await ask(USERNAME);
  await dialog("Log In to https://github.com");

  await fireEvent.input(usernameField(), { target: { value: "me" } });
  await fireEvent.input(passwordField(), { target: { value: "secret" } });
  await fireEvent.click(button("Log In"));

  await waitFor(() => expect(answers()).toEqual([{ id: first, answer: "me" }]));
  const second = await ask(PASSWORD);

  await waitFor(() =>
    expect(answers()).toEqual([
      { id: first, answer: "me" },
      { id: second, answer: "secret" },
    ]),
  );
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("with the username in the URL only the password is typed", async () => {
  await open();
  const id = await ask(PASSWORD);
  await dialog("Log In to https://github.com");

  expect(usernameField().value).toBe("me");
  expect(usernameField().readOnly).toBe(true);
  expect(document.activeElement).toBe(passwordField());

  await fireEvent.input(passwordField(), { target: { value: "secret" } });
  await fireEvent.click(button("Log In"));

  expect(answers()).toEqual([{ id, answer: "secret" }]);
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("the login does not go out with empty fields", async () => {
  await open();
  await ask(USERNAME);
  await dialog("Log In to https://github.com");

  await fireEvent.click(button("Log In"));
  expect(screen.getByRole("alert").textContent).toBe("Username cannot be empty");

  await fireEvent.input(usernameField(), { target: { value: "me" } });
  await fireEvent.click(button("Log In"));
  expect(screen.getByRole("alert").textContent).toBe("Password cannot be empty");

  expect(answers()).toEqual([]);
});

test("passphrase_with_the_intellij_texts", async () => {
  await open();
  const id = await ask({ type: "passphrase", key: "/Users/me/.ssh/id_ed25519" });

  await dialog("SSH Key Passphrase");

  expect(screen.getByText('Password for the SSH key "/Users/me/.ssh/id_ed25519":')).toBeTruthy();
  const field = screen.getByLabelText(/Password for the SSH key/) as HTMLInputElement;
  expect(field.type).toBe("password");
  expect(field.autocomplete).toBe("off");
  await fireEvent.input(field, { target: { value: "phrase" } });
  await fireEvent.click(button("OK"));
  expect(answers()).toEqual([{ id, answer: "phrase" }]);
});

test("ssh_password_with_the_intellij_texts", async () => {
  await open();
  await ask({ type: "sshPassword", user: "git@example.com" });

  await dialog("SSH Password Login");

  expect(screen.getByText("Password for user git@example.com:")).toBeTruthy();
});

test("unknown_host_asks_yes_or_no", async () => {
  await open();
  const text = "The authenticity of host 'h' can't be established.\nAre you sure?";
  const yes = await ask({ type: "hostKey", text });

  await dialog("SSH Confirmation");
  expect(screen.getByText(/The authenticity of host/).textContent).toBe(text);
  expect(screen.queryByRole("textbox")).toBeNull();
  expect(document.activeElement).toBe(button("No"));
  await fireEvent.click(button("Yes"));
  expect(answers()).toEqual([{ id: yes, answer: "yes" }]);

  const no = await ask({ type: "hostKey", text });
  await dialog("SSH Confirmation");
  await fireEvent.click(button("No"));
  expect(answers()).toEqual([
    { id: yes, answer: "yes" },
    { id: no, answer: "no" },
  ]);
});

test("cancel_answers_null", async () => {
  await open();
  const id = await ask(USERNAME);
  await dialog("Log In to https://github.com");

  await fireEvent.click(button("Cancel"));

  expect(answers()).toEqual([{ id, answer: null }]);
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("escape_answers_null", async () => {
  await open();
  const id = await ask({ type: "hostKey", text: "Are you sure?" });
  const shown = await dialog("SSH Confirmation");

  await fireEvent.keyDown(shown, { key: "Escape" });

  expect(answers()).toEqual([{ id, answer: null }]);
});

test("password_does_not_stay_in_the_state_on_close", async () => {
  await open();
  await ask(PASSWORD);
  await dialog("Log In to https://github.com");
  await fireEvent.input(passwordField(), { target: { value: "secret" } });
  await fireEvent.click(button("Cancel"));

  await ask(PASSWORD);
  await dialog("Log In to https://github.com");
  expect(passwordField().value).toBe("");
  await fireEvent.input(passwordField(), { target: { value: "secret" } });
  await fireEvent.click(button("Log In"));

  await ask({ type: "passphrase", key: "k" });
  await dialog("SSH Key Passphrase");
  expect((screen.getByLabelText(/Password for the SSH key/) as HTMLInputElement).value).toBe("");
});

test("if git does not ask for the password, the dialog closes and forgets it", async () => {
  await open();
  vi.useFakeTimers();
  const first = await ask(USERNAME);
  await vi.advanceTimersByTimeAsync(0);
  await fireEvent.input(usernameField(), { target: { value: "me" } });
  await fireEvent.input(passwordField(), { target: { value: "secret" } });
  await fireEvent.click(button("Log In"));
  expect(screen.queryByRole("dialog")).toBeTruthy();

  await vi.advanceTimersByTimeAsync(60_000);

  expect(screen.queryByRole("dialog")).toBeNull();
  const late = await ask(PASSWORD);
  await vi.advanceTimersByTimeAsync(0);
  expect(answers()).toEqual([{ id: first, answer: "me" }]);
  expect(passwordField().value).toBe("");
  expect(late).toBeGreaterThan(first);
});

test("a_withdrawn_question_closes_the_dialog", async () => {
  await open();
  const id = await ask(USERNAME);
  await dialog("Log In to https://github.com");
  await fireEvent.input(usernameField(), { target: { value: "me" } });
  await fireEvent.input(passwordField(), { target: { value: "secret" } });

  await dismiss(id);

  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(answers()).toEqual([]);
  await ask(USERNAME);
  await dialog("Log In to https://github.com");
  expect(usernameField().value).toBe("");
  expect(passwordField().value).toBe("");
});

test("questions are handled one at a time", async () => {
  await open();
  const first = await ask({ type: "sshPassword", user: "a@h" });
  const second = await ask({ type: "sshPassword", user: "b@h" });
  const third = await ask({ type: "sshPassword", user: "c@h" });
  await screen.findByText("Password for user a@h:");

  await dismiss(second);
  await fireEvent.click(button("Cancel"));

  await screen.findByText("Password for user c@h:");
  expect(answers()).toEqual([{ id: first, answer: null }]);
  expect(third).toBeGreaterThan(first);
});
