import type { Settings } from "../lib/api";

type PreviewSettings = Pick<Settings, "previewPushOnCommit" | "previewPushProtectedOnly">;

// `GitPushAfterCommitDialog.showOrPush`.
export function shouldPreview(
  { previewPushOnCommit, previewPushProtectedOnly }: PreviewSettings,
  protectedTarget: boolean,
  canPush: boolean,
): boolean {
  return (previewPushOnCommit && (!previewPushProtectedOnly || protectedTarget)) || !canPush;
}

// `commit` says whether the commit was made: one that fails, or is cancelled in a warning, pushes nothing.
// `alive`: the commit view is still open, and with it its repository.
export async function commitAndPush(
  commit: () => Promise<boolean>,
  push: () => Promise<void>,
  alive: () => boolean = () => true,
): Promise<void> {
  if ((await commit()) && alive()) await push();
}
