import { describe, expect, test } from "vitest";
import type { MergeOption } from "../lib/api";
import {
  MERGE_OPTIONS,
  REBASE_OPTIONS,
  mergeOptionsAvailable,
  mergeOptionsCompatible,
  parseGitVersion,
  rebaseFlagText,
  rebaseOptionsCompatible,
  rememberedMergeOptions,
  type RebaseOption,
} from "./options";

const MODERN = parseGitVersion("git version 2.39.5 (Apple Git-154)");
const compatible = (...chosen: MergeOption[]) => [...mergeOptionsCompatible(chosen)].sort();
const ALL: MergeOption[] = ["ffOnly", "message", "noCommit", "noFf", "noVerify", "squash"];

describe("merge", () => {
  test("the_options_come_out_in_intellijs_order_with_their_description", () => {
    expect(MERGE_OPTIONS.map(({ id, flag }) => [id, flag])).toEqual([
      ["noFf", "--no-ff"],
      ["ffOnly", "--ff-only"],
      ["squash", "--squash"],
      ["message", "-m"],
      ["noCommit", "--no-commit"],
      ["noVerify", "--no-verify"],
    ]);
    expect(MERGE_OPTIONS.map(({ description }) => description)).toEqual([
      "Create a merge commit even if it can be fast-forwarded",
      "Merge only if it can be fast-forwarded",
      "Create a single commit for all merged changes",
      "Specify a message for the merge commit",
      "Do not commit the merge result",
      "Bypass the pre-merge and commit message hooks",
    ]);
  });

  // `GitMergeOption.isOptionSuitable`: with each one chosen, the ones that are greyed out.
  test("incompatible_options_are_disabled", () => {
    const grey = (...chosen: MergeOption[]) =>
      ALL.filter((option) => !compatible(...chosen).includes(option));

    expect(grey()).toEqual([]);
    expect(grey("noFf")).toEqual(["ffOnly", "squash"]);
    expect(grey("ffOnly")).toEqual(["message", "noFf", "squash"]);
    expect(grey("squash")).toEqual(["ffOnly", "message", "noFf"]);
    expect(grey("message")).toEqual(["ffOnly", "noCommit", "squash"]);
    expect(grey("noCommit")).toEqual(["message"]);
    expect(grey("noVerify")).toEqual([]);
    expect(grey("noFf", "noVerify")).toEqual(["ffOnly", "squash"]);
    expect(grey("message", "noFf")).toEqual(["ffOnly", "noCommit", "squash"]);
  });

  test("what_is_chosen_is_never_greyed_out_so_it_can_be_removed", () => {
    for (const { id } of MERGE_OPTIONS) {
      expect(mergeOptionsCompatible([id]).has(id)).toBe(true);
    }
  });

  test("no_verify_only_with_git_2_24", () => {
    const ids = (text: string) =>
      mergeOptionsAvailable(parseGitVersion(text)).map(({ id }) => id);

    expect(ids("2.23.9")).not.toContain("noVerify");
    expect(ids("git version 2.24.0")).toContain("noVerify");
    expect(ids("2.39.5 (Apple Git-154)")).toContain("noVerify");
    expect(ids("3.0.0")).toContain("noVerify");
    // Without knowing the version, an option that git might not know is not risked.
    expect(mergeOptionsAvailable(null).map(({ id }) => id)).not.toContain("noVerify");
    expect(ids("2.24.0")).toHaveLength(6);
    expect(ids("2.23.0")).toHaveLength(5);
  });

  test("what_is_remembered_loses_no_verify_if_git_does_not_know_it_and_comes_out_in_enum_order", () => {
    const saved: MergeOption[] = ["noVerify", "message", "noFf"];

    expect(rememberedMergeOptions(saved, MODERN)).toEqual(["noFf", "message", "noVerify"]);
    expect(rememberedMergeOptions(saved, parseGitVersion("2.20.1"))).toEqual(["noFf", "message"]);
    // What is incompatible stays and is seen greyed out, like when loading the settings in IntelliJ.
    expect(rememberedMergeOptions(["ffOnly", "noFf"], MODERN)).toEqual(["noFf", "ffOnly"]);
  });
});

describe("git version", () => {
  test("reads_the_version_with_or_without_the_prefix", () => {
    expect(parseGitVersion("git version 2.39.5 (Apple Git-154)")).toEqual([2, 39, 5]);
    expect(parseGitVersion("2.24")).toEqual([2, 24, 0]);
    expect(parseGitVersion("odd")).toBeNull();
    expect(parseGitVersion("")).toBeNull();
  });
});

describe("rebase", () => {
  const OLD = parseGitVersion("2.21.0");
  const grey = (selected: RebaseOption[], version = MODERN) =>
    REBASE_OPTIONS.map(({ id }) => id).filter(
      (id) => !rebaseOptionsCompatible(selected, version).has(id),
    );

  test("the_options_come_out_in_intellijs_order_with_their_description", () => {
    expect(REBASE_OPTIONS.map(({ id }) => id)).toEqual([
      "switchBranch",
      "onto",
      "rebaseMerges",
      "keepEmpty",
      "root",
      "interactive",
    ]);
    expect(REBASE_OPTIONS.map(({ description }) => description)).toEqual([
      "Select another branch to rebase",
      "Specify a new base for the rebased commits",
      "Recreate commits topology",
      "Do not remove empty commits during rebase",
      "Rebase all commits in the branch",
      "Edit commits before rebasing",
    ]);
    expect(REBASE_OPTIONS.filter(({ separatorAbove }) => separatorAbove).map(({ id }) => id)).toEqual(
      ["interactive"],
    );
  });

  test("the_flag_text_changes_with_the_git_version", () => {
    expect(rebaseFlagText("rebaseMerges", MODERN)).toBe("--rebase-merges");
    expect(rebaseFlagText("rebaseMerges", OLD)).toBe("--preserve-merges");
    expect(rebaseFlagText("rebaseMerges", null)).toBe("--preserve-merges");
    expect(rebaseFlagText("onto", MODERN)).toBe("--onto");
    expect(rebaseFlagText("switchBranch", MODERN)).toBe("");
    expect(rebaseFlagText("interactive", OLD)).toBe("--interactive");
  });

  test("rebase_merges_and_interactive_exclude_each_other_only_with_git_older_than_2_22", () => {
    expect(grey(["rebaseMerges", "interactive"])).toEqual([]);
    expect(grey(["interactive"], OLD)).toEqual(["rebaseMerges"]);
    expect(grey(["rebaseMerges"], OLD)).toEqual(["interactive"]);
    expect(grey(["keepEmpty", "root", "onto"], OLD)).toEqual([]);
    expect(grey([], OLD)).toEqual([]);
  });
});
