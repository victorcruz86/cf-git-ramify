import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import {
  framesRunning,
  measureDiff,
  measureMerge,
  nextFrame,
  runPerf,
  VISIBLE_FRAME_LIMIT_MS,
} from "./run";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ emit: vi.fn(), listen: vi.fn() }));

const change = (path: string, state: string) => ({
  path,
  oldPath: null,
  state,
  staged: false,
  unstaged: true,
});

const TIMING = { sidesMs: 40, buildMs: 90, paintMs: 16, builtAtEpochMs: 0 };

let drawn: ((event: { payload: unknown }) => void) | undefined;

afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

beforeEach(() => {
  drawn = undefined;
  vi.mocked(listen).mockReset();
  vi.mocked(listen).mockImplementation(async (name, handler) => {
    if (name === "diff-drawn") drawn = handler as typeof drawn;
    return () => {};
  });
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockResolvedValue([change(".idea/a", "untracked"), change("a.c", "modified")]);
});

test("opens_the_commit_view_and_the_diff_of_the_first_modified", async () => {
  const steps: string[] = [];
  const result = await measureDiff({
    showCommit: async () => void steps.push("commit"),
    showDiff: async (file) => {
      steps.push(file.path);
      drawn?.({ payload: { ...TIMING, builtAtEpochMs: Date.now() } });
    },
  });

  expect(steps).toEqual(["commit", "a.c"]);
  expect(result).toMatchObject({ path: "a.c", sidesMs: 40, buildMs: 90, paintMs: 16 });
  expect(result).toHaveProperty("openMs", expect.any(Number));
});

test("the_open_time_ends_when_the_editors_are_created_whether_there_is_a_frame_or_not", async () => {
  vi.useFakeTimers();
  for (const paintMs of [16, null]) {
    const start = Date.now();
    const result = await measureDiff({
      showCommit: async () => {},
      showDiff: async () => {
        vi.advanceTimersByTime(5_300);
        drawn?.({ payload: { ...TIMING, paintMs, builtAtEpochMs: start + 300 } });
      },
    });

    expect(result).toMatchObject({ openMs: 300, paintMs });
  }
});

test("a_failure_opening_the_diff_is_reported_right_away_and_releases_the_listener", async () => {
  const unlisten = vi.fn();
  vi.mocked(listen).mockImplementation(async () => unlisten);

  const result = await measureDiff({
    showCommit: async () => {},
    showDiff: async () => {
      throw { kind: "failed", message: "could not be opened" };
    },
  });

  expect(result).toEqual({ path: "a.c", error: "could not be opened" });
  expect(unlisten).toHaveBeenCalled();
});

test("without_modified_files_there_is_no_diff", async () => {
  vi.mocked(invoke).mockResolvedValue([change("new", "untracked")]);
  const showDiff = vi.fn();

  const result = await measureDiff({ showCommit: async () => {}, showDiff });

  expect(result).toBeNull();
  expect(showDiff).not.toHaveBeenCalled();
});

test("if_the_diff_is_not_drawn_it_says_so_and_releases_the_listener", async () => {
  vi.useFakeTimers();
  const unlisten = vi.fn();
  vi.mocked(listen).mockImplementation(async () => unlisten);

  const pending = measureDiff({ showCommit: async () => {}, showDiff: async () => {} });
  await vi.advanceTimersByTimeAsync(200_000);

  await expect(pending).resolves.toEqual({
    path: "a.c",
    error: expect.stringContaining("was not drawn"),
  });
  expect(unlisten).toHaveBeenCalled();
});

function page(visibilityState: string, focused: boolean) {
  vi.spyOn(document, "visibilityState", "get").mockReturnValue(visibilityState as DocumentVisibilityState);
  vi.spyOn(document, "hasFocus").mockReturnValue(focused);
}

async function resolvedAt(result: Promise<boolean>, ms: number) {
  let done: boolean | null = null;
  result.then((value) => (done = value));
  await vi.advanceTimersByTimeAsync(ms);
  return done;
}

test("a_hidden_page_decides_early_that_there_are_no_frames", async () => {
  vi.useFakeTimers();
  vi.stubGlobal("requestAnimationFrame", () => 0);
  page("hidden", false);

  expect(await resolvedAt(framesRunning(), 1_500)).toBe(false);
});

test.each([
  ["visible", false],
  ["hidden", true],
])("a_page_that_claims_to_be_visible_waits_for_the_frame: %s %s", async (state, focused) => {
  vi.useFakeTimers();
  vi.stubGlobal("requestAnimationFrame", () => 0);
  page(state, focused);

  const running = framesRunning();
  expect(await resolvedAt(running, VISIBLE_FRAME_LIMIT_MS - 1)).toBeNull();
  await vi.advanceTimersByTimeAsync(1);
  await expect(running).resolves.toBe(false);
});

test("a_late_first_frame_with_the_page_visible_does_not_lose_the_measurement", async () => {
  vi.useFakeTimers();
  vi.stubGlobal("requestAnimationFrame", (run: FrameRequestCallback) => {
    setTimeout(() => run(0), 4_500);
    return 0;
  });
  page("visible", false);

  expect(await resolvedAt(framesRunning(), 4_500)).toBe(true);
});

test("a_page_that_becomes_visible_when_shown_also_waits", async () => {
  vi.useFakeTimers();
  vi.stubGlobal("requestAnimationFrame", () => 0);
  page("hidden", false);
  const running = framesRunning();
  page("visible", false);

  expect(await resolvedAt(running, 1_500)).toBeNull();
  await vi.advanceTimersByTimeAsync(VISIBLE_FRAME_LIMIT_MS);
  await expect(running).resolves.toBe(false);
});

test("next_frame_gives_null_if_no_frame_arrives", async () => {
  vi.useFakeTimers();
  vi.stubGlobal("requestAnimationFrame", () => 0);

  const frame = nextFrame(2_000);
  await vi.advanceTimersByTimeAsync(2_000);

  await expect(frame).resolves.toBeNull();
});

test("next_frame_gives_how_long_it_took", async () => {
  vi.stubGlobal("requestAnimationFrame", (run: FrameRequestCallback) => {
    run(0);
    return 0;
  });

  expect(await nextFrame(2_000)).toBeGreaterThanOrEqual(0);
});

test("without_a_visible_window_it_reports_without_scroll_and_says_so", async () => {
  const report = vi.fn();
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "log_rows") return [];
    if (command === "perf_report") report(JSON.parse((args as { json: string }).json));
    return [];
  });

  await runPerf({
    scroller: null,
    viewId: 1,
    openCommit: async () => {},
    expectedScrollHeight: 0,
    showCommit: async () => {},
    showDiff: async () => {},
  });

  expect(report).toHaveBeenCalledWith(
    expect.objectContaining({ windowVisible: false, frames: 0, diff: null }),
  );
});

test("opens_the_merge_window_with_the_commit_c_conflict", async () => {
  vi.mocked(invoke).mockResolvedValue([
    change("a.c", "unmerged"),
    change("builtin/commit.c", "unmerged"),
    change("b.c", "modified"),
  ]);
  const open = vi.fn(async () => {});

  const result = await measureMerge(open);

  expect(open).toHaveBeenCalledWith("builtin/commit.c");
  expect(result).toMatchObject({ path: "builtin/commit.c", openMs: expect.any(Number) });
});

test("without_commit_c_in_conflict_opens_the_first_conflicted_file", async () => {
  vi.mocked(invoke).mockResolvedValue([change("b.c", "modified"), change("a.c", "unmerged")]);
  const open = vi.fn(async () => {});

  await measureMerge(open);

  expect(open).toHaveBeenCalledWith("a.c");
});

test("without_conflicts_there_is_no_merge_window", async () => {
  const open = vi.fn();

  expect(await measureMerge(open)).toBeNull();
  expect(open).not.toHaveBeenCalled();
});

test("a_failure_opening_the_merge_window_is_reported", async () => {
  vi.mocked(invoke).mockResolvedValue([change("a.c", "unmerged")]);

  const result = await measureMerge(async () => {
    throw { kind: "failed", message: "could not be opened" };
  });

  expect(result).toEqual({ path: "a.c", error: "could not be opened" });
});

test("the_report_carries_the_merge_measurement", async () => {
  const report = vi.fn();
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "perf_report") report(JSON.parse((args as { json: string }).json));
    if (command === "repo_status") return [change("a.c", "unmerged")];
    return [];
  });

  await runPerf({
    scroller: null,
    viewId: 1,
    openCommit: async () => {},
    expectedScrollHeight: 0,
    showCommit: async () => {},
    showDiff: async () => {},
  });

  expect(report).toHaveBeenCalledWith(
    expect.objectContaining({ merge: expect.objectContaining({ path: "a.c" }) }),
  );
});
