import { beforeEach, describe, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { FileRevision, LocalState } from "../lib/api";
import {
  compareChangesWithLocal,
  compareRevisions,
  compareSameVersion,
  compareWithBranch,
  compareWithLocal,
  compareWithRevision,
  contentTitle,
  requestTitle,
} from "./actions";
import { get } from "svelte/store";
import { changesTab } from "../changes/tab";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const ROOT = "/work/repo";
const CURRENT = "c".repeat(40);
const OLDER = "a".repeat(40);
const NEWER = "e".repeat(40);
const LATER = "b".repeat(40);

const revision = (hash: string, commitTime: number): FileRevision => ({
  hash,
  commitTime,
  authorTime: commitTime,
  author: "Test",
  message: "message\n",
});

function state(change: Partial<LocalState> = {}): LocalState {
  return {
    root: ROOT,
    modified: false,
    current: revision(CURRENT, 200),
    headLabel: "main",
    ...change,
  };
}

function mockCommands(commands: Record<string, (args: never) => unknown>) {
  const all: Record<string, (args: never) => unknown> = {
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

const opened = () => (calls("diff_window_open_request").at(-1) as { request: unknown }).request;

beforeEach(() => {
  vi.mocked(invoke).mockReset();
});

describe("same_version_titles_are_full_hash_and_local_and_swap_when_the_target_is_newer", () => {
  test("a modified file is compared with its revision as `Local`", async () => {
    mockCommands({ compare_local_state: () => state({ modified: true }) });

    await compareSameVersion("src/a.txt");

    expect(opened()).toEqual({
      kind: "revisions",
      title: `a.txt (${ROOT}/src)`,
      files: [
        {
          left: { revision: CURRENT, path: "src/a.txt", title: CURRENT },
          right: { revision: "local", path: "src/a.txt", title: "Local" },
        },
      ],
    });
  });

  test("an unmodified file names the revision it has", async () => {
    mockCommands({ compare_local_state: () => state() });

    await compareSameVersion("a.txt");

    expect(opened()).toMatchObject({
      title: `a.txt (${ROOT})`,
      files: [
        {
          left: { revision: CURRENT, title: CURRENT },
          right: { revision: "local", title: `Local (${CURRENT})` },
        },
      ],
    });
  });

  test("without a revision of the file it says so and opens nothing", async () => {
    mockCommands({ compare_local_state: () => state({ current: null }) });

    await expect(compareSameVersion("a.txt")).rejects.toThrow("Cannot get revision content");
    expect(calls("diff_window_open_request")).toHaveLength(0);
  });

  test("a newer revision of an unmodified file goes right and the local file left", async () => {
    mockCommands({
      compare_local_state: () => state(),
      compare_last_revision: () => revision(NEWER, 300),
    });

    await compareWithRevision("a.txt", NEWER);

    expect(calls("compare_last_revision")[0]).toEqual({ revision: NEWER, path: null });
    expect(opened()).toMatchObject({
      files: [
        {
          left: { revision: "local", title: `Local (${CURRENT})` },
          right: { revision: NEWER, title: NEWER },
        },
      ],
    });
  });

  test("at the same time the hash decides which one is newer", async () => {
    mockCommands({
      compare_local_state: () => state(),
      compare_last_revision: (args: { revision: string }) =>
        revision(args.revision, 200),
    });

    await compareWithRevision("a.txt", OLDER);
    expect(opened()).toMatchObject({ files: [{ left: { revision: OLDER } }] });

    await compareWithRevision("a.txt", NEWER);
    expect(opened()).toMatchObject({ files: [{ left: { revision: "local" } }] });
  });

  test("with local changes the sides never swap", async () => {
    mockCommands({
      compare_local_state: () => state({ modified: true }),
      compare_last_revision: () => revision(LATER, 999),
    });

    await compareWithRevision("a.txt", LATER, "old/a.txt");

    expect(opened()).toMatchObject({
      title: `a.txt (${ROOT})`,
      files: [
        {
          left: { revision: LATER, path: "old/a.txt", title: LATER },
          right: { revision: "local", path: "a.txt", title: "Local" },
        },
      ],
    });
  });
});

describe("compare_with_branch_titles_are_branch_and_current_local", () => {
  test("on a branch the right side is `<branch> (Local)`", async () => {
    mockCommands({
      compare_local_state: () => state(),
      compare_resolve: () => revision(OLDER, 100),
      compare_changes_with: () => [{ path: "src/a.txt", oldPath: null, status: "M" }],
    });

    await compareWithBranch("src/a.txt", "feature");

    expect(calls("compare_resolve")[0]).toEqual({ kind: "branch", name: "feature" });
    expect(calls("compare_changes_with")[0]).toEqual({ revision: OLDER, paths: ["src/a.txt"] });
    expect(opened()).toEqual({
      kind: "revisions",
      title: `a.txt (${ROOT}/src)`,
      files: [
        {
          left: { revision: OLDER, path: "src/a.txt", title: "feature" },
          right: { revision: "local", path: "src/a.txt", title: "main (Local)" },
        },
      ],
    });
  });

  test("with a detached HEAD the right side names its short hash", async () => {
    mockCommands({
      compare_local_state: () => state({ headLabel: "cccccccc" }),
      compare_resolve: () => revision(OLDER, 100),
      compare_changes_with: () => [],
      compare_last_revision: () => revision(OLDER, 100),
    });

    await compareWithBranch("a.txt", "origin/main");

    expect(opened()).toMatchObject({
      files: [
        {
          left: { title: "origin/main" },
          right: { title: "cccccccc (Local)" },
        },
      ],
    });
  });

  test("a file that the branch does not have is compared with nothing", async () => {
    mockCommands({
      compare_local_state: () => state(),
      compare_resolve: () => revision(OLDER, 100),
      compare_changes_with: () => [{ path: "a.txt", oldPath: null, status: "A" }],
    });

    await compareWithBranch("a.txt", "feature");

    expect(opened()).toMatchObject({ files: [{ left: { revision: null, title: "feature" } }] });
  });
});

describe("a_renamed_file_titles_old_arrow_new", () => {
  test("a rename between two revisions", async () => {
    mockCommands({ compare_local_state: () => state() });

    await compareRevisions(OLDER, LATER, [{ path: "dir/new.txt", revisionPath: "dir/old.txt" }]);

    expect(opened()).toEqual({
      kind: "revisions",
      title: `old.txt -> new.txt (${ROOT}/dir)`,
      files: [
        {
          left: { revision: OLDER, path: "dir/old.txt", title: OLDER },
          right: { revision: LATER, path: "dir/new.txt", title: LATER },
        },
      ],
    });
  });

  test("a rename against the local file", async () => {
    mockCommands({ compare_local_state: () => state() });

    await compareWithLocal(OLDER, [{ path: "new.txt", revisionPath: "old.txt" }]);

    expect(opened()).toMatchObject({
      title: `old.txt -> new.txt (${ROOT})`,
      files: [
        {
          left: { revision: OLDER, path: "old.txt", title: OLDER },
          right: { revision: "local", path: "new.txt", title: "Local" },
        },
      ],
    });
  });

  test("the window titles follow DiffRequestFactoryImpl", () => {
    expect(contentTitle(ROOT, "a.txt")).toBe(`a.txt (${ROOT})`);
    expect(requestTitle(ROOT, "a/x.txt", "a/x.txt")).toBe(`x.txt (${ROOT}/a)`);
    expect(requestTitle(ROOT, null, "a/x.txt")).toBe(`x.txt (${ROOT}/a)`);
    expect(requestTitle(ROOT, "a/x.txt", "b/x.txt")).toBe(`x.txt (${ROOT}/a -> ${ROOT}/b)`);
    expect(requestTitle(ROOT, "a/x.txt", "b/y.txt")).toBe(
      `x.txt -> y.txt (${ROOT}/a -> ${ROOT}/b)`,
    );
  });
});

describe("folders_and_several_files_go_to_the_changes_window", () => {
  test("they open the Changes tab instead of the diff window", async () => {
    mockCommands({
      compare_changes_with: () => [{ path: "dir/a.txt", oldPath: null, status: "M" }],
      compare_changes_between: () => [{ path: "b.txt", oldPath: null, status: "A" }],
      repo_state: () => ({ operation: null, branch: "main", label: "main", head: CURRENT }),
    });

    await compareWithLocal(OLDER, [{ path: "dir", folder: true }]);
    expect(get(changesTab)).toMatchObject({
      title: `Changes Between ${OLDER.slice(0, 8)} and local version in dir`,
      left: OLDER,
      right: "local",
      files: [{ path: "dir/a.txt", oldPath: null, status: "M" }],
      paths: ["dir"],
      head: CURRENT,
    });

    await compareRevisions(OLDER, LATER, [{ path: "a.txt" }, { path: "b.txt" }]);
    expect(get(changesTab)).toMatchObject({
      title: `Changes Between ${OLDER.slice(0, 8)} and ${LATER.slice(0, 8)} in a.txt, b.txt`,
      left: OLDER,
      right: LATER,
      head: null,
    });
    expect(calls("diff_window_open_request")).toHaveLength(0);
  });

  // The HEAD is read before the list, so the list never looks newer than its HEAD.
  test("the HEAD of a local comparison is read before its list", async () => {
    mockCommands({
      compare_changes_with: () => [],
      repo_state: () => ({ operation: null, branch: "main", label: "main", head: CURRENT }),
    });

    await compareWithLocal(OLDER, [{ path: "a.txt" }, { path: "b.txt" }]);

    const order = vi
      .mocked(invoke)
      .mock.calls.map(([name]) => name)
      .filter((name) => name === "repo_state" || name === "compare_changes_with");
    expect(order).toEqual(["repo_state", "compare_changes_with"]);
  });
});

// `ShowDiffWithLocalAction` of a changes browser: each change's after revision against the local
// file (its other path for a rename only on disk there), nothing for a change with neither, and
// the window opens on the change under the cursor (`WithLocalRequestChain`'s `ListSelection`).
test("compare_with_local_from_a_changes_browser_opens_on_the_lead_change", async () => {
  mockCommands({ compare_local_state: () => state() });
  const base = OLDER;
  const changes = [
    { file: { path: "a.txt", oldPath: null, status: "M" }, hash: NEWER, base },
    { file: { path: "gone.txt", oldPath: null, status: "D" }, hash: NEWER, base },
    { file: { path: "dir/new.txt", oldPath: "dir/old.txt", status: "R" }, hash: NEWER, base },
  ];

  await compareChangesWithLocal(changes, false, new Set(["a.txt", "dir/old.txt"]), changes[2]);

  expect(opened()).toEqual({
    kind: "revisions",
    title: `new.txt -> old.txt (${ROOT}/dir)`,
    selected: 1,
    files: [
      {
        left: { revision: NEWER, path: "a.txt", title: NEWER },
        right: { revision: "local", path: "a.txt", title: "Your version" },
      },
      {
        left: { revision: NEWER, path: "dir/new.txt", title: NEWER },
        right: { revision: "local", path: "dir/old.txt", title: "Your version" },
      },
    ],
  });
});
