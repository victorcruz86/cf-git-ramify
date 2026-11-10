import type { ViewerContent, ViewerTarget } from "../lib/api";

// `GitRevisionNumber.toShortString` = `DvcsUtil.getShortHash`.
export const SHORT_HASH = 8;
// What `AbstractVcsVirtualFile` shows as its revision when the content could not be loaded.
const FAILED_REVISION = "0";

export interface ViewerTab {
  key: string;
  path: string;
  revision: string | null;
  // Null while it loads.
  content: ViewerContent | null;
  // The load failed: the revision shows as `0` and the text is empty.
  failed: boolean;
  // A local file's `viewer_stamp` when it was read: `repo-changed` reads it again only if it moved.
  stamp: string | null;
}

export type FileRef = Pick<ViewerTarget, "path" | "revision">;

// A path and a revision are one tab: opening them again goes to it.
export const tabKey = ({ path, revision }: FileRef) => `${revision ?? ""}\0${path}`;

export const fileName = (path: string) => path.slice(path.lastIndexOf("/") + 1);

// `AbstractVcsVirtualFile.getPresentableName`: `<name> (<revision>)`; a local file is its name.
export function tabTitle({ path, revision, failed }: FileRef & { failed?: boolean }): string {
  const name = fileName(path);
  if (revision === null) return name;
  return `${name} (${failed ? FAILED_REVISION : revision.slice(0, SHORT_HASH)})`;
}

// With `UISettings.openTabsAtTheEnd` off (its default), a new tab goes right after the active one.
export function withTarget(
  tabs: ViewerTab[],
  active: string | null,
  target: FileRef,
): { tabs: ViewerTab[]; key: string } {
  const key = tabKey(target);
  if (tabs.some((tab) => tab.key === key)) return { tabs, key };
  const tab: ViewerTab = {
    key,
    path: target.path,
    revision: target.revision,
    content: null,
    failed: false,
    stamp: null,
  };
  const at = tabs.findIndex((candidate) => candidate.key === active);
  const next = [...tabs];
  next.splice(at < 0 ? tabs.length : at + 1, 0, tab);
  return { tabs: next, key };
}

// Closing the active tab activates the one on its left, or the first one if it was first
// (`UISettings.activeRightEditorOnClose` and `activeMruEditorOnClose` off).
export function withoutTab(
  tabs: ViewerTab[],
  active: string | null,
  key: string,
): { tabs: ViewerTab[]; active: string | null } {
  const at = tabs.findIndex((tab) => tab.key === key);
  if (at < 0) return { tabs, active };
  const next = tabs.filter((tab) => tab.key !== key);
  if (active !== key) return { tabs: next, active };
  return { tabs: next, active: next[Math.max(at - 1, 0)]?.key ?? null };
}

const UNITS = ["B", "kB", "MB", "GB", "TB", "PB", "EB"];

// `StringUtilRt.formatFileSize`: metric units (1 kB = 1000 B) and `DecimalFormat("0.##")`, whose
// `HALF_EVEN` rounds an exact half to even where `toFixed` rounds it up. Always with a dot.
export function formatFileSize(size: number): string {
  if (size === 0) return "0 B";
  const rank = Math.floor((Math.log10(size) + 0.0000021714778384307465) / 3);
  const value = size / Math.pow(1000, rank);
  const scaled = value * 100;
  let rounded = Number(value.toFixed(2));
  if (!Number.isInteger(scaled) && Number.isInteger(scaled * 2)) {
    const down = Math.floor(scaled);
    rounded = (down % 2 === 0 ? down : down + 1) / 100;
  }
  return `${rounded} ${UNITS[rank]}`;
}

// `large.file.preview.notification` (`IB:1970`), as `LargeFileNotificationProvider` fills it.
export const previewText = (size: number, shown: number) =>
  `The file is too large (${formatFileSize(size)}). Showing a read-only preview of the first ${formatFileSize(shown)}.`;

// Over the limit, `binary.file.too.large` (`IB:1968`); under it, Ramify's own text, since IntelliJ
// would open an image or archive editor that Ramify does not have.
export function binaryText(path: string, size: number, tooLarge: boolean): string {
  if (tooLarge) {
    return `Binary file ${fileName(path)} is too large (${formatFileSize(size)})`;
  }
  return `${fileName(path)} is a binary file (${formatFileSize(size)}).`;
}

// Ramify's own text: Edit Source skips such a file, so it is one deleted after its tab opened.
export function absentText({ revision }: FileRef): string {
  if (revision === null) return "The file is no longer on disk.";
  return `The file does not exist in ${revision.slice(0, SHORT_HASH)}.`;
}
