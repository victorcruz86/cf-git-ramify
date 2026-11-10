import { expect, test } from "vitest";
import { isIpcFailure, smokeRepoAllowed } from "./smoke";

test("only accepts a repository inside a ramify-smoke folder", () => {
  expect(smokeRepoAllowed("/private/tmp/ramify-smoke-x1/repo")).toBe(true);
  expect(smokeRepoAllowed("/Users/victor/PROJECTS/my-repo")).toBe(false);
  expect(smokeRepoAllowed("/Users/victor/ramify-smoke")).toBe(false);
  expect(smokeRepoAllowed(null)).toBe(false);
});

test("tells an IPC failure from an operation error", () => {
  expect(isIpcFailure("Command log_foo not found")).toBe(true);
  expect(isIpcFailure("invalid args `viewId` for command `log_rows`: missing field")).toBe(true);
  expect(isIpcFailure({ kind: "failed", message: "unknown variant `x`, expected `plain`" })).toBe(true);
  expect(isIpcFailure({ kind: "failed", message: "fatal: not a git repository" })).toBe(false);
});
