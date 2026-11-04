import type { ChangedFile, ChangeGroup, PathsSelection } from "../lib/api";

// A file in the list and what is compared when it is opened. Without `base`, the old side does not exist.
export interface LogChange {
  file: ChangedFile;
  hash: string;
  base: string | null;
}

export interface ChangeSection {
  // The "Changes to …" node. Without it, the commit's own changes.
  title: string | null;
  folders: { name: string; changes: LogChange[] }[];
}

const kindOf = (file: ChangedFile) => file.status[0];

function zippedStatus(before: string, after: string, moved: boolean): string {
  if (before === "A") return "A";
  if (before === "D") return "M";
  if (after === "D") return "D";
  return moved ? "R" : "M";
}

// `CommittedChangesTreeBrowser.zipChanges`: what happens to a file across several commits, from oldest to
// newest, is a single change. What is created and deleted inside the selection disappears.
export function zipChanges(commits: { hash: string; group: ChangeGroup }[]): LogChange[] {
  const zipped = new Map<string, LogChange>();
  for (const { hash, group } of commits) {
    for (const file of group.files) {
      const known = zipped.get(file.oldPath ?? file.path);
      if (!known) {
        zipped.set(file.path, { file, hash, base: group.base });
        continue;
      }
      zipped.delete(file.oldPath ?? file.path);
      const [before, after] = [kindOf(known.file), kindOf(file)];
      if (before === "A" && after === "D") continue;
      const origin = before === "A" ? null : (known.file.oldPath ?? file.oldPath);
      const moved = origin !== null && origin !== file.path;
      const change = {
        path: file.path,
        oldPath: moved ? origin : null,
        status: zippedStatus(before, after, moved),
      };
      zipped.set(file.path, { file: change, hash, base: known.base });
    }
  }
  return [...zipped.values()];
}

// `FileHistoryUtil.affectsFile` and `affectsDirectory`.
export function affectsPaths(file: ChangedFile, paths: string[]): boolean {
  const touches = (path: string, filter: string) =>
    path === filter || path.startsWith(`${filter.replace(/\/+$/, "")}/`);
  return paths.some(
    (filter) => touches(file.path, filter) || (file.oldPath && touches(file.oldPath, filter)),
  );
}

const directory = (path: string) => path.slice(0, Math.max(0, path.lastIndexOf("/")));

export const shortName = (path: string) => path.slice(path.lastIndexOf("/") + 1);

export function sectionOf(title: string | null, changes: LogChange[]): ChangeSection {
  const folders = new Map<string, LogChange[]>();
  const sorted = [...changes].sort((a, b) => (a.file.path < b.file.path ? -1 : 1));
  for (const change of sorted) {
    const name = directory(change.file.path);
    folders.set(name, [...(folders.get(name) ?? []), change]);
  }
  return { title, folders: [...folders].map(([name, changes]) => ({ name, changes })) };
}

// `VcsLogUtil.getSizeText`.
export function sizeText(size: number): string {
  const floor = (value: number) => String(Math.floor(value * 10) / 10);
  if (size < 1000) return String(size);
  if (size < 10_000) return `${floor(size / 1000)}K`;
  if (size < 1_000_000) return `${Math.floor(size / 1000)}K`;
  if (size < 10_000_000) return `${floor(size / 1_000_000)}M`;
  return `${Math.floor(size / 1_000_000)}M`;
}

// What `git diff` needs to apply or revert those files: the commit and the base of each one,
// with the two paths of a rename. A file does not enter twice (a merge shows it under
// each parent).
export function pathsSelection(changes: LogChange[]): PathsSelection[] {
  const groups: PathsSelection[] = [];
  const seen = new Set<string>();
  for (const { file, hash, base } of changes) {
    const paths = [file.path, ...(file.oldPath ? [file.oldPath] : [])].filter(
      (path) => !seen.has(path),
    );
    if (paths.length === 0) continue;
    paths.forEach((path) => seen.add(path));
    const group = groups.find((known) => known.hash === hash && known.base === base);
    if (group) group.paths.push(...paths);
    else groups.push({ hash, base, paths });
  }
  return groups;
}
