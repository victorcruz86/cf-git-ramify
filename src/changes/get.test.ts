import { beforeEach, describe, expect, test, vi } from "vitest";
import { get } from "svelte/store";
import { invoke } from "@tauri-apps/api/core";
import type { GetItem } from "../lib/api";
import { notifications } from "../lib/notifications";
import { tasks } from "../lib/tasks";
import {
  answerGetQuestion,
  getDialogsUi,
  getQuestion,
  getRevision,
  LOADING_CONTENT,
  presentableName,
  type GetUi,
} from "./get";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}), emitTo: vi.fn() }));

const ROOT = "/work/repo";
const HEAD = "c".repeat(40);
const OLD = "a".repeat(40);
const ITEMS: GetItem[] = [{ path: "src/a.txt", revision: OLD, oldPath: null }];
// What the check read of each path, which the write compares with.
const SEEN = [{ path: "src/a.txt", hash: "h".repeat(64) }];

function mockCommands(commands: Record<string, (args: never) => unknown>) {
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    const handler = commands[command];
    if (!handler) throw new Error(`unexpected command: ${command}`);
    return handler(args as never);
  });
}

const calls = (command: string) =>
  vi.mocked(invoke).mock.calls.filter(([name]) => name === command).map(([, args]) => args);

function fakeUi(answer: boolean) {
  const asked: { message: string; ok: string }[] = [];
  const errors: { message: string; icon: string }[] = [];
  const ui: GetUi = {
    confirm: async (message, ok) => {
      asked.push({ message, ok });
      return answer;
    },
    error: async (message, icon) => {
      errors.push({ message, icon });
    },
  };
  return { ui, asked, errors };
}

beforeEach(() => {
  vi.mocked(invoke).mockReset();
});

describe("get_over_a_modified_file_asks_overwrite_modified_file_and_cancel_stops", () => {
  const check = (modified: string[]) => () => ({ root: ROOT, head: HEAD, modified, seen: SEEN });

  test("Cancel writes nothing", async () => {
    mockCommands({ revision_get_check: check(["src/a.txt"]) });
    const { ui, asked } = fakeUi(false);

    expect(await getRevision(ITEMS, { ui })).toBeNull();

    expect(asked).toEqual([
      { message: "File /work/repo/src/a.txt has been locally modified.", ok: "Overwrite Modified File" },
    ]);
    expect(calls("revision_get")).toHaveLength(0);
  });

  test("Overwrite writes with the HEAD that was read", async () => {
    mockCommands({
      revision_get_check: check(["src/a.txt"]),
      revision_get: () => ({ copy: "copy-1", written: ["src/a.txt"], changed: [] }),
    });
    const { ui } = fakeUi(true);

    expect(await getRevision(ITEMS, { ui })).toEqual({
      copy: "copy-1",
      written: ["src/a.txt"],
      changed: [],
    });

    expect(calls("revision_get")).toEqual([{ items: ITEMS, expectedHead: HEAD, seen: SEEN }]);
    expect(get(notifications).at(-1)?.title).toBe("Get of 1 file. A snapshot was saved.");
  });

  test("several modified files have the plural texts", async () => {
    mockCommands({ revision_get_check: check(["a.txt", "b.txt"]) });
    const { ui, asked } = fakeUi(false);

    await getRevision(ITEMS, { ui });

    expect(asked).toEqual([
      { message: "Some files were locally modified.", ok: "Overwrite Modified Files" },
    ]);
  });

  test("without modified files nothing is asked", async () => {
    mockCommands({
      revision_get_check: check([]),
      revision_get: () => ({ copy: "copy-1", written: ["src/a.txt"], changed: [] }),
    });
    const { ui, asked } = fakeUi(false);

    await getRevision(ITEMS, { ui });

    expect(asked).toHaveLength(0);
    expect(calls("revision_get")).toHaveLength(1);
  });
});

test("the write runs as Loading Content and passes the HEAD of the list", async () => {
  let running: string[] = [];
  mockCommands({
    revision_get_check: () => ({ root: ROOT, head: HEAD, modified: [], seen: SEEN }),
    revision_get: () => {
      running = get(tasks).map((task) => task.title);
      return { copy: "copy-1", written: ["src/a.txt"], changed: [] };
    },
  });

  await getRevision(ITEMS, { ui: fakeUi(true).ui, expectedHead: OLD });

  expect(running).toEqual([LOADING_CONTENT]);
  expect(calls("revision_get")).toEqual([{ items: ITEMS, expectedHead: OLD, seen: SEEN }]);
});

test("a file left because it changed meanwhile is reported", async () => {
  mockCommands({
    revision_get_check: () => ({ root: ROOT, head: HEAD, modified: [], seen: SEEN }),
    revision_get: () => ({ copy: "copy-1", written: [], changed: ["src/a.txt"] }),
  });

  await getRevision(ITEMS, { ui: fakeUi(true).ui });

  expect(get(notifications).at(-1)).toMatchObject({
    kind: "warning",
    title: '"src/a.txt" changed after the snapshot was saved and was left as it is.',
  });
});

// `get_version.rs` puts an item written at the old path of a rename, whose local file at the new
// path changed and was left, in both lists: Restore is offered for what the old path held.
test("an item written at the old path and left at the new one offers Restore", async () => {
  const items: GetItem[] = [{ path: "new.txt", revision: OLD, oldPath: "old.txt" }];
  mockCommands({
    revision_get_check: () => ({ root: ROOT, head: HEAD, modified: [], seen: SEEN }),
    revision_get: () => ({ copy: "copy-2", written: ["old.txt"], changed: ["new.txt"] }),
  });

  await getRevision(items, { ui: fakeUi(true).ui });

  const shown = get(notifications).slice(-2);
  expect(shown[0]).toMatchObject({ kind: "info", title: "Get of 1 file. A snapshot was saved." });
  expect(shown[0].actions?.map((action) => action.label)).toEqual(["Restore"]);
  expect(shown[1]).toMatchObject({
    kind: "warning",
    title: '"new.txt" changed after the snapshot was saved and was left as it is.',
  });
});

test("errors say Cannot load revision or Cannot save content", async () => {
  const failing = (kind: string, message: string) => {
    mockCommands({
      revision_get_check: () => ({ root: ROOT, head: HEAD, modified: [], seen: SEEN }),
      revision_get: () => {
        throw { kind, message };
      },
    });
  };
  const { ui, errors } = fakeUi(true);

  failing("load_failed", "bad object");
  await getRevision(ITEMS, { ui });
  failing("save_failed", "Permission denied");
  await getRevision(ITEMS, { ui });
  failing("failed", "Repository has already been changed");
  await getRevision(ITEMS, { ui });

  expect(errors).toEqual([
    { message: "Cannot load revision: bad object", icon: "information" },
    { message: "Cannot save content: Permission denied", icon: "error" },
    { message: "Repository has already been changed", icon: "error" },
  ]);
});

test("a write that fails after the copy says so and offers its Restore", async () => {
  mockCommands({
    revision_get_check: () => ({ root: ROOT, head: HEAD, modified: [], seen: SEEN }),
    revision_get: () => {
      throw {
        kind: "save_failed",
        message: "Permission denied",
        details: { copy: "copy-7", written: ["a.txt"], changed: ["b.txt"] },
      };
    },
    snapshots_restore: () => "copy-8",
  });
  const { ui, errors } = fakeUi(true);

  await getRevision(ITEMS, { ui });

  expect(errors).toEqual([
    {
      message:
        "Cannot save content: Permission denied. Your previous work is in snapshot copy-7",
      icon: "error",
    },
  ]);
  const shown = get(notifications);
  expect(shown.at(-2)?.title).toBe("Get of 1 file. A snapshot was saved.");
  expect(shown.at(-1)?.title).toBe(
    '"b.txt" changed after the snapshot was saved and was left as it is.',
  );
  shown.at(-2)?.actions?.[0].run();
  await vi.waitFor(() => expect(calls("snapshots_restore")).toEqual([{ id: "copy-7", expectedRepo: ROOT }]));
});

test("a second question while one is open is answered Cancel", async () => {
  const first = getDialogsUi.confirm("first", "OK");
  expect(await getDialogsUi.confirm("second", "OK")).toBe(false);
  expect(get(getQuestion)?.request).toEqual({ kind: "confirm", message: "first", ok: "OK" });
  answerGetQuestion(true);
  expect(await first).toBe(true);
});

// `FileNameSplittingTest.testSplitByLetter`: each text is what its own length gives.
test("the path of the question is shortened by letter", () => {
  const path = "c:/dir1/dir2/dir3/dir4/file.txt";
  const expected = [
    "t",
    "xt",
    "txt",
    ".txt",
    "e.txt",
    "le.txt",
    "ile.txt",
    "file.txt",
    ".file.txt",
    "..file.txt",
    "...file.txt",
    "c...file.txt",
    "c.../file.txt",
    "c:.../file.txt",
    "c:...4/file.txt",
    "c:/...4/file.txt",
    "c:/...r4/file.txt",
    "c:/d...r4/file.txt",
    "c:/d...ir4/file.txt",
    "c:/di...ir4/file.txt",
    "c:/di...dir4/file.txt",
    "c:/dir...dir4/file.txt",
    "c:/dir.../dir4/file.txt",
    "c:/dir1.../dir4/file.txt",
    "c:/dir1/...3/dir4/file.txt",
    "c:/dir1/...r3/dir4/file.txt",
    "c:/dir1/d...r3/dir4/file.txt",
    "c:/dir1/d...ir3/dir4/file.txt",
    "c:/dir1/di...ir3/dir4/file.txt",
    "c:/dir1/dir2/dir3/dir4/file.txt",
  ];
  for (const text of expected) expect(presentableName(path, text.length)).toBe(text);
});
