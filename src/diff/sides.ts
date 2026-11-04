import type { FileSides, Side, WorktreeSides } from "../lib/api";

const BYTES_IN_MB = 1024 * 1024;

export function sideMessage(side: Side): string | null {
  switch (side.kind) {
    case "binary":
      return "Binary file";
    case "tooLarge":
      return `File is too large to show (${(side.bytes / BYTES_IN_MB).toFixed(1)} MB)`;
    default:
      return null;
  }
}

export function sideText(side: Side): string {
  return side.kind === "text" ? side.content : "";
}

// Two identical text sides: a mode change or a rename without edits.
export function unchangedText(sides: FileSides, against: string): string | null {
  if (sides.old.kind !== "text" || sides.new.kind !== "text") return null;
  return sides.old.content === sides.new.content ? `No changes compared to ${against}` : null;
}

export function sameSides(left: WorktreeSides, right: WorktreeSides): boolean {
  return (
    left.hash === right.hash &&
    left.editable === right.editable &&
    left.conflicted === right.conflicted &&
    sideText(left.old) === sideText(right.old) &&
    sideText(left.new) === sideText(right.new) &&
    left.old.kind === right.old.kind &&
    left.new.kind === right.new.kind &&
    left.hunks.length === right.hunks.length &&
    left.hunks.every((hunk, index) => hunk.id === right.hunks[index].id)
  );
}
