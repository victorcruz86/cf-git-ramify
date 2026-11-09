import { changesAdd, repoStatus, snapshotsRestore } from "../lib/api";
import { errorText } from "../lib/errors";
import { notify } from "../lib/notifications";
import { taskResult } from "../lib/tasks";
import { RESTORED } from "../patch/flow";

// The texts of a failed add, here and in `ChangesView.AddUnversioned` (`ChangesPanel.svelte`).
export const ADD_FAILED = "Failed to add files";
export const ADD_PROBLEMS = "The following problems have occurred when adding the files:";
// rama's own, as the other copies of what was only in the stage say it.
export const STAGE_COPY_SAVED = "A snapshot was saved of what existed only in the stage.";

// `Git.Add` of the Git file menu (`GitFileUtils.addPaths`: `git add --ignore-errors -A`): a folder
// adds what is under it, and the repository's own folder ("") everything that changed. What was
// only in the index is saved first, and can be restored.
export async function addFiles(repo: string, paths: string[]): Promise<void> {
  const wanted = paths.includes("")
    ? (await repoStatus(false)).map((change) => change.path)
    : paths;
  if (wanted.length === 0) return;
  let copy: string | null;
  try {
    copy = await taskResult<string | null>(await changesAdd(repo, wanted));
  } catch (failure) {
    if ((failure as { kind?: string }).kind === "cancelled") return;
    notify({ kind: "error", title: ADD_FAILED, body: `${ADD_PROBLEMS}\n${errorText(failure)}` });
    return;
  }
  if (copy) {
    const saved = copy;
    notify({
      kind: "info",
      title: STAGE_COPY_SAVED,
      actions: [{ label: "Restore", run: () => void restore(saved, repo) }],
    });
  }
}

async function restore(copy: string, repo: string) {
  try {
    const previous = await snapshotsRestore(copy, repo);
    notify({
      kind: "info",
      title: RESTORED,
      actions: [{ label: "Undo", run: () => void restore(previous, repo) }],
    });
  } catch (failure) {
    notify({ kind: "error", title: errorText(failure) });
  }
}
