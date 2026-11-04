import { describe, expect, test } from "vitest";
import type { PullInfo, PullOption } from "../lib/api";
import {
  PULL_OPTIONS,
  branchesOf,
  chooseBranch,
  chooseRemote,
  incompatible,
  pullTitle,
  selectionProblem,
} from "./pullOptions";

const info = (overrides: Partial<PullInfo> = {}): PullInfo => ({
  branch: "main",
  remotes: ["origin", "upstream"],
  remoteBranches: ["origin/main", "origin/feature/x", "upstream/main", "upstream/dev"],
  tracked: ["origin", "main"],
  ...overrides,
});

describe("options", () => {
  test("the_options_come_out_in_intellijs_order_with_their_description", () => {
    expect(PULL_OPTIONS.map(({ id, flag }) => [id, flag])).toEqual([
      ["rebase", "--rebase"],
      ["ffOnly", "--ff-only"],
      ["noFf", "--no-ff"],
      ["squash", "--squash"],
      ["noCommit", "--no-commit"],
      ["noVerify", "--no-verify"],
    ]);
    expect(PULL_OPTIONS.map(({ description }) => description)).toEqual([
      "Rebase incoming changes on top of the current branch",
      "Merge only if it can be fast-forwarded",
      "Create a merge commit even if it can be fast-forwarded",
      "Create a single commit for all pulled changes",
      "Merge, but do not commit the result",
      "Bypass the pre-merge and commit message hooks",
    ]);
  });

  test("incompatible_options_greyed_out", () => {
    const grey = (...chosen: PullOption[]) => [...incompatible(chosen)].sort();

    expect(grey()).toEqual([]);
    expect(grey("rebase")).toEqual(["ffOnly", "noCommit", "noFf", "squash"]);
    expect(grey("ffOnly")).toEqual(["noFf", "rebase", "squash"]);
    expect(grey("noFf")).toEqual(["ffOnly", "rebase", "squash"]);
    expect(grey("squash")).toEqual(["ffOnly", "noFf", "rebase"]);
    expect(grey("noCommit")).toEqual(["rebase"]);
    expect(grey("noVerify")).toEqual([]);
    expect(grey("squash", "noCommit")).toEqual(["ffOnly", "noFf", "rebase"]);
    expect(grey("noCommit", "noVerify")).toEqual(["rebase"]);
  });

  test("the_chosen_one_is_never_greyed_out_so_it_can_be_removed", () => {
    for (const { id } of PULL_OPTIONS) {
      expect(incompatible([id]).has(id)).toBe(false);
    }
  });
});

describe("remote and branch", () => {
  test("title_pull_to_branch", () => {
    expect(pullTitle("main")).toBe("Pull to main");
    expect(pullTitle("feature/x")).toBe("Pull to feature/x");
    expect(pullTitle(null)).toBe("Pull");
    expect(pullTitle("")).toBe("Pull");
  });

  test("tracked_remote_and_branch_by_default", () => {
    const tracked = info({ tracked: ["upstream", "dev"] });

    expect(chooseRemote(tracked)).toBe("upstream");
    expect(chooseBranch(tracked, "upstream", null)).toBe("dev");
  });

  test("without_a_tracked_remote_uses_origin_or_the_first", () => {
    expect(chooseRemote(info({ tracked: null }))).toBe("origin");
    expect(chooseRemote(info({ tracked: null, remotes: ["zeta", "alpha"] }))).toBe("zeta");
    expect(chooseRemote(info({ tracked: null, remotes: [] }))).toBeNull();
  });

  test("without_a_tracked_one_uses_the_one_with_the_same_name", () => {
    const bare = info({ tracked: null, branch: "dev" });

    expect(chooseBranch(bare, "upstream", null)).toBe("dev");
    expect(chooseBranch(bare, "origin", null)).toBe("");
    expect(chooseBranch(info({ tracked: null, branch: null }), "origin", null)).toBe("");
  });

  test("what_was_already_chosen_is_kept_if_it_exists_in_the_remote", () => {
    expect(chooseBranch(info(), "origin", "feature/x")).toBe("feature/x");
    expect(chooseBranch(info(), "upstream", "feature/x")).toBe("main");
  });

  test("a_remotes_branches_go_without_its_prefix_and_the_longest_remote_wins", () => {
    expect(branchesOf(info(), "origin")).toEqual(["main", "feature/x"]);
    const odd = info({ remotes: ["a", "a/b"], remoteBranches: ["a/b/c", "a/d"] });
    expect(branchesOf(odd, "a")).toEqual(["d"]);
    expect(branchesOf(odd, "a/b")).toEqual(["c"]);
    expect(branchesOf(info(), null)).toEqual([]);
  });

  test("selection_errors", () => {
    const full = info();

    expect(selectionProblem(full, null, "main")).toEqual({
      field: "remote",
      text: "Select the remote to pull from",
    });
    expect(selectionProblem(full, "origin", "")).toEqual({
      field: "branch",
      text: "The branch to pull from should be selected.",
    });
    expect(selectionProblem(full, "origin", "dev")).toEqual({
      field: "branch",
      text: "There is no matching branch.",
    });
    expect(selectionProblem(full, "origin", "main")).toBeNull();
    expect(selectionProblem(full, "origin", "feature/x")).toBeNull();
  });
});
