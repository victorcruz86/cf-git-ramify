import { expect, test } from "vitest";
import { deleteProblems, deleteQuestion } from "./deleteFiles";

test("the_question_names_one_file_and_counts_several", () => {
  expect(deleteQuestion(["docs/asistente-mms-spike.md"])).toBe("Delete 'asistente-mms-spike.md'?");
  expect(deleteQuestion(["a.ts", "b/c.ts", "d.ts"])).toBe("Are you sure you want to delete 3 selected files?");
});

test("the_problems_show_at_most_ten_names", () => {
  expect(deleteProblems(["a.md", "b.md"])).toBe("Could not erase files or folders:\na.md,\n  b.md");
  const many = Array.from({ length: 12 }, (_, index) => `f${index}.md`);
  expect(deleteProblems(many)).toBe(
    `Could not erase files or folders:\n${many.slice(0, 10).join(",\n  ")}\n  ...`,
  );
});
