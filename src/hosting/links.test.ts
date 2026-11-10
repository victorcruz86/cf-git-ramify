import { invoke } from "@tauri-apps/api/core";
import { beforeEach, expect, test, vi } from "vitest";
import type { LinkTarget } from "../lib/api";
import { hostingItems } from "./links";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const commit: LinkTarget = { kind: "commit", hash: "1234567890abcdef1234567890abcdef12345678" };
let references: { name: string; url: string }[] | Error = [];

beforeEach(() => {
  references = [];
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command !== "github_links") return undefined;
    if (references instanceof Error) throw { kind: "failed", message: references.message };
    return references;
  });
});

test("no_github_remote_hides_the_items", async () => {
  expect(await hostingItems(commit, "open", "Open on GitHub")).toEqual([]);
  expect(await hostingItems(commit, "copy", "Copy Link to GitHub Repository")).toEqual([]);
  expect(invoke).toHaveBeenCalledWith("github_links", { target: commit });
});

test("one_reference_is_one_item_that_opens_or_copies_its_url", async () => {
  references = [{ name: "https://github.com/o/r", url: "https://github.com/o/r/commit/abc" }];

  const [open] = await hostingItems(commit, "open", "Open on GitHub");
  expect(open).toMatchObject({ label: "Open on GitHub" });
  expect(open).not.toHaveProperty("submenu");
  (open as { run: () => void }).run();
  expect(invoke).toHaveBeenLastCalledWith("open_url", { url: "https://github.com/o/r/commit/abc" });

  const [copy] = await hostingItems(commit, "copy", "Copy Link to GitHub Repository");
  (copy as { run: () => void }).run();
  expect(invoke).toHaveBeenLastCalledWith("clipboard_write", {
    text: "https://github.com/o/r/commit/abc",
  });
});

test("two_github_remotes_give_a_submenu_named_by_url", async () => {
  references = [
    { name: "https://github.com/o/r", url: "https://github.com/o/r/commit/abc" },
    { name: "https://github.com/me/my fork", url: "https://github.com/me/my_fork/commit/abc" },
  ];

  const items = await hostingItems(commit, "open", "Open on GitHub");

  expect(items).toHaveLength(1);
  const { label, submenu } = items[0] as { label: string; submenu: { label: string; run: () => void }[] };
  expect(label).toBe("Open on GitHub");
  expect(submenu.map((item) => item.label)).toEqual([
    "https://github.com/o/r",
    "https://github.com/me/my fork",
  ]);
  submenu[1].run();
  expect(invoke).toHaveBeenLastCalledWith("open_url", {
    url: "https://github.com/me/my_fork/commit/abc",
  });
});

test("a_failure_to_find_the_references_is_reported_when_the_item_is_used", async () => {
  references = new Error("Cannot retrieve latest revision for file");
  const fail = vi.fn();

  const items = await hostingItems({ kind: "file", path: "a.txt" }, "open", "GitHub", fail);

  expect(items).toHaveLength(1);
  expect(fail).not.toHaveBeenCalled();
  (items[0] as { run: () => void }).run();
  expect(fail).toHaveBeenCalledWith({ kind: "failed", message: "Cannot retrieve latest revision for file" });
});
