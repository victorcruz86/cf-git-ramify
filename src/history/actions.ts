import {
  clipboardWrite,
  compareLocalState,
  diffWindowOpenRequest,
  LOCAL_REVISION,
  selectionHistoryShowAffected,
  type SideRef,
} from "../lib/api";
import { contentTitle, requestTitle } from "../compare/actions";
import { revisionNumbersText } from "../lib/revisions";
import { LOCAL_CHANGES, type HistoryRow } from "./selection";

// The popup of the table (`VcsSelectionHistoryDialog.Popup`, `VA:479-484`), for the local file
// at `path`.

// `VcsHistoryUtil.showDifferencesInBackground.makeTitle`: the revision number, with ` (Local)`
// (`diff.title.local`, `V:104`) for the local file.
const LOCAL_SIDE_TITLE = `${LOCAL_CHANGES} (Local)`;

// `VcsHistoryUtil.loadContentForDiff`: a revision at its own path, the local file, or, past the
// oldest revision (`VcsFileRevision.NULL`), nothing with an empty number.
function side(row: HistoryRow | null, path: string): SideRef {
  if (row === null) return { revision: null, path, title: "" };
  if (row.kind === "local") return { revision: LOCAL_REVISION, path, title: LOCAL_SIDE_TITLE };
  return { revision: row.revision.hash, path: row.revision.path, title: row.revision.hash };
}

// `GitDiffFromHistoryHandler.showDiffForOne` and `showDiffForTwo` of a file (`doShowDiff` →
// `VcsHistoryUtil.showDiff`): titled by both revisions' paths when both are commits, else by the
// local file. A merge row does not get `Choose Parent to Compare`: it is compared with the row
// after it, as any other.
export async function compareRows(
  older: HistoryRow | null,
  newer: HistoryRow,
  path: string,
): Promise<void> {
  const { root } = await compareLocalState(path);
  const title =
    older?.kind === "revision" && newer.kind === "revision"
      ? requestTitle(root, older.revision.path, newer.revision.path)
      : contentTitle(root, path);
  await diffWindowOpenRequest({
    kind: "revisions",
    files: [{ left: side(older, path), right: side(newer, path) }],
    title,
  });
}

// `MyDiffAfterWithLocalAction`: the revision against the local file.
export const compareWithLocalFile = (row: HistoryRow, path: string) =>
  compareRows(row, { kind: "local", time: null }, path);

// `ShowAllAffectedGenericAction`: the commit in the main window's `Changes` window.
export async function showAffectedFiles(row: HistoryRow): Promise<void> {
  if (row.kind !== "revision") return;
  await selectionHistoryShowAffected({ hash: row.revision.hash, path: row.revision.path });
}

// `CopyRevisionNumberAction`: the selected revisions, the local row left out, oldest first.
export async function copyRevisionNumbers(rows: HistoryRow[]): Promise<void> {
  const hashes = rows.flatMap((row) => (row.kind === "revision" ? [row.revision.hash] : []));
  if (hashes.length > 0) await clipboardWrite(revisionNumbersText(hashes));
}
