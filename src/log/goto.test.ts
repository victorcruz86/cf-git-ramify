import { expect, test } from "vitest";
import type { LogRef, LogRow } from "../lib/api";
import { adjacentLabel, arrowTooltip, jumpFailure, resolveTarget } from "./goto";

const refs: LogRef[] = [
  { name: "HEAD", kind: "head" },
  { name: "main", kind: "local" },
  { name: "feature/one", kind: "local" },
  { name: "feature/two", kind: "local" },
  { name: "origin/main", kind: "remote" },
  { name: "abcd", kind: "tag" },
];

function row(overrides: Partial<LogRow> = {}): LogRow {
  return {
    hash: "1234567890".repeat(4),
    subject: "one",
    author: "Ana",
    time: new Date(2026, 9, 30, 14, 11).getTime() / 1000,
    commitTime: 0,
    samePerson: true,
    parentCount: 1,
    refs: [],
    labels: { compact: [], normal: [] },
    highlight: { bold: false, muted: false, branch: false },
    print: { nodeX: 0, nodeColor: 0, width: 2, edges: [], arrows: [] },
    ...overrides,
  };
}

const date = new Intl.DateTimeFormat(undefined, { dateStyle: "short" }).format(
  new Date(2026, 9, 30, 14, 11),
);
const time = new Intl.DateTimeFormat(undefined, { timeStyle: "short" }).format(
  new Date(2026, 9, 30, 14, 11),
);

test("go_to_prefers_the_first_ref_that_starts_the_same", () => {
  expect(resolveTarget("feature", refs)).toEqual({ kind: "ref", value: "feature/one" });
  expect(resolveTarget("feature/t", refs)).toEqual({ kind: "ref", value: "feature/two" });
  // A ref beats a hash that is spelled the same, and the comparison is case-sensitive.
  expect(resolveTarget("abcd", refs)).toEqual({ kind: "ref", value: "abcd" });
  expect(resolveTarget("ABCD", refs)).toEqual({ kind: "hash", value: "ABCD" });
  expect(resolveTarget("Main", refs).kind).toBe("invalid");
});

test("go_to_hash_of_four_or_more_hexadecimals", () => {
  expect(resolveTarget("abc", [])).toEqual({ kind: "invalid", value: "abc" });
  expect(resolveTarget("abc1", [])).toEqual({ kind: "hash", value: "abc1" });
  expect(resolveTarget("a".repeat(40), []).kind).toBe("hash");
  expect(resolveTarget("a".repeat(41), []).kind).toBe("invalid");
  expect(resolveTarget("abcg", [])).toEqual({ kind: "invalid", value: "abcg" });
  // What git log leaves around a hash, when copying it.
  expect(resolveTarget("(abc1)", [])).toEqual({ kind: "hash", value: "abc1" });
  expect(resolveTarget("'abc1'", [])).toEqual({ kind: "hash", value: "abc1" });
  expect(resolveTarget("`abc1\"", [])).toEqual({ kind: "hash", value: "abc1" });
});

test("go_to_error_messages", () => {
  const missing = { row: null, hash: null };
  const hidden = { row: null, hash: "1234567890".repeat(4) };

  expect(jumpFailure({ kind: "invalid", value: "x" }, missing)).toBe(
    "'x' does not look like a commit hash",
  );
  expect(jumpFailure({ kind: "hash", value: "x" }, missing)).toBe(
    "Commit or reference 'x' not found",
  );
  expect(jumpFailure({ kind: "hash", value: "abcdef0123456789" }, missing)).toBe(
    "Commit or reference 'abcdef01' not found",
  );
  expect(jumpFailure({ kind: "hash", value: "abcdef0123456789" }, hidden)).toBe(
    "Commit or reference 'abcdef01' doesn't match the filters",
  );
  expect(jumpFailure({ kind: "ref", value: "main" }, hidden)).toBe(
    "Commit '12345678' doesn't match the filters",
  );
});

test("parents_popup_text", () => {
  expect(adjacentLabel(row({ subject: "one" }))).toBe(
    `12345678 "one" by Ana on ${date} at ${time}`,
  );
  const long = adjacentLabel(row({ subject: "x".repeat(41), samePerson: false }));
  expect(long).toBe(`12345678 "${"x".repeat(37)}..." by Ana* on ${date} at ${time}`);
  expect(adjacentLabel(row({ subject: "x".repeat(40) }))).toContain(`"${"x".repeat(40)}"`);
});

test("arrow_tooltip", () => {
  expect(arrowTooltip(row({ subject: "x".repeat(51) }))).toEqual({
    subject: `"${"x".repeat(47)}..."`,
    rest: ` by Ana on ${date} at ${time}`,
  });
  expect(arrowTooltip(row({ subject: "x".repeat(50) })).subject).toBe(`"${"x".repeat(50)}"`);
});
