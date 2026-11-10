import { EditorView } from "@codemirror/view";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import type { DiffRequest, Hunk, RevisionPair, RevisionSides, WorktreeSides } from "../lib/api";
import DiffWindow from "./DiffWindow.svelte";

// What `EditorView.lineWrapping` puts on the content; `view.lineWrapping` only follows it after a
// measure, which jsdom does not do.
const wrapped = (view: EditorView) => view.contentDOM.classList.contains("cm-lineWrapping");

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(), emit: vi.fn() }));

const HUNK: Hunk = { id: "h1", oldStart: 1, oldLines: 1, newStart: 1, newLines: 1 };
const OTHER: Hunk = { id: "h2", oldStart: 3, oldLines: 1, newStart: 3, newLines: 1 };

const SIDES: WorktreeSides = {
  old: { kind: "text", content: "a\nb\nc\nd\n" },
  new: { kind: "text", content: "a\nB\nc\nD\n" },
  hunks: [HUNK, OTHER],
  editable: true,
  note: null,
  hash: "hash",
  conflicted: false,
  submodule: null,
};

const STATUS = [
  { path: "a.txt", oldPath: null, state: "modified", staged: false, unstaged: true },
  { path: "b.txt", oldPath: null, state: "modified", staged: false, unstaged: true },
];

// jsdom does no layout: CodeMirror measures with this.
Range.prototype.getClientRects = () => [] as unknown as DOMRectList;

Element.prototype.scrollIntoView = () => {};

// Everything near what is visible: in the combined view each file has its editor.
vi.stubGlobal(
  "IntersectionObserver",
  class {
    constructor(private readonly notify: (entries: unknown[]) => void) {}
    observe(target: Element) {
      queueMicrotask(() =>
        this.notify([{ target, isIntersecting: true, boundingClientRect: { height: 100 } }]),
      );
    }
    unobserve() {}
    disconnect() {}
  },
);

const handlers = new Map<string, (payload: unknown) => void>();

function deliver(event: string, payload?: unknown) {
  handlers.get(event)?.(payload);
}

function mockCommands(commands: Record<string, (args: never) => unknown> = {}) {
  const all: Record<string, (args: never) => unknown> = {
    diff_window_request: () => ({ kind: "worktree", path: "a.txt" }),
    repo_status: () => STATUS,
    worktree_sides: () => SIDES,
    show_window: () => undefined,
    settings_get: () => ({ nonModalCommit: false, stagingArea: false }),
    perf_enabled: () => false,
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

function emitted(event: string) {
  return vi.mocked(emit).mock.calls.filter(([name]) => name === event).map(([, payload]) => payload);
}

beforeEach(() => {
  localStorage.clear();
  handlers.clear();
  vi.mocked(invoke).mockReset();
  vi.mocked(emit).mockReset();
  vi.mocked(emit).mockResolvedValue(undefined);
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
  const view = render(DiffWindow);
  await waitFor(() => expect(document.querySelector(".rm-editor .cm-line")).not.toBeNull());
  await waitFor(() => expect(handlers.has("selection-lost")).toBe(true));
  return view;
}

const checks = () => [...document.querySelectorAll<HTMLElement>(".rm-check")];

function clickCheck(check: HTMLElement) {
  check.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
}

function selectionState(
  included: string[],
  selections: Record<string, unknown> = {},
  owner: "none" | "commit-view" | "dialog" = "commit-view",
) {
  deliver("selection-state", { owner, included, selections, lists: null });
}

test("requests_what_is_chosen_and_is_shown_when_done", async () => {
  mockCommands();

  await open();

  await waitFor(() => expect(emitted("diff-ready")).toHaveLength(1));
  await waitFor(() => expect(calls("show_window")).toHaveLength(1));
});

test("in_measurement_mode_reports_its_timings_when_the_diff_is_drawn", async () => {
  mockCommands({ perf_enabled: () => true });

  await open();

  await waitFor(() => expect(emitted("diff-drawn")).toHaveLength(1));
  expect(emitted("diff-drawn")[0]).toEqual({
    sidesMs: expect.any(Number),
    buildMs: expect.any(Number),
    builtAtEpochMs: expect.any(Number),
    paintMs: expect.any(Number),
  });
});

test("outside_measurement_mode_nothing_is_measured", async () => {
  mockCommands();

  await open();
  await new Promise((resolve) => setTimeout(resolve, 100));

  expect(emitted("diff-drawn")).toHaveLength(0);
});

test("without_frames_the_drawn_notice_arrives_without_paint_time", async () => {
  const frames = vi.spyOn(window, "requestAnimationFrame").mockReturnValue(0);
  const hidden = vi.spyOn(document, "visibilityState", "get").mockReturnValue("hidden");
  const unfocused = vi.spyOn(document, "hasFocus").mockReturnValue(false);
  try {
    mockCommands({ perf_enabled: () => true });
    render(DiffWindow);

    await waitFor(() => expect(emitted("diff-drawn")).toHaveLength(1), { timeout: 4_000 });
    expect(emitted("diff-drawn")[0]).toMatchObject({ paintMs: null });
  } finally {
    frames.mockRestore();
    hidden.mockRestore();
    unfocused.mockRestore();
  }
}, 6_000);

const PARENT_HEAD = "c".repeat(40);

test("a_submodule_in_local_changes_is_titled_by_the_parent_head_and_your_version", async () => {
  mockCommands({
    worktree_sides: () => ({
      ...SIDES,
      old: { kind: "text", content: "a".repeat(40) },
      new: { kind: "text", content: "b".repeat(40) },
      hunks: [],
      editable: false,
      hash: null,
      submodule: PARENT_HEAD,
    }),
  });
  render(DiffWindow);
  await waitFor(() => expect(document.querySelector(".titles")).not.toBeNull());

  await waitFor(() =>
    expect(
      [...document.querySelectorAll(".titles .title > span:first-of-type")].map((title) =>
        title.textContent?.trim(),
      ),
    ).toEqual([PARENT_HEAD, "Your version"]),
  );
});

test("a_submodule_without_a_parent_commit_has_the_default_left_title", async () => {
  mockCommands({
    worktree_sides: () => ({
      ...SIDES,
      old: { kind: "absent" },
      new: { kind: "text", content: "b".repeat(40) },
      hunks: [],
      editable: false,
      hash: null,
      submodule: "",
    }),
  });
  render(DiffWindow);

  await waitFor(() =>
    expect(document.querySelector(".titles .title > span:first-of-type")?.textContent?.trim()).toBe(
      "Base version",
    ),
  );
});

test("the_align_setting_changes_the_option", async () => {
  mockCommands();
  await open();
  expect(JSON.parse(localStorage.getItem("diff.options") ?? "{}").align).toBeFalsy();

  await fireEvent.click(screen.getByRole("button", { name: "Settings" }));
  await fireEvent.click(await screen.findByRole("menuitem", { name: /Align Changes Highlighting/ }));

  expect(JSON.parse(localStorage.getItem("diff.options") ?? "{}").align).toBe(true);
});

// Both sides of the file shown.
const shownViews = () =>
  [...document.querySelectorAll<HTMLElement>(".rm-editors .cm-editor")].map(
    (dom) => EditorView.findFromDOM(dom) as EditorView,
  );

const menuLabels = (menu: HTMLElement) =>
  [...menu.querySelectorAll('[role="menuitem"]')].map((item) => item.getAttribute("aria-label"));

const ticked = (name: string) =>
  screen.getByRole("menuitem", { name }).querySelector(".tick")?.textContent === "✓";

test("the_gear_has_soft_wrap_before_align", async () => {
  mockCommands();
  await open();
  const before = shownViews();
  expect(before).toHaveLength(2);
  // The user's place `Default` has it on.
  expect(before.map(wrapped)).toEqual([true, true]);

  await fireEvent.click(screen.getByRole("button", { name: "Settings" }));
  const menu = await screen.findByRole("menu");
  expect(menuLabels(menu)).toEqual(["Soft-Wrap", "Align Changes Highlighting"]);
  expect(ticked("Soft-Wrap")).toBe(true);
  await fireEvent.click(screen.getByRole("menuitem", { name: "Soft-Wrap" }));

  expect(shownViews()).toEqual(before);
  expect(before.map(wrapped)).toEqual([false, false]);
  expect(localStorage.getItem("diff.wrap.Default")).toBe("false");
  expect(JSON.parse(localStorage.getItem("diff.options") ?? "{}")).not.toHaveProperty("wrap");

  await fireEvent.click(screen.getByRole("button", { name: "Settings" }));
  expect(ticked("Soft-Wrap")).toBe(false);
  await fireEvent.click(screen.getByRole("menuitem", { name: "Soft-Wrap" }));

  expect(shownViews()).toEqual(before);
  expect(before.map(wrapped)).toEqual([true, true]);
  expect(localStorage.getItem("diff.wrap.Default")).toBe("true");
});

test("right_click_on_the_gutter_offers_soft_wrap", async () => {
  mockCommands();
  await open();
  const before = shownViews();

  const number = document.querySelector<HTMLElement>(".cm-gutters .cm-gutterElement + .cm-gutterElement");
  await fireEvent.contextMenu(number as HTMLElement);

  const menu = await screen.findByRole("menu");
  expect(menuLabels(menu)).toEqual(["Soft-Wrap"]);
  expect(ticked("Soft-Wrap")).toBe(true);
  await fireEvent.click(screen.getByRole("menuitem", { name: "Soft-Wrap" }));

  expect(shownViews()).toEqual(before);
  expect(before.map(wrapped)).toEqual([false, false]);
  expect(localStorage.getItem("diff.wrap.Default")).toBe("false");

  // The text keeps the menu it had.
  await fireEvent.contextMenu(document.querySelector(".rm-editor .cm-line") as HTMLElement);
  expect(menuLabels(await screen.findByRole("menu"))).toEqual([
    "Rollback Lines",
    "Move Lines to Another Changelist...",
    "Show Diff for Lines",
  ]);
});

test("an_included_file_has_all_its_hunks_checked", async () => {
  mockCommands();
  await open();

  selectionState(["a.txt"]);

  await waitFor(() =>
    expect(checks().map((check) => check.getAttribute("aria-checked"))).toEqual(["true", "true"]),
  );
  expect(screen.getByRole("checkbox", { name: "Include into commit", checked: true })).toBeTruthy();
});

test("unchecking_a_hunk_chooses_the_others_and_reports_it_to_the_commit_view", async () => {
  mockCommands();
  await open();
  selectionState(["a.txt"]);
  await waitFor(() => expect(checks()[0].getAttribute("aria-checked")).toBe("true"));

  clickCheck(checks()[0]);

  await waitFor(() => expect(emitted("selection-changed")).toHaveLength(1));
  expect(emitted("selection-changed")[0]).toEqual({
    path: "a.txt",
    selection: [{ hunk: "h2", lines: null }],
    included: true,
    places: { h2: "line 4" },
  });
  await waitFor(() => expect(checks()[0].getAttribute("aria-checked")).toBe("false"));
});

test("with_nothing_checked_the_file_leaves_the_commit", async () => {
  mockCommands();
  await open();
  selectionState(["a.txt"], { "a.txt": [{ hunk: "h1", lines: null }] });
  await waitFor(() => expect(checks()[0].getAttribute("aria-checked")).toBe("true"));

  clickCheck(checks()[0]);

  await waitFor(() => expect(emitted("selection-changed")).toHaveLength(1));
  expect(emitted("selection-changed")[0]).toMatchObject({ selection: [], included: false });
});

describe("who_owns_the_line_checkboxes", () => {
  const titleCheck = () => screen.queryByRole("checkbox", { name: "Include into commit" });

  test("without a commit session the diff shows no line checkboxes", async () => {
    mockCommands();
    await open();

    selectionState(["a.txt"], {}, "none");
    await new Promise((resolve) => setTimeout(resolve, 50));

    expect(checks()).toHaveLength(0);
    expect(titleCheck()).toBeNull();
  });

  test("before the application says who owns them there are no line checkboxes", async () => {
    mockCommands();
    await open();
    await new Promise((resolve) => setTimeout(resolve, 50));

    expect(checks()).toHaveLength(0);
    expect(titleCheck()).toBeNull();
  });

  test("line checkboxes in the window change the inclusion of the open dialog", async () => {
    mockCommands();
    await open();
    selectionState(["a.txt"], {}, "dialog");
    await waitFor(() => expect(checks()[0].getAttribute("aria-checked")).toBe("true"));

    clickCheck(checks()[0]);

    await waitFor(() => expect(emitted("selection-changed")).toHaveLength(1));
    expect(emitted("selection-changed")[0]).toEqual({
      path: "a.txt",
      selection: [{ hunk: "h2", lines: null }],
      included: true,
      places: { h2: "line 4" },
    });
  });

  test("closing the dialog clears the window's checkboxes", async () => {
    mockCommands();
    await open();
    selectionState(["a.txt"], { "a.txt": [{ hunk: "h1", lines: null }] }, "dialog");
    await waitFor(() => expect(checks()).toHaveLength(2));
    expect(titleCheck()).not.toBeNull();

    selectionState([], {}, "none");

    await waitFor(() => expect(checks()).toHaveLength(0));
    expect(titleCheck()).toBeNull();
  });

  test("in non-modal mode the commit view still owns the line checkboxes", async () => {
    mockCommands();
    await open();

    selectionState(["a.txt"], {}, "commit-view");

    await waitFor(() => expect(checks()).toHaveLength(2));
    expect(titleCheck()).not.toBeNull();
    clickCheck(checks()[1]);
    await waitFor(() => expect(emitted("selection-changed")).toHaveLength(1));
  });
});

test("hunks_of_another_list_have_no_checkbox", async () => {
  mockCommands();
  await open();

  deliver("selection-state", {
    owner: "commit-view",
    included: ["a.txt"],
    selections: {},
    lists: {
      lists: [],
      active: "default",
      assignments: { "a.txt": "default" },
      hunkAssignments: { "a.txt": { h1: "default", h2: "other" } },
    },
  });

  await waitFor(() => expect(checks()).toHaveLength(1));
  expect(checks()[0].dataset.hunk).toBe("h1");
});

test("reports_the_lost_selection", async () => {
  mockCommands();
  await open();

  deliver("selection-lost", { path: "a.txt", hunks: ["h1"], text: "\"a.txt\" has changed: x" });

  expect(await screen.findByText("\"a.txt\" has changed: x")).toBeTruthy();
});

test("rereads_the_file_when_it_changes_on_disk", async () => {
  mockCommands();
  await open();
  const before = calls("worktree_sides").length;

  deliver("repo-changed");

  await waitFor(() => expect(calls("worktree_sides").length).toBeGreaterThan(before));
});

test("a_binary_file_says_why_it_is_not_compared", async () => {
  mockCommands({
    worktree_sides: () => ({ ...SIDES, old: { kind: "binary" }, new: { kind: "binary" }, hunks: [] }),
  });
  render(DiffWindow);

  expect(await screen.findByText("Binary file")).toBeTruthy();
  expect(document.querySelector(".cm-editor")).toBeNull();
});

test("explains_why_the_local_side_is_read_only", async () => {
  mockCommands({ worktree_sides: () => ({ ...SIDES, editable: false, note: "Read-only: x" }) });
  await open();

  expect(screen.getByText("Read-only: x")).toBeTruthy();
});

test("the_arrows_switch_files_with_the_shortcut", async () => {
  mockCommands({ diff_window_open: () => undefined });
  await open();

  await fireEvent.keyDown(window, { key: "ArrowRight", ctrlKey: true, shiftKey: true });

  await waitFor(() => expect(calls("diff_window_open")).toHaveLength(1));
  expect(calls("diff_window_open")[0][1]).toEqual({ path: "b.txt" });

  deliver("diff-open", "b.txt");
  expect(await screen.findByText("b.txt")).toBeTruthy();
});

test("escape_hides_the_window", async () => {
  mockCommands({ hide_window: () => undefined });
  await open();

  await fireEvent.keyDown(window, { key: "Escape" });

  await waitFor(() => expect(calls("hide_window")).toHaveLength(1));
});

// One of the two editors of the file that is shown: 0 is the left one.
function sideView(index: number): EditorView {
  const contents = document.querySelectorAll<HTMLElement>(".rm-editors .cm-content");
  return EditorView.findFromDOM(contents[index]) as EditorView;
}

// The local side of the file that is shown, or the one of `index` in the combined view.
function localView(index = 0): EditorView {
  const contents = [...document.querySelectorAll<HTMLElement>(".rm-editors")].map(
    (editors) => editors.querySelectorAll<HTMLElement>(".cm-content")[1],
  );
  return EditorView.findFromDOM(contents[index]) as EditorView;
}

function type(text: string, index = 0) {
  localView(index).dispatch({ changes: { from: 0, insert: text }, userEvent: "input.type" });
}

const shownText = (index = 0) => localView(index).state.sliceDoc();
const save = () => fireEvent.keyDown(window, { key: "s", metaKey: true });
const writes = () => calls("file_write").map(([, args]) => args);
const written = (path: string, content: string, expectedHash: string) => ({
  path,
  content,
  expectedHash,
});

function deferred<T>() {
  let resolve: (value: T) => void = () => {};
  let reject: (reason: unknown) => void = () => {};
  const promise = new Promise<T>((done, failed) => {
    resolve = done;
    reject = failed;
  });
  return { promise, resolve, reject };
}

const CHANGED = { kind: "failed", message: "\"a.txt\" has changed; nothing was written" };

describe("with_unsaved_changes", () => {
  test("switching_files_saves_before_removing_the_editor", async () => {
    mockCommands({ file_write: () => "new", diff_window_open: () => undefined });
    await open();
    type("X");

    deliver("diff-open", "b.txt");

    await waitFor(() => expect(writes()).toHaveLength(1));
    expect(writes()[0]).toEqual(written("a.txt", "Xa\nB\nc\nD\n", "hash"));
    expect(await screen.findByText("b.txt")).toBeTruthy();
  });

  test("if_it_cannot_be_saved_the_file_is_not_switched_and_the_text_is_kept", async () => {
    mockCommands({
      file_write: () => {
        throw CHANGED;
      },
      diff_window_open: () => undefined,
    });
    await open();
    type("X");

    deliver("diff-open", "b.txt");

    expect(await screen.findByText(CHANGED.message)).toBeTruthy();
    expect(shownText()).toBe("Xa\nB\nc\nD\n");
    expect(document.querySelector(".path")?.textContent).toBe("a.txt");
    // The title and the path stored in the backend go back to the file that is still open.
    await waitFor(() => expect(calls("diff_window_open").at(-1)?.[1]).toEqual({ path: "a.txt" }));
  });

  test("after_discarding_what_was_typed_the_file_can_be_switched", async () => {
    let current = SIDES;
    mockCommands({
      worktree_sides: () => current,
      file_write: () => {
        throw CHANGED;
      },
      diff_window_open: () => undefined,
    });
    await open();
    type("X");
    current = { ...SIDES, hash: "another" };
    deliver("repo-changed");
    await save();
    await screen.findByText(CHANGED.message);

    await fireEvent.click(await screen.findByRole("button", { name: /Reload and Discard/ }));
    await waitFor(() => expect(document.querySelector(".titles")?.textContent).not.toContain("*"));
    deliver("diff-open", "b.txt");

    await waitFor(() => expect(document.querySelector(".path")?.textContent).toBe("b.txt"));
  });

  test("switching_to_the_combined_view_saves_first", async () => {
    mockCommands({ file_write: () => "new" });
    await open();
    type("X");

    await fireEvent.click(screen.getByRole("button", { name: "Show Combined Diff" }));

    await waitFor(() => expect(writes()).toHaveLength(1));
    await waitFor(() => expect(document.querySelectorAll("section")).toHaveLength(2));
  });

  test("if_it_cannot_be_saved_the_combined_view_is_not_entered", async () => {
    mockCommands({
      file_write: () => {
        throw CHANGED;
      },
    });
    await open();
    type("X");

    await fireEvent.click(screen.getByRole("button", { name: "Show Combined Diff" }));

    expect(await screen.findByText(CHANGED.message)).toBeTruthy();
    expect(document.querySelectorAll("section")).toHaveLength(0);
    expect(shownText()).toBe("Xa\nB\nc\nD\n");
  });

  test("leaving_the_combined_view_saves_everything", async () => {
    mockCommands({ file_write: () => "new" });
    await open();
    await fireEvent.click(screen.getByRole("button", { name: "Show Combined Diff" }));
    await waitFor(() => expect(document.querySelectorAll(".rm-editors")).toHaveLength(2));
    type("X", 0);
    type("Y", 1);

    await fireEvent.click(screen.getByRole("button", { name: "Show Combined Diff" }));

    await waitFor(() => expect(writes()).toHaveLength(2));
    await waitFor(() => expect(document.querySelectorAll("section")).toHaveLength(0));
  });

  test("in_the_combined_view_saving_saves_all_the_files", async () => {
    mockCommands({ file_write: () => "new" });
    await open();
    await fireEvent.click(screen.getByRole("button", { name: "Show Combined Diff" }));
    await waitFor(() => expect(document.querySelectorAll(".rm-editors")).toHaveLength(2));
    type("X", 0);
    type("Y", 1);

    await save();

    await waitFor(() => expect(writes()).toHaveLength(2));
  });

  test("what_is_typed_in_another_pane_during_a_save_is_also_saved", async () => {
    const second = deferred<string>();
    mockCommands({
      file_write: (args: { path: string; content: string }) =>
        args.path === "b.txt" && !args.content.startsWith("Z") ? second.promise : "new",
    });
    await open();
    await fireEvent.click(screen.getByRole("button", { name: "Show Combined Diff" }));
    await waitFor(() => expect(document.querySelectorAll(".rm-editors")).toHaveLength(2));
    type("X", 0);
    type("Y", 1);

    await fireEvent.click(screen.getByRole("button", { name: "Show Combined Diff" }));
    await waitFor(() => expect(writes()).toHaveLength(2));
    type("Z", 0);
    second.resolve("new");

    await waitFor(() => expect(writes()).toHaveLength(3));
    expect(writes()[2]).toMatchObject({ path: "a.txt", content: "ZXa\nB\nc\nD\n" });
    await waitFor(() => expect(document.querySelectorAll("section")).toHaveLength(0));
  });

  test("if_something_always_remains_unsaved_it_does_not_switch_and_says_so", async () => {
    mockCommands({
      file_write: () => {
        type("Z");
        return "new";
      },
      diff_window_open: () => undefined,
    });
    await open();
    type("X");

    deliver("diff-open", "b.txt");

    expect(await screen.findByText(/keeps changing while it is being saved/)).toBeTruthy();
    expect(document.querySelector(".path")?.textContent).toBe("a.txt");
    expect(writes().length).toBeLessThan(20);
  });

  test("if_a_file_leaves_the_list_it_is_saved_before_removing_it", async () => {
    let status = STATUS;
    mockCommands({ repo_status: () => status, file_write: () => "new" });
    await open();
    await fireEvent.click(screen.getByRole("button", { name: "Show Combined Diff" }));
    await waitFor(() => expect(document.querySelectorAll(".rm-editors")).toHaveLength(2));
    type("Y", 1);

    status = [STATUS[0]];
    deliver("repo-changed");

    await waitFor(() => expect(writes()).toHaveLength(1));
    expect(writes()[0]).toMatchObject({ path: "b.txt", content: "Ya\nB\nc\nD\n" });
  });

  test("if_the_file_leaving_the_list_cannot_be_saved_it_stays", async () => {
    let status = STATUS;
    mockCommands({
      repo_status: () => status,
      file_write: () => {
        throw CHANGED;
      },
    });
    await open();
    await fireEvent.click(screen.getByRole("button", { name: "Show Combined Diff" }));
    await waitFor(() => expect(document.querySelectorAll(".rm-editors")).toHaveLength(2));
    type("Y", 1);

    status = [STATUS[0]];
    deliver("repo-changed");

    expect(await screen.findByText(CHANGED.message)).toBeTruthy();
    expect(document.querySelectorAll("section")).toHaveLength(2);
    expect(shownText(1)).toBe("Ya\nB\nc\nD\n");
  });

  test.each([
    ["escape", () => fireEvent.keyDown(window, { key: "Escape" })],
    ["el_boton_de_cerrar", () => deliver("diff-close")],
  ])("closing_with_%s_saves_and_hides", async (_name, close) => {
    mockCommands({ file_write: () => "new", hide_window: () => undefined });
    await open();
    type("X");

    await close();

    await waitFor(() => expect(calls("hide_window")).toHaveLength(1));
    expect(writes()).toHaveLength(1);
  });

  test("if_it_cannot_be_saved_on_closing_the_window_stays_open", async () => {
    mockCommands({
      file_write: () => {
        throw CHANGED;
      },
      hide_window: () => undefined,
    });
    await open();
    type("X");

    deliver("diff-close");

    expect(await screen.findByText(CHANGED.message)).toBeTruthy();
    expect(calls("hide_window")).toHaveLength(0);
  });

  function loseFocus() {
    window.dispatchEvent(new Event("blur"));
  }

  test("losing_focus_saves_what_was_typed", async () => {
    mockCommands({ file_write: () => "new" });
    await open();
    type("X");

    loseFocus();

    await waitFor(() => expect(writes()).toHaveLength(1));
    expect(writes()[0]).toEqual(written("a.txt", "Xa\nB\nc\nD\n", "hash"));
  });

  test("losing_focus_with_nothing_typed_does_not_save", async () => {
    mockCommands();
    await open();

    loseFocus();
    await new Promise((done) => setTimeout(done, 20));

    expect(writes()).toHaveLength(0);
  });

  test("losing_focus_saves_all_the_files_of_the_combined_view", async () => {
    mockCommands({ file_write: () => "new" });
    await open();
    await fireEvent.click(screen.getByRole("button", { name: "Show Combined Diff" }));
    await waitFor(() => expect(document.querySelectorAll(".rm-editors")).toHaveLength(2));
    type("X", 0);
    type("Y", 1);

    loseFocus();

    await waitFor(() => expect(writes()).toHaveLength(2));
  });

  test("if_it_cannot_be_saved_on_losing_focus_the_text_stays_and_it_is_not_retried", async () => {
    mockCommands({
      file_write: () => {
        throw CHANGED;
      },
    });
    await open();
    type("X");

    loseFocus();

    expect(await screen.findByText(CHANGED.message)).toBeTruthy();
    await new Promise((done) => setTimeout(done, 20));
    expect(writes()).toHaveLength(1);
    expect(shownText()).toBe("Xa\nB\nc\nD\n");
    expect(document.querySelector(".titles")?.textContent).toContain("*");
  });

  // Rollback from the main window: what was typed is saved, goes into the snapshot and stops being
  // unsaved, and the diff shows what the rollback leaves.
  test("after_saving_for_an_operation_the_diff_follows_the_disk", async () => {
    let sides = SIDES;
    mockCommands({
      file_write: () => "new",
      diff_flushed: () => undefined,
      worktree_sides: () => sides,
    });
    await open();
    type("X");

    deliver("diff-flush");
    await waitFor(() => expect(calls("diff_flushed")[0]?.[1]).toEqual({ saved: true }));
    sides = { ...SIDES, new: SIDES.old, hunks: [], hash: "reverted" };
    deliver("repo-changed");

    await waitFor(() => expect(shownText()).toBe("a\nb\nc\nd\n"));
    expect(document.querySelector(".titles")?.textContent).not.toContain("*");
  });

  test("opening_another_repository_asks_to_save_first", async () => {
    const saved = deferred<string>();
    mockCommands({ file_write: () => saved.promise, diff_flushed: () => undefined });
    await open();
    type("X");

    deliver("diff-flush");

    await waitFor(() => expect(writes()).toHaveLength(1));
    expect(calls("diff_flushed")).toHaveLength(0);
    saved.resolve("new");
    await waitFor(() => expect(calls("diff_flushed")).toHaveLength(1));
    expect(calls("diff_flushed")[0][1]).toEqual({ saved: true });
  });

  test("if_it_cannot_be_saved_the_other_repository_is_not_opened", async () => {
    mockCommands({
      file_write: () => {
        throw CHANGED;
      },
      diff_flushed: () => undefined,
    });
    await open();
    type("X");

    deliver("diff-flush");

    await waitFor(() => expect(calls("diff_flushed")[0]?.[1]).toEqual({ saved: false }));
    expect(screen.getByText(CHANGED.message)).toBeTruthy();
  });

  test("what_is_typed_during_the_save_is_not_lost", async () => {
    const first = deferred<string>();
    let sent = 0;
    mockCommands({ file_write: () => (sent++ === 0 ? first.promise : "second") });
    await open();
    type("X");

    await save();
    await waitFor(() => expect(writes()).toHaveLength(1));
    type("Y");
    const reads = calls("worktree_sides").length;
    first.resolve("first");

    await waitFor(() => expect(document.querySelector(".titles")?.textContent).toContain("*"));
    expect(shownText()).toBe("YXa\nB\nc\nD\n");
    expect(calls("worktree_sides")).toHaveLength(reads);

    await save();

    await waitFor(() => expect(writes()).toHaveLength(2));
    expect(writes()[1]).toEqual(written("a.txt", "YXa\nB\nc\nD\n", "first"));
  });

  test("saving_twice_in_a_row_is_not_a_conflict", async () => {
    const first = deferred<string>();
    mockCommands({
      file_write: (args: { expectedHash: string }) => {
        if (args.expectedHash !== "hash") throw CHANGED;
        return first.promise;
      },
    });
    await open();
    type("X");

    await save();
    await save();
    first.resolve("new");

    await waitFor(() => expect(document.querySelector(".titles")?.textContent).not.toContain("*"));
    expect(writes()).toHaveLength(1);
    expect(screen.queryByText(CHANGED.message)).toBeNull();
  });

  test("saving_before_the_preview_does_not_bring_back_the_earlier_text", async () => {
    const after = { ...SIDES, new: { kind: "text", content: "Xa\nB\nc\nD\n" }, hash: "new" };
    const refreshed = deferred<WorktreeSides>();
    let saved = false;
    mockCommands({
      file_write: () => {
        saved = true;
        return "new";
      },
      worktree_preview: () => [HUNK, OTHER],
      worktree_sides: () => (saved ? refreshed.promise : SIDES),
    });
    await open();
    type("X");

    await save();
    await waitFor(() => expect(writes()).toHaveLength(1));
    await new Promise((resolve) => setTimeout(resolve, 200));

    expect(shownText()).toBe("Xa\nB\nc\nD\n");
    refreshed.resolve(after as WorktreeSides);
    await waitFor(() => expect(document.querySelector(".titles")?.textContent).not.toContain("*"));
    expect(shownText()).toBe("Xa\nB\nc\nD\n");
  });
});

describe("when_the_file_changes_on_disk", () => {
  async function reloadWith(sides: Partial<WorktreeSides>, first: Partial<WorktreeSides> = {}) {
    let current = { ...SIDES, ...first };
    mockCommands({ worktree_sides: () => current });
    await open();
    current = { ...current, ...sides };
    deliver("repo-changed");
  }

  const CRLF = "a\r\nb\r\nc\r\nd\r\n";

  test("with_mixed_endings_the_lines_are_still_those_of_git", async () => {
    const mixed = "a\r\nB\nc\r\nd\r\n";
    await reloadWith(
      { new: { kind: "text", content: mixed } },
      { new: { kind: "text", content: CRLF } },
    );

    await waitFor(() => expect(localView().state.doc.lines).toBe(4 + 1));
    await waitFor(() => expect(localView().state.lineBreak).toBe("\n"));
    expect(localView().state.sliceDoc()).toBe(mixed);
  });

  test("from_crlf_to_lf_it_does_not_write_crlf_again_when_saving", async () => {
    await reloadWith(
      { new: { kind: "text", content: "a\nb\n" } },
      { new: { kind: "text", content: CRLF } },
    );

    await waitFor(() => expect(localView().state.sliceDoc()).toBe("a\nb\n"));
  });

  test("if_it_becomes_read_only_the_editor_stops_editing", async () => {
    await reloadWith({ editable: false, note: "Read-only: x" });

    await screen.findByText("Read-only: x");
    await waitFor(() => expect(localView().state.readOnly).toBe(true));
  });

  test("with_unsaved_changes_the_editor_is_kept_and_reports", async () => {
    let current = SIDES;
    mockCommands({ worktree_sides: () => current });
    await open();
    type("X");
    current = { ...SIDES, hash: "another", new: { kind: "text", content: "a\r\nb\n" } };

    deliver("repo-changed");

    expect(await screen.findByText(/unsaved/)).toBeTruthy();
    expect(shownText()).toBe("Xa\nB\nc\nD\n");
  });
});

test("moving_the_only_chosen_hunk_removes_it_from_the_selection", async () => {
  const LISTS = {
    lists: [
      { id: "default", name: "Changes", comment: "" },
      { id: "other", name: "Other", comment: "" },
    ],
    active: "default",
    assignments: { "a.txt": "default" },
    hunkAssignments: {},
  };
  mockCommands({ changelists_move_hunks: () => LISTS });
  await open();
  deliver("selection-state", {
    owner: "commit-view",
    included: ["a.txt"],
    selections: { "a.txt": [{ hunk: "h1", lines: null }] },
    lists: LISTS,
  });
  await waitFor(() => expect(checks()[0].getAttribute("aria-checked")).toBe("true"));
  const line = [...document.querySelectorAll<HTMLElement>(".rm-editor:last-child .cm-line")].find(
    (item) => item.textContent === "B",
  ) as HTMLElement;
  localView().dispatch({ selection: { anchor: 2 } });

  await fireEvent.keyDown(window, { key: "M", metaKey: true, shiftKey: true });
  await fireEvent.click(await screen.findByRole("button", { name: "OK" }));

  await waitFor(() => expect(emitted("selection-changed")).toHaveLength(1));
  expect(line).toBeTruthy();
  expect(emitted("selection-changed")[0]).toMatchObject({
    path: "a.txt",
    selection: [],
    included: false,
  });
});

test("a_file_not_included_checks_nothing_even_if_a_selection_remains", async () => {
  mockCommands();
  await open();

  selectionState([], { "a.txt": [{ hunk: "h1", lines: null }] });

  await waitFor(() => expect(checks().length).toBeGreaterThan(0));
  await waitFor(() =>
    expect(checks().map((check) => check.getAttribute("aria-checked"))).toEqual(["false", "false"]),
  );
});

describe("with_staging_area", () => {
  const STAGED: WorktreeSides = { ...SIDES, editable: false, hash: null, note: "In the stage" };

  function mockStaging(commands: Record<string, (args: never) => unknown> = {}) {
    mockCommands({
      settings_get: () => ({ nonModalCommit: true, stagingArea: true }),
      staging_sides: (args: { comparison: string }) =>
        args.comparison === "staged" ? STAGED : SIDES,
      staging_stage_hunks: () => null,
      staging_unstage_hunks: () => null,
      ...commands,
    });
  }

  async function openStaged() {
    await open();
    deliver("diff-comparison", "staged");
    await waitFor(() => expect(screen.getByText("In the stage")).toBeTruthy());
  }

  test("compares_the_stage_with_the_disk_and_not_with_head", async () => {
    mockStaging();

    await open();

    expect(calls("staging_sides")[0][1]).toEqual({ path: "a.txt", comparison: "local" });
    expect(calls("worktree_sides")).toHaveLength(0);
    expect(screen.getByText("Staged")).toBeTruthy();
    expect(screen.getByText("Local")).toBeTruthy();
  });

  test("opening_from_staged_compares_head_with_the_stage", async () => {
    mockStaging();

    await openStaged();

    expect(screen.getByText("HEAD")).toBeTruthy();
    expect(screen.getByText("Staged")).toBeTruthy();
    expect(screen.queryByText("Local")).toBeNull();
    await waitFor(() =>
      expect(checks().map((check) => check.getAttribute("aria-checked"))).toEqual(["true", "true"]),
    );
  });

  test("the_comparison_is_chosen_in_the_toolbar", async () => {
    mockStaging();
    await open();

    await fireEvent.click(screen.getByRole("button", { name: /Staged and Local/ }));
    await fireEvent.click(await screen.findByRole("menuitem", { name: "HEAD and Staged" }));

    await waitFor(() => expect(screen.getByText("In the stage")).toBeTruthy());
  });

  test("the_margin_of_the_stage_stays_when_nobody_owns_the_lines", async () => {
    mockStaging();
    await open();

    selectionState([], {}, "none");

    await waitFor(() => expect(checks()).toHaveLength(2));
  });

  test("the_margin_checkbox_moves_the_hunk_to_the_stage", async () => {
    mockStaging();
    await open();

    clickCheck(checks()[1]);

    await waitFor(() =>
      expect(calls("staging_stage_hunks")).toEqual([
        ["staging_stage_hunks", { path: "a.txt", selection: [{ hunk: "h2", lines: null }] }],
      ]),
    );
    await waitFor(() => expect(emitted("repo-changed")).toHaveLength(1));
    expect(emitted("selection-changed")).toHaveLength(0);
  });

  test("the_margin_checkbox_takes_the_hunk_out_of_the_stage", async () => {
    mockStaging();
    await openStaged();
    await waitFor(() => expect(checks()[0].getAttribute("aria-checked")).toBe("true"));

    clickCheck(checks()[0]);

    await waitFor(() =>
      expect(calls("staging_unstage_hunks")).toEqual([
        ["staging_unstage_hunks", { path: "a.txt", selection: [{ hunk: "h1", lines: null }] }],
      ]),
    );
    expect(calls("staging_stage_hunks")).toHaveLength(0);
  });

  test("the_top_checkbox_acts_on_all_the_hunks", async () => {
    mockStaging();
    await open();

    await fireEvent.click(screen.getByRole("checkbox", { name: "Stage" }));

    await waitFor(() =>
      expect(calls("staging_stage_hunks")[0][1]).toEqual({
        path: "a.txt",
        selection: [
          { hunk: "h1", lines: null },
          { hunk: "h2", lines: null },
        ],
      }),
    );
  });

  test("a_hunk_that_no_longer_exists_reports_and_rereads", async () => {
    mockStaging({
      staging_stage_hunks: () => {
        throw {
          kind: "selection_lost",
          message: "lost",
          details: { path: "a.txt", hunks: ["h1"] },
        };
      },
    });
    await open();
    const reads = calls("staging_sides").length;

    clickCheck(checks()[0]);

    expect((await screen.findByRole("status")).textContent).toContain("so the stage was not touched");
    await waitFor(() => expect(calls("staging_sides").length).toBeGreaterThan(reads));
  });

  test("if_a_snapshot_of_the_stage_was_saved_it_can_be_restored", async () => {
    mockStaging({ staging_stage_hunks: () => "snapshot", snapshots_restore: () => "previous" });
    await open();

    clickCheck(checks()[0]);
    await fireEvent.click(await screen.findByRole("button", { name: "Restore" }));

    await waitFor(() =>
      expect(calls("snapshots_restore")).toEqual([["snapshots_restore", { id: "snapshot" }]]),
    );
  });

  test("undoing_lines_is_not_offered_because_its_hunks_are_others", async () => {
    mockStaging();
    await open();

    await fireEvent.keyDown(window, { key: "z", code: "KeyZ", metaKey: true, altKey: true });

    expect(calls("lines_rollback")).toHaveLength(0);
  });

  test("what_was_typed_is_compared_with_the_stage", async () => {
    mockStaging({ staging_preview: () => [HUNK] });
    await open();

    type("x");

    await waitFor(() => expect(calls("staging_preview")).toHaveLength(1));
    expect(calls("worktree_preview")).toHaveLength(0);
  });

  test("a_file_in_conflict_is_shown_without_stage_actions", async () => {
    const conflicted: WorktreeSides = { ...SIDES, editable: false, conflicted: true, note: "With conflict" };
    mockStaging({ staging_sides: () => conflicted });
    render(DiffWindow);

    await screen.findByText("With conflict");

    await waitFor(() => expect(document.querySelector(".rm-editors")).not.toBeNull());
    expect(screen.getByText("HEAD")).toBeTruthy();
    expect(screen.getByText("Your version")).toBeTruthy();
    expect(document.querySelector(".rm-check")).toBeNull();
    expect(screen.queryByRole("checkbox")).toBeNull();
  });

  describe("with_the_three_versions", () => {
    const THREE = {
      head: "a\nb\n",
      staged: "a\nB\n",
      local: "a\nB\nc\n",
      stagedHash: "sh",
      localHash: "lh",
    };

    function mockThree(commands: Record<string, (args: never) => unknown> = {}) {
      mockStaging({
        staging_three_sides: () => THREE,
        merge_blocks: () => [],
        index_write: () => ({ hash: "new", copy: null }),
        diff_window_open: () => undefined,
        diff_flushed: () => undefined,
        ...commands,
      });
    }

    function stagedView(): EditorView {
      const content = document.querySelector<HTMLElement>('[data-pane="base"] .cm-content');
      return EditorView.findFromDOM(content as HTMLElement) as EditorView;
    }

    async function openThree() {
      await open();
      deliver("diff-comparison", "three");
      await waitFor(() => expect(document.querySelector('[data-pane="base"] .cm-content')).not.toBeNull());
    }

    test("the_three_versions_comparison_opens_three_panes", async () => {
      mockThree();

      await openThree();

      expect(calls("staging_three_sides")[0][1]).toEqual({ path: "a.txt" });
      expect(stagedView().state.sliceDoc()).toBe(THREE.staged);
      expect(screen.getByRole("button", { name: /Compare HEAD, Staged and Local Versions/ })).toBeTruthy();
      expect(screen.getByRole("button", { name: "Show Combined Diff" })).toHaveProperty("disabled", true);
    });

    test("editing_staged_and_switching_files_saves_it", async () => {
      mockThree();
      await openThree();
      stagedView().dispatch({ changes: { from: 0, insert: "X" }, userEvent: "input.type" });

      deliver("diff-open", "b.txt");

      await waitFor(() =>
        expect(calls("index_write")).toEqual([
          ["index_write", { path: "a.txt", content: "Xa\nB\n", expectedHash: "sh" }],
        ]),
      );
      await waitFor(() => expect(calls("staging_three_sides").at(-1)?.[1]).toEqual({ path: "b.txt" }));
    });

    test("saving_for_an_operation_writes_the_stage", async () => {
      mockThree();
      await openThree();
      stagedView().dispatch({ changes: { from: 0, insert: "X" }, userEvent: "input.type" });

      deliver("diff-flush");

      await waitFor(() => expect(calls("diff_flushed")).toEqual([["diff_flushed", { saved: true }]]));
      expect(calls("index_write")).toHaveLength(1);
    });

    test("if_the_stage_cannot_be_saved_the_file_is_not_switched", async () => {
      mockThree({
        index_write: () => {
          throw CHANGED;
        },
      });
      await openThree();
      stagedView().dispatch({ changes: { from: 0, insert: "X" }, userEvent: "input.type" });

      deliver("diff-open", "b.txt");

      await waitFor(() => expect(calls("index_write")).toHaveLength(1));
      await waitFor(() => expect(calls("diff_window_open")).toEqual([["diff_window_open", { path: "a.txt" }]]));
      expect(stagedView().state.sliceDoc()).toBe("Xa\nB\n");
    });

    test("a_file_without_the_three_versions_is_compared_two_at_a_time", async () => {
      mockThree({
        staging_three_sides: () => {
          throw { kind: "no_three_sides", message: "without the three" };
        },
      });
      await open();

      deliver("diff-comparison", "three");

      await waitFor(() => expect(calls("staging_sides").length).toBeGreaterThan(1));
      expect(calls("staging_sides").at(-1)?.[1]).toEqual({ path: "a.txt", comparison: "local" });
      expect(document.querySelector('[data-pane="base"]')).toBeNull();
    });
  });

  test("turning_the_mode_off_compares_with_head_again", async () => {
    mockStaging();
    await open();

    deliver("settings-changed", { nonModalCommit: true, stagingArea: false });

    await waitFor(() => expect(calls("worktree_sides").length).toBeGreaterThan(0));
    await waitFor(() => expect(screen.getByText("Your version")).toBeTruthy());
    expect(screen.queryByRole("button", { name: /Staged and Local/ })).toBeNull();
  });
});

describe("with_a_request_of_two_revisions", () => {
  const LEFT = "a".repeat(40);
  const RIGHT = "b".repeat(40);
  const pair = (path: string, right: string | null = "local"): RevisionPair => ({
    left: { revision: LEFT, path, title: LEFT },
    right: { revision: right, path, title: right === "local" ? "Local" : RIGHT },
  });
  const REQUEST: DiffRequest = {
    kind: "revisions",
    files: [pair("x.txt"), pair("y.txt", RIGHT)],
    title: "x.txt (/repo)",
  };
  const REVISION_SIDES: RevisionSides = {
    ...SIDES,
    oldPreview: false,
    newPreview: false,
    identical: null,
  };

  function mockRevisions(commands: Record<string, (args: never) => unknown> = {}) {
    mockCommands({
      diff_window_request: () => REQUEST,
      repo_status: () => {
        throw new Error("a request of two revisions never reads the status");
      },
      worktree_sides: () => {
        throw new Error("a request of two revisions never reads HEAD and the disk");
      },
      compare_sides: () => REVISION_SIDES,
      compare_preview: () => [HUNK],
      diff_window_open_request: () => undefined,
      ...commands,
    });
  }

  const titles = () =>
    [...document.querySelectorAll(".titles .title > span:first-of-type")].map((title) =>
      title.textContent?.trim(),
    );

  // `ListSelection`'s index: the window opens on the pair asked for, and the same files asked for
  // again from another one of them only move to it.
  test("a_revisions_request_opens_on_its_selected_pair", async () => {
    mockRevisions({ diff_window_request: () => ({ ...REQUEST, selected: 1 }) });
    await open();

    expect(calls("compare_sides")[0][1]).toEqual(pair("y.txt", RIGHT));
    expect(document.querySelector(".count")?.textContent).toBe("2/2");

    deliver("diff-request", { ...REQUEST, selected: 0 });
    await waitFor(() => expect(document.querySelector(".count")?.textContent).toBe("1/2"));
  });

  // The re-review, Minor A: a revision whose object is missing is git's error in the window,
  // never an absent side, so there is no hunk to revert against the local file.
  test("a_missing_revision_shows_git_s_error_and_nothing_to_revert", async () => {
    mockRevisions({
      diff_window_request: () => ({ ...REQUEST, files: [pair("x.txt")] }),
      compare_sides: () => {
        throw { kind: "git", message: "fatal: not a tree object" };
      },
    });
    render(DiffWindow);

    expect((await screen.findByRole("alert")).textContent).toContain("fatal: not a tree object");
    expect(document.querySelector(".rm-editor .cm-line")).toBeNull();
    expect(checks()).toHaveLength(0);
    expect(writes()).toHaveLength(0);
  });

  // `GitSubmoduleDiffRequestProvider`: the full hash of each revision, else the default title.
  test("a_submodule_in_the_log_is_titled_by_the_full_hashes", async () => {
    const submodule = {
      left: { revision: LEFT, path: "libs/lib", title: "aaaaaaaa" },
      right: { revision: RIGHT, path: "libs/lib", title: "bbbbbbbb" },
    };
    mockRevisions({
      diff_window_request: () => ({ kind: "revisions", title: "lib (/repo/libs)", files: [submodule] }),
      compare_sides: () => ({ ...REVISION_SIDES, submodule: LEFT }),
    });
    await open();

    expect(titles()).toEqual([LEFT, RIGHT]);
  });

  test("a_submodule_against_the_local_side_has_your_version_on_the_right", async () => {
    const against = {
      left: { revision: LEFT, path: "libs/lib", title: LEFT },
      right: { revision: "local", path: "libs/lib", title: "Local" },
    };
    mockRevisions({
      diff_window_request: () => ({ kind: "revisions", title: "lib (/repo/libs)", files: [against] }),
      compare_sides: () => ({ ...REVISION_SIDES, submodule: LEFT }),
    });
    await open();

    expect(titles()).toEqual([LEFT, "Your version"]);
  });

  test("a_revisions_request_lists_only_its_files_and_never_reads_status", async () => {
    mockRevisions();
    await open();

    expect(calls("compare_sides")[0][1]).toEqual(pair("x.txt"));
    expect(calls("repo_status")).toHaveLength(0);
    expect(document.querySelector(".path")?.textContent).toBe("x.txt");
    expect(document.querySelector(".count")?.textContent).toBe("1/2");
    expect(titles()).toEqual([LEFT, "Local"]);
    // No line checkboxes, no stage and no combined view.
    expect(checks()).toHaveLength(0);
    expect(document.querySelector(".all")).toBeNull();
    expect(screen.getByRole("button", { name: "Show Combined Diff" }).hasAttribute("disabled")).toBe(
      true,
    );

    await fireEvent.click(screen.getByRole("button", { name: "Compare Next File" }));

    await waitFor(() => expect(calls("compare_sides").at(-1)?.[1]).toEqual(pair("y.txt", RIGHT)));
    await waitFor(() => expect(titles()).toEqual([LEFT, RIGHT]));
    expect(document.querySelector(".count")?.textContent).toBe("2/2");
    expect(calls("diff_window_open")).toHaveLength(0);
    expect(calls("repo_status")).toHaveLength(0);
  });

  test("repository changes reread the sides without reading the status", async () => {
    mockRevisions();
    await open();

    deliver("repo-changed");

    await waitFor(() => expect(calls("compare_sides").length).toBeGreaterThan(1));
    expect(calls("repo_status")).toHaveLength(0);
  });

  test("lines of two revisions cannot be rolled back or moved", async () => {
    mockRevisions();
    await open();
    localView().dispatch({ selection: { anchor: 2 } });
    const line = [...document.querySelectorAll<HTMLElement>(".rm-editor:last-child .cm-line")][1];

    await fireEvent.contextMenu(line);

    const items = menuLabels(await screen.findByRole("menu"));
    expect(items).toEqual(["Show Diff for Lines"]);
  });

  test("the_local_side_saves_with_the_hash_check_and_refuses_a_changed_disk", async () => {
    let current = REVISION_SIDES;
    let refuse = false;
    mockRevisions({
      compare_sides: () => current,
      file_write: (args: { content: string }) => {
        if (refuse) throw CHANGED;
        current = { ...current, hash: "saved", new: { kind: "text", content: args.content } };
        return "saved";
      },
    });
    await open();
    type("X");
    await waitFor(() => expect(calls("compare_preview")).toHaveLength(1));
    expect(calls("compare_preview")[0][1]).toEqual({
      ...pair("x.txt"),
      content: "Xa\nB\nc\nD\n",
    });

    await save();

    await waitFor(() => expect(writes()).toEqual([written("x.txt", "Xa\nB\nc\nD\n", "hash")]));

    // Devin writes the file: the next save is refused and the text stays.
    refuse = true;
    type("Y");
    current = { ...current, hash: "devin" };
    deliver("repo-changed");
    expect(await screen.findByText(/has changed on disk and there are unsaved changes/)).toBeTruthy();
    await save();

    expect(await screen.findByText(CHANGED.message)).toBeTruthy();
    expect(writes().at(-1)).toEqual(written("x.txt", "YXa\nB\nc\nD\n", "saved"));
    expect(shownText()).toBe("YXa\nB\nc\nD\n");
  });

  test("a local side on the left is edited and saved with the hash check", async () => {
    const swapped = {
      left: { revision: "local", path: "x.txt", title: `Local (${LEFT})` },
      right: { revision: RIGHT, path: "x.txt", title: RIGHT },
    };
    mockRevisions({
      diff_window_request: () => ({ kind: "revisions", title: "x.txt", files: [swapped] }),
      compare_sides: () => ({ ...REVISION_SIDES, editable: true, hash: "hash" }),
      file_write: () => "saved",
    });
    await open();
    const left = sideView(0);
    await waitFor(() => expect(left.state.readOnly).toBe(false));
    expect(localView().state.readOnly).toBe(true);

    left.dispatch({ changes: { from: 0, insert: "X" }, userEvent: "input.type" });

    await waitFor(() => expect(titles()[0]).toBe(`Local (${LEFT}) *`));
    await waitFor(() => expect(calls("compare_preview")).toHaveLength(1));
    expect(calls("compare_preview")[0][1]).toEqual({ ...swapped, content: "Xa\nb\nc\nd\n" });
    await save();
    await waitFor(() => expect(writes()).toEqual([written("x.txt", "Xa\nb\nc\nd\n", "hash")]));
  });

  describe("a_large_or_binary_revision_shows_the_241_texts", () => {
    async function shown(sides: Partial<RevisionSides>) {
      mockRevisions({ compare_sides: () => ({ ...REVISION_SIDES, ...sides }) });
      render(DiffWindow);
      await waitFor(() => expect(calls("compare_sides").length).toBeGreaterThan(0));
    }

    test("identical binaries", async () => {
      await shown({ old: { kind: "binary" }, new: { kind: "binary" }, identical: true });

      expect(await screen.findByText("Files contents are identical")).toBeTruthy();
      expect(document.querySelector(".rm-editor")).toBeNull();
    });

    test("different binaries", async () => {
      await shown({ old: { kind: "binary" }, new: { kind: "absent" }, identical: false });

      expect(await screen.findByText("Files contents are different")).toBeTruthy();
    });

    test("two previews", async () => {
      await shown({ oldPreview: true, newPreview: true, editable: false });

      expect(await screen.findByText("Files are too large to compare")).toBeTruthy();
      expect(document.querySelector(".rm-editor")).toBeNull();
    });

    test("one preview", async () => {
      await shown({ oldPreview: true, editable: false });

      expect(await screen.findByText("File is too large. Only preview is loaded.")).toBeTruthy();
      await waitFor(() => expect(document.querySelector(".rm-editor .cm-line")).not.toBeNull());
    });

    test("identical texts", async () => {
      await shown({ new: SIDES.old, hunks: [] });

      expect(await screen.findByText("Contents are identical")).toBeTruthy();
    });
  });

  test("a new request replaces the one shown, and a working-tree file goes back to the status", async () => {
    mockRevisions({ repo_status: () => STATUS, worktree_sides: () => SIDES });
    await open();

    deliver("diff-request", {
      kind: "revisions",
      title: "z.txt",
      files: [pair("z.txt")],
    });

    await waitFor(() => expect(document.querySelector(".path")?.textContent).toBe("z.txt"));
    expect(document.querySelector(".count")?.textContent).toBe("1/1");
    expect(calls("compare_sides").at(-1)?.[1]).toEqual(pair("z.txt"));

    deliver("diff-open", "b.txt");

    await waitFor(() => expect(calls("worktree_sides").at(-1)?.[1]).toEqual({ path: "b.txt" }));
    await waitFor(() => expect(document.querySelector(".count")?.textContent).toBe("2/2"));
  });

  describe("a local side on the left in the unified view", () => {
    const swapped = {
      left: { revision: "local", path: "x.txt", title: `Local (${LEFT})` },
      right: { revision: RIGHT, path: "x.txt", title: RIGHT },
    };

    function mockSwapped() {
      mockRevisions({
        diff_window_request: () => ({ kind: "revisions", title: "x.txt", files: [swapped] }),
        compare_sides: () => ({ ...REVISION_SIDES, editable: true, hash: "hash" }),
        file_write: () => "saved",
      });
    }

    const viewer = (name: string) =>
      fireEvent.click(screen.getByRole("button", { name: /viewer/ })).then(() =>
        fireEvent.click(screen.getByRole("menuitem", { name })),
      );

    test("is edited and saved with the hash check", async () => {
      localStorage.setItem("diff.options", JSON.stringify({ unified: true }));
      mockSwapped();
      await open();
      expect(document.querySelectorAll(".rm-editors .cm-content")).toHaveLength(1);
      const only = sideView(0);
      await waitFor(() => expect(only.state.readOnly).toBe(false));
      expect(only.state.sliceDoc()).toBe("a\nb\nc\nd\n");

      only.dispatch({ changes: { from: 0, insert: "X" }, userEvent: "input.type" });

      await waitFor(() => expect(titles()).toEqual([`Local (${LEFT}) *`]));
      await waitFor(() => expect(calls("compare_preview")).toHaveLength(1));
      await save();
      await waitFor(() =>
        expect(writes()).toEqual([written("x.txt", "Xa\nb\nc\nd\n", "hash")]),
      );
    });

    test("switching to it with unsaved text keeps the text on the local side", async () => {
      mockSwapped();
      await open();
      sideView(0).dispatch({ changes: { from: 0, insert: "X" }, userEvent: "input.type" });
      await waitFor(() => expect(titles()[0]).toBe(`Local (${LEFT}) *`));

      await viewer("Unified viewer");

      await waitFor(() =>
        expect(document.querySelectorAll(".rm-editors .cm-content")).toHaveLength(1),
      );
      expect(sideView(0).state.sliceDoc()).toBe("Xa\nb\nc\nd\n");
      expect(sideView(0).state.readOnly).toBe(false);
      expect(titles()).toEqual([`Local (${LEFT}) *`]);
      await save();
      await waitFor(() =>
        expect(writes()).toEqual([written("x.txt", "Xa\nb\nc\nd\n", "hash")]),
      );
    });
  });

  // The backend answers an open request by sending it back to the window, as commands.rs does.
  function echoing() {
    return {
      diff_window_open_request: (args: { request: DiffRequest }) => {
        const { request } = args;
        queueMicrotask(() =>
          request.kind === "worktree"
            ? deliver("diff-open", request.path)
            : deliver("diff-request", request),
        );
      },
      diff_window_open: (args: { path: string }) => {
        queueMicrotask(() => deliver("diff-open", args.path));
      },
    };
  }

  test("a refused request restores the one shown once, without resending it again", async () => {
    mockRevisions({
      ...echoing(),
      file_write: () => {
        throw CHANGED;
      },
    });
    await open();
    type("X");

    deliver("diff-request", { kind: "revisions", title: "z.txt", files: [pair("z.txt")] });
    await screen.findByText(CHANGED.message);
    await new Promise((resolve) => setTimeout(resolve, 300));

    expect(writes()).toHaveLength(1);
    expect(calls("diff_window_open_request")).toEqual([
      ["diff_window_open_request", { request: REQUEST }],
    ]);
    expect(document.querySelector(".path")?.textContent).toBe("x.txt");
  });

  test("a refused working-tree file restores the request shown once", async () => {
    mockRevisions({
      ...echoing(),
      file_write: () => {
        throw CHANGED;
      },
    });
    await open();
    type("X");

    deliver("diff-open", "b.txt");
    await screen.findByText(CHANGED.message);
    await new Promise((resolve) => setTimeout(resolve, 300));

    expect(writes()).toHaveLength(1);
    expect(calls("diff_window_open_request")).toHaveLength(1);
    expect(document.querySelector(".path")?.textContent).toBe("x.txt");
  });

  test("if what was typed cannot be saved the request that was shown stays", async () => {
    mockRevisions({
      file_write: () => {
        throw CHANGED;
      },
    });
    await open();
    type("X");

    deliver("diff-request", { kind: "revisions", title: "z.txt", files: [pair("z.txt")] });

    expect(await screen.findByText(CHANGED.message)).toBeTruthy();
    expect(document.querySelector(".path")?.textContent).toBe("x.txt");
    await waitFor(() =>
      expect(calls("diff_window_open_request").at(-1)?.[1]).toEqual({ request: REQUEST }),
    );
  });
});
