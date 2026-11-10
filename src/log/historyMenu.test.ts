import { describe, expect, test, vi } from "vitest";
import type { MenuAction, MenuItem } from "../lib/menu";
import { historyPair } from "./history";
import { historyMenu, type HistoryCommit, type HistoryHandlers } from "./historyMenu";

const HASH_A = "a".repeat(40);
const HASH_B = "b".repeat(40);
const HASH_C = "c".repeat(40);

const commit = (hash: string, change: Partial<HistoryCommit> = {}): HistoryCommit => ({
  hash,
  subject: `subject ${hash[0]}`,
  parentCount: 1,
  entry: { path: "src/a.txt", deleted: false },
  ...change,
});

function handlers(): HistoryHandlers {
  return {
    copy: vi.fn(),
    createPatch: vi.fn(),
    get: vi.fn(),
    showFileAtRevision: vi.fn(),
    compareWithLocal: vi.fn(),
    showDiff: vi.fn(),
    showAllAffected: vi.fn(),
    annotate: vi.fn(),
    showInGitLog: vi.fn(),
    revert: vi.fn(),
    newTag: vi.fn(),
  };
}

// Each entry as `label` (enabled) or `(label)` (disabled), separators as `—`.
const shape = (items: MenuItem[]) =>
  items.map((item) => {
    if (item === null) return "—";
    if ("header" in item) return `# ${item.header}`;
    return item.disabled ? `(${item.label})` : item.label;
  });

const find = (items: MenuItem[], label: string) =>
  items.find((item): item is MenuAction => item !== null && "label" in item && item.label === label)!;

describe("file_history_menu_in_241_order", () => {
  test("one row", () => {
    expect(shape(historyMenu([commit(HASH_A)], handlers()))).toEqual([
      "Copy Revision Number",
      "Create Patch...",
      "—",
      "Get",
      "Show File at Revision",
      "Compare with Local",
      "Show Diff",
      "Show All Affected Files",
      "Annotate",
      "Show in Git Log",
      "—",
      "Revert Commit",
      "—",
      "New Tag…",
    ]);
  });

  test("two rows: Compare, and the one-row entries off", () => {
    const items = historyMenu([commit(HASH_B), commit(HASH_A)], handlers());
    expect(shape(items)).toEqual([
      "Copy Revision Number",
      "Create Patch...",
      "—",
      "(Get)",
      "(Show File at Revision)",
      "(Compare with Local)",
      "Compare",
      "(Show All Affected Files)",
      "(Annotate)",
      "(Show in Git Log)",
      "—",
      "Revert Commits",
      "—",
      "(New Tag…)",
    ]);
  });

  test("Copy Revision Number copies the oldest first; a merge is not reverted", () => {
    const actions = handlers();
    const items = historyMenu([commit(HASH_B, { parentCount: 2 }), commit(HASH_A)], actions);
    find(items, "Copy Revision Number").run?.();
    expect(actions.copy).toHaveBeenCalledWith(`${HASH_A} ${HASH_B}`);
    expect(find(items, "Revert Commits").disabled).toBe(true);
  });

  test("without a patch or the branch operations those entries are off", () => {
    const { createPatch: _patch, revert: _revert, newTag: _tag, ...rest } = handlers();
    const items = historyMenu([commit(HASH_A)], rest);
    expect(find(items, "Create Patch...").disabled).toBe(true);
    expect(find(items, "Revert Commit").disabled).toBe(true);
    expect(find(items, "New Tag…").disabled).toBe(true);
  });
});

// `FileHistoryModel.createRevision` gives `VcsFileRevision.NULL` for the row that deleted the file:
// `Get` and `Show File at Revision` have nothing to take, the rest still works.
test("a_deleted_row_disables_get_and_show_file_at_revision", () => {
  const actions = handlers();
  const deleted = commit(HASH_A, { entry: { path: "src/a.txt", deleted: true } });
  const items = historyMenu([deleted], actions);

  expect(find(items, "Get").disabled).toBe(true);
  expect(find(items, "Show File at Revision").disabled).toBe(true);
  // `AnnotateRevisionFromHistoryAction`: the same empty revision.
  expect(find(items, "Annotate").disabled).toBe(true);
  expect(find(items, "Compare with Local").disabled).toBe(false);
  find(items, "Compare with Local").run?.();
  expect(actions.compareWithLocal).toHaveBeenCalledWith(deleted);
  // The row of a deletion compares its parent with nothing.
  const pair = historyPair(
    { hash: HASH_A, path: "src/a.txt", after: null, before: [{ hash: HASH_B, path: "src/a.txt" }] },
    true,
  );
  expect(pair).toEqual({
    left: { revision: HASH_B, path: "src/a.txt", title: HASH_B },
    right: { revision: null, path: "src/a.txt", title: HASH_A },
  });
});

test("history_pairs_follow_the_rows_paths_and_merges_have_three_sides", () => {
  const renamed = historyPair(
    {
      hash: HASH_A,
      path: "src/new.txt",
      after: { hash: HASH_A, path: "src/new.txt" },
      before: [{ hash: HASH_B, path: "src/old.txt" }],
    },
    false,
  );
  expect(renamed).toEqual({
    left: { revision: HASH_B, path: "src/old.txt", title: "bbbbbbbb" },
    right: { revision: HASH_A, path: "src/new.txt", title: "aaaaaaaa" },
  });
  const first = historyPair(
    { hash: HASH_A, path: "a.txt", after: { hash: HASH_A, path: "a.txt" }, before: [] },
    true,
  );
  expect(first.left).toEqual({ revision: null, path: "a.txt", title: "" });
  const merge = historyPair(
    {
      hash: HASH_A,
      path: "a.txt",
      after: { hash: HASH_A, path: "a.txt" },
      before: [
        { hash: HASH_B, path: "a.txt" },
        { hash: HASH_C, path: "a.txt" },
      ],
    },
    true,
  );
  expect([merge.left.revision, merge.center?.revision, merge.right.revision]).toEqual([
    HASH_B,
    HASH_A,
    HASH_C,
  ]);
  // `MergedChangeDiffRequestProvider`: a parent without the file leaves two sides.
  const oneParent = historyPair(
    {
      hash: HASH_A,
      path: "a.txt",
      after: { hash: HASH_A, path: "a.txt" },
      before: [{ hash: HASH_B, path: "a.txt" }, null],
    },
    true,
  );
  expect(oneParent.center).toBeUndefined();
  expect([oneParent.left.revision, oneParent.right.revision]).toEqual([HASH_B, HASH_A]);
});

test("annotate opens the row's file at its revision annotated", () => {
  const actions = handlers();
  const row = commit(HASH_A, { entry: { path: "src/old.txt", deleted: false } });

  find(historyMenu([row], actions), "Annotate").run?.();

  expect(actions.annotate).toHaveBeenCalledWith(row);
});

// `VcsHistoryActionsGroup`, `anchor="last"` (`g4x:520`): the last item of the menu.
test("each_menu_has_open_on_github_at_241s_place_history", () => {
  const hosting: MenuItem[] = [{ label: "Open on GitHub", run: vi.fn() }];
  const items = historyMenu([commit(HASH_A)], { ...handlers(), hosting });

  expect(shape(items).slice(-3)).toEqual(["—", "New Tag…", "Open on GitHub"]);
  expect(shape(historyMenu([commit(HASH_A)], handlers()))).not.toContain("Open on GitHub");
});
