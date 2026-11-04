import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { get } from "svelte/store";
import { protectedBranches } from "../branches/state";
import type { Settings } from "../lib/api";
import { dismiss, notifications } from "../lib/notifications";
import SettingsView from "./SettingsView.svelte";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const SETTINGS: Settings = {
  gitPath: null,
  stagingArea: false,
  warnCrlf: true,
  warnDetached: true,
  signCommits: false,
  signingKey: null,
  useCredentialHelper: true,
  updateMethod: "merge",
  showUpdateDialog: true,
  updatePaths: [],
  incomingCheck: "auto",
  pushAutoUpdate: false,
  previewPushOnCommit: true,
  previewPushProtectedOnly: false,
  mergeAutoApply: false,
  mergeWhitespace: "default",
  cherryPickSuffix: true,
};

function mockCommands(commands: Record<string, (args: never) => unknown> = {}) {
  const all: Record<string, (args: never) => unknown> = {
    git_version: () => "2.38.0",
    settings_save: (args) => (args as unknown as { settings: Settings }).settings,
    ...commands,
  };
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    const handler = all[command];
    if (!handler) throw new Error(`unexpected command: ${command}`);
    return handler(args as never);
  });
}

function calls(command: string) {
  return vi.mocked(invoke).mock.calls.filter(([name]) => name === command);
}

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  for (const { id } of get(notifications)) dismiss(id);
});

afterEach(cleanup);

function open(settings: Partial<Settings> = {}) {
  const onclose = vi.fn();
  render(SettingsView, { settings: { ...SETTINGS, ...settings }, onclose });
  return onclose;
}

test("incoming_check_is_shown_and_saved", async () => {
  mockCommands();
  open();
  const select = screen.getByLabelText(
    "Explicitly check for incoming commits on remotes:",
  ) as HTMLSelectElement;
  expect(select.value).toBe("auto");
  expect([...select.options].map((option) => option.text)).toEqual(["Auto", "Always", "Never"]);

  await fireEvent.change(select, { target: { value: "never" } });
  await fireEvent.click(ok());

  await waitFor(() => expect(calls("settings_save")).toHaveLength(1));
  const saved = (calls("settings_save")[0][1] as { settings: Settings }).settings;
  expect(saved.incomingCheck).toBe("never");
});

test("use credential helper is shown and saved", async () => {
  mockCommands();
  open();
  const helper = checkbox("Use credential helper");
  expect(helper.checked).toBe(true);

  await fireEvent.click(helper);
  await fireEvent.click(ok());

  await waitFor(() => expect(calls("settings_save")).toHaveLength(1));
  const saved = (calls("settings_save")[0][1] as { settings: Settings }).settings;
  expect(saved.useCredentialHelper).toBe(false);
});

const checkbox = (name: string) => screen.getByRole("checkbox", { name }) as HTMLInputElement;
const pathField = () =>
  screen.getByRole("textbox", { name: "Path to Git executable:" }) as HTMLInputElement;
const ok = () => screen.getByRole("button", { name: "OK" });

test("shows the values and the IntelliJ texts", async () => {
  mockCommands();
  open({ warnCrlf: false, stagingArea: true });

  expect(checkbox("Warn if CRLF line separators are about to be committed").checked).toBe(false);
  expect(checkbox("Warn when committing in detached HEAD or during rebase").checked).toBe(true);
  expect(checkbox("Enable staging area").checked).toBe(true);
  expect(
    screen.getByText("This will disable changelists support. Only for non-modal commit interface."),
  ).toBeTruthy();
  expect(screen.getByText("Commits signing with GPG key is not configured")).toBeTruthy();
  expect(await screen.findByText("Git version is 2.38.0")).toBeTruthy();
});

test("cherry_pick_suffix_setting_on_the_git_screen", async () => {
  mockCommands();
  open();
  const suffix = checkbox(
    "Add the 'cherry-picked from <hash>' suffix when picking commits pushed to protected branches",
  );
  // `ADD_SUFFIX_TO_CHERRY_PICKS_OF_PUBLISHED_COMMITS` is on by default.
  expect(suffix.checked).toBe(true);

  await fireEvent.click(suffix);
  await fireEvent.click(ok());

  await waitFor(() => expect(calls("settings_save")).toHaveLength(1));
  const saved = (calls("settings_save")[0][1] as { settings: Settings }).settings;
  expect(saved.cherryPickSuffix).toBe(false);
});

test("OK saves what changed and closes", async () => {
  mockCommands();
  const onclose = open();

  await fireEvent.click(checkbox("Enable staging area"));
  await fireEvent.click(checkbox("Warn if CRLF line separators are about to be committed"));
  await fireEvent.input(pathField(), { target: { value: "/opt/git" } });
  await fireEvent.click(ok());

  await waitFor(() => expect(onclose).toHaveBeenCalled());
  expect(calls("settings_save")[0][1]).toEqual({
    settings: { ...SETTINGS, stagingArea: true, warnCrlf: false, gitPath: "/opt/git" },
  });
});

test("Cancel does not save", async () => {
  mockCommands();
  const onclose = open();

  await fireEvent.click(checkbox("Enable staging area"));
  await fireEvent.click(screen.getByRole("button", { name: "Cancel" }));

  expect(onclose).toHaveBeenCalled();
  expect(calls("settings_save")).toHaveLength(0);
});

test("the signing key is only edited with signing on and is saved", async () => {
  mockCommands();
  const onclose = open();

  expect(screen.queryByRole("textbox", { name: "GPG key" })).toBeNull();
  await fireEvent.click(checkbox("Sign commits with GPG key:"));
  await fireEvent.input(screen.getByRole("textbox", { name: "GPG key" }), {
    target: { value: "ABC123" },
  });
  expect(screen.getByText("Commits are signed with GPG key: ABC123")).toBeTruthy();
  await fireEvent.click(ok());

  await waitFor(() => expect(onclose).toHaveBeenCalled());
  expect(calls("settings_save")[0][1]).toEqual({
    settings: { ...SETTINGS, signCommits: true, signingKey: "ABC123" },
  });
});

test("Test tries the typed path without saving it", async () => {
  mockCommands({
    git_version: (args) =>
      (args as unknown as { path: string | null }).path === "/other/git" ? "2.40.1" : "2.38.0",
  });
  open();

  await fireEvent.input(pathField(), { target: { value: "/other/git" } });
  await fireEvent.click(screen.getByRole("button", { name: "Test" }));

  expect(await screen.findByText("Git version is 2.40.1")).toBeTruthy();
  expect(calls("settings_save")).toHaveLength(0);
});

test("a path that does not exist is reported on open", async () => {
  mockCommands({
    git_version: () => {
      throw { kind: "git_not_found", message: "Not found" };
    },
  });
  open({ gitPath: "/no/such/git" });

  expect((await screen.findByRole("alert")).textContent).toContain("No such file: /no/such/git");
});

test("changing the path clears the previous result", async () => {
  mockCommands();
  open();
  await screen.findByText("Git version is 2.38.0");

  await fireEvent.input(pathField(), { target: { value: "/other/git" } });

  expect(screen.queryByText("Git version is 2.38.0")).toBeNull();
});

test("if it cannot be saved it says so and does not close", async () => {
  mockCommands({
    settings_save: () => {
      throw { kind: "failed", message: "The settings could not be saved: disk full" };
    },
  });
  const onclose = open();

  await fireEvent.click(ok());

  expect((await screen.findByRole("alert")).textContent).toContain("disk full");
  expect(onclose).not.toHaveBeenCalled();
});

const updatePaths = () =>
  screen.getByRole("textbox", {
    name: 'Filter "Update Project" information by paths:',
  }) as HTMLTextAreaElement;

test("update_path_filter_is_shown_one_per_line_and_empty_is_all", async () => {
  mockCommands();
  open({ updatePaths: ["src", "docs/a b.md"] });

  expect(updatePaths().value).toBe("src\ndocs/a b.md");
  cleanup();
  open();
  expect(updatePaths().value).toBe("");
  expect(updatePaths().placeholder).toBe("All");
});

test("update_path_filter_is_saved_as_a_list_without_empty_lines", async () => {
  mockCommands();
  const onclose = open();

  await fireEvent.input(updatePaths(), { target: { value: "src\n\n  docs  \nsrc\n" } });
  await fireEvent.click(ok());

  await waitFor(() => expect(onclose).toHaveBeenCalled());
  expect(calls("settings_save")[0][1]).toEqual({
    settings: { ...SETTINGS, updatePaths: ["src", "docs"] },
  });
});

test("without_touching_the_path_filter_it_is_saved_all_the_same", async () => {
  mockCommands();
  open({ updatePaths: ["src"] });

  await fireEvent.click(ok());

  await waitFor(() => expect(calls("settings_save")).toHaveLength(1));
  expect((calls("settings_save")[0][1] as { settings: Settings }).settings.updatePaths).toEqual([
    "src",
  ]);
});

const openReady = async () => {
  open();
  await screen.findByText(/Git version/);
};

const repoCommands = {
  repo_settings_get: () => ({ protectedBranches: ["master", "main"], forcePushTargets: [] }),
  protected_branches_set: () => [],
};

test("push_and_update_rows_with_their_texts", async () => {
  mockCommands(repoCommands);
  await openReady();

  expect(await screen.findByLabelText("Protected branches:")).toBeTruthy();
  for (const name of [
    "Auto-update if push of the current branch was rejected",
    "Show Push dialog for Commit and Push",
    "Show Push dialog only when committing to protected branches",
  ]) {
    expect(screen.getByRole("checkbox", { name })).toBeTruthy();
  }
  expect(screen.getByRole("group", { name: "Update method:" })).toBeTruthy();
  expect(screen.getByRole("radio", { name: "Merge" })).toBeTruthy();
  expect(screen.getByRole("radio", { name: "Rebase" })).toBeTruthy();
  expect(screen.getAllByText('Filter "Update Project" information by paths:')).toHaveLength(1);
  expect(screen.getAllByText("Explicitly check for incoming commits on remotes:")).toHaveLength(1);
});

test("clean_working_tree_is_a_fixed_stash", async () => {
  mockCommands();
  await openReady();

  const group = screen.getByRole("group", { name: "Clean working tree using:" });
  const stash = screen.getByRole("radio", { name: "Stash" }) as HTMLInputElement;
  expect(group.contains(stash)).toBe(true);
  expect(stash.checked).toBe(true);
  expect(stash.disabled).toBe(true);
});

test("saves_the_push_and_update_settings", async () => {
  mockCommands();
  await openReady();

  await fireEvent.click(
    screen.getByRole("checkbox", {
      name: "Auto-update if push of the current branch was rejected",
    }),
  );
  await fireEvent.click(
    screen.getByRole("checkbox", {
      name: "Show Push dialog only when committing to protected branches",
    }),
  );
  await fireEvent.click(screen.getByRole("radio", { name: "Rebase" }));
  await fireEvent.click(ok());

  await waitFor(() => expect(calls("settings_save")).toHaveLength(1));
  expect((calls("settings_save")[0][1] as { settings: Settings }).settings).toMatchObject({
    pushAutoUpdate: true,
    previewPushProtectedOnly: true,
    updateMethod: "rebase",
  });
});

test("only_protected_depends_on_showing_the_dialog", async () => {
  mockCommands();
  await openReady();
  const only = screen.getByRole("checkbox", {
    name: "Show Push dialog only when committing to protected branches",
  }) as HTMLInputElement;

  expect(only.disabled).toBe(false);
  await fireEvent.click(
    screen.getByRole("checkbox", { name: "Show Push dialog for Commit and Push" }),
  );

  expect(only.disabled).toBe(true);
});

test("protected_branches_belong_to_the_repository_separated_by_semicolons", async () => {
  mockCommands(repoCommands);
  await openReady();

  const field = (await screen.findByLabelText("Protected branches:")) as HTMLInputElement;
  expect(field.value).toBe("master;main");
  await fireEvent.input(field, { target: { value: " release.* ; ; main" } });
  await fireEvent.click(ok());

  await waitFor(() => expect(calls("protected_branches_set")).toHaveLength(1));
  expect(calls("protected_branches_set")[0][1]).toEqual({ patterns: ["release.*", "main"] });
  await waitFor(() => expect(calls("settings_save")).toHaveLength(1));
  expect(get(protectedBranches)).toEqual(["release.*", "main"]);
});

test("an_invalid_pattern_is_reported_when_saving", async () => {
  mockCommands({ ...repoCommands, protected_branches_set: () => ["("] });
  const onclose = open();

  const field = await screen.findByLabelText("Protected branches:");
  await fireEvent.input(field, { target: { value: "(;main" } });
  await fireEvent.click(ok());

  await waitFor(() => expect(onclose).toHaveBeenCalled());
  expect(get(notifications)).toMatchObject([
    { kind: "warning", title: "Invalid protected branch pattern", body: "(" },
  ]);
});

test("without_a_repository_there_is_no_protected_row", async () => {
  mockCommands();
  await openReady();

  expect(screen.queryByLabelText("Protected branches:")).toBeNull();
  await fireEvent.click(ok());

  await waitFor(() => expect(calls("settings_save")).toHaveLength(1));
  expect(calls("protected_branches_set")).toHaveLength(0);
});

// DiffSettingsConfigurable: the Merge group of Diff & Merge.
test("the_apply_non_conflicting_setting_is_changed_from_the_window", async () => {
  mockCommands();
  open();
  const check = screen.getByRole("checkbox", {
    name: "Automatically apply non-conflicting changes",
  }) as HTMLInputElement;
  expect(check.checked).toBe(false);

  await fireEvent.click(check);
  await fireEvent.click(screen.getByRole("button", { name: "OK" }));

  await waitFor(() => expect(calls("settings_save")).toHaveLength(1));
  expect((calls("settings_save")[0][1] as { settings: Settings }).settings.mergeAutoApply).toBe(
    true,
  );
});
