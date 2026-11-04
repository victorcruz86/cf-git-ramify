import { EditorView } from "@codemirror/view";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import type { MergeBlock, ThreeSides } from "../lib/api";
import { loadOptions } from "./options";
import ThreeVersionsPane from "./ThreeVersionsPane.svelte";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(), emit: vi.fn() }));

// jsdom does no layout: CodeMirror measures with this.
Range.prototype.getClientRects = () => [] as unknown as DOMRectList;
Element.prototype.scrollIntoView = () => {};

const SIDES: ThreeSides = {
  head: "a\nb\nc\nd\n",
  staged: "a\nB\nc\nd\n",
  local: "a\nB\nc\nD\n",
  stagedHash: "sh",
  localHash: "lh",
};

// Line 1 only changes from HEAD to the stage, and line 3 only from the stage to the disk.
const BLOCKS: MergeBlock[] = [
  {
    left: [1, 2],
    base: [1, 2],
    right: [1, 2],
    kind: "modified",
    leftChanged: true,
    rightChanged: false,
    auto: null,
  },
  {
    left: [3, 4],
    base: [3, 4],
    right: [3, 4],
    kind: "modified",
    leftChanged: false,
    rightChanged: true,
    auto: null,
  },
];

const CHANGED = { kind: "failed", message: "\"a.txt\" has changed while the operation was being prepared" };

function mockCommands(commands: Record<string, (args: never) => unknown> = {}) {
  const all: Record<string, (args: never) => unknown> = {
    staging_three_sides: () => SIDES,
    merge_blocks: () => BLOCKS,
    index_write: (args: { content: string }) => ({ hash: `blob:${args.content}`, copy: null }),
    staging_revert_range: () => ({ hash: "new", copy: "snapshot" }),
    file_write: () => "written",
    snapshots_restore: () => "previous",
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

const notices: { kind: string; text: string; action?: { label: string; run: () => void } }[] = [];
let fallbacks = 0;

function show(props: { path?: string; oldPath?: string | null } = {}) {
  return render(ThreeVersionsPane, {
    path: props.path ?? "a.txt",
    oldPath: props.oldPath ?? null,
    version: 0,
    options: loadOptions(),
    onnotice: (kind: string, text: string, action?: { label: string; run: () => void }) =>
      notices.push({ kind, text, action }),
    onfallback: () => fallbacks++,
  });
}

function view(pane: "left" | "base" | "right"): EditorView {
  const content = document.querySelector<HTMLElement>(`[data-pane="${pane}"] .cm-content`);
  return EditorView.findFromDOM(content as HTMLElement) as EditorView;
}

const text = (pane: "left" | "base" | "right") => view(pane).state.sliceDoc();

function operation(pane: "left" | "base" | "right", action: string): HTMLElement | null {
  return document.querySelector<HTMLElement>(
    `[data-pane="${pane}"] .rm-operation[data-action="${action}"]`,
  );
}

function click(element: HTMLElement) {
  element.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true, button: 0 }));
}

async function opened() {
  await waitFor(() => expect(operation("right", "stage")).not.toBeNull());
}

beforeEach(() => {
  notices.length = 0;
  fallbacks = 0;
  vi.mocked(invoke).mockReset();
  vi.mocked(emit).mockReset();
  vi.mocked(emit).mockResolvedValue(undefined);
});

afterEach(cleanup);

test("three_panes_head_staged_local", async () => {
  mockCommands();
  show();

  await opened();

  expect([...document.querySelectorAll(".rm-merge-title")].map((title) => title.textContent)).toEqual([
    "HEAD",
    "Staged",
    "Local",
  ]);
  expect(text("left")).toBe(SIDES.head);
  expect(text("base")).toBe(SIDES.staged);
  expect(text("right")).toBe(SIDES.local);
  expect(calls("merge_blocks")[0][1]).toEqual({
    left: SIDES.head,
    base: SIDES.staged,
    right: SIDES.local,
    policy: "default",
  });
  // SimpleThreesideDiffChange: HEAD→Staged where HEAD changed, and Staged⇄Local where the disk changed.
  expect(operation("left", "unstage")?.getAttribute("aria-label")).toBe("Unstage");
  expect(operation("base", "revert")?.getAttribute("aria-label")).toBe("Revert");
  expect(operation("right", "stage")?.getAttribute("aria-label")).toBe("Stage");
  expect(document.querySelectorAll(".rm-operation")).toHaveLength(3);
  expect(view("left").state.readOnly).toBe(true);
  expect(view("base").state.readOnly).toBe(false);
  expect(view("right").state.readOnly).toBe(false);
});

test("stage_of_a_range_writes_the_index", async () => {
  mockCommands();
  show();
  await opened();

  click(operation("right", "stage") as HTMLElement);

  await waitFor(() =>
    expect(calls("index_write")).toEqual([
      ["index_write", { path: "a.txt", content: "a\nB\nc\nD\n", expectedHash: "sh" }],
    ]),
  );
  expect(text("base")).toBe("a\nB\nc\nD\n");
  expect(text("right")).toBe(SIDES.local);
  await waitFor(() => expect(vi.mocked(emit).mock.calls.map(([name]) => name)).toContain("repo-changed"));
  expect(calls("file_write")).toHaveLength(0);
});

test("unstage_of_a_range_returns_head_to_the_index", async () => {
  mockCommands();
  show();
  await opened();

  click(operation("left", "unstage") as HTMLElement);

  await waitFor(() =>
    expect(calls("index_write")).toEqual([
      ["index_write", { path: "a.txt", content: "a\nb\nc\nd\n", expectedHash: "sh" }],
    ]),
  );
  expect(text("left")).toBe(SIDES.head);
});

test("revert_of_a_range_changes_the_file_with_a_snapshot", async () => {
  mockCommands();
  show();
  await opened();

  click(operation("base", "revert") as HTMLElement);

  await waitFor(() =>
    expect(calls("staging_revert_range")).toEqual([
      ["staging_revert_range", { path: "a.txt", content: "a\nB\nc\nd\n", expectedHash: "lh" }],
    ]),
  );
  expect(calls("file_write")).toHaveLength(0);
  expect(calls("index_write")).toHaveLength(0);
  const notice = notices.find((found) => found.action?.label === "Restore");
  expect(notice?.kind).toBe("info");
  notice?.action?.run();
  await waitFor(() =>
    expect(calls("snapshots_restore")).toEqual([["snapshots_restore", { id: "snapshot" }]]),
  );
});

test("what_is_typed_on_the_disk_is_saved_without_a_snapshot", async () => {
  mockCommands();
  const pane = show();
  await opened();
  view("right").dispatch({ changes: { from: 0, insert: "X" }, userEvent: "input.type" });

  expect(await pane.component.flush()).toBe(true);

  expect(calls("file_write")).toEqual([
    ["file_write", { path: "a.txt", content: "Xa\nB\nc\nD\n", expectedHash: "lh" }],
  ]);
  expect(calls("staging_revert_range")).toHaveLength(0);
});

test("index_changed_outside_is_not_overwritten", async () => {
  let reads = 0;
  mockCommands({
    staging_three_sides: () => (reads++ === 0 ? SIDES : { ...SIDES, stagedHash: "from Devin" }),
    index_write: () => {
      throw CHANGED;
    },
  });
  const pane = show();
  await opened();
  view("base").dispatch({ changes: { from: 0, insert: "mine " }, userEvent: "input.type" });

  expect(await pane.component.flush()).toBe(false);

  expect(calls("index_write")).toHaveLength(1);
  expect(notices.map((notice) => notice.text)).toContain(CHANGED.message);
  expect(text("base")).toBe("mine a\nB\nc\nd\n");
  expect(await screen.findByText(/The stage has changed outside/)).toBeTruthy();
  expect(pane.component.isDirty()).toBe(true);

  await fireEvent.click(screen.getByRole("button", { name: "Reload and Discard Edits" }));

  await waitFor(() => expect(text("base")).toBe(SIDES.staged));
  expect(pane.component.isDirty()).toBe(false);
});

test("only_with_the_three_versions", async () => {
  mockCommands({
    staging_three_sides: () => {
      throw { kind: "no_three_sides", message: "without the three" };
    },
  });
  show();

  await waitFor(() => expect(fallbacks).toBe(1));
  expect(document.querySelector(".cm-editor")).toBeNull();
  expect(notices).toHaveLength(0);
});

test("title_with_rename_orig_arrow_new", async () => {
  mockCommands();
  show({ path: "src/new.txt", oldPath: "src/old.txt" });

  // getTitle(status.path, status.origPath): IntelliJ puts the path of `status.path` first.
  expect(await screen.findByText("new.txt -> old.txt (src)")).toBeTruthy();
});

test("without_rename_the_title_is_the_file_and_its_folder", async () => {
  mockCommands();
  show({ path: "src/a.txt" });

  expect(await screen.findByText("a.txt (src)")).toBeTruthy();
});

test("what_was_typed_is_compared_again", async () => {
  mockCommands();
  show();
  await opened();
  const before = calls("merge_blocks").length;

  view("right").dispatch({ changes: { from: 0, insert: "X" }, userEvent: "input.type" });

  await waitFor(() => expect(calls("merge_blocks").length).toBe(before + 1));
  expect(calls("merge_blocks").at(-1)?.[1]).toMatchObject({ right: "Xa\nB\nc\nD\n" });
});

test("with_the_disk_changed_outside_and_nothing_typed_it_is_reread", async () => {
  let reads = 0;
  mockCommands({
    staging_three_sides: () => (reads++ === 0 ? SIDES : { ...SIDES, local: "Devin\n", localHash: "d" }),
  });
  const pane = show();
  await opened();

  await pane.rerender({ version: 1 });

  await waitFor(() => expect(text("right")).toBe("Devin\n"));
  expect(pane.component.isDirty()).toBe(false);
});

const GONE = { kind: "no_three_sides", message: "without the three" };

for (const pane of ["base", "right"] as const) {
  test(`without_three_versions_unsaved_changes_are_not_lost (${pane})`, async () => {
    let reads = 0;
    mockCommands({
      staging_three_sides: () => {
        if (reads++ === 0) return SIDES;
        throw GONE;
      },
    });
    const pane_ = show();
    await opened();
    view(pane).dispatch({ changes: { from: 0, insert: "mine " }, userEvent: "input.type" });
    const typed = text(pane);

    await pane_.rerender({ version: 1 });

    expect(
      await screen.findByText(/The file no longer has HEAD, stage and disk at the same time/),
    ).toBeTruthy();
    expect(fallbacks).toBe(0);
    expect(text(pane)).toBe(typed);
    expect(pane_.component.isDirty()).toBe(true);

    await fireEvent.click(screen.getByRole("button", { name: "Reload and Discard Edits" }));

    await waitFor(() => expect(fallbacks).toBe(1));
  });
}

test("without_three_versions_and_nothing_typed_it_switches_to_two_panes_without_notice", async () => {
  let reads = 0;
  mockCommands({
    staging_three_sides: () => {
      if (reads++ === 0) return SIDES;
      throw GONE;
    },
  });
  const pane = show();
  await opened();

  await pane.rerender({ version: 1 });

  await waitFor(() => expect(fallbacks).toBe(1));
  expect(screen.queryByRole("alert")).toBeNull();
  expect(notices).toHaveLength(0);
});
