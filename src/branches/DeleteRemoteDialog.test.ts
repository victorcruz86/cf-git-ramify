import { cleanup, render, screen } from "@testing-library/svelte";
import { afterEach, expect, test } from "vitest";
import DeleteRemoteDialog from "./DeleteRemoteDialog.svelte";

afterEach(cleanup);

// `GitBranchUiHandlerImpl.confirmRemoteBranchDeletion` (`B:1253-1257`): the plural title, question
// and button with several branches, and the trackers of all of them.
test("several_remote_branches_get_one_plural_question", () => {
  render(DeleteRemoteDialog, {
    branches: ["origin/a", "origin/b"],
    tracking: ["a", "b"],
    onanswer: () => {},
  });

  expect(screen.getByRole("dialog", { name: "Delete Remote Branches" })).toBeTruthy();
  expect(screen.getByText(/Delete remote branches/).textContent?.replace(/\s+/g, " ").trim()).toBe(
    "Delete remote branches origin/a, origin/b?",
  );
  expect(screen.getByText(/Delete tracking local branches: a, b/)).toBeTruthy();
  expect(screen.getByRole("button", { name: "Delete All" })).toBeTruthy();
});

test("one_remote_branch_keeps_the_singular_texts", () => {
  render(DeleteRemoteDialog, { branches: ["origin/a"], tracking: [], onanswer: () => {} });

  expect(screen.getByRole("dialog", { name: "Delete Remote Branch" })).toBeTruthy();
  expect(screen.getByText(/Delete remote branch/).textContent?.replace(/\s+/g, " ").trim()).toBe(
    "Delete remote branch origin/a?",
  );
  expect(screen.getByRole("button", { name: "Delete" })).toBeTruthy();
});
