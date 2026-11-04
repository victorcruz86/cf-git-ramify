import {
  commitFiles,
  fileSides,
  logRows,
  mergeWindowOpen,
  onDiffDrawn,
  perfReport,
  repoStatus,
  type DiffTiming,
  type FileChange,
} from "../lib/api";
import { errorText } from "../lib/errors";
import { frameStats } from "./stats";

const SCROLL_MS = 10_000;
const PAGE = 200;
const DIFF_TIMEOUT_MS = 120_000;
// With the screen locked WebKit suspends the page about 7 s after it opens: deciding there are
// no frames has to fit before that, leaving room to measure the rest.
export const HIDDEN_FRAME_LIMIT_MS = 1_500;
export const VISIBLE_FRAME_LIMIT_MS = 5_000;

export interface DiffTarget {
  showCommit: () => Promise<void>;
  showDiff: (file: FileChange) => Promise<void>;
}

export interface PerfTarget extends DiffTarget {
  // Null if `requestAnimationFrame` does not run: screen locked or window covered.
  scroller: HTMLElement | null;
  viewId: number;
  openCommit: (hash: string) => Promise<void>;
  expectedScrollHeight: number;
}

// `openMs` runs until the editors are created, with or without a frame. The paint goes apart in `paintMs`.
export type DiffMeasure =
  | (DiffTiming & { path: string; openMs: number })
  | { path: string | null; error: string };

// `openMs` is how long the command takes to create the merge window; when it is painted is not measured.
export type MergeMeasure =
  | { path: string; openMs: number }
  | { path: string | null; error: string };

// The file that the phase 3 measurement opens in the merge window.
const MERGE_PATH = "builtin/commit.c";

export async function runPerf(target: PerfTarget): Promise<void> {
  const { scroller, viewId, openCommit, expectedScrollHeight } = target;
  const readyAtEpochMs = Date.now();
  const hash = scroller ? await findTextCommit(viewId) : null;
  if (hash) await openCommit(hash);
  const deltas = scroller ? await scrollThroughLog(scroller) : [];
  const scrollHeight = scroller?.scrollHeight ?? 0;
  const diff = await measureDiff(target);
  const merge = await measureMerge();
  await perfReport(
    JSON.stringify({
      readyAtEpochMs,
      timeOriginEpochMs: performance.timeOrigin,
      windowVisible: scroller !== null,
      stages: stages(readyAtEpochMs),
      scrollHeight,
      expectedScrollHeight,
      diff,
      merge,
      ...frameStats(deltas),
    }),
  );
}

// How long the next frame takes, or null if it does not arrive: without a visible window there are no frames.
export function nextFrame(limitMs: number): Promise<number | null> {
  const start = performance.now();
  return new Promise((resolve) => {
    const limit = setTimeout(() => resolve(null), limitMs);
    requestAnimationFrame(() => {
      clearTimeout(limit);
      resolve(performance.now() - start);
    });
  });
}

const pageVisible = () => document.visibilityState === "visible" || document.hasFocus();

export const frameLimit = () => (pageVisible() ? VISIBLE_FRAME_LIMIT_MS : HIDDEN_FRAME_LIMIT_MS);

// A visible window may take a while to give its first frame if the main thread is busy. That is
// why only a hidden page decides early, and it is checked on expiry: right after being shown it still is.
export function framesRunning(): Promise<boolean> {
  return new Promise((resolve) => {
    let limit = setTimeout(() => {
      if (!pageVisible()) resolve(false);
      else limit = setTimeout(() => resolve(false), VISIBLE_FRAME_LIMIT_MS - HIDDEN_FRAME_LIMIT_MS);
    }, HIDDEN_FRAME_LIMIT_MS);
    requestAnimationFrame(() => {
      clearTimeout(limit);
      resolve(true);
    });
  });
}

// Leaves the Commit view with the first modified file in the diff window. Null only if there is
// none: a failure or a diff that is not drawn go in the report.
export async function measureDiff(target: DiffTarget): Promise<DiffMeasure | null> {
  let path: string | null = null;
  let limit: ReturnType<typeof setTimeout> | undefined;
  let stopListening = () => {};
  try {
    const file = (await repoStatus(false)).find((change) => change.state === "modified");
    if (!file) return null;
    path = file.path;
    await target.showCommit();
    const startEpochMs = Date.now();
    const timing = await new Promise<DiffTiming>((resolve, reject) => {
      limit = setTimeout(
        () => reject(new Error(`The diff was not drawn in ${DIFF_TIMEOUT_MS / 1000} s`)),
        DIFF_TIMEOUT_MS,
      );
      onDiffDrawn(resolve).then((unlisten) => {
        stopListening = unlisten;
        return target.showDiff(file);
      }).catch(reject);
    });
    return { path, openMs: timing.builtAtEpochMs - startEpochMs, ...timing };
  } catch (failure) {
    return { path, error: errorText(failure) };
  } finally {
    clearTimeout(limit);
    stopListening();
  }
}

// Opens the merge window with `builtin/commit.c` in conflict, or with the first conflicted file
// if that one is not. It is left open: the CPU and memory measurement that follows includes it. Null if the
// repository has no conflicts.
export async function measureMerge(
  open: (path: string) => Promise<void> = mergeWindowOpen,
): Promise<MergeMeasure | null> {
  let path: string | null = null;
  try {
    const conflicts = (await repoStatus(false)).filter((change) => change.state === "unmerged");
    const file = conflicts.find((change) => change.path === MERGE_PATH) ?? conflicts[0];
    if (!file) return null;
    path = file.path;
    const start = performance.now();
    await open(path);
    return { path, openMs: performance.now() - start };
  } catch (failure) {
    return { path, error: errorText(failure) };
  }
}

function stages(readyAtEpochMs: number): Record<string, number> {
  const marks = Object.fromEntries(
    performance.getEntriesByType("mark").map((mark) => [mark.name, mark.startTime]),
  );
  return { ...marks, ready: readyAtEpochMs - performance.timeOrigin };
}

async function findTextCommit(viewId: number): Promise<string | null> {
  for (const row of await logRows(viewId, 0, PAGE)) {
    const files = await commitFiles(row.hash);
    if (files.length === 0) continue;
    const sides = await fileSides(row.hash, files[0]);
    if (sides.old.kind === "text" && sides.new.kind === "text") return row.hash;
  }
  return null;
}

function scrollThroughLog(scroller: HTMLElement): Promise<number[]> {
  const distance = scroller.scrollHeight - scroller.clientHeight;
  const deltas: number[] = [];
  return new Promise((resolve) => {
    const start = performance.now();
    let previous = start;
    const step = (now: number) => {
      deltas.push(now - previous);
      previous = now;
      const progress = Math.min(1, (now - start) / SCROLL_MS);
      scroller.scrollTop = progress * distance;
      if (progress < 1) requestAnimationFrame(step);
      else resolve(deltas.slice(1));
    };
    requestAnimationFrame(step);
  });
}
