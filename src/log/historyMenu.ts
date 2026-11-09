import type { HistoryEntry } from "../lib/api";
import type { MenuAction, MenuItem } from "../lib/menu";
import { revisionsText } from "./contextMenu";

// A selected row of the history, from the top: its commit, and the file in it (`null` while the
// entry is not read).
export interface HistoryCommit {
  hash: string;
  subject: string;
  parentCount: number;
  entry: HistoryEntry | null;
}

// Who carries out each entry; without `vcs` the branch operations come out disabled.
export interface HistoryHandlers {
  copy: (text: string) => void;
  createPatch?: () => void;
  get: (commit: HistoryCommit) => void;
  showFileAtRevision: (commit: HistoryCommit) => void;
  compareWithLocal: (commit: HistoryCommit) => void;
  showDiff: () => void;
  showAllAffected: (commit: HistoryCommit) => void;
  annotate: (commit: HistoryCommit) => void;
  showInGitLog: (commit: HistoryCommit) => void;
  revert?: (commits: HistoryCommit[]) => void;
  newTag?: (commit: HistoryCommit) => void;
}

const SEPARATOR = null;
// `action.description.cant.revert.merge.commit`.
const CANT_REVERT_MERGE = "Reverting merge commits is not allowed";

// `L:67-68`: `Show Diff` with one row, `Compare` with two or more.
export const showDiffLabel = (count: number) => (count > 1 ? "Compare" : "Show Diff");

// `Vcs.FileHistory.ContextMenu` (`LX:233-245`) with `Git.FileHistory.ContextMenu` in
// `VcsHistoryActionsGroup` (`X:291-299`), without `Show Diff in a New Tab or Window` (decision 8) and
// the hosting group. `FileHistorySingleCommitAction`: one row; `Get`, `Show File at Revision` and
// `Annotate` are off on the row that deleted the file (`FileHistoryModel.createRevision` gives
// `NULL`).
export function historyMenu(commits: HistoryCommit[], handlers: HistoryHandlers): MenuItem[] {
  if (commits.length === 0) return [];
  const single = commits.length === 1 ? commits[0] : null;
  const present = single !== null && single.entry !== null && !single.entry.deleted;
  const onOne = (label: string, run: (commit: HistoryCommit) => void, enabled = true): MenuAction => ({
    label,
    disabled: !single || !enabled,
    run: () => single && run(single),
  });
  const merges = commits.some((commit) => commit.parentCount > 1);
  const { revert, newTag } = handlers;
  return [
    {
      label: "Copy Revision Number",
      shortcut: "⌥⇧⌘C",
      run: () => handlers.copy(revisionsText(commits.map((commit) => commit.hash))),
    },
    {
      label: "Create Patch...",
      disabled: !handlers.createPatch,
      run: () => handlers.createPatch?.(),
    },
    SEPARATOR,
    onOne("Get", handlers.get, present),
    onOne("Show File at Revision", handlers.showFileAtRevision, present),
    onOne("Compare with Local", handlers.compareWithLocal),
    { label: showDiffLabel(commits.length), shortcut: "⌘D", run: handlers.showDiff },
    { ...onOne("Show All Affected Files", handlers.showAllAffected), shortcut: "⌃⌘A" },
    // `Vcs.Log.AnnotateRevisionAction` (`AnnotateRevisionFromHistoryAction`): off on the row that
    // deleted the file, as `Get`.
    onOne("Annotate", handlers.annotate, present),
    onOne("Show in Git Log", handlers.showInGitLog),
    SEPARATOR,
    {
      label: commits.length === 1 ? "Revert Commit" : "Revert Commits",
      disabled: !revert || merges,
      title: merges ? CANT_REVERT_MERGE : undefined,
      run: () => revert?.(commits),
    },
    SEPARATOR,
    { ...onOne("New Tag…", (commit) => newTag?.(commit)), disabled: !single || !newTag },
  ];
}
