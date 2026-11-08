export type ActionId =
  | "ActivateCommitToolWindow"
  | "ActivateVersionControlToolWindow"
  | "ChangesView.AddUnversioned"
  | "ChangesView.GroupBy.Directory"
  | "ChangesView.Move"
  | "ChangesView.Rename"
  | "ChangesView.Revert"
  | "ChangesView.SaveToShelve"
  | "ChangesView.SetDefault"
  | "ChangesView.ShelveSilently"
  | "CheckinProject"
  | "CollapseAll"
  | "CommitView.Commit"
  | "Diff.ShowDiff"
  | "ExpandAll"
  | "Git.Branches"
  | "Git.Commit.And.Push.Executor"
  | "ShowSettings"
  | "Vcs.CopyRevisionNumberAction"
  | "Vcs.Log.FocusTextFilter"
  | "Vcs.Log.GoToChild"
  | "Vcs.Log.GoToParent"
  | "Vcs.Log.GoToRef"
  | "Vcs.Log.Refresh"
  | "Vcs.Log.ShowTooltip"
  | "Vcs.Push"
  | "Vcs.ShowMessageHistory"
  | "Vcs.ToggleAmendCommitMode"
  | "Vcs.UpdateProject";

const BINDINGS: Record<string, ActionId> = {
  "Meta+K": "CheckinProject",
  "Alt+Meta+K": "Git.Commit.And.Push.Executor",
  "Meta+T": "Vcs.UpdateProject",
  "Meta+Shift+K": "Vcs.Push",
  "Meta+0": "ActivateCommitToolWindow",
  "Meta+9": "ActivateVersionControlToolWindow",
  "Alt+Meta+Z": "ChangesView.Revert",
  // `Git.Stage.Add` uses the shortcut of `ChangesView.AddUnversioned`.
  "Alt+Meta+A": "ChangesView.AddUnversioned",
  "Meta+Shift+M": "ChangesView.Move",
  // `ChangesView.SaveToShelve` has no shortcut: it is only in the keymap.
  "Meta+Shift+H": "ChangesView.ShelveSilently",
  "Ctrl+M": "Vcs.ShowMessageHistory",
  "Ctrl+Alt+M": "Vcs.ToggleAmendCommitMode",
  F2: "ChangesView.Rename",
  "Shift+F6": "ChangesView.Rename",
  "Ctrl+Space": "ChangesView.SetDefault",
  "Meta+D": "Diff.ShowDiff",
  "Ctrl+P": "ChangesView.GroupBy.Directory",
  "Meta+Enter": "CommitView.Commit",
  "Ctrl+Enter": "CommitView.Commit",
  "Meta+=": "ExpandAll",
  "Meta++": "ExpandAll",
  "Meta+-": "CollapseAll",
  "Meta+,": "ShowSettings",
  "Meta+R": "Vcs.Log.Refresh",
  "Meta+F": "Vcs.Log.GoToRef",
  "Meta+L": "Vcs.Log.FocusTextFilter",
  ArrowRight: "Vcs.Log.GoToParent",
  ArrowLeft: "Vcs.Log.GoToChild",
  F1: "Vcs.Log.ShowTooltip",
  "Ctrl+J": "Vcs.Log.ShowTooltip",
  "Alt+Meta+Shift+C": "Vcs.CopyRevisionNumberAction",
};

// With ⌥ macOS changes `key` to another character (⌥Z gives "Ω"): there the physical key counts.
function keyName(event: KeyboardEvent): string {
  if (event.altKey && /^Key[A-Z]$/.test(event.code)) return event.code.slice(3);
  if (event.key === " ") return "Space";
  return event.key.length === 1 ? event.key.toUpperCase() : event.key;
}

function chord(event: KeyboardEvent): string {
  const parts: string[] = [];
  if (event.ctrlKey) parts.push("Ctrl");
  if (event.altKey) parts.push("Alt");
  if (event.metaKey) parts.push("Meta");
  // ⌘+ is ⇧= on many keyboards: there ⇧ is part of the character and not of the shortcut.
  if (event.shiftKey && event.key !== "+") parts.push("Shift");
  parts.push(keyName(event));
  return parts.join("+");
}

export function matchShortcut(event: KeyboardEvent): ActionId | null {
  return BINDINGS[chord(event)] ?? null;
}

// The chords of an action, as `matchShortcut` reads them (`Meta+Shift+H`).
export function shortcutsOf(action: ActionId): string[] {
  return Object.keys(BINDINGS).filter((chord) => BINDINGS[chord] === action);
}

export type BranchPanelActionId =
  | "Git.Log.Branches.Navigate.Log.To.Selected.Branch"
  | "Git.New.Branch.In.Log"
  | "Git.Rename.Local.Branch";

// They apply with focus in the branches panel: the selection comes from
// `BranchesTreePanel.getData`, and outside it F2 is `ChangesView.Rename`.
const BRANCH_PANEL_BINDINGS: Record<string, BranchPanelActionId> = {
  F2: "Git.Rename.Local.Branch",
  "Shift+F6": "Git.Rename.Local.Branch",
  "Alt+F1": "Git.Log.Branches.Navigate.Log.To.Selected.Branch",
  "Alt+Meta+N": "Git.New.Branch.In.Log",
};

export function matchBranchPanelShortcut(event: KeyboardEvent): BranchPanelActionId | null {
  return BRANCH_PANEL_BINDINGS[chord(event)] ?? null;
}

export type ShelfActionId =
  | "ChangesView.UnshelveSilently"
  | "ShelveChanges.UnshelveWithDialog"
  | "ShelvedChanges.Rename";

// They apply with focus in the Shelf tab, whose tree gives the selection: `ChangesView.UnshelveSilently`
// (`VcsActions.xml:239`), `ShelveChanges.UnshelveWithDialog` and `ShelvedChanges.Rename`
// (`$default.xml:1128,1131`), with control and meta swapped for macOS.
const SHELF_BINDINGS: Record<string, ShelfActionId> = {
  "Alt+Meta+U": "ChangesView.UnshelveSilently",
  "Meta+Shift+U": "ShelveChanges.UnshelveWithDialog",
  F2: "ShelvedChanges.Rename",
  "Shift+F6": "ShelvedChanges.Rename",
};

export function matchShelfShortcut(event: KeyboardEvent): ShelfActionId | null {
  return SHELF_BINDINGS[chord(event)] ?? null;
}

export type LogActionId = "Git.Reword.Commit";

// They apply with focus in the log table, like F2 in the branches panel: outside it F2 is
// `ChangesView.Rename`.
const LOG_BINDINGS: Record<string, LogActionId> = {
  F2: "Git.Reword.Commit",
  "Shift+F6": "Git.Reword.Commit",
};

export function matchLogShortcut(event: KeyboardEvent): LogActionId | null {
  return LOG_BINDINGS[chord(event)] ?? null;
}

export type DiffActionId =
  | "CloseDiff"
  | "Diff.NextChange"
  | "Diff.PrevChange"
  | "NextDiff"
  | "PreviousDiff"
  | "SaveAll"
  | "Vcs.MoveChangedLinesToChangelist"
  | "Vcs.RollbackChangedLines";

// The actions of the diff window: `Diff.*` switch file, `NextDiff` switches change.
const DIFF_BINDINGS: Record<string, DiffActionId> = {
  F7: "NextDiff",
  "Shift+F7": "PreviousDiff",
  "Ctrl+Shift+ArrowRight": "Diff.NextChange",
  "Ctrl+Shift+ArrowLeft": "Diff.PrevChange",
  "Meta+S": "SaveAll",
  "Alt+Meta+Z": "Vcs.RollbackChangedLines",
  "Meta+Shift+M": "Vcs.MoveChangedLinesToChangelist",
  Escape: "CloseDiff",
  "Meta+W": "CloseDiff",
};

export function matchDiffShortcut(event: KeyboardEvent): DiffActionId | null {
  return DIFF_BINDINGS[chord(event)] ?? null;
}
