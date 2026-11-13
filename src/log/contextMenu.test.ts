import { expect, test, vi } from "vitest";
import type { LogRef } from "../lib/api";
import type { MenuAction, MenuItem } from "../lib/menu";
import { branchActions, type BranchNode } from "../branches/model";
import {
  contextMenu,
  type ContextHandlers,
  type LogSelection,
  type RefOperation,
  type RepoState,
} from "./contextMenu";
import { showRepositoryAtRevision } from "../repositories/tab";
import {
  compareBranches,
  compareRevisions,
  compareWithLocal,
  showDiffWithWorkingTree,
} from "../compare/actions";

vi.mock("../repositories/tab", () => ({ showRepositoryAtRevision: vi.fn() }));
vi.mock("../compare/actions", () => ({
  compareBranches: vi.fn(async () => {}),
  compareRevisions: vi.fn(async () => {}),
  compareWithLocal: vi.fn(async () => {}),
  showDiffWithWorkingTree: vi.fn(async () => {}),
}));

const HASH = "a1b2c3d4".repeat(5);
const OTHER = "e5f6a7b8".repeat(5);
const ON_MAIN: RepoState = { currentBranch: "main" };

const local = (name: string): LogRef => ({ name, kind: "local" });
const remote = (name: string): LogRef => ({ name, kind: "remote" });
const tag = (name: string): LogRef => ({ name, kind: "tag" });

function selection(refs: LogRef[] = [], overrides: Partial<LogSelection> = {}): LogSelection {
  return {
    commits: [{ hash: HASH, subject: "one", refs }],
    parents: [1],
    children: [],
    ...overrides,
  };
}

function handlers(overrides: Partial<ContextHandlers> = {}): ContextHandlers {
  return { copy: vi.fn(), goToChild: vi.fn(), goToParent: vi.fn(), vcs: vi.fn(), ...overrides };
}

const isAction = (item: MenuItem): item is MenuAction => item !== null && "label" in item;

const labels = (items: MenuItem[]) =>
  items.map((item) => (item === null ? "—" : isAction(item) ? item.label : item.header));

function find(items: MenuItem[], label: string): MenuAction {
  const found = items.filter(isAction).find((item) => item.label === label);
  if (!found) throw new Error(`${label} not found: ${labels(items)}`);
  return found;
}

// `Vcs.Log.ContextMenu` (`LX:188-197`) with `Git.Log.ContextMenu.CheckoutBrowse` before
// `Compare Versions` (`X:301-306`): Compare Versions only with two commits, Compare with Local
// only with one.
test("menu_in_intellij_order", () => {
  const items = contextMenu(selection([local("other"), tag("v1")]), ON_MAIN, handlers());

  expect(labels(items)).toEqual([
    "Copy Revision Number",
    "Copy Commit Subject",
    "Create Patch...",
    "Cherry-Pick",
    "—",
    "Checkout",
    "Show Repository at Revision",
    "Compare with Local",
    "—",
    "Reset Current Branch to Here…",
    "Revert Commit",
    "Undo Commit…",
    "—",
    "Edit Commit Message…",
    "Fixup…",
    "Squash Into…",
    "Drop Commit",
    "Interactively Rebase from Here…",
    "Push All up to Here…",
    "—",
    "Branch 'other'",
    "Tag 'v1'",
    "New Branch…",
    "New Tag…",
    "—",
    "Go to Child Commit",
    "Go to Parent Commit",
  ]);

  const two = contextMenu(
    selection([], {
      commits: [
        { hash: HASH, subject: "one", refs: [] },
        { hash: OTHER, subject: "two", refs: [] },
      ],
    }),
    ON_MAIN,
    handlers(),
  );
  expect(labels(two).slice(0, 8)).toEqual([
    "Copy Revision Number",
    "Copy Commit Subject",
    "Create Patch...",
    "Cherry-Pick",
    "—",
    "Show Repository at Revision",
    "Compare Versions",
    "—",
  ]);
  expect(find(two, "Show Repository at Revision").disabled).toBe(true);
});

// `CompareRevisionsFromLogAction` and `ShowDiffAfterWithLocalFromLogActionProvider`: through
// `showDiffForPaths`, the older selected commit on the left, the path filter's paths, and always the
// `Changes` window.
test("compare_versions_and_compare_with_local_use_the_path_filter", () => {
  const paths = ["src", "README.md"];
  const two = contextMenu(
    selection([], {
      commits: [
        { hash: HASH, subject: "one", refs: [] },
        { hash: OTHER, subject: "two", refs: [] },
      ],
    }),
    ON_MAIN,
    handlers({ paths }),
  );
  find(two, "Compare Versions").run?.();
  const listed = paths.map((path) => ({ path, folder: true }));
  expect(compareRevisions).toHaveBeenCalledWith(OTHER, HASH, listed);

  const one = contextMenu(selection(), ON_MAIN, handlers({ paths }));
  find(one, "Compare with Local").run?.();
  expect(compareWithLocal).toHaveBeenCalledWith(HASH, listed);
});

test("create_patch_runs_its_handler_and_is_off_without_one", () => {
  const createPatch = vi.fn();
  expect(find(contextMenu(selection(), ON_MAIN, handlers()), "Create Patch...").disabled).toBe(true);

  const items = contextMenu(selection(), ON_MAIN, handlers({ createPatch }));
  find(items, "Create Patch...").run?.();

  expect(createPatch).toHaveBeenCalled();
});

test("show_repository_at_revision_opens_the_commit_in_the_repositories_window", () => {
  const items = contextMenu(selection(), ON_MAIN, handlers());

  const browse = find(items, "Show Repository at Revision");
  expect(browse.disabled).toBe(false);
  browse.run?.();
  expect(showRepositoryAtRevision).toHaveBeenCalledWith(HASH);
});

test("shortcuts_visible_in_the_menu", () => {
  const items = contextMenu(selection(), ON_MAIN, handlers());

  expect(find(items, "Copy Revision Number").shortcut).toBe("⌥⇧⌘C");
  expect(find(items, "Go to Child Commit").shortcut).toBe("←");
  expect(find(items, "Go to Parent Commit").shortcut).toBe("→");
});

test("copy_hash_and_subject", () => {
  const copy = vi.fn();
  const several: LogSelection = {
    commits: [
      { hash: HASH, subject: "one", refs: [] },
      { hash: OTHER, subject: "two", refs: [] },
    ],
    parents: [],
    children: [],
  };
  const items = contextMenu(several, ON_MAIN, handlers({ copy }));

  // `CopyRevisionNumberAction` reverses the rows: the oldest first.
  find(items, "Copy Revision Number").run?.();
  expect(copy).toHaveBeenLastCalledWith(`${OTHER} ${HASH}`);

  find(items, "Copy Commit Subject").run?.();
  expect(copy).toHaveBeenLastCalledWith("one\ntwo");
});

test("checkout_in_the_menu_lists_branches_and_revision", () => {
  const vcs = vi.fn();
  const refs = [local("main"), local("other"), local("third"), remote("origin/other"), tag("v1")];
  const items = contextMenu(selection(refs), ON_MAIN, handlers({ vcs }));

  const { submenu } = find(items, "Checkout");

  // Neither the current branch, nor the remote ones, nor the tags.
  expect(labels(submenu!)).toEqual(["other", "third", "Revision 'a1b2c3d4'"]);
  find(submenu!, "third").run?.();
  expect(vcs).toHaveBeenLastCalledWith({ kind: "checkout", target: "third" });
  find(submenu!, "Revision 'a1b2c3d4'").run?.();
  expect(vcs).toHaveBeenLastCalledWith({ kind: "checkout", target: HASH });
});

test("without_other_branches_checkout_is_a_single_row", () => {
  const vcs = vi.fn();
  const items = contextMenu(selection([local("main")]), ON_MAIN, handlers({ vcs }));

  const checkout = find(items, "Checkout Revision 'a1b2c3d4'");

  expect(checkout.submenu).toBeUndefined();
  checkout.run?.();
  expect(vcs).toHaveBeenLastCalledWith({ kind: "checkout", target: HASH });
});

test("push_all_up_to_here_is_enabled_on_detached_head", () => {
  const vcs = vi.fn();

  const onBranch = find(contextMenu(selection(), ON_MAIN, handlers({ vcs })), "Push All up to Here…");
  expect(onBranch.disabled).toBeFalsy();
  onBranch.run?.();
  expect(vcs).toHaveBeenLastCalledWith({ kind: "pushUpTo", hash: HASH });

  const detached = find(
    contextMenu(selection(), { currentBranch: null }, handlers({ vcs })),
    "Push All up to Here…",
  );
  expect(detached.disabled).toBeFalsy();
  detached.run?.();
  expect(vcs).toHaveBeenLastCalledWith({ kind: "pushUpTo", hash: HASH });
});

test("commit_actions_call_the_injected_function", () => {
  const vcs = vi.fn();
  const items = contextMenu(selection(), ON_MAIN, handlers({ vcs }));

  find(items, "Reset Current Branch to Here…").run?.();
  expect(vcs).toHaveBeenLastCalledWith({ kind: "reset", hash: HASH });
  find(items, "New Branch…").run?.();
  expect(vcs).toHaveBeenLastCalledWith({ kind: "newBranch", hash: HASH });
  find(items, "New Tag…").run?.();
  expect(vcs).toHaveBeenLastCalledWith({ kind: "newTag", hash: HASH });
});

test("without_injected_function_branch_items_are_disabled", () => {
  const items = contextMenu(
    selection([local("other"), tag("v1")]),
    ON_MAIN,
    handlers({ vcs: undefined }),
  );

  for (const label of [
    "Checkout",
    "Cherry-Pick",
    "Revert Commit",
    "Reset Current Branch to Here…",
    "Edit Commit Message…",
    "Drop Commit",
    "Push All up to Here…",
    "Branch 'other'",
    "Tag 'v1'",
    "New Branch…",
    "New Tag…",
  ]) {
    expect(find(items, label).disabled, label).toBe(true);
  }
  expect(find(items, "Copy Revision Number").disabled).toBeFalsy();
});

test("several_commits_hide_groups_and_disable_single_commit_items", () => {
  const several: LogSelection = {
    commits: [
      { hash: HASH, subject: "one", refs: [local("other")] },
      { hash: OTHER, subject: "two", refs: [] },
    ],
    parents: [],
    children: [],
  };
  const items = contextMenu(several, ON_MAIN, handlers());

  expect(labels(items)).toEqual([
    "Copy Revision Number",
    "Copy Commit Subject",
    "Create Patch...",
    "Cherry-Pick",
    "—",
    "Show Repository at Revision",
    "Compare Versions",
    "—",
    "Reset Current Branch to Here…",
    "Revert Commits",
    "—",
    "Drop Commits",
    "Squash Commits…",
    "Push All up to Here…",
    "—",
    "New Branch…",
    "New Tag…",
    "—",
    "Go to Child Commit",
    "Go to Parent Commit",
  ]);
  // Cherry-Pick and Revert work with several commits.
  expect(find(items, "Cherry-Pick").disabled).toBeFalsy();
  expect(find(items, "Revert Commits").disabled).toBeFalsy();
  for (const label of [
    "Show Repository at Revision",
    "Reset Current Branch to Here…",
    "Push All up to Here…",
    "New Branch…",
    "New Tag…",
    "Go to Child Commit",
    "Go to Parent Commit",
  ]) {
    expect(find(items, label).disabled, label).toBe(true);
  }
});

test("cherry_pick_and_revert_carry_the_commits_in_log_order", () => {
  const vcs = vi.fn();
  const several: LogSelection = {
    commits: [
      { hash: HASH, subject: "one", refs: [] },
      { hash: OTHER, subject: "two", refs: [local("other")] },
    ],
    parents: [],
    children: [],
  };
  const items = contextMenu(several, ON_MAIN, handlers({ vcs }));
  const commits = [
    { hash: HASH, subject: "one" },
    { hash: OTHER, subject: "two" },
  ];

  find(items, "Cherry-Pick").run?.();
  expect(vcs).toHaveBeenLastCalledWith({ kind: "cherryPick", commits });
  find(items, "Revert Commits").run?.();
  expect(vcs).toHaveBeenLastCalledWith({ kind: "revert", commits });
});

test("revert_disabled_on_merges_with_its_text", () => {
  const vcs = vi.fn();
  const merge: LogSelection = {
    commits: [{ hash: HASH, subject: "unir", refs: [], parentCount: 2 }],
    parents: [1],
    children: [],
  };

  const alone = find(contextMenu(merge, ON_MAIN, handlers({ vcs })), "Revert Commit");
  expect(alone.disabled).toBe(true);
  expect(alone.title).toBe("Reverting merge commits is not allowed");
  alone.run?.();

  // One among several is enough, and Cherry-Pick stays on.
  const mixed: LogSelection = {
    commits: [{ hash: OTHER, subject: "one", refs: [], parentCount: 1 }, ...merge.commits],
    parents: [],
    children: [],
  };
  const items = contextMenu(mixed, ON_MAIN, handlers({ vcs }));
  expect(find(items, "Revert Commits").disabled).toBe(true);
  expect(find(items, "Revert Commits").title).toBe("Reverting merge commits is not allowed");
  expect(find(items, "Cherry-Pick").disabled).toBeFalsy();

  // Without merges there is no text.
  const plain = find(contextMenu(selection(), ON_MAIN, handlers({ vcs })), "Revert Commit");
  expect(plain.disabled).toBeFalsy();
  expect(plain.title).toBeUndefined();
});

test("no_commits_no_menu", () => {
  const empty: LogSelection = { commits: [], parents: [], children: [] };
  expect(contextMenu(empty, ON_MAIN, handlers())).toEqual([]);
});

test("go_to_child_and_parent", () => {
  const goToChild = vi.fn();
  const goToParent = vi.fn();
  const items = contextMenu(
    selection([], { parents: [2], children: [] }),
    ON_MAIN,
    handlers({ goToChild, goToParent }),
  );

  expect(find(items, "Go to Child Commit").disabled).toBe(true);
  find(items, "Go to Parent Commit").run?.();
  expect(goToParent).toHaveBeenCalled();
});

test("two_branches_go_as_single_rows_and_three_in_a_submenu", () => {
  const two = contextMenu(selection([local("a"), remote("origin/a")]), ON_MAIN, handlers());
  expect(labels(two)).toContain("Branch 'a'");
  expect(labels(two)).toContain("Branch 'origin/a'");
  expect(labels(two)).not.toContain("Branches");

  const three = contextMenu(
    selection([local("main"), local("a"), local("b"), remote("origin/a")]),
    ON_MAIN,
    handlers(),
  );
  // The current branch does not count.
  const { submenu } = find(three, "Branches");
  expect(labels(submenu!)).toEqual(["a", "b", "origin/a"]);
});

test("one_tag_goes_as_a_single_row_and_two_in_a_submenu", () => {
  const vcs = vi.fn();
  const one = contextMenu(selection([tag("v1")]), ON_MAIN, handlers({ vcs }));
  find(find(one, "Tag 'v1'").submenu!, "Delete").run?.();
  expect(vcs).toHaveBeenLastCalledWith({ kind: "ref", operation: "delete", ref: tag("v1") });

  const two = contextMenu(selection([tag("v1"), tag("v2")]), ON_MAIN, handlers());
  expect(labels(find(two, "Tags").submenu!)).toEqual(["v1", "v2"]);
});

// `GitLogBranchOperationsActionGroup` uses `LocalBranchActions` and `RemoteBranchActions`, those of the popup.
test("local_and_remote_branch_submenu_like_the_popup", () => {
  const vcs = vi.fn();
  const items = contextMenu(
    selection([local("other"), remote("origin/other")]),
    ON_MAIN,
    handlers({ vcs }),
  );

  const own = find(items, "Branch 'other'").submenu!;
  expect(labels(own)).toEqual([
    "Checkout",
    "New Branch from 'other'…",
    "Checkout and Rebase onto 'main'",
    "—",
    "Compare with 'main'",
    "Show Diff with Working Tree",
    "—",
    "Rebase 'main' onto 'other'",
    "Merge 'other' into 'main'",
    "—",
    "Update",
    "Push…",
    "—",
    "Rename…",
    "Delete",
  ]);
  find(own, "Rename…").run?.();
  expect(vcs).toHaveBeenLastCalledWith({ kind: "ref", operation: "rename", ref: local("other") });
  const runs: [string, RefOperation][] = [
    ["Checkout and Rebase onto 'main'", "checkoutAndRebase"],
    ["Rebase 'main' onto 'other'", "rebase"],
    ["Merge 'other' into 'main'", "merge"],
  ];
  for (const [label, operation] of runs) {
    expect(find(own, label).disabled, label).toBe(false);
    find(own, label).run?.();
    expect(vcs).toHaveBeenLastCalledWith({ kind: "ref", operation, ref: local("other") });
  }

  const theirs = find(items, "Branch 'origin/other'").submenu!;
  expect(labels(theirs)).toEqual([
    "Checkout",
    "New Branch from 'origin/other'…",
    "Checkout and Rebase onto 'main'",
    "—",
    "Compare with 'main'",
    "Show Diff with Working Tree",
    "—",
    "Rebase 'main' onto 'origin/other'",
    "Merge 'origin/other' into 'main'",
    "—",
    "Pull into 'main' Using Merge",
    "Pull into 'main' Using Rebase",
    "—",
    "Delete",
  ]);
  const pulls: [string, RefOperation][] = [
    ["Pull into 'main' Using Merge", "pullMerge"],
    ["Pull into 'main' Using Rebase", "pullRebase"],
  ];
  for (const [label, operation] of pulls) {
    find(theirs, label).run?.();
    expect(vcs).toHaveBeenLastCalledWith({ kind: "ref", operation, ref: remote("origin/other") });
  }
  find(theirs, "Compare with 'main'").run?.();
  expect(compareBranches).toHaveBeenLastCalledWith("origin/other", "main");
  find(own, "Show Diff with Working Tree").run?.();
  expect(showDiffWithWorkingTree).toHaveBeenLastCalledWith("other");
});

test("the_branch_submenu_says_and_disables_the_same_as_the_popup", () => {
  const shape = (items: MenuItem[]) =>
    items.map((item) =>
      item !== null && isAction(item)
        ? { label: item.label, disabled: item.disabled ?? false, title: item.title }
        : item,
    );
  for (const current of ["main", null]) {
    const items = contextMenu(
      selection([local("other"), remote("origin/other")]),
      { currentBranch: current },
      handlers(),
    );
    const state = { current, remotes: true, onvcs: vi.fn() };
    const popup = (name: string, section: "local" | "remote") =>
      branchActions({ name, section, tracked: "origin/other" } as BranchNode, state);

    expect(shape(find(items, "Branch 'other'").submenu!)).toEqual(shape(popup("other", "local")));
    expect(shape(find(items, "Branch 'origin/other'").submenu!)).toEqual(
      shape(popup("origin/other", "remote")),
    );
  }
  // With a detached HEAD, Rebase is turned off with IntelliJ's reason.
  const detached = contextMenu(selection([local("other")]), { currentBranch: null }, handlers());
  const rebase = find(find(detached, "Branch 'other'").submenu!, "Rebase 'HEAD' onto 'other'");
  expect(rebase.disabled).toBe(true);
  expect(rebase.title).toBe("Rebase is not possible in the detached HEAD state");
});

test("long_name_is_shortened_in_the_submenu", () => {
  const name = "n".repeat(45);
  const items = contextMenu(selection([local(name)]), ON_MAIN, handlers());

  const row = find(items, `Branch '${name}'`).submenu!;

  expect(labels(row)).toContain(`New Branch from '${"n".repeat(39)}…'…`);
});

test("repository_patterns_decide_which_remote_is_not_deleted", () => {
  const items = contextMenu(
    selection([remote("origin/release-1"), remote("origin/main")]),
    ON_MAIN,
    handlers({ protectedBranches: ["release.*"] }),
  );

  const delete_ = (branch: string) => find(find(items, branch).submenu ?? [], "Delete");
  expect(delete_("Branch 'origin/release-1'").disabled).toBe(true);
  expect(delete_("Branch 'origin/main'").disabled).toBe(false);
});

test("a_protected_remote_branch_is_not_deleted_from_the_menu", () => {
  const items = contextMenu(
    selection([remote("origin/main"), remote("origin/other")]),
    ON_MAIN,
    handlers({ protectedBranches: ["master", "main"] }),
  );

  expect(find(find(items, "Branch 'origin/main'").submenu ?? [], "Delete").disabled).toBe(true);
  expect(find(find(items, "Branch 'origin/other'").submenu ?? [], "Delete").disabled).toBe(false);
});

// `Git.Hosting.Open.In.Browser.Group` is `anchor="last"` in `Vcs.Log.ContextMenu` (`g4x:519`): after
// `Go to Parent Commit`, with no separator of its own. Without a GitHub repository it is not there.
test("each_menu_has_open_on_github_at_241s_place_log", () => {
  const hosting: MenuItem[] = [{ label: "Open on GitHub", run: vi.fn() }];
  const items = contextMenu(selection([]), ON_MAIN, handlers({ hosting }));

  expect(labels(items).slice(-3)).toEqual(["Go to Child Commit", "Go to Parent Commit", "Open on GitHub"]);
  expect(labels(contextMenu(selection([]), ON_MAIN, handlers()))).not.toContain("Open on GitHub");
});

// Ramify's own (`docs/parity-matrix.md`): with `copyBranchName` given, the submenu of each branch
// ends with the row the branches popup has; a tag has none, and without it no branch has one.
test("a_branch_submenu_ends_with_copy_branch_name_when_it_is_on", () => {
  const copyBranchName = vi.fn();
  const items = contextMenu(
    selection([local("feature/HT-1-a"), remote("origin/x"), tag("v1")]),
    ON_MAIN,
    handlers({ copyBranchName }),
  );
  const localMenu = find(items, "Branch 'feature/HT-1-a'").submenu ?? [];
  const remoteMenu = find(items, "Branch 'origin/x'").submenu ?? [];

  expect(labels(localMenu).slice(-2)).toEqual(["—", "Copy Branch Name"]);
  expect(labels(remoteMenu).slice(-2)).toEqual(["—", "Copy Branch Name"]);
  expect(labels(find(items, "Tag 'v1'").submenu ?? [])).not.toContain("Copy Branch Name");
  find(localMenu, "Copy Branch Name").run?.();
  expect(copyBranchName).toHaveBeenLastCalledWith("feature/HT-1-a");
  find(remoteMenu, "Copy Branch Name").run?.();
  expect(copyBranchName).toHaveBeenLastCalledWith("origin/x");

  const off = contextMenu(selection([local("feature/HT-1-a")]), ON_MAIN, handlers());
  expect(labels(find(off, "Branch 'feature/HT-1-a'").submenu ?? [])).not.toContain("Copy Branch Name");
});
