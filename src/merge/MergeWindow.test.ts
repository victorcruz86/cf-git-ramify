import { EditorView } from "@codemirror/view";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { ConflictSides, MergeBlock, RangeCommit, Settings } from "../lib/api";
import MergeWindow from "./MergeWindow.svelte";
import { unprocessedMessage, withSeparator } from "./model";

// What `EditorView.lineWrapping` puts on the content; `view.lineWrapping` only follows it after a
// measure, which jsdom does not do.
const wrapped = (view: EditorView) => view.contentDOM.classList.contains("cm-lineWrapping");

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(), emit: vi.fn() }));

// jsdom does no layout: CodeMirror measures with this.
Range.prototype.getClientRects = () => [] as unknown as DOMRectList;
Element.prototype.scrollIntoView = () => {};

const SETTINGS = { mergeAutoApply: false, mergeWhitespace: "default" } as Settings;

const CONFLICT: MergeBlock = {
  kind: "conflict",
  left: [1, 2],
  base: [1, 2],
  right: [1, 2],
  leftChanged: true,
  rightChanged: true,
  auto: null,
};

const text = (value: string) => ({ kind: "text" as const, text: value });

// A conflict on the second line.
const SIDES: ConflictSides = {
  path: "a.txt",
  base: text("a\nb\nc\n"),
  yours: text("a\nX\nc\n"),
  theirs: text("a\nY\nc\n"),
  titles: ["Your version", "Result", "Changes from server"],
  hash: "h",
  reversed: false,
  blocks: [CONFLICT],
  details: [null, null],
};

// One change on each side, without conflict.
const NON_CONFLICTING: ConflictSides = {
  ...SIDES,
  base: text("a\nb\nc\nd\n"),
  yours: text("a\nX\nc\nd\n"),
  theirs: text("a\nb\nc\nD\n"),
  blocks: [
    { ...CONFLICT, kind: "modified", rightChanged: false },
    { ...CONFLICT, kind: "modified", left: [3, 4], base: [3, 4], right: [3, 4], leftChanged: false },
  ],
};

const SAME: ConflictSides = { ...SIDES, yours: SIDES.base, theirs: SIDES.base, blocks: [] };

const handlers = new Map<string, (payload: unknown) => void>();

function deliver(event: string, payload?: unknown) {
  handlers.get(event)?.(payload);
}

// Commands that launch a task return its id, and the result arrives later with
// `task-changed`. The ids are not repeated: `taskResult` remembers the finished tasks.
let nextTask = 1000;

function task(value: unknown = null, failure?: { kind: string; text: string }) {
  return () => {
    const id = nextTask++;
    setTimeout(() =>
      deliver("task-changed", {
        id,
        title: "",
        text: failure?.text ?? "",
        fraction: null,
        state: failure ? "failed" : "done",
        kind: failure?.kind ?? null,
        value,
      }),
    );
    return id;
  };
}

function mockCommands(commands: Record<string, (args: never) => unknown> = {}) {
  const all: Record<string, (args: never) => unknown> = {
    merge_window_contents: () => null,
    merge_window_path: () => "a.txt",
    settings_get: () => SETTINGS,
    settings_save: (args: { settings: Settings }) => args.settings,
    conflict_sides: task(SIDES),
    merge_working_file: () => ({ hash: "h", separator: "\n", conflicted: true }),
    merge_blocks: () => [],
    show_window: () => undefined,
    merge_window_done: () => undefined,
    conflicts_accept: task(),
    conflict_resolve: task(),
    merge_left: () => undefined,
    merge_leave_received: () => undefined,
    merge_window_ready: () => undefined,
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

const done = () => calls("merge_window_done").map(([, args]) => (args as { result: string }).result);

beforeEach(() => {
  handlers.clear();
  vi.mocked(invoke).mockReset();
  vi.mocked(listen).mockReset();
  vi.mocked(listen).mockImplementation(async (event, handler) => {
    handlers.set(event as string, (payload) =>
      handler({ event: event as string, id: 0, payload } as never),
    );
    return () => {};
  });
});

afterEach(cleanup);

async function open() {
  const view = render(MergeWindow);
  await waitFor(() => expect(calls("show_window")).toHaveLength(1));
  return view;
}

function centre(): EditorView {
  const dom = document.querySelector<HTMLElement>('[data-pane="base"] .cm-editor');
  return EditorView.findFromDOM(dom as HTMLElement) as EditorView;
}

function edit(insert = "z") {
  centre().dispatch({ changes: { from: 0, insert } });
}

const button = (name: string) => screen.getByRole("button", { name });
const footer = () => document.querySelector<HTMLElement>(".buttons")!;

async function answer(title: string, choice: string) {
  const dialog = await screen.findByRole("dialog", { name: title });
  await fireEvent.click(within(dialog).getByRole("button", { name: choice }));
}

test("buttons_accept_left_accept_right_cancel_apply", async () => {
  mockCommands({ conflict_sides: task(SAME) });
  await open();

  const [left, right] = [...footer().querySelectorAll(".left button")].map((b) => b.textContent);
  const ends = [...footer().querySelectorAll(".right button")];
  expect([left, right]).toEqual(["Accept Left", "Accept Right"]);
  // On macOS, Cancel before Apply, and Apply is the default button.
  expect(ends.map((b) => b.textContent)).toEqual(["Cancel", "Apply"]);
  expect(ends[1].classList.contains("default")).toBe(true);

  await fireEvent.keyDown(window, { key: "Enter", metaKey: true });

  await waitFor(() => expect(done()).toEqual(["resolved"]));
  expect(calls("conflict_resolve")).toHaveLength(1);
});

test("apply_without_pending_resolves_and_closes", async () => {
  mockCommands({ conflict_sides: task(SAME) });
  await open();

  await fireEvent.click(button("Apply"));

  await waitFor(() => expect(done()).toEqual(["resolved"]));
  expect(calls("conflict_resolve")[0][1]).toEqual({
    path: "a.txt",
    content: "a\nb\nc\n",
    expectedHash: "h",
  });
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("apply_with_pending_asks_apply_changes_and_mark_resolved", async () => {
  mockCommands();
  await open();

  await fireEvent.click(button("Apply"));
  const dialog = await screen.findByRole("dialog", { name: "Apply Changes" });
  expect(dialog.textContent).toContain("There is one conflict left unprocessed.");
  expect(dialog.textContent).toContain("Save changes and mark the conflict resolved anyway?");
  await answer("Apply Changes", "Continue Merge");

  expect(calls("conflict_resolve")).toHaveLength(0);
  await fireEvent.click(button("Apply"));
  await answer("Apply Changes", "Apply Changes and Mark Resolved");

  await waitFor(() => expect(done()).toEqual(["resolved"]));
  expect((calls("conflict_resolve")[0][1] as { content: string }).content).toBe("a\nb\nc\n");
});

test("the_text_of_what_is_pending_is_that_of_merge_util", () => {
  const tail = " left unprocessed.\nSave changes and mark the conflict resolved anyway?";
  expect(unprocessedMessage({ changes: 0, conflicts: 1 })).toBe(`There is one conflict${tail}`);
  expect(unprocessedMessage({ changes: 0, conflicts: 3 })).toBe(`There are 3 conflicts${tail}`);
  expect(unprocessedMessage({ changes: 1, conflicts: 0 })).toBe(`There is one change${tail}`);
  expect(unprocessedMessage({ changes: 1, conflicts: 2 })).toBe(
    `There is one change and 2 conflicts${tail}`,
  );
  expect(unprocessedMessage({ changes: 1200, conflicts: 1 })).toBe(
    `There are 1,200 changes and one conflict${tail}`,
  );
});

test("accept_left_with_changes_asks_to_discard", async () => {
  mockCommands({ conflict_sides: task({ ...SIDES, reversed: true }) });
  await open();
  edit();

  await fireEvent.click(button("Accept Left"));
  const dialog = await screen.findByRole("dialog", { name: "Accept Left" });
  expect(dialog.textContent).toContain(
    "There are unsaved changes in the result file. Discard changes and accept left anyway?",
  );
  await answer("Accept Left", "Continue Merge");
  expect(calls("conflicts_accept")).toHaveLength(0);

  await fireEvent.click(button("Accept Left"));
  await answer("Accept Left", "Discard Changes and Accept Left");

  await waitFor(() => expect(done()).toEqual(["yours"]));
  // R1: the side as the user sees it, also in a rebase; the backend inverts.
  expect(calls("conflicts_accept")[0][1]).toEqual({ paths: ["a.txt"], side: "yours" });
});

test("accept_right_without_changes_does_not_ask", async () => {
  mockCommands();
  await open();

  await fireEvent.click(button("Accept Right"));

  await waitFor(() => expect(done()).toEqual(["theirs"]));
  expect(calls("conflicts_accept")[0][1]).toEqual({ paths: ["a.txt"], side: "theirs" });
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("cancel_without_changes_closes_without_asking", async () => {
  mockCommands();
  await open();

  await fireEvent.click(button("Cancel"));

  await waitFor(() => expect(done()).toEqual(["cancelled"]));
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("cancel_with_changes_asks_cancel_merge", async () => {
  mockCommands();
  await open();
  edit();

  await fireEvent.click(button("Cancel"));
  const dialog = await screen.findByRole("dialog", { name: "Cancel Merge" });
  expect(dialog.textContent).toContain(
    "There are unsaved changes in the result file. Discard changes and cancel merge anyway?",
  );
  await answer("Cancel Merge", "Continue Merge");
  expect(done()).toEqual([]);

  await fireEvent.click(button("Cancel"));
  await answer("Cancel Merge", "Discard Changes and Cancel Merge");

  await waitFor(() => expect(done()).toEqual(["cancelled"]));
});

test("closing_the_window_is_cancelling", async () => {
  mockCommands();
  await open();
  edit();

  deliver("merge-close");
  await answer("Cancel Merge", "Discard Changes and Cancel Merge");

  await waitFor(() => expect(done()).toEqual(["cancelled"]));
});

test("file_changed_outside_shows_conflict_is_not_valid_and_does_not_write", async () => {
  let hash = "h";
  mockCommands({ merge_working_file: () => ({ hash, separator: "\n", conflicted: true }) });
  await open();
  deliver("repo-changed");
  await Promise.resolve();
  expect(screen.queryByText("Conflict is not valid and no longer can be resolved.")).toBeNull();

  hash = "other";
  deliver("repo-changed");

  await screen.findByText("Conflict is not valid and no longer can be resolved.");
  expect((button("Apply") as HTMLButtonElement).disabled).toBe(true);
  await fireEvent.keyDown(window, { key: "Enter", metaKey: true });
  expect(calls("conflict_resolve")).toHaveLength(0);
  edit();
  await fireEvent.click(button("Abort Resolve"));
  await waitFor(() => expect(done()).toEqual(["cancelled"]));
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("with_conflict_is_not_valid_accept_left_and_right_do_not_overwrite_the_file", async () => {
  let hash = "h";
  mockCommands({ merge_working_file: () => ({ hash, separator: "\n", conflicted: true }) });
  await open();

  hash = "other";
  deliver("repo-changed");
  await screen.findByText("Conflict is not valid and no longer can be resolved.");

  for (const name of ["Accept Left", "Accept Right"]) {
    expect((button(name) as HTMLButtonElement).disabled, name).toBe(true);
    await fireEvent.click(button(name));
  }
  // Not even by force: the command does not go out.
  for (const side of ["left", "right"] as const) {
    (button(side === "left" ? "Accept Left" : "Accept Right") as HTMLButtonElement).disabled = false;
    await fireEvent.click(button(side === "left" ? "Accept Left" : "Accept Right"));
  }
  await new Promise((resolve) => setTimeout(resolve, 20));
  expect(calls("conflicts_accept")).toHaveLength(0);
  expect(done()).toEqual([]);
});

test("if_apply_arrives_late_the_changed_file_is_not_written_and_the_window_stays", async () => {
  mockCommands({
    conflict_sides: task(SAME),
    conflict_resolve: task(null, { kind: "outdated", text: "changed" }),
  });
  await open();

  await fireEvent.click(button("Apply"));

  await screen.findByText("Conflict is not valid and no longer can be resolved.");
  expect((button("Apply") as HTMLButtonElement).disabled).toBe(true);
  expect(done()).toEqual([]);
});

test("binary_only_offers_accept_left_right_and_cancel", async () => {
  mockCommands({
    conflict_sides: task({ ...SIDES, yours: { kind: "binary" }, blocks: [] }),
  });
  await open();

  const names = [...footer().querySelectorAll("button")].map((b) => b.textContent);
  expect(names).toEqual(["Accept Left", "Accept Right", "Cancel"]);
  expect(document.querySelector(".rm-merge")).toBeNull();
  expect(screen.getByText("Your version")).not.toBeNull();

  await fireEvent.click(button("Cancel"));
  await waitFor(() => expect(done()).toEqual(["cancelled"]));
});

test("everything_processed_shows_save_changes_and_finish_merging", async () => {
  mockCommands({ conflict_sides: task(NON_CONFLICTING) });
  await open();
  expect(screen.queryByText("All changes have been processed.")).toBeNull();

  await fireEvent.click(button("All"));

  await screen.findByText("All changes have been processed.");
  await fireEvent.click(button("Save changes and finish merging"));
  await waitFor(() => expect(done()).toEqual(["resolved"]));
  expect((calls("conflict_resolve")[0][1] as { content: string }).content).toBe("a\nX\nc\nD\n");
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("the_top_bar_applies_the_non_conflicting_and_counts_what_is_pending", async () => {
  mockCommands({ conflict_sides: task(NON_CONFLICTING) });
  await open();
  expect(screen.getByText("Apply non-conflicting changes:")).not.toBeNull();
  expect(screen.getByText("2 changes. No conflicts.")).not.toBeNull();
  expect((button("Resolve Automatically") as HTMLButtonElement).disabled).toBe(false);

  await fireEvent.click(button("Left"));

  expect(centre().state.doc.toString()).toBe("a\nX\nc\nd\n");
  await screen.findByText("1 change. No conflicts.");
  expect((button("Left") as HTMLButtonElement).disabled).toBe(true);
  expect((button("Right") as HTMLButtonElement).disabled).toBe(false);
});

test("apply_the_non_conflicting_on_opening_if_the_setting_is_on", async () => {
  mockCommands({
    conflict_sides: task(NON_CONFLICTING),
    settings_get: () => ({ ...SETTINGS, mergeAutoApply: true }),
  });
  await open();

  await waitFor(() => expect(centre().state.doc.toString()).toBe("a\nX\nc\nD\n"));
  // What is applied on opening is a change of the result, like in IntelliJ.
  await fireEvent.click(button("Cancel"));
  await screen.findByRole("dialog", { name: "Cancel Merge" });
});

test("without_the_setting_the_result_starts_with_the_base", async () => {
  mockCommands({ conflict_sides: task(NON_CONFLICTING) });
  await open();

  expect(centre().state.doc.toString()).toBe("a\nb\nc\nd\n");
});

test("changing_whitespace_with_changes_asks_update_highlighting_settings", async () => {
  mockCommands({ merge_blocks: () => [CONFLICT] });
  await open();
  edit();

  await fireEvent.click(button("Do not ignore"));
  await fireEvent.click(await screen.findByRole("menuitem", { name: "Ignore whitespaces" }));
  const dialog = await screen.findByRole("dialog", { name: "Update Highlighting Settings" });
  expect(dialog.textContent).toContain(
    "Changing highlighting requires the file merge restart. Discard unsaved changes and restart merge anyway?",
  );
  await answer("Update Highlighting Settings", "Continue Merge");
  expect(calls("merge_blocks")).toHaveLength(0);
  expect(button("Do not ignore")).not.toBeNull();

  await fireEvent.click(button("Do not ignore"));
  await fireEvent.click(await screen.findByRole("menuitem", { name: "Ignore whitespaces" }));
  await answer("Update Highlighting Settings", "Discard Changes and Restart Merge");

  await waitFor(() => expect(calls("settings_save")).toHaveLength(1));
  expect(calls("merge_blocks")[0][1]).toMatchObject({ policy: "ignore" });
  expect((calls("settings_save")[0][1] as { settings: Settings }).settings.mergeWhitespace).toBe(
    "ignore",
  );
  expect(centre().state.doc.toString()).toBe("a\nb\nc\n");
  expect(button("Ignore whitespaces")).not.toBeNull();
});

test("changing_whitespace_without_changes_does_not_ask", async () => {
  mockCommands({ merge_blocks: () => [CONFLICT] });
  await open();

  await fireEvent.click(button("Do not ignore"));
  await fireEvent.click(await screen.findByRole("menuitem", { name: "Trim whitespaces" }));

  await waitFor(() => expect(calls("merge_blocks")).toHaveLength(1));
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("the_remembered_policy_recalculates_the_blocks_on_opening", async () => {
  mockCommands({ settings_get: () => ({ ...SETTINGS, mergeWhitespace: "trim" }) });
  await open();

  expect(calls("merge_blocks")[0][1]).toEqual({
    left: "a\nX\nc\n",
    base: "a\nb\nc\n",
    right: "a\nY\nc\n",
    policy: "trim",
  });
  expect(button("Trim whitespaces")).not.toBeNull();
});

// R6: the editor works with LF, and Apply writes with the line break of the working file.
test("crlf_breaks_are_read_as_lf_and_written_like_the_file", async () => {
  const crlf = text("a\r\nb\r\n");
  mockCommands({
    conflict_sides: task({ ...SAME, base: crlf, yours: crlf, theirs: text("a\rb\r") }),
    merge_working_file: () => ({ hash: "h", separator: "\r\n", conflicted: true }),
  });
  await open();

  expect(calls("merge_blocks")[0][1]).toMatchObject({ left: "a\nb\n", base: "a\nb\n", right: "a\nb\n" });
  expect(centre().state.doc.toString()).toBe("a\nb\n");
  edit("x\r\n");
  await fireEvent.click(button("Apply"));

  await waitFor(() => expect(done()).toEqual(["resolved"]));
  expect((calls("conflict_resolve")[0][1] as { content: string }).content).toBe("x\r\na\r\nb\r\n");
});

test("the_result_is_written_with_a_single_kind_of_break", () => {
  expect(withSeparator("a\nb\r\nc\r", "\n")).toBe("a\nb\nc");
  expect(withSeparator("a\nb\n", "\r")).toBe("a\rb\r");
});

test("file_resolved_outside_shows_conflict_is_not_valid", async () => {
  let conflicted = true;
  mockCommands({ merge_working_file: () => ({ hash: "h", separator: "\n", conflicted }) });
  await open();

  conflicted = false;
  deliver("repo-changed");

  await screen.findByText("Conflict is not valid and no longer can be resolved.");
  expect((button("Apply") as HTMLButtonElement).disabled).toBe(true);
  await fireEvent.click(button("Abort Resolve"));
  await waitFor(() => expect(done()).toEqual(["cancelled"]));
});

const left = () => calls("merge_left").map(([, args]) => (args as { closed: boolean }).closed);

test("opening_another_repository_with_a_half_done_merge_asks", async () => {
  mockCommands();
  await open();
  edit();

  deliver("merge-leave");
  // The acknowledgment arrives before asking: the backend moves to the long deadline for the answer.
  await waitFor(() => expect(calls("merge_leave_received")).toHaveLength(1));
  expect(left()).toEqual([]);
  await answer("Cancel Merge", "Continue Merge");
  await waitFor(() => expect(left()).toEqual([false]));

  deliver("merge-leave");
  await answer("Cancel Merge", "Discard Changes and Cancel Merge");
  await waitFor(() => expect(left()).toEqual([false, true]));
  // The window is destroyed by `repo_open`, which is no longer of this repository.
  expect(done()).toEqual([]);
});

test("opening_another_repository_without_changes_closes_without_asking", async () => {
  mockCommands();
  await open();

  deliver("merge-leave");

  await waitFor(() => expect(left()).toEqual([true]));
  expect(calls("merge_leave_received")).toHaveLength(1);
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("the_page_announces_it_is_already_listening_before_loading", async () => {
  mockCommands();
  await open();

  const order = vi.mocked(invoke).mock.calls.map(([name]) => name);
  expect(order.indexOf("merge_window_ready")).toBeGreaterThanOrEqual(0);
  expect(order.indexOf("merge_window_ready")).toBeLessThan(order.indexOf("conflict_sides"));
});

test("right_click_on_the_gutter_offers_soft_wrap_and_stores_it_for_merge", async () => {
  localStorage.clear();
  mockCommands({ conflict_sides: task(SAME) });
  await open();
  await waitFor(() => expect(document.querySelectorAll(".rm-merge .cm-gutters")).toHaveLength(3));
  const views = () =>
    [...document.querySelectorAll<HTMLElement>(".rm-merge .cm-editor")].map(
      (dom) => EditorView.findFromDOM(dom) as EditorView,
    );
  const before = views();
  // The place `Merge` starts off, as in IntelliJ.
  expect(before.map(wrapped)).toEqual([false, false, false]);

  const gutter = document.querySelector<HTMLElement>('[data-pane="right"] .cm-gutters .cm-gutterElement');
  await fireEvent.contextMenu(gutter as HTMLElement);
  const menu = await screen.findByRole("menu");
  expect(within(menu).getAllByRole("menuitem").map((item) => item.getAttribute("aria-label"))).toEqual([
    "Soft-Wrap",
  ]);
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Soft-Wrap" }));

  expect(views()).toEqual(before);
  expect(before.map(wrapped)).toEqual([true, true, true]);
  expect(localStorage.getItem("diff.wrap.Merge")).toBe("true");
  expect(localStorage.getItem("diff.wrap.Default")).toBeNull();

  // The text has no menu of its own.
  await fireEvent.contextMenu(document.querySelector('[data-pane="base"] .cm-line') as HTMLElement);
  expect(screen.queryByRole("menu")).toBeNull();
});

test("the_merge_window_opens_with_the_wrap_of_its_place", async () => {
  localStorage.clear();
  localStorage.setItem("diff.wrap.Merge", "true");
  mockCommands({ conflict_sides: task(SAME) });
  await open();
  await waitFor(() => expect(document.querySelectorAll(".rm-merge .cm-editor")).toHaveLength(3));

  const views = [...document.querySelectorAll<HTMLElement>(".rm-merge .cm-editor")].map(
    (dom) => EditorView.findFromDOM(dom) as EditorView,
  );
  expect(views.map(wrapped)).toEqual([true, true, true]);
});

// Contents mode: three texts given by whoever opened it (a patch that did not apply), without a
// file in conflict.
const CONTENTS = {
  id: "7",
  title: "Merge Revisions for /repo/a.txt",
  titles: ["Local changes", "Result", "Patched version"],
  left: "a\r\nX\r\nc\r\n",
  base: "a\nb\nc\n",
  right: "a\nY\nc\n",
  path: "a.txt",
};

function mockContents(commands: Record<string, (args: never) => unknown> = {}) {
  const refused = (name: string) => () => {
    throw new Error(`${name} in contents mode`);
  };
  mockCommands({
    merge_window_contents: () => CONTENTS,
    merge_window_path: () => null,
    merge_blocks: () => [CONFLICT],
    conflict_sides: refused("conflict_sides"),
    merge_working_file: refused("merge_working_file"),
    conflict_resolve: refused("conflict_resolve"),
    conflicts_accept: refused("conflicts_accept"),
    ...commands,
  });
}

const doneArgs = () => calls("merge_window_done").map(([, args]) => args);

test("contents_mode_saves_into_the_done_event_and_never_calls_conflict_resolve", async () => {
  mockContents();
  await open();
  edit();
  // `check()` is not for this mode: there is no working file to compare.
  deliver("repo-changed");

  await fireEvent.click(button("Apply"));
  await answer("Apply Changes", "Apply Changes and Mark Resolved");

  await waitFor(() => expect(doneArgs()).toEqual([{ result: "resolved", content: "za\nb\nc\n" }]));
  expect(calls("conflict_resolve")).toHaveLength(0);
  expect(calls("conflict_sides")).toHaveLength(0);
  expect(calls("merge_working_file")).toHaveLength(0);
  expect(screen.queryByText("Conflict is not valid and no longer can be resolved.")).toBeNull();
  // The blocks are those of the given texts, with their line breaks normalised.
  expect(calls("merge_blocks")[0][1]).toEqual({
    left: "a\nX\nc\n",
    base: "a\nb\nc\n",
    right: "a\nY\nc\n",
    policy: "default",
  });
});

test("contents_mode_shows_the_given_titles", async () => {
  mockContents();
  await open();

  const titles = [...document.querySelectorAll(".rm-merge-title")].map((title) => title.textContent);
  expect(titles).toEqual(CONTENTS.titles);
  expect(centre().state.doc.toString()).toBe("a\nb\nc\n");
});

test("contents_mode_accepts_a_side_as_its_text_without_touching_the_index", async () => {
  mockContents();
  await open();

  await fireEvent.click(button("Accept Left"));

  await waitFor(() => expect(doneArgs()).toEqual([{ result: "yours", content: "a\nX\nc\n" }]));
  expect(calls("conflicts_accept")).toHaveLength(0);
});

test("contents_mode_cancel_returns_no_text", async () => {
  mockContents();
  await open();

  await fireEvent.click(button("Cancel"));

  await waitFor(() => expect(doneArgs()).toEqual([{ result: "cancelled" }]));
});

// A patch conflict (`ApplyPatchAction.applyContent`): Cancel is `Abort…` (V:381), and it, or closing
// the window, asks `Abort Patch` (V:382-386) instead of the discard question. `Continue Resolve`
// keeps the window and its edits.
test("abort_asks_abort_and_rollback_skip_or_continue_resolve", async () => {
  mockCommands({
    merge_window_contents: () => ({ ...CONTENTS, abort: true }),
    merge_window_path: () => null,
    merge_blocks: () => [CONFLICT],
  });
  await open();
  edit();

  expect(screen.queryByRole("button", { name: "Cancel" })).toBeNull();
  await fireEvent.click(button("Abort…"));
  const question = await screen.findByRole("dialog", { name: "Abort Patch" });
  expect(within(question).getByText("Abort and Rollback applying patch or Skip this file?")).toBeTruthy();
  expect(within(question).getAllByRole("button").map((item) => item.textContent)).toEqual([
    "Continue Resolve",
    "Skip",
    "Abort and Rollback",
  ]);
  await fireEvent.click(within(question).getByRole("button", { name: "Continue Resolve" }));
  expect(screen.queryByRole("dialog", { name: "Abort Patch" })).toBeNull();
  expect(doneArgs()).toEqual([]);
  expect(centre().state.doc.toString()).toBe("za\nb\nc\n");

  deliver("merge-close");
  await answer("Abort Patch", "Skip");
  await waitFor(() => expect(doneArgs()).toEqual([{ result: "skip" }]));
});

test("abort_and_rollback_closes_the_patch_merge_as_aborted", async () => {
  mockCommands({
    merge_window_contents: () => ({ ...CONTENTS, abort: true }),
    merge_window_path: () => null,
    merge_blocks: () => [CONFLICT],
  });
  await open();

  await fireEvent.click(button("Abort…"));
  await answer("Abort Patch", "Abort and Rollback");

  await waitFor(() => expect(doneArgs()).toEqual([{ result: "abort" }]));
});

const HEAD = "1".repeat(40);
const BASE = "2".repeat(40);
const PICKED = "3".repeat(40);
const [ONE, TWO, THREE] = ["a", "b", "c"].map((letter) => letter.repeat(40));

// A cherry-pick: the local commits since the merge base on the left, the picked one on the right.
const PICKING: ConflictSides = {
  ...SIDES,
  titles: ["Local Changes", "Result", "Changes from cherry-pick 33333333"],
  details: [
    { kind: "range", from: BASE, to: HEAD },
    { kind: "commit", hash: PICKED },
  ],
};

const RANGE: RangeCommit[] = [
  { hash: ONE, subject: "touches b", conflicted: false },
  { hash: TWO, subject: "touches a", conflicted: true },
  { hash: THREE, subject: "touches c", conflicted: false },
];

const changed = (hash: string, path: string) => ({
  hash,
  group: {
    parent: null,
    subject: null,
    base: "f".repeat(40),
    merge: false,
    files: [
      { path: "other.txt", oldPath: null, status: "M" },
      { path, oldPath: null, status: "M" },
    ],
    size: 2,
  },
});

function mockDetails(sides: ConflictSides) {
  vi.mocked(invoke).mockClear();
  mockCommands({
    conflict_sides: task(sides),
    merge_range_commits: task(RANGE),
    // One commit's changes, and those of several.
    commit_changes_of: (args: { hash: string }) => [changed(args.hash, "a.txt").group],
    commit_changes_start: (args: { hashes: string[] }) =>
      task({ commits: args.hashes.map((hash) => changed(hash, "a.txt")), maxSize: 2 })(),
    commit_details: (args: { hashes: string[] }) =>
      args.hashes.map((hash) => ({
        hash,
        subject: `subject ${hash[0]}`,
        body: "body",
        author: "Ana",
        authorEmail: "ana@example.com",
        authorTime: 100,
        committer: "Ana",
        committerEmail: "ana@example.com",
        commitTime: 100,
        samePerson: true,
        refs: [],
        parents: [],
        links: {},
      })),
    task_cancel: () => undefined,
    file_sides: () => ({
      old: { kind: "text", content: "a\n" },
      new: { kind: "text", content: "b\n" },
    }),
  });
}

const titleTexts = () =>
  [...document.querySelectorAll(".rm-merge-title")].map((title) => [
    title.querySelector(".rm-merge-title-text")?.textContent ?? title.textContent,
    title.querySelector(".rm-merge-title-link")?.textContent ?? null,
  ]);

test("show_details_appears_only_in_merge_rebase_and_cherry_pick", async () => {
  mockDetails(PICKING);
  await open();

  // At the right of the left and the right title, not the result's.
  expect(titleTexts()).toEqual([
    ["Local Changes", "Show Details"],
    ["Result", null],
    ["Changes from cherry-pick 33333333", "Show Details"],
  ]);
  cleanup();

  // Without what the operation gives (a revert, an unstash, a patch), the titles alone.
  mockDetails(SIDES);
  await open();
  expect(titleTexts().map(([, link]) => link)).toEqual([null, null, null]);
  cleanup();

  // A binary file has them too.
  const binary = { kind: "binary" as const };
  mockDetails({ ...PICKING, yours: binary, theirs: binary, base: binary, blocks: [] });
  await open();
  expect(screen.getAllByRole("button", { name: "Show Details" })).toHaveLength(2);
});

test("a_single_commit_shows_its_changes_with_the_conflicted_file_and_its_message", async () => {
  mockDetails(PICKING);
  await open();

  await fireEvent.click(document.querySelectorAll<HTMLElement>(".rm-merge-title-link")[1]);

  const dialog = await screen.findByRole("dialog", { name: "Changes from cherry-pick 33333333" });
  await waitFor(() => expect(calls("commit_changes_of")).toHaveLength(1));
  expect(calls("commit_changes_of")[0][1]).toEqual({ hash: PICKED, mode: "merged" });
  const message = within(dialog).getByLabelText("Commit message");
  await waitFor(() => expect(message.textContent).toBe("subject 3\n\nbody"));
  const chosen = await within(dialog).findByRole("treeitem", { name: "a.txt", selected: true });
  expect(chosen).toBeTruthy();
  // Keys belong to the dialog: ⌘⏎ does not apply the merge underneath.
  await fireEvent.keyDown(window, { key: "Enter", metaKey: true });
  expect(calls("conflict_resolve")).toHaveLength(0);

  await fireEvent.click(within(dialog).getByRole("button", { name: "Close" }));
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("a_range_side_lists_its_commits_filtered_by_the_conflicted_file", async () => {
  mockDetails(PICKING);
  await open();

  await fireEvent.click(document.querySelectorAll<HTMLElement>(".rm-merge-title-link")[0]);

  const dialog = await screen.findByRole("dialog", { name: "Local Changes" });
  expect(calls("merge_range_commits")[0][1]).toEqual({ from: BASE, to: HEAD, path: "a.txt" });
  const list = await within(dialog).findByRole("listbox", { name: "Commits" });
  const subjects = () => within(list).getAllByRole("option").map((option) => option.textContent?.trim());
  const filter = within(dialog).getByRole("checkbox", { name: "Filter by conflicted file" });
  expect((filter as HTMLInputElement).checked).toBe(true);
  // Only the commits that touched the file, the first one selected.
  expect(subjects()).toEqual(["touches a"]);
  await waitFor(() => expect(calls("commit_changes_of").at(-1)?.[1]).toMatchObject({ hash: TWO }));

  // Without the filter every commit shows, and the selection stays.
  await fireEvent.click(filter);
  expect(subjects()).toEqual(["touches b", "touches a", "touches c"]);
  const selected = () =>
    within(list)
      .getAllByRole("option")
      .filter((option) => option.getAttribute("aria-selected") === "true")
      .map((option) => option.textContent?.trim());
  expect(selected()).toEqual(["touches a"]);

  // Several selected: their changes together.
  await fireEvent.click(within(list).getByRole("option", { name: "touches c" }), { metaKey: true });
  await waitFor(() =>
    expect(calls("commit_changes_start").at(-1)?.[1]).toMatchObject({ hashes: [TWO, THREE] }),
  );

  // Filtering again keeps what still shows.
  await fireEvent.click(filter);
  expect(selected()).toEqual(["touches a"]);

  await fireEvent.click(within(dialog).getByRole("button", { name: "OK" }));
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("the_range_list_has_a_speed_search_and_a_double_click_shows_the_diff", async () => {
  mockDetails(PICKING);
  await open();
  await fireEvent.click(document.querySelectorAll<HTMLElement>(".rm-merge-title-link")[0]);
  const dialog = await screen.findByRole("dialog", { name: "Local Changes" });
  const filter = await within(dialog).findByRole("checkbox", { name: "Filter by conflicted file" });
  await fireEvent.click(filter);
  const list = within(dialog).getByRole("listbox", { name: "Commits" });
  const selected = () =>
    within(list)
      .getAllByRole("option")
      .filter((option) => option.getAttribute("aria-selected") === "true")
      .map((option) => option.textContent?.trim());

  // Typing goes to the first subject that matches, and the arrows go between the matches.
  for (const key of "touches c") await fireEvent.keyDown(list, { key });
  expect(within(list).getByRole("status").textContent).toBe("touches c");
  expect(selected()).toEqual(["touches c"]);
  await fireEvent.keyDown(list, { key: "Backspace" });
  await fireEvent.keyDown(list, { key: "ArrowUp" });
  expect(selected()).toEqual(["touches a"]);
  await fireEvent.keyDown(list, { key: "Escape" });
  expect(within(list).queryByRole("status")).toBeNull();
  expect(screen.getByRole("dialog", { name: "Local Changes" })).toBeTruthy();

  // A double click on a change opens its diff over the dialog.
  const change = await within(dialog).findByRole("treeitem", { name: "a.txt" });
  await fireEvent.dblClick(change);
  const diff = await screen.findByRole("dialog", { name: "a.txt" });
  await waitFor(() => expect(calls("file_sides")).toHaveLength(1));
  await fireEvent.click(within(diff).getByRole("button", { name: "Close" }));
  expect(screen.queryByRole("dialog", { name: "a.txt" })).toBeNull();
  expect(screen.getByRole("dialog", { name: "Local Changes" })).toBeTruthy();
});
