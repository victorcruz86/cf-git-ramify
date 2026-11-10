import { BASE_VERSION, YOUR_VERSION } from "../changes/tab";
import { LOCAL_REVISION, type FileSides, type RevisionSides, type Side, type WorktreeSides } from "../lib/api";

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

// `DB:41`, `DB:42`, `DB:46-47`, `DB:273`.
export const IDENTICAL_TEXT = "Contents are identical";
export const TOO_LARGE_TEXT = "Files are too large to compare";
export const BINARY_IDENTICAL_TEXT = "Files contents are identical";
export const BINARY_DIFFERENT_TEXT = "Files contents are different";
export const PREVIEW_TEXT = "File is too large. Only preview is loaded.";

// Two identical text sides: a mode change or a rename without edits. Without `against`, the
// comparison of two revisions, with 241's text.
export function unchangedText(sides: FileSides, against: string | null): string | null {
  if (sides.old.kind !== "text" || sides.new.kind !== "text") return null;
  if (sides.old.content !== sides.new.content) return null;
  return against === null ? IDENTICAL_TEXT : `No changes compared to ${against}`;
}

// What two revisions show instead of the editors: two previews cannot be compared, and a binary
// side is compared by its bytes, as `BinaryDiffViewer` does.
export function revisionsMessage(sides: RevisionSides): string | null {
  if (sides.oldPreview && sides.newPreview) return TOO_LARGE_TEXT;
  if (sides.identical !== null) {
    return sides.identical ? BINARY_IDENTICAL_TEXT : BINARY_DIFFERENT_TEXT;
  }
  return sideMessage(sides.old) ?? sideMessage(sides.new);
}

// `GitSubmoduleDiffRequestProvider`: each side is titled by its revision number, which is the full
// hash of the commit, and the default one when it has none (`ChangeDiffRequestProducer`). In Local
// Changes the left side is the parent's HEAD, and the local side has no revision.
export function submoduleTitle(revision: string | null, fallback: string): string {
  return revision && revision !== LOCAL_REVISION ? revision : fallback;
}

export const submoduleLeftTitle = (revision: string | null) =>
  submoduleTitle(revision, BASE_VERSION);
export const submoduleRightTitle = (revision: string | null) =>
  submoduleTitle(revision, YOUR_VERSION);

export function sameSides(
  left: WorktreeSides | RevisionSides,
  right: WorktreeSides | RevisionSides,
): boolean {
  return (
    left.hash === right.hash &&
    left.editable === right.editable &&
    left.conflicted === right.conflicted &&
    left.submodule === right.submodule &&
    sideText(left.old) === sideText(right.old) &&
    sideText(left.new) === sideText(right.new) &&
    left.old.kind === right.old.kind &&
    left.new.kind === right.new.kind &&
    left.hunks.length === right.hunks.length &&
    left.hunks.every((hunk, index) => hunk.id === right.hunks[index].id) &&
    sameRevisionFields(left, right)
  );
}

// Two revisions also differ by what is cut or binary: a binary has no text to tell them apart.
function sameRevisionFields(left: WorktreeSides, right: WorktreeSides): boolean {
  const [a, b] = [left as Partial<RevisionSides>, right as Partial<RevisionSides>];
  return (
    a.identical === b.identical && a.oldPreview === b.oldPreview && a.newPreview === b.newPreview
  );
}
