import { isolateHistory, redo, undo } from "@codemirror/commands";
import { EditorSelection } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, expect, test, vi } from "vitest";
import type { MergeBlock, MergeBlockKind } from "../lib/api";
import { MergeEditors } from "./editors";
import { canResolveAutomatically, type MergePane } from "./model";

// jsdom does no layout: CodeMirror measures with this.
Range.prototype.getClientRects = () => [] as unknown as DOMRectList;
Element.prototype.scrollIntoView = () => {};

let current: Merge | null = null;

afterEach(() => {
  current?.editors.destroy();
  current = null;
  document.body.textContent = "";
});

// DiffTestCase.parseSource: `_` is a line break.
const parse = (text: string) => text.replaceAll("_", "\n");

type Sides = "left" | "right" | "both";
type Range2 = [number, number];

function block(
  kind: MergeBlockKind,
  left: Range2,
  base: Range2,
  right: Range2,
  sides: Sides = "both",
  auto: string | null = null,
): MergeBlock {
  return { kind, left, base, right, leftChanged: sides !== "right", rightChanged: sides !== "left", auto };
}

// MergeTestBase.TestBuilder, on top of MergeEditors.
class Merge {
  readonly handlers = { onchange: vi.fn(), onallprocessed: vi.fn() };
  readonly parent = document.body.appendChild(document.createElement("div"));
  readonly editors: MergeEditors;

  constructor(left: string, base: string, right: string, blocks: MergeBlock[]) {
    this.editors = new MergeEditors(
      this.parent,
      {
        left: parse(left),
        base: parse(base),
        right: parse(right),
        blocks,
        titles: ["Yours", "Result", "Theirs"],
      },
      this.handlers,
    );
  }

  view(pane: MergePane): EditorView {
    const dom = this.parent.querySelector<HTMLElement>(`[data-pane="${pane}"] .cm-editor`);
    return EditorView.findFromDOM(dom as HTMLElement) as EditorView;
  }

  get centre() {
    return this.view("base");
  }

  state(index: number) {
    return this.editors.states()[index];
  }

  apply(index: number, side: "left" | "right", whole = false) {
    this.editors.apply(index, side, whole);
  }

  ignore(index: number, side: "left" | "right", whole = false) {
    this.editors.ignore(index, side, whole);
  }

  resolved(index: number): "none" | "left" | "right" | "both" {
    const { left, right } = this.state(index);
    if (left && right) return "both";
    return left ? "left" : right ? "right" : "none";
  }

  // DiffUtil.getLinesContent of the center's range.
  content(index: number): string {
    const { from, to } = this.state(index).output;
    if (from === to) return "";
    const doc = this.centre.state.doc;
    return doc.sliceString(doc.line(from + 1).from, doc.line(to).to);
  }

  range(index: number): number[] {
    const { block: original, output } = this.state(index);
    return [...original.left, output.from, output.to, ...original.right];
  }

  canResolve(index: number) {
    return canResolveAutomatically(this.state(index), "base");
  }

  expectText(text: string) {
    expect(this.editors.result()).toBe(parse(text));
  }

  // Each edit is a command of its own, like `write` in MergeTestBase.
  replace(from: number, to: number, text: string) {
    this.centre.dispatch({
      changes: { from, to, insert: parse(text) },
      annotations: isolateHistory.of("full"),
      userEvent: "input.type",
    });
  }

  offset(line: number, column: number) {
    return this.centre.state.doc.line(line + 1).from + column;
  }

  private find(text: string): [number, number] {
    const doc = this.editors.result();
    const target = parse(text);
    const index = doc.indexOf(target);
    expect(index).toBeGreaterThanOrEqual(0);
    expect(doc.indexOf(target, index + 1)).toBe(-1);
    return [index, index + target.length];
  }

  replaceText(old: string, text: string) {
    const [from, to] = this.find(old);
    this.replace(from, to, text);
  }

  deleteText(old: string) {
    this.replaceText(old, "");
  }

  insertBefore(old: string, text: string) {
    const [from] = this.find(old);
    this.replace(from, from, text);
  }

  insertAfter(old: string, text: string) {
    const [, to] = this.find(old);
    this.replace(to, to, text);
  }

  private record() {
    return { text: this.editors.result(), states: structuredClone(this.editors.states()) };
  }

  undo(count = 1) {
    for (let step = 0; step < count; step++) expect(undo(this.centre)).toBe(true);
  }

  checkUndo(count: number, action: () => void) {
    const initial = this.record();
    action();
    const after = this.record();
    this.undo(count);
    const undone = this.record();
    for (let step = 0; step < count; step++) expect(redo(this.centre)).toBe(true);
    const redone = this.record();
    expect(undone).toEqual(initial);
    expect(redone).toEqual(after);
  }
}

function doTest(left: string, base: string, right: string, blocks: MergeBlock[]): Merge {
  current?.editors.destroy();
  current = new Merge(left, base, right, blocks);
  return current;
}

const one = (kind: MergeBlockKind, range: Range2, sides: Sides = "both", auto: string | null = null) =>
  block(kind, range, range, range, sides, auto);

function operations(merge: Merge, pane: MergePane) {
  return [...merge.parent.querySelectorAll<HTMLElement>(`[data-pane="${pane}"] .rm-operation`)].map(
    (element) => ({
      action: element.dataset.action,
      index: Number(element.dataset.index),
      icon: element.dataset.icon,
      title: element.title,
    }),
  );
}

function caretLine(view: EditorView) {
  return view.state.doc.lineAt(view.state.selection.main.head).number - 1;
}

// Tests of our own for the task.

test("center_starts_with_the_base", () => {
  const merge = doTest("a_X_c", "a_b_c", "a_Y_c", [one("conflict", [1, 2])]);

  merge.expectText("a_b_c");
  expect(merge.editors.modified()).toBe(false);
  expect(undo(merge.centre)).toBe(false);
  expect(merge.view("left").state.readOnly).toBe(true);
  expect(merge.view("right").state.readOnly).toBe(true);
  expect(merge.centre.state.readOnly).toBe(false);
  expect(
    [...merge.parent.querySelectorAll(".rm-merge-title")].map((title) => title.textContent),
  ).toEqual(["Yours", "Result", "Theirs"]);
});

test("apply_left_copies_its_lines", () => {
  const merge = doTest("a_L1_L2_c", "a_b_c", "a_b_c", [
    block("modified", [1, 3], [1, 2], [1, 2], "left"),
  ]);

  merge.apply(0, "left");

  merge.expectText("a_L1_L2_c");
  expect(merge.resolved(0)).toBe("both");
  expect(merge.state(0).output).toEqual({ from: 1, to: 3 });
  expect(merge.editors.modified()).toBe(true);
  expect(merge.handlers.onchange).toHaveBeenCalled();
});

test("apply_both_sides_of_a_conflict_leaves_left_and_then_right", () => {
  const merge = doTest("a_L_c", "a_b_c", "a_R_c", [one("conflict", [1, 2])]);

  merge.apply(0, "left");
  merge.expectText("a_L_c");
  expect(merge.resolved(0)).toBe("left");

  merge.apply(0, "right");
  merge.expectText("a_L_R_c");
  expect(merge.resolved(0)).toBe("both");
  expect(merge.content(0)).toBe("L\nR");
});

test("apply_with_modifier_resolves_the_whole_block", () => {
  const merge = doTest("a_L_c", "a_b_c", "a_R_c", [one("conflict", [1, 2])]);

  merge.apply(0, "right", true);

  merge.expectText("a_R_c");
  expect(merge.resolved(0)).toBe("both");
  expect(merge.editors.counts()).toEqual({ changes: 0, conflicts: 0 });
});

test("ignore_one_side_of_a_conflict_leaves_the_other_pending", () => {
  const merge = doTest("a_L_c", "a_b_c", "a_R_c", [one("conflict", [1, 2])]);

  merge.ignore(0, "left");

  merge.expectText("a_b_c");
  expect(merge.resolved(0)).toBe("left");
  expect(merge.editors.counts()).toEqual({ changes: 0, conflicts: 1 });

  merge.apply(0, "right");
  merge.expectText("a_R_c");
  expect(merge.resolved(0)).toBe("both");
});

test("editing_inside_a_block_marks_it_edited_and_resolve_no_longer_applies", () => {
  const merge = doTest("y z", "x y z", "x y", [one("conflict", [0, 1], "both", "y")]);
  expect(operations(merge, "base").map((operation) => operation.action)).toEqual(["resolve"]);

  merge.replace(2, 3, "U");
  expect(merge.state(0).edited).toBe(true);
  expect(operations(merge, "base")).toEqual([]);

  merge.editors.resolve(0);

  merge.expectText("x U z");
  expect(merge.resolved(0)).toBe("none");
});

test("ranges_follow_the_text_when_edited_before", () => {
  const merge = doTest("a_b_L_d", "a_b_c_d", "a_b_c_d", [
    block("modified", [2, 3], [2, 3], [2, 3], "left"),
  ]);

  merge.replace(0, 0, "new_");
  expect(merge.state(0).output).toEqual({ from: 3, to: 4 });
  merge.replace(merge.offset(2, 1), merge.offset(2, 1), "zz");
  expect(merge.state(0).output).toEqual({ from: 3, to: 4 });

  merge.apply(0, "left");
  merge.expectText("new_a_bzz_L_d");
});

test("apply_non_conflicting_left_all_right", () => {
  const left = "1_L_2_B_3_S_4_X";
  const base = "1_A_2_B_3_C_4_D";
  const right = "1_A_2_R_3_S_4_Y";
  const blocks = [
    one("modified", [1, 2], "left"),
    one("modified", [3, 4], "right"),
    one("modified", [5, 6], "both"),
    one("conflict", [7, 8]),
  ];

  let merge = doTest(left, base, right, blocks);
  merge.editors.applyNonConflicting("left");
  merge.expectText("1_L_2_B_3_S_4_D");
  expect(merge.editors.counts()).toEqual({ changes: 1, conflicts: 1 });

  merge = doTest(left, base, right, blocks);
  merge.editors.applyNonConflicting("right");
  merge.expectText("1_A_2_R_3_S_4_D");
  expect(merge.editors.counts()).toEqual({ changes: 1, conflicts: 1 });

  merge = doTest(left, base, right, blocks);
  merge.editors.applyNonConflicting("all");
  merge.expectText("1_L_2_R_3_S_4_D");
  expect(merge.editors.counts()).toEqual({ changes: 0, conflicts: 1 });
  expect(merge.resolved(3)).toBe("none");
});

test("resolve_automatically_only_touches_conflicts_with_auto_not_edited", () => {
  const merge = doTest("y z_K_y z_K_L", "x y z_K_x y z_K_B", "x y_K_x y_K_R", [
    one("conflict", [0, 1], "both", "y"),
    one("conflict", [2, 3], "both", "y"),
    one("conflict", [4, 5]),
  ]);
  merge.replace(merge.offset(2, 2), merge.offset(2, 3), "U");

  merge.editors.resolveAutomatically();

  merge.expectText("y_K_x U z_K_B");
  expect([0, 1, 2].map((index) => merge.resolved(index))).toEqual(["both", "none", "none"]);
});

// MagicResolvedConflictsAction goes through canResolveChangeAutomatically(change, BASE), which also accepts
// the changes without conflict: it applies them from the side that changed.
test("resolve_automatically_also_applies_the_non_conflicting_like_intellij", () => {
  const merge = doTest("L_K_y z", "A_K_x y z", "A_K_x y", [
    one("modified", [0, 1], "left"),
    one("conflict", [2, 3], "both", "y"),
  ]);

  merge.editors.resolveAutomatically();

  merge.expectText("L_K_y");
  expect(merge.editors.counts()).toEqual({ changes: 0, conflicts: 0 });
});

test("everything_processed_notifies_once", () => {
  const merge = doTest("L_K_M", "A_K_B", "A_K_R", [
    one("modified", [0, 1], "left"),
    one("conflict", [2, 3]),
  ]);

  merge.apply(0, "left");
  expect(merge.handlers.onallprocessed).not.toHaveBeenCalled();

  merge.apply(1, "right", true);
  merge.ignore(1, "left");
  merge.editors.applyNonConflicting("all");

  expect(merge.handlers.onallprocessed).toHaveBeenCalledTimes(1);
});

test("navigating_jumps_between_unresolved_blocks", () => {
  const merge = doTest("a_L_c_M_e_N_g", "a_b_c_d_e_f_g", "a_b_c_d_e_f_g", [
    one("modified", [1, 2], "left"),
    one("modified", [3, 4], "left"),
    one("modified", [5, 6], "left"),
  ]);
  merge.ignore(1, "left");
  merge.centre.dispatch({ selection: EditorSelection.cursor(0) });

  expect(merge.editors.navigate(1)).toBe(true);
  expect(caretLine(merge.centre)).toBe(1);
  expect(merge.editors.navigate(1)).toBe(true);
  expect(caretLine(merge.centre)).toBe(5);
  expect(caretLine(merge.view("left"))).toBe(5);
  expect(merge.editors.navigate(1)).toBe(false);
  expect(merge.editors.navigate(-1)).toBe(true);
  expect(caretLine(merge.centre)).toBe(1);
  expect(merge.editors.navigate(-1)).toBe(false);
});

test("navigating_uses_the_last_pane_with_focus", () => {
  const merge = doTest("p_q_a_L_c", "a_b_c", "a_b_c", [
    block("inserted", [0, 2], [0, 0], [0, 0], "left"),
    block("modified", [3, 4], [1, 2], [1, 2], "left"),
  ]);
  merge.centre.dispatch({ selection: EditorSelection.cursor(merge.offset(1, 0)) });
  const left = merge.view("left");
  left.contentDOM.dispatchEvent(new FocusEvent("focus"));
  left.contentDOM.dispatchEvent(new FocusEvent("blur"));

  expect(merge.editors.navigate(1)).toBe(true);
  expect(caretLine(left)).toBe(3);
  expect(caretLine(merge.centre)).toBe(1);
});

test("changing_whitespace_recalculates_the_blocks", () => {
  const merge = doTest(" x_new_y_z", "x_ y _z", "x_y _new_ z", [
    block("conflict", [0, 4], [0, 3], [0, 4]),
  ]);
  merge.apply(0, "left");
  expect(merge.editors.modified()).toBe(true);
  merge.handlers.onchange.mockClear();

  merge.editors.setWhitespace("ignore", [
    block("modified", [0, 1], [0, 1], [0, 1], "left"),
    block("inserted", [1, 2], [1, 1], [1, 1], "left"),
  ]);

  merge.expectText("x_ y _z");
  expect(merge.editors.modified()).toBe(false);
  expect(undo(merge.centre)).toBe(false);
  expect(merge.editors.states().map((state) => state.output)).toEqual([
    { from: 0, to: 1 },
    { from: 1, to: 1 },
  ]);
  expect(merge.editors.counts()).toEqual({ changes: 2, conflicts: 0 });
  expect(merge.handlers.onchange).toHaveBeenCalled();

  // Recalculating resolves nothing, even if no blocks remain.
  merge.editors.setWhitespace("trim", []);
  expect(merge.editors.counts()).toEqual({ changes: 0, conflicts: 0 });
  expect(merge.handlers.onallprocessed).not.toHaveBeenCalled();
});

test("margin_shows_arrow_accept_cross_ignore_and_wand_resolve_with_their_tooltip", () => {
  const ctrl = "Ctrl+click to resolve conflict";
  const merge = doTest("L_K_y z", "A_K_x y z", "A_K_x y", [
    one("modified", [0, 1], "left"),
    one("conflict", [2, 3], "both", "y"),
  ]);

  expect(operations(merge, "left")).toEqual([
    { action: "apply", index: 0, icon: "arrowRight", title: "Accept" },
    { action: "ignore", index: 0, icon: "remove", title: "Ignore" },
    { action: "apply", index: 1, icon: "arrowRight", title: `Accept\n${ctrl}` },
    { action: "ignore", index: 1, icon: "remove", title: `Ignore\n${ctrl}` },
  ]);
  expect(operations(merge, "right")).toEqual([
    { action: "apply", index: 1, icon: "arrow", title: `Accept\n${ctrl}` },
    { action: "ignore", index: 1, icon: "remove", title: `Ignore\n${ctrl}` },
  ]);
  expect(operations(merge, "base")).toEqual([
    { action: "resolve", index: 1, icon: "magicResolve", title: "Resolve" },
  ]);

  const leftApply = merge.parent.querySelector<HTMLElement>(
    '[data-pane="left"] .rm-operation[data-action="apply"][data-index="1"]',
  ) as HTMLElement;
  leftApply.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 0 }));
  merge.expectText("A_K_y z");
  expect(merge.resolved(1)).toBe("left");
  expect(operations(merge, "right")[0]).toMatchObject({ icon: "arrowLeftDown" });
  expect(operations(merge, "base")).toEqual([]);

  const rightIgnore = merge.parent.querySelector<HTMLElement>(
    '[data-pane="right"] .rm-operation[data-action="ignore"]',
  ) as HTMLElement;
  rightIgnore.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 0 }));
  expect(merge.resolved(1)).toBe("both");

  const merge2 = doTest("L", "A", "R", [one("conflict", [0, 1])]);
  const apply = merge2.parent.querySelector<HTMLElement>(
    '[data-pane="right"] .rm-operation[data-action="apply"]',
  ) as HTMLElement;
  apply.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 0, ctrlKey: true }));
  merge2.expectText("R");
  expect(merge2.resolved(0)).toBe("both");
});

test("the_dividers_join_each_block_of_a_side_with_the_center", () => {
  const merge = doTest("L_b", "A_b", "A_b", [one("modified", [0, 1], "left")]);

  const dividers = [...merge.parent.querySelectorAll(".rm-divider")];

  expect(dividers).toHaveLength(2);
  expect(dividers[0].querySelectorAll("path.rm-modified-band")).toHaveLength(1);
  expect(dividers[1].querySelectorAll("path")).toHaveLength(0);

  merge.apply(0, "left");
  expect(dividers[0].querySelectorAll("path.rm-resolved-band")).toHaveLength(1);
});

test("the_changed_words_are_highlighted_in_their_block", () => {
  const merge = doTest("one TWO three", "one two three", "one two three", [
    one("modified", [0, 1], "left"),
  ]);

  const marks = [...merge.parent.querySelectorAll('[data-pane="left"] .rm-word-modified')];

  expect(marks.map((mark) => mark.textContent)).toEqual(["TWO"]);
});

// Deleting the lines of a block leaves it on its line: at the start, in the middle and at the end.

test("apply_right_and_then_left_on_line_0_respects_the_order", () => {
  const merge = doTest("L_K_b", "a_K_b", "K_b", [block("conflict", [0, 1], [0, 1], [0, 0])]);

  merge.apply(0, "right");
  merge.expectText("K_b");
  expect(merge.state(0).output).toEqual({ from: 0, to: 0 });
  merge.apply(0, "left");

  merge.expectText("L_K_b");
  expect(merge.state(0).output).toEqual({ from: 0, to: 1 });
});

test("block_deleted_on_line_0_stays_on_line_0", () => {
  const merge = doTest("a_K_b", "a_K_b", "K_b", [block("deleted", [0, 1], [0, 1], [0, 0], "right")]);

  merge.apply(0, "right");

  merge.expectText("K_b");
  expect(merge.state(0).output).toEqual({ from: 0, to: 0 });
  expect(merge.resolved(0)).toBe("both");
});

test("block_deleted_in_the_middle_stays_on_its_line", () => {
  const merge = doTest("X_s_a_b", "x_s_a_b", "x_s_b", [
    block("modified", [0, 1], [0, 1], [0, 1], "left"),
    block("deleted", [2, 3], [2, 3], [2, 2], "right"),
  ]);

  merge.apply(1, "right");

  merge.expectText("x_s_b");
  expect(merge.state(1).output).toEqual({ from: 2, to: 2 });
  expect(merge.state(0)).toMatchObject({ output: { from: 0, to: 1 }, edited: false });
});

test("block_deleted_on_the_last_line_stays_at_the_end", () => {
  const merge = doTest("K_B_s_a", "K_b_s_a", "K_b_s", [
    block("modified", [1, 2], [1, 2], [1, 2], "left"),
    block("deleted", [3, 4], [3, 4], [3, 3], "right"),
  ]);

  merge.apply(1, "right");

  merge.expectText("K_b_s");
  expect(merge.state(1).output).toEqual({ from: 3, to: 3 });
  expect(merge.state(0)).toMatchObject({ output: { from: 1, to: 2 }, edited: false });
});

// Cases ported from MergeTest.kt. The blocks are those IntelliJ expects, and the same ones that
// `merge3::blocks` gives for those texts.

test("test_modifications", () => {
  let merge = doTest("x", "x", "y", [one("modified", [0, 1], "right")]);
  merge.apply(0, "right");
  expect(merge.resolved(0)).toBe("both");
  expect(merge.content(0)).toBe("y");
  merge.expectText("y");

  merge = doTest("x", "x", "y", [one("modified", [0, 1], "right")]);
  merge.apply(0, "left");
  expect(merge.resolved(0)).toBe("both");
  expect(merge.content(0)).toBe("x");
  merge.expectText("x");

  merge = doTest("X_x_Z", "X_x_Z", "X_y_Z", [one("modified", [1, 2], "right")]);
  merge.apply(0, "right");
  expect(merge.resolved(0)).toBe("both");
  expect(merge.content(0)).toBe("y");
  merge.expectText("X_y_Z");

  merge = doTest("z", "x", "y", [one("conflict", [0, 1])]);
  merge.apply(0, "right");
  expect(merge.resolved(0)).toBe("right");
  expect(merge.content(0)).toBe("y");
  merge.apply(0, "left");
  expect(merge.resolved(0)).toBe("both");
  expect(merge.content(0)).toBe(parse("y_z"));
  merge.expectText("y_z");

  merge = doTest("z", "x", "y", [one("conflict", [0, 1])]);
  merge.apply(0, "left");
  expect(merge.resolved(0)).toBe("left");
  expect(merge.content(0)).toBe("z");
  merge.apply(0, "right");
  expect(merge.resolved(0)).toBe("both");
  expect(merge.content(0)).toBe(parse("z_y"));
  merge.expectText("z_y");

  merge = doTest("X", "X", "X_y", [block("inserted", [1, 1], [1, 1], [1, 2], "right")]);
  merge.apply(0, "right");
  expect(merge.resolved(0)).toBe("both");
  expect(merge.content(0)).toBe("y");
  merge.expectText("X_y");

  merge = doTest("X", "X", "X_y", [block("inserted", [1, 1], [1, 1], [1, 2], "right")]);
  merge.apply(0, "left");
  expect(merge.resolved(0)).toBe("both");
  expect(merge.content(0)).toBe("");
  merge.expectText("X");

  merge = doTest("X_z", "X", "X_y", [block("conflict", [1, 2], [1, 1], [1, 2])]);
  merge.apply(0, "right");
  expect(merge.resolved(0)).toBe("right");
  expect(merge.content(0)).toBe("y");
  merge.apply(0, "left");
  expect(merge.resolved(0)).toBe("both");
  expect(merge.content(0)).toBe(parse("y_z"));
  merge.expectText("X_y_z");

  merge = doTest("X_z", "X_y", "X", [block("conflict", [1, 2], [1, 2], [1, 1])]);
  merge.apply(0, "left");
  expect(merge.resolved(0)).toBe("both");
  expect(merge.content(0)).toBe("z");
  merge.expectText("X_z");

  merge = doTest("X_z", "X_y", "X", [block("conflict", [1, 2], [1, 2], [1, 1])]);
  merge.apply(0, "right");
  expect(merge.resolved(0)).toBe("right");
  expect(merge.content(0)).toBe("");
  merge.apply(0, "left");
  expect(merge.resolved(0)).toBe("both");
  expect(merge.content(0)).toBe("z");
  merge.expectText("X_z");
});

test("test_modifications_ignore", () => {
  let merge = doTest("x", "x", "y", [one("modified", [0, 1], "right")]);
  merge.ignore(0, "right");
  expect(merge.resolved(0)).toBe("both");
  expect(merge.content(0)).toBe("x");
  merge.expectText("x");

  merge = doTest("x", "x", "y", [one("modified", [0, 1], "right")]);
  merge.ignore(0, "left");
  expect(merge.resolved(0)).toBe("both");
  expect(merge.content(0)).toBe("x");
  merge.expectText("x");

  merge = doTest("z", "x", "y", [one("conflict", [0, 1])]);
  merge.ignore(0, "right");
  expect(merge.resolved(0)).toBe("right");
  expect(merge.content(0)).toBe("x");
  merge.ignore(0, "left");
  expect(merge.resolved(0)).toBe("both");
  expect(merge.content(0)).toBe("x");
  merge.expectText("x");

  merge = doTest("z", "x", "y", [one("conflict", [0, 1])]);
  merge.apply(0, "right");
  expect(merge.resolved(0)).toBe("right");
  expect(merge.content(0)).toBe("y");
  merge.ignore(0, "left");
  expect(merge.resolved(0)).toBe("both");
  expect(merge.content(0)).toBe("y");
  merge.expectText("y");

  merge = doTest("z", "x", "y", [one("conflict", [0, 1])]);
  merge.ignore(0, "right");
  expect(merge.resolved(0)).toBe("right");
  expect(merge.content(0)).toBe("x");
  merge.apply(0, "left");
  expect(merge.resolved(0)).toBe("both");
  expect(merge.content(0)).toBe("z");
  merge.expectText("z");

  merge = doTest("X_z", "X_y", "X", [block("conflict", [1, 2], [1, 2], [1, 1])]);
  merge.ignore(0, "right");
  expect(merge.resolved(0)).toBe("right");
  expect(merge.content(0)).toBe("y");
  merge.apply(0, "left");
  expect(merge.resolved(0)).toBe("both");
  expect(merge.content(0)).toBe("z");
  merge.expectText("X_z");

  merge = doTest("X_z", "X_y", "X", [block("conflict", [1, 2], [1, 2], [1, 1])]);
  merge.ignore(0, "left");
  expect(merge.resolved(0)).toBe("left");
  expect(merge.content(0)).toBe("y");
  merge.apply(0, "right");
  expect(merge.resolved(0)).toBe("both");
  expect(merge.content(0)).toBe("");
  merge.expectText("X");
});

test("test_modifications_modifiers", () => {
  let merge = doTest("x", "x", "y", [one("modified", [0, 1], "right")]);
  merge.apply(0, "right", true);
  expect(merge.resolved(0)).toBe("both");
  expect(merge.content(0)).toBe("y");
  merge.expectText("y");

  merge = doTest("x", "x", "y", [one("modified", [0, 1], "right")]);
  merge.ignore(0, "right", true);
  expect(merge.resolved(0)).toBe("both");
  expect(merge.content(0)).toBe("x");
  merge.expectText("x");

  merge = doTest("z", "x", "y", [one("conflict", [0, 1])]);
  merge.apply(0, "right", true);
  expect(merge.resolved(0)).toBe("both");
  expect(merge.content(0)).toBe("y");
  merge.expectText("y");

  merge = doTest("z", "x", "y", [one("conflict", [0, 1])]);
  merge.ignore(0, "right", true);
  expect(merge.resolved(0)).toBe("both");
  expect(merge.content(0)).toBe("x");
  merge.expectText("x");
});

test("test_resolve", () => {
  const single = () => doTest("y z", "x y z", "x y", [one("conflict", [0, 1], "both", "y")]);
  const double = () =>
    doTest("y z_Y_x y", "x y z_Y_x y z", "x y_Y_y z", [
      one("conflict", [0, 1], "both", "y"),
      one("conflict", [2, 3], "both", "y"),
    ]);

  let merge = single();
  merge.editors.resolve(0);
  expect(merge.resolved(0)).toBe("both");
  expect(merge.content(0)).toBe("y");
  merge.expectText("y");

  merge = double();
  merge.editors.resolve(0);
  expect(merge.resolved(0)).toBe("both");
  expect(merge.content(0)).toBe("y");
  merge.editors.resolve(1);
  expect(merge.resolved(1)).toBe("both");
  expect(merge.content(1)).toBe("y");
  merge.expectText("y_Y_y");

  merge = double();
  merge.apply(0, "left", true);
  expect(merge.resolved(0)).toBe("both");
  expect(merge.content(0)).toBe("y z");
  merge.editors.resolve(1);
  expect(merge.resolved(1)).toBe("both");
  expect(merge.content(1)).toBe("y");
  merge.expectText("y z_Y_y");

  merge = single();
  expect(merge.canResolve(0)).toBe(true);
  merge.replace(2, 3, "U");
  merge.expectText("x U z");
  expect(merge.canResolve(0)).toBe(false);

  merge = double();
  expect(merge.canResolve(0)).toBe(true);
  expect(merge.canResolve(1)).toBe(true);
  merge.replace(2, 3, "U");
  expect(merge.canResolve(0)).toBe(false);
  expect(merge.canResolve(1)).toBe(true);
  merge.expectText("x U z_Y_x y z");
  merge.checkUndo(1, () => merge.editors.resolve(1));
  expect(merge.resolved(1)).toBe("both");
  expect(merge.content(1)).toBe("y");
  expect(merge.canResolve(0)).toBe(false);
  expect(merge.canResolve(1)).toBe(false);
  merge.expectText("x U z_Y_y");
  merge.replace(2, 3, "y");
  expect(merge.canResolve(0)).toBe(true);
  expect(merge.canResolve(1)).toBe(false);
  merge.checkUndo(1, () => merge.editors.resolve(0));
  expect(merge.canResolve(0)).toBe(false);
  expect(merge.canResolve(1)).toBe(false);
  merge.expectText("y_Y_y");
});

test("test_undo_simple", () => {
  const conflict = () => doTest("x", "y", "z", [one("conflict", [0, 1])]);

  let merge = conflict();
  merge.checkUndo(1, () => merge.apply(0, "right"));
  merge = conflict();
  merge.checkUndo(1, () => merge.apply(0, "right", true));
  merge = conflict();
  merge.checkUndo(1, () => merge.ignore(0, "right"));
  merge = conflict();
  merge.checkUndo(1, () => merge.ignore(0, "right", true));
  merge = conflict();
  merge.checkUndo(2, () => {
    merge.apply(0, "right");
    merge.apply(0, "left");
  });
  merge = conflict();
  merge.checkUndo(2, () => {
    merge.apply(0, "right");
    merge.apply(0, "right");
  });

  merge = doTest("X", "X_y", "X", [block("deleted", [1, 1], [1, 2], [1, 1])]);
  merge.checkUndo(1, () => merge.apply(0, "right"));

  const inserted = () => doTest("X", "X", "X_y", [block("inserted", [1, 1], [1, 1], [1, 2], "right")]);
  merge = inserted();
  merge.checkUndo(1, () => merge.apply(0, "left"));
  merge = inserted();
  merge.checkUndo(1, () => merge.apply(0, "right"));

  const deleted = () => doTest("X_z", "X_y", "X", [block("conflict", [1, 2], [1, 2], [1, 1])]);
  merge = deleted();
  merge.checkUndo(1, () => merge.apply(0, "left"));
  merge = deleted();
  merge.checkUndo(1, () => merge.apply(0, "right"));

  merge = doTest("y_X", "X", "X_z", [
    block("inserted", [0, 1], [0, 0], [0, 0], "left"),
    block("inserted", [2, 2], [1, 1], [1, 2], "right"),
  ]);
  merge.checkUndo(1, () => merge.apply(0, "left"));
  merge.checkUndo(1, () => merge.apply(1, "right"));
  merge.expectText("y_X_z");
});

test("test_range_modification", () => {
  const simple = () =>
    doTest("X_x_y_z_Y", "X_a_b_c_Y", "X_x_y_z_Y", [one("modified", [1, 4], "both")]);

  let merge = simple();
  expect(merge.content(0)).toBe(parse("a_b_c"));
  merge.checkUndo(1, () => merge.replace(merge.offset(2, 0), merge.offset(2, 1), "D"));
  expect(merge.content(0)).toBe(parse("a_D_c"));
  expect(merge.state(0).output).toEqual({ from: 1, to: 4 });

  merge = simple();
  merge.checkUndo(1, () => merge.replace(merge.offset(2, -1), merge.offset(2, 2), "D"));
  expect(merge.content(0)).toBe("aDc");
  expect(merge.state(0).output).toEqual({ from: 1, to: 2 });

  merge = simple();
  merge.checkUndo(1, () => merge.replaceText("c", "u_x"));
  expect(merge.content(0)).toBe(parse("a_b_u_x"));
  expect(merge.state(0).output).toEqual({ from: 1, to: 5 });

  merge = simple();
  merge.checkUndo(1, () => merge.deleteText("c_"));
  expect(merge.content(0)).toBe(parse("a_b"));
  expect(merge.state(0).output).toEqual({ from: 1, to: 3 });

  merge = simple();
  merge.checkUndo(1, () => merge.insertBefore("b", "q"));
  expect(merge.content(0)).toBe(parse("a_qb_c"));
  expect(merge.state(0).output).toEqual({ from: 1, to: 4 });

  merge = simple();
  merge.checkUndo(1, () => merge.insertAfter("b", "q"));
  expect(merge.content(0)).toBe(parse("a_bq_c"));
  expect(merge.state(0).output).toEqual({ from: 1, to: 4 });

  merge = simple();
  merge.checkUndo(1, () => merge.replace(0, 0, "a_b_c_"));
  expect(merge.content(0)).toBe(parse("a_b_c"));
  expect(merge.state(0).output).toEqual({ from: 4, to: 7 });

  merge = doTest("A_X_x_y_z_Y", "A_X_a_b_c_Y", "A_X_x_y_z_Y", [one("modified", [2, 5], "both")]);
  expect(merge.content(0)).toBe(parse("a_b_c"));
  merge.checkUndo(1, () => merge.replaceText("X_a_b", "q_w_e"));
  expect(merge.content(0)).toBe("c");

  merge = doTest("X_x_y_z_Y_A", "X_a_b_c_Y_A", "X_x_y_z_Y_A", [one("modified", [1, 4], "both")]);
  expect(merge.content(0)).toBe(parse("a_b_c"));
  merge.checkUndo(1, () => merge.replaceText("c_Y", "q"));
  expect(merge.content(0)).toBe(parse("a_b"));

  merge = doTest("A_X_x_Y_A", "A_X_b_Y_A", "A_X_x_z_Y_A", [
    block("conflict", [2, 3], [2, 3], [2, 4]),
  ]);
  expect(merge.content(0)).toBe("b");
  merge.checkUndo(1, () => merge.replaceText("X_b_Y", "q"));
  expect(merge.content(0)).toBe("");
  expect(merge.state(0).output).toEqual({ from: 2, to: 2 });
  merge.expectText("A_q_A");
});

const NON_CONFLICTS = {
  left: [
    "1 ======",
    "insert left",
    "2 ======",
    "remove right",
    "3 ======",
    "new both",
    "4 ======",
    "modify both",
    "5 ======",
    "modify",
    "6 ======",
    "7 ======",
    "8 ======",
  ].join("_"),
  base: [
    "1 ======",
    "2 ======",
    "remove right",
    "3 ======",
    "4 ======",
    "modify",
    "5 ======",
    "modify",
    "6 ======",
    "7 ======",
    "delete modify",
    "8 ======",
  ].join("_"),
  right: [
    "1 ======",
    "2 ======",
    "3 ======",
    "new both",
    "4 ======",
    "modify both",
    "5 ======",
    "modify right",
    "6 ======",
    "7 ======",
    "modify",
    "8 ======",
  ].join("_"),
  blocks: [
    block("inserted", [1, 2], [1, 1], [1, 1], "left"),
    block("deleted", [3, 4], [2, 3], [2, 2], "right"),
    block("inserted", [5, 6], [4, 4], [3, 4], "both"),
    block("modified", [7, 8], [5, 6], [5, 6], "both"),
    block("modified", [9, 10], [7, 8], [7, 8], "right"),
    block("conflict", [12, 12], [10, 11], [10, 11]),
  ],
};

test("test_non_conflicts_actions", () => {
  const { left, base, right, blocks } = NON_CONFLICTS;
  const pending = (merge: Merge) => merge.editors.states().filter((state) => !state.left || !state.right);
  const lines = (...items: string[]) => items.join("\n");

  let merge = doTest(left, base, right, blocks);
  merge.checkUndo(1, () => merge.editors.applyNonConflicting("all"));
  expect(pending(merge)).toHaveLength(1);
  expect(merge.editors.result()).toBe(
    lines(
      "1 ======",
      "insert left",
      "2 ======",
      "3 ======",
      "new both",
      "4 ======",
      "modify both",
      "5 ======",
      "modify right",
      "6 ======",
      "7 ======",
      "delete modify",
      "8 ======",
    ),
  );

  merge = doTest(left, base, right, blocks);
  merge.checkUndo(1, () => merge.editors.applyNonConflicting("left"));
  expect(pending(merge)).toHaveLength(3);
  expect(merge.editors.result()).toBe(
    lines(
      "1 ======",
      "insert left",
      "2 ======",
      "remove right",
      "3 ======",
      "new both",
      "4 ======",
      "modify both",
      "5 ======",
      "modify",
      "6 ======",
      "7 ======",
      "delete modify",
      "8 ======",
    ),
  );

  merge = doTest(left, base, right, blocks);
  merge.checkUndo(1, () => merge.editors.applyNonConflicting("right"));
  expect(pending(merge)).toHaveLength(2);
  expect(merge.editors.result()).toBe(
    lines(
      "1 ======",
      "2 ======",
      "3 ======",
      "new both",
      "4 ======",
      "modify both",
      "5 ======",
      "modify right",
      "6 ======",
      "7 ======",
      "delete modify",
      "8 ======",
    ),
  );

  merge = doTest(left, base, right, blocks);
  merge.replace(merge.offset(5, 0), merge.offset(5, 0), "USER ");
  merge.checkUndo(1, () => merge.editors.applyNonConflicting("all"));
  expect(pending(merge)).toHaveLength(2);
  expect(merge.editors.result()).toBe(
    lines(
      "1 ======",
      "insert left",
      "2 ======",
      "3 ======",
      "new both",
      "4 ======",
      "USER modify",
      "5 ======",
      "modify right",
      "6 ======",
      "7 ======",
      "delete modify",
      "8 ======",
    ),
  );

  merge = doTest(left, base, right, blocks);
  merge.replace(merge.offset(7, 0), merge.offset(7, 0), "USER ");
  merge.checkUndo(1, () => merge.editors.applyNonConflicting("right"));
  expect(pending(merge)).toHaveLength(3);
  expect(merge.editors.result()).toBe(
    lines(
      "1 ======",
      "2 ======",
      "3 ======",
      "new both",
      "4 ======",
      "modify both",
      "5 ======",
      "USER modify",
      "6 ======",
      "7 ======",
      "delete modify",
      "8 ======",
    ),
  );
});

test("test_no_conflicts", () => {
  const merge = doTest("a_Ins1_b_ccc", "a_b_ccc", "a_b_dddd", [
    block("inserted", [1, 2], [1, 1], [1, 1], "left"),
    block("modified", [3, 4], [2, 3], [2, 3], "right"),
  ]);
  expect(merge.range(0)).toEqual([1, 2, 1, 1, 1, 1]);
  expect(merge.range(1)).toEqual([3, 4, 2, 3, 2, 3]);

  merge.apply(0, "left");
  expect(merge.range(0)).toEqual([1, 2, 1, 2, 1, 1]);
  expect(merge.range(1)).toEqual([3, 4, 3, 4, 2, 3]);
  merge.expectText("a_Ins1_b_ccc");

  merge.editors.applyNonConflicting("all");
  expect(merge.range(0)).toEqual([1, 2, 1, 2, 1, 1]);
  expect(merge.range(1)).toEqual([3, 4, 3, 4, 2, 3]);
  merge.expectText("a_Ins1_b_dddd");
  expect(merge.resolved(0)).toBe("both");
  expect(merge.resolved(1)).toBe("both");
});

test("test_conflicting_change", () => {
  const merge = doTest("1_2_3_X_a_b_c_Y_Ver1_Ver12_Z", "X_a_b_c_Y_Ver12_Ver23_Z", "X_Y_Ver23_Ver3_Z", [
    block("inserted", [0, 3], [0, 0], [0, 0], "left"),
    block("deleted", [4, 7], [1, 4], [1, 1], "right"),
    block("conflict", [8, 10], [5, 7], [2, 4]),
  ]);

  merge.editors.applyNonConflicting("all");

  expect([0, 1, 2].map((index) => merge.resolved(index))).toEqual(["both", "both", "none"]);
  merge.expectText("1_2_3_X_Y_Ver12_Ver23_Z");
});

test("test_apply_merge_then_undo", () => {
  const merge = doTest("X_b_Y", "X_1_2_3_Y", "X_a_Y", [block("conflict", [1, 2], [1, 4], [1, 2])]);
  expect(merge.range(0)).toEqual([1, 2, 1, 4, 1, 2]);

  merge.apply(0, "right");
  expect(merge.range(0)).toEqual([1, 2, 1, 2, 1, 2]);
  expect(merge.content(0)).toBe("a");

  merge.checkUndo(1, () => {
    merge.apply(0, "left");
    expect(merge.range(0)).toEqual([1, 2, 1, 3, 1, 2]);
    expect(merge.content(0)).toBe(parse("a_b"));
  });
  merge.expectText("X_a_b_Y");
});

test("test_apply_modified_deleted_conflict", () => {
  const merge = doTest("X_Y", "X_1_2_3_Y", "X_a_Y", [block("conflict", [1, 1], [1, 4], [1, 2])]);
  expect(merge.range(0)).toEqual([1, 1, 1, 4, 1, 2]);
  expect(merge.content(0)).toBe(parse("1_2_3"));

  merge.editors.applyNonConflicting("all");
  expect(merge.resolved(0)).toBe("none");

  merge.apply(0, "right");
  expect(merge.range(0)).toEqual([1, 1, 1, 2, 1, 2]);
  expect(merge.resolved(0)).toBe("both");
  merge.expectText("X_a_Y");
});

test("test_invalidating_change", () => {
  const merge = doTest("X_1_2_Y", "X_1_Ins_2_Y", "X_1_2_Y", [
    block("deleted", [2, 2], [2, 3], [2, 2], "both"),
  ]);
  expect(merge.range(0)).toEqual([2, 2, 2, 3, 2, 2]);

  merge.deleteText("1_Ins_2_");

  expect(merge.resolved(0)).toBe("both");
  expect(merge.range(0)).toEqual([2, 2, 2, 2, 2, 2]);
});

test("test_apply_several_actions", () => {
  const merge = doTest("X_1_Y_2_Z_3_4_U_W_", "X_a_Y_b_Z_c_U_d_W_", "X_a_Y_B_Z_C_U_D_W_", [
    one("modified", [1, 2], "left"),
    one("conflict", [3, 4]),
    block("conflict", [5, 7], [5, 6], [5, 6]),
    block("conflict", [8, 8], [7, 8], [7, 8]),
  ]);

  merge.apply(0, "left");
  expect(merge.resolved(0)).toBe("both");

  merge.apply(3, "right");
  expect(merge.resolved(3)).toBe("both");
  merge.expectText("X_1_Y_b_Z_c_U_D_W_");

  merge.apply(1, "right");
  expect(merge.resolved(1)).toBe("right");
  merge.expectText("X_1_Y_B_Z_c_U_D_W_");

  merge.apply(1, "left");
  merge.apply(2, "left");
  merge.apply(2, "right");
  expect(merge.resolved(1)).toBe("both");
  expect(merge.resolved(2)).toBe("both");
  merge.expectText("X_1_Y_B_2_Z_3_4_C_U_D_W_");
});

test("test_ignore_change_action", () => {
  const merge = doTest("X_1_Y_2_Z", "X_a_Y_b_Z", "X_a_Y_B_Z", [
    one("modified", [1, 2], "left"),
    one("conflict", [3, 4]),
  ]);

  merge.ignore(0, "left");
  expect(merge.range(0)).toEqual([1, 2, 1, 2, 1, 2]);
  merge.expectText("X_a_Y_b_Z");

  merge.ignore(0, "right");
  expect(merge.resolved(0)).toBe("both");
  expect(merge.range(0)).toEqual([1, 2, 1, 2, 1, 2]);
  merge.expectText("X_a_Y_b_Z");
});

test("test_replace_base_with_branch", () => {
  const merge = doTest("a_X_b_c", "A_X_B_c", "1_X_1_c", [
    one("conflict", [0, 1]),
    one("conflict", [2, 3]),
  ]);

  merge.apply(0, "left");
  merge.expectText("a_X_B_c");

  merge.replaceText("a_X_B_c", "a_X_b_c");

  expect(merge.range(0)).toEqual([0, 1, 0, 1, 0, 1]);
  expect(merge.range(1)).toEqual([2, 3, 2, 3, 2, 3]);
});

test("test_ignored", () => {
  const ignored = [
    block("modified", [0, 1], [0, 1], [0, 1], "left"),
    block("inserted", [1, 2], [1, 1], [1, 1], "left"),
    block("modified", [2, 3], [1, 2], [1, 2], "both"),
    block("inserted", [3, 3], [2, 2], [2, 3], "right"),
    block("modified", [3, 4], [2, 3], [3, 4], "right"),
  ];

  let merge = doTest(" x_new_y_z", "x_ y _z", "x_y _new_ z", ignored);
  merge.checkUndo(1, () => merge.editors.applyNonConflicting("all"));
  merge.expectText(" x_new_y_new_ z");
  expect([0, 1, 2, 3, 4].map((index) => merge.resolved(index))).toEqual([
    "both",
    "both",
    "both",
    "both",
    "both",
  ]);
  merge.undo(1);
  merge.apply(1, "right");
  merge.apply(2, "right");
  merge.checkUndo(1, () => merge.editors.applyNonConflicting("all"));
  merge.expectText(" x_y _new_ z");

  merge = doTest(" x_new_y1_z", "x_ y _z", "x_y2 _new_ z", [
    block("modified", [0, 1], [0, 1], [0, 1], "left"),
    block("conflict", [1, 3], [1, 2], [1, 3]),
    block("modified", [3, 4], [2, 3], [3, 4], "right"),
  ]);
  merge.checkUndo(1, () => merge.editors.applyNonConflicting("all"));
  merge.expectText(" x_ y _ z");
  expect([0, 1, 2].map((index) => merge.resolved(index))).toEqual(["both", "none", "both"]);

  merge = doTest(" x_new_y_z", "x_ y _z", "x_y _new_ z", [
    block("conflict", [0, 4], [0, 3], [0, 4], "both", parse(" x_new_y_new_ z")),
  ]);
  expect(merge.range(0)).toEqual([0, 4, 0, 3, 0, 4]);
  merge.editors.setWhitespace("ignore", ignored);
  expect(undo(merge.centre)).toBe(false);
  expect(merge.range(0)).toEqual([0, 1, 0, 1, 0, 1]);
  expect(merge.range(1)).toEqual([1, 2, 1, 1, 1, 1]);
  expect(merge.range(2)).toEqual([2, 3, 1, 2, 1, 2]);
  expect(merge.range(3)).toEqual([3, 3, 2, 2, 2, 3]);
  expect(merge.range(4)).toEqual([3, 4, 2, 3, 3, 4]);
  merge.checkUndo(1, () => merge.editors.applyNonConflicting("right"));
  merge.expectText("x_y _new_ z");
  expect([0, 1, 2, 3, 4].map((index) => merge.resolved(index))).toEqual([
    "none",
    "none",
    "both",
    "both",
    "both",
  ]);
});
