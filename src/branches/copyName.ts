import { clipboardWrite } from "../lib/api";
import { errorText } from "../lib/errors";
import { notify } from "../lib/notifications";

// Not in IntelliJ 2024.1 (`docs/parity-matrix.md`): the branch menus and ⌘C in the branches popup
// put the full name on the clipboard, a remote branch with its remote (`origin/feature/x`).
export const COPY_BRANCH_NAME = "Copy Branch Name";

export async function copyBranchName(name: string, write: (text: string) => Promise<void> = clipboardWrite) {
  try {
    await write(name);
    notify({ kind: "info", title: "Branch name copied", body: name });
  } catch (failure) {
    notify({ kind: "error", title: "Could not copy the branch name", body: errorText(failure) });
  }
}
