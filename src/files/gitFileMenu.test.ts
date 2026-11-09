import { describe, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { MenuAction, MenuItem } from "../lib/menu";
import {
  currentFileItems,
  currentFileTitle,
  fileActionRun,
  readFileContext,
  runInPlace,
  gitFileMenu,
  gitSubmenu,
  type FileTarget,
  type GitFileSelection,
  type GitFileState,
} from "./gitFileMenu";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const file = (path: string, state: FileTarget["state"] = null): FileTarget => ({
  path,
  dir: false,
  state,
});
const folder = (path: string, state: FileTarget["state"] = null): FileTarget => ({
  path,
  dir: true,
  state,
});

function facts(change: Partial<GitFileState> = {}): GitFileState {
  return {
    hasChanges: true,
    conflicts: false,
    hasCommits: true,
    branchesToCompare: true,
    run: vi.fn(),
    ...change,
  };
}

const inTree = (...files: FileTarget[]): GitFileSelection => ({ files, editor: false });
const inEditor = (...files: FileTarget[]): GitFileSelection => ({ files, editor: true });

// Each entry as `label` (enabled) or `(label)` (disabled), separators as `—`.
const shape = (items: MenuItem[]) =>
  items.map((item) => {
    if (item === null) return "—";
    if ("header" in item) return `# ${item.header}`;
    return item.disabled ? `(${item.label})` : item.label;
  });

const find = (items: MenuItem[], label: string) =>
  items.find((item): item is MenuAction => item !== null && "label" in item && item.label === label)!;

describe("file_several_and_folder_texts_and_enablement_as_241", () => {
  test("a tracked file: the one-file actions", () => {
    expect(shape(gitFileMenu(inTree(file("src/a.ts")), facts()))).toEqual([
      // `isActionEnabled`: a file without changes is not committed.
      "(Commit File…)",
      "(Add)",
      "—",
      "Annotate with Git Blame",
      "Compare with the Same Repository Version",
      "Compare with Revision...",
      "Compare with Branch…",
      "Show History",
      "Show Current Revision",
      "Rollback...",
    ]);
  });

  test("a modified file is committed and added", () => {
    const items = gitFileMenu(inTree(file("src/a.ts", "modified")), facts());
    expect(find(items, "Commit File…").disabled).toBe(false);
    expect(find(items, "Add").disabled).toBe(false);
    expect(find(items, "Add").shortcut).toBe("⌥⌘A");
  });

  test("an unversioned file: Add, but nothing that needs it in the repository", () => {
    expect(shape(gitFileMenu(inTree(file("new.txt", "untracked")), facts()))).toEqual([
      "Commit File…",
      "Add",
      "—",
      "(Annotate)",
      "(Compare with the Same Repository Version)",
      "(Compare with Revision...)",
      "Compare with Branch…",
      "(Show History)",
      "(Show Current Revision)",
      "Rollback...",
    ]);
  });

  test("an added file is not in the repository yet either", () => {
    const items = gitFileMenu(inTree(file("new.txt", "added")), facts());
    expect(find(items, "Compare with the Same Repository Version").disabled).toBe(true);
    expect(find(items, "Show Current Revision").disabled).toBe(true);
  });

  test("an ignored file is neither committed nor added", () => {
    const items = gitFileMenu(inTree(file("debug.log", "ignored")), facts());
    expect(find(items, "Commit File…").disabled).toBe(true);
    expect(find(items, "Add").disabled).toBe(true);
  });

  test("a_binary_file_cannot_be_annotated", () => {
    // `AnnotateLocalFileAction.isEnabled`: a binary file type, or a file the viewer read as binary.
    // Without a provider the entry keeps its own text.
    // Rust says a type is binary with each tree entry; the menu keeps no list of its own.
    const png = gitFileMenu(inTree({ ...file("logo.PNG"), binary: true }), facts());
    expect(find(png, "Annotate").disabled).toBe(true);
    const read = gitFileMenu(inEditor({ ...file("data.bin"), binary: true }), facts());
    expect(find(read, "Annotate").disabled).toBe(true);
    expect(find(gitFileMenu(inTree(file("notes.txt")), facts()), "Annotate with Git Blame").disabled).toBe(false);
  });

  test("a folder cannot be annotated, and says so", () => {
    const annotate = find(gitFileMenu(inTree(folder("src")), facts()), "Annotate");
    expect(annotate.disabled).toBe(true);
    // `annotate.cannot.annotate.dir` (`B:18`).
    expect(annotate.title).toBe("Cannot annotate directory");
  });

  test("the viewer says whether its tab can be annotated and is", () => {
    const run = vi.fn();
    const revisionTab = gitFileMenu(inEditor(), facts({ run, annotation: { can: true, on: true } }));
    const annotate = find(revisionTab, "Annotate with Git Blame");
    expect(annotate.disabled).toBe(false);
    expect(annotate.checked).toBe(true);
    annotate.run!();
    expect(run).toHaveBeenCalledWith("Annotate", []);
    const binaryTab = gitFileMenu(inEditor(), facts({ annotation: { can: false, on: false } }));
    expect(find(binaryTab, "Annotate").disabled).toBe(true);
  });

  test("several files: plural texts, the one-file actions off", () => {
    expect(
      shape(gitFileMenu(inTree(file("a.ts", "modified"), file("b.ts")), facts())),
    ).toEqual([
      "Commit Files…",
      "Add",
      "—",
      "(Annotate)",
      "(Compare with the Same Repository Version)",
      "(Compare with Revision...)",
      "(Compare with Branch…)",
      "(Show History)",
      "(Show Current Revision)",
      "Rollback...",
    ]);
  });

  test("a folder: Commit Directory…, compared through the Changes window", () => {
    expect(shape(gitFileMenu(inTree(folder("src")), facts()))).toEqual([
      "Commit Directory…",
      "Add",
      "—",
      "(Annotate)",
      "(Compare with the Same Repository Version)",
      "Compare with Revision...",
      "Compare with Branch…",
      "Show History",
      "(Show Current Revision)",
      "Rollback...",
    ]);
  });

  test("several folders, and a folder first: by the first selected path", () => {
    const items = gitFileMenu(inTree(folder("src"), file("a.ts")), facts());
    expect(find(items, "Commit Directories…").disabled).toBe(false);
    // `VcsContextUtil.selectedFile` is the first one: a folder.
    expect(find(items, "Compare with Revision...").disabled).toBe(false);
    expect(find(items, "Compare with Branch…").disabled).toBe(true);
  });

  test("the repository's root is a folder, compared only below it", () => {
    const items = gitFileMenu(inTree(folder("")), facts());
    expect(find(items, "Commit Directory…").disabled).toBe(false);
    expect(find(items, "Add").disabled).toBe(false);
    expect(find(items, "Compare with Revision...").disabled).toBe(true);
    expect(find(items, "Compare with Branch…").disabled).toBe(true);
    expect(currentFileTitle(inTree(folder("")))).toBe("Selected Directory");
  });

  test("an ignored folder is not added", () => {
    const items = gitFileMenu(inTree(folder("build", "ignored")), facts());
    expect(find(items, "Add").disabled).toBe(true);
    // `isActionEnabled`: a folder is committed, unless it is ignored.
    expect(find(items, "Commit Directory…").disabled).toBe(true);
  });

  test("Rollback... needs a change anywhere; Compare with Branch… a commit and a branch", () => {
    const items = gitFileMenu(
      inTree(file("a.ts")),
      facts({ hasChanges: false, branchesToCompare: false }),
    );
    expect(find(items, "Rollback...").disabled).toBe(true);
    expect(find(items, "Compare with Branch…").disabled).toBe(true);
    const fresh = gitFileMenu(inTree(file("a.ts")), facts({ hasCommits: false }));
    expect(find(fresh, "Compare with Branch…").disabled).toBe(true);
  });

  test("Resolve Conflicts… shows only with conflicts, after a separator", () => {
    const items = gitFileMenu(inTree(file("a.ts", "unmerged")), facts({ conflicts: true }));
    expect(shape(items).slice(-3)).toEqual(["Rollback...", "—", "Resolve Conflicts…"]);
  });

  test("Show History for Selection… is only in the editor", () => {
    const items = gitFileMenu(inEditor(file("src/a.ts")), facts());
    expect(shape(items).slice(7, 10)).toEqual([
      "Show History",
      "(Show History for Selection…)",
      "Show Current Revision",
    ]);
  });

  test("each entry runs its action with the files it applies to", () => {
    const state = facts({ conflicts: true });
    const selection = inTree(folder("src"), file("a.ts", "modified"));
    const items = gitFileMenu(selection, state);
    find(items, "Commit Directories…").run?.();
    expect(state.run).toHaveBeenLastCalledWith("CheckinFiles", selection.files);
    find(items, "Compare with Revision...").run?.();
    expect(state.run).toHaveBeenLastCalledWith("Compare.Selected", [selection.files[0]]);
    find(items, "Resolve Conflicts…").run?.();
    expect(state.run).toHaveBeenLastCalledWith("Git.ResolveConflicts", selection.files);
  });

  test("the Git submenu of the tree holds them", () => {
    const submenu = gitSubmenu(inTree(file("a.ts")), facts());
    expect(submenu.label).toBe("Git");
    expect(submenu.disabled).toBeFalsy();
    expect(shape(submenu.submenu ?? [])[0]).toBe("(Commit File…)");
  });
});

// `TabbedShowHistoryAction.isEnabled`: one file or folders in the repository, never several files or
// a file git does not have yet; it runs with the selection.
test("show_history_in_the_git_file_menu_is_enabled_now", () => {
  const enabled = (...files: FileTarget[]) =>
    !find(gitFileMenu(inTree(...files), facts()), "Show History").disabled;
  expect(enabled(file("src/a.ts"))).toBe(true);
  expect(enabled(file("src/a.ts", "modified"))).toBe(true);
  expect(enabled(folder("src"), folder("docs"))).toBe(true);
  expect(enabled(folder(""))).toBe(true);
  expect(enabled(file("a.ts"), file("b.ts"))).toBe(false);
  expect(enabled(folder("src"), file("a.ts"))).toBe(false);
  expect(enabled(folder(""), folder("src"))).toBe(false);
  expect(enabled(file("new.txt", "untracked"))).toBe(false);
  expect(enabled(file("new.txt", "added"))).toBe(false);
  const state = facts();
  const selection = inEditor(file("src/a.ts"));
  find(gitFileMenu(selection, state), "Show History").run?.();
  expect(state.run).toHaveBeenLastCalledWith("Vcs.ShowTabbedFileHistory", selection.files);
  expect(currentFileItems(selection, state)[6]).toEqual({
    id: "Vcs.ShowTabbedFileHistory",
    text: "Show History",
    enabled: true,
  });
});

// `SelectedBlockHistoryAction`: hidden outside an editor (`setEnabledAndVisible(false)`, 241:43-48),
// enabled on a local file of the repository whose tab has a text editor (`isEnabled`).
test("show_history_for_selection_is_enabled_with_a_viewer_selection", () => {
  const label = "Show History for Selection…";
  const entry = (selection: GitFileSelection, change: Partial<GitFileState> = {}) =>
    find(gitFileMenu(selection, facts({ textEditor: true, ...change })), label);
  expect(entry(inEditor(file("src/a.ts"))).disabled).toBe(false);
  expect(entry(inEditor(file("src/a.ts", "modified"))).disabled).toBe(false);
  // A file at a revision gives no local file; a binary or a preview has no text editor.
  expect(entry(inEditor()).disabled).toBe(true);
  expect(entry(inEditor(file("src/a.ts")), { textEditor: false }).disabled).toBe(true);
  // `fileInVcsByFileStatus`.
  expect(entry(inEditor(file("new.txt", "untracked"))).disabled).toBe(true);
  expect(entry(inEditor(file("new.txt", "added"))).disabled).toBe(true);
  expect(entry(inTree(file("src/a.ts")))).toBeUndefined();

  const state = facts({ textEditor: true });
  const selection = inEditor(file("src/a.ts"));
  find(gitFileMenu(selection, state), label).run?.();
  expect(state.run).toHaveBeenLastCalledWith("Vcs.ShowHistoryForBlock", selection.files);
  expect(currentFileItems(selection, state)[7]).toEqual({
    id: "Vcs.ShowHistoryForBlock",
    text: label,
    enabled: true,
  });
  expect(currentFileItems(inTree(file("src/a.ts")), state)[7].enabled).toBe(false);
});

describe("current_file_title_follows_the_selection", () => {
  test("`Current File` from the editor or with nothing selected", () => {
    expect(currentFileTitle(inEditor(file("a.ts")))).toBe("Current File");
    expect(currentFileTitle(inEditor())).toBe("Current File");
    expect(currentFileTitle(inTree())).toBe("Current File");
  });

  test("`Selected File(s)` and `Selected Directory(ies)` by the first path", () => {
    expect(currentFileTitle(inTree(file("a.ts")))).toBe("Selected File");
    expect(currentFileTitle(inTree(file("a.ts"), folder("src")))).toBe("Selected Files");
    expect(currentFileTitle(inTree(folder("src")))).toBe("Selected Directory");
    expect(currentFileTitle(inTree(folder("src"), file("a.ts"), file("b.ts")))).toBe(
      "Selected Directories",
    );
  });

  test("the main menu's entries are those of `Git.FileActions`, with their texts", () => {
    expect(currentFileItems(inTree(folder("src")), facts())).toEqual([
      { id: "CheckinFiles", text: "Commit Directory…", enabled: true },
      { id: "Git.Add", text: "Add", enabled: true },
      { id: "Annotate", text: "Annotate", enabled: false },
      { id: "Compare.SameVersion", text: "Compare with the Same Repository Version", enabled: false },
      { id: "Compare.Selected", text: "Compare with Revision...", enabled: true },
      { id: "Git.CompareWithBranch", text: "Compare with Branch…", enabled: true },
      { id: "Vcs.ShowTabbedFileHistory", text: "Show History", enabled: true },
      { id: "Vcs.ShowHistoryForBlock", text: "Show History for Selection…", enabled: false },
    ]);
  });

  test("with nothing selected every entry is off and Commit has no subject", () => {
    const items = currentFileItems(inTree(), facts());
    expect(items.every((item) => !item.enabled)).toBe(true);
    expect(items[0].text).toBe("Commit…");
    expect(items[2].text).toBe("Annotate");
  });
});

describe("the_native_entries_run_only_when_on", () => {
  test("an entry that is off, or not there, does nothing", () => {
    const state = facts();
    expect(fileActionRun(inTree(file("a.ts")), state, "CheckinFiles")).toBeNull();
    expect(fileActionRun(inTree(file("a.ts")), state, "Git.ResolveConflicts")).toBeNull();
    expect(fileActionRun(inTree(file("a.ts")), state, "Vcs.Show.Log")).toBeNull();
    fileActionRun(inTree(file("a.ts")), state, "Compare.SameVersion")?.();
    expect(state.run).toHaveBeenCalledWith("Compare.SameVersion", [file("a.ts")]);
  });

  test("the viewer reads the status of its file, the conflicts and the branches", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "repo_status") {
        return [
          { path: "a.ts", oldPath: null, state: "modified", staged: false, unstaged: true },
          { path: "b.ts", oldPath: null, state: "unmerged", staged: false, unstaged: true },
        ];
      }
      if (command === "repo_state") return { operation: null, branch: "main", label: "main", head: "h" };
      if (command === "branch_list") {
        return { local: [{ name: "main", tracked: null }], remote: [], recent: [], current: "main", previous: null };
      }
      throw new Error(command);
    });
    const run = vi.fn();

    const { selection, state } = await readFileContext("a.ts", run);

    expect(selection).toEqual({ files: [file("a.ts", "modified")], editor: true });
    expect(state).toEqual({
      hasChanges: true,
      conflicts: true,
      hasCommits: true,
      branchesToCompare: false,
      run,
    });
    expect((await readFileContext(null, run)).selection).toEqual({ files: [], editor: true });
    // What the viewer read of its file.
    const binary = await readFileContext("a.ts", run, true);
    expect(binary.selection.files).toEqual([{ ...file("a.ts", "modified"), binary: true }]);
  });
});

test("annotate from the tree opens the file in the viewer with its annotations", async () => {
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockResolvedValue(undefined);

  expect(await runInPlace("Annotate", [file("src/a.ts")])).toBe(true);

  expect(invoke).toHaveBeenCalledWith("viewer_open", {
    targets: [{ path: "src/a.ts", revision: null, line: null, annotate: true }],
  });
});
