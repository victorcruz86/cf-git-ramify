import type { ApiError, SelectionLost } from "./api";

function isApiError(failure: unknown): failure is ApiError {
  return typeof failure === "object" && failure !== null && "kind" in failure;
}

// `conflict_resolve` with a file that changed after reading it: the merge window shows it and
// disables Apply. The kind is checked, never the text.
export function isOutdated(failure: unknown): boolean {
  return isApiError(failure) && failure.kind === "outdated";
}

function lostCount(lost: SelectionLost, places: string[]): string {
  const count =
    lost.hunks.length === 1
      ? "1 selected change no longer exists"
      : `${lost.hunks.length} selected changes no longer exist`;
  return places.length > 0 ? `${count} (${places.join(", ")})` : count;
}

// Devin changed a file with chosen lines: what still exists is not touched.
export function lostText(lost: SelectionLost, places: string[] = []): string {
  return `"${lost.path}" has changed: ${lostCount(lost, places)}. The rest stays selected`;
}

export function lostRollbackText(lost: SelectionLost): string {
  return `"${lost.path}" has changed: ${lostCount(lost, [])}, so nothing was rolled back`;
}

export function lostStageText(lost: SelectionLost): string {
  return `"${lost.path}" has changed: ${lostCount(lost, [])}, so the stage was not touched`;
}

// The backend messages are already readable. `commit_running` also fires during a slow rollback,
// and `selection_lost` needs to say which file to touch.
export function errorText(failure: unknown, places: string[] = []): string {
  if (failure instanceof Error) return failure.message;
  if (!isApiError(failure)) return String(failure);
  if (failure.kind === "commit_running") {
    return "There is another operation in progress in this repository. Wait for it to finish and try again";
  }
  if (failure.kind === "partial_merge") {
    return "A merge must be committed whole: include all of its changes in the commit, without leaving files out or choosing lines";
  }
  if (failure.kind === "selection_lost" && failure.details) {
    const { path } = failure.details;
    const count = lostCount(failure.details, places);
    return `"${path}" has changed: ${count}, so the commit was not made. Review the selection and try again`;
  }
  return failure.message;
}
