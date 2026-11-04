import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { get } from "svelte/store";
import type {
  BranchList,
  MergeOption,
  MergeOutcome,
  RebaseFlag,
  RebaseOutcome,
  RebaseRun,
  RepoState,
} from "../lib/api";
import { dismiss, notifications, type Shown } from "../lib/notifications";
import { gitMenuEnabled } from "../operation/operation";
import {
  chooseMergeBranch,
  gitFlow,
  rebasedMessage,
  type GitApi,
  type GitRequest,
  type GitUi,
  type MergeChoice,
  type RebaseChoice,
} from "./flow";
import { parseGitVersion, type GitVersion } from "./options";
import MergeDialog from "./MergeDialog.svelte";
import RebaseDialog from "./RebaseDialog.svelte";
import FilesDialog from "./FilesDialog.svelte";

const MODERN = parseGitVersion("2.39.5") as GitVersion;
const HEAD = "a1b2c3d4".repeat(5);

beforeEach(() => get(notifications).forEach(({ id }) => dismiss(id)));
afterEach(cleanup);

describe("MergeDialog", () => {
  function open(
    props: {
      current?: string | null;
      branches?: string[];
      branch?: string;
      options?: MergeOption[];
      version?: GitVersion | null;
      unmerged?: Promise<string[] | null>;
    } = {},
  ) {
    const onanswer = vi.fn();
    render(MergeDialog, {
      current: "main",
      branches: ["main", "side", "done", "origin/side"],
      branch: "side",
      options: [],
      version: MODERN,
      unmerged: Promise.resolve(["side", "origin/side"]),
      onanswer,
      ...props,
    });
    return { onanswer };
  }

  const branch = () => screen.getByRole("combobox", { name: "Branch" }) as HTMLInputElement;
  const merge = () => screen.getByRole("button", { name: "Merge" });
  const modify = () => screen.getByRole("button", { name: "Modify options" });
  const option = (flag: string) => screen.getByRole("checkbox", { name: flag }) as HTMLInputElement;

  test("title_merge_into_branch_and_merge_button", () => {
    open();
    expect(screen.getByRole("dialog", { name: "Merge into main" })).toBeTruthy();
    expect(merge().className).toContain("default");
    expect(screen.getByText("git merge")).toBeTruthy();
    expect(branch().value).toBe("side");
    expect(branch().placeholder).toBe("Specify branch");
  });

  test("without_current_branch_the_title_is_merge_changes", () => {
    open({ current: null });
    expect(screen.getByRole("dialog", { name: "Merge Changes" })).toBeTruthy();
  });

  test("empty_branch_gives_the_merge_branch_should_be_selected", async () => {
    const { onanswer } = open({ branch: "" });
    // Errors do not appear before the first attempt.
    expect(screen.queryByRole("alert")).toBeNull();

    await fireEvent.click(merge());

    expect((await screen.findByRole("alert")).textContent).toBe(
      "The merge branch should be selected.",
    );
    expect(onanswer).not.toHaveBeenCalled();
  });

  test("branch_that_does_not_exist_gives_there_is_no_matching_branch", async () => {
    const { onanswer } = open();
    await fireEvent.input(branch(), { target: { value: "made-up" } });

    await fireEvent.click(merge());

    expect((await screen.findByRole("alert")).textContent).toBe("There is no matching branch.");
    expect(onanswer).not.toHaveBeenCalled();
  });

  test("already_merged_branch_gives_already_merged", async () => {
    const { onanswer } = open({ branch: "done" });
    // Wait for the unmerged branches to arrive, which are loaded separately.
    await new Promise((resolve) => setTimeout(resolve, 0));

    await fireEvent.click(merge());

    expect((await screen.findByRole("alert")).textContent).toBe("Branch 'done' is already merged");
    expect(onanswer).not.toHaveBeenCalled();

    await fireEvent.input(branch(), { target: { value: "side" } });
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
    await fireEvent.click(merge());
    expect(onanswer).toHaveBeenCalledWith({ branch: "side", options: [], message: "" });
  });

  test("if_it_could_not_be_known_which_are_unmerged_there_is_no_warning", async () => {
    const { onanswer } = open({ branch: "done", unmerged: Promise.resolve(null) });
    await new Promise((resolve) => setTimeout(resolve, 0));

    await fireEvent.click(merge());

    expect(onanswer).toHaveBeenCalledWith({ branch: "done", options: [], message: "" });
  });

  test("message_visible_only_with_m", async () => {
    const { onanswer } = open();
    expect(screen.queryByLabelText("Commit message:")).toBeNull();

    await fireEvent.click(modify());
    expect(screen.getByText("Add Merge Options")).toBeTruthy();
    await fireEvent.click(option("-m"));
    const message = screen.getByLabelText("Commit message:") as HTMLTextAreaElement;
    await fireEvent.input(message, { target: { value: "my merge" } });
    // Removing `-m` hides the text and clears it, like `updateCommitMessagePanel`.
    await fireEvent.click(screen.getByRole("button", { name: "Remove -m" }));
    expect(screen.queryByLabelText("Commit message:")).toBeNull();
    await fireEvent.click(option("-m"));
    expect((screen.getByLabelText("Commit message:") as HTMLTextAreaElement).value).toBe("");

    await fireEvent.input(screen.getByLabelText("Commit message:"), {
      target: { value: "another" },
    });
    await fireEvent.click(merge());

    expect(onanswer).toHaveBeenCalledWith({ branch: "side", options: ["message"], message: "another" });
  });

  test("merge_incompatible_ones_are_seen_greyed_out", async () => {
    open();
    await fireEvent.click(modify());
    for (const flag of ["--no-ff", "--ff-only", "--squash", "-m", "--no-commit", "--no-verify"]) {
      expect(option(flag).disabled).toBe(false);
    }
    expect(screen.getByText("Do not commit the merge result")).toBeTruthy();

    await fireEvent.click(option("--squash"));

    expect(option("--squash").checked).toBe(true);
    expect(option("--squash").disabled).toBe(false);
    expect(option("--no-ff").disabled).toBe(true);
    expect(option("--ff-only").disabled).toBe(true);
    expect(option("-m").disabled).toBe(true);
    expect(option("--no-commit").disabled).toBe(false);
    expect(option("--no-verify").disabled).toBe(false);

    await fireEvent.click(option("--squash"));
    expect(option("--no-ff").disabled).toBe(false);
  });

  test("no_verify_only_with_git_2_24", async () => {
    open({ version: parseGitVersion("2.23.0") });
    await fireEvent.click(modify());

    expect(screen.queryByRole("checkbox", { name: "--no-verify" })).toBeNull();
    expect(screen.getByRole("checkbox", { name: "--no-ff" })).toBeTruthy();
  });

  test("remembered_options_come_out_as_buttons_and_travel_in_the_answer", async () => {
    const { onanswer } = open({ options: ["noFf", "noVerify"] });
    expect(screen.getByText("--no-ff")).toBeTruthy();

    await fireEvent.click(screen.getByRole("button", { name: "Remove --no-verify" }));
    await fireEvent.click(modify());
    await fireEvent.click(option("--no-commit"));
    await fireEvent.click(merge());

    expect(onanswer).toHaveBeenCalledWith({
      branch: "side",
      options: ["noFf", "noCommit"],
      message: "",
    });
  });

  test("cancelling_answers_nothing", async () => {
    const { onanswer } = open();

    await fireEvent.click(screen.getByRole("button", { name: "Cancel" }));

    expect(onanswer).toHaveBeenCalledWith(null);
  });

  test("the_default_branch_is_the_remembered_one_and_otherwise_the_upstreams_or_the_first", () => {
    const names = ["main", "side", "origin/side"];
    expect(chooseMergeBranch("side", names, null)).toBe("side");
    // `nameForRemoteOperations` of the branch followed by the current one: here, the local one with the same name.
    expect(chooseMergeBranch("deleted", names, "origin/side")).toBe("side");
    expect(chooseMergeBranch(null, names, "origin/other")).toBe("main");
    expect(chooseMergeBranch(null, ["side", "origin/x"], "origin/side")).toBe("side");
    expect(chooseMergeBranch(null, [], null)).toBe("");
  });
});

describe("RebaseDialog", () => {
  function open(
    props: {
      branches?: string[];
      refs?: string[];
      upstream?: string;
      options?: RebaseFlag[];
      version?: GitVersion | null;
      validate?: (revisions: string[]) => Promise<boolean[]>;
    } = {},
  ) {
    const onanswer = vi.fn();
    const validate = vi.fn(async (revisions: string[]) => revisions.map(() => true));
    render(RebaseDialog, {
      branches: ["main", "side"],
      refs: ["main", "side", "origin/main", "v1"],
      upstream: "",
      options: [],
      version: MODERN,
      validate,
      onanswer,
      ...props,
    });
    return { onanswer, validate: props.validate ?? validate };
  }

  const upstream = () => screen.getByRole("combobox", { name: "Upstream" }) as HTMLInputElement;
  const onto = () => screen.queryByRole("combobox", { name: "New base" }) as HTMLInputElement | null;
  const source = () =>
    screen.queryByRole("combobox", { name: "Source branch" }) as HTMLInputElement | null;
  const rebase = () => screen.getByRole("button", { name: "Rebase" }) as HTMLButtonElement;
  const modify = () => screen.getByRole("button", { name: "Modify options" });
  const option = (name: string) => screen.getByRole("checkbox", { name }) as HTMLInputElement;
  const type = (field: HTMLElement, value: string) => fireEvent.input(field, { target: { value } });
  const params = (overrides: object = {}) => ({
    upstream: null,
    onto: null,
    branch: null,
    interactive: false,
    rebaseMerges: false,
    keepEmpty: false,
    root: false,
    ...overrides,
  });

  test("title_and_rebase_button", () => {
    open();
    expect(screen.getByRole("dialog", { name: "Rebase" })).toBeTruthy();
    expect(rebase().className).toContain("default");
    expect(screen.getByText("git rebase")).toBeTruthy();
    expect(upstream().placeholder).toBe("branch or hash");
  });

  test("rebase_without_a_base_cannot_be_accepted", () => {
    open();
    expect(rebase().disabled).toBe(true);
  });

  test("accepting_sends_the_base_and_nothing_else", async () => {
    const { onanswer, validate } = open({ upstream: "main" });
    expect(rebase().disabled).toBe(false);

    await fireEvent.click(rebase());

    await waitFor(() => expect(onanswer).toHaveBeenCalled());
    expect(validate).toHaveBeenCalledWith(["main"]);
    expect(onanswer).toHaveBeenCalledWith({
      params: params({ upstream: "main" }),
      newBase: "main",
    });
  });

  test("rebase_onto_moves_the_value_between_fields", async () => {
    const { onanswer } = open();
    await type(upstream(), "origin/main");
    expect(onto()).toBeNull();

    await fireEvent.click(modify());
    expect(screen.getByText("Add Rebase Options")).toBeTruthy();
    await fireEvent.click(option("--onto"));

    // The value goes to the new base and the field above becomes the old base.
    expect(onto()?.value).toBe("origin/main");
    expect(onto()?.placeholder).toBe("new base (branch or hash)");
    expect(upstream().value).toBe("");
    expect(upstream().placeholder).toBe("old base (branch or hash)");
    expect(rebase().disabled).toBe(true);

    await type(upstream(), "old");
    await fireEvent.click(rebase());
    await waitFor(() => expect(onanswer).toHaveBeenCalled());
    expect(onanswer).toHaveBeenCalledWith({
      params: params({ upstream: "old", onto: "origin/main" }),
      newBase: "origin/main",
    });

    // Removing it gives the value back to the usual field.
    await fireEvent.click(option("--onto"));
    expect(onto()).toBeNull();
    expect(upstream().value).toBe("origin/main");
    expect(upstream().placeholder).toBe("branch or hash");
  });

  test("empty_field_errors_with_intellijs_texts", async () => {
    open();
    await fireEvent.click(modify());
    await fireEvent.click(option("--onto"));

    // With `--onto`, the field above is the upstream and the new base one is empty.
    const texts = () => screen.getAllByRole("alert").map((item) => item.textContent);
    expect(texts()).toEqual(["Select an upstream", "Select a new base"]);

    await fireEvent.click(option("--onto"));
    expect(texts()).toEqual(["Select a new base"]);
  });

  test("root_disables_upstream", async () => {
    const { onanswer } = open({ upstream: "main" });
    await fireEvent.click(modify());

    await fireEvent.click(option("--root"));

    expect(upstream().disabled).toBe(true);
    expect(screen.getByRole("button", { name: "Remove --root" })).toBeTruthy();
    await fireEvent.click(rebase());
    await waitFor(() => expect(onanswer).toHaveBeenCalled());
    // There is no base to send, but the one there was is remembered.
    expect(onanswer).toHaveBeenCalledWith({
      params: params({ root: true }),
      newBase: "main",
    });
  });

  test("root_without_a_base_can_be_accepted", async () => {
    open({ options: ["root"] });
    expect(upstream().disabled).toBe(true);
    expect(rebase().disabled).toBe(false);
  });

  test("the_branch_to_rebase_asks_for_an_existing_local_one", async () => {
    const { onanswer } = open({ upstream: "main" });
    expect(source()).toBeNull();
    await fireEvent.click(modify());
    await fireEvent.click(option("Select another branch to rebase"));
    expect(source()?.placeholder).toBe("source branch");

    await fireEvent.click(rebase());
    expect((await screen.findAllByRole("alert")).map((item) => item.textContent)).toEqual([
      "Select a branch to switch to",
    ]);

    await type(source() as HTMLElement, "made-up");
    expect((await screen.findAllByRole("alert")).map((item) => item.textContent)).toEqual([
      "Branch 'made-up' does not exist",
    ]);
    expect(onanswer).not.toHaveBeenCalled();

    await type(source() as HTMLElement, "side");
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
    await fireEvent.click(rebase());
    await waitFor(() => expect(onanswer).toHaveBeenCalled());
    expect(onanswer).toHaveBeenCalledWith({
      params: params({ upstream: "main", branch: "side" }),
      newBase: "main",
    });
  });

  test("a_base_that_git_does_not_resolve_gives_there_is_no_such_branch_or_tag", async () => {
    const validate = vi.fn(async (revisions: string[]) => revisions.map((name) => name !== "nothing"));
    const { onanswer } = open({ upstream: "nothing", validate });

    await fireEvent.click(rebase());

    expect((await screen.findByRole("alert")).textContent).toBe(
      "There is no such branch or tag",
    );
    expect(onanswer).not.toHaveBeenCalled();

    await type(upstream(), "main");
    await fireEvent.click(rebase());
    await waitFor(() => expect(onanswer).toHaveBeenCalled());
  });

  test("remembered_options_come_out_as_buttons_and_travel_in_the_answer", async () => {
    const { onanswer } = open({ upstream: "main", options: ["keepEmpty", "interactive"] });
    expect(screen.getByText("--keep-empty")).toBeTruthy();
    expect(screen.getByText("--interactive")).toBeTruthy();

    await fireEvent.click(screen.getByRole("button", { name: "Remove --interactive" }));
    await fireEvent.click(modify());
    await fireEvent.click(option("--rebase-merges"));
    await fireEvent.click(rebase());

    await waitFor(() => expect(onanswer).toHaveBeenCalled());
    expect(onanswer).toHaveBeenCalledWith({
      params: params({ upstream: "main", keepEmpty: true, rebaseMerges: true }),
      newBase: "main",
    });
  });

  test("with_git_older_than_2_22_the_flag_is_preserve_merges_and_it_does_not_go_with_interactive", async () => {
    open({ upstream: "main", version: parseGitVersion("2.21.0"), options: ["interactive"] });
    await fireEvent.click(modify());

    expect(option("--preserve-merges").disabled).toBe(true);
    await fireEvent.click(screen.getByRole("button", { name: "Remove --interactive" }));
    expect(option("--preserve-merges").disabled).toBe(false);
  });

  test("the_options_come_separated_and_switch_branch_has_no_flag", async () => {
    open();
    await fireEvent.click(modify());
    const labels = within(screen.getByRole("group", { name: "Add Rebase Options" }))
      .getAllByRole("checkbox")
      .map((box) => box.getAttribute("aria-label"));

    expect(labels).toEqual([
      "Select another branch to rebase",
      "--onto",
      "--rebase-merges",
      "--keep-empty",
      "--root",
      "--interactive",
    ]);
  });

  test("cancelling_answers_nothing", async () => {
    const { onanswer } = open();

    await fireEvent.click(screen.getByRole("button", { name: "Cancel" }));

    expect(onanswer).toHaveBeenCalledWith(null);
  });
});

describe("FilesDialog", () => {
  test("lists_the_files_as_text_and_closes_with_ok", async () => {
    const onanswer = vi.fn();
    render(FilesDialog, {
      title: "Untracked Files Preventing Merge",
      message: "These untracked files will be overwritten by Merge",
      files: ["a.txt", "<b>x</b>.txt"],
      onanswer,
    });

    expect(screen.getByRole("dialog", { name: "Untracked Files Preventing Merge" })).toBeTruthy();
    expect(screen.getByText("These untracked files will be overwritten by Merge")).toBeTruthy();
    expect(screen.getByText("<b>x</b>.txt")).toBeTruthy();
    await fireEvent.click(screen.getByRole("button", { name: "OK" }));
    expect(onanswer).toHaveBeenCalled();
  });
});

// The flow, with a fake interface and repository.
const NORMAL: RepoState = { operation: null, branch: "main", label: "main", head: HEAD };
const LIST = {
  local: [
    { name: "main", tracked: "origin/main" },
    { name: "side", tracked: null },
  ],
  remote: ["origin/main", "origin/side"],
  recent: [],
  current: "main",
  previous: null,
} as BranchList;
const MERGED: MergeOutcome = {
  kind: "merged",
  reason: { files: 3, commits: 2, range: ["aaa", "bbb"], matching: null },
};

// What `rebase::Process` returns: with nothing saved.
const ran = (outcome: RebaseOutcome): RebaseRun => ({ outcome, title: null, saved: [] });

function api(overrides: Partial<GitApi> = {}): GitApi {
  return {
    state: vi.fn(async () => NORMAL),
    branches: vi.fn(async () => LIST),
    tags: vi.fn(async () => ["v1"]),
    settings: vi.fn(async () => ({
      protectedBranches: [],
      forcePushTargets: [],
      merge: { branch: null, options: [] as MergeOption[] },
      rebase: { options: [] as RebaseFlag[], newBase: null },
    })),
    version: vi.fn(async () => "git version 2.39.5 (Apple Git-154)"),
    unmerged: vi.fn(async () => ["side", "origin/side"]),
    saveMerge: vi.fn(async () => {}),
    merge: vi.fn(async () => MERGED),
    commitMerge: vi.fn(async () => {}),
    summary: vi.fn(async () => MERGED),
    validate: vi.fn(async (revisions: string[]) => revisions.map(() => true)),
    saveRebase: vi.fn(async () => {}),
    rebase: vi.fn(async () => ran({ kind: "done" })),
    published: vi.fn(async () => null),
    ...overrides,
  };
}

function ui(answers: unknown[] = [], overrides: Partial<GitUi> = {}) {
  const asked: GitRequest[] = [];
  const ask = vi.fn(async (request: GitRequest) => {
    asked.push(request);
    return answers.shift() ?? null;
  });
  const parts: GitUi = {
    ask: ask as GitUi["ask"],
    resolveConflicts: vi.fn(async () => "resolved" as const),
    rebaseStopped: vi.fn(),
    rebaseConflicts: vi.fn(async () => {}),
    viewCommits: vi.fn(),
    ...overrides,
  };
  return { parts, asked };
}

const shown = () => get(notifications);
const titled = (title: string) => shown().find((item) => item.title === title) as Shown;
const click = (title: string, label: string) =>
  titled(title).actions?.find((action) => action.label === label)?.run();
const CHOICE: MergeChoice = { branch: "side", options: [], message: "" };

describe("Merge…", () => {
  test("asks_with_the_local_and_remote_branches_and_what_is_remembered", async () => {
    const backend = api({
      settings: vi.fn(async () => ({
        protectedBranches: [],
        forcePushTargets: [],
        merge: { branch: "origin/side", options: ["noVerify", "noFf"] as MergeOption[] },
        rebase: { options: [], newBase: null },
      })),
    });
    const { parts, asked } = ui([null]);

    await gitFlow(parts, backend).merge();

    expect(asked[0]).toMatchObject({
      kind: "merge",
      current: "main",
      branches: ["main", "side", "origin/main", "origin/side"],
      branch: "origin/side",
      options: ["noFf", "noVerify"],
    });
    await expect((asked[0] as Extract<GitRequest, { kind: "merge" }>).unmerged).resolves.toEqual([
      "side",
      "origin/side",
    ]);
    expect(backend.merge).not.toHaveBeenCalled();
    expect(backend.saveMerge).not.toHaveBeenCalled();
  });

  test("remembered_no_verify_is_lost_if_git_is_older_than_2_24", async () => {
    const backend = api({
      version: vi.fn(async () => "2.20.1"),
      settings: vi.fn(async () => ({
        protectedBranches: [],
        forcePushTargets: [],
        merge: { branch: null, options: ["noVerify", "noFf"] as MergeOption[] },
        rebase: { options: [], newBase: null },
      })),
    });
    const { parts, asked } = ui([null]);

    await gitFlow(parts, backend).merge();

    expect(asked[0]).toMatchObject({ options: ["noFf"] });
  });

  test("remembers_branch_and_options_not_the_message", async () => {
    const backend = api();
    const choice: MergeChoice = {
      branch: "side",
      options: ["noFf", "message"],
      message: "my message",
    };

    await gitFlow(ui([choice]).parts, backend).merge();

    expect(backend.saveMerge).toHaveBeenCalledWith("side", ["noFf", "message"]);
    expect(JSON.stringify((backend.saveMerge as ReturnType<typeof vi.fn>).mock.calls)).not.toContain(
      "my message",
    );
    expect(backend.merge).toHaveBeenCalledWith("side", ["noFf", "message"], "my message", HEAD);
  });

  test("the_message_only_travels_with_the_m_option", async () => {
    const backend = api();

    await gitFlow(ui([{ ...CHOICE, options: ["noFf"], message: "loose" }]).parts, backend).merge();

    expect(backend.merge).toHaveBeenCalledWith("side", ["noFf"], null, HEAD);
  });

  test("merge_hidden_during_a_merge", async () => {
    const state = (operation: RepoState["operation"], branch: string | null = "main") => ({
      ...NORMAL,
      operation,
      branch,
    });
    // The native menu does not hide: it turns it off with any half-done operation or with a detached HEAD.
    expect(gitMenuEnabled(NORMAL)).toEqual(expect.arrayContaining(["Git.Merge", "Git.Rebase"]));
    for (const busy of [state("merging"), state("rebasing"), state("cherryPicking"), state("reverting")]) {
      expect(gitMenuEnabled(busy)).not.toContain("Git.Merge");
      expect(gitMenuEnabled(busy)).not.toContain("Git.Rebase");
    }
    expect(gitMenuEnabled(state(null, null))).not.toContain("Git.Merge");
    expect(gitMenuEnabled(state(null, null))).not.toContain("Git.Rebase");

    // And if it arrives anyway, it opens nothing.
    for (const busy of [state("merging"), state("rebasing"), state(null, null)]) {
      const backend = api({ state: vi.fn(async () => busy) });
      const { parts, asked } = ui([CHOICE]);
      const flow = gitFlow(parts, backend);

      await flow.merge();
      await flow.rebase();

      expect(asked).toEqual([]);
      expect(backend.merge).not.toHaveBeenCalled();
    }
  });

  test("result_n_files_updated_in_m_commits", async () => {
    const { parts } = ui([CHOICE]);

    await gitFlow(parts, api()).merge();

    expect(titled("3 files updated in 2 commits")).toBeTruthy();
    click("3 files updated in 2 commits", "View commits");
    expect(parts.viewCommits).toHaveBeenCalledWith(["aaa", "bbb"]);
  });

  test("with_a_path_filter_the_body_says_how_many_match", async () => {
    const outcome: MergeOutcome = {
      kind: "merged",
      reason: { files: 1, commits: 1, range: ["a", "b"], matching: 0 },
    };

    await gitFlow(ui([CHOICE]).parts, api({ merge: vi.fn(async () => outcome) })).merge();

    expect(titled("1 file updated in 1 commit").body).toBe("No commits matching filters");
  });

  test("without_commits_but_with_files_says_n_files_updated", async () => {
    const outcome: MergeOutcome = {
      kind: "merged",
      reason: { files: 2, commits: 0, range: null, matching: null },
    };

    await gitFlow(
      ui([{ ...CHOICE, options: ["noCommit"] }]).parts,
      api({ merge: vi.fn(async () => outcome) }),
    ).merge();

    expect(titled("2 files updated").actions).toBeUndefined();
  });

  test("already_up_to_date_says_all_files_are_up_to_date", async () => {
    await gitFlow(
      ui([CHOICE]).parts,
      api({ merge: vi.fn(async (): Promise<MergeOutcome> => ({ kind: "upToDate" })) }),
    ).merge();

    expect(titled("All files are up to date")).toBeTruthy();
  });

  const conflicting = () =>
    api({ merge: vi.fn(async (): Promise<MergeOutcome> => ({ kind: "conflicts" })) });

  test("conflict_opens_conflicts_and_on_resolving_does_merge_commit_except_no_commit", async () => {
    // With commit: once everything is resolved, the merge commit.
    const backend = conflicting();
    const resolving = vi.fn(async (options?: { onresolved?: () => Promise<void> }) => {
      await options?.onresolved?.();
      return "resolved" as const;
    });
    await gitFlow(ui([CHOICE], { resolveConflicts: resolving }).parts, backend).merge();
    expect(resolving).toHaveBeenCalledTimes(1);
    expect(backend.commitMerge).toHaveBeenCalledTimes(1);

    // With `--no-commit` nothing is committed, like `GitMergeAction`.
    const noCommit = conflicting();
    const again = vi.fn(async (options?: { onresolved?: () => Promise<void> }) => {
      await options?.onresolved?.();
      return "resolved" as const;
    });
    await gitFlow(
      ui([{ ...CHOICE, options: ["noCommit"] }], { resolveConflicts: again }).parts,
      noCommit,
    ).merge();
    expect(again).toHaveBeenCalledTimes(1);
    expect(noCommit.commitMerge).not.toHaveBeenCalled();
  });

  test("with_squash_the_resolved_conflict_is_also_committed", async () => {
    // `GitMerge.displayDialog` only skips the commit with `--no-commit`.
    const backend = conflicting();
    const resolving = vi.fn(async (options?: { onresolved?: () => Promise<void> }) => {
      await options?.onresolved?.();
      return "resolved" as const;
    });

    await gitFlow(
      ui([{ ...CHOICE, options: ["squash"] }], { resolveConflicts: resolving }).parts,
      backend,
    ).merge();

    expect(backend.commitMerge).toHaveBeenCalledTimes(1);
  });

  test("conflict_resolved_and_committed_says_n_files_updated_in_m_commits", async () => {
    const backend = conflicting();
    const resolving = vi.fn(async (options?: { onresolved?: () => Promise<void> }) => {
      await options?.onresolved?.();
      return "resolved" as const;
    });
    const { parts } = ui([CHOICE], { resolveConflicts: resolving });

    await gitFlow(parts, backend).merge();

    // The summary is asked for on committing, from the HEAD before the merge.
    expect(backend.summary).toHaveBeenCalledWith("side", HEAD);
    expect(titled("3 files updated in 2 commits")).toBeTruthy();
    click("3 files updated in 2 commits", "View commits");
    expect(parts.viewCommits).toHaveBeenCalledWith(["aaa", "bbb"]);
  });

  test("without_resolving_or_with_no_commit_there_is_no_summary", async () => {
    const pending = conflicting();
    await gitFlow(ui([CHOICE]).parts, pending).merge();
    expect(pending.summary).not.toHaveBeenCalled();

    const noCommit = conflicting();
    const resolving = vi.fn(async (options?: { onresolved?: () => Promise<void> }) => {
      await options?.onresolved?.();
      return "resolved" as const;
    });
    await gitFlow(
      ui([{ ...CHOICE, options: ["noCommit"] }], { resolveConflicts: resolving }).parts,
      noCommit,
    ).merge();
    expect(noCommit.summary).not.toHaveBeenCalled();
  });

  test("if_the_summary_fails_nothing_more_is_notified", async () => {
    const backend = api({
      merge: vi.fn(async (): Promise<MergeOutcome> => ({ kind: "conflicts" })),
      summary: vi.fn(async () => {
        throw { kind: "failed", message: "bad" };
      }),
    });
    const resolving = vi.fn(async (options?: { onresolved?: () => Promise<void> }) => {
      await options?.onresolved?.();
      return "resolved" as const;
    });

    await gitFlow(ui([CHOICE], { resolveConflicts: resolving }).parts, backend).merge();

    expect(shown()).toEqual([]);
  });

  test("local_changes_give_git_merge_failed_with_view_them", async () => {
    const outcome: MergeOutcome = { kind: "localChanges", reason: { files: ["a.txt", "b.txt"] } };
    const { parts, asked } = ui([CHOICE, null]);
    await gitFlow(parts, api({ merge: vi.fn(async () => outcome) })).merge();

    const card = titled("Git Merge Failed");
    expect(card.kind).toBe("error");
    expect(card.body).toBe(
      "Your local changes will be overwritten by merge.\nCommit, stash, or revert them to proceed.",
    );
    expect(card.actions?.map(({ label }) => label)).toEqual(["View them"]);

    click("Git Merge Failed", "View them");
    await vi.waitFor(() => expect(asked).toHaveLength(2));

    expect(asked[1]).toEqual({
      kind: "files",
      title: "Local Changes Prevent from Merge",
      message:
        "Your local changes will be overwritten by merge.\nCommit, stash, or revert them to proceed.",
      files: ["a.txt", "b.txt"],
    });
  });

  test("untracked_gives_untracked_files_prevent_merge_with_view_files", async () => {
    const outcome: MergeOutcome = { kind: "untracked", reason: { files: ["new.txt"] } };
    const { parts, asked } = ui([CHOICE, null]);
    await gitFlow(parts, api({ merge: vi.fn(async () => outcome) })).merge();

    const card = titled("Untracked Files Prevent Merge");
    expect(card.kind).toBe("error");
    expect(card.body).toBe("Move or commit them before Merge");

    click("Untracked Files Prevent Merge", "View Files…");
    await vi.waitFor(() => expect(asked).toHaveLength(2));

    expect(asked[1]).toEqual({
      kind: "files",
      title: "Untracked Files Preventing Merge",
      message: "These untracked files will be overwritten by Merge",
      files: ["new.txt"],
    });
  });

  test("other_failures_give_git_merge_failed_with_what_git_said", async () => {
    await gitFlow(
      ui([CHOICE]).parts,
      api({ merge: vi.fn(async (): Promise<MergeOutcome> => ({ kind: "failed", reason: "bad" })) }),
    ).merge();
    expect(titled("Git Merge failed").body).toBe("bad");

    await gitFlow(
      ui([CHOICE]).parts,
      api({ merge: vi.fn(async (): Promise<MergeOutcome> => ({ kind: "unmerged" })) }),
    ).merge();
    expect(shown().filter((item) => item.title === "Git Merge failed")).toHaveLength(2);
  });

  test("if_the_repository_changed_the_error_comes_out_as_a_notice", async () => {
    const backend = api({
      merge: vi.fn(async () => {
        throw { kind: "failed", message: "Repository has already been changed" };
      }),
    });

    await gitFlow(ui([CHOICE]).parts, backend).merge();

    const card = titled("Git Merge failed");
    expect(card.kind).toBe("error");
    expect(card.body).toBe("Repository has already been changed");
  });

  test("cancelling_the_dialog_does_nothing", async () => {
    const backend = api();

    await gitFlow(ui([null]).parts, backend).merge();

    expect(backend.merge).not.toHaveBeenCalled();
    expect(shown()).toEqual([]);
  });
});

describe("Rebase…", () => {
  const choice = (overrides: Partial<RebaseChoice["params"]> = {}, newBase = "main"): RebaseChoice => ({
    params: {
      upstream: "main",
      onto: null,
      branch: null,
      interactive: false,
      rebaseMerges: false,
      keepEmpty: false,
      root: false,
      ...overrides,
    },
    newBase,
  });
  const remembered = (options: RebaseFlag[], newBase: string | null) =>
    vi.fn(async () => ({
      protectedBranches: [],
      forcePushTargets: [],
      merge: { branch: null, options: [] as MergeOption[] },
      rebase: { options, newBase },
    }));

  test("asks_with_the_branches_and_the_tags", async () => {
    const { parts, asked } = ui([null]);

    await gitFlow(parts, api()).rebase();

    expect(asked[0]).toMatchObject({
      kind: "rebase",
      branches: ["main", "side"],
      refs: ["main", "side", "origin/main", "origin/side", "v1"],
      upstream: "",
      options: [],
    });
  });

  test("rebase_remembers_flags_and_new_base_if_it_resolves", async () => {
    const valid = api({ settings: remembered(["keepEmpty", "root"], "origin/main") });
    const first = ui([null]);

    await gitFlow(first.parts, valid).rebase();

    expect(valid.validate).toHaveBeenCalledWith(["origin/main"]);
    expect(first.asked[0]).toMatchObject({ upstream: "origin/main", options: ["keepEmpty", "root"] });

    // If it no longer resolves, the field stays empty, like `loadSettings`.
    const gone = api({
      settings: remembered(["keepEmpty"], "deleted"),
      validate: vi.fn(async (revisions: string[]) => revisions.map(() => false)),
    });
    const second = ui([null]);
    await gitFlow(second.parts, gone).rebase();
    expect(second.asked[0]).toMatchObject({ upstream: "", options: ["keepEmpty"] });

    // And without a remembered base nothing is validated.
    const none = api({ settings: remembered([], null) });
    await gitFlow(ui([null]).parts, none).rebase();
    expect(none.validate).not.toHaveBeenCalled();
  });

  test("accepting_saves_the_flags_and_the_base_and_launches_the_rebase", async () => {
    const backend = api();
    const answer = choice({ keepEmpty: true, root: true, upstream: null }, "origin/main");

    await gitFlow(ui([answer]).parts, backend).rebase();

    // `--onto` and the branch to rebase are not remembered, like `REBASE_FLAGS`.
    expect(backend.saveRebase).toHaveBeenCalledWith(["keepEmpty", "root"], "origin/main");
    expect(backend.rebase).toHaveBeenCalledWith(answer.params, HEAD);
  });

  test("the_flags_are_saved_in_intellijs_order", async () => {
    const backend = api();

    await gitFlow(
      ui([choice({ interactive: true, rebaseMerges: true, keepEmpty: true })]).parts,
      backend,
    ).rebase();

    expect(backend.saveRebase).toHaveBeenCalledWith(
      ["rebaseMerges", "keepEmpty", "interactive"],
      "main",
    );
  });

  test("successful_rebase_says_rebased_branch_on_base", async () => {
    const backend = api({
      state: vi
        .fn()
        .mockResolvedValueOnce(NORMAL)
        .mockResolvedValue({ ...NORMAL, branch: "side", label: "side" }),
    });

    await gitFlow(ui([choice({ upstream: "main" })]).parts, backend).rebase();

    const card = titled("Rebase successful");
    expect(card.kind).toBe("info");
    expect(card.body).toBe("Rebased side on main");
  });

  test("the_success_message_is_the_one_of_git_successful_rebase", () => {
    expect(rebasedMessage("side", "main", false)).toBe("Rebased side on main");
    expect(rebasedMessage("side", null, false)).toBe("Rebased side");
    expect(rebasedMessage(null, "main", false)).toBe("Rebased on main");
    expect(rebasedMessage(null, null, false)).toBe("Rebased");
    expect(rebasedMessage("side", "main", true)).toBe("Checked out side and rebased it on main");
    expect(rebasedMessage(null, null, true)).toBe("Checked out and rebased it");
  });

  test("with_onto_the_base_of_the_notice_is_the_new_one", async () => {
    const backend = api();

    await gitFlow(
      ui([choice({ upstream: "old", onto: "origin/main" }, "origin/main")]).parts,
      backend,
    ).rebase();

    expect(titled("Rebase successful").body).toBe("Rebased main on origin/main");
  });

  test("with_another_branch_it_says_checked_out_and_rebased_it", async () => {
    const backend = api({
      state: vi
        .fn()
        .mockResolvedValueOnce(NORMAL)
        .mockResolvedValue({ ...NORMAL, branch: "side", label: "side" }),
    });

    await gitFlow(ui([choice({ branch: "side" })]).parts, backend).rebase();

    expect(titled("Rebase successful").body).toBe("Checked out side and rebased it on main");
  });

  test("with_conflicts_task_2_follows", async () => {
    const backend = api({
      rebase: vi.fn(async () => ran({ kind: "conflicts" })),
    });
    const { parts } = ui([choice()]);

    await gitFlow(parts, backend).rebase();

    expect(parts.rebaseConflicts).toHaveBeenCalledTimes(1);
    expect(parts.rebaseStopped).not.toHaveBeenCalled();
  });

  test("stopped_for_editing_uses_the_card_of_task_2", async () => {
    const backend = api({
      rebase: vi.fn(async () => ran({ kind: "editing" })),
      state: vi
        .fn()
        .mockResolvedValueOnce(NORMAL)
        .mockResolvedValue({ ...NORMAL, operation: "rebasing" }),
    });
    const { parts } = ui([choice()]);

    await gitFlow(parts, backend).rebase();

    expect(parts.rebaseStopped).toHaveBeenCalledWith({ kind: "editing" }, []);
  });

  test("a_failure_that_did_not_start_the_rebase_offers_neither_retry_nor_abort", async () => {
    const backend = api({
      rebase: vi.fn(async () =>
        ran({ kind: "failed", reason: "cannot rebase: You have unstaged changes." }),
      ),
    });
    const { parts } = ui([choice()]);

    await gitFlow(parts, backend).rebase();

    const card = titled("Rebase failed");
    expect(card.kind).toBe("error");
    expect(card.body).toBe("cannot rebase: You have unstaged changes.");
    expect(card.actions).toBeUndefined();
    expect(parts.rebaseStopped).not.toHaveBeenCalled();
  });

  test("a_failure_with_the_rebase_in_progress_goes_to_the_card_of_task_2", async () => {
    const outcome: RebaseOutcome = { kind: "failed", reason: "bad" };
    const backend = api({
      rebase: vi.fn(async () => ran(outcome)),
      state: vi
        .fn()
        .mockResolvedValueOnce(NORMAL)
        .mockResolvedValue({ ...NORMAL, operation: "rebasing" }),
    });
    const { parts } = ui([choice()]);

    await gitFlow(parts, backend).rebase();

    expect(parts.rebaseStopped).toHaveBeenCalledWith(outcome, []);
  });

  test("a_command_error_comes_out_as_rebase_failed", async () => {
    const backend = api({
      rebase: vi.fn(async () => {
        throw { kind: "failed", message: "Repository has already been changed" };
      }),
    });

    await gitFlow(ui([choice()]).parts, backend).rebase();

    expect(titled("Rebase failed").body).toBe("Repository has already been changed");
  });

  // `GitRebase.java` goes through `GitRebaseProcess`: published commits are asked about, and what was saved
  // that does not come back is notified.
  test("published_rebase_asks_first_and_cancelling_does_not_rebase", async () => {
    const backend = api({ published: vi.fn(async () => "origin/main") });
    const { parts, asked } = ui([choice({ upstream: "main" }), null]);

    await gitFlow(parts, backend).rebase();

    expect(backend.published).toHaveBeenCalledWith("main", "main");
    expect(asked[1]).toEqual({ kind: "publishedCommits" });
    expect(backend.rebase).not.toHaveBeenCalled();
  });

  test("what_was_saved_that_does_not_come_back_is_notified_and_what_is_stopped_stays_saved", async () => {
    const saved = { sha: "abc", message: "Uncommitted changes before rebase" };
    const blocked = { kind: "restoreBlocked" as const, reason: { files: ["a.txt"] } };
    const back = api({
      rebase: vi.fn(async () => ({ ...ran({ kind: "done" }), saved: [{ saved, restore: blocked }] })),
    });
    const restored = vi.fn();
    const first = ui([choice({ upstream: "main" })], { restored });
    await gitFlow(first.parts, back).rebase();
    expect(restored).toHaveBeenCalledWith([{ saved, restore: blocked }], {
      operation: "rebase",
      from: "main",
    });

    const stopped = api({
      rebase: vi.fn(async () => ({
        ...ran({ kind: "conflicts" }),
        saved: [{ saved, restore: null }],
      })),
    });
    const second = ui([choice({ upstream: "main" })]);
    await gitFlow(second.parts, stopped).rebase();
    expect(second.parts.rebaseConflicts).toHaveBeenCalledWith([saved]);
  });

  test("cancelling_the_dialog_does_nothing", async () => {
    const backend = api();

    await gitFlow(ui([null]).parts, backend).rebase();

    expect(backend.saveRebase).not.toHaveBeenCalled();
    expect(backend.rebase).not.toHaveBeenCalled();
  });
});
