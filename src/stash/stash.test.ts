import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { get } from "svelte/store";
import type { BranchList, RepoState, StashEntry, Unstashed } from "../lib/api";
import { dismiss, notifications } from "../lib/notifications";
import { onTabRequested } from "../lib/tabs";
import {
  clearConfirmation,
  dropConfirmation,
  stashFiles,
  stashFlow,
  unstashBranchProblem,
  type StashApi,
  type StashRequest,
  type StashUi,
  type UnstashChoice,
} from "./flow";
import type { SavedChange, SavedDiff } from "./savedPatches";
import StashDialog from "./StashDialog.svelte";
import UnstashAsDialog from "./UnstashAsDialog.svelte";
import UnstashDialog from "./UnstashDialog.svelte";

const HEAD = "a1b2c3d4".repeat(5);
const NAMES = { local: ["main", "taken"], remote: ["origin/main"] };

function entry(index: number, subject: string): StashEntry {
  return {
    sha: String(index).repeat(40),
    parents: ["p".repeat(40), "i".repeat(40)],
    time: 1_700_000_000_000,
    index,
    subject,
    branch: "main",
    prefix: "On main",
    message: subject,
  };
}

const shown = () => get(notifications);
const titles = () => shown().map(({ title }) => title);

beforeEach(() => shown().forEach(({ id }) => dismiss(id)));
afterEach(cleanup);

describe("StashDialog", () => {
  test("the_stash_dialog_texts_and_keep_index", async () => {
    const onanswer = vi.fn();
    render(StashDialog, { root: "/work/ramify", branch: "main", onanswer });

    const dialog = screen.getByRole("dialog", { name: "Stash" });
    expect(within(dialog).getByText("Git Root:")).toBeTruthy();
    expect(within(dialog).getByText("/work/ramify")).toBeTruthy();
    expect(within(dialog).getByText("Current Branch:")).toBeTruthy();
    expect(within(dialog).getByText("main")).toBeTruthy();
    const message = within(dialog).getByLabelText("Message:") as HTMLInputElement;
    expect(document.activeElement).toBe(message);
    expect(message.title).toBe("Enter stash message here.");
    const keep = within(dialog).getByLabelText("Keep index") as HTMLInputElement;
    expect(keep.checked).toBe(false);

    await fireEvent.input(message, { target: { value: "my work" } });
    await fireEvent.click(keep);
    await fireEvent.click(within(dialog).getByRole("button", { name: "Create Stash" }));

    expect(onanswer).toHaveBeenCalledWith({ message: "my work", keepIndex: true });
  });

  test("cancel_answers_nothing", async () => {
    const onanswer = vi.fn();
    render(StashDialog, { root: "/work/ramify", branch: "main", onanswer });

    await fireEvent.click(screen.getByRole("button", { name: "Cancel" }));

    expect(onanswer).toHaveBeenCalledWith(null);
  });
});

describe("confirmations", () => {
  test("remove_stash_asks_with_its_name_and_message", () => {
    expect(dropConfirmation(entry(0, "my work"))).toEqual({
      title: "Remove Stash stash@{0}?",
      message: 'Do you want to remove stash@{0}?\n"my work"',
    });
  });

  test("remove_all_stashes_asks_first", () => {
    expect(clearConfirmation()).toEqual({
      title: "Remove All Stashes?",
      message: "Remove all stashes? This cannot be undone.",
    });
  });

  test("the_branch_is_checked_like_validate_name", () => {
    expect(unstashBranchProblem("  ", NAMES)).toBe("Specify name for the new branch");
    expect(unstashBranchProblem("head", NAMES)).toBe("HEAD is a reserved keyword");
    expect(unstashBranchProblem("a..b", NAMES)).toBe("Branch name a..b is not valid");
    expect(unstashBranchProblem("taken", NAMES)).toBe("Branch name taken already exists");
    expect(unstashBranchProblem("origin/main", NAMES)).toBe(
      "Branch name origin/main conflicts with remote branch with the same name",
    );
    expect(unstashBranchProblem("rescued", NAMES)).toBeNull();
  });
});

describe("UnstashAsDialog", () => {
  function open() {
    const onanswer = vi.fn();
    render(UnstashAsDialog, {
      root: "ramify",
      branch: "main",
      entry: entry(0, "my work"),
      names: NAMES,
      onanswer,
    });
    return { onanswer, dialog: screen.getByRole("dialog", { name: "Unstash Changes in ramify" }) };
  }

  test("unstash_changes_in_root_with_the_rows_and_the_ok_texts", async () => {
    const { dialog } = open();

    expect(within(dialog).getByText("Current branch:")).toBeTruthy();
    expect(within(dialog).getByText("main")).toBeTruthy();
    const branch = within(dialog).getByLabelText("As new branch:") as HTMLInputElement;
    expect(document.activeElement).toBe(branch);
    expect(branch.title).toBe("");
    const pop = within(dialog).getByLabelText("Pop stash") as HTMLInputElement;
    const index = within(dialog).getByLabelText("Reinstate index") as HTMLInputElement;
    expect([pop.checked, index.checked]).toEqual([false, false]);
    expect(within(dialog).getByRole("button", { name: "Apply Stash" })).toBeTruthy();

    await fireEvent.click(pop);
    expect(within(dialog).getByRole("button", { name: "Pop Stash" })).toBeTruthy();

    await fireEvent.input(branch, { target: { value: "rescued" } });
    expect(within(dialog).getByRole("button", { name: "Branch" })).toBeTruthy();
    expect([pop.disabled, index.disabled]).toEqual([true, true]);
  });

  // Decision 11: each checkbox is its own option.
  test("both_checkboxes_are_honoured", async () => {
    const { dialog, onanswer } = open();

    await fireEvent.click(within(dialog).getByLabelText("Reinstate index"));
    await fireEvent.click(within(dialog).getByRole("button", { name: "Apply Stash" }));

    expect(onanswer).toHaveBeenCalledWith({
      entry: entry(0, "my work"),
      branch: "",
      pop: false,
      reinstateIndex: true,
    });
  });

  test("a_branch_that_exists_cannot_be_accepted", async () => {
    const { dialog, onanswer } = open();

    await fireEvent.input(within(dialog).getByLabelText("As new branch:"), {
      target: { value: "taken" },
    });

    expect(within(dialog).getByText("Branch name taken already exists")).toBeTruthy();
    const ok = within(dialog).getByRole("button", { name: "Branch" }) as HTMLButtonElement;
    expect(ok.disabled).toBe(true);
    expect(onanswer).not.toHaveBeenCalled();
  });
});

describe("UnstashDialog", () => {
  function open(list: StashEntry[] = [entry(0, "newer"), entry(1, "older")]) {
    const onanswer = vi.fn();
    const actions = {
      load: vi.fn(async () => list),
      drop: vi.fn(async () => {}),
      clear: vi.fn(async () => {}),
      view: vi.fn(async (): Promise<SavedChange[] | null> => [
        { file: { path: "a.txt", oldPath: null, status: "M" }, group: null, source: "0".repeat(40) },
      ]),
      viewDiff: vi.fn(
        async (): Promise<SavedDiff> => ({
          left: { kind: "text", content: "local\n" },
          right: { kind: "text", content: "stashed\n" },
          titles: ["Base version", "0".repeat(40)],
        }),
      ),
      unstash: vi.fn(async () => true),
    };
    render(UnstashDialog, { root: "/work/ramify", branch: "main", names: NAMES, ...actions, onanswer });
    return { onanswer, actions, dialog: screen.getByRole("dialog", { name: "Unstash Changes" }) };
  }

  test("unstash_changes_lists_the_stashes_with_the_first_selected", async () => {
    const { dialog, onanswer, actions } = open();

    await waitFor(() => expect(within(dialog).getAllByRole("option")).toHaveLength(2));
    const rows = within(dialog).getAllByRole("option");
    expect(rows[0].textContent?.replace(/\s+/g, " ").trim()).toBe("stash@{0}: On main: newer");
    expect(rows[0].getAttribute("aria-selected")).toBe("true");
    for (const label of ["Git Root:", "Current Branch:", "Stashes:", "As new branch:"]) {
      expect(within(dialog).getByText(label)).toBeTruthy();
    }
    expect(within(dialog).getByRole("button", { name: "Close" })).toBeTruthy();

    await fireEvent.click(rows[1]);
    await fireEvent.click(within(dialog).getByLabelText("Reinstate index"));
    await fireEvent.click(within(dialog).getByRole("button", { name: "Apply Stash" }));

    const choice = { entry: entry(1, "older"), branch: "", pop: false, reinstateIndex: true };
    await waitFor(() => expect(onanswer).toHaveBeenCalledWith(choice));
    expect(actions.unstash).toHaveBeenCalledWith(choice);
  });

  // `GitUnstashDialog.doOKAction`: it only closes when the unstash completed.
  test("the_dialog_stays_open_until_the_unstash_completes", async () => {
    const { dialog, actions, onanswer } = open();
    await waitFor(() => expect(within(dialog).getAllByRole("option")).toHaveLength(2));
    actions.unstash.mockResolvedValueOnce(false);

    await fireEvent.click(within(dialog).getByRole("button", { name: "Apply Stash" }));

    await waitFor(() => expect(actions.unstash).toHaveBeenCalledTimes(1));
    expect(onanswer).not.toHaveBeenCalled();
    const ok = within(dialog).getByRole("button", { name: "Apply Stash" }) as HTMLButtonElement;
    await waitFor(() => expect(ok.disabled).toBe(false));
    expect(within(dialog).getAllByRole("option")[0].getAttribute("aria-selected")).toBe("true");
  });

  // `GitStashOperations.viewStash(project, stash, compareWithLocal = true)`: the changes browser, each
  // file of the stash against the local one.
  test("view_shows_the_changes_of_the_stash_against_local", async () => {
    const { dialog, actions } = open();
    await waitFor(() => expect(within(dialog).getAllByRole("option")).toHaveLength(2));

    await fireEvent.click(within(dialog).getByRole("button", { name: "View" }));

    const view = await screen.findByRole("dialog", { name: "Paths Affected by stash@{0}" });
    expect(within(view).getByRole("treeitem", { name: "a.txt" })).toBeTruthy();
    expect(actions.view).toHaveBeenCalledWith(entry(0, "newer"));
    await waitFor(() => expect(within(view).getByText("Base version")).toBeTruthy());
    expect(actions.viewDiff).toHaveBeenCalledWith(entry(0, "newer"), {
      file: { path: "a.txt", oldPath: null, status: "M" },
      group: null,
      source: "0".repeat(40),
    });
    await fireEvent.click(within(view).getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("dialog", { name: "Paths Affected by stash@{0}" })).toBeNull();
    expect(screen.getByRole("dialog", { name: "Unstash Changes" })).toBeTruthy();
  });

  // The error is a notification, not a row of the list.
  test("a_view_that_cannot_load_shows_nothing", async () => {
    const { dialog, actions } = open();
    await waitFor(() => expect(within(dialog).getAllByRole("option")).toHaveLength(2));
    actions.view.mockResolvedValueOnce(null);

    await fireEvent.click(within(dialog).getByRole("button", { name: "View" }));

    await waitFor(() => expect(actions.view).toHaveBeenCalledTimes(1));
    expect(screen.queryByRole("dialog", { name: "Paths Affected by stash@{0}" })).toBeNull();
  });

  test("a_branch_checks_and_disables_both_options", async () => {
    const { dialog } = open();
    await waitFor(() => expect(within(dialog).getAllByRole("option")).toHaveLength(2));
    const pop = within(dialog).getByLabelText("Pop stash") as HTMLInputElement;
    const index = within(dialog).getByLabelText("Reinstate index") as HTMLInputElement;

    await fireEvent.input(within(dialog).getByLabelText("As new branch:"), {
      target: { value: "rescued" },
    });

    expect(within(dialog).getByRole("button", { name: "Branch" })).toBeTruthy();
    expect([pop.checked, pop.disabled, index.checked, index.disabled]).toEqual([
      true,
      true,
      true,
      true,
    ]);

    await fireEvent.input(within(dialog).getByLabelText("As new branch:"), {
      target: { value: "" },
    });

    expect(within(dialog).getByRole("button", { name: "Apply Stash" })).toBeTruthy();
    expect([pop.checked, pop.disabled, index.checked, index.disabled]).toEqual([
      false,
      false,
      false,
      false,
    ]);
  });

  test("without_stashes_nothing_applies", async () => {
    const { dialog } = open([]);

    await waitFor(() => {
      const ok = within(dialog).getByRole("button", { name: "Apply Stash" }) as HTMLButtonElement;
      expect(ok.disabled).toBe(true);
    });
    for (const name of ["View", "Drop", "Clear"]) {
      expect((within(dialog).getByRole("button", { name }) as HTMLButtonElement).disabled).toBe(true);
    }
  });

  test("drop_asks_remove_stash_and_reloads", async () => {
    const { dialog, actions } = open();
    await waitFor(() => expect(within(dialog).getAllByRole("option")).toHaveLength(2));

    await fireEvent.click(within(dialog).getByRole("button", { name: "Drop" }));
    const confirm = screen.getByRole("dialog", { name: "Remove Stash stash@{0}?" });
    expect(within(confirm).getByText(/Do you want to remove stash@\{0\}\?/)).toBeTruthy();
    await fireEvent.click(within(confirm).getByRole("button", { name: "Yes" }));

    await waitFor(() => expect(actions.drop).toHaveBeenCalledWith(entry(0, "newer")));
    await waitFor(() => expect(actions.load).toHaveBeenCalledTimes(2));
  });

  test("clear_asks_remove_all_stashes_and_no_keeps_them", async () => {
    const { dialog, actions } = open();
    await waitFor(() => expect(within(dialog).getAllByRole("option")).toHaveLength(2));

    await fireEvent.click(within(dialog).getByRole("button", { name: "Clear" }));
    const confirm = screen.getByRole("dialog", { name: "Remove All Stashes?" });
    expect(within(confirm).getByText("Remove all stashes? This cannot be undone.")).toBeTruthy();
    await fireEvent.click(within(confirm).getByRole("button", { name: "No" }));

    expect(actions.clear).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog", { name: "Unstash Changes" })).toBeTruthy();
  });
});

describe("stashFlow", () => {
  const STATE: RepoState = { operation: null, branch: "main", label: "main", head: HEAD };
  const BRANCHES: BranchList = {
    local: [],
    remote: ["origin/main"],
    recent: [],
    current: "main",
    previous: null,
  };

  function setup(answers: Partial<Record<StashRequest["kind"], unknown>>, unstashed?: Unstashed) {
    const asked: StashRequest[] = [];
    const completed: boolean[] = [];
    const ui: StashUi = {
      ask: async <T,>(request: StashRequest) => {
        asked.push(request);
        const answer = answers[request.kind] ?? null;
        // The Unstash dialog runs the unstash itself and closes only if it completed.
        if (request.kind === "unstash" && answer) {
          completed.push(await request.unstash(answer as UnstashChoice));
        }
        return answer as T | null;
      },
      resolveConflicts: vi.fn(async () => "resolved" as const),
    };
    const api: StashApi = {
      state: vi.fn(async () => STATE),
      branches: vi.fn(async () => BRANCHES),
      list: vi.fn(async () => [entry(0, "my work")]),
      save: vi.fn(async () => {}),
      unstash: vi.fn(async () => unstashed ?? { kind: "applied" as const }),
      drop: vi.fn(async () => {}),
      clear: vi.fn(async () => {}),
      changes: vi.fn(async () => ({ files: [], groups: [] })),
      sides: vi.fn(async () => ({
        old: { kind: "text" as const, content: "base\n" },
        new: { kind: "text" as const, content: "stashed\n" },
      })),
      local: vi.fn(async () => ({ kind: "text" as const, content: "local\n" })),
    };
    const flow = stashFlow(ui, api);
    flow.repositoryChanged({ path: "/work/ramify" });
    return { flow, api, ui, asked, completed };
  }

  test("stash_changes_saves_and_offers_view_git_stashes", async () => {
    const { flow, api, asked } = setup({ stash: { message: "work", keepIndex: true } });
    const requested: string[] = [];
    const stop = onTabRequested((tab) => requested.push(tab));

    await flow.stashChanges();

    expect(asked[0]).toEqual({ kind: "stash", root: "/work/ramify", branch: "main" });
    expect(api.save).toHaveBeenCalledWith("work", true);
    const card = shown().find(({ title }) => title === "Files stashed successfully");
    expect(card?.actions?.map(({ label }) => label)).toEqual(["View Git stashes"]);
    card?.actions?.[0].run();
    expect(requested).toEqual(["stash"]);
    stop();
  });

  test("a_failed_stash_names_the_root", async () => {
    const { flow, api } = setup({ stash: { message: "", keepIndex: false } });
    vi.mocked(api.save).mockRejectedValue({ kind: "git", message: "fatal: nope" });

    await flow.stashChanges();

    expect(titles()).toEqual(["Failed to stash 'ramify'"]);
    expect(shown()[0].body).toContain("fatal: nope");
  });

  test("without_a_branch_the_dialog_says_no_active_branch", async () => {
    const { flow, api, asked } = setup({});
    vi.mocked(api.state).mockResolvedValue({ ...STATE, branch: null });

    await flow.stashChanges();

    expect(asked[0]).toMatchObject({ branch: "<no active branch>" });
    expect(api.save).not.toHaveBeenCalled();
  });

  test("unstash_passes_both_options_and_the_head", async () => {
    const choice = { entry: entry(0, "my work"), branch: "", pop: true, reinstateIndex: true };
    const { flow, api, completed } = setup({ unstash: choice });

    await flow.unstashChanges();

    expect(api.unstash).toHaveBeenCalledWith(entry(0, "my work").sha, true, true, null, HEAD);
    expect(titles()).toEqual(["Patch successfully applied"]);
    expect(completed).toEqual([true]);
  });

  test("unstash_as_a_branch_sends_only_the_branch", async () => {
    const choice = { entry: entry(0, "my work"), branch: " rescued ", pop: true, reinstateIndex: true };
    const { flow, api, asked } = setup({ unstashAs: choice });

    await flow.unstashAs(entry(0, "my work"));

    expect(asked[0]).toMatchObject({ kind: "unstashAs", root: "ramify", branch: "main" });
    expect(api.unstash).toHaveBeenCalledWith(entry(0, "my work").sha, false, false, "rescued", HEAD);
  });

  test("an_index_conflict_says_unstash_failed_due_to_an_index_conflict", async () => {
    const choice = { entry: entry(0, "w"), branch: "", pop: false, reinstateIndex: true };
    const { flow, completed } = setup(
      { unstash: choice },
      { kind: "indexConflict", reason: "Conflicts in index." },
    );

    await flow.unstashChanges();

    expect(completed).toEqual([false]);

    expect(titles()).toEqual(["Unstash failed due to an index conflict"]);
    expect(shown()[0].body).toBe("Conflicts in index.");
  });

  test("conflicts_open_the_conflicts_dialog_and_then_say_applied", async () => {
    const choice = { entry: entry(0, "w"), branch: "", pop: true, reinstateIndex: false };
    const { flow, ui, completed } = setup({ unstash: choice }, { kind: "conflicts" });

    await flow.unstashChanges();

    expect(completed).toEqual([true]);
    expect(ui.resolveConflicts).toHaveBeenCalledWith(
      expect.objectContaining({ title: "Unstashed with conflicts" }),
    );
    expect(titles()).toEqual(["Patch successfully applied"]);
  });

  test("conflicts_left_say_they_were_not_resolved_with_resolve_conflicts", async () => {
    const choice = { entry: entry(0, "w"), branch: "", pop: true, reinstateIndex: false };
    const { flow, ui, completed } = setup({ unstash: choice }, { kind: "conflicts" });
    vi.mocked(ui.resolveConflicts).mockImplementation(async (options) => {
      options?.onpending?.();
      return "pending";
    });

    await flow.unstashChanges();

    expect(completed).toEqual([false]);

    const card = shown()[0];
    expect(card.title).toBe("Conflicts were not resolved during unstash");
    expect(card.body).toBe("Unstash is not complete, you have unresolved merges in your working tree.");
    expect(card.actions?.map(({ label }) => label)).toEqual(["Resolve conflicts…"]);
    card.actions?.[0].run();
    expect(ui.resolveConflicts).toHaveBeenCalledTimes(2);
  });

  test("untracked_and_local_changes_use_the_unstash_helpers", async () => {
    const choice = { entry: entry(0, "w"), branch: "", pop: false, reinstateIndex: false };
    const untracked = setup(
      { unstash: choice },
      { kind: "untracked", reason: { files: ["n.txt"] } },
    );
    await untracked.flow.unstashChanges();
    const card = shown()[0];
    expect([card.title, card.body]).toEqual([
      "Untracked Files Prevent Unstash",
      "Move or commit them before unstash",
    ]);
    card.actions?.[0].run();
    await waitFor(() =>
      expect(untracked.asked.at(-1)).toEqual({
        kind: "files",
        title: "Untracked Files Preventing Unstash",
        message: "These untracked files will be overwritten by unstash",
        files: ["n.txt"],
      }),
    );
    dismiss(card.id);

    const local = setup({ unstash: choice }, { kind: "localChanges", reason: { files: ["a.txt"] } });
    await local.flow.unstashChanges();
    expect(titles()).toEqual(["Git Unstash Failed"]);
    expect(shown()[0].actions?.map(({ label }) => label)).toEqual(["View them"]);
  });

  // `getRevisionChanges(…, --first-parent, local = true)`: only the stash's own files, the local file
  // on the left (a current revision, titled with the default) and the stash's version on the right.
  test("the_changes_of_view_and_their_error", async () => {
    const { flow, api, asked } = setup({});
    const own = { path: "a.txt", oldPath: null, status: "M" };
    vi.mocked(api.changes).mockResolvedValueOnce({
      files: [own],
      groups: [
        { title: "Index on main", sha: "i".repeat(40), files: [own] },
        { title: "Untracked files on main", sha: "u".repeat(40), files: [{ path: "u.txt", oldPath: null, status: "A" }] },
      ],
    });
    await flow.unstashChanges();
    const request = asked[0] as Extract<StashRequest, { kind: "unstash" }>;
    const stash = entry(0, "w");

    const changes = await request.view(stash);
    expect(changes).toEqual([{ file: own, group: null, source: stash.sha }]);
    expect(await request.viewDiff(stash, changes![0])).toEqual({
      left: { kind: "text", content: "local\n" },
      right: { kind: "text", content: "stashed\n" },
      titles: ["Base version", stash.sha],
    });
    expect(api.local).toHaveBeenCalledWith("a.txt");
    expect(api.sides).toHaveBeenCalledWith(stash.sha, "a.txt", null);

    // `case 'A'`: an added file has no before revision, so there is nothing local on the left.
    const added = { file: { path: "n.txt", oldPath: null, status: "A" }, group: null, source: stash.sha };
    vi.mocked(api.local).mockClear();
    expect(await request.viewDiff(stash, added)).toEqual({
      left: { kind: "absent" },
      right: { kind: "text", content: "stashed\n" },
      titles: ["Base version", stash.sha],
    });
    expect(api.local).not.toHaveBeenCalled();

    vi.mocked(api.changes).mockRejectedValueOnce({ kind: "git", message: "fatal: gone" });
    expect(await request.view(stash)).toBeNull();
    expect(titles()).toEqual(["Paths Affected by stash@{0}"]);
    expect(shown()[0].body).toContain("fatal: gone");
  });

  test("drop_asks_first_and_no_keeps_the_stash", async () => {
    const { flow, api, asked } = setup({});

    await flow.drop(entry(0, "my work"));

    expect(asked[0]).toEqual({
      kind: "stashConfirm",
      title: "Remove Stash stash@{0}?",
      message: 'Do you want to remove stash@{0}?\n"my work"',
    });
    expect(api.drop).not.toHaveBeenCalled();
  });
});

describe("stashFiles", () => {
  test("stash_files_notifies_and_warns_about_other_staged_files", async () => {
    localStorage.clear();
    const push = vi.fn(async () => {});

    await stashFiles("/work/ramify", ["a.txt"], true, push);

    expect(push).toHaveBeenCalledWith(["a.txt"]);
    expect(titles()).toEqual([
      "Files stashed successfully",
      "Staging area in 'ramify' contains modifications to other files that were saved to the stash",
    ]);
    const warning = shown()[1];
    warning.actions?.[0].run();
    shown().forEach(({ id }) => dismiss(id));

    await stashFiles("/work/ramify", ["a.txt"], true, push);

    expect(titles()).toEqual(["Files stashed successfully"]);
  });

  test("a_failure_says_failed_to_stash_files", async () => {
    const push = vi.fn(async () => {
      throw { kind: "git", message: "fatal: bad" };
    });

    await stashFiles("/work/ramify", ["a.txt"], false, push);

    expect(titles()).toEqual(["Failed to Stash Files"]);
  });
});
