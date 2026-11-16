// The texts of `VirtualFileDeleteProvider` (2024.1, `UIBundle` and `IdeBundle`) for `$Delete` of
// unversioned files in Local Changes. Ramify passes files only: a folder of the tree stands for the
// unversioned files under it, which are counted one by one.
export const DELETE_TITLE = "Delete";
export const ERROR_TITLE = "Error";
const SHOWN_PROBLEMS = 10;

const nameOf = (path: string) => path.slice(path.lastIndexOf("/") + 1);

// `are.you.sure.you.want.to.delete.selected.file(s.and.files).confirmation.message`.
export function deleteQuestion(paths: string[]): string {
  return paths.length === 1
    ? `Delete '${nameOf(paths[0])}'?`
    : `Are you sure you want to delete ${paths.length} selected files?`;
}

// `dialog.message.could.not.erase.files.or.folders.0.1`: at most ten names, then `...`.
export function deleteProblems(names: string[]): string {
  const more = names.length > SHOWN_PROBLEMS ? "\n  ..." : "";
  return `Could not erase files or folders:\n${names.slice(0, SHOWN_PROBLEMS).join(",\n  ")}${more}`;
}
