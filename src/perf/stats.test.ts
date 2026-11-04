import { expect, test } from "vitest";
import { frameStats } from "./stats";

test("computes_percentiles", () => {
  const deltas = Array.from({ length: 100 }, (_, index) => index + 1);
  expect(frameStats(deltas.reverse())).toEqual({
    frames: 100,
    p50: 50,
    p95: 95,
    p99: 99,
    over50ms: 50,
  });
});

test("without_frames", () => {
  expect(frameStats([])).toEqual({ frames: 0, p50: 0, p95: 0, p99: 0, over50ms: 0 });
});
