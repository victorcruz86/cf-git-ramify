import {
  Annotation,
  ChangeSet,
  Facet,
  StateEffect,
  StateField,
  type Text,
} from "@codemirror/state";
import type { MergeBlock } from "../lib/api";

// Ported from MergeThreesideViewer, TextMergeChange, MergeModelBase and DiffUtil (line ranges).

export type MergeSide = "left" | "right";
export type MergePane = "left" | "base" | "right";

// `output` is the range of the block in the center, which follows the text. `left` and `right` say whether that
// side has already been applied or ignored. `oneside` is `myOnesideAppliedConflict`: one side of a conflict
// has already been applied, and applying the other adds it behind. `edited` is `isChangeRangeModified`.
export interface BlockState {
  block: MergeBlock;
  output: { from: number; to: number };
  left: boolean;
  right: boolean;
  edited: boolean;
  oneside: boolean;
}

export interface Counts {
  changes: number;
  conflicts: number;
}

export interface Texts {
  left: Text;
  base: Text;
  right: Text;
}

export function initialBlocks(blocks: MergeBlock[]): BlockState[] {
  return blocks.map((block) => ({
    block,
    output: { from: block.base[0], to: block.base[1] },
    left: false,
    right: false,
    edited: false,
    oneside: false,
  }));
}

export const isResolved = (state: BlockState) => state.left && state.right;
export const isConflict = (state: BlockState) => state.block.kind === "conflict";

// MergeConflictType.isChange: the center always counts as changed.
export function isChange(state: BlockState, pane: MergePane): boolean {
  if (pane === "base") return true;
  return pane === "left" ? state.block.leftChanged : state.block.rightChanged;
}

export function isResolvedIn(state: BlockState, pane: MergePane): boolean {
  return pane === "base" ? isResolved(state) : state[pane];
}

// The range of a block in each pane.
export function rangeIn(state: BlockState, pane: MergePane): [number, number] {
  return pane === "base" ? [state.output.from, state.output.to] : state.block[pane];
}

// ThreesideTextDiffViewerEx: the counters only count what is pending.
export function counts(states: readonly BlockState[]): Counts {
  let changes = 0;
  let conflicts = 0;
  for (const state of states) {
    if (isResolved(state)) continue;
    if (isConflict(state)) conflicts++;
    else changes++;
  }
  return { changes, conflicts };
}

const number = (value: number) => value.toLocaleString("en-US");

// merge.differences.status.text
export function statusText({ changes, conflicts }: Counts): string {
  const first =
    changes === 0 ? "No changes" : changes === 1 ? "1 change" : `${number(changes)} changes`;
  const second =
    conflicts === 0
      ? "No conflicts"
      : conflicts === 1
        ? "1 conflict"
        : `${number(conflicts)} conflicts`;
  return `${first}. ${second}.`;
}

// merge.dialog.apply.partially.resolved.changes.confirmation.message, with its `choice`s.
export function unprocessedMessage({ changes, conflicts }: Counts): string {
  const verb = changes === 0 ? (conflicts >= 2 ? "are" : "is") : changes >= 2 ? "are" : "is";
  const first = changes === 0 ? "" : changes === 1 ? "one change" : `${number(changes)} changes`;
  const and = changes > 0 && conflicts > 0 ? " and " : "";
  const second =
    conflicts === 0 ? "" : conflicts === 1 ? "one conflict" : `${number(conflicts)} conflicts`;
  return (
    `There ${verb} ${first}${and}${second} left unprocessed.\n` +
    "Save changes and mark the conflict resolved anyway?"
  );
}

// The editor works with `\n`. When reading, CRLF and CR become LF.
export const normalized = (text: string) => text.replace(/\r\n?/g, "\n");

// When writing, the line break of the working file. A `\r` pasted in the center is removed.
export const withSeparator = (text: string, separator: string) =>
  text.replace(/\r/g, "").replace(/\n/g, separator);

// MergeThreesideViewer.canResolveChangeAutomatically. `auto` exists only if the conflict can be
// resolved, and holds while the block has the text of the base.
export function canResolveAutomatically(state: BlockState, pane: MergePane): boolean {
  if (isConflict(state)) {
    return pane === "base" && state.block.auto !== null && !state.left && !state.right && !state.edited;
  }
  return !isResolved(state) && isChange(state, pane) && !state.edited;
}

// DiffUtil.updateRangeOnModification, without `greedy`.
export function updateRangeOnModification(
  start: number,
  end: number,
  changeStart: number,
  changeEnd: number,
  shift: number,
): { from: number; to: number; damaged: boolean } {
  if (end <= changeStart) return { from: start, to: end, damaged: false };
  if (start >= changeEnd) return { from: start + shift, to: end + shift, damaged: false };
  if (start <= changeStart && end >= changeEnd) {
    return { from: start, to: end + shift, damaged: false };
  }
  const newChangeEnd = changeEnd + shift;
  if (start >= changeStart && end <= changeEnd) {
    return { from: newChangeEnd, to: newChangeEnd, damaged: true };
  }
  if (start < changeStart) return { from: start, to: changeStart, damaged: true };
  return { from: newChangeEnd, to: end + shift, damaged: true };
}

// DiffUtil.getLinesContent: without the final break, and empty if the range has no lines.
export function linesContent(doc: Text, from: number, to: number): string {
  if (from >= to) return "";
  return doc.sliceString(doc.line(from + 1).from, doc.line(to).to);
}

export function linesOf(doc: Text, from: number, to: number): string[] {
  const lines: string[] = [];
  for (let line = from; line < to; line++) lines.push(doc.line(line + 1).text);
  return lines;
}

const breaks = (text: string) => text.split("\n").length - 1;

function isEdited(state: BlockState, doc: Text, base: Text): boolean {
  const [from, to] = state.block.base;
  return linesContent(doc, state.output.from, state.output.to) !== linesContent(base, from, to);
}

// MergeModelBase.processDocumentChange and its version in MergeThreesideViewer: a deletion that is
// left without lines is resolved.
function processChange(
  states: readonly BlockState[],
  changeStart: number,
  changeEnd: number,
  shift: number,
  affected: Set<number>,
): readonly BlockState[] {
  let next: BlockState[] | null = null;
  states.forEach((state, index) => {
    const { from, to } = state.output;
    const range = updateRangeOnModification(from, to, changeStart, changeEnd, shift);
    const touched = range.damaged || (changeEnd >= from && changeStart <= to);
    if (touched) affected.add(index);
    const emptied = range.from === range.to && state.block.kind === "deleted" && !isResolved(state);
    if (range.from === from && range.to === to && !emptied) return;
    let moved: BlockState = { ...state, output: { from: range.from, to: range.to } };
    if (emptied) moved = { ...moved, left: true, right: true };
    next ??= [...states];
    next[index] = moved;
  });
  return next ?? states;
}

// Each change of the text as lines `[from, to)` of the previous text and how many lines it adds, like
// `getAffectedLineRange` and `countLinesShift` of IntelliJ's DocumentListener. DocumentImpl.replaceString
// does not count the beginning or the end that do not change.
export function lineChanges(
  changes: ChangeSet,
  before: Text,
  after: Text,
): { start: number; end: number; shift: number }[] {
  const result: { start: number; end: number; shift: number }[] = [];
  changes.iterChanges((fromA, toA, fromB, _toB, inserted) => {
    let old = before.sliceString(fromA, toA, "\n");
    let text = inserted.sliceString(0, inserted.length, "\n");
    let prefix = 0;
    while (prefix < old.length && prefix < text.length && old[prefix] === text[prefix]) prefix++;
    let suffix = 0;
    while (
      suffix < old.length - prefix &&
      suffix < text.length - prefix &&
      old[old.length - 1 - suffix] === text[text.length - 1 - suffix]
    ) {
      suffix++;
    }
    old = old.slice(prefix, old.length - suffix);
    text = text.slice(prefix, text.length - suffix);
    if (old === "" && text === "") return;
    const start = after.lineAt(fromB + prefix).number - 1;
    const removed = breaks(old);
    result.push({ start, end: start + removed + 1, shift: breaks(text) - removed });
  });
  return result;
}

// The ranges after a change of the center, like the DocumentListener of MergeModelBase.
export function mapStates(
  states: readonly BlockState[],
  changes: ChangeSet,
  before: Text,
  after: Text,
  base: Text,
): BlockState[] {
  if (states.length === 0 || changes.empty) return states as BlockState[];
  let current = states;
  const affected = new Set<number>();
  for (const { start, end, shift } of lineChanges(changes, before, after)) {
    current = processChange(current, start, end, shift, affected);
  }
  return refreshEdited(current, affected, after, base);
}

// A touched block is a new object even if its range does not change: its text did change, and with it
// the highlighted words (`invalidateHighlighters`).
function refreshEdited(
  current: readonly BlockState[],
  affected: Set<number>,
  after: Text,
  base: Text,
): BlockState[] {
  if (affected.size === 0) return current as BlockState[];
  const result = [...current];
  for (const index of affected) {
    result[index] = { ...result[index], edited: isEdited(result[index], after, base) };
  }
  return result;
}

// DiffUtil.applyModification as a text change: the lines `[line1, line2)` become `lines`.
// Null if nothing changes.
export function linesModification(
  doc: Text,
  line1: number,
  line2: number,
  lines: string[],
): { from: number; to: number; insert: string } | null {
  if (line1 === line2 && lines.length === 0) return null;
  if (line1 === line2) {
    const text = lines.join("\n");
    if (line1 === doc.lines) return { from: doc.length, to: doc.length, insert: `\n${text}` };
    const at = doc.line(line1 + 1).from;
    return { from: at, to: at, insert: `${text}\n` };
  }
  let from = doc.line(line1 + 1).from;
  let to = doc.line(line2).to;
  if (lines.length > 0) return { from, to, insert: lines.join("\n") };
  if (from > 0) from--;
  else if (to < doc.length) to++;
  return { from, to, insert: "" };
}

// LineTokenizer.tokenize(text, false): without the last empty line.
function tokenize(text: string): string[] {
  if (text === "") return [];
  const lines = text.split("\n");
  if (lines.at(-1) === "") lines.pop();
  return lines;
}

// A command on the center, like `executeMergeCommand`: it accumulates the text changes and the states
// to dispatch them in a single transaction.
export class MergeEdit {
  changes: ChangeSet;
  states: BlockState[];

  constructor(
    public doc: Text,
    states: readonly BlockState[],
    private readonly texts: Texts,
  ) {
    this.changes = ChangeSet.empty(doc.length);
    this.states = [...states];
  }

  private set(index: number, state: BlockState) {
    this.states = [...this.states];
    this.states[index] = state;
  }

  // DiffUtil.applyModification.
  private modify(line1: number, line2: number, lines: string[]) {
    const doc = this.doc;
    const found = linesModification(doc, line1, line2, lines);
    if (!found) return;
    const change = ChangeSet.of(found, doc.length, "\n");
    const after = change.apply(doc);
    if (line1 !== line2 && lines.length === 0) {
      // Here Ramify departs from IntelliJ. Its DocumentListener measures the deletion with
      // getAffectedLineRange, which also counts the line break it takes along: deleting line 0
      // leaves the block at 1, and deleting one in the middle or the last empties the block above.
      // A later `appendChange` would then write somewhere else. The deleted lines are
      // exactly [line1, line2), and that is how the ranges are moved.
      const affected = new Set<number>();
      const moved = processChange(this.states, line1, line2, line1 - line2, affected);
      this.states = refreshEdited(moved, affected, after, this.texts.base);
    } else {
      this.states = mapStates(this.states, change, doc, after, this.texts.base);
    }
    this.changes = this.changes.compose(change);
    this.doc = after;
  }

  private withOutput(state: BlockState, from: number, to: number): BlockState {
    const moved = { ...state, output: { from, to } };
    return { ...moved, edited: isEdited(moved, this.doc, this.texts.base) };
  }

  // MergeModelBase.moveChangesAfterInsertion: the inserted block includes what is new, and the neighbors
  // that start or end on that line move aside.
  private moveAfterInsertion(index: number, start: number, end: number) {
    const own = this.states[index];
    if (own.output.from !== start || own.output.to !== end) {
      this.set(index, this.withOutput(own, start, end));
    }
    let before = true;
    for (let other = 0; other < this.states.length; other++) {
      const { from, to } = this.states[other].output;
      if (to < start) continue;
      if (from > end) break;
      if (other === index) {
        before = false;
        continue;
      }
      const nextFrom = before ? Math.min(from, start) : end;
      const nextTo = before ? Math.min(to, start) : Math.max(to, end);
      if (from !== nextFrom || to !== nextTo) {
        this.set(other, this.withOutput(this.states[other], nextFrom, nextTo));
      }
    }
  }

  // MergeModelBase.replaceChange.
  private replaceLines(index: number, lines: string[]) {
    const { from, to } = this.states[index].output;
    this.modify(from, to, lines);
    if (from === to) this.moveAfterInsertion(index, from, from + lines.length);
  }

  // MergeModelBase.appendChange.
  private appendLines(index: number, lines: string[]) {
    const { from, to } = this.states[index].output;
    this.modify(to, to, lines);
    this.moveAfterInsertion(index, from, to + lines.length);
  }

  markResolved(index: number, side?: MergeSide) {
    const state = this.states[index];
    if (side === undefined) {
      if (!isResolved(state)) this.set(index, { ...state, left: true, right: true });
    } else if (!state[side]) {
      this.set(index, { ...state, [side]: true });
    }
  }

  // MergeThreesideViewer.ignoreChange.
  ignore(index: number, side: MergeSide, whole: boolean) {
    if (!isConflict(this.states[index]) || whole) this.markResolved(index);
    else this.markResolved(index, side);
  }

  // MergeThreesideViewer.replaceChange.
  replace(index: number, side: MergeSide, whole: boolean) {
    const state = this.states[index];
    if (state[side]) return;
    if (!isChange(state, side)) {
      this.markResolved(index);
      return;
    }
    const [from, to] = state.block[side];
    const lines = linesOf(this.texts[side], from, to);
    if (!isConflict(state)) {
      this.replaceLines(index, lines);
      this.markResolved(index);
      return;
    }
    if (state.oneside) this.appendLines(index, lines);
    else this.replaceLines(index, lines);
    const opposite = state.block[side === "left" ? "right" : "left"];
    if (whole || opposite[0] === opposite[1]) {
      this.markResolved(index);
    } else {
      this.set(index, { ...this.states[index], oneside: true });
      this.markResolved(index, side);
    }
  }

  // MergeThreesideViewer.resolveChangeAutomatically.
  resolve(index: number, pane: MergePane) {
    const state = this.states[index];
    if (!canResolveAutomatically(state, pane)) return;
    if (isConflict(state)) {
      this.replaceLines(index, tokenize(state.block.auto ?? ""));
      this.markResolved(index);
      return;
    }
    const master: MergeSide =
      pane === "base" ? (state.block.leftChanged ? "left" : "right") : pane;
    this.replace(index, master, false);
  }
}

// A state of the three panes. The center changes it with each edit; the sides receive it.
export const setStates = StateEffect.define<readonly BlockState[]>();

// The name of the command, like that of `executeMergeCommand`.
export const mergeCommand = Annotation.define<string>();

export const paneFacet = Facet.define<MergePane, MergePane>({ combine: (values) => values[0] });
export const baseFacet = Facet.define<Text, Text | null>({ combine: (values) => values[0] ?? null });

export const statesField = StateField.define<readonly BlockState[]>({
  create: () => [],
  update(states, transaction) {
    // When undoing several joined edits several states arrive: the last one is the oldest.
    const given = transaction.effects.filter((effect) => effect.is(setStates)).at(-1);
    if (given) return given.value as readonly BlockState[];
    const base = transaction.state.facet(baseFacet);
    if (!transaction.docChanged || !base) return states;
    return mapStates(states, transaction.changes, transaction.startState.doc, transaction.state.doc, base);
  },
});
