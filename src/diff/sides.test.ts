import { expect, test } from "vitest";
import { sideMessage, sideText, unchangedText } from "./sides";

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
