import type { IgnoreEntry, IgnoreTarget, IgnoreTargets } from "../lib/api";
import type { MenuAction } from "../lib/menu";
import { collectFiles, type TreeNode } from "./tree";

// `Git.Ignore.File` (`GitIgnoreFileActionGroup`) of `ChangesViewPopupMenu`, after `Add to VCS`.
// `vcs.add.to.ignore.file.action.group.text` with `.gitignore`.
export const ADD_TO_GITIGNORE = "Add to .gitignore";
// `git.add.to.exclude.file.action.text`.
export const GIT_EXCLUDE = ".git/info/exclude";
// `vcs.add.to.ignore.file.create.ignore.file.confirmation.title` and `.message`.
export const CREATE_GITIGNORE_TITLE = "Create .gitignore";
export const createGitignoreQuestion = (location: string) => `Create .gitignore file in ${location}?`;

const untracked = (node: TreeNode) => {
  const files = collectFiles([node]);
  return files.length > 0 && files.every((file) => file.state === "untracked");
};

// `getSelectedFiles`: the files and folders selected in the tree (`EXACTLY_SELECTED_FILES`), else
// every file under the selection. Only unversioned ones: a folder counts when all its files are.
export function ignoreEntries(nodes: TreeNode[]): IgnoreEntry[] {
  const exact: IgnoreEntry[] = [];
  for (const node of nodes) {
    if (node.kind === "file" && node.change?.state === "untracked") {
      exact.push({ path: node.change.path, directory: false });
    } else if (node.kind === "directory" && node.folder && untracked(node)) {
      exact.push({ path: node.folder, directory: true });
    }
  }
  if (exact.length > 0) return exact;
  return collectFiles(nodes)
    .filter((file) => file.state === "untracked")
    .map((file) => ({ path: file.path, directory: false }));
}

// The group's rows (`IgnoreFileActionGroup.update`): the `.gitignore` files that cover the selection
// by their path from the root, else `Add to .gitignore` to create one, then `.git/info/exclude`
// (`AddToGitExcludeAction`). Without the targets, the exclude file alone.
export function ignoreMenu(
  targets: IgnoreTargets | null,
  run: (target: IgnoreTarget) => void,
): MenuAction {
  const rows: MenuAction[] = (targets?.gitignores ?? []).map((path) => ({
    label: path,
    run: () => run({ kind: "gitignore", path }),
  }));
  if (rows.length === 0 && targets?.create) {
    rows.push({ label: ADD_TO_GITIGNORE, run: () => run({ kind: "create" }) });
  }
  rows.push({ label: GIT_EXCLUDE, run: () => run({ kind: "exclude" }) });
  return { label: ADD_TO_GITIGNORE, submenu: rows };
}
