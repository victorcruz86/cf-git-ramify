// The change stripe of a diff editor: `DiffDrawUtil.LineHighlighterBuilder` gives every changed
// block with a background a highlighter on `STRIPE_LAYER`, coloured by
// `DiffStripeMarkerTextAttributes`. Ramify draws it in its own lane (plan decisions 1-3).
import type { Hunk } from "../lib/api";
import { changeType } from "./blocks";
import type { Side } from "./gutter";

export type StripeKind = "inserted" | "deleted" | "modified" | "conflict";

export interface StripeBlock {
  // 0-based lines of that side, end exclusive.
  start: number;
  end: number;
  kind: StripeKind;
}

export interface StripeMark {
  // Fractions of the lane, from 0 to 1.
  top: number;
  height: number;
  kind: StripeKind;
  line: number;
}

// In points: a one-line change in a long file stays visible.
export const MIN_MARK_HEIGHT = 3;

// The heights follow the document's lines, not their pixel heights (plan decision 3).
export function stripeMarks(blocks: StripeBlock[], lineCount: number, laneHeight: number): StripeMark[] {
  if (lineCount <= 0 || laneHeight <= 0) return [];
  const least = Math.min(1, MIN_MARK_HEIGHT / laneHeight);
  return blocks
    .filter((block) => block.end > block.start && block.start < lineCount)
    .map((block) => {
      // Between a keystroke and the new hunks, a block can reach past the last line.
      const end = Math.min(block.end, lineCount);
      const height = Math.max(least, (end - block.start) / lineCount);
      const top = Math.min(block.start / lineCount, 1 - height);
      return { top: Math.max(0, top), height, kind: block.kind, line: block.start };
    });
}

// The line at a height of the lane, for a click.
export function lineAtLane(y: number, laneHeight: number, lineCount: number): number {
  if (laneHeight <= 0 || lineCount <= 0) return 0;
  const line = Math.floor((y / laneHeight) * lineCount);
  return Math.min(lineCount - 1, Math.max(0, line));
}

// The blocks one editor marks. `DiffDrawUtil.LineHighlighterBuilder.done`: an empty range has no
// background but still gets its stripe highlighter, so a side with no lines for a change marks it
// at its place, one line tall. The unified view does the same for lines only in the other version
// (plan decision 7: not as tall as those lines).
export function stripeBlocks(hunks: Hunk[], side: Side, unified: boolean, lineCount: number): StripeBlock[] {
  const blocks: StripeBlock[] = [];
  for (const hunk of hunks) {
    const start = side === "old" ? hunk.oldStart : hunk.newStart;
    const lines = side === "old" ? hunk.oldLines : hunk.newLines;
    const kind = changeType(hunk);
    if (lines > 0) blocks.push({ start, end: start + lines, kind });
    else if (lineCount > 0) {
      const at = Math.min(start, lineCount - 1);
      blocks.push({ start: at, end: at + 1, kind });
    }
  }
  return blocks;
}
