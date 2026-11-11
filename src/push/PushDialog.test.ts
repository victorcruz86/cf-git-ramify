import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import type { CommitSummary, PushInfo, PushTags } from "../lib/api";
import PushDialog from "./PushDialog.svelte";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}), emitTo: vi.fn() }));

const HASH = "a1b2c3d4".repeat(5);

const INFO: PushInfo = {
  repository: "ramify",
  branch: "main",
  head: HASH,
  remotes: ["origin", "upstream"],
  remoteBranches: ["origin/main", "origin/feature/x"],
  target: { remote: "origin", branch: "main", isNew: false },
  prePushHook: false,
  protectedBranches: ["master", "main"],
  forcePushTargets: [],
};

const commit = (index: number): CommitSummary => ({
  hash: String(index).padStart(40, "0"),
  subject: `commit ${index}`,
  author: "Ana",
  time: 0,
});

const COMMITS = [commit(1), commit(2)];

interface Options {
  info?: Partial<PushInfo>;
  commits?: CommitSummary[];
  tags?: PushTags | null;
  source?: { revision: string; label: string };
  outgoing?: (remote: string, target: string) => Promise<CommitSummary[]>;
  defineRemote?: () => Promise<PushInfo | null>;
  preview?: boolean | null;
}

async function open({ info, commits = COMMITS, tags = null, source, ...rest }: Options = {}) {
  const onanswer = vi.fn();
  const outgoing = rest.outgoing ?? vi.fn(async () => commits);
  render(PushDialog, {
    info: { ...INFO, ...info },
    source: source ?? { revision: "main", label: "main" },
    tags,
    outgoing,
    defineRemote: rest.defineRemote,
    preview: rest.preview,
    onanswer,
  });
  await waitFor(() => expect(outgoing).toHaveBeenCalled());
  await Promise.resolve();
  return { onanswer, outgoing };
}

const push = () => screen.getByRole("button", { name: "Push" }) as HTMLButtonElement;
const tree = () => screen.getByRole("tree", { name: "Commits to push" });
const rows = () => within(tree()).getAllByRole("treeitem");
const pushTags = () => screen.getByRole("checkbox", { name: "Push tags:" }) as HTMLInputElement;
const tagsMode = () => screen.getByRole("combobox", { name: "Push tags mode" }) as HTMLSelectElement;
const targetLink = () => screen.getByRole("button", { name: "Target branch" });
const targetInput = () => screen.getByRole("combobox", { name: "Target branch" }) as HTMLInputElement;

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "commit_changes_of") {
      return [
        {
          parent: null,
          subject: null,
          base: null,
          merge: false,
          size: 1,
          files: [{ path: "src/a.txt", status: "M", oldPath: null }],
        },
      ];
    }
    if (command === "commit_details") {
      return [
        {
          hash: commit(1).hash,
          subject: "the whole message",
          body: "",
          author: "Ana",
          authorEmail: "",
          authorTime: 0,
          committer: "Ana",
          committerEmail: "",
          commitTime: 0,
          samePerson: true,
          refs: [],
          links: {},
          parents: [],
        },
      ];
    }
    return null;
  });
});

afterEach(cleanup);

test("title_push_commits_to_repo", async () => {
  await open();

  expect(screen.getByRole("dialog", { name: "Push Commits to ramify" })).toBeTruthy();
  expect(push().className).toContain("default");
  expect(screen.getByRole("button", { name: "Cancel" })).toBeTruthy();
});

test("row_branch_arrow_remote_and_editable_target", async () => {
  const { onanswer, outgoing } = await open();

  const row = rows()[0];
  expect(row.textContent?.replace(/\s+/g, " ").trim()).toBe("main → origin : main");
  expect(within(row).getByRole("button", { name: "Remote" }).textContent?.trim()).toBe("origin");

  await fireEvent.click(targetLink());
  const input = targetInput();
  expect(input.value).toBe("main");
  expect(push().disabled).toBe(true);

  await fireEvent.input(input, { target: { value: "feature/x" } });
  await fireEvent.keyDown(input, { key: "Enter" });

  expect(push().disabled).toBe(false);
  expect(targetLink().textContent?.trim()).toBe("feature/x");
  await waitFor(() => expect(outgoing).toHaveBeenCalledWith("origin", "feature/x"));

  await fireEvent.click(push());
  expect(onanswer).toHaveBeenCalledWith({
    force: false,
    preview: true,
    rememberTarget: false,
    remote: "origin",
    target: "feature/x",
    tags: null,
    noVerify: false,
  });
});

test("f2_edits_the_target_and_escape_leaves_it_as_it_was", async () => {
  const { onanswer, outgoing } = await open();

  await fireEvent.keyDown(tree(), { key: "F2" });
  await fireEvent.input(targetInput(), { target: { value: "other" } });
  await fireEvent.keyDown(targetInput(), { key: "Escape" });

  expect(targetLink().textContent?.trim()).toBe("main");
  expect(outgoing).toHaveBeenCalledTimes(1);
  expect(onanswer).not.toHaveBeenCalled();
});

test("keys_also_work_with_focus_on_a_row", async () => {
  const { onanswer } = await open();
  const row = await screen.findByRole("treeitem", { name: "commit 1" });

  await fireEvent.keyDown(row, { key: "ArrowDown" });
  expect(screen.getByRole("treeitem", { name: "commit 2" }).getAttribute("aria-selected")).toBe(
    "false",
  );
  expect(row.getAttribute("aria-selected")).toBe("true");

  await fireEvent.keyDown(row, { key: "ArrowDown" });
  expect(screen.getByRole("treeitem", { name: "commit 2" }).getAttribute("aria-selected")).toBe(
    "true",
  );

  await fireEvent.keyDown(row, { key: "Enter" });
  expect(onanswer).toHaveBeenCalled();
});

test("the_remotes_menu_goes_inside_the_dialog", async () => {
  await open();

  await fireEvent.click(screen.getByRole("button", { name: "Remote" }));

  const menu = screen.getByRole("menu", { name: "Remote" });
  expect(screen.getByRole("dialog").contains(menu)).toBe(true);
  const define = within(menu).getByRole("menuitem", { name: "Define remote" });
  expect((define as HTMLButtonElement).disabled).toBe(true);
});

test("an_invalid_target_does_not_leave_editing_with_enter", async () => {
  await open();

  await fireEvent.click(targetLink());
  await fireEvent.input(targetInput(), { target: { value: "a b" } });
  await fireEvent.keyDown(targetInput(), { key: "Enter" });

  expect(screen.getByRole("alert").textContent).toBe("Invalid destination branch name: a b");
  expect(targetInput()).toBeTruthy();
  expect(push().disabled).toBe(true);
});

test("on_leaving_the_field_an_invalid_target_stays_and_says_so", async () => {
  const { outgoing } = await open();

  await fireEvent.click(targetLink());
  await fireEvent.input(targetInput(), { target: { value: "a b" } });
  await fireEvent.blur(targetInput());

  expect(targetInput().value).toBe("a b");
  expect(screen.getByRole("alert").textContent).toBe("Invalid destination branch name: a b");
  expect(push().disabled).toBe(true);
  expect(outgoing).toHaveBeenCalledTimes(1);
});

test("pressing_push_with_an_invalid_target_does_not_push_elsewhere", async () => {
  const { onanswer } = await open();

  await fireEvent.click(targetLink());
  await fireEvent.input(targetInput(), { target: { value: "a b" } });
  await fireEvent.blur(targetInput());
  await fireEvent.click(push());
  await fireEvent.submit(push().closest("form")!);

  expect(onanswer).not.toHaveBeenCalled();
});

test("after_fixing_the_target_it_can_be_pushed_to", async () => {
  const { onanswer } = await open();

  await fireEvent.click(targetLink());
  await fireEvent.input(targetInput(), { target: { value: "a b" } });
  await fireEvent.blur(targetInput());
  await fireEvent.input(targetInput(), { target: { value: "valid" } });
  expect(screen.queryByRole("alert")).toBeNull();
  await fireEvent.blur(targetInput());
  await fireEvent.click(push());

  expect(onanswer).toHaveBeenCalledWith(expect.objectContaining({ target: "valid" }));
});

test("escape_abandons_an_invalid_target", async () => {
  await open();

  await fireEvent.click(targetLink());
  await fireEvent.input(targetInput(), { target: { value: "a b" } });
  await fireEvent.blur(targetInput());
  await fireEvent.keyDown(targetInput(), { key: "Escape" });

  expect(targetLink().textContent?.trim()).toBe("main");
  expect(push().disabled).toBe(false);
});

test("badge_new", async () => {
  await open({ info: { target: { remote: "origin", branch: "fresh", isNew: true } } });
  expect(within(rows()[0]).getByText("New")).toBeTruthy();
  cleanup();

  await open();
  expect(within(rows()[0]).queryByText("New")).toBeNull();
});

test("the_badge_follows_the_typed_target", async () => {
  await open();

  await fireEvent.click(targetLink());
  await fireEvent.input(targetInput(), { target: { value: "fresh" } });
  await fireEvent.keyDown(targetInput(), { key: "Enter" });

  expect(within(rows()[0]).getByText("New")).toBeTruthy();
});

test("choosing_another_remote_reloads_the_commits", async () => {
  const { onanswer, outgoing } = await open();

  await fireEvent.click(screen.getByRole("button", { name: "Remote" }));
  await fireEvent.click(screen.getByRole("menuitem", { name: "upstream" }));

  await waitFor(() => expect(outgoing).toHaveBeenCalledWith("upstream", "main"));
  expect(screen.getByRole("button", { name: "Remote" }).textContent?.trim()).toBe("upstream");
  await fireEvent.click(push());
  expect(onanswer).toHaveBeenCalledWith(expect.objectContaining({ remote: "upstream" }));
});

test("without_remotes_offers_to_define_one_and_does_not_allow_pushing", async () => {
  const fresh = { ...INFO, target: { remote: "origin", branch: "main", isNew: true } };
  const defineRemote = vi.fn(async () => fresh);
  const onanswer = vi.fn();
  const outgoing = vi.fn(async () => COMMITS);
  render(PushDialog, {
    info: { ...INFO, remotes: [], target: { remote: null, branch: "main", isNew: false } },
    source: { revision: "main", label: "main" },
    tags: null,
    outgoing,
    defineRemote,
    onanswer,
  });

  expect(outgoing).not.toHaveBeenCalled();
  expect(push().disabled).toBe(true);
  expect(screen.queryByRole("button", { name: "Target branch" })).toBeNull();

  await fireEvent.click(screen.getByRole("button", { name: "Define remote" }));

  await waitFor(() => expect(outgoing).toHaveBeenCalledWith("origin", "main"));
  expect(push().disabled).toBe(false);
});

test("without_a_definer_the_link_stays_disabled", () => {
  render(PushDialog, {
    info: { ...INFO, remotes: [], target: { remote: null, branch: "main", isNew: false } },
    source: { revision: "main", label: "main" },
    tags: null,
    outgoing: vi.fn(async () => []),
    onanswer: vi.fn(),
  });

  expect((screen.getByRole("button", { name: "Define remote" }) as HTMLButtonElement).disabled).toBe(
    true,
  );
});

test("a_repository_without_commits_cannot_be_pushed", () => {
  const outgoing = vi.fn(async () => []);
  render(PushDialog, {
    info: { ...INFO, head: null },
    source: { revision: "main", label: "main" },
    tags: null,
    outgoing,
    onanswer: vi.fn(),
  });

  expect(screen.getByText("Empty repository")).toBeTruthy();
  expect(outgoing).not.toHaveBeenCalled();
  expect(push().disabled).toBe(true);
});

test("without_commits_to_push_it_can_go_on", async () => {
  const { onanswer } = await open({ commits: [] });

  await waitFor(() => expect(rows()).toHaveLength(1));
  await fireEvent.click(push());

  expect(onanswer).toHaveBeenCalled();
});

test("if_the_commits_cannot_be_read_it_says_so_and_does_not_allow_pushing", async () => {
  await open({
    outgoing: vi.fn(async () => {
      throw { kind: "failed", message: "fatal: bad revision" };
    }),
  });

  expect(await screen.findByText("fatal: bad revision")).toBeTruthy();
  expect(push().disabled).toBe(true);
});

test("an_old_response_does_not_overwrite_the_new_one", async () => {
  let settle: (commits: CommitSummary[]) => void = () => {};
  const slow = new Promise<CommitSummary[]>((resolve) => (settle = resolve));
  const outgoing = vi.fn(async () => [commit(3)]);
  outgoing.mockReturnValueOnce(slow);
  await open({ outgoing });

  await fireEvent.click(targetLink());
  await fireEvent.input(targetInput(), { target: { value: "other" } });
  await fireEvent.keyDown(targetInput(), { key: "Enter" });
  await screen.findByText("commit 3");

  settle([commit(9)]);
  await Promise.resolve();
  await Promise.resolve();

  expect(screen.queryByText("commit 9")).toBeNull();
  expect(screen.getByText("commit 3")).toBeTruthy();
});

test("shows_20_commits_and_the_rest_with_dots", async () => {
  const commits = Array.from({ length: 45 }, (_, index) => commit(index));
  await open({ commits });

  await screen.findByText("commit 19");
  expect(screen.queryByText("commit 20")).toBeNull();

  await fireEvent.click(screen.getByRole("button", { name: "..." }));

  expect(screen.getByText("commit 39")).toBeTruthy();
  expect(screen.queryByText("commit 40")).toBeNull();

  await fireEvent.click(screen.getByRole("button", { name: "..." }));

  expect(screen.getByText("commit 44")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "..." })).toBeNull();
});

test("selecting_a_commit_shows_changes_and_detail", async () => {
  await open();
  const row = await screen.findByRole("treeitem", { name: "commit 1" });
  expect(row.getAttribute("aria-selected")).toBe("false");
  expect(screen.queryByRole("region", { name: "Commit details" })).toBeNull();

  await fireEvent.click(row);

  expect(row.getAttribute("aria-selected")).toBe("true");
  expect(await screen.findByRole("treeitem", { name: "a.txt" })).toBeTruthy();
  expect(await screen.findByText("the whole message")).toBeTruthy();
  const details = vi.mocked(invoke).mock.calls.filter(([name]) => name === "commit_details");
  expect(details.at(-1)?.[1]).toEqual({ hashes: [commit(1).hash] });
  // IntelliJ does not show the commit's branches here, and without the log open they would not be known.
  const asked = vi.mocked(invoke).mock.calls.map(([name]) => name);
  expect(asked).not.toContain("branches_containing");
  expect(screen.queryByText(/In branches|Not in any branch/)).toBeNull();
});

test("on_open_the_branch_row_shows_the_changes_of_all_its_commits", async () => {
  await open();

  await waitFor(() => expect(rows()[0].getAttribute("aria-selected")).toBe("true"));

  await waitFor(() => {
    const started = vi.mocked(invoke).mock.calls.find(([name]) => name === "commit_changes_start");
    expect(started?.[1]).toMatchObject({ hashes: COMMITS.map(({ hash }) => hash) });
  });
  expect(screen.queryByRole("region", { name: "Commit details" })).toBeNull();
});

test("cmd_click_adds_and_shift_click_marks_a_range", async () => {
  const commits = [commit(1), commit(2), commit(3)];
  await open({ commits });
  const row = (index: number) => screen.getByRole("treeitem", { name: `commit ${index}` });
  await screen.findByText("commit 3");

  await fireEvent.click(row(1));
  await fireEvent.click(row(3), { metaKey: true });
  expect(row(1).getAttribute("aria-selected")).toBe("true");
  expect(row(2).getAttribute("aria-selected")).toBe("false");
  expect(row(3).getAttribute("aria-selected")).toBe("true");

  await fireEvent.click(row(1));
  await fireEvent.click(row(3), { shiftKey: true });
  expect(row(2).getAttribute("aria-selected")).toBe("true");
});

test("run_git_hooks_only_with_a_hook", async () => {
  const { onanswer } = await open();
  expect(screen.queryByRole("checkbox", { name: "Run Git hooks" })).toBeNull();
  await fireEvent.click(push());
  expect(onanswer).toHaveBeenCalledWith(expect.objectContaining({ noVerify: false }));
  cleanup();

  const hooked = await open({ info: { prePushHook: true } });
  const hooks = screen.getByRole("checkbox", { name: "Run Git hooks" }) as HTMLInputElement;
  expect(hooks.checked).toBe(true);
  expect(hooks.closest("label")?.title).toBe(
    "If unchecked, Git hooks will be skipped with the '--no-verify' parameter",
  );

  await fireEvent.click(hooks);
  await fireEvent.click(push());

  expect(hooked.onanswer).toHaveBeenCalledWith(expect.objectContaining({ noVerify: true }));
});

test("push_tags_remembers_the_choice", async () => {
  const { onanswer } = await open({ tags: "currentBranch" });

  expect(pushTags().checked).toBe(true);
  expect(tagsMode().value).toBe("currentBranch");
  expect(tagsMode().disabled).toBe(false);
  await fireEvent.click(push());

  expect(onanswer).toHaveBeenCalledWith(expect.objectContaining({ tags: "currentBranch" }));
});

test("push_tags_unchecked_by_default_and_the_mode_disabled", async () => {
  const { onanswer } = await open();

  expect(pushTags().checked).toBe(false);
  expect(tagsMode().disabled).toBe(true);
  expect(tagsMode().value).toBe("all");
  expect([...tagsMode().options].map(({ text }) => text)).toEqual(["All", "Current Branch"]);

  await fireEvent.click(pushTags());
  expect(tagsMode().disabled).toBe(false);
  await fireEvent.change(tagsMode(), { target: { value: "currentBranch" } });
  await fireEvent.click(push());

  expect(onanswer).toHaveBeenCalledWith(expect.objectContaining({ tags: "currentBranch" }));
});

test("cancel_answers_no", async () => {
  const { onanswer } = await open();

  await fireEvent.click(screen.getByRole("button", { name: "Cancel" }));

  expect(onanswer).toHaveBeenCalledWith(null);
});

const FEATURE: Partial<PushInfo> = {
  branch: "feature",
  target: { remote: "origin", branch: "feature", isNew: false },
};

async function openForceMenu() {
  await fireEvent.click(screen.getByRole("button", { name: "More push options" }));
  return screen.getByRole("menuitem", { name: "Force Push" }) as HTMLButtonElement;
}

test("force_push_button_in_the_dropdown", async () => {
  const { onanswer } = await open({ info: FEATURE });

  expect(screen.queryByRole("menuitem", { name: "Force Push" })).toBeNull();
  await fireEvent.click(await openForceMenu());

  expect(screen.getByRole("dialog", { name: "Force Push" })).toBeTruthy();
  expect(onanswer).not.toHaveBeenCalled();
  await fireEvent.click(
    within(screen.getByRole("dialog", { name: "Force Push" })).getByRole("button", {
      name: "Force Push",
    }),
  );

  expect(onanswer).toHaveBeenCalledWith(
    expect.objectContaining({ target: "feature", force: true, rememberTarget: false }),
  );
});

test("cancelling_the_warning_leaves_the_push_dialog_open", async () => {
  const { onanswer } = await open({ info: FEATURE });

  await fireEvent.click(await openForceMenu());
  await fireEvent.click(
    within(screen.getByRole("dialog", { name: "Force Push" })).getByRole("button", {
      name: "Cancel",
    }),
  );

  expect(screen.queryByRole("dialog", { name: "Force Push" })).toBeNull();
  expect(onanswer).not.toHaveBeenCalled();
});

test("a_remembered_target_forces_without_warning", async () => {
  const { onanswer } = await open({
    info: { ...FEATURE, forcePushTargets: ["origin:feature"] },
  });

  await fireEvent.click(await openForceMenu());

  expect(screen.queryByRole("dialog", { name: "Force Push" })).toBeNull();
  expect(onanswer).toHaveBeenCalledWith(expect.objectContaining({ force: true }));
});

test("protected_disables_with_force_push_prohibited", async () => {
  const { onanswer } = await open();

  const item = await openForceMenu();

  expect(item.disabled).toBe(true);
  expect(item.title).toBe("Force push to main is prohibited");
  await fireEvent.click(item);
  expect(onanswer).not.toHaveBeenCalled();
});

test("a_remembered_target_is_not_valid_if_it_is_now_protected", async () => {
  await open({ info: { forcePushTargets: ["origin:main"] } });

  expect((await openForceMenu()).disabled).toBe(true);
});

test("the_repository_patterns_decide_what_is_protected", async () => {
  await open({ info: { ...FEATURE, protectedBranches: ["fea.*"] } });

  expect((await openForceMenu()).title).toBe("Force push to feature is prohibited");
});

test("editing_the_target_to_a_protected_one_disables_force_push", async () => {
  await open({ info: FEATURE });

  await fireEvent.click(screen.getByRole("button", { name: "Target branch" }));
  const field = screen.getByRole("combobox", { name: "Target branch" });
  await fireEvent.input(field, { target: { value: "master" } });
  await fireEvent.keyDown(field, { key: "Enter" });

  expect((await openForceMenu()).disabled).toBe(true);
});

test("the_full_name_of_a_protected_one_also_disables_force_push", async () => {
  await open({
    info: { ...FEATURE, target: { remote: "origin", branch: "refs/heads/main", isNew: false } },
  });

  expect((await openForceMenu()).disabled).toBe(true);
});

test("without_being_able_to_push_it_cannot_force", () => {
  render(PushDialog, {
    info: { ...INFO, ...FEATURE, target: { remote: null, branch: "feature", isNew: false } },
    source: { revision: "feature", label: "feature" },
    tags: null,
    outgoing: vi.fn(async () => []),
    onanswer: vi.fn(),
  });

  expect(
    (screen.getByRole("button", { name: "More push options" }) as HTMLButtonElement).disabled,
  ).toBe(true);
});

test("enter_never_forces", async () => {
  const { onanswer } = await open({ info: FEATURE });

  await fireEvent.keyDown(tree(), { key: "Enter" });

  expect(onanswer).toHaveBeenCalledWith(expect.objectContaining({ force: false }));
});

test("only_after_a_commit_the_preview_checkbox_appears", async () => {
  const label = "For Commit and Push to non-protected branches, preview commits before push";

  await open({ info: FEATURE });
  expect(screen.queryByRole("checkbox", { name: label })).toBeNull();
  cleanup();

  const { onanswer } = await open({ info: FEATURE, preview: true });
  await fireEvent.click(screen.getByRole("checkbox", { name: label }));
  await fireEvent.click(push());

  expect(onanswer).toHaveBeenCalledWith(expect.objectContaining({ preview: false }));
});

test("with_a_protected_target_the_preview_checkbox_does_not_appear", async () => {
  await open({ preview: true });

  expect(screen.queryByRole("checkbox", { name: /preview commits/ })).toBeNull();
});

test("there_is_no_upstream_checkbox", async () => {
  await open();

  expect(screen.queryByRole("checkbox", { name: /upstream/i })).toBeNull();
});

test("the_diff_detail_opens_with_double_click", async () => {
  await open();
  await fireEvent.click(await screen.findByRole("treeitem", { name: "commit 1" }));
  const file = await screen.findByRole("treeitem", { name: "a.txt" });
  expect(screen.queryByRole("region", { name: "Diff Preview" })).toBeNull();

  await fireEvent.dblClick(file);

  expect(await screen.findByRole("region", { name: "Diff Preview" })).toBeTruthy();
  await fireEvent.click(screen.getByRole("button", { name: "Close" }));
  expect(screen.queryByRole("region", { name: "Diff Preview" })).toBeNull();
});
