import type { Hunk } from "../lib/api";

// The spacers that keep the two columns of an aligned diff level when lines wrap, as
// AlignedDiffModel.updateInlayHeights does with its inlays and @codemirror/merge's updateSpacers
// with its widgets.

export type SpacerSide = "left" | "right";

// A line on each side where the two columns must be level: a hunk's start, its end, and the end of
// the document. Lines are 0-based; the end of the document is the number of lines of each side.
export interface Boundary {
  left: number;
  right: number;
}

// The top of each boundary on one side, in px, counting only text: that side's own spacers are
// subtracted. Measuring this way, the spacers never feed back into what they are computed from.
export type Measured = readonly number[];

export interface Spacer {
  side: SpacerSide;
  line: number;
  height: number;
}

// For each boundary, the spacer that levels it, on the side that is behind, or none.
export type Spacers = (Spacer | null)[];

// Walks the boundaries in order: each spacer moves everything below it, so the next difference is
// measured after it. Before a hunk this is IntelliJ's top inlay (the unchanged text since the last
// hunk wrapped differently), after it the bottom inlay (the body is taller on one side), and the
// last one levels the end of the document.
export function alignSpacers(
  boundaries: readonly Boundary[],
  left: Measured,
  right: Measured,
  tolerance = 1,
): Spacers {
  let leftOffset = 0;
  let rightOffset = 0;
  return boundaries.map((boundary, index) => {
    const difference = left[index] + leftOffset - (right[index] + rightOffset);
    if (difference > tolerance) {
      rightOffset += difference;
      return { side: "right", line: boundary.right, height: difference };
    }
    if (difference < -tolerance) {
      leftOffset -= difference;
      return { side: "left", line: boundary.left, height: -difference };
    }
    return null;
  });
}

// The text-only tops of `lines` from the tops a column shows. A spacer goes before the text of its
// line, inside that line's block, so it is above the later lines only; the end of the document
// (`lines >= docLines`) is below all of them.
export function withoutSpacers(
  lines: readonly number[],
  tops: readonly number[],
  own: readonly { line: number; height: number }[],
  docLines: number,
): number[] {
  return lines.map((line, index) => {
    let top = tops[index];
    for (const spacer of own) {
      if (line >= docLines || spacer.line < line) top -= spacer.height;
    }
    return top;
  });
}

// Each hunk's start and end, then the end of the documents, from git's 0-based hunks.
export function boundariesOf(
  hunks: readonly Hunk[],
  leftLines: number,
  rightLines: number,
): Boundary[] {
  const boundaries = hunks.flatMap((hunk) => [
    { left: hunk.oldStart, right: hunk.newStart },
    { left: hunk.oldStart + hunk.oldLines, right: hunk.newStart + hunk.newLines },
  ]);
  return [...boundaries, { left: leftLines, right: rightLines }];
}
