import { EditorView } from "@codemirror/view";
import { afterEach, expect, test, vi } from "vitest";
import type { Hunk } from "../lib/api";
import { DiffEditors, type DiffData, type Handlers } from "./editor";

const OPTIONS = { highlight: "words", collapse: false, align: true, sync: true } as const;

let editors: DiffEditors | null = null;

// jsdom does no layout: CodeMirror measures with this.
Range.prototype.getClientRects = () => [] as unknown as DOMRectList;

function open(data: DiffData, extra: { unified?: boolean; editable?: boolean } = {}) {
  const handlers: Handlers = { ontoggle: vi.fn(), onedit: vi.fn(), onfocus: vi.fn() };
  const parent = document.body.appendChild(document.createElement("div"));
  editors = new DiffEditors(parent, {
    path: "a.txt",
    data,
    draft: null,
    editable: extra.editable ?? true,
    unified: extra.unified ?? false,
    embedded: false,
    options: OPTIONS,
    handlers,
  });
  return { parent, handlers };
}

afterEach(() => {
  editors?.destroy();
  editors = null;
  document.body.textContent = "";
});

const hunk = (oldStart: number, oldLines: number, newStart: number, newLines: number): Hunk => ({
  id: `h${oldStart}`,
  oldStart,
  oldLines,
  newStart,
  newLines,
});

test("word_highlighting_marks_only_what_changes", () => {
  const { parent } = open(data("one two three\n", "one TWO three\n", [hunk(0, 1, 0, 1)]));

  const marks = [...parent.querySelectorAll('[class*="rm-word-"]')].map((mark) => mark.textContent);

  expect(marks).toContain("TWO");
  expect(marks).not.toContain("one TWO three");
});

const data = (oldText: string, newText: string, hunks: Hunk[]): DiffData => ({
  oldText,
  newText,
  hunks,
  selectable: hunks,
  selection: hunks.map((item) => ({ hunk: item.id, lines: null })),
});

function lineTexts(parent: HTMLElement, selector: string): string[] {
  return [...parent.querySelectorAll(selector)].map((line) => line.textContent ?? "");
}

// With repetitive text there are several minimal diffs: git's is painted, not another.
test("in_repetitive_text_the_hunk_lines_are_marked_and_no_others", () => {
  const before = "x\ny\nx\ny\nx\ny\n";
  const after = "x\ny\nx\ny\nx\ny\nx\ny\n";

  const { parent } = open(data(before, after, [hunk(2, 0, 2, 2)]));

  expect(lineTexts(parent, ".cm-line.rm-inserted")).toEqual(["x", "y"]);
  const lines = [...parent.querySelectorAll(".rm-editor:last-child .cm-line")];
  expect(lines.map((line) => line.classList.contains("rm-inserted"))).toEqual([
    false,
    false,
    true,
    true,
    false,
    false,
    false,
    false,
    false,
  ]);
});

test("each_aligned_side_has_the_gap_of_the_lines_it_lacks", () => {
  const { parent } = open(data("a\nb\n", "a\nnew\nother\nb\n", [hunk(1, 0, 1, 2)]));

  const spacers = [...parent.querySelectorAll<HTMLElement>(".rm-spacer")];

  expect(spacers.map((spacer) => spacer.style.height)).toEqual(["40px"]);
});

test("the_hunk_checkbox_is_on_its_line_of_the_local_side", () => {
  const { parent } = open(data("a\nb\n", "a\nc\n", [hunk(1, 1, 1, 1)]));

  const checks = parent.querySelectorAll<HTMLElement>(".rm-check");

  expect(checks).toHaveLength(1);
  expect(checks[0].dataset.hunk).toBe("h1");
  expect(checks[0].getAttribute("aria-checked")).toBe("true");
});

test("changing_the_selection_changes_the_checkboxes_without_rebuilding_the_editor", () => {
  const { parent } = open(data("a\nb\n", "a\nc\n", [hunk(1, 1, 1, 1)]));
  const dom = parent.querySelector(".cm-content");

  editors?.setSelection([{ hunk: "h1", lines: [1] }]);

  expect(parent.querySelector(".cm-content")).toBe(dom);
  const lines = [...parent.querySelectorAll<HTMLElement>(".rm-check")].map((check) => [
    check.dataset.line,
    check.getAttribute("aria-checked"),
  ]);
  expect(lines).toEqual([
    ["0", "false"],
    ["1", "true"],
  ]);
});

test("saving_a_crlf_file_keeps_its_line_endings", () => {
  const text = "one\r\ntwo\r\n";
  open(data("one\r\n", text, [hunk(1, 0, 1, 1)]));

  expect(editors?.content()).toBe(text);
  expect(editors?.dirty).toBe(false);
});

test("with_mixed_endings_the_lines_are_those_of_git", () => {
  const text = "one\r\ntwo\nthree\r\n";
  const { parent } = open(data(text, text, []));

  expect(editors?.content()).toBe(text);
  expect(parent.querySelectorAll(".rm-editor:last-child .cm-line")).toHaveLength(4);
});

test("a_read_only_file_is_not_edited", () => {
  const { parent } = open(data("a\n", "b\n", [hunk(0, 1, 0, 1)]), { editable: false });
  const content = parent.querySelector<HTMLElement>(".rm-editor:last-child .cm-content");

  const view = EditorView.findFromDOM(content as HTMLElement);

  expect(view?.state.readOnly).toBe(true);
});

test("the_unified_view_has_checkbox_and_number_on_the_old_lines", () => {
  const change = hunk(1, 2, 1, 1);
  const { parent } = open(
    {
      ...data("a\nb\nc\nd\n", "a\nz\nd\n", [change]),
      selection: [{ hunk: "h1", lines: [0, 2] }],
    },
    { unified: true },
  );

  expect(lineTexts(parent, ".rm-old-line")).toEqual(["b", "c"]);
  const checks = [...parent.querySelectorAll<HTMLElement>(".rm-check")];
  expect(checks.map((check) => [check.dataset.line, check.getAttribute("aria-checked")])).toEqual([
    ["0", "true"],
    ["1", "false"],
    ["2", "true"],
  ]);
  expect(lineTexts(parent, ".rm-old-numbers .cm-gutterElement").slice(1)).toEqual([
    "1",
    "2",
    "3",
    "",
    "4",
    "5",
  ]);
});

test("reloading_the_file_keeps_the_editor_and_changes_the_hunks", () => {
  const { parent, handlers } = open(data("a\nb\n", "a\nc\n", [hunk(1, 1, 1, 1)]));
  const dom = parent.querySelector(".cm-content");

  editors?.setData(data("a\nb\n", "z\na\nc\n", [hunk(0, 0, 0, 1), hunk(1, 1, 2, 1)]));

  expect(parent.querySelector(".cm-content")).toBe(dom);
  expect(editors?.content()).toBe("z\na\nc\n");
  expect(editors?.dirty).toBe(false);
  expect(parent.querySelectorAll(".rm-check")).toHaveLength(2);
  expect(handlers.onedit).not.toHaveBeenCalled();
});

test("after_saving_there_are_no_changes_and_the_checkboxes_come_back", () => {
  const { parent } = open(data("a\nb\n", "a\nc\n", [hunk(1, 1, 1, 1)]));
  const view = EditorView.findFromDOM(
    parent.querySelector<HTMLElement>(".rm-editor:last-child .cm-content") as HTMLElement,
  );

  view?.dispatch({ changes: { from: 0, insert: "z" } });
  editors?.setDraftHunks([hunk(0, 2, 0, 2)]);
  expect(editors?.dirty).toBe(true);
  expect(parent.querySelectorAll(".rm-check")).toHaveLength(0);

  editors?.markSaved(editors.snapshot());
  editors?.setData(data("a\nb\n", "za\nc\n", [hunk(0, 2, 0, 2)]));

  expect(editors?.dirty).toBe(false);
  expect(parent.querySelectorAll(".rm-check")).toHaveLength(1);
});

test("what_is_marked_saved_is_what_was_sent_not_what_was_typed_afterwards", () => {
  const { parent } = open(data("a\n", "b\n", [hunk(0, 1, 0, 1)]));
  const view = EditorView.findFromDOM(
    parent.querySelector<HTMLElement>(".rm-editor:last-child .cm-content") as HTMLElement,
  ) as EditorView;
  view.dispatch({ changes: { from: 0, insert: "X" } });

  const sent = editors?.snapshot();
  view.dispatch({ changes: { from: 0, insert: "Y" } });
  editors?.markSaved(sent as never);

  expect(editors?.dirty).toBe(true);
  expect(editors?.textOf(sent as never)).toBe("Xb\n");
  expect(editors?.content()).toBe("YXb\n");
});

test("reloading_a_crlf_file_does_not_insert_breaks_in_a_line", () => {
  const before = "a\r\nb\r\nc\r\n";
  const after = "a\r\nx\r\ny\r\nc\r\n";
  open(data(before, before, []));

  editors?.setData(data(before, after, [hunk(1, 1, 1, 2)]));

  expect(editors?.content()).toBe(after);
});

test("says_whether_its_editors_fit_other_content", () => {
  const crlf = "a\r\nb\r\n";
  open(data(crlf, crlf, []));

  expect(editors?.fits(data(crlf, "a\r\nc\r\n", []), true)).toBe(true);
  expect(editors?.fits(data(crlf, "a\r\nb\n", []), true)).toBe(false);
  expect(editors?.fits(data(crlf, "a\nb\n", []), true)).toBe(false);
  expect(editors?.fits(data("a\nb\n", crlf, []), true)).toBe(false);
  expect(editors?.fits(data(crlf, crlf, []), false)).toBe(false);
});

test("folding_leaves_the_changes_in_view", () => {
  const lines = Array.from({ length: 40 }, (_, index) => `l${index}`);
  const before = `${lines.join("\n")}\n`;
  const after = before.replace("l20\n", "change\n");
  const parent = document.body.appendChild(document.createElement("div"));
  editors = new DiffEditors(parent, {
    path: "a.txt",
    data: data(before, after, [hunk(20, 1, 20, 1)]),
    draft: null,
    editable: true,
    unified: false,
    embedded: false,
    options: { ...OPTIONS, collapse: true },
    handlers: { ontoggle: vi.fn(), onedit: vi.fn(), onfocus: vi.fn() },
  });

  const folds = lineTexts(parent, ".rm-fold");

  expect(folds).toContain("16 unchanged lines");
  expect(lineTexts(parent, ".cm-line.rm-modified")).toEqual(["l20", "change"]);
});
