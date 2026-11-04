// The texts of `GitUnstructuredEditor`, the free text editor that IntelliJ opens for the message of
// a `reword` and for a rebase list it does not understand (`GitBundle`:
// rebase.interactive.edit.commit.message.dialog.title, ...ok.action.title y
// rebase.interactive.unstructured.editor.dialog.root.label).
export const EDITOR_TITLE = "Git Commit Message";
export const EDITOR_OK = "Continue Rebasing";

export const rootLabel = (root: string): string => `Git Root: ${root}`;

// `rebase.interactive.noop.dialog.title` and `rebase.interactive.noop.dialog.text`.
export const NOOP_TITLE = "Git Rebase";
export const NOOP_TEXT =
  "There are no commits to rebase because the current branch is directly below the base branch, or they point to the same commit (the 'noop' situation).\n Do you want to continue (this will reset the current branch to the base branch)?";
