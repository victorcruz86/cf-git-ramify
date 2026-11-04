import { expect, test } from "vitest";
import { highlightMessage, validateMessage } from "./validate";

const codes = (message: string) => validateMessage(message).map((issue) => issue.code);

test("long_subject", () => {
  expect(codes("a".repeat(73))).toEqual(["subject-too-long"]);
  expect(codes("a".repeat(72))).toEqual([]);
});

test("second_line", () => {
  expect(codes("subject\ntext")).toEqual(["second-line-not-empty"]);
  expect(codes("subject\n\ntext")).toEqual([]);
});

test("empty", () => {
  expect(codes("")).toEqual(["empty"]);
  expect(codes("  \n ")).toEqual(["empty"]);
});

test("long_body", () => {
  const issues = validateMessage(`subject\n\n${"b".repeat(73)}`);
  expect(issues.map((issue) => issue.code)).toEqual(["body-too-long"]);
  expect(issues[0]).toMatchObject({ line: 2, start: 72, end: 73 });
});

test("the_subject_excess_is_marked_from_the_margin", () => {
  expect(validateMessage("a".repeat(75))[0]).toMatchObject({ line: 0, start: 72, end: 75 });
});

test("inspection_texts", () => {
  expect(validateMessage("a".repeat(73))[0].text).toBe("Subject cannot exceed 72 characters");
  expect(validateMessage("a\nb")[0].text).toBe("Missing blank line between subject and body");
  expect(validateMessage(`a\n\n${"b".repeat(73)}`)[0].text).toBe(
    "Body lines cannot exceed 72 characters",
  );
});

test("marks_only_the_inspection_segments", () => {
  const message = `${"a".repeat(74)}\nbc`;

  expect(highlightMessage(message, validateMessage(message))).toEqual([
    { text: "a".repeat(72), marked: false },
    { text: "aa", marked: true },
    { text: "\n", marked: false },
    { text: "bc", marked: true },
  ]);
});

test("without_inspections_it_is_a_single_segment", () => {
  expect(highlightMessage("hello", [])).toEqual([{ text: "hello", marked: false }]);
});
