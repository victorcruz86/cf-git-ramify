import { checkinFilesText } from "../commit/checkin";
import { compareSameVersion } from "../compare/actions";
import {
  compareWithBranchPopup,
  compareWithRevisionPopup,
  nothingToCompare,
} from "../compare/popups";
import {
  branchList,
  repoState,
  repoStatus,
  type CurrentFileItem,
  type FileState,
} from "../lib/api";
import type { MenuAction, MenuItem } from "../lib/menu";
import { hasLocalChanges, stateOf, statusIndex } from "../tree/model";
import { openInViewer } from "../viewer/window";
import { showCurrentRevision } from "./currentRevision";

// A selected file or folder: its path in the repository ("" is the repository itself) and the
// status `ChangeListManager.getStatus` gives it, `null` for a tracked file without changes (and for
// a folder, which has none of its own unless it is ignored).
export interface FileTarget {
  path: string;
  dir: boolean;
  state: FileState | null;
  // Binary: by its type, which Rust says with each tree entry, or by its bytes, which the viewer
  // read.
  binary?: boolean;
}

// What the file actions apply to (`VcsDataKeys.VIRTUAL_FILES`) and whether the menu comes from an
// editor (`CommonDataKeys.EDITOR`). A file at a revision is not a local file: it gives no files.
export interface GitFileSelection {
  files: FileTarget[];
  editor: boolean;
}

// The ids are those of the actions in plugin.xml, `VcsActions.xml` and `ActionsBundle`.
export type GitFileActionId =
  | "CheckinFiles"
  | "Git.Add"
  | "Annotate"
  | "Compare.SameVersion"
  | "Compare.Selected"
  | "Git.CompareWithBranch"
  | "Vcs.ShowTabbedFileHistory"
  | "Vcs.ShowHistoryForBlock"
  | "Show.Current.Revision"
  | "ChangesView.Revert"
  | "Git.ResolveConflicts";

// What the actions depend on besides the selection, and who carries them out.
export interface GitFileState {
  // `ChangeListManager.getAllChanges` is not empty: `RollbackAction.hasReversibleFiles`.
  hasChanges: boolean;
  // A conflicted file anywhere: `GitResolveConflictsAction.isEnabled`.
  conflicts: boolean;
  // `Repository.isFresh` is false.
  hasCommits: boolean;
  // `GitCompareWithBranchAction.nothingToCompare` is false.
  branchesToCompare: boolean;
  // The viewer's tab: whether it can be annotated (a file at a revision has no local file, and
  // `AnnotateVcsVirtualFileAction` takes it) and whether its annotations are shown.
  annotation?: { can: boolean; on: boolean };
  // The viewer's tab has a text editor (`VcsSelectionUtil.getSelection` needs one): its lines can
  // be selected for Show History for Selection.
  textEditor?: boolean;
  run(action: GitFileActionId, files: FileTarget[]): void;
}

// `Git.FileActions` (`X:100-111`): the entries of `Git > Current File`, in their order.
export const CURRENT_FILE_ACTIONS: GitFileActionId[] = [
  "CheckinFiles",
  "Git.Add",
  "Annotate",
  "Compare.SameVersion",
  "Compare.Selected",
  "Git.CompareWithBranch",
  "Vcs.ShowTabbedFileHistory",
  "Vcs.ShowHistoryForBlock",
];

export const isFileAction = (id: string): id is GitFileActionId =>
  (CURRENT_FILE_ACTIONS as string[]).includes(id) ||
  id === "Show.Current.Revision" ||
  id === "ChangesView.Revert" ||
  id === "Git.ResolveConflicts";

// `B:559` (mnemonic stripped), `V:982`, `B:1324-1325`.
const GIT_GROUP = "Git";
const CURRENT_FILE = "Current File";

// `Git.Add` uses the shortcut of `ChangesView.AddUnversioned`.
const ADD_SHORTCUT = "⌥⌘A";

interface Entry {
  id: GitFileActionId;
  label: string;
  enabled: boolean;
  visible: boolean;
  // What the action is run with: `VcsContextUtil.selectedFile` for the one-file ones.
  files: FileTarget[];
  shortcut?: string;
  // Why it is off.
  title?: string;
  checked?: boolean;
}

// `AbstractVcs.fileInVcsByFileStatus`: neither unversioned, nor added, nor ignored.
const inRepository = (state: FileState | null) =>
  state !== "untracked" && state !== "added" && state !== "copied" && state !== "ignored";

// `CommonCheckinFilesAction.Manager.isActionEnabled` (a folder, or a file with a status other than
// ignored) and `ScheduleForAdditionWithIgnoredFilesConfirmationAction.isEnabled` (an unversioned
// file, a folder that is not ignored, or a file `GitAdd.isStatusForAddition` takes) come to the
// same rule here: rama has no confirmation for adding ignored files, so those are left out.
// Cost: an ignored file cannot be added from this menu (241 asks, then runs `git add -f`); it takes
// `git add -f` in a terminal.
const changedOrFolder = (target: FileTarget) =>
  target.dir ? target.state !== "ignored" : target.state !== null && target.state !== "ignored";

// The repository's own folder, the root row of the tree. rama compares folders below it only: the
// comparisons take a path inside the repository. Cost: Compare with Revision... and Compare with
// Branch… of the whole repository are not offered here (241 has them on the project root): the user
// compares a top-level folder at a time instead.
const isRoot = (target: FileTarget) => target.dir && target.path === "";

// `VcsLogSingleFileHistoryProvider` takes one file; `VcsLogDirectoryHistoryProvider`, folders or the
// repository itself, not both. Several files have no history (`canShowNewFileHistory` is false).
function showsHistory(files: FileTarget[]): boolean {
  if (files.length === 0 || !files.every((target) => inRepository(target.state))) return false;
  if (files.length === 1 && !files[0].dir) return true;
  if (!files.every((target) => target.dir)) return false;
  return files.every(isRoot) || !files.some(isRoot);
}

// `annotate.cannot.annotate.dir` (`B:18`).
const CANNOT_ANNOTATE_DIRECTORY = "Cannot annotate directory";

// `AnnotateToggleAction.update` with `AnnotateLocalFileAction.isEnabled`: one file in the
// repository, of a type that is not binary; the provider's name then (`A:270`), else `Annotate`.
// The viewer says it for its tab.
export function canAnnotate(files: FileTarget[]): boolean {
  const file = files.length === 1 && !files[0].dir ? files[0] : null;
  return file !== null && inRepository(file.state) && !file.binary;
}

function annotateEntry(files: FileTarget[], state: GitFileState) {
  const can = state.annotation?.can ?? canAnnotate(files);
  return {
    label: can ? "Annotate with Git Blame" : "Annotate",
    enabled: can,
    checked: state.annotation?.on ?? false,
    title: files.length === 1 && files[0].dir ? CANNOT_ANNOTATE_DIRECTORY : undefined,
  };
}

// `AbstractShowDiffAction.isEnabled`: one file, not a folder, in the repository.
const oneFileInRepository = (files: FileTarget[]) =>
  files.length === 1 && !files[0].dir && inRepository(files[0].state);

function entries(selection: GitFileSelection, state: GitFileState): Entry[] {
  const { files, editor } = selection;
  const first = files[0];
  const single = files.length === 1 ? files : [];
  const oneFile = oneFileInRepository(files);
  const entry = (
    id: GitFileActionId,
    label: string,
    enabled: boolean,
    on: FileTarget[] = files,
    visible = true,
  ): Entry => ({ id, label, enabled, visible, files: on });
  return [
    entry(
      "CheckinFiles",
      checkinFilesText(files.map((target) => ({ path: target.path, directory: target.dir }))),
      files.some(changedOrFolder),
    ),
    { ...entry("Git.Add", "Add", files.some(changedOrFolder)), shortcut: ADD_SHORTCUT },
    (() => {
      const { label, enabled, checked, title } = annotateEntry(files, state);
      return { ...entry("Annotate", label, enabled, single), checked, title };
    })(),
    entry("Compare.SameVersion", "Compare with the Same Repository Version", oneFile, single),
    // `CompareWithSelectedRevisionAction.update`: a first folder compares, else one file.
    entry(
      "Compare.Selected",
      "Compare with Revision...",
      (first?.dir === true && !isRoot(first)) || oneFile,
      first ? [first] : [],
    ),
    // `DvcsCompareWithAction.update`: one file or folder of a repository with something to compare.
    entry(
      "Git.CompareWithBranch",
      "Compare with Branch…",
      files.length === 1 && !isRoot(files[0]) && state.hasCommits && state.branchesToCompare,
      single,
    ),
    // `TabbedShowHistoryAction.isEnabled`: one file, or folders, in the repository.
    entry("Vcs.ShowTabbedFileHistory", "Show History", showsHistory(files)),
    // `SelectedBlockHistoryAction.update`: only in an editor (`setEnabledAndVisible(false)`
    // without one), on a local file of the repository (`isEnabled`: a VCS for the file and
    // `fileInVcsByFileStatus`) with a selection, which the caret line is without one.
    entry(
      "Vcs.ShowHistoryForBlock",
      "Show History for Selection…",
      editor && oneFile && state.textEditor === true,
      single,
      editor,
    ),
    entry("Show.Current.Revision", "Show Current Revision", oneFile, single),
    entry("ChangesView.Revert", "Rollback...", state.hasChanges),
    entry("Git.ResolveConflicts", "Resolve Conflicts…", state.conflicts, files, state.conflicts),
  ];
}

const asAction = (item: Entry, state: GitFileState): MenuAction => ({
  label: item.label,
  disabled: !item.enabled,
  ...(item.shortcut ? { shortcut: item.shortcut } : {}),
  ...(item.title ? { title: item.title } : {}),
  ...(item.checked ? { checked: true } : {}),
  run: () => state.run(item.id, item.files),
});

// `Git.ContextMenu` (`X:265-275`) without `Rollback File...` (hidden for the modal commit, G§0.1)
// and without `GitRepositoryActions`: what is not visible is left out, and so is a separator left
// with nothing after it.
export function gitFileMenu(selection: GitFileSelection, state: GitFileState): MenuItem[] {
  const shown = new Map(
    entries(selection, state)
      .filter((item) => item.visible)
      .map((item) => [item.id, asAction(item, state)]),
  );
  const pick = (ids: GitFileActionId[]) =>
    ids.flatMap((id) => {
      const item = shown.get(id);
      return item ? [item] : [];
    });
  const items: MenuItem[] = [
    ...pick(["CheckinFiles", "Git.Add"]),
    null,
    ...pick([
      "Annotate",
      "Compare.SameVersion",
      "Compare.Selected",
      "Git.CompareWithBranch",
      "Vcs.ShowTabbedFileHistory",
      "Vcs.ShowHistoryForBlock",
      "Show.Current.Revision",
      "ChangesView.Revert",
    ]),
  ];
  const resolve = pick(["Git.ResolveConflicts"]);
  return resolve.length > 0 ? [...items, null, ...resolve] : items;
}

// `Git ▸` of the Project tree and of the viewer's editor (`VcsFileGroupPopup` → `Git.ContextMenu`).
export function gitSubmenu(selection: GitFileSelection, state: GitFileState): MenuAction {
  return { label: GIT_GROUP, submenu: gitFileMenu(selection, state) };
}

const plural = (count: number, one: string, many: string) => (count === 1 ? one : many);

// `GitFileActionGroup.update`.
export function currentFileTitle({ files, editor }: GitFileSelection): string {
  if (editor || files.length === 0) return CURRENT_FILE;
  return files[0].dir
    ? `Selected ${plural(files.length, "Directory", "Directories")}`
    : `Selected ${plural(files.length, "File", "Files")}`;
}

// The entries of `Git > Current File` for the native menu, with their texts.
export function currentFileItems(
  selection: GitFileSelection,
  state: GitFileState,
): CurrentFileItem[] {
  const all = new Map(entries(selection, state).map((item) => [item.id, item]));
  return CURRENT_FILE_ACTIONS.map((id) => {
    const item = all.get(id)!;
    return { id, text: item.label, enabled: item.enabled && item.visible };
  });
}

// What choosing `id` does now (`Git > Current File` from the native menu): nothing when the entry
// is off or not there for this selection.
export function fileActionRun(
  selection: GitFileSelection,
  state: GitFileState,
  id: string,
): (() => void) | null {
  const item = entries(selection, state).find((entry) => entry.id === id);
  if (!item || !item.enabled || !item.visible) return null;
  return () => state.run(item.id, item.files);
}

// The selection and the state of a window that keeps no status of its own, the viewer: its local
// file (`null` for a file at a revision), read now, and whether it read that file as binary.
export async function readFileContext(
  path: string | null,
  run: GitFileState["run"],
  binary = false,
): Promise<{ selection: GitFileSelection; state: GitFileState }> {
  const [changes, repo, branches] = await Promise.all([
    repoStatus(true),
    repoState(),
    branchList(),
  ]);
  const index = statusIndex(changes);
  return {
    selection: {
      files:
        path === null
          ? []
          : [{ path, dir: false, state: stateOf(index, path), ...(binary ? { binary } : {}) }],
      editor: true,
    },
    state: {
      hasChanges: hasLocalChanges(index),
      conflicts: changes.some((change) => change.state === "unmerged"),
      hasCommits: repo.head !== "",
      branchesToCompare: !nothingToCompare(branches),
      run,
    },
  };
}

// What any window carries out itself: the comparisons of one file or folder (the diff window, the
// `Changes` window, their popups), Show Current Revision and Annotate, which opens the file in the
// viewer with its annotations (the viewer toggles its own tab instead). `false` for the others. A
// failure, such as a `CompareError`, is the caller's to show with its text.
export async function runInPlace(id: GitFileActionId, files: FileTarget[]): Promise<boolean> {
  const [target] = files;
  if (!target) return false;
  if (id === "Compare.SameVersion") await compareSameVersion(target.path);
  else if (id === "Compare.Selected") await compareWithRevisionPopup(target.path, target.dir);
  else if (id === "Git.CompareWithBranch") await compareWithBranchPopup(target.path, target.dir);
  else if (id === "Show.Current.Revision") await showCurrentRevision(target.path);
  else if (id === "Annotate") {
    await openInViewer({ path: target.path, revision: null, line: null, annotate: true });
  }
  else return false;
  return true;
}
