import type { SortType } from "../lib/api";

export type LogColumn = "subject" | "author" | "date" | "hash";
export type DiffPreviewAt = "bottom" | "right";

export interface Highlights {
  myCommits: boolean;
  mergeCommits: boolean;
  currentBranch: boolean;
}

// Split of the dividers: what the first pane of each one takes.
export interface Splits {
  table: number;
  details: number;
  diff: number;
  branches: number;
}

export interface LogOptions {
  sort: SortType;
  compactRefs: boolean;
  tagNames: boolean;
  longEdges: boolean;
  preferCommitDate: boolean;
  refsOnLeft: boolean;
  columns: LogColumn[];
  highlights: Highlights;
  showDetails: boolean;
  diffPreview: boolean;
  diffPreviewAt: DiffPreviewAt;
  showChangesToParents: boolean;
  showOnlyAffected: boolean;
  // `SHOW_GIT_BRANCHES_LOG_PROPERTY`: by default, only on the main tab.
  showBranches: boolean;
  splits: Splits;
}

// `VcsLogApplicationSettings.State`, and the proportions of `MainFrame`.
export const defaultOptions = (main = false): LogOptions => ({
  sort: "off",
  compactRefs: true,
  tagNames: false,
  longEdges: false,
  preferCommitDate: false,
  refsOnLeft: false,
  columns: ["subject", "author", "date"],
  highlights: { myCommits: true, mergeCommits: true, currentBranch: true },
  showDetails: true,
  diffPreview: false,
  diffPreviewAt: "bottom",
  showChangesToParents: false,
  showOnlyAffected: false,
  showBranches: main,
  splits: { table: 0.7, details: 0.7, diff: 0.7, branches: 0.3 },
});

// What was saved may come from another version: what is missing takes its default value.
export function readOptions(stored: unknown, main = false): LogOptions {
  const options = defaultOptions(main);
  if (typeof stored !== "object" || stored === null) return options;
  const values = stored as Partial<LogOptions>;
  return {
    ...options,
    ...values,
    highlights: { ...options.highlights, ...values.highlights },
    splits: { ...options.splits, ...values.splits },
  };
}

// Showing a column puts it at the end, as `VcsLogColumnOrderStorage` does.
export const toggleColumn = (columns: LogColumn[], column: LogColumn): LogColumn[] =>
  columns.includes(column) ? columns.filter((other) => other !== column) : [...columns, column];
