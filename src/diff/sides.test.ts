import { expect, test } from "vitest";
import type { RevisionSides, WorktreeSides } from "../lib/api";
import {
  sameSides,
  sideMessage,
  sideText,
  submoduleLeftTitle,
  submoduleRightTitle,
  unchangedText,
} from "./sides";

test("no_message_for_text", () => {
  expect(sideMessage({ kind: "text", content: "hello" })).toBeNull();
});

test("no_message_for_absent", () => {
  expect(sideMessage({ kind: "absent" })).toBeNull();
});

test("message_for_binary", () => {
  expect(sideMessage({ kind: "binary" })).toBe("Binary file");
});

test("message_for_large_file", () => {
  expect(sideMessage({ kind: "tooLarge", bytes: 6291456 })).toBe(
    "File is too large to show (6.0 MB)",
  );
});

test("absent_side_is_empty", () => {
  expect(sideText({ kind: "absent" })).toBe("");
});

test("two_equal_sides_have_no_changes", () => {
  const text = { kind: "text", content: "a\n" } as const;

  expect(unchangedText({ old: text, new: text }, "the first parent")).toBe(
    "No changes compared to the first parent",
  );
  expect(unchangedText({ old: text, new: { kind: "absent" } }, "HEAD")).toBeNull();
  expect(unchangedText({ old: text, new: { kind: "text", content: "b\n" } }, "HEAD")).toBeNull();
});

test("two_revision_sides_differ_by_their_preview_and_binary_fields", () => {
  const sides: RevisionSides = {
    old: { kind: "binary" },
    new: { kind: "binary" },
    hunks: [],
    editable: false,
    note: null,
    hash: null,
    conflicted: false,
    submodule: null,
    oldPreview: false,
    newPreview: false,
    identical: true,
  };

  expect(sameSides(sides, { ...sides })).toBe(true);
  expect(sameSides(sides, { ...sides, identical: false })).toBe(false);
  expect(sameSides(sides, { ...sides, oldPreview: true })).toBe(false);
  expect(sameSides(sides, { ...sides, newPreview: true })).toBe(false);
});

test("the_diff_title_ends_with_submodule_and_the_sides_are_full_hashes", () => {
  const hash = "d".repeat(40);

  expect(submoduleLeftTitle(hash)).toBe(hash);
  expect(submoduleRightTitle(hash)).toBe(hash);
  expect(submoduleLeftTitle(null)).toBe("Base version");
  expect(submoduleLeftTitle("")).toBe("Base version");
  expect(submoduleRightTitle(null)).toBe("Your version");
  // The local side has no revision number.
  expect(submoduleRightTitle("local")).toBe("Your version");
});

test("a_submodule_changes_the_sides_that_are_compared", () => {
  const sides: WorktreeSides = {
    old: { kind: "text", content: "a" },
    new: { kind: "text", content: "a" },
    hunks: [],
    editable: false,
    note: null,
    hash: null,
    conflicted: false,
    submodule: null,
  };

  expect(sameSides(sides, { ...sides, submodule: "c".repeat(40) })).toBe(false);
});
