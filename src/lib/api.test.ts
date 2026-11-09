import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  onDiffRequest,
  onGitAction,
  onGitFileAction,
  onViewerOpen,
  branchesContaining,
  commitChangesOf,
  commitChangesStart,
  commitDetails,
  fileSides,
  changelistsMoveHunks,
  commitChangesSelected,
  fileHunks,
  linesRollback,
  clipboardWrite,
  logAdjacentRows,
  logGraphClick,
  logGraphHover,
  logRowOf,
  logRecentFilters,
  logRefs,
  logRememberFilter,
  logUsers,
  logViewOpen,
  logViewSet,
  mergeBlocks,
  type CommitRequest,
  type ViewSpec,
  type LineSelection,
} from "./api";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn() }));

const selection: LineSelection[] = [
  { hunk: "h1", lines: null },
  { hunk: "h2", lines: [0, 2] },
];

describe("hunks and selection by lines", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    vi.mocked(invoke).mockResolvedValue(undefined);
  });

  it("requests the hunks of a file", async () => {
    await fileHunks("a.txt");
    expect(invoke).toHaveBeenCalledWith("file_hunks", { path: "a.txt" });
  });

  it("rolls back lines with the selection as is", async () => {
    await linesRollback("a.txt", selection);
    expect(invoke).toHaveBeenCalledWith("lines_rollback", { path: "a.txt", selection });
  });

  it("moves hunks to a list", async () => {
    await changelistsMoveHunks("a.txt", ["h1"], "list");
    expect(invoke).toHaveBeenCalledWith("changelists_move_hunks", {
      path: "a.txt",
      hunks: ["h1"],
      id: "list",
    });
  });

  it("commits with the selection and without the content of the partial files", async () => {
    const request: CommitRequest = {
      added: [],
      removed: [],
      partial: [],
      message: "partial",
      options: { amend: false, signOff: false, skipHooks: false, author: null, cleanup: false },
    };

    await commitChangesSelected(request, [{ path: "a.txt", selection }]);

    expect(invoke).toHaveBeenCalledWith("commit_changes_selected", {
      request,
      selections: [{ path: "a.txt", selection }],
    });
  });
});

describe("log sorting and folding", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    vi.mocked(invoke).mockResolvedValue({ viewId: 1, count: 3 });
  });

  it("changes the view", async () => {
    const summary = await logViewSet(1, { sort: "linear", longEdges: true });
    expect(invoke).toHaveBeenCalledWith("log_view_set", {
      viewId: 1,
      patch: { sort: "linear", longEdges: true },
    });
    expect(summary).toEqual({ viewId: 1, count: 3 });
  });

  it("asks what gets highlighted when hovering the graph", async () => {
    const target = { kind: "edge", up: 1, down: 2, style: "dotted" } as const;
    await logGraphHover(1, target);
    expect(invoke).toHaveBeenCalledWith("log_graph_hover", { viewId: 1, target });
  });

  it("requests the row of a target and those of its neighbors", async () => {
    await logRowOf(1, { kind: "hash", value: "abcd" });
    expect(invoke).toHaveBeenCalledWith("log_row_of", {
      viewId: 1,
      target: { kind: "hash", value: "abcd" },
    });
    await logAdjacentRows(1, 3, true);
    expect(invoke).toHaveBeenCalledWith("log_adjacent_rows", { viewId: 1, row: 3, parent: true });
  });

  it("copies through the backend", async () => {
    await clipboardWrite("text");
    expect(invoke).toHaveBeenCalledWith("clipboard_write", { text: "text" });
  });

  it("reports a click on the graph", async () => {
    await logGraphClick(1, { kind: "node", row: 7 });
    expect(invoke).toHaveBeenCalledWith("log_graph_click", {
      viewId: 1,
      target: { kind: "node", row: 7 },
    });
  });
});

describe("filtered views", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    vi.mocked(invoke).mockResolvedValue(undefined);
  });

  it("opens a view as a task of its tab", async () => {
    const spec: ViewSpec = {
      sort: "standard",
      longEdges: false,
      collapsed: false,
      filter: {
        branches: ["main"],
        users: [],
        since: null,
        until: null,
        paths: [],
        text: null,
        hashes: [],
        revisions: [],
        ranges: [],
      },
    };

    await logViewOpen(2, spec);

    expect(invoke).toHaveBeenCalledWith("log_view_open", { tab: 2, spec });
  });

  it("requests users and references", async () => {
    await logUsers();
    await logRefs();

    expect(invoke).toHaveBeenCalledWith("log_users");
    expect(invoke).toHaveBeenCalledWith("log_refs");
  });

  it("requests the sides against the chosen parent", async () => {
    const file = { path: "a.txt", oldPath: null, status: "M" };

    await fileSides("abc", file, "parent");
    await fileSides("abc", file, null);
    await fileSides("abc", file);

    const args = { hash: "abc", path: "a.txt", oldPath: null };
    expect(invoke).toHaveBeenCalledWith("file_sides", { ...args, base: "parent", noBase: false });
    expect(invoke).toHaveBeenCalledWith("file_sides", { ...args, base: null, noBase: true });
    expect(invoke).toHaveBeenCalledWith("file_sides", { ...args, base: null, noBase: false });
  });

  it("requests the details and the changes of a commit", async () => {
    await commitDetails(["abc"]);
    await branchesContaining("abc");
    await commitChangesOf("abc", "parents");
    await commitChangesStart(["abc", "def"], 50000);

    expect(invoke).toHaveBeenCalledWith("commit_details", { hashes: ["abc"] });
    expect(invoke).toHaveBeenCalledWith("branches_containing", { hash: "abc" });
    expect(invoke).toHaveBeenCalledWith("commit_changes_of", { hash: "abc", mode: "parents" });
    expect(invoke).toHaveBeenCalledWith("commit_changes_start", {
      hashes: ["abc", "def"],
      limit: 50000,
    });
  });

  it("reads and saves the recents of the repository", async () => {
    await logRecentFilters();
    await logRememberFilter("Branch", ["main"]);

    expect(invoke).toHaveBeenCalledWith("log_recent_filters");
    expect(invoke).toHaveBeenCalledWith("log_remember_filter", {
      filter: "Branch",
      values: ["main"],
    });
  });
});

describe("three-way merge", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    vi.mocked(invoke).mockResolvedValue([]);
  });

  it("recomputes the blocks with another policy without rereading", async () => {
    await mergeBlocks("a\n", "b\n", "c\n", "ignore");

    expect(invoke).toHaveBeenCalledWith("merge_blocks", {
      left: "a\n",
      base: "b\n",
      right: "c\n",
      policy: "ignore",
    });
  });
});

// `emit_to(label, …)` also reaches a listener registered without a target (`{ kind: "Any" }`,
// tauri `match_any_or_filter`): the Git actions sent to one window are heard by that window only.
describe("a_targeted_git_action_reaches_its_own_window_only", () => {
  const targetOf = (event: string) =>
    vi.mocked(listen).mock.calls.filter(([name]) => name === event).at(-1)?.[2];

  it("the main window listens as main, the viewer as viewer", async () => {
    vi.mocked(listen).mockClear();
    window.history.replaceState(null, "", "/index.html");
    await onGitAction(() => {});
    expect(targetOf("git-action")).toEqual({ target: "main" });
    await onGitFileAction(() => {});
    expect(targetOf("git-file-action")).toEqual({ target: "main" });

    window.history.replaceState(null, "", "/index.html?window=viewer");
    await onGitAction(() => {});
    expect(targetOf("git-action")).toEqual({ target: "viewer" });
    window.history.replaceState(null, "", "/");
  });

  // `viewer_open` sends `viewer-open` to the viewer and `diff_open` sends `diff-request` to the
  // diff window (`emit_to`): another window that imports these helpers does not hear them.
  it("the viewer and the diff window hear their own requests only", async () => {
    vi.mocked(listen).mockClear();
    window.history.replaceState(null, "", "/index.html?window=viewer");
    await onViewerOpen(() => {});
    expect(targetOf("viewer-open")).toEqual({ target: "viewer" });

    window.history.replaceState(null, "", "/index.html?window=diff");
    await onDiffRequest(() => {});
    expect(targetOf("diff-request")).toEqual({ target: "diff" });
    window.history.replaceState(null, "", "/");
  });
});
