// The change stripe of a diff editor: `DiffDrawUtil.LineHighlighterBuilder` gives every changed
// block with a background a highlighter on `STRIPE_LAYER`, coloured by
// `DiffStripeMarkerTextAttributes`. Ramify draws it in its own lane (plan decisions 1-3).

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
    .filter((block) => block.end > block.start)
    .map((block) => {
      const height = Math.max(least, (block.end - block.start) / lineCount);
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
