import { cleanup, fireEvent, render, screen, within } from "@testing-library/svelte";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { get } from "svelte/store";
import type {
  ChangeList,
  Changelists,
  CommitDetails,
  RebaseCommit,
  RebasePlan,
  RebaseRun,
  RepoState,
  Rewritten,
  RewriteMenu,
} from "../lib/api";
import type { CheckinRequest } from "../commit/checkin";
import { closeCommitDialog, commitDialog, requestCommit, routeCommitRequests } from "../commit/dialogStore";
import { matchLogShortcut, matchShortcut } from "../lib/keymap";
import { dismiss, notifications, type Shown } from "../lib/notifications";
import { contextMenu, type ContextHandlers, type LogSelection } from "./contextMenu";
import {
  editingFlow,
  isEditing,
  rewordFromKeyboard,
  type CommitMessageRequest,
  type DropAnswer,
  type EditingApi,
  type EditingUi,
} from "./editing";
import type { MenuAction, MenuItem } from "../lib/menu";
import CommitMessageDialog from "./CommitMessageDialog.svelte";
import DropConfirmDialog from "./DropConfirmDialog.svelte";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}), emitTo: vi.fn() }));

const H1 = "11111111".repeat(5);
const H2 = "22222222".repeat(5);
const HEAD = "abcdef01".repeat(5);
const NEW = "fedcba98".repeat(5);
// The changes of the active list, as the dialog is asked for them.
const ACTIVE: CheckinRequest = { kind: "project", paths: ["a.txt"], lists: ["default"] };
const STATE: RepoState = { operation: null, branch: "main", label: "main", head: HEAD };
const ALLOWED: RewriteMenu = {
  rebase: null,
  reword: null,
  drop: null,
  squash: null,
  uncommit: null,
  autosquash: null,
};
const LISTS: Changelists = {
  lists: [
    { id: "default", name: "Changes", comment: "" },
    { id: "other", name: "Other", comment: "" },
  ] satisfies ChangeList[],
  active: "default",
  assignments: {},
  hunkAssignments: {},
};

const details = (hash: string, subject: string, body = ""): CommitDetails => ({
  hash,
  subject,
  body,
  author: "Ana Perez",
  authorEmail: "ana@example.com",
  authorTime: 0,
  committer: "Ana Perez",
  committerEmail: "ana@example.com",
  commitTime: 0,
  samePerson: true,
  refs: [],
  parents: [],
  links: {},
});

const BASE = "0badc0de".repeat(5);
const COMMITS: RebaseCommit[] = [
  { action: "pick", hash: H1, subject: "one", full: H1, message: "one" },
  { action: "pick", hash: H2, subject: "two", full: H2, message: "two" },
];
const PLAN: RebasePlan = {
  entries: [
    { action: "drop", hash: H1, subject: "one" },
    { action: "pick", hash: H2, subject: "two" },
  ],
  messages: [],
};

const rewritten = (overrides: Partial<Rewritten> = {}): Rewritten => ({
  oldHead: HEAD,
  newHead: NEW,
  outcome: { kind: "done" },
  title: null,
  saved: [],
  ...overrides,
});

function api(overrides: Partial<EditingApi> = {}): EditingApi {
  return {
    state: vi.fn(async () => STATE),
    askBeforeDropping: vi.fn(async () => true),
    stopAskingBeforeDropping: vi.fn(async () => {}),
    check: vi.fn(async () => null),
    details: vi.fn(async (hashes: string[]) =>
      hashes.map((hash) =>
        hash === H1 ? details(H1, "one", "body of one") : details(H2, "two"),
      ),
    ),
    reword: vi.fn(async () => rewritten()),
    drop: vi.fn(async () => rewritten()),
    squash: vi.fn(async () => rewritten()),
    undo: vi.fn(async () => ({ kind: "done" }) as const),
    undoImpossible: vi.fn(async () => null),
    validate: vi.fn(async () => null),
    checkSingle: vi.fn(async () => null),
    uncommit: vi.fn(async () => ["a.txt"]),
    autosquash: vi.fn(async (): Promise<RebaseRun> => ({ outcome: { kind: "done" }, title: null, saved: [] })),
    stagingArea: vi.fn(async () => false),
    changelists: vi.fn(async () => LISTS),
    moveToChangelist: vi.fn(async () => {}),
    activeChanges: vi.fn(async (): Promise<CheckinRequest | null> => ACTIVE),
    interactiveStart: vi.fn(async () => ({ base: BASE, commits: COMMITS })),
    interactive: vi.fn(async (): Promise<RebaseRun> => ({ outcome: { kind: "done" }, title: null, saved: [] })),
    ...overrides,
  };
}

function ui(answer: (request: CommitMessageRequest) => Promise<string | null> = async () => "new") {
  return {
    confirmDrop: vi.fn(async (_count: number): Promise<DropAnswer> => ({ ok: true, doNotAsk: false })),
    editMessage: vi.fn(answer),
    chooseChangelist: vi.fn(async (_lists: ChangeList[], initial: string) => ({
      name: initial,
      setActive: false,
    })),
    showCommit: vi.fn(),
    interactiveRebase: vi.fn(async (): Promise<RebasePlan | null> => PLAN),
    rebaseStopped: vi.fn(),
    rebaseConflicts: vi.fn(async () => {}),
    restored: vi.fn(),
  } satisfies EditingUi;
}

const shown = () => get(notifications);
const last = (): Shown => shown()[shown().length - 1];
const commit = (hash: string, subject: string) => ({ hash, subject });

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  shown().forEach(({ id }) => dismiss(id));
  vi.useRealTimers();
});

const keydown = (key: string, shiftKey = false) => new KeyboardEvent("keydown", { key, shiftKey });

test("f2_in_the_log_opens_edit_commit_message_and_in_commit_is_still_rename", async () => {
  expect(matchLogShortcut(keydown("F2"))).toBe("Git.Reword.Commit");
  expect(matchLogShortcut(keydown("F6", true))).toBe("Git.Reword.Commit");
  // Outside the log (the Commit view), F2 and ⇧F6 are still Rename.
  expect(matchShortcut(keydown("F2"))).toBe("ChangesView.Rename");
  expect(matchShortcut(keydown("F6", true))).toBe("ChangesView.Rename");
  expect(matchLogShortcut(keydown("F3"))).toBeNull();

  const vcs = vi.fn();
  const menu = vi.fn(async () => ALLOWED);
  expect(await rewordFromKeyboard([{ ...commit(H1, "one"), refs: [] }], menu, vcs)).toBe(true);
  expect(vcs).toHaveBeenCalledWith({ kind: "reword", commit: commit(H1, "one") });
  // Like the turned-off action: with another selection or a reason, F2 does nothing.
  vcs.mockClear();
  const two = [{ ...commit(H1, "one"), refs: [] }, { ...commit(H2, "two"), refs: [] }];
  expect(await rewordFromKeyboard(two, menu, vcs)).toBe(false);
  const refused = vi.fn(async () => ({ ...ALLOWED, reword: "Cannot reword during merge" }));
  expect(await rewordFromKeyboard([{ ...commit(H1, "one"), refs: [] }], refused, vcs)).toBe(false);
  expect(vcs).not.toHaveBeenCalled();
});

describe("Edit Commit Message", () => {
  test("reword_dialog_with_full_message", async () => {
    const backend = api();
    const dialogs = ui();
    const flow = editingFlow(dialogs, backend);

    await flow.run({ kind: "reword", commit: commit(H1, "one") });

    expect(backend.check).toHaveBeenCalledWith([H1], "reword");
    expect(dialogs.editMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        kind: "commitMessage",
        title: "Edit Commit Message",
        label: "Edit message for commit 11111111 by Ana Perez",
        message: "one\n\nbody of one",
      }),
    );
    expect(backend.reword).toHaveBeenCalledWith(H1, "new", HEAD);
  });

  test("cancelling_the_dialog_does_not_rewrite_and_moved_head_is_reported_by_the_dialog", async () => {
    const backend = api();
    const cancelled = editingFlow(ui(async () => null), backend);
    await cancelled.run({ kind: "reword", commit: commit(H1, "one") });
    expect(backend.reword).not.toHaveBeenCalled();

    let validation: string | null = null;
    const moved = api({
      validate: vi.fn(async () => "Cannot edit selected commit: repository state was changed"),
    });
    const flow = editingFlow(
      ui(async (request) => {
        validation = await request.validate();
        return null;
      }),
      moved,
    );
    await flow.run({ kind: "reword", commit: commit(H1, "one") });
    // The core checks it, with the selection's commits and the HEAD the dialog saw.
    expect(moved.validate).toHaveBeenCalledWith([H1], HEAD);
    expect(validation).toBe("Cannot edit selected commit: repository state was changed");
  });

  test("check_failing_on_click_warns_without_opening_the_dialog", async () => {
    const backend = api({ check: vi.fn(async () => "The commit is not in the current branch") });
    const dialogs = ui();
    const flow = editingFlow(dialogs, backend);

    await flow.run({ kind: "reword", commit: commit(H1, "one") });

    expect(dialogs.editMessage).not.toHaveBeenCalled();
    expect(last()).toMatchObject({
      kind: "error",
      title: "Could not Edit Commit Message",
      body: "The commit is not in the current branch",
    });
  });
});

test("squash_joins_the_messages_with_two_blank_lines", async () => {
  const backend = api();
  const dialogs = ui(async () => "joined");
  const flow = editingFlow(dialogs, backend);

  await flow.run({ kind: "squash", commits: [commit(H2, "two"), commit(H1, "one")] });

  expect(dialogs.editMessage).toHaveBeenCalledWith(
    expect.objectContaining({
      title: "Squashed Commit Message",
      label: "Edit message for squashed commit",
      message: "two\n\n\none\n\nbody of one",
    }),
  );
  expect(backend.squash).toHaveBeenCalledWith([H2, H1], "joined", HEAD);
  expect(last()).toMatchObject({ kind: "info", title: "Commits Squashed" });
});

test("drop_asks_drop_commits_with_ok_and_cancel", async () => {
  const backend = api();
  const dialogs = ui();
  const flow = editingFlow(dialogs, backend);

  const H3 = "33333333".repeat(5);
  await flow.run({ kind: "drop", commits: [commit(H3, "three"), commit(H2, "two"), commit(H1, "one")] });

  expect(dialogs.editMessage).not.toHaveBeenCalled();
  expect(dialogs.confirmDrop).toHaveBeenCalledWith(3);
  expect(backend.drop).toHaveBeenCalledWith([H3, H2, H1], HEAD);
  expect(last()).toMatchObject({ kind: "info", title: "Dropped 3 commits" });
  await flow.run({ kind: "drop", commits: [commit(H1, "one")] });
  expect(last()).toMatchObject({ kind: "info", title: "Dropped 1 commit" });
});

describe("the Drop Commits question dialog", () => {
  afterEach(cleanup);

  test("is a warning with the title, message, OK as default, Cancel and an unchecked Don't ask again", async () => {
    const answer = vi.fn();
    render(DropConfirmDialog, { count: 3, onanswer: answer });

    expect(screen.getByText("Drop Commits")).toBeTruthy();
    expect(screen.getByText("Do you want to drop 3 commits?")).toBeTruthy();
    expect((screen.getByRole("checkbox", { name: "Don't ask again" }) as HTMLInputElement).checked).toBe(false);
    expect(screen.getByRole("button", { name: "OK" }).classList.contains("default")).toBe(true);
    expect(screen.getByRole("button", { name: "Cancel" })).toBeTruthy();
    expect(document.querySelector("svg")).not.toBeNull();

    await fireEvent.click(screen.getByRole("checkbox", { name: "Don't ask again" }));
    await fireEvent.click(screen.getByRole("button", { name: "OK" }));
    expect(answer).toHaveBeenCalledWith({ ok: true, doNotAsk: true });
  });

  test("Cancel answers no even with the box checked, and one commit is singular", async () => {
    const answer = vi.fn();
    render(DropConfirmDialog, { count: 1, onanswer: answer });

    expect(screen.getByText("Drop Commit")).toBeTruthy();
    expect(screen.getByText("Do you want to drop 1 commit?")).toBeTruthy();
    await fireEvent.click(screen.getByRole("checkbox", { name: "Don't ask again" }));
    await fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(answer).toHaveBeenCalledWith({ ok: false, doNotAsk: false });
  });
});

test("cancelling_the_drop_question_runs_nothing", async () => {
  const backend = api();
  const dialogs = ui();
  dialogs.confirmDrop.mockResolvedValue({ ok: false, doNotAsk: true });

  await editingFlow(dialogs, backend).run({ kind: "drop", commits: [commit(H1, "one")] });

  expect(backend.state).not.toHaveBeenCalled();
  expect(backend.drop).not.toHaveBeenCalled();
  expect(backend.stopAskingBeforeDropping).not.toHaveBeenCalled();
});

test("drop_asks_after_the_check_and_before_reading_the_state", async () => {
  const order: string[] = [];
  const backend = api({
    check: vi.fn(async () => {
      order.push("check");
      return null;
    }),
    state: vi.fn(async () => {
      order.push("state");
      return STATE;
    }),
  });
  const dialogs = ui();
  dialogs.confirmDrop.mockImplementation(async () => {
    order.push("ask");
    return { ok: true, doNotAsk: false };
  });

  await editingFlow(dialogs, backend).run({ kind: "drop", commits: [commit(H1, "one")] });

  expect(order).toEqual(["check", "ask", "state"]);
});

test("a_refused_check_does_not_ask", async () => {
  const backend = api({ check: vi.fn(async () => "Cannot drop during merge") });
  const dialogs = ui();

  await editingFlow(dialogs, backend).run({ kind: "drop", commits: [commit(H1, "one")] });

  expect(dialogs.confirmDrop).not.toHaveBeenCalled();
  expect(last()).toMatchObject({ kind: "error", title: "Could not Drop Commits" });
});

test("dont_ask_again_is_saved_only_on_ok", async () => {
  const checkedOk = api();
  const dialogs = ui();
  dialogs.confirmDrop.mockResolvedValue({ ok: true, doNotAsk: true });
  await editingFlow(dialogs, checkedOk).run({ kind: "drop", commits: [commit(H1, "one")] });
  expect(checkedOk.stopAskingBeforeDropping).toHaveBeenCalledTimes(1);
  expect(checkedOk.drop).toHaveBeenCalled();

  const checkedCancel = api();
  const cancelling = ui();
  cancelling.confirmDrop.mockResolvedValue({ ok: false, doNotAsk: true });
  await editingFlow(cancelling, checkedCancel).run({ kind: "drop", commits: [commit(H1, "one")] });
  expect(checkedCancel.stopAskingBeforeDropping).not.toHaveBeenCalled();
  expect(checkedCancel.drop).not.toHaveBeenCalled();

  const unchecked = api();
  await editingFlow(ui(), unchecked).run({ kind: "drop", commits: [commit(H1, "one")] });
  expect(unchecked.stopAskingBeforeDropping).not.toHaveBeenCalled();
});

test("without_the_setting_drop_does_not_ask", async () => {
  const backend = api({ askBeforeDropping: vi.fn(async () => false) });
  const dialogs = ui();

  await editingFlow(dialogs, backend).run({ kind: "drop", commits: [commit(H1, "one")] });

  expect(dialogs.confirmDrop).not.toHaveBeenCalled();
  expect(backend.drop).toHaveBeenCalledWith([H1], HEAD);
});

test("drop_success_lists_the_subjects", async () => {
  const hash = (n: number) => String(n).padStart(2, "0").repeat(20);
  const many = Array.from({ length: 12 }, (_, n) => commit(hash(n), `subject ${n}`));
  await editingFlow(ui(), api()).run({ kind: "drop", commits: many });
  expect(last()).toMatchObject({
    title: "Dropped 12 commits",
    body: [...many.slice(0, 10).map(({ subject }) => `"${subject}"`), "..."].join("\n"),
  });

  const long = "x".repeat(45);
  const exact = "y".repeat(40);
  await editingFlow(ui(), api()).run({
    kind: "drop",
    commits: [commit(H1, long), commit(H2, exact)],
  });
  expect(last()).toMatchObject({
    title: "Dropped 2 commits",
    body: `"${"x".repeat(37)}..."\n"${exact}"`,
  });
});

describe("Undo", () => {
  test("success_commit_message_changed_with_undo", async () => {
    const backend = api();
    const flow = editingFlow(ui(), backend);

    await flow.run({ kind: "reword", commit: commit(H1, "one") });

    const card = last();
    expect(card).toMatchObject({ kind: "info", title: "Commit Message Changed" });
    expect(card.actions?.map(({ label }) => label)).toEqual(["Undo"]);
    card.actions?.[0].run();
    await vi.waitFor(() => expect(backend.undo).toHaveBeenCalledWith("reword", rewritten()));
    // The card goes away on click, and an Undo that goes well gives no further notice.
    expect(shown().some(({ id }) => id === card.id)).toBe(false);
    expect(shown()).toHaveLength(0);
  });

  test("impossible_or_failed_undo_says_so_with_its_title", async () => {
    const impossible = api({
      undo: vi.fn(async () => ({ kind: "impossible", reason: "Repository has already been changed" }) as const),
    });
    const flow = editingFlow(ui(), impossible);
    await flow.run({ kind: "drop", commits: [commit(H1, "one")] });
    last().actions?.[0].run();
    await vi.waitFor(() =>
      expect(last()).toMatchObject({
        kind: "error",
        title: "Cannot undo drop",
        body: "Repository has already been changed",
      }),
    );

    const droppingFails = api({ undo: vi.fn(async () => ({ kind: "failed", reason: "error: no" }) as const) });
    await editingFlow(ui(), droppingFails).run({ kind: "drop", commits: [commit(H1, "one")] });
    last().actions?.[0].run();
    await vi.waitFor(() =>
      expect(last()).toMatchObject({ kind: "error", title: "Undo drop failed", body: "error: no" }),
    );

    const failing = api({ undo: vi.fn(async () => ({ kind: "failed", reason: "error: no" }) as const) });
    const other = editingFlow(ui(), failing);
    await other.run({ kind: "squash", commits: [commit(H2, "two"), commit(H1, "one")] });
    last().actions?.[0].run();
    await vi.waitFor(() =>
      expect(last()).toMatchObject({ kind: "error", title: "Undo Squash Failed", body: "error: no" }),
    );
  });

  test("undo_does_not_expire_by_time", async () => {
    const backend = api();
    const flow = editingFlow(ui(), backend);
    await flow.run({ kind: "drop", commits: [commit(H1, "one")] });
    const card = last();

    vi.advanceTimersByTime(60_000);

    const still = shown().find(({ id }) => id === card.id);
    expect(still?.sticky).toBe(true);
    expect(still?.actions?.map(({ label }) => label)).toEqual(["Undo"]);
    // And it keeps undoing afterwards.
    still?.actions?.[0].run();
    await vi.waitFor(() => expect(backend.undo).toHaveBeenCalledWith("drop", rewritten()));
  });

  test("undo_expires_if_the_repository_changes", async () => {
    const backend = api();
    const flow = editingFlow(ui(), backend);
    await flow.run({ kind: "reword", commit: commit(H1, "one") });
    const card = last();

    // A change that does not prevent it leaves it.
    await flow.repositoryRefreshed();
    expect(shown().some(({ id }) => id === card.id)).toBe(true);
    expect(backend.undoImpossible).toHaveBeenCalledWith(rewritten());

    // Time does not take it away either: only a change that moves HEAD from `newHead`.
    vi.advanceTimersByTime(60_000);
    await flow.repositoryRefreshed();
    expect(shown().some(({ id }) => id === card.id)).toBe(true);

    vi.mocked(backend.undoImpossible).mockResolvedValue("Repository has already been changed");
    await flow.repositoryRefreshed();
    expect(shown().some(({ id }) => id === card.id)).toBe(false);
    // Once expired, another change does not ask about it again.
    vi.mocked(backend.undoImpossible).mockClear();
    await flow.repositoryRefreshed();
    expect(backend.undoImpossible).not.toHaveBeenCalled();

    // Opening another repository also takes it away.
    await flow.run({ kind: "reword", commit: commit(H1, "one") });
    const next = last();
    flow.repositoryChanged();
    expect(shown().some(({ id }) => id === next.id)).toBe(false);
  });
});

describe("Undo Commit", () => {
  const uncommit = (hash = H1, subject = "one") => ({ kind: "uncommit", commit: commit(hash, subject) }) as const;

  test("undo_commit_asks_select_target_changelist_with_the_subject", async () => {
    const backend = api({ state: vi.fn(async () => ({ ...STATE, head: H1 })) });
    const dialogs = ui(async () => null);
    dialogs.chooseChangelist.mockResolvedValue({ name: "Other", setActive: true });
    const flow = editingFlow(dialogs, backend);

    await flow.run(uncommit());

    // The chooser starts from the commit's subject, with the current lists.
    expect(dialogs.chooseChangelist).toHaveBeenCalledWith(LISTS.lists, "one");
    expect(backend.checkSingle).toHaveBeenCalledWith(H1, "uncommit");
    // The reset goes with the HEAD seen before the chooser, and the paths move to the chosen list.
    expect(backend.uncommit).toHaveBeenCalledWith(H1, H1);
    expect(backend.moveToChangelist).toHaveBeenCalledWith(["a.txt"], "Other", true);
  });

  test("cancelling_the_chooser_does_not_undo", async () => {
    const backend = api();
    const dialogs = ui();
    dialogs.chooseChangelist.mockResolvedValue(null as never);
    const flow = editingFlow(dialogs, backend);

    await flow.run(uncommit());

    expect(backend.uncommit).not.toHaveBeenCalled();
    expect(backend.moveToChangelist).not.toHaveBeenCalled();
    expect(shown()).toHaveLength(0);
  });

  test("with_staging_area_there_are_no_changelists_or_chooser", async () => {
    const backend = api({ stagingArea: vi.fn(async () => true) });
    const dialogs = ui();
    const flow = editingFlow(dialogs, backend);

    await flow.run(uncommit());

    expect(dialogs.chooseChangelist).not.toHaveBeenCalled();
    expect(backend.uncommit).toHaveBeenCalledWith(H1, HEAD);
    expect(backend.moveToChangelist).not.toHaveBeenCalled();
    expect(last()).toMatchObject({ kind: "info", title: "Undo commit successful" });
  });

  test("undo_commit_successful_and_failed", async () => {
    const flow = editingFlow(ui(), api());
    await flow.run(uncommit());
    // Without Undo or anything else: the reset notice.
    expect(last()).toMatchObject({ kind: "info", title: "Undo commit successful" });
    expect(last().actions).toBeUndefined();

    shown().forEach(({ id }) => dismiss(id));
    const failing = api({
      uncommit: vi.fn(async () => {
        throw { kind: "git", message: "fatal: could not" };
      }),
    });
    await editingFlow(ui(), failing).run(uncommit());
    expect(last()).toMatchObject({ kind: "error", title: "Undo commit failed" });
    expect(last().body).toContain("fatal: could not");
    expect(failing.moveToChangelist).not.toHaveBeenCalled();
    expect(shown().some(({ title }) => title === "Undo commit successful")).toBe(false);
  });

  test("undo_commit_on_a_protected_branch_is_rejected", async () => {
    const pushed = "The commit is already pushed to protected branch 'origin/main'";
    const backend = api({ checkSingle: vi.fn(async () => pushed) });
    const dialogs = ui();
    const flow = editingFlow(dialogs, backend);

    await flow.run(uncommit());

    // Like `Messages.showErrorDialog` with the action's title, before the chooser.
    expect(last()).toMatchObject({ kind: "error", title: "Cannot Undo Commit", body: pushed });
    expect(dialogs.chooseChangelist).not.toHaveBeenCalled();
    expect(backend.uncommit).not.toHaveBeenCalled();
  });

  test("if_the_changes_cannot_be_moved_it_says_so_and_the_reset_is_already_done", async () => {
    const backend = api({
      moveToChangelist: vi.fn(async () => {
        throw { kind: "git", message: "no list" };
      }),
    });
    await editingFlow(ui(), backend).run(uncommit());

    expect(backend.uncommit).toHaveBeenCalled();
    expect(shown().some(({ title }) => title === "Undo commit successful")).toBe(true);
    expect(last()).toMatchObject({ kind: "error" });
  });
});

describe("Fixup… and Squash Into…", () => {
  const fixup = { kind: "fixup", commit: commit(H1, "one") } as const;
  const squashInto = { kind: "squashInto", commit: commit(H1, "one") } as const;

  test("Fixup opens the commit dialog with fixup! subject and Commit and Rebase…", async () => {
    const backend = api();
    const dialogs = ui();
    const flow = editingFlow(dialogs, backend);

    await flow.run(fixup);

    expect(backend.checkSingle).toHaveBeenCalledWith(H1, "autosquash");
    expect(dialogs.showCommit).toHaveBeenCalledWith({
      request: ACTIVE,
      prefilled: "fixup! one",
      rebaseFrom: `${H1}~`,
      head: HEAD,
    });
    await flow.run(squashInto);
    expect(dialogs.showCommit).toHaveBeenLastCalledWith({
      request: ACTIVE,
      prefilled: "squash! one",
      rebaseFrom: `${H1}~`,
      head: HEAD,
    });
    // It does not touch the repository: the commit and the rebase are launched by the dialog.
    expect(backend.autosquash).not.toHaveBeenCalled();
  });

  test("through the routing in modal mode Fixup puts the active list in the Commit Changes dialog", async () => {
    routeCommitRequests({ mode: () => "modal", nonModal: vi.fn() });
    try {
      const dialogs = { ...ui(), showCommit: vi.fn(requestCommit) };

      await editingFlow(dialogs, api()).run(fixup);

      expect(get(commitDialog)).toEqual({
        request: ACTIVE,
        executor: "commit",
        prefilled: "fixup! one",
        rebaseFrom: `${H1}~`,
        head: HEAD,
      });
    } finally {
      closeCommitDialog();
      routeCommitRequests(null);
    }
  });

  test("with_an_operation_in_progress_commit_and_rebase_is_not_offered", async () => {
    const dialogs = ui();
    const backend = api({ state: vi.fn(async () => ({ ...STATE, operation: "merging" as const })) });

    await editingFlow(dialogs, backend).run(fixup);

    // The HEAD travels with the base only: without an executor there is nothing to check it against.
    expect(dialogs.showCommit).toHaveBeenCalledWith({ request: ACTIVE, prefilled: "fixup! one" });
    // With a detached HEAD there is no operation: it is offered.
    const detached = api({ state: vi.fn(async () => ({ ...STATE, branch: null })) });
    await editingFlow(dialogs, detached).run(fixup);
    expect(dialogs.showCommit).toHaveBeenLastCalledWith({
      request: ACTIVE,
      prefilled: "fixup! one",
      rebaseFrom: `${H1}~`,
      head: HEAD,
    });
  });

  test("without changes in the active list Fixup says Nothing to Commit", async () => {
    const backend = api({ activeChanges: vi.fn(async () => null) });
    const dialogs = ui();

    await editingFlow(dialogs, backend).run(fixup);

    expect(dialogs.showCommit).not.toHaveBeenCalled();
    expect(last()).toMatchObject({
      kind: "info",
      title: "Nothing to Commit",
      body: "No changes detected",
    });
  });

  test("a_failing_check_says_cannot_create_fixup_or_squash_commit", async () => {
    const reason = "Selected commit has 2 parents";
    const backend = api({ checkSingle: vi.fn(async () => reason) });
    const dialogs = ui();
    const flow = editingFlow(dialogs, backend);

    await flow.run(fixup);
    expect(last()).toMatchObject({ kind: "error", title: "Cannot Create Fixup Commit", body: reason });
    await flow.run(squashInto);
    expect(last()).toMatchObject({ kind: "error", title: "Cannot Create Squash Commit", body: reason });
    expect(dialogs.showCommit).not.toHaveBeenCalled();
    expect(backend.activeChanges).not.toHaveBeenCalled();
  });

  test("the_rebase_after_the_commit_warns_rebase_successful", async () => {
    const backend = api();
    const flow = editingFlow(ui(), backend);

    await flow.rebaseAfterCommit(`${H1}~`);

    expect(backend.autosquash).toHaveBeenCalledWith(`${H1}~`);
    expect(last()).toMatchObject({
      kind: "info",
      title: "Rebase successful",
      body: `Rebased main on ${H1}~`,
    });
  });

  test("a_rebase_that_stops_or_does_not_start_is_reported_like_the_others", async () => {
    const conflicts: RebaseRun = { outcome: { kind: "conflicts" }, title: null, saved: [] };
    const dialogs = ui();
    await editingFlow(dialogs, api({ autosquash: vi.fn(async () => conflicts) })).rebaseAfterCommit("x~");
    expect(dialogs.rebaseConflicts).toHaveBeenCalledWith([]);
    expect(shown().some(({ title }) => title === "Rebase successful")).toBe(false);

    const refused: RebaseRun = {
      outcome: { kind: "failed", reason: "You are in the detached HEAD state in r.\nRebase is not possible." },
      title: "Rebase not allowed",
      saved: [],
    };
    await editingFlow(ui(), api({ autosquash: vi.fn(async () => refused) })).rebaseAfterCommit("x~");
    expect(last()).toMatchObject({ kind: "error", title: "Rebase not allowed" });

    const broken = api({
      autosquash: vi.fn(async () => {
        throw { kind: "git", message: "fatal: mal" };
      }),
    });
    await editingFlow(ui(), broken).rebaseAfterCommit("x~");
    expect(last()).toMatchObject({ kind: "error", title: "Rebase failed" });
  });
});

test("a_rebase_that_stops_continues_like_task_2_without_undo", async () => {
  const stopped = rewritten({ outcome: { kind: "conflicts" } });
  const backend = api({ drop: vi.fn(async () => stopped) });
  const dialogs = ui();
  const flow = editingFlow(dialogs, backend);

  await flow.run({ kind: "drop", commits: [commit(H1, "one")] });

  expect(dialogs.rebaseConflicts).toHaveBeenCalledWith([]);
  expect(shown().some(({ title }) => title === "Dropped 1 commit")).toBe(false);
  // One that does not get to start says so with its title.
  const refused = api({
    drop: vi.fn(async () =>
      rewritten({ title: "Rebase not allowed", outcome: { kind: "failed", reason: "There is an unfinished merge process in r." } }),
    ),
  });
  await editingFlow(ui(), refused).run({ kind: "drop", commits: [commit(H1, "one")] });
  expect(last()).toMatchObject({ kind: "error", title: "Rebase not allowed" });
});

describe("Interactively Rebase from Here…", () => {
  const action = { kind: "interactiveRebase", commit: commit(H1, "one") } as const;

  test("opens_rebasing_commits_and_launches_the_chosen_onto_the_parent", async () => {
    const backend = api();
    const dialogs = ui();

    await editingFlow(dialogs, backend).run(action);

    expect(backend.checkSingle).toHaveBeenCalledWith(H1, "rebase");
    expect(backend.interactiveStart).toHaveBeenCalledWith(H1);
    expect(dialogs.interactiveRebase).toHaveBeenCalledWith(COMMITS);
    expect(backend.interactive).toHaveBeenCalledWith(H1, PLAN, HEAD);
    expect(last()).toMatchObject({ title: "Rebase successful", body: `Rebased main on ${BASE}` });
    expect(isEditing(action)).toBe(true);
  });

  test("cancelling_the_dialog_does_not_launch_git", async () => {
    const backend = api();
    const dialogs = ui();
    dialogs.interactiveRebase.mockResolvedValue(null);

    await editingFlow(dialogs, backend).run(action);

    expect(backend.interactive).not.toHaveBeenCalled();
  });

  test("without_a_log_list_git_generates_it_and_the_dialog_appears_afterwards", async () => {
    const backend = api({ interactiveStart: vi.fn(async () => ({ base: BASE, commits: null })) });
    const dialogs = ui();

    await editingFlow(dialogs, backend).run(action);

    expect(dialogs.interactiveRebase).not.toHaveBeenCalled();
    expect(backend.interactive).toHaveBeenCalledWith(H1, null, HEAD);
  });

  test("a_todo_that_does_not_match_offers_retry_with_gits", async () => {
    const invalid =
      "Cannot perform the rebase because an invalid 'git-rebase-todo' file was generated. Do you want to retry the rebase using Git native file generation?";
    const backend = api({
      interactive: vi
        .fn()
        .mockRejectedValueOnce({ kind: "git_error", message: invalid })
        .mockResolvedValueOnce({ outcome: { kind: "done" }, title: null, saved: [] }),
    });

    await editingFlow(ui(), backend).run(action);

    const card = last();
    expect(card).toMatchObject({ kind: "error", title: "Rebase failed", body: invalid });
    expect(card.actions?.map(({ label }) => label)).toEqual(["Retry"]);
    card.actions?.[0].run();
    await vi.waitFor(() => expect(backend.interactive).toHaveBeenLastCalledWith(H1, null, HEAD));
  });

  test("edit_stops_and_notifies_rebase_stopped_for_editing", async () => {
    const backend = api({
      interactive: vi.fn(async (): Promise<RebaseRun> => ({ outcome: { kind: "editing" }, title: null, saved: [] })),
    });
    const dialogs = ui();

    await editingFlow(dialogs, backend).run(action);

    // The `Rebase stopped for editing` card with Continue and Abort is that of `src/operation`.
    expect(dialogs.rebaseStopped).toHaveBeenCalledWith({ kind: "editing" }, []);
    expect(shown().some(({ title }) => title === "Rebase successful")).toBe(false);
  });

  test("what_cannot_be_done_says_cannot_start_rebase", async () => {
    const backend = api({ checkSingle: vi.fn(async () => "Cannot rebase during rebase") });

    await editingFlow(ui(), backend).run(action);

    expect(last()).toMatchObject({ kind: "error", title: "Cannot Start Rebase", body: "Cannot rebase during rebase" });
    expect(backend.interactiveStart).not.toHaveBeenCalled();
  });

  test("in_the_menu_after_squash_commits_only_with_one_commit", () => {
    const one: LogSelection = { commits: [{ ...commit(H1, "one"), refs: [] }], parents: [], children: [] };
    const vcs = vi.fn();
    const handlers = (editing: RewriteMenu): ContextHandlers => ({
      copy: vi.fn(),
      goToChild: vi.fn(),
      goToParent: vi.fn(),
      vcs,
      editing,
    });
    const items = contextMenu(one, { currentBranch: "main" }, handlers(ALLOWED));
    const actions = items.filter((item): item is MenuAction => item !== null && "label" in item);
    const labels = actions.map(({ label }) => label);
    expect(labels.indexOf("Interactively Rebase from Here…")).toBe(labels.indexOf("Drop Commit") + 1);
    actions.find(({ label }) => label === "Interactively Rebase from Here…")?.run?.();
    expect(vcs).toHaveBeenCalledWith(action);

    const refused = contextMenu(one, { currentBranch: "main" }, handlers({ ...ALLOWED, rebase: "Cannot rebase during merge" }));
    expect(
      refused.find((item) => item !== null && "label" in item && item.label === "Interactively Rebase from Here…"),
    ).toMatchObject({ disabled: true, title: "Cannot rebase during merge" });

    const two: LogSelection = {
      commits: [{ ...commit(H2, "two"), refs: [] }, { ...commit(H1, "one"), refs: [] }],
      parents: [],
      children: [],
    };
    const several = contextMenu(two, { currentBranch: "main" }, handlers(ALLOWED));
    expect(several.some((item) => item !== null && "label" in item && item.label === "Interactively Rebase from Here…")).toBe(false);
  });
});

describe("menu", () => {
  const isAction = (item: MenuItem): item is MenuAction => item !== null && "label" in item;
  const labels = (items: MenuItem[]) => items.filter(isAction).map(({ label }) => label);
  const find = (items: MenuItem[], label: string) => {
    const found = items.filter(isAction).find((item) => item.label === label);
    if (!found) throw new Error(`${label} not found: ${labels(items)}`);
    return found;
  };
  const one: LogSelection = { commits: [{ ...commit(H1, "one"), refs: [] }], parents: [], children: [] };
  const two: LogSelection = {
    commits: [{ ...commit(H2, "two"), refs: [] }, { ...commit(H1, "one"), refs: [] }],
    parents: [],
    children: [],
  };
  const handlers = (editing?: RewriteMenu): ContextHandlers => ({
    copy: vi.fn(),
    goToChild: vi.fn(),
    goToParent: vi.fn(),
    vcs: vi.fn(),
    editing,
  });

  test("disabled_entries_say_why", () => {
    const reason = "The commit is already pushed to protected branch 'origin/main'";
    const single = contextMenu(one, { currentBranch: "main" }, handlers({ ...ALLOWED, reword: "Cannot reword during merge", drop: reason }));

    expect(find(single, "Edit Commit Message…")).toMatchObject({
      disabled: true,
      title: "Cannot reword during merge",
      shortcut: "F2",
    });
    // With a reason, Drop keeps the template text, and Squash is seen turned off.
    expect(find(single, "Drop Commits")).toMatchObject({ disabled: true, title: reason });
    expect(find(single, "Squash Commits…")).toMatchObject({ disabled: true, title: reason });

    const several = contextMenu(two, { currentBranch: "main" }, handlers({ ...ALLOWED, drop: reason, squash: reason }));
    expect(labels(several)).not.toContain("Edit Commit Message…");
    expect(find(several, "Squash Commits…")).toMatchObject({ disabled: true, title: reason });
  });

  test("undo_commit_fixup_and_squash_into_only_with_one_commit_and_turned_off_with_their_reason", () => {
    const refused = "The selected commit is not the last in the current branch";
    const single = contextMenu(one, { currentBranch: "main" }, handlers({ ...ALLOWED, uncommit: refused, autosquash: null }));
    // Between Revert Commit and the root of editing, like `Git.Log.ContextMenu`.
    const order = labels(single);
    expect(order.indexOf("Undo Commit…")).toBe(order.indexOf("Revert Commit") + 1);
    expect(order.indexOf("Fixup…")).toBe(order.indexOf("Edit Commit Message…") + 1);
    expect(order.indexOf("Squash Into…")).toBe(order.indexOf("Fixup…") + 1);
    expect(find(single, "Undo Commit…")).toMatchObject({ disabled: true, title: refused });
    expect(find(single, "Fixup…")).toMatchObject({ disabled: false });
    expect(find(single, "Squash Into…").title).toBeUndefined();

    const vcs = vi.fn();
    const on = contextMenu(one, { currentBranch: "main" }, { ...handlers(ALLOWED), vcs });
    find(on, "Undo Commit…").run?.();
    find(on, "Fixup…").run?.();
    find(on, "Squash Into…").run?.();
    expect(vcs.mock.calls.map(([action]) => action.kind)).toEqual(["uncommit", "fixup", "squashInto"]);
    expect(vcs).toHaveBeenCalledWith({ kind: "fixup", commit: commit(H1, "one") });

    const several = labels(contextMenu(two, { currentBranch: "main" }, handlers(ALLOWED)));
    for (const hidden of ["Undo Commit…", "Fixup…", "Squash Into…"]) {
      expect(several).not.toContain(hidden);
    }
    expect(isEditing({ kind: "uncommit", commit: commit(H1, "one") })).toBe(true);
    expect(isEditing({ kind: "fixup", commit: commit(H1, "one") })).toBe(true);
  });

  test("enabled_entries_launch_their_action", () => {
    const single = contextMenu(one, { currentBranch: "main" }, handlers(ALLOWED));
    expect(find(single, "Drop Commit").disabled).toBe(false);
    expect(find(single, "Edit Commit Message…").title).toBeUndefined();
    // Squash with a single commit is not seen.
    expect(labels(single)).not.toContain("Squash Commits…");

    const vcs = vi.fn();
    const several = contextMenu(two, { currentBranch: "main" }, { ...handlers(ALLOWED), vcs });
    find(several, "Squash Commits…").run?.();
    expect(vcs).toHaveBeenCalledWith({
      kind: "squash",
      commits: [commit(H2, "two"), commit(H1, "one")],
    });
    expect(isEditing({ kind: "squash", commits: [] })).toBe(true);
    expect(isEditing({ kind: "revert", commits: [] })).toBe(false);
  });
});

describe("CommitMessageDialog", () => {
  let history: string[];
  beforeEach(() => {
    history = [];
    vi.mocked(invoke).mockReset();
    vi.mocked(invoke).mockImplementation(async (command) => (command === "messages_list" ? history : null));
  });
  afterEach(() => cleanup());

  const request = (validate: () => Promise<string | null>): CommitMessageRequest => ({
    kind: "commitMessage",
    title: "Edit Commit Message",
    label: "Edit message for commit 11111111 by Ana Perez",
    message: "one\n\nbody",
    validate,
  });

  test("dialog_rejects_a_commit_on_a_protected_branch_without_losing_the_message", async () => {
    vi.useRealTimers();
    const pushed = `Cannot edit selected commits: commit ${H1} was pushed to protected branch 'origin/main'`;
    const backend = api({ validate: vi.fn(async () => pushed) });
    let asked: CommitMessageRequest | null = null;
    const flow = editingFlow(
      ui(async (request) => {
        asked = request;
        return null;
      }),
      backend,
    );
    await flow.run({ kind: "squash", commits: [commit(H2, "two"), commit(H1, "one")] });
    expect(asked).not.toBeNull();

    const onanswer = vi.fn();
    render(CommitMessageDialog, { request: asked!, onanswer });
    const field = screen.getByLabelText("Commit Message") as HTMLTextAreaElement;
    await fireEvent.input(field, { target: { value: "what I typed" } });
    await fireEvent.click(screen.getByRole("button", { name: "OK" }));

    expect(await screen.findByText(pushed)).toBeTruthy();
    expect(backend.validate).toHaveBeenCalledWith([H2, H1], HEAD);
    // The dialog stays open with what was typed, and nothing is rewritten.
    expect(onanswer).not.toHaveBeenCalled();
    expect(field.value).toBe("what I typed");
    expect(screen.getByRole("dialog", { name: "Squashed Commit Message" })).toBeTruthy();
    expect(backend.squash).not.toHaveBeenCalled();
  });

  test("the_dialog_is_the_commit_editor_without_amend_and_validates_on_accept", async () => {
    vi.useRealTimers();
    const onanswer = vi.fn();
    const validate = vi.fn(
      async (): Promise<string | null> => "Cannot edit selected commit: repository state was changed",
    );
    render(CommitMessageDialog, { request: request(validate), onanswer });

    expect(screen.getByRole("dialog", { name: "Edit Commit Message" })).toBeTruthy();
    expect(screen.getByText("Edit message for commit 11111111 by Ana Perez")).toBeTruthy();
    const field = screen.getByLabelText("Commit Message") as HTMLTextAreaElement;
    expect(field.value).toBe("one\n\nbody");
    expect(field.selectionStart).toBe(0);
    expect(screen.queryByText("Amend")).toBeNull();

    await fireEvent.click(screen.getByRole("button", { name: "OK" }));
    expect(await screen.findByText("Cannot edit selected commit: repository state was changed")).toBeTruthy();
    expect(onanswer).not.toHaveBeenCalled();

    validate.mockResolvedValue(null);
    await fireEvent.input(field, { target: { value: "new" } });
    await fireEvent.keyDown(field, { key: "Enter", metaKey: true });
    await vi.waitFor(() => expect(onanswer).toHaveBeenCalledWith("new"));
  });

  describe("history", () => {
    const pushes = () =>
      vi
        .mocked(invoke)
        .mock.calls.filter(([command]) => command === "messages_push")
        .map(([, args]) => args);
    const ok = async () => null;

    test("the_reword_dialog_has_the_history_button", async () => {
      history = ["newest", "older"];
      render(CommitMessageDialog, { request: request(ok), onanswer: vi.fn() });
      const field = screen.getByLabelText("Commit Message") as HTMLTextAreaElement;
      const history_button = screen.getByRole("button", { name: "Commit Message History" }) as HTMLButtonElement;
      expect(history_button.title).toBe("Commit Message History (⌃M)");
      await vi.waitFor(() => expect(history_button.disabled).toBe(false));

      await fireEvent.keyDown(field, { key: "m", code: "KeyM", ctrlKey: true });
      const menu = await screen.findByRole("menu", { name: "Commit Message History" });
      expect(within(menu).getAllByRole("menuitem").map((entry) => entry.textContent?.trim())).toEqual([
        "newest",
        "older",
      ]);
      await fireEvent.click(within(menu).getByRole("menuitem", { name: "older" }));
      expect(field.value).toBe("older");
    });

    test("the_history_button_is_disabled_with_an_empty_history", async () => {
      render(CommitMessageDialog, { request: request(ok), onanswer: vi.fn() });
      await vi.waitFor(() => expect(vi.mocked(invoke)).toHaveBeenCalledWith("messages_list"));
      const history_button = screen.getByRole("button", { name: "Commit Message History" }) as HTMLButtonElement;
      expect(history_button.disabled).toBe(true);
    });

    test("a_changed_message_goes_to_history_on_any_close", async () => {
      const field = () => screen.getByLabelText("Commit Message") as HTMLTextAreaElement;
      for (const close of [
        () => fireEvent.click(screen.getByRole("button", { name: "OK" })),
        () => fireEvent.click(screen.getByRole("button", { name: "Cancel" })),
        () => fireEvent.keyDown(field(), { key: "Escape" }),
      ]) {
        vi.mocked(invoke).mockClear();
        const onanswer = vi.fn();
        const view = render(CommitMessageDialog, { request: request(ok), onanswer });
        await fireEvent.input(field(), { target: { value: "new text\n\n  \n" } });
        await close();
        await vi.waitFor(() => expect(onanswer).toHaveBeenCalled());
        expect(pushes()).toEqual([{ message: "new text" }]);
        view.unmount();
      }
    });

    test("an_unchanged_message_calls_nothing_and_a_refused_ok_neither", async () => {
      const onanswer = vi.fn();
      const refuse = vi.fn(async () => "Cannot edit");
      render(CommitMessageDialog, { request: request(refuse), onanswer });
      await fireEvent.input(screen.getByLabelText("Commit Message"), { target: { value: "changed" } });
      await fireEvent.click(screen.getByRole("button", { name: "OK" }));
      expect(await screen.findByText("Cannot edit")).toBeTruthy();
      expect(pushes()).toEqual([]);
      cleanup();

      const same = vi.fn();
      render(CommitMessageDialog, { request: request(ok), onanswer: same });
      await fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
      expect(same).toHaveBeenCalledWith(null);
      expect(pushes()).toEqual([]);
    });
  });
});
