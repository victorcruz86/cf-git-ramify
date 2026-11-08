// The model of the Commit Changes dialog: what is included, which list it opens on, what the
// message says and what each executor calls itself. No Svelte here.
import type { ChangeList, FileSelection, PartialFile } from "../lib/api";
import type { CheckinRequest } from "./checkin";

// Decision 6: the dialog opens at 900×700 the first time, and then as it was left, under IntelliJ's
// keys (`DIMENSION_SERVICE_KEY` and the splitters' `PropertiesComponent` keys).
export const SIZE_KEY = "CommitChangelistDialog2";
export const SPLITTER_KEY = "CommitChangeListDialog.SPLITTER_PROPORTION_2";
export const DETAILS_PROPORTION_KEY = "CommitChangeListDialog.DETAILS_SPLITTER_PROPORTION_2";
export const DETAILS_SHOWN_KEY = "CommitChangeListDialog.DETAILS_SHOW_OPTION_";

// A changelist with the paths of its changes, which is how the dialog sees it.
export interface ListWithFiles extends ChangeList {
  files: string[];
}

// `LocalChangeList.getAllDefaultNames`.
const DEFAULT_NAMES = ["Changes", "Default Changelist", "Default"];

function hasDefaultName(list: ChangeList): boolean {
  return DEFAULT_NAMES.includes(list.name);
}

// `CheckinActionUtil.getInitiallySelectedChangeList` for `Commit…` and
// `getInitiallySelectedChangeListFor` for `Commit File…`.
export function initialList(
  request: CheckinRequest,
  lists: ListWithFiles[],
  active: string,
): string {
  if (request.kind === "project") {
    const selected = request.lists.find((id) => lists.some((list) => list.id === id));
    if (selected) return selected;
    for (const path of request.paths) {
      const holder = lists.find((list) => list.files.includes(path));
      if (holder) return holder.id;
    }
    return active;
  }
  const wanted = new Set(request.paths);
  const affected = lists.filter((list) => list.files.some((path) => wanted.has(path)));
  if (affected.some((list) => list.id === active)) return active;
  return affected.length === 1 ? affected[0].id : active;
}

// `VcsImplUtil.filterChangesUnder` for `Commit File…`: a root that is not itself a change, such as
// a directory, stands for the changes at or under it. Unversioned files are not changes: they go in
// only when they were chosen themselves (`CheckinActionUtil.getIncludedChanges`).
export function expandPaths(
  request: CheckinRequest,
  changed: string[],
  unversioned: string[],
): CheckinRequest {
  if (request.kind !== "files") return request;
  const known = new Set([...changed, ...unversioned]);
  const paths = request.paths.flatMap((root) => {
    if (known.has(root)) return [root];
    const prefix = `${root.replace(/\/+$/, "")}/`;
    return changed.filter((path) => path.startsWith(prefix));
  });
  return { ...request, paths: [...new Set(paths)] };
}

// `CheckinActionUtil.getIncludedChanges`: the selected changes and unversioned files; with none
// selected, every change of the list. What the dialog does not list cannot be included.
export function initialInclusion(
  request: CheckinRequest,
  list: ListWithFiles,
  unversioned: string[],
): Set<string> {
  if (request.kind === "project" && request.paths.length === 0) return new Set(list.files);
  const shown = new Set([...list.files, ...unversioned]);
  return new Set(request.paths.filter((path) => shown.has(path)));
}

// `AbstractCommitMessagePolicy.getCommitMessageForList`.
function listMessage(list: ChangeList): string | undefined {
  if (list.comment.trim() !== "") return list.comment;
  return hasDefaultName(list) ? undefined : list.name;
}

// `SingleChangeListCommitMessagePolicy.init`. An absent text is `undefined`, not an empty one.
export function initialMessage(input: {
  prefilled?: string;
  list: ChangeList;
  mergeMessage?: string;
  lastMessage?: string;
  clearInitial: boolean;
}): string {
  if (input.clearInitial) return input.prefilled ?? "";
  return (
    input.prefilled ?? listMessage(input.list) ?? input.mergeMessage ?? input.lastMessage ?? ""
  );
}

// `lastKnownComment` of `SingleChangeListCommitMessagePolicy`: the prefilled text, or, once the list
// had no message of its own, `LAST_COMMIT_MESSAGE` (even if `MERGE_MSG` was shown instead). A text
// picked from the history replaces it.
export function knownMessage(input: {
  prefilled?: string;
  list: ChangeList;
  lastMessage?: string;
}): string | undefined {
  if (input.prefilled !== undefined) return input.prefilled;
  return listMessage(input.list) === undefined ? input.lastMessage : undefined;
}

// `onChangelistChanged`: the draft stays as the old list's comment and the new list brings its own
// message, else `known` (`knownMessage`). The caller skips this when `Clear initial commit message`
// is on, or when the list is the same, as IntelliJ does.
export function onListChanged(
  draft: string,
  from: ChangeList,
  to: ChangeList,
  known?: string,
): { save: { id: string; comment: string }; message: string } {
  return {
    save: { id: from.id, comment: draft },
    message: listMessage(to) ?? known ?? "",
  };
}

// `onDialogClosed` and `onAfterCommit`: the comment to write when the dialog closes, if any.
// `committed` is a commit that went through; a partial one keeps the original comment, and a full
// one of the default list leaves it empty.
export function onClosed(input: {
  committed: boolean;
  draft: string;
  list: ChangeList;
  fullyIncluded: boolean;
}): { id: string; comment: string } | null {
  const { committed, draft, list, fullyIncluded } = input;
  if (!committed) return { id: list.id, comment: draft };
  if (!fullyIncluded) return null;
  return { id: list.id, comment: hasDefaultName(list) ? "" : draft };
}

export type Executor = "commit" | "shelve" | "patch";

// What an executor other than the commit receives (Shelve and Create Patch): the repository the
// changes are in, the list, the message, the included files (with the old name of a rename) and,
// as for the commit, the chosen lines.
export interface ExecutorChanges {
  repo: string;
  list: string;
  message: string;
  files: { path: string; oldPath: string | null }[];
  selections: FileSelection[];
}

// `close` closes the dialog: the executor calls it once it goes ahead (Create Patch, after Patch
// File Settings). Until then the dialog stays open, and an executor that gives up leaves it open.
// `here` says whether the repository is still the open one.
export interface ExecutorContext {
  close: () => void;
  here: () => boolean;
}

export type ExecutorRun = (changes: ExecutorChanges, context: ExecutorContext) => Promise<void>;

// `saveCommitMessageBeforeCommit` for an executor (`SingleChangeListCommitMessagePolicy.onDialogClosed`
// with `onBeforeCommit`): the message becomes the list's comment when the whole list goes in, and
// the list keeps its own comment otherwise. Unlike a commit, nothing clears it afterwards.
export function beforeExecutor(input: {
  draft: string;
  list: ChangeList;
  fullyIncluded: boolean;
}): { id: string; comment: string } | null {
  return input.fullyIncluded ? { id: input.list.id, comment: input.draft } : null;
}

// The texts come from VcsBundle (`commit.dialog.title`, `shelve.changes.action`,
// `create.patch.commit.action.title`, `action.name.create.patch`), GitBundle (`commit.action.name`)
// and DvcsBundle (`action.commit.and.push.text`), without mnemonics. Shelve and patch have no VCS
// options and no unversioned files.
export function dialogTexts(executor: Executor): {
  title: string;
  primary: string;
  dropdown: string[];
  options: boolean;
  unversioned: boolean;
} {
  switch (executor) {
    case "commit":
      return {
        title: "Commit Changes",
        primary: "Commit",
        dropdown: ["Commit and Push…"],
        options: true,
        unversioned: true,
      };
    case "shelve":
      return {
        title: "Shelve Changes",
        primary: "Shelve Changes",
        dropdown: [],
        options: false,
        unversioned: false,
      };
    case "patch":
      return {
        title: "Create Patch",
        primary: "Create Patch…",
        dropdown: [],
        options: false,
        unversioned: false,
      };
  }
}

// `hasDiffs`: with nothing included every button is disabled, even with amend.
export function canCommit(included: Set<string>): boolean {
  return included.size > 0;
}
