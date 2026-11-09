import { fireEvent, screen, waitFor, within } from "@testing-library/svelte";
import { beforeEach, describe, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { BranchList, FileRevision } from "../lib/api";
import {
  branchChoices,
  compareWithBranchPopup,
  compareWithRevisionPopup,
  compareWithTagPopup,
  nothingToCompare,
} from "./popups";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const ROOT = "/work/repo";
const HASH = "a".repeat(40);
const OTHER = "b".repeat(40);

const BRANCHES: BranchList = {
  local: [
    { name: "main", tracked: null },
    { name: "Zeta", tracked: null },
    { name: "alpha", tracked: null },
    { name: "beta", tracked: null },
  ],
  remote: ["origin/main", "origin/Alpha"],
  recent: [],
  current: "main",
  previous: null,
};

const revision = (hash: string, author: string, message: string): FileRevision => ({
  hash,
  commitTime: 100,
  authorTime: 100,
  author,
  message,
});

function mockCommands(commands: Record<string, (args: never) => unknown> = {}) {
  const all: Record<string, (args: never) => unknown> = {
    branch_list: () => BRANCHES,
    compare_tags: () => ["v1.0", "v2.0"],
    compare_local_state: () => ({
      root: ROOT,
      modified: true,
      current: null,
      headLabel: "main",
    }),
    compare_resolve: () => revision(HASH, "Test", "x\n"),
    compare_changes_with: () => [{ path: "a.txt", oldPath: null, status: "M" }],
    compare_last_revision: () => revision(HASH, "Test", "x\n"),
    diff_window_open_request: () => undefined,
    ...commands,
  };
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    const handler = all[command];
    if (!handler) throw new Error(`unexpected command: ${command}`);
    return handler(args as never);
  });
}

const calls = (command: string) =>
  vi.mocked(invoke).mock.calls.filter(([name]) => name === command).map(([, args]) => args);

const options = (dialog: HTMLElement) =>
  within(dialog)
    .queryAllByRole("option")
    .map((option) => option.textContent?.trim());

async function typeInto(dialog: HTMLElement, text: string) {
  for (const key of text) await fireEvent.keyDown(dialog, { key });
}

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  document.body.innerHTML = "";
});

describe("popups_list_branches_and_tags_with_speed_search", () => {
  test("the branches but the current one, local before remote, each sorted", async () => {
    mockCommands();

    const done = compareWithBranchPopup("a.txt");
    const dialog = await screen.findByRole("dialog", { name: "Select Branch to Compare" });

    expect(within(dialog).getByText("Select Branch to Compare")).toBeTruthy();
    expect(options(dialog)).toEqual(["alpha", "beta", "Zeta", "origin/Alpha", "origin/main"]);
    await typeInto(dialog, "ze");
    expect(options(dialog)).toEqual(["Zeta"]);
    await fireEvent.keyDown(dialog, { key: "Enter" });
    await done;

    expect(screen.queryByRole("dialog")).toBeNull();
    expect(calls("compare_resolve")).toEqual([{ kind: "branch", name: "Zeta" }]);
    expect(calls("diff_window_open_request")).toHaveLength(1);
  });

  test("Escape closes the popup without comparing", async () => {
    mockCommands();

    const done = compareWithBranchPopup("a.txt");
    const dialog = await screen.findByRole("dialog", { name: "Select Branch to Compare" });
    await fireEvent.keyDown(dialog, { key: "Escape" });
    await done;

    expect(screen.queryByRole("dialog")).toBeNull();
    expect(calls("compare_resolve")).toHaveLength(0);
  });

  test("the tags in git's order, chosen with a click", async () => {
    mockCommands();

    const done = compareWithTagPopup("a.txt");
    const dialog = await screen.findByRole("dialog", { name: "Select Tag to Compare" });
    expect(options(dialog)).toEqual(["v1.0", "v2.0"]);
    await typeInto(dialog, "2");
    expect(options(dialog)).toEqual(["v2.0"]);
    await fireEvent.click(within(dialog).getByRole("option", { name: "v2.0" }));
    await done;

    expect(calls("compare_resolve")).toEqual([{ kind: "tag", name: "v2.0" }]);
    expect(calls("diff_window_open_request")).toHaveLength(1);
  });

  test("the action has nothing to compare without another branch", () => {
    const only = { ...BRANCHES, local: [{ name: "main", tracked: null }], remote: [] };

    expect(nothingToCompare(only)).toBe(true);
    expect(nothingToCompare({ ...only, current: null })).toBe(false);
    expect(nothingToCompare(BRANCHES)).toBe(false);
    expect(branchChoices({ ...BRANCHES, current: null })).toContain("main");
  });
});

describe("a_missing_branch_says_d_102", () => {
  test("a file the branch does not have", async () => {
    mockCommands({ compare_changes_with: () => [], compare_last_revision: () => null });

    const done = compareWithBranchPopup("src/a.txt");
    const dialog = await screen.findByRole("dialog", { name: "Select Branch to Compare" });
    await fireEvent.click(within(dialog).getByRole("option", { name: "beta" }));

    await expect(done).rejects.toThrow(`File ${ROOT}/src/a.txt doesn't exist in branch 'beta'`);
    expect(calls("diff_window_open_request")).toHaveLength(0);
  });

  test("a file the tag does not have", async () => {
    mockCommands({ compare_changes_with: () => [], compare_last_revision: () => null });

    const done = compareWithTagPopup("a.txt");
    const dialog = await screen.findByRole("dialog", { name: "Select Tag to Compare" });
    await fireEvent.click(within(dialog).getByRole("option", { name: "v1.0" }));

    await expect(done).rejects.toThrow(`File ${ROOT}/a.txt doesn't exist in tag 'v1.0'`);
  });

  test("tags that cannot be read say `Cannot load tags`", async () => {
    mockCommands({
      compare_tags: () => {
        throw { kind: "failed", message: "fatal: broken" };
      },
    });

    await expect(compareWithTagPopup("a.txt")).rejects.toThrow("Cannot load tags");
  });
});

describe("file_revisions_popup", () => {
  test("lists hash, date and author with the message of the selected row", async () => {
    mockCommands({
      compare_file_revisions: () => [
        revision(HASH, "Ana", "first\n\nbody\n"),
        revision(OTHER, "Devin", "second\n"),
      ],
      compare_last_revision: (args: { revision: string }) =>
        revision(args.revision, "Devin", "second\n"),
    });

    const done = compareWithRevisionPopup("a.txt");
    const dialog = await screen.findByRole("dialog", { name: "File Revisions" });
    const rows = within(dialog).getAllByRole("row");
    expect(rows.map((row) => row.querySelectorAll('[role="gridcell"]')[0].textContent)).toEqual([
      HASH,
      OTHER,
    ]);
    expect(rows[1].textContent).toContain("Devin");
    const message = within(dialog).getByRole("textbox", { name: "Commit message" });
    expect((message as HTMLTextAreaElement).value).toBe("first\n\nbody\n");

    await typeInto(dialog, "devin");
    expect(rows[1].getAttribute("aria-selected")).toBe("true");
    expect((message as HTMLTextAreaElement).value).toBe("second\n");
    await fireEvent.keyDown(dialog, { key: "Enter" });
    await done;

    await waitFor(() => expect(calls("diff_window_open_request")).toHaveLength(1));
    expect(calls("diff_window_open_request")[0]).toMatchObject({
      request: { files: [{ left: { revision: OTHER } }] },
    });
  });

  // The history walk gives each revision its path: before a rename, the old one.
  test("a revision from before a rename is compared at its old path", async () => {
    mockCommands({
      compare_file_revisions: () => [{ ...revision(OTHER, "Devin", "renamed\n"), path: "old.txt" }],
      compare_last_revision: (args: { revision: string }) =>
        revision(args.revision, "Devin", "renamed\n"),
    });

    const done = compareWithRevisionPopup("a.txt");
    const dialog = await screen.findByRole("dialog", { name: "File Revisions" });
    await fireEvent.keyDown(dialog, { key: "Enter" });
    await done;

    await waitFor(() => expect(calls("diff_window_open_request")).toHaveLength(1));
    expect(calls("diff_window_open_request")[0]).toMatchObject({
      request: {
        files: [{ left: { revision: OTHER, path: "old.txt" }, right: { path: "a.txt" } }],
      },
    });
  });
});
