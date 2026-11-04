import { expect, test } from "vitest";
import {
  branchProblem,
  cleanOnTyping,
  cleanName,
  isValidRefName,
  overwriteEnabled,
  type BranchNames,
} from "./refName";

const NAMES: BranchNames = {
  local: ["main", "feature/one"],
  remote: ["origin/main"],
};

test("name_is_cleaned_while_typing", () => {
  expect(cleanOnTyping("my branch")).toBe("my-branch");
  expect(cleanOnTyping("my   branch")).toBe("my-branch");
  expect(cleanOnTyping("a\tb")).toBe("a-b");
  expect(cleanOnTyping("a~b^c:d?e*f[g\\h\"i")).toBe("abcdefghi");
  expect(cleanOnTyping("-branch")).toBe("branch");
  expect(cleanOnTyping(".branch")).toBe("branch");
  expect(cleanOnTyping("/branch")).toBe("branch");
  expect(cleanOnTyping("a//b")).toBe("a/b");
  expect(cleanOnTyping("a..b")).toBe("a.b");
  expect(cleanOnTyping("a@{b")).toBe("ab");
  expect(cleanOnTyping("a./b/.c")).toBe("a/b/c");
  // While typing, the ending is kept: more may come.
  expect(cleanOnTyping("branch.")).toBe("branch.");
});

test("on_accept_the_ending_is_removed_too", () => {
  expect(cleanName("branch.")).toBe("branch");
  expect(cleanName("branch/")).toBe("branch");
  expect(cleanName("branch.lock")).toBe("branch");
  expect(cleanName(" branch ")).toBe("-branch-");
});

test("name_valid_like_git_ref_name_validator", () => {
  expect(isValidRefName("feature/one")).toBe(true);
  for (const name of ["", "  ", "-a", ".a", "/a", "a b", "a..b", "a.lock", "a/", "a~b", "a@{b"]) {
    expect(isValidRefName(name)).toBe(false);
  }
});

const onApply = (text: string, overwrite = false, showReset = true) =>
  branchProblem(text, NAMES, { onApply: true, overwrite, showReset });

test("branch_dialog_errors", () => {
  expect(onApply("")).toBe("Specify name for the new branch");
  expect(onApply("   ")).toBe("Specify name for the new branch");
  expect(onApply("HEAD")).toBe("HEAD is a reserved keyword");
  expect(onApply("head")).toBe("HEAD is a reserved keyword");
  expect(onApply("main")).toBe(
    "Branch name main already exists.\nChange the name or overwrite existing branch",
  );
  expect(onApply("main", false, false)).toBe("Branch name main already exists");
  expect(onApply("origin/main")).toBe(
    "Branch name origin/main conflicts with remote branch with the same name",
  );
  expect(onApply("feature")).toBe(
    "Branch name feature conflicts with local branch directory with the same name",
  );
  expect(onApply("new")).toBeNull();
});

test("empty_is_only_an_error_on_accept", () => {
  expect(branchProblem("", NAMES, { onApply: false, overwrite: false, showReset: true })).toBeNull();
});

test("overwrite_only_with_conflict", () => {
  expect(overwriteEnabled("main", NAMES)).toBe(true);
  expect(overwriteEnabled("new", NAMES)).toBe(false);
  expect(overwriteEnabled("origin/main", NAMES)).toBe(false);
  expect(onApply("main", true)).toBeNull();
});
