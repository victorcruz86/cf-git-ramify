import { afterEach, expect, test, vi } from "vitest";
import { emptyFilters } from "./filters";
import { LOG_TAB_KINDS, onLogTabRequested, openLogTab, type LogTabInit } from "./tabKinds";

vi.mock("../lib/tabs", () => ({ requestTab: vi.fn() }));

let stop: (() => void) | null = null;

afterEach(() => {
  stop?.();
  stop = null;
});

test("show_in_git_log_opens_a_range_tab_from_the_heads_remote", async () => {
  // `Show '…' in Git Log`: a log tab named after its range, not remembered, and found again by it.
  expect(LOG_TAB_KINDS.range).toEqual({ title: expect.any(Function), persisted: false, reused: true });
  expect(LOG_TAB_KINDS.range.title("forker/main..forker/feature")).toBe("Log: forker/main..forker/feature");

  const asked: { kind: string; key: string; init: LogTabInit }[] = [];
  stop = onLogTabRequested(async (kind, key, init) => {
    asked.push({ kind, key, init: init() });
  });
  const range: [string, string] = ["forker/main", "forker/feature"];
  await openLogTab("range", "forker/main..forker/feature", () => ({
    name: "forker/main..forker/feature",
    filters: { ...emptyFilters(), ranges: [range] },
  }));

  expect(asked).toEqual([
    {
      kind: "range",
      key: "forker/main..forker/feature",
      init: { name: "forker/main..forker/feature", filters: { ...emptyFilters(), ranges: [range] } },
    },
  ]);
});
