import { EditorView, type BlockInfo } from "@codemirror/view";
import { afterEach, expect, test, vi } from "vitest";
import type { Hunk } from "../lib/api";
import { DiffEditors, type DiffData, type EditorOptions, type Handlers } from "./editor";

// What `EditorView.lineWrapping` puts on the content; `view.lineWrapping` only follows it after a
// measure, which jsdom does not do.
const wrapped = (view: EditorView) => view.contentDOM.classList.contains("cm-lineWrapping");

const OPTIONS = {
  highlight: "words",
  collapse: false,
  align: true,
  sync: true,
  wrap: false,
} as const satisfies EditorOptions;

let editors: DiffEditors | null = null;

// jsdom does no layout: CodeMirror measures with this.
Range.prototype.getClientRects = () => [] as unknown as DOMRectList;

function open(
  data: DiffData,
  extra: { unified?: boolean; editable?: boolean; options?: EditorOptions } = {},
) {
  const handlers: Handlers = { ontoggle: vi.fn(), onedit: vi.fn(), onfocus: vi.fn() };
  const parent = document.body.appendChild(document.createElement("div"));
  editors = new DiffEditors(parent, {
    path: "a.txt",
    data,
    draft: null,
    editable: extra.editable ?? true,
    unified: extra.unified ?? false,
    embedded: false,
    options: extra.options ?? OPTIONS,
    handlers,
  });
  return { parent, handlers };
}

afterEach(() => {
  editors?.destroy();
  editors = null;
  document.body.textContent = "";
  vi.useRealTimers();
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

const views = (parent: HTMLElement) =>
  [...parent.querySelectorAll<HTMLElement>(".cm-editor")].map(
    (dom) => EditorView.findFromDOM(dom) as EditorView,
  );

test("wrap_off_keeps_todays_configuration", () => {
  const { parent } = open(data("a\nb\n", "a\nc\n", [hunk(1, 1, 1, 1)]));

  expect(parent.querySelector(".cm-lineWrapping")).toBeNull();
  expect(views(parent).map(wrapped)).toEqual([false, false]);
  expect(parent.querySelectorAll<HTMLElement>(".rm-spacer")).toHaveLength(0);
});

test("soft_wrap_wraps_both_sides_without_rebuilding_the_views", () => {
  const { parent } = open(data("a\nb\n", "a\nnew\nother\nb\n", [hunk(1, 0, 1, 2)]));
  const before = views(parent);
  const contents = [...parent.querySelectorAll(".cm-content")];

  editors?.setOptions({ ...OPTIONS, wrap: true });

  expect(views(parent)).toEqual(before);
  expect(before.map(wrapped)).toEqual([true, true]);
  expect([...parent.querySelectorAll(".cm-content")]).toEqual(contents);
  expect(contents.every((content) => content.classList.contains("cm-lineWrapping"))).toBe(true);
  // The aligned diff keeps its line-count spacers here.
  const spacers = [...parent.querySelectorAll<HTMLElement>(".rm-spacer")];
  expect(spacers.map((spacer) => spacer.style.height)).toEqual(["40px"]);

  editors?.setOptions({ ...OPTIONS, wrap: false });

  expect(before.map(wrapped)).toEqual([false, false]);
  expect(parent.querySelector(".cm-lineWrapping")).toBeNull();
});

test("a_diff_opened_with_wrap_on_wraps_from_the_start", () => {
  const { parent } = open(data("a\nb\n", "a\nc\n", [hunk(1, 1, 1, 1)]), {
    options: { ...OPTIONS, wrap: true },
  });

  expect(views(parent).map(wrapped)).toEqual([true, true]);
});

test("unified_old_lines_wrap_too", () => {
  const { parent } = open(data("a\nb\n", "a\nc\n", [hunk(1, 1, 1, 1)]), { unified: true });
  const old = () => parent.querySelector<HTMLElement>(".rm-old-line") as HTMLElement;
  expect(getComputedStyle(old()).whiteSpace).toBe("pre");

  editors?.setOptions({ ...OPTIONS, wrap: true });

  expect(getComputedStyle(old()).whiteSpace).toBe("pre-wrap");
});

test("the_checkbox_stays_on_the_first_row_of_a_wrapped_line", () => {
  const { parent } = open(data("a\nb\n", "a\nc\n", [hunk(1, 1, 1, 1)]), {
    options: { ...OPTIONS, wrap: true },
  });
  const check = parent.querySelector<HTMLElement>(".rm-check") as HTMLElement;
  const cell = check.closest<HTMLElement>(".cm-gutterElement") as HTMLElement;

  expect(getComputedStyle(cell).alignItems).toBe("flex-start");
  expect(getComputedStyle(check).height).toBe("20px");
});

const WRAPPED = { ...OPTIONS, wrap: true } as const satisfies EditorOptions;

// The measuring pass waits for an animation frame; the tests move the frames by hand.
const fakeFrames = () =>
  vi.useFakeTimers({ toFake: ["requestAnimationFrame", "cancelAnimationFrame"] });

// jsdom lays nothing out: this makes each line of `extras` (0-based, mutable) take that many px
// more, as wrapped rows would.
function wrapLines(view: EditorView, extras: Record<number, number>) {
  const real = view.lineBlockAt.bind(view);
  return vi.spyOn(view, "lineBlockAt").mockImplementation((pos) => {
    const block = real(pos);
    const line = view.state.doc.lineAt(pos).number - 1;
    let above = 0;
    for (const [wrapped, extra] of Object.entries(extras)) {
      if (Number(wrapped) < line) above += extra;
    }
    const own = extras[line] ?? 0;
    return {
      from: block.from,
      to: block.to,
      length: block.length,
      type: block.type,
      widget: block.widget,
      widgetLineBreaks: block.widgetLineBreaks,
      top: block.top + above,
      height: block.height + own,
      bottom: block.bottom + above + own,
    } as BlockInfo;
  });
}

const wrapLine = (view: EditorView, line: number, extra: number) =>
  wrapLines(view, { [line]: extra });

const spacerHeights = (parent: HTMLElement, editor: "first" | "last") =>
  [...parent.querySelectorAll<HTMLElement>(`.rm-editor:${editor}-child .rm-spacer`)].map(
    (spacer) => spacer.style.height,
  );

test("with_wrap_off_spacers_are_line_counts_times_20", () => {
  fakeFrames();
  const { parent } = open(data("a\nb\n", "a\nnew\nother\nb\n", [hunk(1, 0, 1, 2)]));
  const [left] = views(parent);
  wrapLine(left, 0, 20);

  vi.advanceTimersToNextFrame();
  vi.advanceTimersToNextFrame();

  expect(spacerHeights(parent, "first")).toEqual(["40px"]);
  expect(spacerHeights(parent, "last")).toEqual([]);
});

test("with_wrap_on_spacers_follow_the_measured_heights", () => {
  fakeFrames();
  // "long" is unchanged and wraps in two rows on the left only: the right one waits above the hunk.
  const { parent } = open(data("a\nlong\nc\n", "a\nlong\nC\n", [hunk(2, 1, 2, 1)]), {
    options: WRAPPED,
  });
  const [left] = views(parent);
  wrapLine(left, 1, 20);
  expect(parent.querySelectorAll(".rm-spacer")).toHaveLength(0);

  vi.advanceTimersToNextFrame();

  expect(spacerHeights(parent, "first")).toEqual([]);
  expect(spacerHeights(parent, "last")).toEqual(["20px"]);
  const spacer = parent.querySelector<HTMLElement>(".rm-editor:last-child .rm-spacer");
  expect(spacer?.nextElementSibling?.textContent).toBe("C");
  // It levels unchanged text, like the top inlay: nothing is painted.
  expect(spacer?.classList.contains("rm-spacer-blank")).toBe(true);
});

test("a_second_measuring_pass_dispatches_nothing", () => {
  fakeFrames();
  const { parent } = open(data("a\nlong\nc\n", "a\nlong\nC\n", [hunk(2, 1, 2, 1)]), {
    options: WRAPPED,
  });
  const [left, right] = views(parent);
  const measured = wrapLine(left, 1, 20);
  vi.advanceTimersToNextFrame();
  expect(spacerHeights(parent, "last")).toEqual(["20px"]);

  editors?.setOptions({ ...WRAPPED, sync: false });
  measured.mockClear();
  const dispatches = [vi.spyOn(left, "dispatch"), vi.spyOn(right, "dispatch")];
  vi.advanceTimersToNextFrame();

  expect(measured).toHaveBeenCalled();
  for (const dispatch of dispatches) expect(dispatch).not.toHaveBeenCalled();
  expect(spacerHeights(parent, "last")).toEqual(["20px"]);
});

test("the_two_number_gutters_have_the_same_width_when_aligned", () => {
  const lines = Array.from({ length: 120 }, (_, index) => `l${index}`).join("\n");
  const { parent } = open(data("a\nb\n", `${lines}\n`, [hunk(0, 2, 0, 120)]), {
    options: WRAPPED,
  });

  const widths = () =>
    [...parent.querySelectorAll<HTMLElement>(".rm-editor")].map((editor) => {
      const cell = editor.querySelector<HTMLElement>(".cm-lineNumbers .cm-gutterElement");
      return getComputedStyle(cell as HTMLElement).minWidth;
    });

  expect(widths()).toEqual(["calc(3ch + 12px)", "calc(3ch + 12px)"]);

  editors?.setOptions({ ...WRAPPED, align: false });

  expect(widths()).toEqual(["20px", "20px"]);
});

test("a_change_within_a_pixel_dispatches_nothing", () => {
  fakeFrames();
  const { parent } = open(data("a\nlong\nc\n", "a\nlong\nC\n", [hunk(2, 1, 2, 1)]), {
    options: WRAPPED,
  });
  const [left, right] = views(parent);
  const extras = { 1: 20 };
  wrapLines(left, extras);
  vi.advanceTimersToNextFrame();

  extras[1] = 20.5;
  editors?.setOptions({ ...WRAPPED, sync: false });
  const dispatches = [vi.spyOn(left, "dispatch"), vi.spyOn(right, "dispatch")];
  vi.advanceTimersToNextFrame();

  for (const dispatch of dispatches) expect(dispatch).not.toHaveBeenCalled();
  expect(spacerHeights(parent, "last")).toEqual(["20px"]);
});

// When another spacer moves, the whole list is sent: a spacer half a pixel off must not keep its
// old DOM, or what is shown would no longer be what is subtracted.
test("a_spacer_sent_again_shows_its_exact_height", () => {
  fakeFrames();
  const before = "a\nlong\nc\nd\nlong\nf\n";
  const after = "a\nlong\nC\nd\nlong\nF\n";
  const { parent } = open(data(before, after, [hunk(2, 1, 2, 1), hunk(5, 1, 5, 1)]), {
    options: WRAPPED,
  });
  const [left] = views(parent);
  const extras: Record<number, number> = { 1: 20, 4: 30 };
  wrapLines(left, extras);
  vi.advanceTimersToNextFrame();
  expect(spacerHeights(parent, "last")).toEqual(["20px", "30px"]);

  extras[1] = 20.5;
  extras[4] = 35;
  editors?.setOptions({ ...WRAPPED, sync: false });
  vi.advanceTimersToNextFrame();

  expect(spacerHeights(parent, "last")).toEqual(["20.5px", "35px"]);
});
