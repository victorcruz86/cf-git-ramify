import type { Hunk, LineSelection } from "../lib/api";

export type Side = "old" | "new";
export type Check = "on" | "off" | "partial";

// A margin checkbox: a null `line` is the one of the whole hunk.
export interface MarginCheck {
  hunk: Hunk;
  line: number | null;
  checked: boolean;
}

const totalLines = (hunk: Hunk) => hunk.oldLines + hunk.newLines;
const indexes = new WeakMap<LineSelection[], Map<string, LineSelection>>();

function entryOf(selection: LineSelection[], hunk: Hunk): LineSelection | undefined {
  let index = indexes.get(selection);
  if (!index) {
    index = new Map(selection.map((entry) => [entry.hunk, entry]));
    indexes.set(selection, index);
  }
  return index.get(hunk.id);
}

const without = (selection: LineSelection[], hunk: Hunk) =>
  selection.filter((entry) => entry.hunk !== hunk.id);

export function hunkCheck(selection: LineSelection[], hunk: Hunk): Check {
  const entry = entryOf(selection, hunk);
  if (!entry) return "off";
  if (entry.lines === null || entry.lines.length >= totalLines(hunk)) return "on";
  return entry.lines.length === 0 ? "off" : "partial";
}

export function lineChecked(selection: LineSelection[], hunk: Hunk, line: number): boolean {
  const entry = entryOf(selection, hunk);
  return entry !== undefined && (entry.lines === null || entry.lines.includes(line));
}

function allLines(hunk: Hunk): number[] {
  return Array.from({ length: totalLines(hunk) }, (_, line) => line);
}

// It is replaced in place so the order of the selection does not depend on the clicks.
function withEntry(selection: LineSelection[], hunk: Hunk, lines: number[] | null) {
  const entry = { hunk: hunk.id, lines };
  return entryOf(selection, hunk)
    ? selection.map((current) => (current.hunk === hunk.id ? entry : current))
    : [...selection, entry];
}

export function toggleLine(
  selection: LineSelection[],
  hunk: Hunk,
  line: number,
): LineSelection[] {
  const entry = entryOf(selection, hunk);
  const current = entry ? (entry.lines ?? allLines(hunk)) : [];
  const next = current.includes(line)
    ? current.filter((other) => other !== line)
    : [...current, line].sort((left, right) => left - right);
  if (next.length === 0) return without(selection, hunk);
  return withEntry(selection, hunk, next.length === totalLines(hunk) ? null : next);
}

export function toggleHunk(selection: LineSelection[], hunk: Hunk): LineSelection[] {
  return hunkCheck(selection, hunk) === "on"
    ? without(selection, hunk)
    : withEntry(selection, hunk, null);
}

// What the margin shows. A file that is included without a selection of its own is committed whole.
export function effectiveSelection(
  hunks: Hunk[],
  selection: LineSelection[],
  included: boolean,
): LineSelection[] {
  if (!included) return [];
  if (selection.length > 0) return selection;
  return hunks.map((hunk) => ({ hunk: hunk.id, lines: null }));
}

// What is stored: with nothing checked the file leaves the commit; with everything, it goes whole.
// Only a part is a selection that can be lost if the file changes.
export function normalizeSelection(
  hunks: Hunk[],
  selection: LineSelection[],
): { selection: LineSelection[]; included: boolean } {
  if (selection.length === 0) return { selection, included: false };
  const whole = hunks.every((hunk) => hunkCheck(selection, hunk) === "on");
  return { selection: whole ? [] : selection, included: true };
}

export function fileCheck(hunks: Hunk[], selection: LineSelection[]): Check {
  const states = new Set(hunks.map((hunk) => hunkCheck(selection, hunk)));
  if (states.size === 1 && !states.has("partial")) return states.has("on") ? "on" : "off";
  return states.size === 0 ? "off" : "partial";
}

const startOf = (hunk: Hunk, side: Side) => (side === "old" ? hunk.oldStart : hunk.newStart);
const linesOf = (hunk: Hunk, side: Side) => (side === "old" ? hunk.oldLines : hunk.newLines);
const offsetOf = (hunk: Hunk, side: Side) => (side === "old" ? 0 : hunk.oldLines);

// The checkboxes of one side, per document line, like SimpleLocalChangeListDiffViewer: a single
// checkbox on the local side if the hunk goes whole or does not go; one per line, each on its side,
// if it goes partially; and the one of the line under the mouse, which allows starting to choose by
// lines.
export function marginFor(
  hunks: Hunk[],
  selection: LineSelection[],
  side: Side,
  hover: { side: Side; line: number } | null,
): Map<number, MarginCheck> {
  const margin = new Map<number, MarginCheck>();
  for (const hunk of hunks) {
    const state = hunkCheck(selection, hunk);
    if (state === "partial") {
      for (let index = 0; index < linesOf(hunk, side); index++) {
        const line = offsetOf(hunk, side) + index;
        margin.set(startOf(hunk, side) + index, {
          hunk,
          line,
          checked: lineChecked(selection, hunk, line),
        });
      }
      continue;
    }
    if (side === "new") margin.set(hunk.newStart, { hunk, line: null, checked: state === "on" });
  }
  if (hover?.side === side && !margin.has(hover.line)) {
    const hunk = hunks.find(
      (candidate) =>
        hover.line >= startOf(candidate, side) &&
        hover.line < startOf(candidate, side) + linesOf(candidate, side),
    );
    if (hunk) {
      margin.set(hover.line, {
        hunk,
        line: offsetOf(hunk, side) + hover.line - startOf(hunk, side),
        checked: hunkCheck(selection, hunk) === "on",
      });
    }
  }
  return margin;
}

export function hunksAt(hunks: Hunk[], side: Side, from: number, to: number): Hunk[] {
  return hunks.filter((hunk) => {
    const start = startOf(hunk, side);
    return start <= to && start + linesOf(hunk, side) > from;
  });
}

// With the cursor the whole hunk applies, like `tracker.rollbackChanges`. A selection chooses those
// lines, each with its pair in the same position, because together they form the change of that
// line.
export function linesSelection(
  hunks: Hunk[],
  side: Side,
  from: number,
  to: number,
  exact: boolean,
): LineSelection[] {
  return hunksAt(hunks, side, from, to).map((hunk) => {
    const first = Math.max(from, startOf(hunk, side)) - startOf(hunk, side);
    const last = Math.min(to, startOf(hunk, side) + linesOf(hunk, side) - 1) - startOf(hunk, side);
    if (!exact || (first === 0 && last === linesOf(hunk, side) - 1)) {
      return { hunk: hunk.id, lines: null };
    }
    const lines = new Set<number>();
    for (let position = first; position <= last; position++) {
      if (position < hunk.oldLines) lines.add(position);
      if (position < hunk.newLines) lines.add(hunk.oldLines + position);
    }
    return { hunk: hunk.id, lines: [...lines].sort((left, right) => left - right) };
  });
}

// To say what was lost. Lines count from 1, as in the margin.
export function hunkPlace(hunk: Hunk): string {
  if (hunk.newLines === 0) return `deleted at line ${hunk.newStart + 1}`;
  if (hunk.newLines === 1) return `line ${hunk.newStart + 1}`;
  return `lines ${hunk.newStart + 1}–${hunk.newStart + hunk.newLines}`;
}
