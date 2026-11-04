import { expect, test, vi } from "vitest";
import { RowCache, visibleRange } from "./rowCache";

const fetchPage = async (start: number, count: number) =>
  Array.from({ length: count }, (_, offset) => start + offset);

test("range_at_the_start", () => {
  expect(visibleRange(0, 260, 26, 1000, 5)).toEqual({ start: 0, end: 15 });
});

test("range_at_the_end", () => {
  expect(visibleRange(25740, 260, 26, 1000, 5)).toEqual({ start: 985, end: 1000 });
});

test("range_without_rows", () => {
  expect(visibleRange(0, 260, 26, 0, 5)).toEqual({ start: 0, end: 0 });
});

test("does_not_repeat_requests", async () => {
  const spy = vi.fn(fetchPage);
  const cache = new RowCache(spy);
  await Promise.all([cache.ensure(0, 10), cache.ensure(5, 20)]);
  expect(spy).toHaveBeenCalledTimes(1);
  expect(cache.get(5)).toBe(5);
});

test("range_between_two_pages", async () => {
  const cache = new RowCache(fetchPage);
  await cache.ensure(190, 210);
  expect(cache.get(190)).toBe(190);
  expect(cache.get(209)).toBe(209);
});

test("short_page_counts_as_loaded", async () => {
  const cache = new RowCache(async () => [1, 2, 3]);
  await cache.ensure(0, 100);
  expect(cache.loaded(50)).toBe(true);
  expect(cache.loaded(250)).toBe(false);
});

test("limits_pages", async () => {
  const cache = new RowCache(fetchPage);
  for (let page = 0; page < 25; page++) {
    await cache.ensure(page * 200, page * 200 + 1);
  }
  expect(cache.pageCount).toBe(20);
  expect(cache.get(0)).toBeUndefined();
  expect(cache.get(24 * 200)).toBe(24 * 200);
});
