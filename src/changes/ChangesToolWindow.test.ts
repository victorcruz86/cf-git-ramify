import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { get } from "svelte/store";
import { invoke } from "@tauri-apps/api/core";
import type { ChangedFile } from "../lib/api";
import {
  forgetToolWindows,
  onWindowActivationRequested,
  registeredToolWindows,
} from "../shell/toolWindows";
import { compareWithLocal, showDiffWithWorkingTree } from "../compare/actions";
import { dismiss, notifications } from "../lib/notifications";
import { tasks } from "../lib/tasks";
import ChangesToolWindow from "./ChangesToolWindow.svelte";
import { answerGetQuestion, getQuestion } from "./get";
import { changesTab, showChanges } from "./tab";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn() }));

// jsdom has no ResizeObserver, which the changes tree needs.
vi.stubGlobal(
  "ResizeObserver",
  class {
    observe() {}
    unobserve() {}
    disconnect() {}
  },
);

const OLD = "a".repeat(40);
const NEW = "b".repeat(40);
const HEAD = "c".repeat(40);

const file = (path: string, status = "M", oldPath: string | null = null): ChangedFile => ({
  path,
  oldPath,
  status,
});

let commands: Record<string, (args: never) => unknown>;

beforeEach(() => {
  forgetToolWindows();
  commands = {
    repo_state: () => ({ operation: null, branch: "main", label: "main", head: HEAD }),
    diff_window_open_request: () => undefined,
  };
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    const handler = commands[command];
    if (!handler) throw new Error(`unexpected command: ${command}`);
    return handler(args as never);
  });
});

afterEach(() => {
  cleanup();
  get(notifications).forEach(({ id }) => dismiss(id));
});

const calls = (command: string) =>
  vi.mocked(invoke).mock.calls.filter(([name]) => name === command).map(([, args]) => args);

function mount() {
  render(ChangesToolWindow, { repo: "/work/repo", active: true });
}

const toolbar = () => screen.getByRole("toolbar", { name: "Changes" });
const toolbarButtons = () =>
  within(toolbar())
    .getAllByRole("button")
    .map((button) => button.getAttribute("aria-label"));

const row = (name: string) => screen.getByRole("treeitem", { name });

async function popupLabels(name: string) {
  await fireEvent.contextMenu(row(name));
  const menu = await screen.findByRole("menu");
  const labels = within(menu)
    .getAllByRole("menuitem")
    .map((item) => item.getAttribute("aria-label"));
  await fireEvent.keyDown(menu, { key: "Escape" });
  return labels;
}

test("a_new_changes_tab_replaces_the_open_one", async () => {
  const activated: string[] = [];
  const stop = onWindowActivationRequested((id) => activated.push(id));
  mount();

  await showChanges({
    title: `Changes Between ${OLD.slice(0, 8)} and local version`,
    left: OLD,
    right: "local",
    files: [file("src/a.txt"), file("b.txt", "A")],
  });
  await screen.findByRole("treeitem", { name: "a.txt" });
  await showChanges({
    title: `Paths Affected by Commit ${NEW.slice(0, 8)}`,
    left: OLD,
    right: NEW,
    files: [file("c.txt", "D")],
    message: "Fix the bug\n\nWith details",
  });

  await screen.findByRole("treeitem", { name: "c.txt" });
  expect(screen.getAllByRole("tab").map((tab) => tab.textContent)).toEqual([
    `Paths Affected by Commit ${NEW.slice(0, 8)}`,
  ]);
  expect(screen.queryByRole("treeitem", { name: "a.txt" })).toBeNull();
  expect(screen.getByLabelText("Commit message").textContent).toBe("Fix the bug\n\nWith details");
  // Registered on first use, and brought to the front each time.
  expect(get(registeredToolWindows)).toEqual(["changes"]);
  expect(activated).toEqual(["changes", "changes"]);
  stop();
});

test("refresh_and_get_appear_only_with_a_local_side", async () => {
  mount();
  await showChanges({ title: "Between", left: OLD, right: NEW, files: [file("a.txt")] });
  await screen.findByRole("treeitem", { name: "a.txt" });

  expect(toolbarButtons()).toEqual(["Show Diff", "Group By"]);
  expect(await popupLabels("a.txt")).toEqual(["Show Diff", "Create Patch..."]);

  await showChanges({
    title: "With local",
    left: OLD,
    right: "local",
    files: [file("a.txt")],
    paths: ["a.txt"],
  });
  await screen.findByRole("tab", { name: "With local" });

  expect(toolbarButtons()).toEqual(["Refresh", "Show Diff", "Get", "Group By"]);
  // `Get` needs a selection (`GetVersionActionProvider.update`).
  expect(screen.getByRole("button", { name: "Get" }).hasAttribute("disabled")).toBe(true);
  expect(await popupLabels("a.txt")).toEqual(["Show Diff", "Create Patch...", "Get"]);
  expect(screen.getByRole("button", { name: "Get" }).hasAttribute("disabled")).toBe(false);
});

test("double click shows the chosen file in the diff window with 241's titles", async () => {
  mount();
  await showChanges({
    title: "With local",
    left: OLD,
    right: "local",
    files: [file("src/new.txt", "R", "src/old.txt"), file("added.txt", "A"), file("gone.txt", "D")],
  });

  await fireEvent.click(await screen.findByRole("treeitem", { name: "new.txt" }));
  await fireEvent.dblClick(row("new.txt"));
  await fireEvent.click(row("added.txt"));
  await fireEvent.click(row("gone.txt"), { metaKey: true });
  await fireEvent.keyDown(row("gone.txt"), { key: "d", metaKey: true });

  expect(calls("diff_window_open_request")).toEqual([
    {
      request: {
        kind: "revisions",
        title: "Changes: new.txt",
        files: [
          {
            left: { revision: OLD, path: "src/old.txt", title: OLD },
            right: { revision: "local", path: "src/new.txt", title: "Your version" },
          },
        ],
      },
    },
    {
      request: {
        kind: "revisions",
        title: "Changes: added.txt",
        files: [
          {
            left: { revision: null, path: "added.txt", title: "Base version" },
            right: { revision: "local", path: "added.txt", title: "Your version" },
          },
          {
            left: { revision: OLD, path: "gone.txt", title: OLD },
            right: { revision: null, path: "gone.txt", title: "Your version" },
          },
        ],
      },
    },
  ]);
});

test("Get writes the left revision at its old path with the HEAD of the list, then refreshes", async () => {
  let listed = [file("src/new.txt", "R", "src/old.txt")];
  commands.compare_changes_with = () => listed;
  const seen = [
    { path: "src/new.txt", hash: "1".repeat(64) },
    { path: "src/old.txt", hash: null },
  ];
  commands.revision_get_check = () => ({
    root: "/work/repo",
    head: HEAD,
    modified: ["src/new.txt"],
    seen,
  });
  commands.revision_get = () => {
    listed = [];
    return { copy: "copy-1", written: ["src/new.txt"], changed: [] };
  };
  mount();
  await compareWithLocal(OLD, [{ path: "src" }, { path: "other.txt" }]);
  await fireEvent.click(await screen.findByRole("treeitem", { name: "new.txt" }));

  await fireEvent.click(screen.getByRole("button", { name: "Get" }));
  await waitFor(() => expect(get(getQuestion)?.request.kind).toBe("confirm"));
  answerGetQuestion(true);

  await screen.findByText("No differences");
  expect(calls("revision_get")).toEqual([
    {
      items: [{ path: "src/new.txt", revision: OLD, oldPath: "src/old.txt" }],
      expectedHead: HEAD,
      seen,
    },
  ]);
  // The list is read again for the same paths.
  expect(calls("compare_changes_with")).toEqual([
    { revision: OLD, paths: ["src", "other.txt"] },
    { revision: OLD, paths: ["src", "other.txt"] },
  ]);
  expect(get(changesTab)?.title).toBe(`Changes Between ${OLD.slice(0, 8)} and local version in src, other.txt`);
});

test("a refresh that fails shows its error and an empty list", async () => {
  commands.compare_changes_with = () => {
    throw { kind: "failed", message: "git diff failed: bad revision" };
  };
  mount();
  await showChanges({ title: "With local", left: OLD, right: "local", files: [file("a.txt")] });
  await screen.findByRole("treeitem", { name: "a.txt" });

  await fireEvent.keyDown(screen.getByRole("tree"), { key: "r", metaKey: true });

  await screen.findByText("git diff failed: bad revision");
  expect(get(changesTab)?.files).toEqual([]);
});

test("opening another repository forgets the tab", async () => {
  mount();
  await showChanges({ title: "Between", left: OLD, right: NEW, files: [file("a.txt")] });
  await screen.findByRole("treeitem", { name: "a.txt" });

  forgetToolWindows();

  await waitFor(() => expect(screen.queryByRole("treeitem", { name: "a.txt" })).toBeNull());
  expect(get(changesTab)).toBeNull();
});

// `GitShowDiffWithBranchPanel`: `git diff -R` puts the working tree on the left (`loadTotalDiff`),
// whose `CurrentContentRevision` has no number, so its title is `Base version`. No Refresh and no
// `Get`: `CompareBranchesDiffPanel` has Get from Branch, and Show Diff alone in its popup.
test("show_diff_with_working_tree_fills_the_changes_window", async () => {
  const revision = { hash: NEW, commitTime: 1, authorTime: 1, author: "Ana", message: "side" };
  commands.compare_resolve = () => revision;
  commands.compare_changes_from_working_tree = () => [
    file("kept.txt"),
    file("only-side.txt", "A"),
    file("dir/old.txt", "R100", "dir/new.txt"),
  ];
  mount();

  await showDiffWithWorkingTree("side");

  await screen.findByRole("treeitem", { name: "kept.txt" });
  expect(calls("compare_resolve")).toEqual([{ kind: "branch", name: "side" }]);
  expect(calls("compare_changes_from_working_tree")).toEqual([{ revision: NEW }]);
  expect(screen.getAllByRole("tab").map((tab) => tab.textContent)).toEqual([
    "Changes Between side and Current Working Tree",
  ]);
  expect(toolbarButtons()).toEqual(["Show Diff", "Get from Branch", "Group By"]);
  expect(await popupLabels("kept.txt")).toEqual(["Show Diff", "Get from Branch"]);
  expect(screen.getByText(/^Difference between current working tree on/).textContent?.replace(/\s+/g, " ")).toBe(
    "Difference between current working tree on main and files in side: Swap branches",
  );

  await fireEvent.click(row("kept.txt"));
  await fireEvent.click(row("only-side.txt"), { metaKey: true });
  await fireEvent.click(row("old.txt"), { metaKey: true });
  await fireEvent.keyDown(row("kept.txt"), { key: "d", metaKey: true });
  expect(calls("diff_window_open_request")).toEqual([
    {
      request: {
        kind: "revisions",
        title: "Changes: old.txt",
        files: [
          {
            left: { revision: "local", path: "dir/new.txt", title: "Base version" },
            right: { revision: NEW, path: "dir/old.txt", title: NEW },
          },
          {
            left: { revision: "local", path: "kept.txt", title: "Base version" },
            right: { revision: NEW, path: "kept.txt", title: NEW },
          },
          {
            left: { revision: null, path: "only-side.txt", title: "Base version" },
            right: { revision: NEW, path: "only-side.txt", title: NEW },
          },
        ],
      },
    },
  ]);

  commands.compare_changes_from_working_tree = () => [];
  await showDiffWithWorkingTree("side");
  await screen.findByText("No differences");

  commands.compare_changes_from_working_tree = () => {
    throw "bad object";
  };
  await expect(showDiffWithWorkingTree("side")).rejects.toThrow(
    "Could not load diff with side: bad object",
  );
  commands.compare_resolve = () => null;
  await expect(showDiffWithWorkingTree("gone")).rejects.toThrow("Could not load diff with gone:");
});

// `LoadingCommittedChangeListPanel.setChangeList(changeList, toSelect)`: Show All Affected Files
// opens with the history's file chosen.
test("a_tab_can_open_with_a_file_chosen", async () => {
  mount();
  await showChanges({
    title: "Paths Affected by Commit",
    left: OLD,
    right: NEW,
    files: [file("src/a.txt"), file("src/b.txt")],
    select: "src/b.txt",
  });

  await screen.findByRole("treeitem", { name: "b.txt" });
  expect(row("b.txt").getAttribute("aria-selected")).toBe("true");
  expect(row("a.txt").getAttribute("aria-selected")).toBe("false");
});

// `CompareBranchesDiffPanel.swapSides`: the label, the list and the sides turn round, and the
// setting stays for the next one; `GetVersionActionProvider` (`Get from Branch`) writes the
// branch's side of each change over the working tree, with `Get from Branch '<name>'` as the
// confirmation's title, then reads the list again (`copyChangesFromBranch`, `reloadTotalDiff`).
test("swap_branches_and_get_from_branch", async () => {
  localStorage.clear();
  const revision = { hash: NEW, commitTime: 1, authorTime: 1, author: "Ana", message: "side" };
  commands.compare_resolve = () => revision;
  let listed = [
    file("kept.txt"),
    file("only-side.txt", "A"),
    file("only-here.txt", "D"),
    file("dir/old.txt", "R100", "dir/new.txt"),
  ];
  commands.compare_changes_from_working_tree = () => listed;
  commands.revision_get_check = () => ({ root: "/work/repo", head: HEAD, modified: ["kept.txt"], seen: [] });
  let running: string[] = [];
  commands.revision_get = () => {
    running = get(tasks).map((task) => task.title);
    listed = [];
    return { copy: "copy-1", written: ["kept.txt"], changed: [] };
  };
  mount();
  await showDiffWithWorkingTree("side");
  await screen.findByRole("treeitem", { name: "kept.txt" });

  await fireEvent.click(screen.getByRole("button", { name: "Swap branches" }));
  expect(screen.getByText(/^Difference between files in/).textContent?.replace(/\s+/g, " ")).toBe(
    "Difference between files in side and current working tree on main: Swap branches",
  );
  await fireEvent.click(row("only-side.txt"));
  await fireEvent.keyDown(row("only-side.txt"), { key: "d", metaKey: true });
  expect(calls("diff_window_open_request").at(-1)).toEqual({
    request: {
      kind: "revisions",
      title: "Changes: only-side.txt",
      files: [
        {
          left: { revision: NEW, path: "only-side.txt", title: NEW },
          right: { revision: null, path: "only-side.txt", title: "Your version" },
        },
      ],
    },
  });
  // The next comparison opens swapped too.
  await showDiffWithWorkingTree("side");
  await screen.findByText(/^Difference between files in/);

  await fireEvent.click(await screen.findByRole("treeitem", { name: "kept.txt" }));
  await fireEvent.click(row("only-side.txt"), { metaKey: true });
  await fireEvent.click(row("only-here.txt"), { metaKey: true });
  await fireEvent.click(row("new.txt"), { metaKey: true });
  await fireEvent.click(screen.getByRole("button", { name: "Get from Branch" }));
  await waitFor(() => expect(get(getQuestion)?.request).toEqual({
    kind: "confirm",
    message: "File /work/repo/kept.txt has been locally modified.",
    ok: "Overwrite Modified File",
    title: "Get from Branch 'side'",
  }));
  answerGetQuestion(true);

  await screen.findByText("No differences");
  expect(calls("revision_get")).toEqual([
    {
      items: [
        { path: "dir/new.txt", revision: NEW, oldPath: "dir/old.txt" },
        { path: "kept.txt", revision: NEW, oldPath: null },
        { path: "only-here.txt", revision: NEW, oldPath: null },
        { path: "only-side.txt", revision: NEW, oldPath: null },
      ],
      expectedHead: HEAD,
      seen: [],
    },
  ]);
  expect(calls("compare_changes_from_working_tree")).toHaveLength(3);
  // `CompareBranchesDiffPanel.java:244` (`DvcsBundle:94`).
  expect(running).toEqual(["Loading Content from Branch…"]);
  localStorage.clear();
});
