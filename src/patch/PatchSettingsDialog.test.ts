import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { save } from "@tauri-apps/plugin-dialog";
import type { PatchPaths } from "../lib/api";
import { answerPatchDialog, patchDialogsUi, patchFlow, type PatchApi } from "./flow";
import PatchDialogs from "./PatchDialogs.svelte";
import PatchSettingsDialog from "./PatchSettingsDialog.svelte";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));

const EXISTING = ["/repo", "/repo/src", "/repo/taken.patch"];

// The fields with `~` expanded, as `patch_paths` gives them.
const check = vi.fn(async (file: string, base: string): Promise<PatchPaths> => {
  const expand = (path: string) => path.trim().replace(/^~(?=\/|$)/, "/Users/me");
  return {
    file: expand(file),
    fileExists: EXISTING.includes(expand(file)),
    base: expand(base),
    baseExists: EXISTING.includes(expand(base)),
  };
});

function open(props: Record<string, unknown> = {}) {
  const onanswer = vi.fn();
  render(PatchSettingsDialog, {
    file: "/repo/fix.patch",
    toClipboard: false,
    base: "/repo",
    commonParent: "/repo/src",
    check,
    onanswer,
    ...props,
  });
  return onanswer;
}

const dialog = () => screen.getByRole("dialog", { name: "Patch File Settings" });
const ok = () => within(dialog()).getByRole("button", { name: "OK" }) as HTMLButtonElement;
const fileField = () => screen.getByRole("textbox", { name: "To file:" }) as HTMLInputElement;
const baseField = () => screen.getByLabelText("Base path:") as HTMLInputElement;

beforeEach(() => {
  check.mockClear();
});
afterEach(() => {
  cleanup();
  answerPatchDialog(null);
});

// `CreatePatchConfigurationPanel.createPanel`, rows in order.
test("the_settings_show_the_rows_of_the_panel", async () => {
  open();

  expect(within(dialog()).getByRole("heading", { name: "Patch File Settings" })).toBeTruthy();
  const radios = screen.getAllByRole("radio") as HTMLInputElement[];
  expect(radios.map((radio) => radio.parentElement?.textContent?.trim())).toEqual([
    "To file:",
    "To clipboard",
  ]);
  expect(radios.map((radio) => radio.checked)).toEqual([true, false]);
  expect(fileField().value).toBe("/repo/fix.patch");
  expect(baseField().value).toBe("/repo");
  expect((screen.getByRole("checkbox", { name: "Reverse patch" }) as HTMLInputElement).checked).toBe(false);
  const encoding = screen.getByLabelText("Encoding:") as HTMLSelectElement;
  expect(encoding.disabled).toBe(true);
  expect(encoding.value).toBe("UTF-8");
  await waitFor(() => expect(check).toHaveBeenCalledWith("/repo/fix.patch", "/repo"));
  expect(ok().disabled).toBe(false);
  expect(screen.queryByRole("button", { name: "Help" })).toBeNull();
});

test("accepting_gives_the_expanded_fields_reverse_and_the_target", async () => {
  const onanswer = open();

  await fireEvent.input(fileField(), { target: { value: " ~/out/a.patch " } });
  await fireEvent.click(screen.getByRole("checkbox", { name: "Reverse patch" }));
  await fireEvent.click(ok());

  await waitFor(() =>
    expect(onanswer).toHaveBeenCalledWith({
      file: "/Users/me/out/a.patch",
      toClipboard: false,
      base: "/repo",
      reverse: true,
    }),
  );
});

test("to_clipboard_starts_as_it_was_left_and_disables_the_file", async () => {
  const onanswer = open({ toClipboard: true });

  const radios = screen.getAllByRole("radio") as HTMLInputElement[];
  expect(radios.map((radio) => radio.checked)).toEqual([false, true]);
  expect(fileField().disabled).toBe(true);
  await fireEvent.click(radios[0]);
  expect(fileField().disabled).toBe(false);
  await fireEvent.click(radios[1]);
  await fireEvent.click(ok());

  await waitFor(() => expect(onanswer.mock.calls[0][0].toClipboard).toBe(true));
});

// V:439-442 and IdeBundle `error.name.cannot.be.empty`: OK stays off until the field is right.
test("the_settings_validation_texts", async () => {
  open();

  const cases: [() => HTMLInputElement, string, string][] = [
    [fileField, "/repo/", ""],
    [fileField, "/", "Name cannot be empty"],
    [fileField, `/${"a".repeat(255)}`, "File path should not be too long."],
    [baseField, "", "Base path cannot be empty!"],
    [baseField, "/gone", "Base dir doesn't exist"],
    [baseField, "/repo/src", ""],
    [fileField, "out/x.patch", "The path must be absolute"],
    [baseField, "repo", "The path must be absolute"],
  ];
  for (const [field, value, text] of cases) {
    await fireEvent.input(fileField(), { target: { value: "/repo/fix.patch" } });
    await fireEvent.input(baseField(), { target: { value: "/repo" } });
    await fireEvent.input(field(), { target: { value } });
    if (text) {
      await waitFor(() => expect(screen.getByRole("alert").textContent).toBe(text));
      expect(ok().disabled, value).toBe(true);
    } else {
      await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
      expect(ok().disabled, value).toBe(false);
    }
  }

  // `/repo` and `/repo/src` hold the changes; a sibling folder does not.
  EXISTING.push("/repo/other");
  await fireEvent.input(baseField(), { target: { value: "/repo/other" } });
  await waitFor(() =>
    expect(screen.getByRole("alert").textContent).toBe(
      "Base path doesn't contain all selected changes (use /repo/src)",
    ),
  );
  expect(ok().disabled).toBe(true);
  EXISTING.pop();
});

test("an_existing_file_shows_the_red_warning_but_can_be_accepted", async () => {
  const onanswer = open();

  await fireEvent.input(fileField(), { target: { value: "/repo/taken.patch" } });

  await waitFor(() =>
    expect(screen.getByRole("status").textContent).toBe(
      "A file with the name '/repo/taken.patch' already exists",
    ),
  );
  expect(ok().disabled).toBe(false);
  await fireEvent.click(ok());
  await waitFor(() => expect(onanswer).toHaveBeenCalledTimes(1));
});

test("browsing_for_the_file_uses_save_patch_to", async () => {
  vi.mocked(save).mockResolvedValue("/repo/chosen.patch");
  open();

  await fireEvent.click(screen.getByRole("button", { name: "Save Patch to" }));

  await waitFor(() => expect(fileField().value).toBe("/repo/chosen.patch"));
  expect(vi.mocked(save).mock.calls[0][0]).toEqual({
    title: "Save Patch to",
    defaultPath: "/repo/fix.patch",
  });
});

test("cancel_and_escape_answer_nothing", async () => {
  const onanswer = open();

  await fireEvent.click(within(dialog()).getByRole("button", { name: "Cancel" }));
  await fireEvent.keyDown(dialog(), { key: "Escape" });

  expect(onanswer.mock.calls).toEqual([[null], [null]]);
});

// The dialogs of the application's flow, through the host: Patch File Settings, then `Save Patch
// File` (Overwrite, Cancel) and `Create Patch` (Close, Reveal in Finder, Don't ask again).
test("the_overwrite_question_and_the_created_dialog_of_the_application", async () => {
  render(PatchDialogs);
  const api: PatchApi = {
    create: vi.fn(async (_entries, _options, to) => ({
      file: to.kind === "file" ? to.path : null,
      snapshot: null,
    })),
    createFromShelf: vi.fn(),
    defaults: vi.fn(async () => ({ folder: null, toClipboard: false, showInFinder: null })),
    paths: check,
    showInFinder: vi.fn(async () => {}),
    reveal: vi.fn(async () => {}),
    restore: vi.fn(async () => ""),
  };
  const entry = { path: "src/a.txt", oldPath: null, before: { kind: "head" as const }, after: { kind: "disk" as const } };

  const running = patchFlow(patchDialogsUi, api).createPatch({ repo: "/repo", entries: [entry] }, "taken");

  await screen.findByRole("dialog", { name: "Patch File Settings" });
  // `taken.patch` exists, so the suggestion is the next free name.
  expect(fileField().value).toBe("/repo/taken1.patch");
  await fireEvent.input(fileField(), { target: { value: "/repo/taken.patch" } });
  await waitFor(() => expect(screen.getByRole("status")).toBeTruthy());
  await fireEvent.click(ok());

  const question = await screen.findByRole("dialog", { name: "Save Patch File" });
  expect(question.textContent).toContain("File taken.patch (/repo) already exists.");
  expect(question.textContent).toContain("Do you want to overwrite it?");
  await fireEvent.click(within(question).getByRole("button", { name: "Overwrite" }));

  const created = await screen.findByRole("dialog", { name: "Create Patch" });
  expect(created.textContent).toContain("Successfully created patch /repo/taken.patch");
  expect(within(created).getAllByRole("button").map((button) => button.textContent?.trim())).toEqual([
    "Close",
    "Reveal in Finder",
  ]);
  await fireEvent.click(within(created).getByRole("checkbox", { name: "Don't ask again" }));
  await fireEvent.click(within(created).getByRole("button", { name: "Reveal in Finder" }));

  expect(await running).toBe(true);
  expect(api.create).toHaveBeenCalledTimes(1);
  expect(api.reveal).toHaveBeenCalledWith("/repo/taken.patch");
  expect(api.showInFinder).toHaveBeenCalledWith("/repo", true);
});

test("cancelling_the_overwrite_writes_nothing", async () => {
  render(PatchDialogs);
  const create = vi.fn();
  const api: PatchApi = {
    create,
    createFromShelf: vi.fn(),
    defaults: async () => ({ folder: null, toClipboard: false, showInFinder: null }),
    paths: check,
    showInFinder: async () => {},
    reveal: async () => {},
    restore: async () => "",
  };
  const entry = { path: "src/a.txt", oldPath: null, before: { kind: "head" as const }, after: { kind: "disk" as const } };

  const running = patchFlow(patchDialogsUi, api).createPatch({ repo: "/repo", entries: [entry] }, "x");
  await screen.findByRole("dialog", { name: "Patch File Settings" });
  await fireEvent.input(fileField(), { target: { value: "/repo/taken.patch" } });
  await waitFor(() => expect(screen.getByRole("status")).toBeTruthy());
  await fireEvent.click(ok());
  const question = await screen.findByRole("dialog", { name: "Save Patch File" });
  await fireEvent.click(within(question).getByRole("button", { name: "Cancel" }));

  expect(await running).toBe(false);
  expect(create).not.toHaveBeenCalled();
});

// Task 4's re-review N2: a relative path left in the disabled `To file:` does not block `To
// clipboard`, which writes no file.
test("a_relative_file_does_not_block_to_clipboard", async () => {
  const onanswer = open({ file: "out/x.patch", toClipboard: true });

  await waitFor(() => expect(check).toHaveBeenCalled());
  await waitFor(() => expect(ok().disabled).toBe(false));
  expect(screen.queryByRole("alert")).toBeNull();
  await fireEvent.click(ok());
  await waitFor(() => expect(onanswer).toHaveBeenCalledWith(expect.objectContaining({ toClipboard: true })));
});
