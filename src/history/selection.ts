import type { BlockLines, SelectionRevision, SelectionUpdate } from "../lib/api";
import { prettyDateTime } from "../log/draw";
import { shortHash } from "../compare/actions";

// The texts of `VcsSelectionHistoryDialog` (241).
// `show.history.dialog.title.template` with `action.name.show.history.for.selection` (`V:626-627`).
export const WINDOW_TITLE = "History for Selection";
// `selection.history.local.revision.text` (`V:899`): the first row.
export const LOCAL_CHANGES = "Local Changes";
// `CurrentRevision.getCommitMessage`: `[` + `vcs.revision.name.current` (`V:15`) + `]`.
export const CURRENT_MESSAGE = "[Current]";
// `history.loading.revisions` (`V:12`): the table before the history arrives.
export const LOADING_REVISIONS = "Loading affected revisions…";
// `CommonBundle.getLoadingTreeNodeText` (`tree.node.loading`): the status, and `LoadingDiffRequest`.
export const LOADING = "Loading...";
// `NoDiffRequest` (`message.nothingToShow`, IdeCoreBundle).
export const NOTHING_TO_SHOW = "Nothing to show";
// `checkbox.show.changed.revisions.only` (`V:180`).
export const CHANGES_ONLY = "Changes only";
// `selection.history.commit.message.label` (`V:898`).
export const COMMIT_MESSAGE_LABEL = "Commit Message:";
// `column.name.revision.version`, `.date`, `column.name.revision.list.author` and
// `label.selected.revision.commit.message` (`V:159,160,110,158`).
export const COLUMNS = ["Version", "Date", "Author", "Commit Message"] as const;

// `selection.history.can.not.load.message` (`V:900`), with the error's text when there is one.
export const cannotLoad = (message: string | null) =>
  `Cannot load revision contents${message ? `: ${message}` : ""}`;
// `diff.content.title.revision.number` (`V:182`): the revision number as `toString` gives it, the
// full hash, or `Local Changes`.
export const revisionTitle = (number: string) => `Revision ${number}`;
// `selection.history.loading.revision.status` (`V:897`).
const loadingRevision = (short: string) => `Loading revision ${short}…`;

// A row of the table: the local text (`CurrentRevision` with `LOCAL_REVISION_NUMBER`), then the
// revisions of the history, newest first.
export type HistoryRow =
  | { kind: "local"; time: number | null }
  | { kind: "revision"; revision: SelectionRevision };

// `BlockData`: `rows` is empty until the history arrives; `blocks[i]` is row `i`'s block once
// loaded, `null` for `EMPTY_BLOCK`.
export interface HistoryData {
  rows: HistoryRow[];
  blocks: (BlockLines | null)[];
  loading: boolean;
  error: string | null;
}

export const emptyData = (): HistoryData => ({ rows: [], blocks: [], loading: true, error: null });

// What an update of the stream adds.
export function withUpdate(data: HistoryData, update: SelectionUpdate): HistoryData {
  if (update.kind === "revisions") {
    const rows: HistoryRow[] = [
      { kind: "local", time: update.localTime },
      ...update.revisions.map((revision) => ({ kind: "revision" as const, revision })),
    ];
    return { ...data, rows };
  }
  const blocks = [...data.blocks];
  blocks[update.index] = update.block;
  return { ...data, blocks };
}

const linesOf = (block: BlockLines | null) => block?.lines ?? [];

function sameLines(one: BlockLines | null, other: BlockLines | null): boolean {
  const [a, b] = [linesOf(one), linesOf(other)];
  return a.length === b.length && a.every((line, index) => line === b[index]);
}

// `filteredRevisions`: a row whose block differs from the older one's, until the block is gone,
// and the oldest revision when it still has the block.
function changedRows({ rows, blocks }: HistoryData): number[] {
  const kept: number[] = [];
  for (let index = 1; index < rows.length; index++) {
    const [newer, older] = [blocks[index - 1], blocks[index]];
    if (newer === undefined || older === undefined) break;
    if (!sameLines(newer, older)) kept.push(index - 1);
    if (older === null) break;
  }
  const initial = rows.length - 1;
  const block = blocks[initial];
  if (initial >= 0 && block !== undefined && block !== null) kept.push(initial);
  return kept;
}

// The rows the table shows, as indexes of `rows`.
export function visibleRows(data: HistoryData, changesOnly: boolean): number[] {
  return changesOnly ? changedRows(data) : data.rows.map((_, index) => index);
}

// `getSelectedRevisionsRange`: from the first selected row to the last, both in `rows`.
function selectedRange(selected: number[]): [number, number] {
  if (selected.length === 0) return [0, 0];
  return [Math.min(...selected), Math.max(...selected) + 1];
}

// `updateRevisionsList`: what stays selected of the range the selection spanned; with none, the
// nearest newer row that is shown (`getNearestVisibleRevision`), else the first one.
export function keptSelection(selected: number[], visible: number[]): number[] {
  const [first, end] = selectedRange(selected);
  const kept = visible.filter((index) => index >= first && index < end);
  if (kept.length > 0 || visible.length === 0) return kept;
  const anchor = selected.length > 0 ? first : -1;
  for (let index = anchor - 1; index > 0; index--) {
    if (visible.includes(index)) return [index];
  }
  return [visible[0]];
}

// A side of the block diff: its text, the line of the file where it starts and its title.
export interface BlockSide {
  text: string;
  start: number;
  title: string | null;
}

export type BlockDiff =
  | { kind: "message"; text: string }
  | { kind: "texts"; older: BlockSide; newer: BlockSide };

const rowNumber = (row: HistoryRow) => (row.kind === "local" ? LOCAL_CHANGES : row.revision.hash);

// `createDiffContent` and `createDiffContentTitle`: past the last row an empty side without a
// title; a block not read yet, `undefined`.
function blockSide({ rows, blocks }: HistoryData, index: number): BlockSide | undefined {
  if (index >= rows.length) return { text: "", start: 0, title: null };
  const block = blocks[index];
  if (block === undefined) return undefined;
  const title = revisionTitle(rowNumber(rows[index]));
  if (block === null) return { text: "", start: 0, title };
  return { text: block.lines.join("\n"), start: block.start, title };
}

// `updateDiff`: the block of the row after the selection on the left, of the first selected row
// on the right.
export function blockDiff(data: HistoryData, selected: number[]): BlockDiff {
  if (selected.length === 0) return { kind: "message", text: NOTHING_TO_SHOW };
  const count = data.rows.length;
  if (count === 0) return { kind: "message", text: LOADING };
  const [first, end] = selectedRange(selected);
  if (first === count && end === count) return { kind: "message", text: NOTHING_TO_SHOW };
  const [older, newer] = [blockSide(data, end), blockSide(data, first)];
  if (older && newer) return { kind: "texts", older, newer };
  return { kind: "message", text: data.loading ? LOADING : cannotLoad(data.error) };
}

// `updateStatusPanel`: the revision being read and how many blocks there are, while it loads.
export function statusText({ rows, blocks, loading }: HistoryData): string {
  if (!loading) return "";
  const current = rows.length > 0 && blocks.length < rows.length ? rows[blocks.length] : null;
  const text = current ? loadingRevision(versionText(current)) : LOADING;
  return rows.length === 0 ? text : `${text} (${blocks.length}/${rows.length})`;
}

// `RevisionColumnInfo`: `VcsUtil.getShortRevisionString`.
export const versionText = (row: HistoryRow) =>
  row.kind === "local" ? LOCAL_CHANGES : shortHash(row.revision.hash);

// `DateColumnInfo`: `formatPrettyDateTime` of the commit time (the local file's time first).
export function dateText(row: HistoryRow, now: number): string {
  const seconds = row.kind === "local" ? row.time : row.revision.commitTime;
  return seconds === null ? "" : prettyDateTime(seconds * 1000, now);
}

// `AuthorColumnInfo`: the author, with `*` when someone else committed; the local row has none.
export function authorText(row: HistoryRow): string {
  if (row.kind === "local") return "";
  const { author, committer } = row.revision;
  return author === committer ? author : `${author}*`;
}

// `getCustomizedRenderer`: `name <email>`, and `, via {0}` (`V:889`) the committer when it differs.
export function authorTooltip(row: HistoryRow): string | null {
  if (row.kind === "local") return null;
  const { author, authorEmail, committer, committerEmail } = row.revision;
  const tooltip = `${author} <${authorEmail}>`;
  return author === committer ? tooltip : `${tooltip}, via ${committer} <${committerEmail}>`;
}

// `getCommitMessage`.
export const messageText = (row: HistoryRow) =>
  row.kind === "local" ? CURRENT_MESSAGE : row.revision.message;

// `MessageColumnInfo.getSubject`: up to the first line break.
export function subjectText(row: HistoryRow): string {
  const message = messageText(row);
  const end = message.search(/[\n\r]/);
  return end < 0 ? message : message.slice(0, end);
}

// What the popup's entries can do with the selection (`VCS_FILE_REVISION` is the first selected
// row unless it is the local one; `VCS_FILE_REVISIONS` leaves the local one out).
export interface SelectionActions {
  compare: boolean;
  compareWithLocal: boolean;
  showAllAffected: boolean;
  copyRevisionNumber: boolean;
  showInGitLog: boolean;
}

export function selectionActions(rows: HistoryRow[], selected: number[]): SelectionActions {
  const chosen = [...selected].sort((a, b) => a - b).map((index) => rows[index]);
  const one = chosen.length === 1 ? chosen[0] : null;
  return {
    // `MyDiffAction.update`.
    compare: chosen.length > 1 || one?.kind === "revision",
    // `MyDiffAfterWithLocalAction.update`.
    compareWithLocal: one?.kind === "revision",
    // `ShowAllAffectedGenericAction.update`.
    showAllAffected: chosen[0]?.kind === "revision",
    // `CopyRevisionNumberAction.update`.
    copyRevisionNumber: chosen.some((row) => row.kind === "revision"),
    // `GitShowCommitInLogAction.update`: `VCS_FILE_REVISION`, as Show All Affected Files.
    showInGitLog: chosen[0]?.kind === "revision",
  };
}

// The two rows `MyDiffAction` compares: the one after the selection (none past the last) and the
// first selected.
export function comparedRows(
  rows: HistoryRow[],
  selected: number[],
): { older: HistoryRow | null; newer: HistoryRow } | null {
  if (rows.length === 0 || selected.length === 0) return null;
  const [first, end] = selectedRange(selected);
  return { older: rows[end] ?? null, newer: rows[first] };
}
