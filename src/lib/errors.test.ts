import { expect, test } from "vitest";
import type { ApiError } from "./api";
import { errorText, lostRollbackText, lostText } from "./errors";

const failure = (kind: ApiError["kind"], message: string, details?: ApiError["details"]) =>
  ({ kind, message, details }) satisfies ApiError;

test("backend_messages_are_shown_as_they_are", () => {
  const kinds: ApiError["kind"][] = [
    "git_not_found",
    "not_a_repository",
    "failed",
    "unmerged",
    "nothing_to_commit",
    "gpg_failed",
    "hook_failed",
    "restore_failed",
  ];

  for (const kind of kinds) {
    expect(errorText(failure(kind, `text of ${kind}`))).toBe(`text of ${kind}`);
  }
});

test("commit_running_does_not_only_talk_about_commits", () => {
  expect(errorText(failure("commit_running", "A commit is still running"))).toBe(
    "There is another operation in progress in this repository. Wait for it to finish and try again",
  );
});

test("partial_merge_says_the_merge_must_go_whole", () => {
  expect(errorText(failure("partial_merge", "x"))).toBe(
    "A merge must be committed whole: include all of its changes in the commit, without leaving files out or choosing lines",
  );
});

test("selection_lost_says_the_file_and_how_many_changes", () => {
  const text = errorText(
    failure("selection_lost", "x", { path: "src/a.txt", hunks: ["h1", "h2"] }),
  );

  expect(text).toContain("src/a.txt");
  expect(text).toContain("2 selected changes");
  expect(text).toContain("the commit was not made");
});

test("selection_lost_with_one_change", () => {
  expect(
    errorText(failure("selection_lost", "x", { path: "a.txt", hunks: ["h1"] })),
  ).toContain("1 selected change");
});

test("an_error_without_a_kind_is_shown_as_it_is", () => {
  expect(errorText("text")).toBe("text");
  expect(errorText(new Error("broken"))).toBe("broken");
});

test("what_was_lost_says_where_it_was_and_what_remains", () => {
  const lost = { path: "a.txt", hunks: ["h1"] };

  expect(lostText(lost, ["lines 3–4"])).toBe(
    "\"a.txt\" has changed: 1 selected change no longer exists (lines 3–4). The rest stays selected",
  );
  expect(errorText(failure("selection_lost", "x", lost), ["line 3"])).toContain("(line 3)");
  expect(lostRollbackText(lost)).toBe(
    "\"a.txt\" has changed: 1 selected change no longer exists, so nothing was rolled back",
  );
});
