import { expect, test } from "vitest";
import { pathsSelection, type LogChange } from "./changes";

const change = (path: string, hash: string, base: string | null, oldPath: string | null = null) =>
  ({ file: { path, oldPath, status: "M" }, hash, base }) satisfies LogChange;

test("files_are_grouped_by_commit_and_base", () => {
  const selection = pathsSelection([
    change("a.txt", "h1", "p1"),
    change("b.txt", "h2", "p2"),
    change("c.txt", "h1", "p1"),
    change("r.txt", "h3", null, "old.txt"),
  ]);

  expect(selection).toEqual([
    { hash: "h1", base: "p1", paths: ["a.txt", "c.txt"] },
    { hash: "h2", base: "p2", paths: ["b.txt"] },
    { hash: "h3", base: null, paths: ["r.txt", "old.txt"] },
  ]);
});

test("a_file_seen_under_two_parents_enters_once", () => {
  const selection = pathsSelection([change("a.txt", "m", "p1"), change("a.txt", "m", "p2")]);

  expect(selection).toEqual([{ hash: "m", base: "p1", paths: ["a.txt"] }]);
});
