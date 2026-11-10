import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { taskResult } from "../lib/tasks";
import GpgKeyDialog from "./GpgKeyDialog.svelte";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(), emit: vi.fn() }));
vi.mock("../lib/tasks", () => ({ taskResult: vi.fn() }));

type Config = { sign: boolean; key: string | null; ssh: boolean };

function mock(config: Config, keys: { id: string; userId: string | null }[]) {
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "gpg_config") return config;
    if (command === "gpg_keys") return keys;
    if (command === "gpg_config_set") return 7;
    throw new Error(`unexpected command: ${command}`);
  });
}

const writes = () => vi.mocked(invoke).mock.calls.filter(([name]) => name === "gpg_config_set");

async function open() {
  const onclose = vi.fn();
  render(GpgKeyDialog, { onclose });
  await waitFor(() => expect(screen.queryByRole("status")).toBeNull());
  return onclose;
}

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(taskResult).mockReset();
  vi.mocked(taskResult).mockResolvedValue(null);
});

afterEach(cleanup);

test("the_dialog_lists_keys_with_their_user_id_and_selects_the_configured_one", async () => {
  mock({ sign: true, key: "BBBB", ssh: false }, [
    { id: "AAAA", userId: "Ann <ann@example.com>" },
    { id: "BBBB", userId: "Bob" },
  ]);
  await open();
  expect(screen.getByRole("dialog", { name: "Configure GPG Key" })).toBeTruthy();
  expect(screen.getByText("Synced with gitconfig")).toBeTruthy();
  const checkbox = screen.getByLabelText("Sign commits with GPG key:") as HTMLInputElement;
  expect(checkbox.checked).toBe(true);
  expect(document.activeElement).toBe(checkbox);
  const combo = screen.getByRole("combobox");
  expect(combo.textContent).toContain("BBBB");
  expect(combo.textContent).toContain("Bob");
  await fireEvent.click(combo);
  const options = screen.getAllByRole("option");
  expect(options.map((option) => option.textContent?.replace(/\s+/g, " ").trim())).toEqual([
    "AAAA Ann <ann@example.com>",
    "BBBB Bob",
  ]);
});

test("a_configured_key_that_is_not_in_the_list_is_still_shown_and_selected", async () => {
  mock({ sign: true, key: "GONE", ssh: false }, [{ id: "AAAA", userId: null }]);
  await open();
  expect(screen.getByRole("combobox").textContent).toContain("GONE");
});

test("an_empty_key_list_says_cannot_find_suitable_private_key", async () => {
  mock({ sign: false, key: null, ssh: false }, []);
  const onclose = await open();
  expect(screen.getByText("Cannot find suitable private key")).toBeTruthy();
  const ok = screen.getByRole("button", { name: "OK" }) as HTMLButtonElement;
  expect(ok.disabled).toBe(false);
  await fireEvent.click(screen.getByLabelText("Sign commits with GPG key:"));
  expect(ok.disabled).toBe(true);
  await fireEvent.click(screen.getByLabelText("Sign commits with GPG key:"));
  await fireEvent.click(ok);
  expect(writes()).toHaveLength(0);
  expect(onclose).toHaveBeenCalledWith(false);
});

test("ok_writes_the_selected_key_and_unchecked_writes_no_key", async () => {
  mock({ sign: false, key: null, ssh: false }, [
    { id: "AAAA", userId: null },
    { id: "BBBB", userId: "Bob" },
  ]);
  const onclose = await open();
  await fireEvent.click(screen.getByLabelText("Sign commits with GPG key:"));
  await fireEvent.click(screen.getByRole("combobox"));
  await fireEvent.click(screen.getByRole("option", { name: /BBBB/ }));
  await fireEvent.click(screen.getByRole("button", { name: "OK" }));
  await waitFor(() => expect(onclose).toHaveBeenCalledWith(true));
  expect(writes()).toEqual([["gpg_config_set", { key: "BBBB" }]]);

  cleanup();
  vi.mocked(invoke).mockReset();
  mock({ sign: true, key: "BBBB", ssh: false }, [{ id: "BBBB", userId: "Bob" }]);
  const second = await open();
  await fireEvent.click(screen.getByLabelText("Sign commits with GPG key:"));
  await fireEvent.click(screen.getByRole("button", { name: "OK" }));
  await waitFor(() => expect(second).toHaveBeenCalledWith(true));
  expect(writes()).toEqual([["gpg_config_set", { key: null }]]);
});

test("ok_with_nothing_changed_does_not_write", async () => {
  mock({ sign: true, key: "BBBB", ssh: false }, [{ id: "BBBB", userId: null }]);
  const onclose = await open();
  await fireEvent.click(screen.getByRole("button", { name: "OK" }));
  expect(onclose).toHaveBeenCalledWith(false);
  expect(writes()).toHaveLength(0);
});

test("with_ssh_the_dialog_shows_the_key_and_ok_with_a_gpg_key_shows_ramifys_refusal", async () => {
  mock({ sign: true, key: "/home/me/.ssh/id_ed25519.pub", ssh: true }, [{ id: "AAAA", userId: null }]);
  const onclose = await open();
  expect(screen.getByRole("combobox").textContent).toContain("/home/me/.ssh/id_ed25519.pub");
  vi.mocked(taskResult).mockRejectedValue({
    kind: "failed",
    message: "gpg.format is ssh: choose the SSH key, not a GPG key",
  });
  await fireEvent.click(screen.getByRole("combobox"));
  await fireEvent.click(screen.getByRole("option", { name: /AAAA/ }));
  await fireEvent.click(screen.getByRole("button", { name: "OK" }));
  expect((await screen.findByRole("alert")).textContent).toContain("gpg.format is ssh");
  expect(onclose).not.toHaveBeenCalled();
  expect(screen.getByRole("dialog")).toBeTruthy();
});

test("a_user_id_with_markup_is_text", async () => {
  mock({ sign: true, key: "AAAA", ssh: false }, [{ id: "AAAA", userId: "<b>Bold</b> <img src=x>" }]);
  await open();
  const combo = screen.getByRole("combobox");
  expect(combo.textContent).toContain("<b>Bold</b> <img src=x>");
  expect(combo.querySelector("b, img")).toBeNull();
});

// A ruling of Task 3: with no key configured, checking the box selects the first key.
test("checking_the_box_preselects_the_first_key", async () => {
  mock({ sign: false, key: null, ssh: false }, [
    { id: "AAAA", userId: "Ann" },
    { id: "BBBB", userId: "Bob" },
  ]);
  const onclose = await open();
  expect(screen.getByRole("combobox").textContent).toBe("");
  await fireEvent.click(screen.getByLabelText("Sign commits with GPG key:"));
  expect(screen.getByRole("combobox").textContent).toContain("AAAA");
  await fireEvent.click(screen.getByRole("button", { name: "OK" }));
  await waitFor(() => expect(onclose).toHaveBeenCalledWith(true));
  expect(writes()).toEqual([["gpg_config_set", { key: "AAAA" }]]);
});

// The final review: a config that cannot be read shows its error, the keys are still listed,
// and OK writes what the user chose.
test("a_failed_config_read_shows_its_error_and_ok_still_writes_the_choice", async () => {
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "gpg_config") throw { kind: "failed", message: "bad config line 3" };
    if (command === "gpg_keys") return [{ id: "AAAA", userId: null }];
    if (command === "gpg_config_set") return 7;
    throw new Error(`unexpected command: ${command}`);
  });
  const onclose = await open();
  expect(screen.getByRole("alert").textContent).toContain("bad config line 3");
  await fireEvent.click(screen.getByLabelText("Sign commits with GPG key:"));
  expect(screen.getByRole("combobox").textContent).toContain("AAAA");
  const ok = screen.getByRole("button", { name: "OK" }) as HTMLButtonElement;
  expect(ok.disabled).toBe(false);
  await fireEvent.click(ok);
  await waitFor(() => expect(onclose).toHaveBeenCalledWith(true));
  expect(writes()).toEqual([["gpg_config_set", { key: "AAAA" }]]);
});

test("a_failed_key_list_shows_only_its_error", async () => {
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "gpg_config") return { sign: false, key: null, ssh: false };
    if (command === "gpg_keys") throw { kind: "failed", message: "gpg exited with 2" };
    throw new Error(`unexpected command: ${command}`);
  });
  await open();
  const alerts = screen.getAllByRole("alert");
  expect(alerts.map((alert) => alert.textContent)).toEqual(["gpg exited with 2"]);
  expect(screen.queryByText("Cannot find suitable private key")).toBeNull();
});
