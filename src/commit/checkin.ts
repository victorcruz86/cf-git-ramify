// What Local Changes asks of the Commit Changes dialog: `Commit…` (`CheckinProject`) and `Commit
// File…` (`CheckinFiles`) only say what is selected; the dialog decides the list and the inclusion
// (`CheckinActionUtil`).
export interface CheckinRequest {
  kind: "project" | "files";
  // The selected files, in the order of the tree.
  paths: string[];
  // The ids of the selected changelist nodes.
  lists: string[];
}

// A selected path for `CheckinFiles`: a file, or a directory that is itself a node.
export interface CheckinRoot {
  path: string;
  directory: boolean;
}

// `CommonCheckinFilesAction.appendSubject`: the subject depends on the first root and the number on
// all of them (`action.name.checkin.file`, `action.name.checkin.directory`).
export function checkinFilesText(roots: CheckinRoot[]): string {
  if (roots.length === 0) return "Commit…";
  const many = roots.length > 1;
  if (roots[0].directory) return many ? "Commit Directories…" : "Commit Directory…";
  return many ? "Commit Files…" : "Commit File…";
}
