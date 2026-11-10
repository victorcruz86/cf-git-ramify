import type {
  ApplyPlan,
  Changelists,
  FileMapping,
  PathMapping,
  PatchInput,
  PlannedFile,
  ShelfFile,
} from "../lib/api";

// What the Apply Patch dialog shows and changes, without the interface: the tree, the legend, the
// inclusion, the strip and base of each file, and what the changelist chooser starts with
// (`ApplyPatchDifferentiatedDialog`, `MyChangeNodeDecorator`, `ChangesLegendCalculator`,
// `CommitLegendPanel`, `AbstractFilePatchInProgress`, `ChangeListChooserPanel`).

// `baseExistsOrAdded`: a file whose target is not there cannot be included, and neither can one the
// patch may not touch.
export const isValid = (file: PlannedFile) => file.status !== "missing" && file.status !== "invalid";

// Ramify's own per-row `Overwrite` of the Unshelve dialog: a binary file that HEAD changed since it
// was shelved, once chosen, is applicable and goes in the answer's `overwrite`.
export function withOverwrites(plan: ApplyPlan, paths: Set<string>): ApplyPlan {
  if (!plan.files.some((file) => file.overwritable && paths.has(file.path))) return plan;
  return {
    ...plan,
    files: plan.files.map((file) =>
      file.overwritable && file.status === "invalid" && paths.has(file.path)
        ? { ...file, status: "applicable", error: null }
        : file,
    ),
  };
}

// `getIncluded(doInitCheck = true)`: every valid file.
export function initialIncluded(plan: ApplyPlan): Set<number> {
  return new Set(plan.files.filter(isValid).map((file) => file.index));
}

// After a new plan, what was included and is still valid (`getIncluded(false)`).
export function keptIncluded(plan: ApplyPlan, included: Set<number>): Set<number> {
  return new Set(plan.files.filter((file) => isValid(file) && included.has(file.index)).map((file) => file.index));
}

// `FilePatchStatus` as the decorator names it (V:398-400): a move is a modification.
export function statusText(file: PlannedFile): string {
  if (file.kind === "added") return "Added";
  if (file.kind === "deleted") return "Deleted";
  return "Modified";
}

// `patch.apply.stripped.description` (V:401).
export const strippedText = (strip: number) => ` stripped ${strip}`;
export const SELECT_MISSING_BASE = "Select missing base"; // V:397
export const NEW_BASE_DETECTED = "New base detected"; // V:396

// `basePathWasChanged`: the target is no longer where the patch put it.
export function baseChanged(file: PlannedFile): boolean {
  return file.kind !== "added" && file.path !== file.original;
}

// `patch.apply.old.new.base.info` (V:402), with a line break for `<br/>`.
export function baseTooltip(file: PlannedFile, repo: string, base: string): string {
  const now = base ? `${repo}/${base}` : repo;
  return `was: ${file.original} (base dir: ${repo})\nnow: ${file.before ?? file.path} (base dir: ${now})`;
}

// The text of a file's row, as the decorator appends it to the name.
export function rowText(file: PlannedFile): string {
  let text = fileName(file.path);
  if (file.strip > 0) text += strippedText(file.strip);
  text += `   ${statusText(file)}`;
  if (file.status === "missing") text += `  ${SELECT_MISSING_BASE}`;
  else if (file.status === "invalid") text += `  ${file.error ?? ""}`;
  else if (baseChanged(file)) text += `  ${NEW_BASE_DETECTED}`;
  return text;
}

export const fileName = (path: string) => path.slice(path.lastIndexOf("/") + 1);

// `commit.legend.*` (V:292-294) of the included files and `patch.apply.missing.base.file.label`
// (V:390) of all of them, three spaces apart, each only when there is one.
export function legendText(files: PlannedFile[], included: Set<number>): string {
  const count = (kind: (file: PlannedFile) => boolean) =>
    files.filter((file) => included.has(file.index) && kind(file)).length;
  const format = (value: number) => value.toLocaleString("en-US");
  const parts: string[] = [];
  const added = count((file) => file.kind === "added");
  const modified = count((file) => file.kind === "modified" || file.kind === "moved");
  const deleted = count((file) => file.kind === "deleted");
  const missing = files.filter((file) => file.status === "missing").length;
  if (added > 0) parts.push(`${format(added)} added`);
  if (modified > 0) parts.push(`${format(modified)} modified`);
  if (deleted > 0) parts.push(`${format(deleted)} deleted`);
  if (missing > 0) parts.push(`${format(missing)} missing base`);
  return parts.join("   ");
}

//
// The tree
//

export interface ApplyRow {
  id: string;
  kind: "folder" | "file";
  name: string;
  depth: number;
  // The files under it, by index.
  indexes: number[];
  file: PlannedFile | null;
}

interface Folder {
  folders: Map<string, Folder>;
  files: PlannedFile[];
}

const collator = new Intl.Collator("en", { numeric: true, sensitivity: "base" });

// Grouped by directory as the changes trees are: folders first, a folder with a single subfolder and
// no files joined with it, then the files by name.
export function applyRows(files: PlannedFile[]): ApplyRow[] {
  const root: Folder = { folders: new Map(), files: [] };
  for (const file of files) {
    const parts = file.path.split("/");
    parts.pop();
    let folder = root;
    for (const part of parts) {
      let next = folder.folders.get(part);
      if (!next) {
        next = { folders: new Map(), files: [] };
        folder.folders.set(part, next);
      }
      folder = next;
    }
    folder.files.push(file);
  }
  const rows: ApplyRow[] = [];
  const indexesOf = (folder: Folder): number[] => [
    ...[...folder.folders.values()].flatMap(indexesOf),
    ...folder.files.map((file) => file.index),
  ];
  const visit = (folder: Folder, prefix: string, depth: number) => {
    const folders = [...folder.folders.entries()].map(([name, child]) => {
      let label = name;
      let current = child;
      while (current.files.length === 0 && current.folders.size === 1) {
        const [[nextName, next]] = current.folders;
        label = `${label}/${nextName}`;
        current = next;
      }
      return { label, current };
    });
    folders.sort((left, right) => collator.compare(left.label, right.label));
    for (const { label, current } of folders) {
      const path = prefix + label;
      rows.push({ id: `dir:${path}`, kind: "folder", name: label, depth, indexes: indexesOf(current), file: null });
      visit(current, `${path}/`, depth + 1);
    }
    const sorted = [...folder.files].sort((left, right) => collator.compare(fileName(left.path), fileName(right.path)));
    for (const file of sorted) {
      rows.push({ id: `file:${file.index}`, kind: "file", name: fileName(file.path), depth, indexes: [file.index], file });
    }
  };
  visit(root, "", 0);
  return rows;
}

//
// Change Directory Paths
//

function withFiles(mapping: PathMapping, files: PlannedFile[], change: (file: PlannedFile, current: FileMapping) => FileMapping): PathMapping {
  const size = Math.max(mapping.files.length, ...files.map((file) => file.index + 1));
  const result: FileMapping[] = Array.from({ length: size }, (_, index) => mapping.files[index] ?? { strip: 0, base: "" });
  for (const file of files) result[file.index] = change(file, result[file.index]);
  return { files: result };
}

// `StripUp`, `ZeroStrip`, `StripDown` and `ResetStrip` on the selected files.
export type StripAction = "up" | "all" | "down" | "reset";

export function canStrip(action: StripAction, files: PlannedFile[]): boolean {
  if (action === "up" || action === "all") return files.some((file) => file.strip < file.maxStrip);
  return files.some((file) => file.strip > 0);
}

export function strip(mapping: PathMapping, files: PlannedFile[], action: StripAction): PathMapping {
  return withFiles(mapping, files, (file, current) => {
    const next = {
      up: Math.min(file.strip + 1, file.maxStrip),
      all: file.maxStrip,
      down: Math.max(file.strip - 1, 0),
      reset: 0,
    }[action];
    return { ...current, strip: next };
  });
}

// `MapDirectory` and `NewBaseSelector`: the chosen folder, relative to the repository, as the base
// of the selected files. A folder outside it is refused (decision 5 of the plan).
export function relativeTo(repo: string, folder: string): string | null {
  const root = repo.replace(/\/+$/, "");
  const chosen = folder.replace(/\/+$/, "");
  if (chosen === root) return "";
  return chosen.startsWith(`${root}/`) ? chosen.slice(root.length + 1) : null;
}

export function mapBase(mapping: PathMapping, files: PlannedFile[], base: string): PathMapping {
  return withFiles(mapping, files, (_, current) => ({ ...current, base }));
}

// `Select missing base` with a file: its folder as the base and only the file name left of the
// patch's path, as `NewBaseSelector(false)` does (Ramify does not rename the patch's file to the one
// chosen).
export function missingBase(mapping: PathMapping, file: PlannedFile, base: string): PathMapping {
  return withFiles(mapping, [file], () => ({ strip: file.maxStrip, base }));
}

//
// The changelist chooser
//

// `LightVirtualFile("clipboardPatchFile")` for the clipboard, otherwise the file's name.
export function patchFileName(source: PatchInput): string {
  if (source.kind === "shelf") return source.name;
  return source.kind === "file" ? fileName(source.path) : "clipboardPatchFile";
}

// The unshelve mode's list (`getPredefinedChangeList`, then `setSuggestedName` without
// `CREATE_CHANGELISTS_AUTOMATICALLY`): the first existing list with one of `names`, else the
// active one.
export function unshelveList(lists: Changelists, names: string[]): ListDraft {
  const named = names.map((name) => lists.lists.find((list) => list.name === name)).find(Boolean);
  const list = named ?? lists.lists.find((item) => item.id === lists.active) ?? lists.lists[0];
  return { name: list?.name ?? "", comment: list?.comment ?? "", typed: false };
}

// The files a plan starts with: every valid one, or only the preselected ones that are valid.
export function preselectedIncluded(plan: ApplyPlan, preselected: ShelfFile[] | null | undefined): Set<number> {
  if (!preselected || preselected.length === 0) return initialIncluded(plan);
  const key = (file: ShelfFile) => `${file.binary}\0${file.before ?? ""}\0${file.after ?? ""}`;
  const chosen = new Set(preselected.map(key));
  return new Set(
    plan.files
      .filter((file) => isValid(file) && file.shelf && chosen.has(key(file.shelf)))
      .map((file) => file.index),
  );
}

export interface ListDraft {
  name: string;
  comment: string;
  // `myLastTypedDescription`: a comment that stays when another list is chosen.
  typed: boolean;
}

// `ChangeListChooserPanel.setDefaultSelection(null)` and `reloadPatchFromFile`: the active list
// with its comment; the patch's message as the comment; without one, the list named like the file
// (`_` as a space), and no new list (`CREATE_CHANGELISTS_AUTOMATICALLY` is off).
export function initialList(plan: ApplyPlan, lists: Changelists, source: PatchInput): ListDraft {
  const active = lists.lists.find((list) => list.id === lists.active) ?? lists.lists[0];
  const draft: ListDraft = { name: active?.name ?? "", comment: active?.comment ?? "", typed: false };
  if (plan.message) return { ...draft, comment: plan.message, typed: true };
  const name = patchFileName(source);
  const suggested = name.replace(/\.[^.]*$/, "").replace(/_/g, " ").trim();
  const existing = lists.lists.find((list) => list.name === suggested);
  return existing ? { name: existing.name, comment: existing.comment, typed: false } : draft;
}

// `updateDescription`: choosing a name shows that list's comment, unless one was typed.
export function renamedList(draft: ListDraft, lists: Changelists, name: string): ListDraft {
  if (draft.typed) return { ...draft, name };
  const existing = lists.lists.find((list) => list.name === name);
  return { name, comment: existing ? existing.comment : "", typed: false };
}

export const EMPTY_NAME = "Cannot create new changelist with empty name"; // V:553
