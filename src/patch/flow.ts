import { writable } from "svelte/store";
import {
  changelistsCreate,
  changelistsGet,
  changelistsMove,
  changelistsSetActive,
  changelistsSetComment,
  mergeWindowContents,
  mergeWindowOpenContents,
  notifyRepoChanged,
  onContentsMergeDone,
  patchAppSettingsGet,
  patchApply,
  patchClipboard,
  patchConflictResolve,
  patchCreate,
  patchOnTheFlySet,
  patchPaths,
  patchPlan,
  patchPreview,
  patchRollback,
  patchSettingsGet,
  patchShowInFinderSet,
  patchStorageSet,
  revealInFinder,
  shelfImportToShelf,
  shelfPatchCreate,
  shelfRecycle,
  snapshotsRestore,
  type ApiError,
  type AppPatch,
  type ApplyChoice,
  type ApplyPlan,
  type PatchApplyResult,
  type ApplyStatus,
  type ChangedFile,
  type Changelists,
  type ClipboardPatch,
  type ContentsMergeDone,
  type FileChange,
  type LineSelection,
  type MergeContents,
  type PatchConflict,
  type PatchCreated,
  type PatchDefaults,
  type PatchEntry,
  type PatchInput,
  type PatchOptions,
  type PatchPaths,
  type PatchPreview,
  type PatchResolved,
  type PatchTarget,
  type PathMapping,
  type PlannedFile,
  type ListFiles,
  type ShelfFile,
  type ShelvedList,
} from "../lib/api";
import { chooseFile, chooseFolder, type ChooserOptions } from "../lib/choosers";
import { errorText } from "../lib/errors";
import { notify } from "../lib/notifications";
import { taskResult } from "../lib/tasks";
import { showShelf } from "../shelf/request";
import { saveShelfSetting } from "../shelf/settings";
import { pathOfKey, scopeOfKey } from "../commit/tree";

// Copy as Patch to Clipboard, Create Patch from Local Changes... and Create Patch...
// (`CreatePatchFromChangesAction`, `CreatePatchCommitExecutor`, `CreatePatchConfigurationPanel`,
// `PatchNameChecker`, `ShelveChangesManager.suggestPatchName`). The texts are VcsBundle's (V:n is
// its line), IdeBundle's and IdeCoreBundle's, without mnemonics.
export const COPIED = "Patch copied to clipboard"; // V:432 `patch.copied.to.clipboard`
export const CREATION_FAILED = "Patch Creation Failed"; // V:433 `patch.creation.failed`
// V:343 `create.patch.error.title`.
export const creationError = (message: string) => `Error creating patch: ${message}`;
export const SETTINGS_TITLE = "Patch File Settings"; // V:344
export const CREATED_TITLE = "Create Patch"; // V:341 `create.patch.commit.action.title`
// V:488 `create.patch.success.confirmation`.
export const createdText = (path: string) => `Successfully created patch ${path}`;
export const REVEAL = "Reveal in Finder"; // `action.RevealIn.name.mac`
export const CLOSE = "Close"; // IdeBundle `action.close`
export const DONT_ASK = "Don't ask again"; // IdeCoreBundle `dialog.options.do.not.ask`
export const SAVE_TITLE = "Save Patch File"; // V:434 `patch.creation.save.patch.file.title`
// V:406 `patch.apply.already.exists.overwrite.prompt`.
export const overwriteText = (name: string, parent: string) =>
  `File ${name} (${parent}) already exists.\nDo you want to overwrite it?`;
export const OVERWRITE = "Overwrite"; // CommonBundle `button.overwrite`
export const SAVE_TO_TITLE = "Save Patch to"; // V:436 `patch.creation.save.to.title`
export const NAME_EMPTY = "Name cannot be empty"; // IdeBundle `error.name.cannot.be.empty`
export const NAME_TOO_LONG = "File path should not be too long."; // V:442
export const BASE_EMPTY = "Base path cannot be empty!"; // V:441
export const BASE_MISSING = "Base dir doesn't exist"; // V:440
// V:439 `patch.creation.wrong.base.path.for.changes.error`.
export const baseOutside = (parent: string) =>
  `Base path doesn't contain all selected changes (use ${parent})`;
// IdeBundle `error.file.with.name.already.exists`.
export const fileExists = (file: string) => `A file with the name '${file}' already exists`;
// Ramify's own: a relative path would be taken from the app's working folder.
export const NOT_ABSOLUTE = "The path must be absolute";
// Ramify's own, as the other copies before overwriting say it.
export const overwrittenText = (name: string) => `${name} was overwritten. A snapshot was saved.`;
export const RESTORED = "Snapshot restored.";

// Ramify's own: a file split across lists whose hunks the last refresh could not read.
export function unreadablePatchText(paths: string[]): string {
  const files = paths.map((path) => `"${path}"`).join(", ");
  return `Could not read the changes of ${files}, split across lists: no patch was made`;
}

// `PatchNameChecker.MAX` and `MAX_PATH`.
const MAX_NAME = 100;
const MAX_PATH = 255;
const EXTENSION = "patch";
const UNNAMED = "unnamed";

// The changes of a patch and the repository they are read from: `entries`, or the files of shelved
// lists (`shelf`, the Shelf tab's Create Patch... and Copy as Patch to Clipboard).
export interface PatchChanges {
  repo: string;
  entries: PatchEntry[];
  shelf?: ListFiles[];
}

export interface PatchSettingsRequest {
  file: string;
  toClipboard: boolean;
  base: string;
  // `ChangesUtil.findCommonAncestor` of the changes, for `Base path doesn't contain…`.
  commonParent: string | null;
  // The two fields with `~` expanded, and whether they exist.
  check: (file: string, base: string) => Promise<PatchPaths>;
}

// The fields as they were accepted, with `~` expanded.
export interface PatchSettingsAnswer {
  file: string;
  toClipboard: boolean;
  base: string;
  reverse: boolean;
}

// `Don't ask again` counts with either button, and closing the dialog is `Close`
// (`shouldSaveOptionsOnCancel`).
export interface CreatedAnswer {
  reveal: boolean;
  dontAsk: boolean;
}

export interface PatchUi {
  settings(request: PatchSettingsRequest): Promise<PatchSettingsAnswer | null>;
  overwrite(name: string, parent: string): Promise<boolean>;
  created(path: string): Promise<CreatedAnswer>;
}

export interface PatchApi {
  create(
    entries: PatchEntry[],
    options: PatchOptions,
    to: PatchTarget,
    remember: boolean,
    repo: string,
  ): Promise<PatchCreated>;
  // The same for the files of shelved lists.
  createFromShelf(
    chosen: ListFiles[],
    options: PatchOptions,
    to: PatchTarget,
    remember: boolean,
    repo: string,
  ): Promise<PatchCreated>;
  defaults(repo: string): Promise<PatchDefaults>;
  paths(file: string, base: string): Promise<PatchPaths>;
  showInFinder(repo: string, show: boolean): Promise<void>;
  reveal(path: string): Promise<void>;
  // The recoverable copy of a worktree file the patch was written over.
  restore(snapshot: string, repo: string): Promise<string>;
}

export const defaultPatchApi: PatchApi = {
  create: async (entries, options, to, remember, repo) =>
    taskResult<PatchCreated>(await patchCreate(entries, options, to, remember, repo)),
  createFromShelf: async (chosen, options, to, remember, repo) =>
    taskResult<PatchCreated>(await shelfPatchCreate(chosen, options, to, remember, repo)),
  defaults: patchSettingsGet,
  paths: patchPaths,
  showInFinder: patchShowInFinderSet,
  reveal: revealInFinder,
  restore: snapshotsRestore,
};

//
// Changes
//

// Local Changes and the Create Patch dialog: HEAD against what the list has of each file, the whole
// file or only its chosen hunks. An unversioned file is new.
export function localEntries(
  files: { path: string; oldPath: string | null; state?: FileChange["state"] }[],
  selections: Map<string, LineSelection[]> = new Map(),
): PatchEntry[] {
  return files.map((file) => {
    const selection = selections.get(file.path);
    return {
      path: file.path,
      oldPath: file.oldPath,
      before: file.state === "untracked" ? null : { kind: "head" },
      after: selection ? { kind: "selection", selection } : { kind: "disk" },
    };
  });
}

// `GitStageCreatePatchActionProvider`: per file, HEAD→index when only its Staged node is chosen,
// index→disk when only its Unstaged or Untracked one, HEAD→disk when both. Conflicts and ignored
// files do not go in.
export function stagingEntries(keys: string[], changes: FileChange[]): PatchEntry[] {
  const staged = new Set<string>();
  const unstaged = new Set<string>();
  for (const key of keys) {
    const scope = scopeOfKey(key);
    if (scope === "staged") staged.add(pathOfKey(key));
    else if (scope === "unstaged" || scope === "untracked") unstaged.add(pathOfKey(key));
  }
  const paths = [...staged, ...[...unstaged].filter((path) => !staged.has(path))];
  return paths.map((path) => {
    const fromHead = staged.has(path);
    return {
      path,
      oldPath: fromHead ? (changes.find((change) => change.path === path)?.oldPath ?? null) : null,
      before: fromHead ? { kind: "head" } : { kind: "index" },
      after: unstaged.has(path) ? { kind: "disk" } : { kind: "index" },
    };
  });
}

// The changes tree of the log: each file between its commit and the base it is shown against.
export function logEntries(
  changes: { file: ChangedFile; hash: string; base: string | null }[],
): PatchEntry[] {
  return changes.map(({ file, hash, base }) => ({
    path: file.path,
    oldPath: file.oldPath,
    before: base ? { kind: "revision", hash: base } : null,
    after: { kind: "revision", hash },
  }));
}

//
// Paths
//

function parts(path: string): string[] {
  const result: string[] = [];
  for (const part of path.split("/")) {
    if (part === "" || part === ".") continue;
    if (part === ".." && result.length > 0 && result[result.length - 1] !== "..") result.pop();
    else result.push(part);
  }
  return result;
}

const joined = (items: string[]) => `/${items.join("/")}`;

export function joinPath(folder: string, name: string): string {
  const trimmed = folder.replace(/\/+$/, "");
  return `${trimmed}/${name}`;
}

// `new File(path)`: its name and its parent.
export function splitPath(path: string): { name: string; parent: string } {
  const trimmed = path.length > 1 ? path.replace(/\/+$/, "") : path;
  const slash = trimmed.lastIndexOf("/");
  if (slash < 0) return { name: trimmed, parent: "" };
  return { name: trimmed.slice(slash + 1), parent: slash === 0 ? "/" : trimmed.slice(0, slash) };
}

// `ChangesUtil.findCommonAncestor` and `CreatePatchConfigurationPanel.setCommonParentPath`: the
// common ancestor of every before and after path, or its folder when it is a file.
export function commonParent(repo: string, entries: PatchEntry[]): string | null {
  const files = entries.flatMap((entry) => {
    const sides: string[] = [];
    if (entry.before) {
      sides.push(entry.before.kind === "index" ? entry.path : (entry.oldPath ?? entry.path));
    }
    if (entry.after) sides.push(entry.path);
    return sides.map((path) => joinPath(repo, path));
  });
  if (files.length === 0) return null;
  const split = files.map(parts);
  let common = split[0];
  for (const other of split.slice(1)) {
    let count = 0;
    while (count < common.length && count < other.length && common[count] === other[count]) count++;
    common = common.slice(0, count);
  }
  const ancestor = joined(common);
  if (files.some((file) => joined(parts(file)) === ancestor)) {
    return splitPath(ancestor).parent || "/";
  }
  return ancestor;
}

// `FileUtil.isAncestor(ancestor, file, false)`, with the case-insensitive file system of macOS.
export function isAncestor(ancestor: string, file: string): boolean {
  const above = parts(ancestor.toLowerCase());
  const below = parts(file.toLowerCase());
  return above.length <= below.length && above.every((part, index) => below[index] === part);
}

//
// Name and validation
//

// `Character.isWhitespace`: the controls it counts, and the Unicode separators other than the
// non-breaking ones.
function javaWhitespace(unit: string): boolean {
  if (/[\t\n\u000b\f\r\u001c-\u001f]/.test(unit)) return true;
  if (unit === " " || unit === " " || unit === " ") return false;
  return /[\p{Zs}\p{Zl}\p{Zp}]/u.test(unit);
}

// `PathUtilRt.suggestFileName(text, false, false)` with `isValidFileNameChar(c, CURRENT, true)`: on
// UTF-16 units, every separator, control, Windows-invalid character, `;`, dot and blank becomes `_`.
export function suggestFileName(text: string): string {
  let result = "";
  for (let index = 0; index < text.length; index++) {
    const unit = text[index];
    const invalid =
      unit === "/" ||
      unit === "\\" ||
      unit.charCodeAt(0) < 32 ||
      '<>:"|?*'.includes(unit) ||
      unit === ";" ||
      unit === "." ||
      javaWhitespace(unit);
    result += invalid ? "_" : unit;
  }
  return result;
}

// `ShelveChangesManager.shortenAndSanitize`, cut at 90 UTF-16 units. A surrogate pair the cut
// would split is left out whole, as the shelf's own cut does (`shelf.rs` `shorten_and_sanitize`);
// `src-tauri/testdata/patch-names.json` holds the cases both are tested with.
export function patchNamePrefix(message: string | null): string {
  const name = suggestFileName(message ?? "") || UNNAMED;
  if (name.length <= MAX_NAME - 10) return name;
  let cut = MAX_NAME - 10;
  const last = name.charCodeAt(cut - 1);
  if (last >= 0xd800 && last <= 0xdbff) cut--;
  return name.slice(0, cut);
}

// `ShelveChangesManager.suggestPatchName` with `FileUtil.findSequentNonexistentFile`: `<name>.patch`,
// `<name>1.patch`, … in the folder, shortening the name while it is 100 characters or more.
export async function suggestPatchFile(
  folder: string,
  message: string | null,
  exists: (path: string) => Promise<boolean>,
): Promise<string> {
  let prefix = patchNamePrefix(message);
  for (;;) {
    let postfix = 0;
    let candidate = joinPath(folder, `${prefix}.${EXTENSION}`);
    while (await exists(candidate)) {
      postfix++;
      candidate = joinPath(folder, `${prefix}${postfix}.${EXTENSION}`);
    }
    if (splitPath(candidate).name.length < MAX_NAME || prefix.length <= 1) return candidate;
    prefix = prefix.slice(0, -1);
  }
}

export type PatchProblem = { field: "file" | "base"; text: string };

// `CreatePatchConfigurationPanel.validateFields`: the name first, then the base path. The file is
// checked even with `To clipboard`, as there; Ramify's own absolute-path check only when it is used.
export function settingsProblem(
  paths: PatchPaths,
  parent: string | null,
  toClipboard = false,
): PatchProblem | null {
  const name = splitPath(paths.file.trim()).name;
  if (!name.trim()) return { field: "file", text: NAME_EMPTY };
  if (paths.file.length > MAX_PATH) return { field: "file", text: NAME_TOO_LONG };
  if (!toClipboard && !paths.file.trim().startsWith("/")) {
    return { field: "file", text: NOT_ABSOLUTE };
  }
  if (!paths.base.trim()) return { field: "base", text: BASE_EMPTY };
  if (!paths.base.trim().startsWith("/")) return { field: "base", text: NOT_ABSOLUTE };
  if (!paths.baseExists) return { field: "base", text: BASE_MISSING };
  if (parent && !isAncestor(paths.base, parent)) return { field: "base", text: baseOutside(parent) };
  return null;
}

// `warnIfFileExists`: the red label under the fields.
export function settingsWarning(paths: PatchPaths): string | null {
  return paths.fileExists ? fileExists(paths.file) : null;
}

//
// The flow
//

const cancelled = (failure: unknown) => (failure as ApiError | undefined)?.kind === "cancelled";

export function patchFlow(ui: PatchUi, api: PatchApi = defaultPatchApi) {
  const make = (changes: PatchChanges, options: PatchOptions, to: PatchTarget, remember: boolean) =>
    changes.shelf
      ? api.createFromShelf(changes.shelf, options, to, remember, changes.repo)
      : api.create(changes.entries, options, to, remember, changes.repo);
  const empty = (changes: PatchChanges) =>
    changes.shelf ? changes.shelf.every((list) => list.files.length === 0) : changes.entries.length === 0;

  // `createIntoClipboard`: no dialog, the repository as the base, and the message of the list.
  async function copyAsPatch(
    changes: PatchChanges,
    message: string | null = null,
  ): Promise<boolean> {
    if (empty(changes)) return false;
    const options = { basePath: changes.repo, reverse: false, message };
    try {
      await make(changes, options, { kind: "clipboard" }, false);
    } catch (failure) {
      if (!cancelled(failure)) {
        notify({ kind: "error", title: CREATION_FAILED, body: errorText(failure) });
      }
      return false;
    }
    notify({ kind: "info", title: COPIED });
    return true;
  }

  // `SHOW_PATCH_IN_EXPLORER`: `true` reveals, `false` does nothing, `null` asks.
  async function afterWriting(repo: string, file: string) {
    const show = (await api.defaults(repo)).showInFinder;
    if (show === false) return;
    if (show === true) {
      await api.reveal(file);
      return;
    }
    const answer = await ui.created(file);
    if (answer.reveal) await api.reveal(file);
    if (answer.dontAsk) await api.showInFinder(repo, answer.reveal);
  }

  // `CreatePatchCommitSession`: Patch File Settings, then the file or the clipboard. `confirmed`
  // runs once the settings are accepted (the Create Patch dialog closes then); with Cancel nothing
  // is done and it is not called. `false` if no patch was made.
  async function createPatch(
    changes: PatchChanges,
    message: string | null,
    confirmed?: () => void,
  ): Promise<boolean> {
    const { repo, entries } = changes;
    let answer: PatchSettingsAnswer | null;
    try {
      const defaults = await api.defaults(repo);
      const exists = async (path: string) => (await api.paths(path, "")).fileExists;
      const file = await suggestPatchFile(defaults.folder ?? repo, message, exists);
      answer = await ui.settings({
        file,
        toClipboard: defaults.toClipboard,
        base: repo,
        commonParent: commonParent(repo, entries),
        check: api.paths,
      });
    } catch (failure) {
      notify({ kind: "error", title: creationError(errorText(failure)) });
      return false;
    }
    if (!answer) return false;
    confirmed?.();
    const options = { basePath: answer.base, reverse: answer.reverse, message };
    let created: PatchCreated;
    try {
      if (answer.toClipboard) {
        await make(changes, options, { kind: "clipboard" }, true);
        notify({ kind: "info", title: COPIED });
        return true;
      }
      // `checkIsFileValid`.
      if ((await api.paths(answer.file, "")).fileExists) {
        const { name, parent } = splitPath(answer.file);
        if (!(await ui.overwrite(name, parent))) return false;
      }
      const to = { kind: "file" as const, path: answer.file };
      created = await make(changes, options, to, true);
    } catch (failure) {
      if (!cancelled(failure)) notify({ kind: "error", title: creationError(errorText(failure)) });
      return false;
    }
    const file = created.file ?? answer.file;
    if (created.snapshot) offerRestore(repo, file, created.snapshot);
    // The patch is there: what fails from here on is its own error.
    try {
      await afterWriting(repo, file);
    } catch (failure) {
      notify({ kind: "error", title: errorText(failure) });
    }
    return true;
  }

  // As Rollback's Restore: the restored snapshot can itself be undone. Both belong to `repo`.
  async function restore(snapshot: string, repo: string) {
    try {
      const previous = await api.restore(snapshot, repo);
      notify({
        kind: "info",
        title: RESTORED,
        actions: [{ label: "Undo", run: () => void restore(previous, repo) }],
      });
    } catch (failure) {
      notify({ kind: "error", title: errorText(failure) });
    }
  }

  function offerRestore(repo: string, file: string, snapshot: string) {
    notify({
      kind: "info",
      title: overwrittenText(splitPath(file).name),
      actions: [{ label: "Restore", run: () => void restore(snapshot, repo) }],
    });
  }

  return { copyAsPatch, createPatch };
}

//
// Apply Patch and Apply Patch from Clipboard
//

// `ApplyPatchAction`, `ApplyPatchFromClipboardAction`, `PatchClipboardListener`,
// `PatchApplier.executePatchGroup`, `ApplyPatchAction.applyContent` and `UndoApplyPatchDialog`.
export const APPLY_TITLE = "Apply Patch"; // V:346 `patch.apply.dialog.title`
export const SELECT_PATCH_TITLE = "Select Patch File"; // V:357
export const CANNOT_APPLY_NOW = "Cannot Apply Patch Now"; // V:380
export const NOTIFICATION_TITLE = "Apply patch"; // V:347 `patch.apply.notification.title`
// V:482-484.
export const ALREADY_APPLIED_TEXT = "All changes in the specified patch are already contained in the code";
export const PARTIALLY_APPLIED_TEXT =
  "Some of the changes in the specified patch were skipped because they are already contained in the code";
export const SUCCESS_TEXT = "Patch successfully applied";
// V:360-363.
export const ABORTED_TITLE = "Apply patch aborted";
export const ABORTED_TEXT = "All files changed during 'Apply Patch' action were rolled back";
export const ROLLBACK_FAILED_TITLE = "Rollback failed";
// V:363, kept as IntelliJ has it, and a second line of Ramify's own: Ramify has no Local History, and
// the copy taken before applying is in Restore Snapshot… of Local Changes.
export const ROLLBACK_FAILED_TEXT =
  "Try using 'Local History' dialog to perform revert manually.\n" +
  "The copy taken before applying is in Restore Snapshot… of Local Changes.";
// V:350-354.
export const LOCAL_VERSION = "Local version";
export const MERGE_RESULT = "Merge result";
export const PATCHED_VERSION = "Patched version";
// `patch.apply.conflict.for.title` with `PatchDiffRequestFactory.getPresentablePath`.
export function conflictTitle(repo: string, path: string): string {
  const { name, parent } = splitPath(joinPath(repo, path));
  return `Patch Conflict for ${name} (${parent})`;
}
// Ramify's own: another repository was opened while the dialog was open, or while the patch was
// being applied (then nothing more is written or rolled back in the new one).
export const REPOSITORY_SWITCHED = "Another repository was opened: the patch was not applied";
export const SWITCHED_WHILE_APPLYING =
  "Another repository was opened while the patch was being applied: the rest was not done";

// `ClipboardAnalyzeListener.MAX_SIZE`.
export const ON_THE_FLY_MAX = 100 * 1024;

// `ApplyPatchMode`; `unshelve` is the Shelf's.
export type ApplyMode = "apply" | "clipboard" | "unshelve";

const EMPTY_MAPPING: PathMapping = { files: [] };

// What the clipboard has when it is not a patch: the dialog shows `No changes detected`.
const EMPTY_PLAN: ApplyPlan = {
  subject: null,
  message: null,
  author: null,
  additionalInfoError: null,
  readError: null,
  files: [],
};

// What the dialog asks of the backend while it is open.
export interface ApplyServices {
  load(source: PatchInput, mapping: PathMapping): Promise<ApplyPlan>;
  preview(source: PatchInput, mapping: PathMapping, index: number): Promise<PatchPreview>;
  chooseFile(options: ChooserOptions): Promise<string | null>;
  chooseFolder(options: Omit<ChooserOptions, "filters">): Promise<string | null>;
  setOnTheFly(on: boolean): Promise<void>;
  // `remove_strategy`, which the Remove checkbox of the unshelve mode saves as it changes.
  setRemoveApplied(on: boolean): Promise<void>;
}

// `lists: null` without changelists (the staging area): there is no chooser. `onTheFly` is the
// checkbox of the clipboard mode.
export interface ApplyDialogRequest {
  mode: ApplyMode;
  repo: string;
  source: PatchInput;
  plan: ApplyPlan;
  lists: Changelists | null;
  onTheFly: boolean;
  services: ApplyServices;
  // The unshelve mode: `Remove successfully applied files from the shelf`, the files chosen in the
  // tab (all when `null`) and the names of the changelist to start on (`getPredefinedChangeList`).
  removeApplied?: boolean;
  preselected?: ShelfFile[] | null;
  listNames?: string[];
}

// `ChangeListChooserPanel.getSelectedList`: the list by its name, created if there is none.
export interface ListChoice {
  name: string;
  comment: string;
  setActive: boolean;
}

// OK: the source and mapping last planned, that plan, the files included (by their index) and the
// changelist.
export interface ApplyAnswer {
  source: PatchInput;
  mapping: PathMapping;
  plan: ApplyPlan;
  selected: number[];
  list: ListChoice | null;
  // The unshelve mode's Remove checkbox; `importToShelf`: the APPLY mode's `Import to Shelf`.
  removeApplied?: boolean;
  importToShelf?: boolean;
  // The binary files chosen with the Unshelve dialog's `Overwrite`, written over what HEAD has.
  overwrite?: string[];
}

export interface ApplyUi {
  dialog(request: ApplyDialogRequest): Promise<ApplyAnswer | null>;
  // `Overwrite Existing Files`: the paths to write over (none with Cancel).
  overwrite(paths: string[], repo: string): Promise<string[]>;
  // The errors of the pre-check, in one `Apply Patch` dialog.
  errors(messages: string[]): Promise<void>;
  // `Patch Applying Partly Failed`: true for `Rollback`.
  partlyFailed(failed: string[], binaries: boolean): Promise<boolean>;
}

export interface ApplyApi {
  plan(source: PatchInput, mapping: PathMapping): Promise<ApplyPlan>;
  apply(source: PatchInput, mapping: PathMapping, choice: ApplyChoice, expectedHead: string): Promise<PatchApplyResult>;
  resolve(conflict: PatchConflict, content: string): Promise<PatchResolved>;
  rollback(snapshot: string): Promise<string>;
  preview(source: PatchInput, mapping: PathMapping, index: number): Promise<PatchPreview>;
  clipboard(limit: number | null): Promise<ClipboardPatch>;
  appSettings(): Promise<AppPatch>;
  rememberFolder(folder: string): Promise<void>;
  setOnTheFly(on: boolean): Promise<void>;
  chooseFile(options: ChooserOptions): Promise<string | null>;
  chooseFolder(options: Omit<ChooserOptions, "filters">): Promise<string | null>;
  // The merge window in its contents mode; `null` if it went away without an answer (another
  // repository was opened, or the app quit).
  merge(contents: MergeContents): Promise<ContentsMergeDone | null>;
  // The list chosen in the dialog, created if new, with its comment and `Set active`: its id.
  useList(choice: ListChoice, repo: string): Promise<string>;
  moveToList(paths: string[], id: string): Promise<void>;
  // After an unshelve with Remove: the files of the list that went through.
  recycle(name: string, files: ShelfFile[], repo: string): Promise<void>;
  // `Import to Shelf`: the included files (by planned path) as a new list.
  importToShelf(source: PatchInput, mapping: PathMapping, selected: string[], repo: string): Promise<ShelvedList>;
  setRemoveApplied(on: boolean): Promise<void>;
}

// The repository the dialog was opened on and its HEAD; `current` is the one open now.
export interface ApplyContext {
  repo: string;
  head: string;
  lists: Changelists | null;
  current: () => string | null;
}

const sleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

// A switch of repository destroys the merge window without `merge-done`: while it is open, the
// window's session is checked, and once it no longer holds these contents the answer is waited for
// a moment longer (the event and the check may cross).
async function mergeContents(contents: MergeContents): Promise<ContentsMergeDone | null> {
  let settle: (done: ContentsMergeDone) => void = () => {};
  const done = new Promise<ContentsMergeDone>((resolve) => (settle = resolve));
  const stop = await onContentsMergeDone((answer) => {
    if (answer.id === contents.id) settle(answer);
  });
  try {
    await mergeWindowOpenContents(contents);
    for (;;) {
      const answer = await Promise.race([done, sleep(1000).then(() => null)]);
      if (answer) return answer;
      const shown = await mergeWindowContents();
      if (shown?.id !== contents.id) return await Promise.race([done, sleep(500).then(() => null)]);
    }
  } finally {
    stop();
  }
}

// `getSelectedList`: an existing list gets the comment of the panel, a new one is created with it.
async function useList(choice: ListChoice, repo: string): Promise<string> {
  const named = (all: Changelists) => all.lists.find((list) => list.name === choice.name);
  let list = named(await changelistsGet([])) ?? named(await changelistsCreate(choice.name));
  if (!list) throw new Error(`There is no changelist "${choice.name}"`);
  if (list.comment !== choice.comment) {
    list = named(await changelistsSetComment(list.id, choice.comment, repo)) ?? list;
  }
  if (choice.setActive) await changelistsSetActive(list.id);
  notifyRepoChanged();
  return list.id;
}

export const defaultApplyApi: ApplyApi = {
  plan: async (source, mapping) => taskResult<ApplyPlan>(await patchPlan(source, mapping)),
  apply: async (source, mapping, choice, head) =>
    taskResult<PatchApplyResult>(await patchApply(source, mapping, choice, head)),
  resolve: async (conflict, content) =>
    taskResult<PatchResolved>(await patchConflictResolve(conflict, content)),
  rollback: async (snapshot) => taskResult<string>(await patchRollback(snapshot)),
  preview: async (source, mapping, index) =>
    taskResult<PatchPreview>(await patchPreview(source, mapping, index)),
  clipboard: patchClipboard,
  appSettings: patchAppSettingsGet,
  rememberFolder: patchStorageSet,
  setOnTheFly: patchOnTheFlySet,
  chooseFile,
  chooseFolder,
  merge: mergeContents,
  useList,
  moveToList: async (paths, id) => {
    await changelistsMove(paths, id);
    notifyRepoChanged();
  },
  recycle: async (name, files, repo) => {
    await taskResult<null>(await shelfRecycle(name, files, repo));
  },
  importToShelf: async (source, mapping, selected, repo) =>
    taskResult<ShelvedList>(await shelfImportToShelf(source, mapping, selected, repo)),
  setRemoveApplied: (on) => saveShelfSetting({ removeStrategy: on }),
};

// `unshelve.loading.patch.error` (V:480) and `action.import.to.shelf` (V:447).
export const cannotLoadPatches = (reason: string) => `Cannot load patch(es): ${reason}`;
export const IMPORT_TO_SHELF_TITLE = "Import to Shelf";

// `ApplyPatchStatus.ORDERED_TYPES` and `and`: the later of the two, but SUCCESS with
// ALREADY_APPLIED is PARTIAL.
const STATUS_ORDER: ApplyStatus[] = ["skip", "success", "alreadyApplied", "partial", "failure", "abort"];

export function andStatus(lhs: ApplyStatus | null, rhs: ApplyStatus | null): ApplyStatus | null {
  if (lhs === null || rhs === null) return lhs ?? rhs;
  const partial: ApplyStatus[] = ["success", "alreadyApplied"];
  if (lhs !== rhs && partial.includes(lhs) && partial.includes(rhs)) return "partial";
  return STATUS_ORDER.indexOf(lhs) >= STATUS_ORDER.indexOf(rhs) ? lhs : rhs;
}

let mergeCount = 0;
// An apply between its OK and its end, which waits on dialogs and on the merge window.
let applyRunning = false;

// While it runs the apply is modal: the app's menu, shortcuts and on-the-fly detection wait, the
// other patch dialogs are refused instead of cancelling the one it asks next, and the main window
// is inert (`applying`).
export const applyInProgress = () => applyRunning;
export const applying = writable(false);

function setApplyRunning(running: boolean) {
  applyRunning = running;
  applying.set(running);
}

export function applyFlow(ui: ApplyUi, api: ApplyApi = defaultApplyApi) {
  const services: ApplyServices = {
    load: (source, mapping) => api.plan(source, mapping),
    preview: (source, mapping, index) => api.preview(source, mapping, index),
    chooseFile: (options) => api.chooseFile(options),
    chooseFolder: (options) => api.chooseFolder(options),
    setOnTheFly: (on) => api.setOnTheFly(on),
    setRemoveApplied: (on) => api.setRemoveApplied(on),
  };

  const failedTo = (failure: unknown) => {
    if (!cancelled(failure)) notify({ kind: "error", title: APPLY_TITLE, body: errorText(failure) });
  };

  // `ApplyPatchAction.actionPerformed`: `Select Patch File` from `PATCH_STORAGE_LOCATION`, whose
  // folder is saved, then the dialog. Nothing while another apply runs.
  async function applyPatch(context: ApplyContext): Promise<void> {
    if (applyRunning) return;
    const settings = await api.appSettings().catch(() => null);
    let file: string | null;
    try {
      file = await api.chooseFile({ title: SELECT_PATCH_TITLE, defaultPath: settings?.folder ?? context.repo });
    } catch (failure) {
      failedTo(failure);
      return;
    }
    if (!file) return;
    const { parent } = splitPath(file);
    if (parent) void api.rememberFolder(parent).catch(() => {});
    await openDialog("apply", { kind: "file", path: file }, context);
  }

  // `ApplyPatchFromClipboardAction`: the dialog over the clipboard's text. Text that is not a patch
  // is not read (`No changes detected`); with no text Ramify cannot have the item disabled, so it says
  // `Cannot Apply Patch Now`.
  async function applyFromClipboard(context: ApplyContext): Promise<void> {
    if (applyRunning) return;
    let clipboard: ClipboardPatch;
    try {
      clipboard = await api.clipboard(null);
    } catch (failure) {
      failedTo(failure);
      return;
    }
    if (clipboard.length === 0) {
      notify({ kind: "error", title: CANNOT_APPLY_NOW });
      return;
    }
    await applyText(context, clipboard.patch);
  }

  // The clipboard dialog over a text already read (`null`: not a patch).
  async function applyText(context: ApplyContext, text: string | null): Promise<void> {
    if (applyRunning) return;
    const source: PatchInput = { kind: "text", text: text ?? "" };
    await openDialog("clipboard", source, context, text === null ? EMPTY_PLAN : undefined);
  }

  async function openDialog(
    mode: ApplyMode,
    source: PatchInput,
    context: ApplyContext,
    given?: ApplyPlan,
    extra: Pick<ApplyDialogRequest, "removeApplied" | "preselected" | "listNames"> = {},
  ) {
    let plan: ApplyPlan;
    let onTheFly = false;
    try {
      plan = given ?? (await api.plan(source, EMPTY_MAPPING));
      if (mode === "clipboard") onTheFly = (await api.appSettings()).detectPatchOnTheFly;
    } catch (failure) {
      failedTo(failure);
      return;
    }
    const answer = await ui.dialog({
      mode,
      repo: context.repo,
      source,
      plan,
      lists: context.lists,
      onTheFly,
      services,
      ...extra,
    });
    if (answer) await run(answer, context);
  }

  // `UnshelveWithDialogAction` with one list: the dialog in its UNSHELVE mode.
  async function unshelveWithDialog(context: ApplyContext, request: UnshelveRequest): Promise<void> {
    if (applyRunning) return;
    const source: PatchInput = { kind: "shelf", name: request.name, files: null };
    await openDialog("unshelve", source, context, undefined, {
      removeApplied: request.removeApplied,
      preselected: request.preselected,
      listNames: request.listNames,
    });
  }

  // `unshelveChangeList` with no dialog: every file of the list (or the chosen ones), into `list`
  // (the active list when `null`).
  async function unshelveSilently(context: ApplyContext, request: SilentRequest): Promise<ApplyStatus | null> {
    if (applyRunning) return null;
    const source: PatchInput = { kind: "shelf", name: request.name, files: request.files };
    let plan: ApplyPlan;
    try {
      plan = await api.plan(source, EMPTY_MAPPING);
    } catch (failure) {
      failedTo(failure);
      return "failure";
    }
    if (plan.readError) {
      notify({ kind: "error", title: APPLY_TITLE, body: cannotLoadPatches(plan.readError) });
      return "failure";
    }
    // A binary file that cannot go back now is left out, with its reason; the rest is applied.
    const unusable = plan.files.filter((file) => file.binary && file.status === "invalid");
    if (unusable.length > 0) await ui.errors(unusable.map((file) => file.error ?? file.path));
    const answer: ApplyAnswer = {
      source,
      mapping: EMPTY_MAPPING,
      plan,
      selected: plan.files.filter((file) => !unusable.includes(file)).map((file) => file.index),
      list: request.list,
      removeApplied: request.removeApplied,
    };
    return await run(answer, context);
  }

  // `PatchApplier.executePatchGroup` around the backend's apply.
  // From OK to the last notification the apply is modal, as IntelliJ's merge window for its
  // conflicts is: see `applyInProgress`.
  async function run(answer: ApplyAnswer, context: ApplyContext): Promise<ApplyStatus | null> {
    setApplyRunning(true);
    try {
      if (answer.importToShelf) {
        await importToShelf(answer, context);
        return null;
      }
      const done = await runApply(answer, context);
      // `updateListAfterUnshelve`: with Remove, what went through leaves the list. Nothing after an
      // abort or a rollback, whose files are back as they were.
      const shelf = answer.source.kind === "shelf" ? answer.source : null;
      const applied = done.applied.flatMap((file) => (file.shelf ? [file.shelf] : []));
      if (shelf && answer.removeApplied && applied.length > 0 && context.current() === context.repo) {
        await api.recycle(shelf.name, applied, context.repo).catch(failedTo);
      }
      return done.status;
    } finally {
      setApplyRunning(false);
    }
  }

  // `ImportToShelfExecutor`: the included files, as the dialog maps them, become a list of the
  // shelf, which the Shelf tab then shows.
  async function importToShelf(answer: ApplyAnswer, context: ApplyContext): Promise<void> {
    if (context.current() !== context.repo) {
      notify({ kind: "error", title: IMPORT_TO_SHELF_TITLE, body: REPOSITORY_SWITCHED });
      return;
    }
    const paths = answer.selected.flatMap((index) =>
      answer.plan.files.filter((file) => file.index === index).map((file) => file.path),
    );
    if (paths.length === 0) return;
    try {
      const list = await api.importToShelf(answer.source, answer.mapping, paths, context.repo);
      showShelf(list.name);
    } catch (failure) {
      if (!cancelled(failure)) notify({ kind: "error", title: IMPORT_TO_SHELF_TITLE, body: errorText(failure) });
    }
  }

  // The status, and the files that went through: written, merged or already there.
  async function runApply(
    answer: ApplyAnswer,
    context: ApplyContext,
  ): Promise<{ status: ApplyStatus | null; applied: PlannedFile[] }> {
    const nothing = { status: null, applied: [] };
    const { repo } = context;
    const switched = () => context.current() !== repo;
    if (switched()) {
      notify({ kind: "error", title: APPLY_TITLE, body: REPOSITORY_SWITCHED });
      return nothing;
    }
    const files = answer.selected.flatMap((index) => answer.plan.files.filter((file) => file.index === index));
    if (files.length === 0) return nothing;
    const existing = files.filter((file) => file.kind === "added" && file.status === "exists").map((file) => file.path);
    const chosen = (answer.overwrite ?? []).filter((path) => files.some((file) => file.path === path));
    const overwrite = [...chosen, ...(existing.length > 0 ? await ui.overwrite(existing, repo) : [])];
    let list: string | null = null;
    if (answer.list && context.lists) {
      try {
        list = await api.useList(answer.list, repo);
      } catch (failure) {
        failedTo(failure);
      }
    }
    const expected = Object.assign({}, ...files.map((file) => file.hashes)) as Record<string, string>;
    const choice = { selected: files.map((file) => file.path), overwrite, expected };
    let result: PatchApplyResult;
    try {
      result = await api.apply(answer.source, answer.mapping, choice, context.head);
    } catch (failure) {
      failedTo(failure);
      return { status: "failure", applied: [] };
    }
    if (result.messages.length > 0) await ui.errors(result.messages);
    const failed = [...result.failed];
    const resolved = new Set<string>();
    let status: ApplyStatus | null = result.status;
    let stopped = false;
    for (const conflict of result.conflicts) {
      if (switched()) {
        stopped = true;
        failed.push(conflict.path);
        continue;
      }
      const outcome = await resolveConflict(repo, conflict, switched);
      if (outcome === "failure") failed.push(conflict.path);
      if (outcome === "success") resolved.add(conflict.path);
      status = andStatus(status, outcome);
      if (outcome === "abort") break;
    }
    if (stopped || switched()) {
      notify({ kind: "error", title: APPLY_TITLE, body: SWITCHED_WHILE_APPLYING });
      return { status, applied: [] };
    }
    let rollback = false;
    if (status === "failure") {
      const binaries = files.some((file) => file.binary);
      rollback = await ui.partlyFailed(failed, binaries);
    }
    if (status === "abort" || rollback) {
      await rollBack(result.snapshot);
      return { status, applied: [] };
    }
    const conflicted = new Set(result.conflicts.map((conflict) => conflict.path));
    const declined = new Set(existing.filter((path) => !overwrite.includes(path)));
    // `applyList`: a file goes through unless it failed, was skipped (a declined overwrite, an
    // unanswered conflict, a deleted file already gone) or was skipped in the merge window.
    const applied = files.filter((file) => {
      const inConflict = conflicted.has(file.path) || (file.before !== null && conflicted.has(file.before));
      const merged = resolved.has(file.path) || (file.before !== null && resolved.has(file.before));
      return (
        !failed.includes(file.path) &&
        !declined.has(file.path) &&
        file.status !== "missing" &&
        (!inConflict || merged)
      );
    });
    // `computeUnderChangeListSync`: what was written goes to the list, whatever the status.
    if (list !== null) {
      const paths = applied.filter((file) => file.status !== "alreadyApplied").map((file) => file.path);
      if (paths.length > 0) await api.moveToList(paths, list).catch(failedTo);
    }
    showStatus(status);
    return { status, applied };
  }

  // One conflict: the merge window, then the result written over the file it was shown.
  async function resolveConflict(
    repo: string,
    conflict: PatchConflict,
    switched: () => boolean,
  ): Promise<ApplyStatus> {
    const contents: MergeContents = {
      id: `patch-${++mergeCount}`,
      title: conflictTitle(repo, conflict.path),
      titles: [LOCAL_VERSION, MERGE_RESULT, PATCHED_VERSION],
      left: conflict.local,
      base: conflict.base,
      right: conflict.patched,
      path: conflict.path,
      abort: true,
    };
    let done: ContentsMergeDone | null;
    try {
      done = await api.merge(contents);
    } catch (failure) {
      failedTo(failure);
      return "failure";
    }
    if (!done || switched()) return "failure";
    if (done.result === "abort") return "abort";
    if (done.result === "skip") return "skip";
    if (done.result === "cancelled" || done.content === undefined) return "failure";
    try {
      const resolved = await api.resolve(conflict, done.content);
      if (resolved.messages.length > 0) await ui.errors(resolved.messages);
      return "success";
    } catch (failure) {
      failedTo(failure);
      return "failure";
    }
  }

  // `rollbackUnderProgressIfNeeded`: back to the copy taken before applying.
  async function rollBack(snapshot: string | null) {
    try {
      if (snapshot) await api.rollback(snapshot);
      notify({ kind: "warning", title: ABORTED_TITLE, body: ABORTED_TEXT });
    } catch {
      notify({ kind: "warning", title: ROLLBACK_FAILED_TITLE, body: ROLLBACK_FAILED_TEXT });
    }
  }

  // `showApplyStatus`.
  function showStatus(status: ApplyStatus | null) {
    if (status === "alreadyApplied") {
      notify({ kind: "info", title: NOTIFICATION_TITLE, body: ALREADY_APPLIED_TEXT });
    } else if (status === "partial") {
      notify({ kind: "info", title: NOTIFICATION_TITLE, body: PARTIALLY_APPLIED_TEXT });
    } else if (status === "success") {
      notify({ kind: "info", title: SUCCESS_TEXT });
    }
  }

  return { applyPatch, applyFromClipboard, applyText, unshelveWithDialog, unshelveSilently };
}

// What the Shelf tab asks of the apply flow (`src/shelf/flow.ts`'s `Unshelver`).
export interface UnshelveRequest {
  name: string;
  preselected: ShelfFile[] | null;
  listNames: string[];
  removeApplied: boolean;
}

export interface SilentRequest {
  name: string;
  files: ShelfFile[] | null;
  list: ListChoice | null;
  removeApplied: boolean;
}

// `PatchClipboardListener` over `ClipboardAnalyzeListener`: with `DETECT_PATCH_ON_THE_FLY` on, the
// clipboard is looked at when the app gains the focus, and a patch shorter than 100 KB that was not
// seen before is offered. Leaving remembers the text as it was, so that what the user copied
// inside the app is not offered.
export function clipboardWatcher(api: Pick<ApplyApi, "appSettings" | "clipboard">, offer: (text: string) => void) {
  let seen: string | null = null;
  const enabled = async () => (await api.appSettings().catch(() => null))?.detectPatchOnTheFly === true;

  async function activated() {
    if (!(await enabled())) return;
    const clipboard = await api.clipboard(ON_THE_FLY_MAX).catch(() => null);
    if (!clipboard?.hash || clipboard.length >= ON_THE_FLY_MAX || clipboard.hash === seen) return;
    seen = clipboard.hash;
    if (clipboard.patch !== null) offer(clipboard.patch);
  }

  async function deactivated() {
    if (!(await enabled())) return;
    const clipboard = await api.clipboard(ON_THE_FLY_MAX).catch(() => null);
    if (clipboard) seen = clipboard.hash;
  }

  return { activated, deactivated };
}

//
// The dialogs of the application
//

export type PatchDialogRequest =
  | ({ kind: "settings" } & PatchSettingsRequest)
  | { kind: "overwrite"; name: string; parent: string }
  | { kind: "created"; path: string }
  | ({ kind: "apply" } & ApplyDialogRequest)
  | { kind: "overwriteExisting"; repo: string; paths: string[] }
  | { kind: "applyErrors"; messages: string[] }
  | { kind: "partlyFailed"; failed: string[]; binaries: boolean };

const open = writable<{ request: PatchDialogRequest; settle: (answer: unknown) => void } | null>(
  null,
);

// What `PatchDialogs.svelte` shows. One at a time: a new one counts the previous as cancelled.
export const patchDialog = { subscribe: open.subscribe };

let current: { settle: (answer: unknown) => void } | null = null;

// `fromApply`: asked by a running apply. Anything else is refused while one runs.
function ask<T>(request: PatchDialogRequest, fromApply = false): Promise<T | null> {
  if (applyRunning && !fromApply) return Promise.resolve(null);
  return new Promise((resolve) => {
    current?.settle(null);
    const entry = { request, settle: (answer: unknown) => resolve(answer as T | null) };
    current = entry;
    open.set(entry);
  });
}

export function answerPatchDialog(answer: unknown): void {
  const entry = current;
  current = null;
  open.set(null);
  entry?.settle(answer);
}

// The questions of the flow as the dialogs of `PatchDialogs.svelte`.
export const patchDialogsUi: PatchUi = {
  settings: (request) => ask<PatchSettingsAnswer>({ kind: "settings", ...request }),
  overwrite: async (name, parent) =>
    (await ask<boolean>({ kind: "overwrite", name, parent })) ?? false,
  created: async (path) =>
    (await ask<CreatedAnswer>({ kind: "created", path })) ?? { reveal: false, dontAsk: false },
};

// The questions of Apply Patch, as the dialogs of `PatchDialogs.svelte`.
export const applyDialogsUi: ApplyUi = {
  dialog: (request) => ask<ApplyAnswer>({ kind: "apply", ...request }),
  overwrite: async (paths, repo) =>
    (await ask<string[]>({ kind: "overwriteExisting", repo, paths }, true)) ?? [],
  errors: async (messages) => {
    await ask<boolean>({ kind: "applyErrors", messages }, true);
  },
  partlyFailed: async (failed, binaries) =>
    (await ask<boolean>({ kind: "partlyFailed", failed, binaries }, true)) ?? false,
};

// The ones the application uses.
export const patches = patchFlow(patchDialogsUi);
export const applies = applyFlow(applyDialogsUi);
