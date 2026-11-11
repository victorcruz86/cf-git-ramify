import { get } from "svelte/store";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  ApplyPlan,
  PatchApplyResult,
  Changelists,
  ContentsMergeDone,
  FileChange,
  MergeContents,
  PatchConflict,
  PatchDefaults,
  PatchEntry,
  PatchPaths,
  PlannedFile,
} from "../lib/api";
import { dismiss, notifications } from "../lib/notifications";
import {
  BASE_EMPTY,
  NOT_ABSOLUTE,
  BASE_MISSING,
  COPIED,
  CREATION_FAILED,
  NAME_EMPTY,
  NAME_TOO_LONG,
  baseOutside,
  commonParent,
  creationError,
  fileExists,
  localEntries,
  logEntries,
  patchFlow,
  patchNamePrefix,
  settingsProblem,
  settingsWarning,
  stagingEntries,
  suggestPatchFile,
  type PatchApi,
  type PatchSettingsAnswer,
  type PatchUi,
  ABORTED_TEXT,
  ABORTED_TITLE,
  ALREADY_APPLIED_TEXT,
  APPLY_TITLE,
  CANNOT_APPLY_NOW,
  NOTIFICATION_TITLE,
  ON_THE_FLY_MAX,
  PARTIALLY_APPLIED_TEXT,
  REPOSITORY_SWITCHED,
  RESTORED,
  ROLLBACK_FAILED_TEXT,
  ROLLBACK_FAILED_TITLE,
  SELECT_PATCH_TITLE,
  SUCCESS_TEXT,
  SWITCHED_WHILE_APPLYING,
  andStatus,
  answerPatchDialog,
  applyInProgress,
  defaultApplyApi,
  patchDialogsUi,
  applyFlow,
  clipboardWatcher,
  type ApplyAnswer,
  type ApplyApi,
  type ApplyDialogRequest,
  type ApplyUi,
} from "./flow";
import sharedNames from "../../src-tauri/testdata/patch-names.json?raw";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({ label: "main" }),
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const ENTRY: PatchEntry = {
  path: "src/a.txt",
  oldPath: null,
  before: { kind: "head" },
  after: { kind: "disk" },
};

const paths = (file: string, existing: string[] = [], base = "/repo"): PatchPaths => ({
  file,
  fileExists: existing.includes(file),
  base,
  baseExists: base === "/repo" || base === "/repo/src",
});

function fakeApi(
  defaults: Partial<PatchDefaults> = {},
  existing: string[] = [],
): PatchApi & { [K in keyof PatchApi]: ReturnType<typeof vi.fn> } {
  return {
    create: vi.fn(async (_entries, _options, to) => ({
      file: to.kind === "file" ? to.path : null,
      snapshot: null,
    })),
    defaults: vi.fn(async () => ({ folder: null, toClipboard: false, showInFinder: null, ...defaults })),
    paths: vi.fn(async (file: string, base: string) => paths(file, existing, base)),
    showInFinder: vi.fn(async () => {}),
    reveal: vi.fn(async () => {}),
    restore: vi.fn(async () => "previous"),
  } as never;
}

function fakeUi(
  answer: Partial<PatchSettingsAnswer> | null = {},
  overwrite = true,
  created = { reveal: false, dontAsk: false },
): PatchUi & { [K in keyof PatchUi]: ReturnType<typeof vi.fn> } {
  return {
    settings: vi.fn(async (request) =>
      answer === null
        ? null
        : { file: request.file, toClipboard: request.toClipboard, base: request.base, reverse: false, ...answer },
    ),
    overwrite: vi.fn(async () => overwrite),
    created: vi.fn(async () => created),
  } as never;
}

const shown = () => get(notifications).map(({ kind, title, body }) => ({ kind, title, body }));
const dismissAll = () => get(notifications).forEach((notice) => dismiss(notice.id));

beforeEach(() => get(notifications).forEach((notice) => dismiss(notice.id)));
afterEach(() => vi.restoreAllMocks());

test("copy_as_patch_puts_the_selected_changes_on_the_clipboard_and_says_so", async () => {
  const api = fakeApi();
  const flow = patchFlow(fakeUi(), api);

  expect(await flow.copyAsPatch({ repo: "/repo", entries: [ENTRY] }, "The list comment")).toBe(true);

  expect(api.create.mock.calls).toEqual([
    [
      [ENTRY],
      { basePath: "/repo", reverse: false, message: "The list comment" },
      { kind: "clipboard" },
      false,
      "/repo",
    ],
  ]);
  expect(shown()).toEqual([{ kind: "info", title: COPIED, body: undefined }]);
});

test("a_failed_copy_says_patch_creation_failed_with_the_reason", async () => {
  const api = fakeApi();
  api.create.mockRejectedValue({ kind: "failed", message: '"image.bin" is binary' });

  expect(await patchFlow(fakeUi(), api).copyAsPatch({ repo: "/repo", entries: [ENTRY] })).toBe(false);

  expect(shown()).toEqual([{ kind: "error", title: CREATION_FAILED, body: '"image.bin" is binary' }]);
});

test("create_patch_suggests_the_message_as_the_file_in_the_remembered_folder", async () => {
  const api = fakeApi({ folder: "/Users/me/Downloads/PATCHS" });
  const ui = fakeUi();

  await patchFlow(ui, api).createPatch({ repo: "/repo", entries: [ENTRY] }, "Fix: the bug.\nMore");

  const request = ui.settings.mock.calls[0][0];
  expect(request.file).toBe("/Users/me/Downloads/PATCHS/Fix__the_bug__More.patch");
  expect(request.base).toBe("/repo");
  expect(request.commonParent).toBe("/repo/src");
  expect(api.create.mock.calls[0].slice(1)).toEqual([
    { basePath: "/repo", reverse: false, message: "Fix: the bug.\nMore" },
    { kind: "file", path: "/Users/me/Downloads/PATCHS/Fix__the_bug__More.patch" },
    true,
    "/repo",
  ]);
});

test("without_a_folder_or_message_it_is_unnamed_in_the_repository_and_numbered_if_taken", async () => {
  const ui = fakeUi(null);
  const api = fakeApi({}, ["/repo/unnamed.patch", "/repo/unnamed1.patch"]);

  await patchFlow(ui, api).createPatch({ repo: "/repo", entries: [ENTRY] }, null);

  expect(ui.settings.mock.calls[0][0].file).toBe("/repo/unnamed2.patch");
  expect(api.create).not.toHaveBeenCalled();
});

test("cancelling_the_settings_keeps_the_create_patch_dialog_open", async () => {
  const confirmed = vi.fn();
  const api = fakeApi();

  expect(await patchFlow(fakeUi(null), api).createPatch({ repo: "/repo", entries: [ENTRY] }, "m", confirmed)).toBe(false);

  expect(confirmed).not.toHaveBeenCalled();
  expect(api.create).not.toHaveBeenCalled();
});

test("reverse_patch_reaches_the_writer", async () => {
  const api = fakeApi();
  const confirmed = vi.fn();

  await patchFlow(fakeUi({ reverse: true }), api).createPatch({ repo: "/repo", entries: [ENTRY] }, "m", confirmed);

  expect(confirmed).toHaveBeenCalledTimes(1);
  expect(api.create.mock.calls[0][1]).toEqual({ basePath: "/repo", reverse: true, message: "m" });
});

// `vcs.patch.to.clipboard`: the radio starts as it was left, and accepting saves it.
test("to_clipboard_is_remembered", async () => {
  const api = fakeApi({ toClipboard: true });
  const ui = fakeUi();

  await patchFlow(ui, api).createPatch({ repo: "/repo", entries: [ENTRY] }, "m");

  expect(ui.settings.mock.calls[0][0].toClipboard).toBe(true);
  expect(api.create.mock.calls[0].slice(2)).toEqual([{ kind: "clipboard" }, true, "/repo"]);
  expect(shown()).toEqual([{ kind: "info", title: COPIED, body: undefined }]);
  expect(ui.created).not.toHaveBeenCalled();
});

// `checkIsFileValid`: `Save Patch File` with Overwrite and Cancel.
test("an_existing_file_asks_before_it_is_overwritten", async () => {
  const taken = "/repo/out/m.patch";
  const ui = fakeUi({ file: taken }, false);
  const api = fakeApi({}, [taken]);

  await patchFlow(ui, api).createPatch({ repo: "/repo", entries: [ENTRY] }, "m");

  expect(ui.overwrite.mock.calls).toEqual([["m.patch", "/repo/out"]]);
  expect(api.create).not.toHaveBeenCalled();

  const yes = fakeUi({ file: taken }, true);
  await patchFlow(yes, api).createPatch({ repo: "/repo", entries: [ENTRY] }, "m");
  expect(api.create.mock.calls[0][2]).toEqual({ kind: "file", path: taken });
});

test("the_created_dialog_offers_reveal_and_remembers_dont_ask_again", async () => {
  const api = fakeApi();
  const ui = fakeUi({}, true, { reveal: true, dontAsk: true });

  await patchFlow(ui, api).createPatch({ repo: "/repo", entries: [ENTRY] }, "m");

  expect(ui.created.mock.calls).toEqual([["/repo/m.patch"]]);
  expect(api.reveal.mock.calls).toEqual([["/repo/m.patch"]]);
  expect(api.showInFinder.mock.calls).toEqual([["/repo", true]]);

  // Close with `Don't ask again` stops both the question and the reveal.
  const closed = fakeUi({}, true, { reveal: false, dontAsk: true });
  const other = fakeApi();
  await patchFlow(closed, other).createPatch({ repo: "/repo", entries: [ENTRY] }, "m");
  expect(other.reveal).not.toHaveBeenCalled();
  expect(other.showInFinder.mock.calls).toEqual([["/repo", false]]);

  // Once remembered it is not asked again.
  const always = fakeApi({ showInFinder: true });
  const never = fakeApi({ showInFinder: false });
  const quiet = fakeUi();
  await patchFlow(quiet, always).createPatch({ repo: "/repo", entries: [ENTRY] }, "m");
  await patchFlow(quiet, never).createPatch({ repo: "/repo", entries: [ENTRY] }, "m");
  expect(quiet.created).not.toHaveBeenCalled();
  expect(always.reveal.mock.calls).toEqual([["/repo/m.patch"]]);
  expect(never.reveal).not.toHaveBeenCalled();
});

test("a_failure_writing_the_file_says_error_creating_patch", async () => {
  const api = fakeApi();
  api.create.mockRejectedValue({ kind: "failed", message: '"x.bin" is binary' });

  await patchFlow(fakeUi(), api).createPatch({ repo: "/repo", entries: [ENTRY] }, "m");

  expect(shown()).toEqual([{ kind: "error", title: creationError('"x.bin" is binary'), body: undefined }]);
});

// Review Focus 5: the changes, the base and the settings are those of the repository that asked.
test("a_repository_switch_during_create_patch_writes_for_the_first_one", async () => {
  const api = fakeApi();
  let open = "/one";
  const ui = fakeUi();
  ui.settings.mockImplementation(async (request) => {
    open = "/two";
    return { file: request.file, toClipboard: false, base: request.base, reverse: false };
  });

  await patchFlow(ui, api).createPatch({ repo: "/one", entries: [ENTRY] }, "m");

  expect(open).toBe("/two");
  expect(api.defaults.mock.calls.every(([repo]) => repo === "/one")).toBe(true);
  expect(api.create.mock.calls[0][1].basePath).toBe("/one");
  expect(api.create.mock.calls[0][4]).toBe("/one");
  expect(api.showInFinder).not.toHaveBeenCalledWith("/two", expect.anything());
});

// `CreatePatchConfigurationPanel.validateFields` and `PatchNameChecker.validateName`.
test("the_settings_validation_texts", () => {
  expect(settingsProblem(paths("/repo/x.patch"), "/repo/src")).toBeNull();
  expect(settingsProblem(paths("/"), null)).toEqual({ field: "file", text: NAME_EMPTY });
  // `new File("/repo/").getName()` is `repo`.
  expect(settingsProblem(paths("/repo/"), null)).toBeNull();
  expect(settingsProblem(paths("  "), null)).toEqual({ field: "file", text: NAME_EMPTY });
  expect(settingsProblem(paths(`/${"a".repeat(255)}`), null)).toEqual({
    field: "file",
    text: NAME_TOO_LONG,
  });
  expect(settingsProblem(paths("/repo/x.patch", [], " "), null)).toEqual({
    field: "base",
    text: BASE_EMPTY,
  });
  expect(settingsProblem(paths("/repo/x.patch", [], "/gone"), null)).toEqual({
    field: "base",
    text: BASE_MISSING,
  });
  expect(settingsProblem(paths("/repo/x.patch", [], "/repo/src"), "/repo")).toEqual({
    field: "base",
    text: baseOutside("/repo"),
  });
  expect(baseOutside("/repo")).toBe("Base path doesn't contain all selected changes (use /repo)");
  expect(BASE_MISSING).toBe("Base dir doesn't exist");
  expect(BASE_EMPTY).toBe("Base path cannot be empty!");
  expect(NAME_TOO_LONG).toBe("File path should not be too long.");
  expect(settingsWarning(paths("/repo/x.patch", ["/repo/x.patch"]))).toBe(
    fileExists("/repo/x.patch"),
  );
  expect(fileExists("/repo/x.patch")).toBe("A file with the name '/repo/x.patch' already exists");
});

// P10: the cases the shelf's own cut (`shelf.rs` `shorten_and_sanitize`) is tested with too.
test("the_patch_name_prefix_follows_the_shared_cases", () => {
  const { cases } = JSON.parse(sharedNames) as { cases: { message: string; prefix: string }[] };
  for (const { message, prefix } of cases) expect(patchNamePrefix(message), message).toBe(prefix);
});

// `PathUtilRt.suggestFileName` and `shortenAndSanitize`.
test("the_patch_name_is_the_message_made_safe_and_short", async () => {
  expect(patchNamePrefix(null)).toBe("unnamed");
  expect(patchNamePrefix("  ")).toBe("__");
  expect(patchNamePrefix('a/b\\c:d*e?f"g<h>i|j;k.l m\tn')).toBe("a_b_c_d_e_f_g_h_i_j_k_l_m_n");
  expect(patchNamePrefix("añ😀 b c")).toBe("añ😀 b_c");
  expect(patchNamePrefix("x".repeat(200))).toBe("x".repeat(90));
  const long = await suggestPatchFile("/f/", "y".repeat(95), async () => false);
  expect(long).toBe(`/f/${"y".repeat(90)}.patch`);
  // Up to `z…z999.patch` every name is taken; `z…z1000.patch` would have 100 characters, so the
  // name loses one and the count starts again.
  const full = `/f/${"z".repeat(90)}`;
  const taken = async (path: string) =>
    path.startsWith(full) && Number(path.slice(full.length, -".patch".length) || 0) < 1000;
  expect(await suggestPatchFile("/f", "z".repeat(90), taken)).toBe(`/f/${"z".repeat(89)}.patch`);
});

test("staging_entries_follow_head_to_index_index_to_disk_and_head_to_disk", () => {
  const changes: FileChange[] = [
    { path: "a.txt", oldPath: null, state: "modified", staged: true, unstaged: true },
    { path: "b.txt", oldPath: "old.txt", state: "renamed", staged: true, unstaged: false },
    { path: "c.txt", oldPath: null, state: "modified", staged: false, unstaged: true },
    { path: "u.txt", oldPath: null, state: "untracked", staged: false, unstaged: true },
    { path: "x.txt", oldPath: null, state: "unmerged", staged: false, unstaged: true },
  ];
  const keys = [
    "a.txt\0staged",
    "b.txt\0staged",
    "a.txt\0unstaged",
    "c.txt\0unstaged",
    "u.txt\0untracked",
    "x.txt\0conflicts",
  ];

  const entries = stagingEntries(keys, changes);

  expect(entries).toEqual([
    { path: "a.txt", oldPath: null, before: { kind: "head" }, after: { kind: "disk" } },
    { path: "b.txt", oldPath: "old.txt", before: { kind: "head" }, after: { kind: "index" } },
    { path: "c.txt", oldPath: null, before: { kind: "index" }, after: { kind: "disk" } },
    { path: "u.txt", oldPath: null, before: { kind: "index" }, after: { kind: "disk" } },
  ]);
});

test("local_and_log_entries", () => {
  const selection = [{ hunk: "h2", lines: null }];
  expect(
    localEntries(
      [
        { path: "a.txt", oldPath: null, state: "modified" },
        { path: "n.txt", oldPath: null, state: "untracked" },
        { path: "r.txt", oldPath: "o.txt", state: "renamed" },
      ],
      new Map([["a.txt", selection]]),
    ),
  ).toEqual([
    { path: "a.txt", oldPath: null, before: { kind: "head" }, after: { kind: "selection", selection } },
    { path: "n.txt", oldPath: null, before: null, after: { kind: "disk" } },
    { path: "r.txt", oldPath: "o.txt", before: { kind: "head" }, after: { kind: "disk" } },
  ]);
  expect(
    logEntries([
      { file: { path: "a.txt", oldPath: null, status: "M" }, hash: "h2", base: "h1" },
      { file: { path: "root.txt", oldPath: null, status: "A" }, hash: "h0", base: null },
    ]),
  ).toEqual([
    { path: "a.txt", oldPath: null, before: { kind: "revision", hash: "h1" }, after: { kind: "revision", hash: "h2" } },
    { path: "root.txt", oldPath: null, before: null, after: { kind: "revision", hash: "h0" } },
  ]);
});

test("the_common_parent_is_the_folder_of_a_single_file", () => {
  expect(commonParent("/repo", [ENTRY])).toBe("/repo/src");
  expect(commonParent("/repo", [ENTRY, { ...ENTRY, path: "src/deep/b.txt" }])).toBe("/repo/src");
  expect(commonParent("/repo", [ENTRY, { ...ENTRY, path: "top.txt" }])).toBe("/repo");
  expect(commonParent("/repo", [{ ...ENTRY, path: "b/x.txt", oldPath: "a/x.txt" }])).toBe("/repo");
  expect(commonParent("/repo", [])).toBeNull();
});

// The file is written: a reveal or the `Don't ask again` save that fails afterwards is its own error.
test("a_failed_reveal_is_reported_on_its_own", async () => {
  const api = fakeApi();
  api.reveal.mockRejectedValue({ kind: "failed", message: "open -R failed" });

  const made = await patchFlow(fakeUi({}, true, { reveal: true, dontAsk: false }), api).createPatch(
    { repo: "/repo", entries: [ENTRY] },
    "m",
  );

  expect(made).toBe(true);
  expect(shown()).toEqual([{ kind: "error", title: "open -R failed", body: undefined }]);
});

// Never lose work: a worktree file the patch was written over comes back from its copy.
test("an_overwritten_worktree_file_can_be_restored_from_its_copy", async () => {
  const taken = "/repo/notes.patch";
  const api = fakeApi({ showInFinder: false }, [taken]);
  api.create.mockResolvedValue({ file: taken, snapshot: "s1" });

  await patchFlow(fakeUi({ file: taken }), api).createPatch({ repo: "/repo", entries: [ENTRY] }, "m");

  const notice = get(notifications).find((shown) => shown.title.includes("notes.patch"));
  expect(notice?.title).toBe("notes.patch was overwritten. A snapshot was saved.");
  expect(notice?.actions?.map((action) => action.label)).toEqual(["Restore"]);
  notice?.actions?.[0].run();
  await vi.waitFor(() => expect(api.restore).toHaveBeenCalledWith("s1", "/repo"));
  await vi.waitFor(() => expect(shown().map((notice) => notice.title)).toContain("Snapshot restored."));
});

// Ramify's own: a relative path would be resolved against the app's folder, so it is refused while
// the dialog is open.
test("relative_paths_are_field_errors", () => {
  expect(settingsProblem(paths("out/x.patch", [], "/repo"), null)).toEqual({
    field: "file",
    text: NOT_ABSOLUTE,
  });
  expect(settingsProblem({ ...paths("/repo/x.patch"), base: "repo", baseExists: true }, null)).toEqual({
    field: "base",
    text: NOT_ABSOLUTE,
  });
});

// Task 4's re-review N2: a relative `To file:` left in the disabled field does not stop `To
// clipboard`; the name checks of IntelliJ still apply.
test("to_clipboard_does_not_check_that_the_file_is_absolute", () => {
  expect(settingsProblem(paths("out/x.patch", [], "/repo"), null, true)).toBeNull();
  expect(settingsProblem(paths("/", [], "/repo"), null, true)).toEqual({ field: "file", text: NAME_EMPTY });
  expect(settingsProblem(paths("out/x.patch", [], "/repo"), null, false)).toEqual({
    field: "file",
    text: NOT_ABSOLUTE,
  });
});

// Task 4's re-review N1: as after a Rollback, the restored snapshot can be undone.
test("a_restored_patch_overwrite_offers_undo", async () => {
  const taken = "/repo/notes.patch";
  const api = fakeApi({ showInFinder: false }, [taken]);
  api.create.mockResolvedValue({ file: taken, snapshot: "s1" });
  api.restore.mockResolvedValueOnce("s2").mockResolvedValueOnce("s3");

  await patchFlow(fakeUi({ file: taken }), api).createPatch({ repo: "/repo", entries: [ENTRY] }, "m");
  get(notifications)
    .find((notice) => notice.title.includes("notes.patch"))
    ?.actions?.[0].run();

  const restored = await vi.waitFor(() => {
    const notice = get(notifications).find((shown) => shown.title === RESTORED);
    if (!notice) throw new Error("not yet");
    return notice;
  });
  expect(restored.actions?.map((action) => action.label)).toEqual(["Undo"]);
  restored.actions?.[0].run();
  await vi.waitFor(() => expect(api.restore.mock.calls).toEqual([["s1", "/repo"], ["s2", "/repo"]]));
});

//
// Apply Patch and Apply Patch from Clipboard
//

const ADDED: PlannedFile = planned(0, "new.txt", "added");
const MODIFIED: PlannedFile = planned(1, "src/a.txt", "modified");
const DELETED: PlannedFile = planned(2, "old.txt", "deleted");
const MISSING: PlannedFile = { ...planned(3, "gone.txt", "modified"), status: "missing" };

function planned(index: number, path: string, kind: PlannedFile["kind"]): PlannedFile {
  return {
    index,
    path,
    before: kind === "added" ? null : path,
    after: kind === "deleted" ? null : path,
    original: path,
    strip: 0,
    maxStrip: path.split("/").length - 1,
    kind,
    binary: false,
    base: false,
    status: "applicable",
    error: null,
    hashes: { [path]: kind === "added" ? "absent" : `hash-of-${path}` },
  };
}

const plan = (files: PlannedFile[], extra: Partial<ApplyPlan> = {}): ApplyPlan => ({
  subject: null,
  message: null,
  author: null,
  additionalInfoError: null,
  readError: null,
  files,
  ...extra,
});

const result = (extra: Partial<PatchApplyResult> = {}): PatchApplyResult => ({
  snapshot: "snap",
  status: "success",
  failed: [],
  conflicts: [],
  messages: [],
  ...extra,
});

const LISTS: Changelists = {
  lists: [
    { id: "default", name: "Changes", comment: "" },
    { id: "l2", name: "Feature", comment: "old comment" },
  ],
  active: "default",
  assignments: {},
  hunkAssignments: {},
};

const CONTEXT = { repo: "/repo", head: "h1", lists: LISTS, current: () => "/repo" };

function applyApi(
  shownPlan: ApplyPlan = plan([ADDED, MODIFIED, DELETED, MISSING]),
): ApplyApi & { [K in keyof ApplyApi]: ReturnType<typeof vi.fn> } {
  return {
    plan: vi.fn(async () => shownPlan),
    apply: vi.fn(async () => result()),
    resolve: vi.fn(async () => ({ hash: "h", messages: [] })),
    rollback: vi.fn(async () => "before-rollback"),
    preview: vi.fn(async () => ({ before: "", after: "", titles: ["Base version", "Patched version"] })),
    clipboard: vi.fn(async () => ({ length: 0, hash: null, patch: null })),
    appSettings: vi.fn(async () => ({ folder: "/Users/me/Downloads/PATCHS", detectPatchOnTheFly: false })),
    rememberFolder: vi.fn(async () => {}),
    setOnTheFly: vi.fn(async () => {}),
    chooseFile: vi.fn(async () => "/Users/me/Downloads/PATCHS/fix_it.patch"),
    chooseFolder: vi.fn(async () => null),
    merge: vi.fn(async (contents: MergeContents) => ({ id: contents.id, result: "resolved", content: "merged\n" })),
    useList: vi.fn(async () => "l2"),
    moveToList: vi.fn(async () => {}),
  } as never;
}

// The dialog answers with every valid file included, the list it was given and no mapping.
function applyUi(
  answer: (request: ApplyDialogRequest) => Partial<ApplyAnswer> | null = () => ({}),
): ApplyUi & { [K in keyof ApplyUi]: ReturnType<typeof vi.fn> } {
  return {
    dialog: vi.fn(async (request: ApplyDialogRequest) => {
      const chosen = answer(request);
      if (chosen === null) return null;
      return {
        source: request.source,
        mapping: { files: [] },
        plan: request.plan,
        selected: request.plan.files.filter((file) => file.status !== "missing").map((file) => file.index),
        list: null,
        ...chosen,
      };
    }),
    overwrite: vi.fn(async (paths: string[]) => paths),
    errors: vi.fn(async () => {}),
    partlyFailed: vi.fn(async () => false),
  } as never;
}

// `ApplyPatchAction.actionPerformed`: `Select Patch File` in `PATCH_STORAGE_LOCATION`, whose folder
// is saved, then the dialog over the chosen file. Its tree is ApplyPatchDialog.test.ts's.
test("apply_patch_asks_select_patch_file_then_shows_the_tree_with_added_modified_deleted_and_missing_base", async () => {
  const api = applyApi();
  const ui = applyUi(() => null);

  await applyFlow(ui, api).applyPatch(CONTEXT);

  expect(api.chooseFile.mock.calls).toEqual([
    [{ title: SELECT_PATCH_TITLE, defaultPath: "/Users/me/Downloads/PATCHS" }],
  ]);
  expect(api.rememberFolder.mock.calls).toEqual([["/Users/me/Downloads/PATCHS"]]);
  const source = { kind: "file", path: "/Users/me/Downloads/PATCHS/fix_it.patch" };
  expect(api.plan.mock.calls).toEqual([[source, { files: [] }]]);
  const request = ui.dialog.mock.calls[0][0];
  expect(request.mode).toBe("apply");
  expect(request.source).toEqual(source);
  expect(request.plan.files.map((file: PlannedFile) => [file.kind, file.status])).toEqual([
    ["added", "applicable"],
    ["modified", "applicable"],
    ["deleted", "applicable"],
    ["modified", "missing"],
  ]);
  expect(request.lists).toBe(LISTS);
  expect(api.apply).not.toHaveBeenCalled();

  // Cancelling `Select Patch File` does nothing more; without a remembered folder it starts in the
  // repository.
  const none = applyApi();
  none.chooseFile.mockResolvedValue(null);
  none.appSettings.mockResolvedValue({ folder: null, detectPatchOnTheFly: false });
  await applyFlow(ui, none).applyPatch(CONTEXT);
  expect(none.chooseFile.mock.calls[0][0].defaultPath).toBe("/repo");
  expect(none.plan).not.toHaveBeenCalled();
});

// OK applies what is included, with the plan's hashes and the HEAD the dialog was opened on, then
// the applied files go to the list the chooser named.
test("ok_applies_the_included_files_into_the_chosen_changelist", async () => {
  const api = applyApi();
  const list = { name: "Feature", comment: "Fix it\n\nBody", setActive: false };
  const ui = applyUi(() => ({ selected: [1, 2], list }));

  await applyFlow(ui, api).applyPatch(CONTEXT);

  expect(api.useList.mock.calls).toEqual([[list, "/repo"]]);
  expect(api.apply.mock.calls).toEqual([
    [
      { kind: "file", path: "/Users/me/Downloads/PATCHS/fix_it.patch" },
      { files: [] },
      {
        selected: ["src/a.txt", "old.txt"],
        overwrite: [],
        expected: { "src/a.txt": "hash-of-src/a.txt", "old.txt": "hash-of-old.txt" },
      },
      "h1",
    ],
  ]);
  expect(api.moveToList.mock.calls).toEqual([[["src/a.txt", "old.txt"], "l2"]]);
  expect(shown()).toEqual([{ kind: "info", title: SUCCESS_TEXT, body: undefined }]);
});

// `PathsVerifier.DelayedPrecheckContext.doDelayed`: the new files that exist are asked in one
// `Overwrite Existing Files`; the ones left out are skipped.
test("overwrite_existing_files_is_asked_for_new_files_that_exist", async () => {
  const exists = (index: number, path: string): PlannedFile => ({ ...planned(index, path, "added"), status: "exists" });
  const api = applyApi(plan([exists(0, "a.txt"), exists(1, "b.txt"), { ...MODIFIED, index: 2 }]));
  const ui = applyUi();
  ui.overwrite.mockResolvedValue(["b.txt"]);

  await applyFlow(ui, api).applyPatch(CONTEXT);

  expect(ui.overwrite.mock.calls).toEqual([[["a.txt", "b.txt"], "/repo"]]);
  expect(api.apply.mock.calls[0][2].overwrite).toEqual(["b.txt"]);
  expect(api.apply.mock.calls[0][2].selected).toEqual(["a.txt", "b.txt", "src/a.txt"]);

  // With none of them, nothing is asked.
  const quiet = applyUi();
  await applyFlow(quiet, applyApi()).applyPatch(CONTEXT);
  expect(quiet.overwrite).not.toHaveBeenCalled();
});

// Finding 6: the binary files chosen with the Unshelve dialog's `Overwrite` go to the apply as
// files to write over, beside the new files `Overwrite Existing Files` accepted.
test("the_binaries_chosen_to_overwrite_reach_the_apply", async () => {
  const binary: PlannedFile = { ...planned(0, "x.bin", "modified"), binary: true, overwritable: true };
  const api = applyApi(plan([binary, { ...MODIFIED, index: 1 }]));
  const ui = applyUi(() => ({ overwrite: ["x.bin", "not-chosen.bin"] }));

  await applyFlow(ui, api).unshelveWithDialog(CONTEXT, {
    name: "Fix_it",
    preselected: null,
    listNames: [],
    removeApplied: false,
  });

  expect(ui.overwrite).not.toHaveBeenCalled();
  expect(api.apply.mock.calls[0][2].overwrite).toEqual(["x.bin"]);
  expect(api.apply.mock.calls[0][2].selected).toEqual(["x.bin", "src/a.txt"]);
});

// `ApplyPatchAction.applyContent`: the merge window titled `Patch Conflict for <name> (<folder>)`
// with Local version, Merge result and Patched version, whose Cancel is `Abort…`.
test("a_conflict_opens_the_merge_window_and_abort_asks_abort_and_rollback_skip_or_continue", async () => {
  const conflict: PatchConflict = {
    path: "src/a.txt",
    base: "one\n",
    local: "local\n",
    patched: "patched\n",
    localHash: "lh",
    movedTo: null,
    newMode: null,
  };
  const api = applyApi();
  api.apply.mockResolvedValue(result({ status: "skip", conflicts: [conflict] }));

  await applyFlow(applyUi(), api).applyPatch(CONTEXT);

  const contents = api.merge.mock.calls[0][0];
  expect(contents).toEqual({
    id: expect.any(String),
    title: "Patch Conflict for a.txt (/repo/src)",
    titles: ["Local version", "Merge result", "Patched version"],
    left: "local\n",
    base: "one\n",
    right: "patched\n",
    path: "src/a.txt",
    abort: true,
  });
  expect(api.resolve.mock.calls).toEqual([[conflict, "merged\n"]]);
  expect(shown()).toEqual([{ kind: "info", title: SUCCESS_TEXT, body: undefined }]);

  // `Skip` leaves the file as it was; nothing else was applied, so nothing is said.
  dismissAll();
  const skipped = applyApi();
  skipped.apply.mockResolvedValue(result({ status: "skip", conflicts: [conflict] }));
  skipped.merge.mockImplementation(async (given: MergeContents) => ({ id: given.id, result: "skip" }));
  await applyFlow(applyUi(), skipped).applyPatch(CONTEXT);
  expect(skipped.resolve).not.toHaveBeenCalled();
  expect(skipped.rollback).not.toHaveBeenCalled();
  expect(shown()).toEqual([]);

  // `Abort and Rollback` stops at that file and rolls back to the copy taken before applying.
  const aborted = applyApi();
  aborted.apply.mockResolvedValue(result({ status: "success", conflicts: [conflict, { ...conflict, path: "b.txt" }] }));
  aborted.merge.mockImplementation(async (given: MergeContents) => ({ id: given.id, result: "abort" }));
  await applyFlow(applyUi(), aborted).applyPatch(CONTEXT);
  expect(aborted.merge).toHaveBeenCalledTimes(1);
  expect(aborted.rollback.mock.calls).toEqual([["snap"]]);
  expect(shown()).toEqual([{ kind: "warning", title: ABORTED_TITLE, body: ABORTED_TEXT }]);

  // The window gone without an answer (the app was quitting) is a failed file: `Patch Applying
  // Partly Failed` offers the rollback.
  dismissAll();
  const gone = applyApi();
  gone.apply.mockResolvedValue(result({ status: "skip", conflicts: [conflict] }));
  gone.merge.mockResolvedValue(null);
  const ui = applyUi();
  ui.partlyFailed.mockResolvedValue(true);
  await applyFlow(ui, gone).applyPatch(CONTEXT);
  expect(gone.resolve).not.toHaveBeenCalled();
  expect(ui.partlyFailed.mock.calls).toEqual([[["src/a.txt"], false]]);
  expect(gone.rollback.mock.calls).toEqual([["snap"]]);

  // Another repository opened while the merge window was open: nothing is resolved, asked or rolled
  // back in the repository open now, and it says so.
  dismissAll();
  const switched = applyApi();
  let open = "/repo";
  switched.apply.mockResolvedValue(result({ status: "skip", conflicts: [conflict] }));
  switched.merge.mockImplementation(async () => {
    open = "/other";
    return null;
  });
  const asked = applyUi();
  await applyFlow(asked, switched).applyPatch({ ...CONTEXT, current: () => open });
  expect(switched.merge).toHaveBeenCalledTimes(1);
  expect(switched.resolve).not.toHaveBeenCalled();
  expect(asked.partlyFailed).not.toHaveBeenCalled();
  expect(switched.rollback).not.toHaveBeenCalled();
  expect(shown()).toEqual([{ kind: "error", title: APPLY_TITLE, body: SWITCHED_WHILE_APPLYING }]);
});

// `askToRollback`: FAILURE shows `Patch Applying Partly Failed` with the failed files; `Rollback`
// goes back to the copy and says `Apply patch aborted`, a failed rollback `Rollback failed`.
test("failures_show_patch_applying_partly_failed_and_rollback_restores", async () => {
  const binary = { ...planned(4, "image.png", "modified"), binary: true };
  const api = applyApi(plan([MODIFIED, binary]));
  api.apply.mockResolvedValue(
    result({ status: "failure", failed: ["src/a.txt"], messages: ["Cannot find file to patch: x"] }),
  );
  const ui = applyUi();
  ui.partlyFailed.mockResolvedValue(true);

  await applyFlow(ui, api).applyPatch(CONTEXT);

  expect(ui.errors.mock.calls).toEqual([[["Cannot find file to patch: x"]]]);
  expect(ui.partlyFailed.mock.calls).toEqual([[["src/a.txt"], true]]);
  expect(api.rollback.mock.calls).toEqual([["snap"]]);
  expect(shown()).toEqual([{ kind: "warning", title: ABORTED_TITLE, body: ABORTED_TEXT }]);
  expect(api.moveToList).not.toHaveBeenCalled();

  dismissAll();
  const failing = applyApi(plan([MODIFIED]));
  failing.apply.mockResolvedValue(result({ status: "failure", failed: ["src/a.txt"] }));
  failing.rollback.mockRejectedValue({ kind: "failed", message: "no" });
  const again = applyUi();
  again.partlyFailed.mockResolvedValue(true);
  await applyFlow(again, failing).applyPatch(CONTEXT);
  expect(again.partlyFailed.mock.calls).toEqual([[["src/a.txt"], false]]);
  expect(shown()).toEqual([{ kind: "warning", title: ROLLBACK_FAILED_TITLE, body: ROLLBACK_FAILED_TEXT }]);
  // V:363, then where Ramify keeps the copy, which it has instead of Local History.
  expect(ROLLBACK_FAILED_TEXT).toBe(
    "Try using 'Local History' dialog to perform revert manually.\n" +
      "The copy taken before applying is in Restore Snapshot… of Local Changes.",
  );

  // Cancel keeps what was applied, and says nothing more for FAILURE.
  dismissAll();
  const kept = applyApi(plan([MODIFIED]));
  kept.apply.mockResolvedValue(result({ status: "failure", failed: ["src/a.txt"] }));
  await applyFlow(applyUi(), kept).applyPatch(CONTEXT);
  expect(kept.rollback).not.toHaveBeenCalled();
  expect(shown()).toEqual([]);
});

// `PatchApplier.showApplyStatus`.
test("the_notifications_for_already_applied_partial_and_success", async () => {
  for (const [status, expected] of [
    ["alreadyApplied", { kind: "info", title: NOTIFICATION_TITLE, body: ALREADY_APPLIED_TEXT }],
    ["partial", { kind: "info", title: NOTIFICATION_TITLE, body: PARTIALLY_APPLIED_TEXT }],
    ["success", { kind: "info", title: SUCCESS_TEXT, body: undefined }],
  ] as const) {
    dismissAll();
    const api = applyApi(plan([MODIFIED]));
    api.apply.mockResolvedValue(result({ status }));
    await applyFlow(applyUi(), api).applyPatch(CONTEXT);
    expect(shown()).toEqual([expected]);
  }
  expect(andStatus("success", "alreadyApplied")).toBe("partial");
  expect(andStatus("skip", "success")).toBe("success");
  expect(andStatus("failure", "abort")).toBe("abort");
  expect(andStatus(null, "skip")).toBe("skip");
});

// Review Focus 3. Ramify cannot disable the menu item while the clipboard has no text, as IntelliJ
// does: it says `Cannot Apply Patch Now`. Text that is not a patch opens the dialog with no
// changes (`No changes detected`) without reading it as a patch.
test("from_clipboard_without_a_patch_says_cannot_apply_patch_now", async () => {
  const api = applyApi();
  const ui = applyUi(() => null);

  await applyFlow(ui, api).applyFromClipboard(CONTEXT);

  expect(api.clipboard.mock.calls).toEqual([[null]]);
  expect(shown()).toEqual([{ kind: "error", title: CANNOT_APPLY_NOW, body: undefined }]);
  expect(ui.dialog).not.toHaveBeenCalled();

  dismissAll();
  api.clipboard.mockResolvedValue({ length: 20 * 1024 * 1024, hash: null, patch: null });
  await applyFlow(ui, api).applyFromClipboard(CONTEXT);
  expect(api.plan).not.toHaveBeenCalled();
  const request = ui.dialog.mock.calls[0][0];
  expect(request.mode).toBe("clipboard");
  expect(request.plan.files).toEqual([]);
  expect(request.source).toEqual({ kind: "text", text: "" });

  const text = "--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-a\n+b\n";
  api.clipboard.mockResolvedValue({ length: text.length, hash: null, patch: text });
  await applyFlow(ui, api).applyFromClipboard(CONTEXT);
  expect(api.plan.mock.calls).toEqual([[{ kind: "text", text }, { files: [] }]]);
  expect(ui.dialog.mock.calls[1][0].onTheFly).toBe(false);
});

// `PatchClipboardListener` and `ClipboardAnalyzeListener`: with the setting on, gaining the focus
// offers a patch on the clipboard once; a text seen before, or seen when the focus left, is not
// offered; 100 KB or more is not looked at.
test("on_the_fly_offers_the_dialog_once_per_clipboard_text", async () => {
  const api = applyApi();
  const offered = vi.fn();
  const watcher = clipboardWatcher(api, offered);
  const text = "--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-a\n+b\n";
  api.clipboard.mockResolvedValue({ length: text.length, hash: "t1", patch: text });

  await watcher.activated();
  expect(api.clipboard).not.toHaveBeenCalled();

  api.appSettings.mockResolvedValue({ folder: null, detectPatchOnTheFly: true });
  await watcher.activated();
  await watcher.activated();
  expect(api.clipboard.mock.calls).toEqual([[ON_THE_FLY_MAX], [ON_THE_FLY_MAX]]);
  expect(offered.mock.calls).toEqual([[text]]);

  // Copied while the app was away, and seen as it left: not offered when it comes back.
  api.clipboard.mockResolvedValue({ length: 4, hash: "t2", patch: "x" });
  await watcher.deactivated();
  await watcher.activated();
  expect(offered).toHaveBeenCalledTimes(1);

  // A new text that is not a patch is remembered, a patch after it is offered.
  api.clipboard.mockResolvedValue({ length: 5, hash: "t3", patch: null });
  await watcher.activated();
  api.clipboard.mockResolvedValue({ length: text.length, hash: "t4", patch: text });
  await watcher.activated();
  expect(offered).toHaveBeenCalledTimes(2);
});

// Review Focus 5: another repository opened while the dialog was open refuses the apply before
// anything is written; the HEAD the dialog saw goes with it (`expected_head`).
test("a_repository_switch_during_apply_is_refused_before_writing", async () => {
  const api = applyApi(plan([MODIFIED]));
  let open = "/repo";
  const ui = applyUi(() => {
    open = "/other";
    return {};
  });

  await applyFlow(ui, api).applyPatch({ ...CONTEXT, current: () => open });

  expect(api.apply).not.toHaveBeenCalled();
  expect(api.useList).not.toHaveBeenCalled();
  expect(shown()).toEqual([{ kind: "error", title: APPLY_TITLE, body: REPOSITORY_SWITCHED }]);

  // HEAD moved in the same repository: the backend refuses, and that is all that is said.
  dismissAll();
  const moved = applyApi(plan([MODIFIED]));
  moved.apply.mockRejectedValue({ kind: "failed", message: "Repository has already been changed" });
  await applyFlow(applyUi(), moved).applyPatch(CONTEXT);
  expect(moved.apply.mock.calls[0][3]).toBe("h1");
  expect(shown()).toEqual([
    { kind: "error", title: APPLY_TITLE, body: "Repository has already been changed" },
  ]);
});

// `PartialChangesUtil.computeUnderChangeListSync`: what was really written goes to the chosen list
// whatever the status; a skipped conflict, a declined overwrite, an already applied or failed file
// stays where it was, and after a rollback nothing moves.
test("only_the_files_really_written_go_to_the_list", async () => {
  const conflict = (path: string): PatchConflict => ({
    path,
    base: "b\n",
    local: "l\n",
    patched: "p\n",
    localHash: "lh",
    movedTo: null,
    newMode: null,
  });
  const files = [
    planned(0, "written.txt", "modified"),
    { ...planned(1, "taken.txt", "added"), status: "exists" as const },
    { ...planned(2, "kept.txt", "added"), status: "exists" as const },
    { ...planned(3, "same.txt", "modified"), status: "alreadyApplied" as const },
    planned(4, "broken.txt", "modified"),
    planned(5, "resolved.txt", "modified"),
    planned(6, "skipped.txt", "modified"),
  ];
  const api = applyApi(plan(files));
  api.apply.mockResolvedValue(
    result({
      status: "failure",
      failed: ["broken.txt"],
      conflicts: [conflict("resolved.txt"), conflict("skipped.txt")],
    }),
  );
  api.merge.mockImplementation(async (given: MergeContents) =>
    given.path === "skipped.txt" ? { id: given.id, result: "skip" } : { id: given.id, result: "resolved", content: "m\n" },
  );
  const ui = applyUi(() => ({ list: { name: "Feature", comment: "", setActive: false } }));
  ui.overwrite.mockResolvedValue(["taken.txt"]);

  await applyFlow(ui, api).applyPatch(CONTEXT);

  expect(ui.partlyFailed).toHaveBeenCalled();
  expect(api.moveToList.mock.calls).toEqual([[["written.txt", "taken.txt", "resolved.txt"], "l2"]]);

  const rolled = applyApi(plan([files[0], files[4]]));
  rolled.apply.mockResolvedValue(result({ status: "failure", failed: ["broken.txt"] }));
  const again = applyUi(() => ({ list: { name: "Feature", comment: "", setActive: false } }));
  again.partlyFailed.mockResolvedValue(true);
  await applyFlow(again, rolled).applyPatch(CONTEXT);
  expect(rolled.moveToList).not.toHaveBeenCalled();
});

// `ApplyPatchAction.applyContent` shows the merge modally: while a conflict waits in the merge
// window, another Apply Patch does nothing and the other patch dialogs are refused instead of
// cancelling the one the apply will ask next.
test("while_an_apply_waits_on_a_conflict_other_patch_dialogs_and_applies_are_refused", async () => {
  let answer: (done: ContentsMergeDone | null) => void = () => {};
  const api = applyApi();
  api.apply.mockResolvedValue(
    result({
      status: "skip",
      conflicts: [{ path: "a.txt", base: "b\n", local: "l\n", patched: "p\n", localHash: "h", movedTo: null, newMode: null }],
    }),
  );
  api.merge.mockImplementation(() => new Promise((resolve) => (answer = resolve)));
  const running = applyFlow(applyUi(), api).applyPatch(CONTEXT);
  await vi.waitFor(() => expect(api.merge).toHaveBeenCalled());

  expect(applyInProgress()).toBe(true);
  const other = applyApi();
  await applyFlow(applyUi(), other).applyPatch(CONTEXT);
  await applyFlow(applyUi(), other).applyFromClipboard(CONTEXT);
  expect(other.chooseFile).not.toHaveBeenCalled();
  expect(other.clipboard).not.toHaveBeenCalled();
  const settings = patchDialogsUi.settings({
    file: "/repo/x.patch",
    toClipboard: false,
    base: "/repo",
    commonParent: null,
    check: async () => ({ file: "", fileExists: false, base: "", baseExists: true }),
  });
  expect(await settings).toBeNull();

  answer({ id: "x", result: "skip" });
  await running;
  expect(applyInProgress()).toBe(false);
  answerPatchDialog(null);
});

// Task 3's carried item: a repository switch destroys the merge window without `merge-done`. While
// the window is open its session is asked every second; once it no longer holds these contents the
// answer is waited for 500 ms more, then the merge is over without one.
test("the_merge_window_gone_without_an_answer_ends_the_wait", async () => {
  vi.useFakeTimers();
  try {
    let session: { id: string } | null = null;
    const handlers: ((event: { payload: unknown }) => void)[] = [];
    vi.mocked(listen).mockImplementation(async (_name, handler) => {
      handlers.push(handler as never);
      return () => {};
    });
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "merge_window_open_contents") session = { id: (args as { input: { id: string } }).input.id };
      if (command === "merge_window_contents") return session;
      return undefined;
    });
    const contents = { id: "m1", title: "t", titles: ["a", "b", "c"] as [string, string, string], left: "", base: "", right: "", path: "a.txt" };

    let done: unknown = "pending";
    void defaultApplyApi.merge(contents).then((value) => (done = value));
    await vi.advanceTimersByTimeAsync(2500);
    expect(done).toBe("pending");
    session = null;
    // The check at 3 s finds it gone; the grace ends at 3.5 s.
    await vi.advanceTimersByTimeAsync(700);
    expect(done).toBe("pending");
    await vi.advanceTimersByTimeAsync(400);
    expect(done).toBeNull();

    // A `merge-done` that arrives inside the 500 ms still wins.
    let late: unknown = "pending";
    void defaultApplyApi.merge({ ...contents, id: "m2" }).then((value) => (late = value));
    await vi.advanceTimersByTimeAsync(0);
    session = null;
    await vi.advanceTimersByTimeAsync(1100);
    for (const handler of handlers) handler({ payload: { id: "m2", result: "resolved", content: "x\n" } });
    await vi.advanceTimersByTimeAsync(0);
    expect(late).toEqual({ id: "m2", result: "resolved", content: "x\n" });
  } finally {
    vi.useRealTimers();
    vi.mocked(listen).mockReset();
    vi.mocked(invoke).mockReset();
  }
});
