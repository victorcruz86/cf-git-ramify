import { expect, test } from "vitest";
import { restoreSelection, snapshotSelection } from "./selection";

const HEIGHT = 24;

const hashAt = (row: number) => `h${row}`;

function snapshot(selected: number[], scrollTop: number, viewport = 240) {
  return snapshotSelection({
    selected,
    hashes: selected.map(hashAt),
    hashAt,
    scrollTop,
    viewportHeight: viewport,
    rowHeight: HEIGHT,
    count: 1000,
  });
}

test("selection_survives_the_refresh", () => {
  const saved = snapshot([12, 15], 240);

  expect(saved.hashes).toEqual(["h12", "h15"]);

  // Three commits have come in at the top: the same commit, at the same height on the screen.
  const rows = new Map([
    ["h12", 15],
    ["h15", 18],
  ]);
  const restored = restoreSelection(saved, rows, true, HEIGHT);

  expect(restored.rows).toEqual([15, 18]);
  expect(restored.scrollTop).toBe(15 * HEIGHT - (12 * HEIGHT - 240));
});

test("selection_that_no_longer_exists_is_lost_and_the_other_remains", () => {
  const saved = snapshot([12, 15], 240);

  const restored = restoreSelection(saved, new Map([["h15", 3]]), false, HEIGHT);

  expect(restored.rows).toEqual([3]);
  expect(restored.scrollTop).toBeNull();
});

test("selection_outside_the_cache_is_saved_anyway", () => {
  const saved = snapshotSelection({
    selected: [5000],
    hashes: ["far"],
    hashAt: (row) => (row < 4000 ? `h${row}` : undefined),
    scrollTop: 240,
    viewportHeight: 240,
    rowHeight: HEIGHT,
    count: 10000,
  });

  expect(saved.hashes).toEqual(["far"]);
  expect(saved.target).toEqual({ hash: "h10", gap: 0 });
});

test("without_a_visible_selection_saves_the_first_visible_row", () => {
  const saved = snapshot([500], 240);

  expect(saved.target).toEqual({ hash: "h10", gap: 0 });
});

test("at_the_very_top_stays_on_top_with_new_commits", () => {
  const saved = snapshot([2], 0);

  const restored = restoreSelection(saved, new Map([["h2", 7]]), true, HEIGHT);

  expect(restored.rows).toEqual([7]);
  expect(restored.scrollTop).toBe(0);
});

test("at_the_very_top_without_new_commits_follows_its_commit", () => {
  const saved = snapshot([2], 0);

  const restored = restoreSelection(saved, new Map([["h2", 7]]), false, HEIGHT);

  expect(restored.scrollTop).toBe(7 * HEIGHT - 2 * HEIGHT);
});

test("the_offset_does_not_go_below_zero", () => {
  const saved = snapshot([12], 216);

  const restored = restoreSelection(saved, new Map([["h12", 0]]), true, HEIGHT);

  expect(restored.scrollTop).toBe(0);
});
