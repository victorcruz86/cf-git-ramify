import { get } from "svelte/store";
import { afterEach, expect, test, vi } from "vitest";
import { dismiss, notifications } from "../lib/notifications";
import { copyBranchName } from "./copyName";

afterEach(() => get(notifications).forEach(({ id }) => dismiss(id)));

const shown = () => get(notifications).map(({ kind, title, body }) => ({ kind, title, body }));

test("copying_writes_the_full_name_and_says_so", async () => {
  const write = vi.fn(async () => {});

  await copyBranchName("bugfix/HT-730-partner-team-removal-error-message", write);

  expect(write).toHaveBeenCalledWith("bugfix/HT-730-partner-team-removal-error-message");
  expect(shown()).toEqual([
    { kind: "info", title: "Branch name copied", body: "bugfix/HT-730-partner-team-removal-error-message" },
  ]);
});

test("a_clipboard_that_fails_shows_an_error_instead", async () => {
  const write = vi.fn(async () => {
    throw new Error("no pasteboard");
  });

  await copyBranchName("main", write);

  expect(shown()).toEqual([{ kind: "error", title: "Could not copy the branch name", body: "no pasteboard" }]);
});
