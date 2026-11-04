import type { Hunk } from "../lib/api";
import type { Side } from "./gutter";

export type ChangeType = "inserted" | "deleted" | "modified";
export type IgnorePolicy = "none" | "trim" | "whitespaces" | "emptyLines";

export interface FoldRun {
  old: number;
  new: number;
  lines: number;
}

// The rule of TextDiffType: what exists on only one side is added or deleted.
export function changeType(hunk: Hunk): ChangeType {
  if (hunk.oldLines === 0) return "inserted";
  return hunk.newLines === 0 ? "deleted" : "modified";
}

export function textLines(text: string): number {
  if (text === "") return 0;
  const breaks = text.split("\n").length - 1;
  return text.endsWith("\n") ? breaks : breaks + 1;
}

// A file treated as a single piece (deleted, link, submodule): the change is drawn whole, but
// without an id it cannot be chosen or undone by lines.
export function wholeFileHunk(oldText: string, newText: string): Hunk | null {
  if (oldText === newText) return null;
  return {
    id: "",
    oldStart: 0,
    oldLines: textLines(oldText),
    newStart: 0,
    newLines: textLines(newText),
  };
}

const MIN_FOLD_LINES = 2;

// The unchanged stretches that get folded, the same on both sides, leaving `context` lines next to
// each change. With no changes nothing is folded: the whole file would have to be shown.
export function foldRuns(
  hunks: Hunk[],
  oldCount: number,
  newCount: number,
  context: number,
): FoldRun[] {
  if (hunks.length === 0) return [];
  const runs: FoldRun[] = [];
  let oldLine = 0;
  let newLine = 0;
  const push = (oldEnd: number, skipStart: number, skipEnd: number) => {
    const start = oldLine + skipStart;
    const lines = oldEnd - skipEnd - start;
    if (lines >= MIN_FOLD_LINES) runs.push({ old: start, new: newLine + skipStart, lines });
  };
  hunks.forEach((hunk, index) => {
    push(hunk.oldStart, index === 0 ? 0 : context, context);
    oldLine = hunk.oldStart + hunk.oldLines;
    newLine = hunk.newStart + hunk.newLines;
  });
  const tail = Math.min(oldCount - oldLine, newCount - newLine);
  push(oldLine + tail, context, 0);
  return runs;
}

// The line on one side that corresponds to one on the other: it is shifted outside the changes and
// interpolated inside them, so the other side advances smoothly when scrolling through a hunk.
export function mapLine(hunks: Hunk[], line: number, side: Side): number {
  let delta = 0;
  for (const hunk of hunks) {
    const [start, lines, target, targetLines] =
      side === "old"
        ? [hunk.oldStart, hunk.oldLines, hunk.newStart, hunk.newLines]
        : [hunk.newStart, hunk.newLines, hunk.oldStart, hunk.oldLines];
    if (line < start) break;
    if (line < start + lines) {
      return target + Math.round(((line - start) / lines) * targetLines);
    }
    delta = target + targetLines - (start + lines);
  }
  return line + delta;
}

const NORMALIZE: Record<Exclude<IgnorePolicy, "none">, (line: string) => string> = {
  trim: (line) => line.trim(),
  whitespaces: (line) => line.replace(/\s+/g, ""),
  emptyLines: (line) => line.replace(/\s+/g, ""),
};

// The whitespace options only change what is drawn: a hidden hunk is still in the file and in the
// commit. That is why whole hunks are removed and the lines of the others are not touched.
export function visibleHunks(
  hunks: Hunk[],
  oldLines: string[],
  newLines: string[],
  policy: IgnorePolicy,
): Hunk[] {
  if (policy === "none") return hunks;
  const normalize = NORMALIZE[policy];
  const clean = (lines: string[]) => {
    const normal = lines.map(normalize);
    return policy === "emptyLines" ? normal.filter((line) => line !== "") : normal;
  };
  return hunks.filter((hunk) => {
    const left = clean(oldLines.slice(hunk.oldStart, hunk.oldStart + hunk.oldLines));
    const right = clean(newLines.slice(hunk.newStart, hunk.newStart + hunk.newLines));
    return left.length !== right.length || left.some((line, index) => line !== right[index]);
  });
}
