import { cleanup, render, screen } from "@testing-library/svelte";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import GpgKeyRow from "./GpgKeyRow.svelte";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({ label: "main" }),
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(), emitTo: vi.fn() }));

let handlers: (() => void)[] = [];

function mock(config: () => unknown, keys: () => unknown = () => []) {
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "gpg_config") return config();
    if (command === "gpg_keys") return keys();
    throw new Error(`unexpected command: ${command}`);
  });
}

beforeEach(() => {
  handlers = [];
  vi.mocked(invoke).mockReset();
  vi.mocked(listen).mockReset();
  vi.mocked(listen).mockImplementation(async (_event, handler) => {
    handlers.push(() => (handler as (event: unknown) => void)({ payload: null }));
    return () => {};
  });
});

afterEach(cleanup);

test("the_row_shows_241s_status_for_no_signing_a_known_key_and_an_unknown_key", async () => {
  let config = { sign: false, key: null as string | null, ssh: false };
  mock(
    () => config,
    () => [{ id: "ABCD1234", userId: "Ann <ann@example.com>" }],
  );
  render(GpgKeyRow, { onconfigure: vi.fn() });
  expect(await screen.findByText("Commits signing with GPG key is not configured")).toBeTruthy();
  expect(screen.getByRole("button", { name: "Configure GPG Key…" })).toHaveProperty("disabled", false);

  config = { sign: true, key: "ABCD1234", ssh: false };
  handlers.forEach((run) => run());
  expect(
    await screen.findByText("Commits are signed with GPG key: Ann <ann@example.com> (ABCD1234)"),
  ).toBeTruthy();

  config = { sign: true, key: "FFFF0000", ssh: false };
  handlers.forEach((run) => run());
  expect(await screen.findByText("Commits are signed with GPG key: FFFF0000")).toBeTruthy();
});

test("the_row_is_disabled_without_a_repository_and_shows_an_error_when_the_config_cannot_be_read", async () => {
  mock(() => Promise.reject({ kind: "not_a_repository", message: "No repository is open" }));
  render(GpgKeyRow, { onconfigure: vi.fn() });
  expect(await screen.findByText("No Git roots in the project")).toBeTruthy();
  expect(screen.getByRole("button", { name: "Configure GPG Key…" })).toHaveProperty("disabled", true);

  cleanup();
  mock(() => Promise.reject({ kind: "failed", message: "bad config" }));
  render(GpgKeyRow, { onconfigure: vi.fn() });
  expect((await screen.findByRole("alert")).textContent).toContain("bad config");
});
