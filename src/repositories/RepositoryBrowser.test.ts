import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { get } from "svelte/store";
import type { TreeEntry } from "../lib/api";
import { notifications } from "../lib/notifications";
import { forgetToolWindows, onWindowActivationRequested, registeredToolWindows } from "../shell/toolWindows";
import RepositoryBrowser from "./RepositoryBrowser.svelte";
import { repositoryTab, showRepositoryAtRevision } from "./tab";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn() }));

// jsdom has no ResizeObserver, which `bind:clientHeight` needs.
vi.stubGlobal(
  "ResizeObserver",
  class {
    observe() {}
    unobserve() {}
    disconnect() {}
  },
);

const ROOT = "/Users/me/rama";
const FIRST = "a1b2c3d4".repeat(5);
const SECOND = "e5f6a7b8".repeat(5);

const file = (path: string): TreeEntry => ({ name: path.split("/").pop()!, path, dir: false });
const folder = (path: string): TreeEntry => ({ name: path.split("/").pop()!, path, dir: true });

// What `tree_list_at` gives per revision and folder.
const revisions: Record<string, Record<string, TreeEntry[]>> = {
  [FIRST]: {
    "": [folder("src"), file("README.md")],
    src: [folder("src/lib"), file("src/a.txt"), file("src/b.txt")],
    "src/lib": [file("src/lib/c.ts")],
  },
  [SECOND]: {
    "": [folder("docs"), file("new.txt")],
    docs: [file("docs/x.md")],
  },
};

function calls(command: string) {
  return vi.mocked(invoke).mock.calls.filter(([name]) => name === command).map(([, args]) => args);
}

const listed = () => calls("tree_list_at") as { revision: string; dir: string }[];
const opened = () => calls("viewer_open").map((args) => (args as { targets: unknown }).targets);
const tree = () => screen.getByRole("tree", { name: "Repositories" });
const row = (name: string) => screen.getByRole("treeitem", { name });
const names = () => screen.queryAllByRole("treeitem").map((item) => item.getAttribute("aria-label"));
const title = () => screen.getByRole("tab").textContent;

async function key(key: string, init: KeyboardEventInit = {}) {
  await fireEvent.keyDown(tree(), { key, ...init });
}

async function expand(name: string) {
  await fireEvent.click(row(name));
  await key("ArrowRight");
}

async function mount(hash = FIRST) {
  showRepositoryAtRevision(hash);
  render(RepositoryBrowser, { repo: ROOT, active: true });
  await waitFor(() => expect(names().length).toBeGreaterThan(0));
}

async function popup(name: string) {
  await fireEvent.contextMenu(row(name));
  return screen.findByRole("menu", { name: "Repositories" });
}

beforeEach(() => {
  forgetToolWindows();
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    switch (command) {
      case "tree_list_at": {
        const { revision, dir } = args as { revision: string; dir: string };
        return revisions[revision]?.[dir] ?? [];
      }
      case "viewer_open":
      case "diff_window_open_request":
        return undefined;
      default:
        throw new Error(`unexpected command: ${command}`);
    }
  });
});

afterEach(cleanup);

test("a_second_revision_replaces_the_tab", async () => {
  const activated: string[] = [];
  const stop = onWindowActivationRequested((id) => activated.push(id));
  await mount(FIRST);

  // `RepositoryBrowser.showRepositoryBrowser`: registered on first use, then brought to the front.
  expect(get(registeredToolWindows)).toContain("repositories");
  expect(activated).toEqual(["repositories"]);
  // `B:1367`, and the root folder itself is not a row (`FileTreeModel` with one root, not shown).
  expect(screen.getAllByRole("tab").map((tab) => tab.textContent)).toEqual(["rama at a1b2c3d4"]);
  expect(names()).toEqual(["src", "README.md"]);
  expect(listed()).toEqual([{ revision: FIRST, dir: "" }]);
  // No toolbar.
  expect(screen.queryByRole("toolbar")).toBeNull();

  await expand("src");
  await waitFor(() => expect(names()).toEqual(["src", "lib", "a.txt", "b.txt", "README.md"]));

  // The same revision again reuses the tab as it is: nothing is read again.
  showRepositoryAtRevision(FIRST);
  await Promise.resolve();
  expect(names()).toEqual(["src", "lib", "a.txt", "b.txt", "README.md"]);
  expect(listed()).toHaveLength(2);
  const tab = get(repositoryTab);

  showRepositoryAtRevision(SECOND);
  await waitFor(() => expect(names()).toEqual(["docs", "new.txt"]));
  expect(screen.getAllByRole("tab").map((tab) => tab.textContent)).toEqual(["rama at e5f6a7b8"]);
  expect(get(repositoryTab)?.id).not.toBe(tab?.id);
  expect(listed().slice(2)).toEqual([{ revision: SECOND, dir: "" }]);
  expect(activated).toEqual(["repositories", "repositories", "repositories"]);
  // Nothing is selected in the new tab.
  expect(screen.queryAllByRole("treeitem", { selected: true })).toEqual([]);

  // Opening another repository forgets the window and its tab.
  forgetToolWindows();
  expect(get(repositoryTab)).toBeNull();
  stop();
});

test("enter_opens_the_file_at_that_revision_read_only", async () => {
  await mount();
  await expand("src");
  await waitFor(() => expect(names()).toContain("a.txt"));

  // Enter on a folder expands or collapses it (`FileSystemTreeImpl.performEnterAction(true)`).
  await fireEvent.click(row("src"));
  await key("Enter");
  expect(names()).toEqual(["src", "README.md"]);
  await key("Enter");
  await waitFor(() => expect(names()).toContain("a.txt"));
  expect(opened()).toEqual([]);

  // Enter acts on the first selected path (`getSelectionPath`), not the lead: a folder selected
  // first is expanded or collapsed, and nothing opens.
  await fireEvent.click(row("lib"));
  await fireEvent.click(row("a.txt"), { metaKey: true });
  await key("Enter");
  await waitFor(() => expect(names()).toContain("c.ts"));
  expect(opened()).toEqual([]);
  await fireEvent.click(row("lib"));
  await key("Enter");
  expect(names()).not.toContain("c.ts");

  // A file first: every selected file opens, each at the revision, in the read-only viewer, in the
  // order they were selected (a path selected again goes last).
  await fireEvent.click(row("a.txt"));
  await fireEvent.click(row("b.txt"), { metaKey: true });
  await fireEvent.click(row("lib"), { metaKey: true });
  await fireEvent.click(row("a.txt"), { metaKey: true });
  await fireEvent.click(row("a.txt"), { metaKey: true });
  await key("Enter");
  expect(opened()).toEqual([
    [
      { path: "src/b.txt", revision: FIRST, line: null },
      { path: "src/a.txt", revision: FIRST, line: null },
    ],
  ]);

  // A double click on a file opens it; on a folder it only expands or collapses it.
  await fireEvent.click(row("README.md"));
  await fireEvent.dblClick(row("README.md"));
  expect(opened()[1]).toEqual([{ path: "README.md", revision: FIRST, line: null }]);
  await fireEvent.click(row("lib"));
  await fireEvent.dblClick(row("lib"));
  await waitFor(() => expect(names()).toContain("c.ts"));
  expect(opened()).toHaveLength(2);

  // `EditSource` (⌘↓, F4).
  await fireEvent.click(row("c.ts"));
  await key("ArrowDown", { metaKey: true });
  await key("F4");
  expect(opened().slice(2)).toEqual([
    [{ path: "src/lib/c.ts", revision: FIRST, line: null }],
    [{ path: "src/lib/c.ts", revision: FIRST, line: null }],
  ]);

  // `Jump to Source` (`A:553`) in the popup.
  const menu = await popup("b.txt");
  const jump = within(menu).getByRole("menuitem", { name: "Jump to Source" });
  expect(jump.textContent).toContain("⌘↓");
  await fireEvent.click(jump);
  expect(opened()[4]).toEqual([{ path: "src/b.txt", revision: FIRST, line: null }]);
});

test("compare_with_local_opens_the_diff", async () => {
  await mount();
  await expand("src");
  await waitFor(() => expect(names()).toContain("a.txt"));

  // `RepositoryBrowserPanel`'s popup: `Jump to Source` and `Compare with Local` (`A:1636`).
  let menu = await popup("a.txt");
  expect(
    within(menu)
      .getAllByRole("menuitem")
      .map((item) => item.getAttribute("aria-label")),
  ).toEqual(["Jump to Source", "Compare with Local"]);
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Compare with Local" }));

  // `DiffRepoWithLocalAction`: the revision (`getRevisionTitle`: the full hash) against the local
  // file (`Your version`), titled from `RemoteFilePath` and the local path.
  expect(calls("diff_window_open_request")).toEqual([
    {
      request: {
        kind: "revisions",
        files: [
          {
            left: { revision: FIRST, path: "src/a.txt", title: FIRST },
            right: { revision: "local", path: "src/a.txt", title: "Your version" },
          },
        ],
        title: `a.txt (src -> ${ROOT}/src)`,
      },
    },
  ]);

  // Several: one diff over the selected files, folders left out.
  await fireEvent.click(row("README.md"), { metaKey: true });
  await fireEvent.click(row("lib"), { metaKey: true });
  menu = await popup("a.txt");
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Compare with Local" }));
  expect(calls("diff_window_open_request")[1]).toEqual({
    request: {
      kind: "revisions",
      files: [
        {
          left: { revision: FIRST, path: "src/a.txt", title: FIRST },
          right: { revision: "local", path: "src/a.txt", title: "Your version" },
        },
        {
          left: { revision: FIRST, path: "README.md", title: FIRST },
          right: { revision: "local", path: "README.md", title: "Your version" },
        },
      ],
      title: `a.txt (src -> ${ROOT}/src)`,
    },
  });

  // The order of the selection: the first file selected is the one shown and titled.
  await fireEvent.click(row("README.md"));
  await fireEvent.click(row("a.txt"), { metaKey: true });
  menu = await popup("a.txt");
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Compare with Local" }));
  expect(
    (calls("diff_window_open_request")[2] as { request: { files: { left: { path: string } }[]; title: string } })
      .request,
  ).toMatchObject({
    files: [{ left: { path: "README.md" } }, { left: { path: "src/a.txt" } }],
    title: `README.md -> ${ROOT}/README.md`,
  });

  // A file at the root has no parent in the revision: both paths whole.
  await fireEvent.click(row("README.md"));
  menu = await popup("README.md");
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Compare with Local" }));
  expect(calls("diff_window_open_request")[3]).toMatchObject({
    request: { title: `README.md -> ${ROOT}/README.md` },
  });

  // Only folders: both items are off.
  menu = await popup("lib");
  for (const item of within(menu).getAllByRole("menuitem")) {
    expect((item as HTMLButtonElement).disabled, item.getAttribute("aria-label")!).toBe(true);
  }
});

test("speed_search_finds_a_row_and_arrows_move_the_selection", async () => {
  await mount();
  expect(title()).toBe("rama at a1b2c3d4");

  await key("r");
  await key("e");
  expect(screen.getByRole("treeitem", { selected: true }).getAttribute("aria-label")).toBe("README.md");
  await key("Escape");

  await key("Home");
  expect(screen.getByRole("treeitem", { selected: true }).getAttribute("aria-label")).toBe("src");
  await key("ArrowDown", { shiftKey: true });
  expect(
    screen.getAllByRole("treeitem", { selected: true }).map((item) => item.getAttribute("aria-label")),
  ).toEqual(["src", "README.md"]);
});

test("collapsing_a_folder_selects_it_when_a_selected_row_was_under_it", async () => {
  await mount();
  await expand("src");
  await waitFor(() => expect(names()).toContain("a.txt"));
  const selectedNames = () =>
    screen.getAllByRole("treeitem", { selected: true }).map((item) => item.getAttribute("aria-label"));

  // `JTree.setExpandedState`: a selected row under the folder, though not the lead, puts the folder
  // in the selection.
  await fireEvent.click(row("a.txt"));
  await fireEvent.click(row("README.md"), { metaKey: true });
  await fireEvent.click(row("src").querySelector(".chevron")!);

  expect(names()).toEqual(["src", "README.md"]);
  expect(selectedNames()).toEqual(["src", "README.md"]);
  expect(tree().getAttribute("aria-activedescendant")).toBe("repositories-row-1");
});

test("a_folder_that_cannot_be_read_says_so_once", async () => {
  const failure = "fatal: not a tree object";
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command !== "tree_list_at") throw new Error(`unexpected command: ${command}`);
    const { dir } = args as { dir: string };
    if (dir === "src") throw { kind: "failed", message: failure };
    return dir === "" ? [folder("src"), folder("docs")] : [file(`${dir}/x.md`)];
  });
  const reported = () => get(notifications).filter((notification) => notification.title === failure);
  await mount();

  await expand("src");
  await waitFor(() => expect(reported()).toHaveLength(1));
  // The rows change: the folder that failed is read again, quietly.
  await expand("docs");
  await waitFor(() => expect(names()).toContain("x.md"));
  expect(listed().filter(({ dir }) => dir === "src").length).toBeGreaterThan(1);
  expect(reported()).toHaveLength(1);
});
