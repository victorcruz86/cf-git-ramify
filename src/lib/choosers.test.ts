import { afterEach, expect, test, vi } from "vitest";
import { open, save } from "@tauri-apps/plugin-dialog";
import { chooseFile, chooseFolder, chooseSaveFile } from "./choosers";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));

afterEach(() => {
  vi.mocked(open).mockReset();
  vi.mocked(save).mockReset();
});

test("the_chooser_wrappers_return_null_on_cancel", async () => {
  vi.mocked(open).mockResolvedValue(null);
  vi.mocked(save).mockResolvedValue(null);

  expect(await chooseFile()).toBeNull();
  expect(await chooseFolder()).toBeNull();
  expect(await chooseSaveFile()).toBeNull();
});

test("choose_file_opens_one_file_and_returns_its_path", async () => {
  vi.mocked(open).mockResolvedValue("/Users/me/a.patch");
  const filters = [{ name: "Patch", extensions: ["patch", "diff"] }];

  const chosen = await chooseFile({ title: "Select Patch File", defaultPath: "/repo", filters });

  expect(chosen).toBe("/Users/me/a.patch");
  expect(open).toHaveBeenCalledWith({
    title: "Select Patch File",
    defaultPath: "/repo",
    filters,
    multiple: false,
    directory: false,
  });
});

test("choose_folder_opens_one_directory", async () => {
  vi.mocked(open).mockResolvedValue("/Users/me/shelf");

  expect(await chooseFolder({ title: "Import Patches" })).toBe("/Users/me/shelf");
  expect(open).toHaveBeenCalledWith({
    title: "Import Patches",
    multiple: false,
    directory: true,
  });
});

test("choose_save_file_returns_the_path_to_write", async () => {
  vi.mocked(save).mockResolvedValue("/Users/me/new.patch");

  expect(await chooseSaveFile({ defaultPath: "/repo/new.patch" })).toBe("/Users/me/new.patch");
  expect(save).toHaveBeenCalledWith({ defaultPath: "/repo/new.patch" });
});

// A list where one was asked for would be a bug of the plugin: only its first path is used.
test("choose_file_takes_the_first_of_a_list", async () => {
  vi.mocked(open).mockResolvedValue(["/a.patch", "/b.patch"] as never);

  expect(await chooseFile()).toBe("/a.patch");
});
