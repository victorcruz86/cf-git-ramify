import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { get } from "svelte/store";
import { protectedBranches } from "../branches/state";
import { commitMode, effectiveStaging, type Settings } from "../lib/api";
import { dismiss, notifications } from "../lib/notifications";
import SettingsView from "./SettingsView.svelte";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn() }));

const SETTINGS: Settings = {
  gitPath: null,
  stagingArea: false,
  warnCrlf: true,
  warnDetached: true,
  warnLargeFiles: true,
  largeFilesLimitMb: 50,
  warnBadFileNames: true,
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
  nonModalCommit: false,
  clearInitialCommitMessage: false,
  showDropCommitDialog: true,
  githubConnectionTimeoutMs: 5000,
};

function mockCommands(commands: Record<string, (args: never) => unknown> = {}) {
  const all: Record<string, (args: never) => unknown> = {
    git_version: () => "2.38.0",
    github_accounts: () => [],
    gpg_config: () => ({ sign: false, key: null, ssh: false }),
    gpg_keys: () => [],
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
const limitField = () =>
  screen.getByRole("spinbutton", { name: "Large file limit in MB" }) as HTMLInputElement;

test("shows the values and the IntelliJ texts", async () => {
  mockCommands();
  open({ warnCrlf: false, stagingArea: true, nonModalCommit: true });

  expect(checkbox("Warn if CRLF line separators are about to be committed").checked).toBe(false);
  expect(checkbox("Warn when committing in detached HEAD or during rebase").checked).toBe(true);
  expect(checkbox("Enable staging area").checked).toBe(true);
  expect(
    screen.getByText("This will disable changelists support. Only for non-modal commit interface."),
  ).toBeTruthy();
  expect(await screen.findByText("Git version is 2.38.0")).toBeTruthy();
});

test("the Commit section shows Use non-modal commit interface with its comment and saves it", async () => {
  mockCommands();
  open();
  // The `Commit` page comes before `Git`, whose own `Commit` group keeps its place.
  const headings = screen.getAllByRole("heading", { level: 3 }).map((heading) => heading.textContent);
  expect(headings.slice(0, 3)).toEqual(["Commit", "Git", "Commit"]);
  const nonModal = checkbox("Use non-modal commit interface");
  // Modal by default: the commit is a dialog.
  expect(nonModal.checked).toBe(false);
  expect(screen.getByText("Applies to projects under Git and Mercurial")).toBeTruthy();
  const clear = checkbox("Clear initial commit message");
  expect(clear.checked).toBe(false);

  await fireEvent.click(nonModal);
  await fireEvent.click(clear);
  await fireEvent.click(ok());

  await waitFor(() => expect(calls("settings_save")).toHaveLength(1));
  const saved = (calls("settings_save")[0][1] as { settings: Settings }).settings;
  expect(saved.nonModalCommit).toBe(true);
  expect(saved.clearInitialCommitMessage).toBe(true);
});

test("the Confirmation section asks to confirm dropping commits and saves it", async () => {
  mockCommands();
  open();
  const headings = screen.getAllByRole("heading", { level: 3 }).map((heading) => heading.textContent);
  expect(headings.indexOf("Confirmation")).toBeGreaterThan(headings.lastIndexOf("Update"));
  const drop = checkbox("Ask for confirmation to drop commits");
  expect(drop.checked).toBe(true);
  expect(screen.getByText("Applicable to Git")).toBeTruthy();

  await fireEvent.click(drop);
  await fireEvent.click(ok());

  await waitFor(() => expect(calls("settings_save")).toHaveLength(1));
  const saved = (calls("settings_save")[0][1] as { settings: Settings }).settings;
  expect(saved.showDropCommitDialog).toBe(false);
});

test("Enable staging area is greyed out while the commit interface is modal", async () => {
  mockCommands();
  open({ stagingArea: true });
  const staging = checkbox("Enable staging area");
  // Greyed out, and what was saved is kept as it is.
  expect(staging.disabled).toBe(true);
  expect(staging.checked).toBe(true);

  // Enabled again as soon as the checkbox above is ticked, without saving.
  await fireEvent.click(checkbox("Use non-modal commit interface"));
  expect(staging.disabled).toBe(false);
  expect(staging.checked).toBe(true);
  expect(calls("settings_save")).toHaveLength(0);

  await fireEvent.click(checkbox("Use non-modal commit interface"));
  expect(staging.disabled).toBe(true);
  expect(staging.checked).toBe(true);
});

test("staging is effective only in non-modal mode", () => {
  const modal = { ...SETTINGS, nonModalCommit: false, stagingArea: true };
  expect(effectiveStaging(modal)).toBe(false);
  expect(commitMode(modal)).toBe("modal");

  const nonModal = { ...SETTINGS, nonModalCommit: true, stagingArea: true };
  expect(effectiveStaging(nonModal)).toBe(true);
  expect(commitMode(nonModal)).toBe("non-modal");

  expect(effectiveStaging({ ...SETTINGS, nonModalCommit: true, stagingArea: false })).toBe(false);
  expect(effectiveStaging({ ...SETTINGS, nonModalCommit: false, stagingArea: false })).toBe(false);
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

test("the two new rows are on the Git settings page and are saved", async () => {
  mockCommands();
  open();
  const large = checkbox("Warn when committing files larger than");
  const limit = limitField();
  const names = checkbox("Warn when committing files with names that might cause issues on other systems");
  // `GitVcsOptions`: both on, with 50 MB.
  expect(large.checked).toBe(true);
  expect(limit.value).toBe("50");
  expect(names.checked).toBe(true);
  expect(screen.getByText("MB")).toBeTruthy();

  await fireEvent.input(limit, { target: { value: "75" } });
  await fireEvent.click(names);
  await fireEvent.click(ok());

  await waitFor(() => expect(calls("settings_save")).toHaveLength(1));
  const saved = (calls("settings_save")[0][1] as { settings: Settings }).settings;
  expect(saved.largeFilesLimitMb).toBe(75);
  expect(saved.warnBadFileNames).toBe(false);
  expect(saved.warnLargeFiles).toBe(true);
});

test("the limit is greyed out while the large file warning is off and stays in 1 to 100000", async () => {
  mockCommands();
  open({ warnLargeFiles: false, largeFilesLimitMb: 20 });
  const limit = limitField();

  expect(limit.disabled).toBe(true);
  expect(limit.value).toBe("20");
  expect(limit.min).toBe("1");
  expect(limit.max).toBe("100000");

  await fireEvent.click(checkbox("Warn when committing files larger than"));

  expect(limit.disabled).toBe(false);
});

test("an empty limit goes back to the saved one and one outside 1 to 100000 is not saved", async () => {
  mockCommands();
  open({ largeFilesLimitMb: 20 });

  // The field refuses the value (`min` and `max`), as the spinner of IntelliJ keeps to its range.
  await fireEvent.input(limitField(), { target: { value: "999999" } });
  await fireEvent.click(ok());
  expect(limitField().validity.valid).toBe(false);
  expect(calls("settings_save")).toHaveLength(0);

  await fireEvent.input(limitField(), { target: { value: "" } });
  await fireEvent.click(ok());

  await waitFor(() => expect(calls("settings_save")).toHaveLength(1));
  const saved = (calls("settings_save")[0][1] as { settings: Settings }).settings;
  expect(saved.largeFilesLimitMb).toBe(20);
});

test("OK saves what changed and closes", async () => {
  mockCommands();
  const onclose = open({ nonModalCommit: true });

  await fireEvent.click(checkbox("Enable staging area"));
  await fireEvent.click(checkbox("Warn if CRLF line separators are about to be committed"));
  await fireEvent.input(pathField(), { target: { value: "/opt/git" } });
  await fireEvent.click(ok());

  await waitFor(() => expect(onclose).toHaveBeenCalled());
  expect(calls("settings_save")[0][1]).toEqual({
    settings: {
      ...SETTINGS,
      nonModalCommit: true,
      stagingArea: true,
      warnCrlf: false,
      gitPath: "/opt/git",
    },
  });
});

test("Cancel does not save", async () => {
  mockCommands();
  const onclose = open({ nonModalCommit: true });

  await fireEvent.click(checkbox("Enable staging area"));
  await fireEvent.click(screen.getByRole("button", { name: "Cancel" }));

  expect(onclose).toHaveBeenCalled();
  expect(calls("settings_save")).toHaveLength(0);
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
  repo_settings_get: () => ({ protectedBranches: ["master", "main"], forcePushTargets: [], cleanTree: "shelve" }),
  protected_branches_set: () => [],
  clean_tree_set: () => null,
  protected_patterns_check: () => null,
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

// G:707 and G:1385-1386, per repository (`GitVcsPanel`), Shelve by default (decision 9).
test("the_radio_saves_per_repository_and_defaults_to_shelve", async () => {
  mockCommands(repoCommands);
  await openReady();

  const group = screen.getByRole("group", { name: "Clean working tree using:" });
  const stash = screen.getByRole("radio", { name: "Stash" }) as HTMLInputElement;
  const shelve = screen.getByRole("radio", { name: "Shelve" }) as HTMLInputElement;
  expect(group.contains(stash) && group.contains(shelve)).toBe(true);
  await waitFor(() => expect(shelve.checked).toBe(true));
  expect(stash.checked).toBe(false);
  expect(shelve.disabled).toBe(false);

  await fireEvent.click(stash);
  await fireEvent.click(ok());

  await waitFor(() => expect(calls("clean_tree_set")).toHaveLength(1));
  expect(calls("clean_tree_set")[0][1]).toEqual({ cleanTree: "stash" });
  await waitFor(() => expect(calls("settings_save")).toHaveLength(1));
  expect((calls("settings_save")[0][1] as { settings: Settings }).settings).not.toHaveProperty("cleanTree");
});

// No value is shown before the repository's is read: a greyed-out Shelve would be wrong for one
// that saves with Stash.
test("the_radio_shows_nothing_while_it_loads", async () => {
  let answer = (_: unknown) => {};
  mockCommands({
    ...repoCommands,
    repo_settings_get: () => new Promise((resolve) => (answer = resolve)),
  });
  open();

  const group = screen.getByRole("group", { name: "Clean working tree using:" });
  expect(group.querySelectorAll("input:checked")).toHaveLength(0);
  answer({ protectedBranches: [], forcePushTargets: [], cleanTree: "stash" });
  const stash = () => screen.getByRole("radio", { name: "Stash" }) as HTMLInputElement;
  await waitFor(() => expect(stash().checked).toBe(true));
  expect(stash().disabled).toBe(false);
});

test("the_radio_shows_what_the_repository_saved_and_is_not_written_unchanged", async () => {
  mockCommands({
    ...repoCommands,
    repo_settings_get: () => ({ protectedBranches: [], forcePushTargets: [], cleanTree: "stash" }),
  });
  await openReady();

  const stash = screen.getByRole("radio", { name: "Stash" }) as HTMLInputElement;
  await waitFor(() => expect(stash.checked).toBe(true));
  await fireEvent.click(ok());

  await waitFor(() => expect(calls("settings_save")).toHaveLength(1));
  expect(calls("clean_tree_set")).toHaveLength(0);
});

test("without_a_repository_the_radio_is_greyed_out_on_shelve", async () => {
  mockCommands();
  await openReady();

  const shelve = screen.getByRole("radio", { name: "Shelve" }) as HTMLInputElement;
  expect(shelve.checked).toBe(true);
  expect(shelve.disabled).toBe(true);
  expect((screen.getByRole("radio", { name: "Stash" }) as HTMLInputElement).disabled).toBe(true);
  await fireEvent.click(ok());
  await waitFor(() => expect(calls("settings_save")).toHaveLength(1));
  expect(calls("clean_tree_set")).toHaveLength(0);
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
  expect(calls("protected_branches_set")[0][1]).toEqual({
    patterns: ["release.*", "main"],
    loadGithubProtection: true,
  });
  await waitFor(() => expect(calls("settings_save")).toHaveLength(1));
  expect(get(protectedBranches)).toEqual(["release.*", "main"]);
});

const BAD = "Skipped invalid protected branch pattern '(': unclosed group";
const check = (text: string) => (text.split(";").map((part) => part.trim()).includes("(") ? BAD : null);

test("an_invalid_pattern_is_shown_while_typing", async () => {
  mockCommands({
    ...repoCommands,
    protected_patterns_check: (args) => check((args as unknown as { text: string }).text),
  });
  open();

  const field = await screen.findByLabelText("Protected branches:");
  expect(screen.queryByText(BAD)).toBeNull();
  await fireEvent.input(field, { target: { value: "main;(" } });

  expect(await screen.findByText(BAD)).toBeTruthy();
  await fireEvent.input(field, { target: { value: "main" } });
  await waitFor(() => expect(screen.queryByText(BAD)).toBeNull());
});

test("a_bad_saved_value_is_shown_on_opening", async () => {
  mockCommands({
    ...repoCommands,
    repo_settings_get: () => ({ protectedBranches: ["main", "("], forcePushTargets: [] }),
    protected_patterns_check: (args) => check((args as unknown as { text: string }).text),
  });
  open();

  expect(await screen.findByText(BAD)).toBeTruthy();
});

test("a_bad_pattern_does_not_block_saving_and_leaves_no_notification", async () => {
  mockCommands({
    ...repoCommands,
    protected_branches_set: () => ["("],
    protected_patterns_check: (args) => check((args as unknown as { text: string }).text),
  });
  const onclose = open();

  const field = await screen.findByLabelText("Protected branches:");
  await fireEvent.input(field, { target: { value: "(;main" } });
  await screen.findByText(BAD);
  await fireEvent.click(ok());

  await waitFor(() => expect(onclose).toHaveBeenCalled());
  expect(calls("protected_branches_set")[0][1]).toEqual({
    patterns: ["(", "main"],
    loadGithubProtection: true,
  });
  expect(get(notifications)).toEqual([]);
});

// P§4.4: GitHub's rules follow the user's text in grey, as the field's empty text, and are not saved.
const withGithub = (repoSettings: Record<string, unknown> = {}) => ({
  ...repoCommands,
  repo_settings_get: () => ({
    protectedBranches: ["main"],
    forcePushTargets: [],
    cleanTree: "shelve",
    loadGithubProtection: true,
    githubProtected: ["release\\/.*", "v1"],
    ...repoSettings,
  }),
});

const suffix = () => document.querySelector(".read-only-suffix")?.textContent ?? null;

test("the_github_rules_follow_the_users_in_grey_and_are_not_saved", async () => {
  mockCommands(withGithub());
  open();

  const field = (await screen.findByLabelText("Protected branches:")) as HTMLInputElement;
  expect(field.value).toBe("main");
  await waitFor(() => expect(suffix()).toBe(";release\\/.*;v1"));

  // While the text differs from the painted one the suffix is hidden; focus loss paints it again.
  await fireEvent.input(field, { target: { value: "main;dev" } });
  expect(suffix()).toBeNull();
  await fireEvent.blur(field);
  expect(suffix()).toBe(";release\\/.*;v1");
  expect(document.querySelector(".read-only .typed")?.textContent).toBe("main;dev");

  await fireEvent.click(ok());
  await waitFor(() => expect(calls("protected_branches_set")).toHaveLength(1));
  expect(calls("protected_branches_set")[0][1]).toEqual({
    patterns: ["main", "dev"],
    loadGithubProtection: true,
  });
  expect(get(protectedBranches)).toEqual(["main", "dev", "release\\/.*", "v1"]);
});

test("a_blank_field_shows_the_github_rules_without_a_separator_after_focus_loss", async () => {
  mockCommands(withGithub({ protectedBranches: [] }));
  open();

  const field = await screen.findByLabelText("Protected branches:");
  await screen.findByText("GitHub rules are added to the local rules and synced on every fetch");
  // `setText` paints nothing for a blank text.
  expect(suffix()).toBeNull();
  await fireEvent.blur(field);
  expect(suffix()).toBe("release\\/.*;v1");
});

test("the_checkbox_is_saved_and_off_leaves_the_github_rules_out", async () => {
  mockCommands(withGithub());
  open();

  const box = (await screen.findByRole("checkbox", {
    name: "Load branch protection rules from GitHub",
  })) as HTMLInputElement;
  expect(box.checked).toBe(true);
  expect(
    screen.getByText("GitHub rules are added to the local rules and synced on every fetch"),
  ).toBeTruthy();
  await fireEvent.click(box);
  // As in 241, the suffix stays until the page is opened again.
  await waitFor(() => expect(suffix()).toBe(";release\\/.*;v1"));
  await fireEvent.click(ok());

  await waitFor(() => expect(calls("protected_branches_set")).toHaveLength(1));
  expect(calls("protected_branches_set")[0][1]).toEqual({
    patterns: ["main"],
    loadGithubProtection: false,
  });
  expect(get(protectedBranches)).toEqual(["main"]);
});

test("with_the_checkbox_off_there_is_no_read_only_text", async () => {
  mockCommands(withGithub({ loadGithubProtection: false }));
  open();

  const box = (await screen.findByRole("checkbox", {
    name: "Load branch protection rules from GitHub",
  })) as HTMLInputElement;
  expect(box.checked).toBe(false);
  await fireEvent.blur(screen.getByLabelText("Protected branches:"));
  expect(suffix()).toBeNull();
});

test("an_invalid_github_rule_is_reported_after_the_users", async () => {
  mockCommands({
    ...withGithub({ githubProtected: ["("] }),
    protected_patterns_check: (args) => check((args as unknown as { text: string }).text),
  });
  open();

  expect(await screen.findByText(BAD)).toBeTruthy();
  expect(calls("protected_patterns_check").map(([, args]) => args)).toEqual([
    { text: "main" },
    { text: "(" },
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

test("connection_timeout_saves_milliseconds_and_shows_seconds", async () => {
  mockCommands({ github_accounts: () => [] });
  open({ githubConnectionTimeoutMs: 7000 });
  const field = screen.getByLabelText(
    "Connection timeout:",
  ) as HTMLInputElement;
  expect(field.value).toBe("7");
  expect(screen.getByText("seconds")).toBeTruthy();

  await fireEvent.input(field, { target: { value: "12" } });
  await fireEvent.click(ok());
  await waitFor(() => expect(calls("settings_save")).toHaveLength(1));
  expect(
    (calls("settings_save")[0][1] as { settings: Settings }).settings
      .githubConnectionTimeoutMs,
  ).toBe(12000);
});

test("connection_timeout_accepts_zero_and_stays_within_zero_to_sixty", async () => {
  mockCommands({ github_accounts: () => [] });
  open();
  const field = screen.getByLabelText(
    "Connection timeout:",
  ) as HTMLInputElement;
  expect(field.min).toBe("0");
  expect(field.max).toBe("60");

  await fireEvent.input(field, { target: { value: "0" } });
  await fireEvent.click(ok());
  await waitFor(() => expect(calls("settings_save")).toHaveLength(1));
  expect(
    (calls("settings_save")[0][1] as { settings: Settings }).settings
      .githubConnectionTimeoutMs,
  ).toBe(0);
});

test("the_github_section_comes_after_the_git_sections_and_has_the_accounts_list", async () => {
  mockCommands({ github_accounts: () => [] });
  open();
  const headings = screen
    .getAllByRole("heading", { level: 3 })
    .map((heading) => heading.textContent);
  expect(headings[headings.length - 1]).toBe("GitHub");
  expect(headings.indexOf("GitHub")).toBeGreaterThan(headings.indexOf("Push"));
  expect(await screen.findByText(/No accounts added/)).toBeTruthy();
});

test("the_old_sign_commits_fields_are_gone", async () => {
  mockCommands();
  open();
  expect(await screen.findByText("Commits signing with GPG key is not configured")).toBeTruthy();
  expect(screen.queryByLabelText("Sign commits")).toBeNull();
  expect(screen.queryByText("Signing key")).toBeNull();
  expect(screen.queryByLabelText(/Signing key/)).toBeNull();
  expect(screen.getByRole("button", { name: "Configure GPG Key…" })).toBeTruthy();
});
