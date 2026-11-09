import { EditorView } from "@codemirror/view";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Annotation, ViewerContent, ViewerTarget } from "../lib/api";
import ViewerWindow from "./ViewerWindow.svelte";
import { annotationOptions, DEFAULT_OPTIONS } from "./annotation";
import { viewerSelection } from "./window";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(), emit: vi.fn() }));

// jsdom does no layout: CodeMirror measures with this.
Range.prototype.getClientRects = () => [] as unknown as DOMRectList;

const HASH = "1234567890abcdef1234567890abcdef12345678";
const wrapped = (view: EditorView) => view.contentDOM.classList.contains("cm-lineWrapping");

const handlers = new Map<string, (payload: unknown) => void>();
let queue: ViewerTarget[] = [];
let contents: Record<string, ViewerContent | Error> = {};
// `viewer_stamp` of each local path: what `repo-changed` compares before reading it again.
let stamps: Record<string, string | null> = {};
// The other commands a test answers.
let more: Record<string, (args: never) => unknown> = {};

const target = (path: string, revision: string | null = null, line: number | null = null) => ({
  path,
  revision,
  line,
});

function mockCommands() {
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    const { path, revision } = (args ?? {}) as { path: string; revision: string | null };
    switch (command) {
      case "viewer_targets": {
        const taken = queue;
        queue = [];
        return taken;
      }
      case "viewer_content": {
        const content = contents[`${revision ?? ""}:${path}`];
        if (content instanceof Error) throw { kind: "failed", message: content.message };
        if (!content) throw new Error(`no content for ${revision}:${path}`);
        return content;
      }
      case "viewer_stamp":
        return stamps[path] ?? null;
      case "show_window":
      case "hide_window":
        return undefined;
      default:
        if (more[command]) return more[command](args as never);
        throw new Error(`unexpected command: ${command}`);
    }
  });
}

function calls(command: string) {
  return vi.mocked(invoke).mock.calls.filter(([name]) => name === command);
}

const tabs = () => within(screen.getByRole("tablist", { name: "Files" })).queryAllByRole("tab");
const titles = () => tabs().map((tab) => tab.textContent?.trim());
const active = () => tabs().find((tab) => tab.getAttribute("aria-selected") === "true");

function activeView(): EditorView {
  const dom = document.querySelector<HTMLElement>(".file:not(.hidden) .cm-editor");
  if (!dom) throw new Error("no editor");
  return EditorView.findFromDOM(dom) as EditorView;
}

// More targets arrive while the window is open: `viewer_open` leaves them and says so.
async function deliver(...targets: ViewerTarget[]) {
  queue.push(...targets);
  handlers.get("viewer-open")?.(null);
}

async function open(...targets: ViewerTarget[]) {
  queue = [...targets];
  render(ViewerWindow);
  await waitFor(() => expect(calls("show_window")).toHaveLength(1));
  await waitFor(() => expect(handlers.has("viewer-open")).toBe(true));
}

beforeEach(() => {
  localStorage.clear();
  annotationOptions.set(DEFAULT_OPTIONS);
  handlers.clear();
  queue = [];
  contents = {
    ":src/a.txt": { kind: "text", text: "one\ntwo\nthree\n", previewOf: null, previewSize: null },
    [`${HASH}:src/a.txt`]: { kind: "text", text: "old one\n", previewOf: null, previewSize: null },
    ":b.txt": { kind: "text", text: "b\n", previewOf: null, previewSize: null },
  };
  stamps = { "src/a.txt": "1", "b.txt": "1" };
  more = {};
  vi.mocked(invoke).mockReset();
  mockCommands();
  vi.mocked(listen).mockReset();
  vi.mocked(listen).mockImplementation(async (event, handler) => {
    handlers.set(event as string, (payload) =>
      handler({ event: event as string, id: 0, payload } as never),
    );
    return () => {};
  });
});

afterEach(cleanup);

test("tabs_reuse_the_same_path_and_revision_and_wrap", async () => {
  await open(target("src/a.txt"), target("src/a.txt", HASH));
  await waitFor(() => expect(titles()).toEqual(["a.txt", "a.txt (12345678)"]));
  expect(active()?.textContent?.trim()).toBe("a.txt (12345678)");

  await deliver(target("src/a.txt", null, 3));

  await waitFor(() => expect(active()?.textContent?.trim()).toBe("a.txt"));
  expect(titles()).toEqual(["a.txt", "a.txt (12345678)"]);
  expect(calls("viewer_content")).toHaveLength(2);
  await waitFor(() => expect(activeView().state.doc.toString()).toBe("one\ntwo\nthree\n"));
  // `SCROLL_TAB_LAYOUT_IN_EDITOR=false`: the tabs go on in rows.
  expect(screen.getByRole("tablist", { name: "Files" }).classList).toContain("rows");
  // The user's soft wrap is on, and the viewer is a place of its own.
  expect(wrapped(activeView())).toBe(true);
  // Every tab is read-only: the padlock with its tooltip (`IB:2367`).
  expect(tabs().map((tab) => tab.querySelector("[title='read-only']"))).not.toContain(null);
  expect(tabs()[0].getAttribute("title")).toBe("src/a.txt");
});

test("a_new_tab_goes_after_the_active_one", async () => {
  await open(target("src/a.txt"), target("b.txt"));
  await waitFor(() => expect(titles()).toEqual(["a.txt", "b.txt"]));

  await fireEvent.click(tabs()[0]);
  await deliver(target("src/a.txt", HASH));

  await waitFor(() => expect(titles()).toEqual(["a.txt", "a.txt (12345678)", "b.txt"]));
});

test("the_line_of_the_target_gets_the_caret_and_is_the_selection", async () => {
  await open(target("src/a.txt", null, 2));
  await waitFor(() => expect(activeView().state.doc.lines).toBe(4));

  await waitFor(() =>
    expect(viewerSelection()).toEqual({
      path: "src/a.txt",
      revision: null,
      startLine: 2,
      endLine: 2,
    }),
  );

  const view = activeView();
  view.dispatch({ selection: { anchor: 0, head: view.state.doc.line(3).from + 1 } });
  expect(viewerSelection()).toEqual({ path: "src/a.txt", revision: null, startLine: 1, endLine: 3 });
});

test("typing_changes_nothing", async () => {
  await open(target("src/a.txt"));
  await waitFor(() => expect(activeView().state.doc.lines).toBe(4));

  // In the middle of `two`, where Backspace, Delete and Enter would all change the text.
  const view = activeView();
  view.dispatch({ selection: { anchor: view.state.doc.line(2).from + 2 } });
  view.contentDOM.focus();
  for (const key of ["Backspace", "Delete", "Enter"]) {
    await fireEvent.keyDown(view.contentDOM, { key });
  }

  expect(view.state.doc.toString()).toBe("one\ntwo\nthree\n");
  expect(view.state.readOnly).toBe(true);
});

test("meta_w_closes_the_active_tab_and_the_last_one_hides_the_window", async () => {
  await open(target("src/a.txt"), target("b.txt"));
  await waitFor(() => expect(titles()).toEqual(["a.txt", "b.txt"]));

  await fireEvent.keyDown(window, { key: "w", metaKey: true });
  await waitFor(() => expect(titles()).toEqual(["a.txt"]));
  expect(active()?.textContent?.trim()).toBe("a.txt");
  expect(calls("hide_window")).toHaveLength(0);

  await fireEvent.click(screen.getByRole("button", { name: "Close a.txt" }));

  await waitFor(() => expect(tabs()).toHaveLength(0));
  expect(calls("hide_window")).toHaveLength(1);
  expect(viewerSelection()).toBeNull();
});

test("thirty_megabytes_give_a_preview_with_the_241_text", async () => {
  contents[":big.txt"] = {
    kind: "text",
    text: "start\n",
    previewOf: 31_457_280,
    previewSize: 2_560_000,
  };

  await open(target("big.txt"));

  expect(
    await screen.findByText(
      "The file is too large (31.46 MB). Showing a read-only preview of the first 2.56 MB.",
    ),
  ).toBeTruthy();
  await waitFor(() => expect(activeView().state.doc.toString()).toBe("start\n"));
});

test("a_png_is_binary", async () => {
  contents[":logo.png"] = { kind: "binary", size: 24, tooLarge: false };
  contents[":huge.zip"] = { kind: "binary", size: 31_457_280, tooLarge: true };

  await open(target("logo.png"), target("huge.zip"));

  // Rust says what is too large: the page has no copy of the limit.
  expect(await screen.findByText("Binary file huge.zip is too large (31.46 MB)")).toBeTruthy();
  await fireEvent.click(tabs()[0]);
  expect(await screen.findByText("logo.png is a binary file (24 B).")).toBeTruthy();
  expect(document.querySelector(".file:not(.hidden) .cm-editor")).toBeNull();
});

// `AbstractVcsVirtualFile.showLoadingContentFailedMessage`: the revision becomes `0`, the text
// empty, and an information dialog says why (`V:98-99`).
test("a_missing_revision_says_could_not_load_content", async () => {
  const missing = "0000000000000000000000000000000000000001";
  contents[`${missing}:a.txt`] = new Error(
    "Could not load content for file /repo/a.txt: fatal: Not a valid object name",
  );

  await open(target("a.txt", missing));

  const dialog = await screen.findByRole("dialog", { name: "Could Not Load Content" });
  expect(
    within(dialog).getByText(
      "Could not load content for file /repo/a.txt: fatal: Not a valid object name",
    ),
  ).toBeTruthy();
  expect(titles()).toEqual(["a.txt (0)"]);
  await waitFor(() => expect(activeView().state.doc.toString()).toBe(""));

  await fireEvent.click(within(dialog).getByRole("button", { name: "OK" }));
  expect(screen.queryByRole("dialog")).toBeNull();
});

const contentReads = () => calls("viewer_content").length;

test("local_tabs_read_the_disk_again_only_when_the_file_changed", async () => {
  await open(target("src/a.txt"), target("src/a.txt", HASH));
  await waitFor(() => expect(titles()).toHaveLength(2));
  await waitFor(() => expect(contentReads()).toBe(2));

  handlers.get("repo-changed")?.({ stash: false, shelf: false });
  await waitFor(() => expect(calls("viewer_stamp").length).toBeGreaterThanOrEqual(2));
  await new Promise((done) => setTimeout(done, 20));
  expect(contentReads()).toBe(2);

  contents[":src/a.txt"] = { kind: "text", text: "edited\n", previewOf: null, previewSize: null };
  stamps["src/a.txt"] = "2";
  handlers.get("repo-changed")?.({ stash: false, shelf: false });

  await waitFor(() => expect(contentReads()).toBe(3));
  expect(calls("viewer_content")[2][1]).toEqual({ path: "src/a.txt", revision: null });
  await fireEvent.click(tabs()[0]);
  await waitFor(() => expect(activeView().state.doc.toString()).toBe("edited\n"));
});

test("a_failed_local_read_says_so_once", async () => {
  contents[":src/a.txt"] = new Error("Could not load content for file /repo/src/a.txt: denied");
  await open(target("src/a.txt"));
  const dialog = await screen.findByRole("dialog", { name: "Could Not Load Content" });
  await fireEvent.click(within(dialog).getByRole("button", { name: "OK" }));

  stamps["src/a.txt"] = "2";
  handlers.get("repo-changed")?.({ stash: false, shelf: false });
  await waitFor(() => expect(contentReads()).toBe(2));
  await new Promise((done) => setTimeout(done, 20));
  expect(screen.queryByRole("dialog")).toBeNull();

  contents[":src/a.txt"] = { kind: "text", text: "back\n", previewOf: null, previewSize: null };
  stamps["src/a.txt"] = "3";
  handlers.get("repo-changed")?.({ stash: false, shelf: false });
  await waitFor(() => expect(activeView().state.doc.toString()).toBe("back\n"));
});

test("the_scroll_and_the_selection_stay_when_the_file_changes", async () => {
  contents[":src/a.txt"] = {
    kind: "text",
    text: "line\n".repeat(100),
    previewOf: null,
    previewSize: null,
  };
  await open(target("src/a.txt"));
  await waitFor(() => expect(activeView().state.doc.lines).toBe(101));
  const view = activeView();
  view.dispatch({ selection: { anchor: 12, head: 30 } });
  view.scrollDOM.scrollTop = 140;

  contents[":src/a.txt"] = {
    kind: "text",
    text: "LINE\n".repeat(100),
    previewOf: null,
    previewSize: null,
  };
  stamps["src/a.txt"] = "2";
  handlers.get("repo-changed")?.({ stash: false, shelf: false });

  await waitFor(() => expect(activeView().state.doc.toString()).toBe("LINE\n".repeat(100)));
  expect(activeView().state.selection.main).toMatchObject({ anchor: 12, head: 30 });
  expect(activeView().scrollDOM.scrollTop).toBe(140);
});

test("soft_wrap_is_remembered_for_the_viewer_place", async () => {
  await open(target("src/a.txt"));
  await waitFor(() => expect(wrapped(activeView())).toBe(true));

  await fireEvent.contextMenu(document.querySelector(".cm-gutters .cm-gutterElement") as HTMLElement);
  await fireEvent.click(await screen.findByRole("menuitem", { name: "Soft-Wrap" }));

  expect(wrapped(activeView())).toBe(false);
  expect(localStorage.getItem("diff.wrap.Viewer")).toBe("false");
});

// The editor popup's `Git ▸` (`Git.ContextMenu`), from the status read now: a modified local file.
// The comparisons open from here; Commit File… goes to the main window with the file. A file at a
// revision is not a local file: Current File has nothing to act on.
test("the_editor_menu_has_the_git_file_menu_of_the_local_file", async () => {
  more = {
    repo_status: () => [
      { path: "src/a.txt", oldPath: null, state: "modified", staged: false, unstaged: true },
    ],
    repo_state: () => ({ operation: null, branch: "main", label: "main", head: HASH }),
    branch_list: () => ({
      local: [{ name: "main", tracked: null }, { name: "other", tracked: null }],
      remote: [],
      recent: [],
      current: "main",
      previous: null,
    }),
    git_file_action: () => undefined,
    git_menu_current_file: () => undefined,
    compare_local_state: () => {
      throw new Error("Cannot get revision content");
    },
  };
  await open(target("src/a.txt"));
  await waitFor(() => activeView());

  await fireEvent.contextMenu(activeView().contentDOM);
  const menu = await screen.findByRole("menu", { name: "Editor" });
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Git" }));
  const git = await screen.findByRole("menu", { name: "Git" });
  const entries = [...git.querySelectorAll('[role="menuitem"]')].map((item) => [
    item.getAttribute("aria-label"),
    (item as HTMLButtonElement).disabled,
  ]);
  expect(entries).toEqual([
    ["Commit File…", false],
    ["Add", false],
    ["Annotate with Git Blame", false],
    ["Compare with the Same Repository Version", false],
    ["Compare with Revision...", false],
    ["Compare with Branch…", false],
    ["Show History", false],
    ["Show History for Selection…", false],
    ["Show Current Revision", false],
    ["Rollback...", false],
  ]);

  await fireEvent.click(within(git).getByRole("menuitem", { name: "Commit File…" }));
  await waitFor(() =>
    expect(calls("git_file_action")).toEqual([
      ["git_file_action", { action: { id: "CheckinFiles", paths: ["src/a.txt"] } }],
    ]),
  );

  // Show History opens in the main window's Log, which gets the file as Commit File… does.
  await fireEvent.contextMenu(activeView().contentDOM);
  await fireEvent.click(
    within(await screen.findByRole("menu", { name: "Editor" })).getByRole("menuitem", { name: "Git" }),
  );
  const again = await screen.findByRole("menu", { name: "Git" });
  await fireEvent.click(within(again).getByRole("menuitem", { name: "Show History" }));
  await waitFor(() =>
    expect(calls("git_file_action").at(-1)).toEqual([
      "git_file_action",
      { action: { id: "Vcs.ShowTabbedFileHistory", paths: ["src/a.txt"] } },
    ]),
  );

  // From `Git > Current File` while this window has the focus: the text of the failure.
  handlers.get("git-action")?.("Compare.SameVersion");
  expect(await screen.findByText("Cannot get revision content")).toBeTruthy();
  await fireEvent.click(screen.getByRole("button", { name: "OK" }));

  await fireEvent.focus(window);
  await waitFor(() =>
    expect(calls("git_menu_current_file").at(-1)?.[1]).toMatchObject({ title: "Current File" }),
  );
  const items = (calls("git_menu_current_file").at(-1)?.[1] as { items: { enabled: boolean }[] })
    .items;
  expect(items[0]).toEqual({ id: "CheckinFiles", text: "Commit File…", enabled: true });
});

// `SelectedBlockHistoryAction`: the lines of the selection (from 0, `Math.min`/`Math.max`) and the
// text shown open `History for Selection`; without a selection, the caret's line. A file at a
// revision has no local file to take it from.
test("show_history_for_selection_opens_the_window_with_the_selected_lines", async () => {
  more = {
    repo_status: () => [],
    repo_state: () => ({ operation: null, branch: "main", label: "main", head: HASH }),
    branch_list: () => ({ local: [], remote: [], recent: [], current: "main", previous: null }),
    git_menu_current_file: () => undefined,
    selection_history_open: () => undefined,
  };
  await open(target("src/a.txt"), target("src/a.txt", HASH));
  await waitFor(() => expect(titles()).toEqual(["a.txt", "a.txt (12345678)"]));
  const entry = async () => {
    await fireEvent.contextMenu(activeView().contentDOM);
    const menu = await screen.findByRole("menu", { name: "Editor" });
    await fireEvent.click(within(menu).getByRole("menuitem", { name: "Git" }));
    const git = await screen.findByRole("menu", { name: "Git" });
    return within(git).getByRole("menuitem", { name: "Show History for Selection…" });
  };

  await waitFor(() => activeView());
  expect(((await entry()) as HTMLButtonElement).disabled).toBe(true);
  await fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });

  await fireEvent.click(tabs()[0]);
  await waitFor(() => expect(activeView().state.doc.toString()).toBe("one\ntwo\nthree\n"));
  const view = activeView();
  // From the end of `three` back into `two`.
  view.dispatch({ selection: { anchor: 13, head: 5 } });
  await fireEvent.click(await entry());

  await waitFor(() =>
    expect(calls("selection_history_open")).toEqual([
      [
        "selection_history_open",
        {
          request: {
            path: "src/a.txt",
            startLine: 1,
            endLine: 2,
            localText: "one\ntwo\nthree\n",
            stream: 0,
          },
        },
      ],
    ]),
  );
});

// A Git menu entry reaches this window only for `Git > Current File` while it has the focus: any
// other id, or the menu used from another window, does nothing here, not even a read.
test("the_viewer_acts_on_a_git_action_only_for_current_file_while_focused", async () => {
  more = {
    git_file_action: () => undefined,
    git_menu_current_file: () => undefined,
  };
  await open(target("src/a.txt"));
  await waitFor(() => activeView());
  const reads = () => calls("repo_status").length;
  const before = reads();

  handlers.get("git-action")?.("ChangesView.Revert");
  handlers.get("git-action")?.("Git.ResolveConflicts");
  const focus = vi.spyOn(document, "hasFocus").mockReturnValue(false);
  handlers.get("git-action")?.("Compare.SameVersion");
  await new Promise((resolve) => setTimeout(resolve, 20));
  focus.mockRestore();

  expect(reads()).toBe(before);
  expect(calls("git_file_action")).toEqual([]);
  expect(calls("compare_local_state")).toEqual([]);
});

// Annotate (decision 7). `annotate_start` launches a task; its result arrives as `task-changed`.
const OLD = "1111111111111111111111111111111111111111";

function annotated(lines: (number | null)[], change: Partial<Annotation> = {}): Annotation {
  return {
    revision: HASH,
    path: "src/a.txt",
    commits: [
      {
        hash: HASH,
        path: "src/a.txt",
        author: "Ann Smith",
        email: "ann@example.com",
        authorTime: 1_577_872_800,
        committerTime: 1_577_872_800,
        subject: "change",
        message: "change",
        previous: { hash: OLD, path: "src/old.txt" },
        number: 2,
        orderColor: "#2f5194",
        authorColor: "#2f5194",
      },
    ],
    lines,
    origins: lines.map((_, index) => index),
    owners: lines.map(() => 0),
    suspicious: false,
    historySize: 2,
    line: null,
    ...change,
  };
}

// The status of the local file, for the Git file menu the viewer reads.
const trackedFile = {
  repo_status: () => [],
  repo_state: () => ({ operation: null, branch: "main", label: "main", head: HASH }),
  branch_list: () => ({ local: [], remote: [], recent: [], current: "main", previous: null }),
  git_menu_current_file: () => undefined,
};

let nextTask = 0;
// Each `annotate_start` gets a task id; `finish` answers the task.
function annotateTasks() {
  more.annotate_start = () => ++nextTask;
  more.task_cancel = () => undefined;
}

function finish(id: number, value: Annotation) {
  handlers.get("task-changed")?.({ id, title: "", text: "", fraction: null, state: "done", kind: null, value });
}

const annotationCells = () =>
  [...document.querySelectorAll<HTMLElement>(".file:not(.hidden) .cm-annotations .cm-annotation")].filter(
    (cell) => (cell.parentElement as HTMLElement).style.visibility !== "hidden",
  );
const lineNumbers = () =>
  document.querySelector(".file:not(.hidden) .cm-lineNumbers .cm-gutterElement:not([style*='hidden'])") as HTMLElement;

async function gutterMenu(): Promise<HTMLElement> {
  await fireEvent.contextMenu(lineNumbers());
  return screen.findByRole("menu", { name: "Editor gutter" });
}

test("the_gutter_menu_annotates_the_local_file_and_closes_it", async () => {
  more = { ...trackedFile };
  annotateTasks();
  await open(target("src/a.txt"));
  await waitFor(() => activeView());

  const menu = await gutterMenu();
  const entries = [...menu.querySelectorAll('[role="menuitem"], [role="separator"]')].map(
    (item) => item.getAttribute("aria-label") ?? "—",
  );
  expect(entries).toEqual(["Annotate with Git Blame", "—", "Soft-Wrap"]);
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Annotate with Git Blame" }));

  await waitFor(() => expect(calls("annotate_start")).toHaveLength(1));
  expect(calls("annotate_start")[0][1]).toEqual({
    request: { path: "src/a.txt", revision: null, localText: "one\ntwo\nthree\n", from: null },
  });
  finish(nextTask, annotated([0, null, 0]));
  await waitFor(() => expect(annotationCells()).toHaveLength(4));
  expect(annotationCells()[0].textContent).toContain("Smith");

  await fireEvent.click(within(await gutterMenu()).getByRole("menuitem", { name: "Close Annotations" }));
  await waitFor(() => expect(annotationCells()).toHaveLength(0));
});

// `annotate_refresh` answers each change of the local file or of the repository.
let refreshes: ((value: unknown) => void)[] = [];
function refreshTasks() {
  refreshes = [];
  more.annotate_refresh = () => new Promise((resolve) => refreshes.push(resolve));
}

async function annotateLocal(lines: (number | null)[]) {
  await open(target("src/a.txt"));
  await waitFor(() => activeView());
  await fireEvent.click(within(await gutterMenu()).getByRole("menuitem", { name: "Annotate with Git Blame" }));
  await waitFor(() => expect(calls("annotate_start")).toHaveLength(1));
  finish(nextTask, annotated(lines));
  await waitFor(() => expect(annotationCells()).toHaveLength(lines.length + 1));
}

// `VcsAnnotationLocalChangesListenerImpl`: an edit with the same base keeps the annotation and maps
// the new text again, without a new blame; the gutter is hidden until it is.
test("a_change_on_disk_recomputes_the_local_annotation", async () => {
  more = { ...trackedFile };
  annotateTasks();
  refreshTasks();
  await annotateLocal([0, 0, 0]);

  contents[":src/a.txt"] = { kind: "text", text: "one\nTWO\nthree\nfour\n", previewOf: null, previewSize: null };
  stamps["src/a.txt"] = "2";
  handlers.get("repo-changed")?.({ stash: false, shelf: false });
  await waitFor(() => expect(calls("annotate_refresh")).toHaveLength(1));
  expect(calls("annotate_refresh")[0][1]).toEqual({
    request: { path: "src/a.txt", revision: HASH, blamedPath: "src/a.txt", owners: [0, 0, 0], localText: "one\nTWO\nthree\nfour\n" },
  });
  // The new text is not shown under the old attribution.
  await waitFor(() => expect(annotationCells()).toHaveLength(0));

  refreshes[0]({ kind: "lines", lines: [0, null, 0, null], origins: [0, 1, 2, 3] });
  await waitFor(() => expect(annotationCells()).toHaveLength(5));
  expect(annotationCells()[1].textContent?.trim()).toBe("");
  expect(calls("annotate_start")).toHaveLength(1);
});

// `GitFileAnnotation.isBaseRevisionChanged` → `FileAnnotation.close()`: a commit from the terminal
// moves the base revision and closes the annotation; so does a check that fails.
test("a_commit_that_moves_the_base_closes_the_annotation", async () => {
  more = { ...trackedFile };
  annotateTasks();
  refreshTasks();
  await annotateLocal([0, 0, 0]);

  handlers.get("repo-changed")?.({ stash: false, shelf: false });
  await waitFor(() => expect(calls("annotate_refresh")).toHaveLength(1));
  refreshes[0]({ kind: "lines", lines: [0, 0, 0], origins: [0, 1, 2] });
  await new Promise((done) => setTimeout(done, 20));
  expect(annotationCells()).toHaveLength(4);

  handlers.get("repo-changed")?.({ stash: false, shelf: false });
  await waitFor(() => expect(calls("annotate_refresh")).toHaveLength(2));
  refreshes[1]({ kind: "closed" });
  await waitFor(() => expect(annotationCells()).toHaveLength(0));
  expect(within(await gutterMenu()).getByRole("menuitem", { name: "Annotate with Git Blame" })).toBeTruthy();
  await fireEvent.keyDown(window, { key: "Escape" });

  // A check that fails (the file removed by a commit) closes it too.
  await fireEvent.click(within(await gutterMenu()).getByRole("menuitem", { name: "Annotate with Git Blame" }));
  await waitFor(() => expect(calls("annotate_start")).toHaveLength(2));
  finish(nextTask, annotated([0, 0, 0]));
  await waitFor(() => expect(annotationCells()).toHaveLength(4));
  more.annotate_refresh = () => Promise.reject({ kind: "git", message: "fatal: no such path" });
  handlers.get("repo-changed")?.({ stash: false, shelf: false });
  await waitFor(() => expect(annotationCells()).toHaveLength(0));
  expect(screen.queryByRole("dialog")).toBeNull();
});

// `AnnotateToggleAction.doAnnotate` (241:139-147): line counts more than one apart show the notice
// `V:687` instead; `Display anyway` shows the annotation, `Hide` leaves it off.
test("a_suspicious_line_count_asks_before_showing", async () => {
  more = { ...trackedFile };
  annotateTasks();
  await open(target("src/a.txt", HASH));
  await waitFor(() => activeView());
  await fireEvent.click(within(await gutterMenu()).getByRole("menuitem", { name: "Annotate with Git Blame" }));
  await waitFor(() => expect(calls("annotate_start")).toHaveLength(1));
  finish(nextTask, annotated([0], { suspicious: true }));

  const notice = await screen.findByRole("alert");
  expect(notice.textContent).toContain(
    "Number of lines annotated by Git is not equal to number of lines in the file. Check file encoding and line separators.",
  );
  expect(annotationCells()).toHaveLength(0);
  await fireEvent.click(within(notice).getByRole("button", { name: "Display anyway" }));
  await waitFor(() => expect(annotationCells()).toHaveLength(2));
  expect(screen.queryByRole("alert")).toBeNull();

  await fireEvent.click(within(await gutterMenu()).getByRole("menuitem", { name: "Close Annotations" }));
  await fireEvent.click(within(await gutterMenu()).getByRole("menuitem", { name: "Annotate with Git Blame" }));
  await waitFor(() => expect(calls("annotate_start")).toHaveLength(2));
  finish(nextTask, annotated([0], { suspicious: true }));
  const again = await screen.findByRole("alert");
  expect(within(again).getByRole("button", { name: "Hide" }).title).toBe("Hide this notification");
  await fireEvent.click(within(again).getByRole("button", { name: "Hide" }));
  expect(screen.queryByRole("alert")).toBeNull();
  expect(within(await gutterMenu()).getByRole("menuitem", { name: "Annotate with Git Blame" })).toBeTruthy();
});

// A target that asks for annotations but is not a text (a binary from the history menu) leaves
// Annotate off; the preview of a larger file has no text editor to annotate (`V:945`).
test("a_binary_or_a_preview_is_not_annotated", async () => {
  more = { ...trackedFile, git_menu_current_file: () => undefined };
  annotateTasks();
  contents[`${HASH}:logo.png`] = { kind: "binary", size: 24, tooLarge: false };
  contents[":big.txt"] = { kind: "text", text: "start\n", previewOf: 31_457_280, previewSize: 2_560_000 };
  await open({ ...target("logo.png", HASH), annotate: true });
  await waitFor(() => expect(titles()).toEqual(["logo.png (12345678)"]));
  // Without an editor nothing in the page has the focus: the window does.
  const focus = vi.spyOn(document, "hasFocus").mockReturnValue(true);
  await fireEvent.focus(window);
  await waitFor(() =>
    expect(
      (calls("git_menu_current_file").at(-1)?.[1] as { items: { id: string; enabled: boolean }[] })?.items.find(
        (item) => item.id === "Annotate",
      ),
    ).toEqual({ id: "Annotate", text: "Annotate", enabled: false }),
  );
  focus.mockRestore();

  await deliver({ ...target("big.txt"), annotate: true });
  const dialog = await screen.findByRole("dialog", { name: "Annotate" });
  expect(within(dialog).getByText("Cannot create text editor for big.txt")).toBeTruthy();
  await fireEvent.click(within(dialog).getByRole("button", { name: "OK" }));
  const item = within(await gutterMenu()).getByRole("menuitem", { name: "Annotate" });
  expect((item as HTMLButtonElement).disabled).toBe(true);
  expect(calls("annotate_start")).toHaveLength(0);
});

// `ShowDiffFromAnnotation`: the commit's every file in the diff window, on the annotated one.
test("show_diff_opens_the_whole_commit_on_the_annotated_file", async () => {
  more = {
    ...trackedFile,
    commit_details: () => [{ hash: HASH, parents: [OLD], subject: "change", body: "" }],
    compare_changes_between: () => [
      { path: "src/a.txt", oldPath: "src/old.txt", status: "R100" },
      { path: "lib/b.txt", oldPath: null, status: "M" },
    ],
    compare_local_state: () => ({ root: "/repo" }),
    diff_window_open_request: () => undefined,
  };
  annotateTasks();
  await open(target("src/a.txt", HASH));
  await waitFor(() => activeView());
  await fireEvent.click(within(await gutterMenu()).getByRole("menuitem", { name: "Annotate with Git Blame" }));
  await waitFor(() => expect(calls("annotate_start")).toHaveLength(1));
  finish(nextTask, annotated([0]));
  await waitFor(() => expect(annotationCells()).toHaveLength(2));

  await fireEvent.contextMenu(annotationCells()[0]);
  await fireEvent.click(
    within(await screen.findByRole("menu", { name: "Annotations" })).getByRole("menuitem", { name: "Show Diff" }),
  );

  await waitFor(() => expect(calls("diff_window_open_request")).toHaveLength(1));
  expect(calls("compare_changes_between")[0][1]).toEqual({ left: OLD, right: HASH, paths: [] });
  const { request } = calls("diff_window_open_request")[0][1] as {
    request: { files: { right: { path: string } }[]; selected: number };
  };
  expect(request.files.map((pair) => pair.right.path)).toEqual(["src/a.txt", "lib/b.txt"]);
  expect(request.selected).toBe(0);
});

test("annotate_previous_revision_opens_the_parent_at_the_translated_line", async () => {
  more = { ...trackedFile };
  annotateTasks();
  contents[`${OLD}:src/old.txt`] = { kind: "text", text: "a\nb\nc\nd\n", previewOf: null, previewSize: null };
  await open(target("src/a.txt", HASH));
  await waitFor(() => activeView());
  await fireEvent.click(within(await gutterMenu()).getByRole("menuitem", { name: "Annotate with Git Blame" }));
  await waitFor(() => expect(calls("annotate_start")).toHaveLength(1));
  expect(calls("annotate_start")[0][1]).toEqual({
    request: { path: "src/a.txt", revision: HASH, localText: null, from: null },
  });
  finish(nextTask, annotated([0]));
  await waitFor(() => expect(annotationCells()).toHaveLength(2));

  await fireEvent.contextMenu(annotationCells()[0]);
  const popup = await screen.findByRole("menu", { name: "Annotations" });
  await fireEvent.click(within(popup).getByRole("menuitem", { name: "Annotate Previous Revision" }));

  await waitFor(() => expect(titles()).toEqual(["a.txt (12345678)", "old.txt (11111111)"]));
  await waitFor(() => expect(calls("annotate_start")).toHaveLength(2));
  expect(calls("annotate_start")[1][1]).toEqual({
    request: { path: "src/old.txt", revision: OLD, localText: null, from: { text: "old one\n", line: 1 } },
  });
  finish(nextTask, annotated([0, 0, 0, 0], { revision: OLD, path: "src/old.txt", line: 3 }));
  await waitFor(() => expect(annotationCells()).toHaveLength(5));
  const view = activeView();
  expect(view.state.doc.lineAt(view.state.selection.main.head).number).toBe(3);
});

test("annotate_from_another_window_opens_the_tab_annotated", async () => {
  more = { ...trackedFile };
  annotateTasks();
  await open(target("b.txt"));
  await waitFor(() => activeView());

  await deliver({ ...target("src/a.txt"), annotate: true });

  await waitFor(() => expect(calls("annotate_start")).toHaveLength(1));
  expect(calls("annotate_start")[0][1]).toMatchObject({ request: { path: "src/a.txt", localText: "one\ntwo\nthree\n" } });
  finish(nextTask, annotated([0, 0, 0]));
  await waitFor(() => expect(annotationCells()).toHaveLength(4));
});

test("an_annotation_that_fails_says_so_with_the_annotate_title", async () => {
  more = { ...trackedFile, annotate_start: () => ++nextTask, task_cancel: () => undefined };
  await open(target("src/a.txt"));
  await waitFor(() => activeView());
  await fireEvent.click(within(await gutterMenu()).getByRole("menuitem", { name: "Annotate with Git Blame" }));
  await waitFor(() => expect(calls("annotate_start")).toHaveLength(1));

  handlers.get("task-changed")?.({
    id: nextTask, title: "", text: "fatal: no such path", fraction: null, state: "failed", kind: "git", value: null,
  });

  const dialog = await screen.findByRole("dialog", { name: "Annotate" });
  expect(within(dialog).getByText("fatal: no such path")).toBeTruthy();
  expect(annotationCells()).toHaveLength(0);
});

test("an_untracked_file_cannot_be_annotated", async () => {
  more = {
    ...trackedFile,
    repo_status: () => [{ path: "src/a.txt", oldPath: null, state: "untracked", staged: false, unstaged: true }],
  };
  await open(target("src/a.txt"));
  await waitFor(() => activeView());

  const annotate = within(await gutterMenu()).getByRole("menuitem", { name: "Annotate" });

  expect((annotate as HTMLButtonElement).disabled).toBe(true);
});
