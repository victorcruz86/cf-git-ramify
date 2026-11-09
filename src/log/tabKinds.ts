import { requestTab } from "../lib/tabs";
import type { FilterState } from "./filters";
import type { LogOptions } from "./options";

// What a tab of the Log shows:
// - `log`: the main tab and `Open New Git Log Tab`, named after their filters (`VcsLogTabsManager`);
// - `update`: `Update Info`, what an update received (`GitUpdateInfoAsLog`);
// - `history`: a file's history (`FileHistoryTabsManager`, group `History`);
// - `compare`: two branches as two logs (`GitCompareBranchesUi`, `Compare: <branch> and <current>`).
export type LogTabKind = "log" | "update" | "history" | "compare";

interface KindRule {
  // The tab's name from what the request named it: a `log` tab is named after its filters.
  title: (name: string) => string;
  // Whether the tab's options are remembered by the repository from one session to the next.
  persisted: boolean;
  // Whether a request with the key of an open tab of that kind goes back to it (find-or-create).
  reused: boolean;
}

const LOG = "Log";

export const LOG_TAB_KINDS: Record<LogTabKind, KindRule> = {
  log: { title: (name) => (name ? `${LOG}: ${name}` : LOG), persisted: true, reused: false },
  // `GitUpdateInfoAsLog.findOrCreateLogUi`, by range; not remembered.
  update: { title: (name) => `Update Info: ${name}`, persisted: false, reused: true },
  // `VcsLogFileHistoryProviderImpl` reuses the tab of that path and revision; 241 reopens the
  // history tabs when the log is created again (`FileHistoryTabsManager.logCreated`).
  history: { title: (name) => `History: ${name}`, persisted: true, reused: true },
  // `GitCompareBranchesFilesManager.openFile`, by ranges; its filters live in memory only.
  compare: { title: (name) => `Compare: ${name}`, persisted: false, reused: true },
};

// The file of a `history` tab, and the revision of `History Up to Here` (`null`: from the Branch
// filter).
export interface HistoryTarget {
  path: string;
  revision: string | null;
}

// The two branches of a `compare` tab (`VcsLogRangeFilter.RefRange`): its upper log shows
// `start..end`, the lower one `end..start`.
export interface CompareTarget {
  start: string;
  end: string;
}

// What a new tab starts with: its name (the part after the kind's prefix) and its filters; a
// `history` tab, its file; a `compare` tab, its branches; and what differs from the default
// options.
export interface LogTabInit {
  name: string;
  filters: FilterState;
  history?: HistoryTarget;
  compare?: CompareTarget;
  options?: Partial<LogOptions>;
}

// `matches`: a tab of that kind whose filters, as they are now, make it the one asked for (a
// folder's history goes back to the `log` tab with its filters, `VcsLogDirectoryHistoryProvider`);
// without it, `KindRule.reused` says whether the key finds a tab. `select`: the commit to show once
// the tab is there.
export interface LogTabRequest {
  matches?: (filters: FilterState) => boolean;
  select?: string;
}

export type LogTabOpener = (
  kind: LogTabKind,
  key: string,
  init: () => LogTabInit,
  request: LogTabRequest,
) => Promise<void>;

// The Log answers these requests: whoever wants a tab of it (a notification's `View Commits`, a
// file's `Show History`) does not hold the panel.
const openers = new Set<LogTabOpener>();
// Who waits for a Log to answer: the Git window mounts it after it comes to the front.
const waiting: (() => void)[] = [];

export function onLogTabRequested(opener: LogTabOpener): () => void {
  openers.add(opener);
  waiting.splice(0).forEach((resume) => resume());
  return () => openers.delete(opener);
}

// The tab of that kind and key, or a new one from `init` (called only then) when the kind is not
// reused or no tab has that key. The Log comes to the front of the Git window first, which mounts
// it if it was not there. Resolves once the tab shows.
export async function openLogTab(
  kind: LogTabKind,
  key: string,
  init: () => LogTabInit,
  request: LogTabRequest = {},
): Promise<void> {
  requestTab("log");
  if (openers.size === 0) await new Promise<void>((resume) => waiting.push(resume));
  await Promise.all([...openers].map((open) => open(kind, key, init, request)));
}
