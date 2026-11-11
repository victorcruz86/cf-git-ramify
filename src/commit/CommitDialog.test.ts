import { EditorView } from "@codemirror/view";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { get } from "svelte/store";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Changelists, FileChange, FileState, Hunk, RepoInfo, WorktreeSides } from "../lib/api";
import { dismiss, notifications } from "../lib/notifications";
import { trackTasks } from "../lib/tasks";
import CommitDialog from "./CommitDialog.svelte";
import { DETAILS_PROPORTION_KEY, DETAILS_SHOWN_KEY, SIZE_KEY, SPLITTER_KEY } from "./dialog";
import { signOffKey } from "./options";

// What `EditorView.lineWrapping` puts on the content; `view.lineWrapping` only follows it after a
// measure, which jsdom does not do.
const wrapped = (view: EditorView) => view.contentDOM.classList.contains("cm-lineWrapping");

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({ label: "main" }),
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(), emitTo: vi.fn() }));

// jsdom has no ResizeObserver, which `bind:clientHeight` needs, and does no layout for CodeMirror.
vi.stubGlobal(
  "ResizeObserver",
  class {
    observe() {}
    unobserve() {}
    disconnect() {}
  },
);
Range.prototype.getClientRects = () => [] as unknown as DOMRectList;
Element.prototype.scrollIntoView = () => {};

const REPO: RepoInfo = {
  path: "/repo",
  head: "abc",
  branch: "main",
  recovery: null,
  pending: [],
  state: { operation: null, branch: "main", label: "main", head: "abcdef1234567890" },
};

const change = (path: string, state: FileState = "modified") =>
  ({ path, oldPath: null, state, staged: false, unstaged: true }) satisfies FileChange;

const LISTS: Changelists = {
  lists: [
    { id: "default", name: "Changes", comment: "" },
    { id: "other", name: "Other", comment: "" },
  ],
  active: "default",
  assignments: { "a.txt": "default", "b.txt": "default", "c.txt": "other" },
  hunkAssignments: {},
};

const STATUS = [change("a.txt"), change("b.txt"), change("c.txt"), change("new.txt", "untracked")];

const H1: Hunk = { id: "h1", oldStart: 1, oldLines: 1, newStart: 1, newLines: 1 };
const H2: Hunk = { id: "h2", oldStart: 3, oldLines: 1, newStart: 3, newLines: 1 };
const SIDES: WorktreeSides = {
  old: { kind: "text", content: "a\nb\nc\nd\n" },
  new: { kind: "text", content: "a\nB\nc\nD\n" },
  hunks: [H1, H2],
  editable: true,
  note: null,
  hash: "hash",
  conflicted: false,
  submodule: null,
};

let repoChanged: () => void = () => {};
let order: string[] = [];

function mockCommands(commands: Record<string, (args: never) => unknown> = {}) {
  const all: Record<string, (args: never) => unknown> = {
    repo_status: () => STATUS,
    changelists_get: () => LISTS,
    changelists_set_comment: () => LISTS,
    merge_message: () => null,
    messages_last: () => null,
    messages_list: () => [],
    messages_push: () => undefined,
    repo_has_commit_hooks: () => false,
    head_message: () => "The last commit",
    repo_state: () => REPO.state,
    commit_warnings: () => [],
    commit_changes: () => {
      order.push("commit");
      return "hash";
    },
    commit_changes_selected: () => {
      order.push("commit");
      return "hash";
    },
    worktree_sides: () => SIDES,
    file_hunks: () => [H1, H2],
    ...commands,
  };
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    const handler = all[command];
    if (!handler) throw new Error(`unexpected command: ${command}`);
    return handler(args as never);
  });
}

function calls(command: string) {
  return vi.mocked(invoke).mock.calls.filter(([name]) => name === command);
}

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(listen).mockReset();
  vi.mocked(listen).mockImplementation(async (event, handler) => {
    if (event === "repo-changed") {
      repoChanged = () => handler({ event: "repo-changed", id: 0, payload: undefined });
    }
    return () => {};
  });
  order = [];
  localStorage.clear();
  get(notifications).forEach((shown) => dismiss(shown.id));
});

afterEach(cleanup);

async function open(props: Record<string, unknown> = {}) {
  const onclose = vi.fn(() => order.push("close"));
  const view = render(CommitDialog, {
    repo: REPO,
    request: { kind: "project", paths: [], lists: [] },
    executor: "commit",
    branch: "main",
    hasHead: true,
    clearInitial: false,
    onclose,
    ...props,
  });
  await screen.findByRole("treeitem", { name: "a.txt" });
  return { ...view, onclose };
}

const dialog = () => screen.getByRole("dialog", { name: "Commit Changes" });
const row = (name: string | RegExp) => screen.getByRole("treeitem", { name });
const checkOf = (name: string | RegExp) => within(row(name)).getByRole("checkbox");
const messageField = () => screen.getByRole("textbox", { name: "Commit Message" });
const commitButton = () => within(dialog()).getByRole("button", { name: "Commit" });
const legendText = () => screen.queryByLabelText("Legend")?.textContent ?? "";

async function type(text: string) {
  await fireEvent.input(messageField(), { target: { value: text } });
}

test("opens titled Commit Changes with the focus in the message", async () => {
  mockCommands();
  await open();

  expect(dialog().getAttribute("aria-modal")).toBe("true");
  expect(within(dialog()).getByRole("heading", { name: "Commit Changes" })).toBeTruthy();
  expect(document.activeElement).toBe(messageField());
});

test("the changelist chooser lists every list with the active one in bold and is disabled with one list", async () => {
  mockCommands();
  await open();

  const chooser = screen.getByRole("combobox", { name: "Changelist:" });
  expect(chooser.textContent).toContain("Changes");
  await type("My draft");
  await fireEvent.click(chooser);
  const options = screen.getAllByRole("option");
  expect(options.map((option) => option.textContent?.trim())).toEqual(["Changes", "Other"]);
  expect(options[0].querySelector("strong")?.textContent).toBe("Changes");
  expect(options[1].querySelector("strong")).toBeNull();

  await fireEvent.click(options[1]);

  expect(row("c.txt")).toBeTruthy();
  expect(screen.queryByRole("treeitem", { name: "a.txt" })).toBeNull();
  // The draft stays with the old list and the new one brings its own message.
  expect(calls("changelists_set_comment")[0][1]).toEqual({ id: "default", comment: "My draft", repo: "/repo" });
  expect((messageField() as HTMLTextAreaElement).value).toBe("Other");

  cleanup();
  mockCommands({ changelists_get: () => ({ ...LISTS, lists: [LISTS.lists[0]] }) });
  await open();

  expect(
    (screen.getByRole("combobox", { name: "Changelist:" }) as HTMLButtonElement).disabled,
  ).toBe(true);
});

test("the tree has no changelist nodes, has checkboxes per file and shows Unversioned Files", async () => {
  mockCommands();
  await open();

  expect(screen.queryByRole("treeitem", { name: /^Changes/ })).toBeNull();
  expect(screen.queryByRole("treeitem", { name: /^Other/ })).toBeNull();
  expect(screen.queryByRole("treeitem", { name: "c.txt" })).toBeNull();
  expect(checkOf("a.txt").getAttribute("aria-checked")).toBe("true");
  expect(checkOf("b.txt").getAttribute("aria-checked")).toBe("true");
  expect(checkOf(/^Unversioned Files/).getAttribute("aria-checked")).toBe("false");
  for (const name of ["Show Diff", "Rollback…", "Refresh", "Group By", "Expand All", "Collapse All"]) {
    expect(within(dialog()).getByRole("button", { name })).toBeTruthy();
  }

  await fireEvent.contextMenu(row("a.txt"));

  const items = within(screen.getByRole("menu", { name: "Changes" })).getAllByRole("menuitem");
  expect(items.map((item) => item.textContent?.replace(/[⌘⌥⇧]+\w?$/, "").trim())).toEqual([
    "Show Diff",
    "Refresh",
    "Add to VCS",
    "Rollback…",
  ]);
  // `Unversioned.Files.Dialog.Popup` is not `ChangesViewPopupMenu`: the item stays, disabled.
  expect((items[2] as HTMLButtonElement).disabled).toBe(true);
});

test("the legend counts included files only, with a thousands separator", async () => {
  const many = Array.from({ length: 1234 }, (_, index) => change(`f${index}.txt`));
  mockCommands({ repo_status: () => [change("a.txt"), ...many] });
  await open();

  expect(legendText()).toBe("1,235 modified");
  await fireEvent.click(checkOf("a.txt"));

  expect(legendText()).toBe("1,234 modified");
  expect(screen.getByText("main")).toBeTruthy();
});

test("Run Git hooks is shown only when the repository has hooks", async () => {
  mockCommands();
  await open();

  expect(screen.getByRole("group", { name: "Commit Options" })).toBeTruthy();
  expect(screen.getByRole("textbox", { name: "Author:" })).toBeTruthy();
  expect(screen.getByRole("checkbox", { name: "Sign-off commit" })).toBeTruthy();
  expect(screen.queryByRole("checkbox", { name: "Run Git hooks" })).toBeNull();
  expect(screen.queryByText("Commit Checks")).toBeNull();

  cleanup();
  mockCommands({ repo_has_commit_hooks: () => true });
  await open();

  const hooks = await screen.findByRole("checkbox", { name: "Run Git hooks" });
  expect(screen.getByText("Commit Checks")).toBeTruthy();
  await fireEvent.click(hooks);
  await type("Message");
  await fireEvent.click(commitButton());

  await waitFor(() => expect(calls("commit_changes")).toHaveLength(1));
  expect(calls("commit_changes")[0][1]).toMatchObject({ request: { options: { skipHooks: true } } });
});

describe("amend", () => {
  test("Amend commit loads the last message only if the text was not edited and the button stays Commit", async () => {
    const drafted = [{ ...LISTS.lists[0], comment: "Draft" }, LISTS.lists[1]];
    mockCommands({ changelists_get: () => ({ ...LISTS, lists: drafted }) });
    await open();
    await waitFor(() => expect((messageField() as HTMLTextAreaElement).value).toBe("Draft"));

    await fireEvent.click(screen.getByRole("checkbox", { name: "Amend commit" }));

    await waitFor(() => expect((messageField() as HTMLTextAreaElement).value).toBe("The last commit"));
    expect(commitButton()).toBeTruthy();
    expect(dialog().getAttribute("aria-label")).toBe("Commit Changes");

    await fireEvent.click(screen.getByRole("checkbox", { name: "Amend commit" }));

    expect((messageField() as HTMLTextAreaElement).value).toBe("Draft");
  });

  test("an edited text stays when amend is turned on", async () => {
    mockCommands();
    await open();
    await type("Edited");

    await fireEvent.keyDown(messageField(), { key: "m", code: "KeyM", ctrlKey: true, altKey: true });

    expect(screen.getByRole("checkbox", { name: "Amend commit", checked: true })).toBeTruthy();
    expect(calls("head_message")).toHaveLength(0);
    expect((messageField() as HTMLTextAreaElement).value).toBe("Edited");
  });
});

test("an empty message shows Comment Is Empty and keeps the dialog open", async () => {
  mockCommands();
  const { onclose } = await open();

  await fireEvent.click(commitButton());

  const error = screen.getByRole("dialog", { name: "Comment Is Empty" });
  expect(within(error).getByText("Please specify comment")).toBeTruthy();
  await fireEvent.click(within(error).getByRole("button", { name: "OK" }));
  expect(screen.queryByRole("dialog", { name: "Comment Is Empty" })).toBeNull();
  expect(dialog()).toBeTruthy();
  expect(onclose).not.toHaveBeenCalled();
  expect(calls("commit_warnings")).toHaveLength(0);
});

// `CheckinHandler.ReturnResult`: the detached HEAD, rebase and file name dialogs answer `CLOSE_WINDOW` on
// Cancel; the CRLF and large file ones answer `CANCEL`.
test("cancelling the detached HEAD warning closes the dialog", async () => {
  mockCommands({ commit_warnings: () => [{ kind: "detachedHead" }] });
  const { onclose } = await open();
  await type("Message");

  await fireEvent.click(commitButton());
  const warning = await screen.findByRole("dialog", { name: "Commit in Detached HEAD" });
  await fireEvent.click(within(warning).getByRole("button", { name: "Cancel" }));

  await waitFor(() => expect(onclose).toHaveBeenCalledTimes(1));
  expect(screen.queryByRole("dialog", { name: "Commit in Detached HEAD" })).toBeNull();
  expect(calls("commit_changes")).toHaveLength(0);
  // A normal cancel: the draft stays as the comment of the list.
  expect(calls("changelists_set_comment").at(-1)?.[1]).toEqual({ id: "default", comment: "Message", repo: "/repo" });
});

test("cancelling the rebase warning closes the dialog", async () => {
  mockCommands({ commit_warnings: () => [{ kind: "rebasing" }] });
  const { onclose } = await open();
  await type("Message");

  await fireEvent.click(commitButton());
  const warning = await screen.findByRole("dialog", { name: "Unfinished Rebase Process" });
  await fireEvent.click(within(warning).getByRole("button", { name: "Cancel" }));

  await waitFor(() => expect(onclose).toHaveBeenCalledTimes(1));
  expect(calls("commit_changes")).toHaveLength(0);
});

test("cancelling the CRLF warning keeps the dialog open", async () => {
  mockCommands({ commit_warnings: () => [{ kind: "crlf", paths: ["a.txt"] }] });
  const { onclose } = await open();
  await type("Message");

  await fireEvent.click(commitButton());
  const warning = await screen.findByRole("dialog", { name: "Line Separators Warning" });
  await fireEvent.click(within(warning).getByRole("button", { name: "Cancel" }));

  await waitFor(() => expect(screen.queryByRole("dialog", { name: "Line Separators Warning" })).toBeNull());
  expect(dialog()).toBeTruthy();
  expect(onclose).not.toHaveBeenCalled();
  expect(calls("commit_changes")).toHaveLength(0);
});

const LARGE_WARNING = [{ kind: "largeFiles", paths: [["big.bin", 62914560 + 5]] }];

test("the large file warning shows over the dialog and Cancel keeps it open", async () => {
  mockCommands({ commit_warnings: () => LARGE_WARNING });
  const { onclose } = await open();
  await type("Message");

  await fireEvent.click(commitButton());
  const warning = await screen.findByRole("dialog", { name: "Commit Check Failed" });
  // Over the commit dialog, which is still there.
  expect(dialog()).toBeTruthy();
  expect(warning.textContent).toContain(
    "You are about to commit a large file into the repository: 1 file with the total size of 60MB.",
  );
  expect(warning.textContent).toContain(
    "It is not recommended to store large files in a Git repository. Consider using Git LFS.",
  );
  await fireEvent.click(within(warning).getByRole("button", { name: "Cancel" }));

  await waitFor(() => expect(screen.queryByRole("dialog", { name: "Commit Check Failed" })).toBeNull());
  expect(dialog()).toBeTruthy();
  expect(onclose).not.toHaveBeenCalled();
  expect(calls("commit_changes")).toHaveLength(0);
});

test("cancelling the large file warning keeps the dialog open", async () => {
  mockCommands({ commit_warnings: () => LARGE_WARNING });
  const { onclose } = await open();
  await type("Message");

  await fireEvent.click(commitButton());
  const warning = await screen.findByRole("dialog", { name: "Commit Check Failed" });
  await fireEvent.click(within(warning).getByRole("button", { name: "Cancel" }));

  await waitFor(() => expect(screen.queryByRole("dialog", { name: "Commit Check Failed" })).toBeNull());
  expect(dialog()).toBeTruthy();
  expect(onclose).not.toHaveBeenCalled();
  expect((messageField() as HTMLTextAreaElement).value).toBe("Message");
});

test("the large file warning counts the files and commits when confirmed", async () => {
  const mb = 1024 * 1024;
  mockCommands({
    commit_warnings: () => [
      {
        kind: "largeFiles",
        paths: [
          ["a.bin", 51 * mb],
          ["b.bin", 52 * mb],
        ],
      },
    ],
  });
  await open();
  await type("Message");

  await fireEvent.click(commitButton());
  const warning = await screen.findByRole("dialog", { name: "Commit Check Failed" });
  expect(warning.textContent).toContain("2 files with the total size of 103MB.");
  await fireEvent.click(within(warning).getByRole("button", { name: "Commit" }));

  await waitFor(() => expect(order).toEqual(["close", "commit"]));
});

const RESERVED_WARNING = [{ kind: "badFileNames", paths: [{ name: "aux.txt", problem: "reserved" }] }];

test("the file name warning for a reserved name says so", async () => {
  mockCommands({ commit_warnings: () => RESERVED_WARNING });
  await open();
  await type("Message");

  await fireEvent.click(commitButton());
  const reserved = await screen.findByRole("dialog", { name: "File Name Is Not Allowed on Windows" });

  expect(reserved.textContent).toContain(
    "Some files use reserved names that are not allowed on Windows: 'aux.txt'.",
  );
  expect(reserved.textContent).toContain(
    "If you commit, it will not be possible to check out the Git repository on Windows.",
  );
});

test("the file name warning for a bad character says so and Commit continues", async () => {
  mockCommands({
    commit_warnings: () => [{ kind: "badFileNames", paths: [{ name: "a:b.txt", problem: "invalidChar" }] }],
  });
  await open();
  await type("Message");

  await fireEvent.click(commitButton());
  const invalid = await screen.findByRole("dialog", { name: "File Name Is Not Allowed on Windows" });
  expect(invalid.textContent).toContain(
    "Some of the file names contain characters that are not allowed on Windows: 'a:b.txt'.",
  );
  expect(invalid.textContent).toContain(
    "If you commit, it might not be possible to check out the Git repository on Windows.",
  );
  await fireEvent.click(within(invalid).getByRole("button", { name: "Commit" }));

  await waitFor(() => expect(order).toEqual(["close", "commit"]));
});

test("cancelling the file name warning closes the dialog and keeps the draft", async () => {
  mockCommands({ commit_warnings: () => RESERVED_WARNING });
  const { onclose } = await open();
  await type("Message");

  await fireEvent.click(commitButton());
  const warning = await screen.findByRole("dialog", { name: "File Name Is Not Allowed on Windows" });
  await fireEvent.click(within(warning).getByRole("button", { name: "Cancel" }));

  await waitFor(() => expect(onclose).toHaveBeenCalledTimes(1));
  expect(calls("commit_changes")).toHaveLength(0);
  expect(calls("changelists_set_comment").at(-1)?.[1]).toEqual({ id: "default", comment: "Message", repo: "/repo" });
});

test("passing the checks closes the dialog and then commits", async () => {
  mockCommands();
  const oncommitted = vi.fn();
  await open({ oncommitted });
  await type("Fix the thing");

  await fireEvent.keyDown(messageField(), { key: "Enter", metaKey: true });

  await waitFor(() => expect(order).toEqual(["close", "commit"]));
  expect(calls("commit_changes")[0][1]).toEqual({
    request: {
      added: ["a.txt", "b.txt"],
      removed: [],
      partial: [],
      message: "Fix the thing",
      options: { amend: false, signOff: false, skipHooks: false, author: null, cleanup: false },
    },
  });
  await waitFor(() => expect(oncommitted).toHaveBeenCalledWith(LISTS.lists[0]));
  expect(get(notifications).map((shown) => shown.title)).toContain("2 files committed: Fix the thing");
  // A full commit of the default list clears its comment.
  expect(calls("changelists_set_comment").at(-1)?.[1]).toEqual({ id: "default", comment: "", repo: "/repo" });
});

test("Commit and Push… commits then opens the push", async () => {
  mockCommands();
  const push = vi.fn(async () => {
    order.push("push");
  });
  const onpreparepush = vi.fn(() => push);
  await open({ onpreparepush });
  await type("Message");

  await fireEvent.click(screen.getByRole("button", { name: "Other Commit Actions" }));
  await fireEvent.click(screen.getByRole("menuitem", { name: /Commit and Push…/ }));

  await waitFor(() => expect(order).toEqual(["close", "commit", "push"]));
  expect(onpreparepush).toHaveBeenCalledTimes(1);
});

test("a commit that fails after closing does not push", async () => {
  mockCommands({
    commit_changes: () => {
      order.push("commit");
      throw { kind: "git", message: "hook failed" };
    },
  });
  const push = vi.fn(async () => {});
  await open({ onpreparepush: () => push });
  await type("Message");

  await fireEvent.click(screen.getByRole("button", { name: "Other Commit Actions" }));
  await fireEvent.click(screen.getByRole("menuitem", { name: /Commit and Push…/ }));

  await waitFor(() => expect(order).toEqual(["close", "commit"]));
  await waitFor(() => expect(get(notifications).map((shown) => shown.title)).toContain("hook failed"));
  expect(push).not.toHaveBeenCalled();
});

describe("another repository opened while the commit runs", () => {
  const FIXUP = { prefilled: "fixup! one", rebaseFrom: "abc~", head: REPO.state.head };
  const elsewhere = () => vi.fn((path: string) => path !== "/repo");

  test("a commit made leaves the lists alone and is still notified", async () => {
    mockCommands();
    const sameRepo = elsewhere();
    const oncommitted = vi.fn();
    await open({ sameRepo, oncommitted });
    await type("Message");

    await fireEvent.keyDown(messageField(), { key: "Enter", metaKey: true });

    await waitFor(() =>
      expect(get(notifications).map((shown) => shown.title)).toContain("2 files committed: Message"),
    );
    expect(sameRepo).toHaveBeenCalledWith("/repo");
    expect(calls("changelists_set_comment")).toHaveLength(0);
    expect(oncommitted).not.toHaveBeenCalled();
  });

  test("a commit that fails does not write its draft", async () => {
    mockCommands({
      commit_changes: () => {
        throw { kind: "git", message: "hook failed" };
      },
    });
    await open({ sameRepo: elsewhere() });
    await type("Message");

    await fireEvent.keyDown(messageField(), { key: "Enter", metaKey: true });

    await waitFor(() => expect(get(notifications).map((shown) => shown.title)).toContain("hook failed"));
    expect(calls("changelists_set_comment")).toHaveLength(0);
  });

  test("neither the push nor the rebase run", async () => {
    mockCommands();
    const push = vi.fn(async () => {});
    const onrebase = vi.fn(async () => {});
    await open({ ...FIXUP, sameRepo: elsewhere(), onpreparepush: () => push, onrebase });
    await waitFor(() => expect((messageField() as HTMLTextAreaElement).value).toBe("fixup! one"));

    await fireEvent.click(screen.getByRole("button", { name: "Other Commit Actions" }));
    await fireEvent.click(screen.getByRole("menuitem", { name: /Commit and Rebase…/ }));
    await waitFor(() => expect(calls("commit_changes")).toHaveLength(1));
    cleanup();
    await open({ sameRepo: elsewhere(), onpreparepush: () => push });
    await type("Message");
    await fireEvent.click(screen.getByRole("button", { name: "Other Commit Actions" }));
    await fireEvent.click(screen.getByRole("menuitem", { name: /Commit and Push…/ }));
    await waitFor(() => expect(calls("commit_changes")).toHaveLength(2));

    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(onrebase).not.toHaveBeenCalled();
    expect(push).not.toHaveBeenCalled();
  });
});

describe("Fixup… and Squash Into…", () => {
  const FIXUP = { prefilled: "fixup! one", rebaseFrom: "abc~", head: REPO.state.head };
  const openActions = () => fireEvent.click(screen.getByRole("button", { name: "Other Commit Actions" }));

  test("the dialog opens with the prefilled message", async () => {
    mockCommands();
    await open(FIXUP);

    await waitFor(() => expect((messageField() as HTMLTextAreaElement).value).toBe("fixup! one"));
  });

  test("Commit and Rebase… is in the drop-down only with a rebase base", async () => {
    mockCommands();
    const { unmount } = await open(FIXUP);
    await openActions();
    expect(screen.getByRole("menuitem", { name: /Commit and Push…/ })).toBeTruthy();
    expect(screen.getByRole("menuitem", { name: /Commit and Rebase…/ })).toBeTruthy();
    unmount();

    await open({ prefilled: "fixup! one" });
    await openActions();
    expect(screen.getByRole("menuitem", { name: /Commit and Push…/ })).toBeTruthy();
    expect(screen.queryByRole("menuitem", { name: /Commit and Rebase…/ })).toBeNull();
  });

  test("Commit and Rebase… commits and then rebases from the base", async () => {
    mockCommands();
    const onrebase = vi.fn(async () => {
      order.push("rebase");
    });
    await open({ ...FIXUP, onrebase });

    await openActions();
    await fireEvent.click(screen.getByRole("menuitem", { name: /Commit and Rebase…/ }));

    await waitFor(() => expect(order).toEqual(["close", "commit", "rebase"]));
    expect(onrebase).toHaveBeenCalledWith("abc~");
  });

  test("a commit that fails is not followed by the rebase", async () => {
    mockCommands({
      commit_changes: () => {
        throw { kind: "git", message: "hook failed" };
      },
    });
    const onrebase = vi.fn(async () => {});
    const { onclose } = await open({ ...FIXUP, onrebase });

    await openActions();
    await fireEvent.click(screen.getByRole("menuitem", { name: /Commit and Rebase…/ }));

    await waitFor(() => expect(onclose).toHaveBeenCalled());
    await waitFor(() => expect(calls("commit_changes")).toHaveLength(1));
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(onrebase).not.toHaveBeenCalled();
  });

  test("with HEAD moved since proposing nothing is committed and the rebase is no longer offered", async () => {
    mockCommands({ repo_state: () => ({ ...REPO.state, head: "moved" }) });
    const onrebase = vi.fn(async () => {});
    const { onclose } = await open({ ...FIXUP, onrebase });

    await openActions();
    await fireEvent.click(screen.getByRole("menuitem", { name: /Commit and Rebase…/ }));

    expect(await screen.findByText(/Commit and Rebase… is no longer offered/)).toBeTruthy();
    expect(calls("commit_changes")).toHaveLength(0);
    expect(onclose).not.toHaveBeenCalled();
    expect(onrebase).not.toHaveBeenCalled();
    await openActions();
    expect(screen.queryByRole("menuitem", { name: /Commit and Rebase…/ })).toBeNull();
  });

  test("HEAD moving while a warning is open stops the commit and the rebase", async () => {
    let reads = 0;
    mockCommands({
      commit_warnings: () => [{ kind: "detachedHead" }],
      // Fine when the button is pressed, moved by the time the warning is confirmed.
      repo_state: () => (++reads === 1 ? REPO.state : { ...REPO.state, head: "moved" }),
    });
    const onrebase = vi.fn(async () => {});
    const { onclose } = await open({ ...FIXUP, onrebase });

    await openActions();
    await fireEvent.click(screen.getByRole("menuitem", { name: /Commit and Rebase…/ }));
    const warning = await screen.findByRole("dialog", { name: "Commit in Detached HEAD" });
    await fireEvent.click(within(warning).getByRole("button", { name: "Commit and Rebase" }));

    expect(await screen.findByText(/Commit and Rebase… is no longer offered/)).toBeTruthy();
    expect(reads).toBe(2);
    expect(calls("commit_changes")).toHaveLength(0);
    expect(onclose).not.toHaveBeenCalled();
    expect(onrebase).not.toHaveBeenCalled();
    await openActions();
    expect(screen.queryByRole("menuitem", { name: /Commit and Rebase…/ })).toBeNull();
  });

  test("a base without the HEAD that was seen is not valid", async () => {
    mockCommands();
    const onrebase = vi.fn(async () => {});
    await open({ prefilled: "fixup! one", rebaseFrom: "abc~", onrebase });

    await openActions();
    await fireEvent.click(screen.getByRole("menuitem", { name: /Commit and Rebase…/ }));

    expect(await screen.findByText(/Commit and Rebase… is no longer offered/)).toBeTruthy();
    expect(calls("commit_changes")).toHaveLength(0);
    expect(onrebase).not.toHaveBeenCalled();
  });

  test("with an operation in progress nothing is committed", async () => {
    mockCommands({ repo_state: () => ({ ...REPO.state, operation: "merging" }) });
    await open({ ...FIXUP, onrebase: vi.fn(async () => {}) });

    await openActions();
    await fireEvent.click(screen.getByRole("menuitem", { name: /Commit and Rebase…/ }));

    expect(await screen.findByText(/Commit and Rebase… is no longer offered/)).toBeTruthy();
    expect(calls("commit_changes")).toHaveLength(0);
  });
});

test("the Diff panel shows line checkboxes and toggling them changes what is committed", async () => {
  mockCommands();
  await open();
  await fireEvent.click(row("a.txt"));

  const diff = screen.getByRole("region", { name: "Diff" });
  await waitFor(() => expect(diff.querySelectorAll(".rm-check")).toHaveLength(2));
  const first = diff.querySelectorAll<HTMLElement>(".rm-check")[0];
  first.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
  await waitFor(() => expect(checkOf("a.txt").getAttribute("aria-checked")).toBe("mixed"));
  await type("Partial");
  await fireEvent.click(commitButton());

  await waitFor(() => expect(calls("commit_changes_selected")).toHaveLength(1));
  expect(calls("commit_changes_selected")[0][1]).toMatchObject({
    request: { added: ["b.txt"] },
    selections: [{ path: "a.txt", selection: [{ hunk: "h2", lines: null }] }],
  });
});

const menuLabels = (menu: HTMLElement) =>
  within(menu)
    .getAllByRole("menuitem")
    .map((item) => item.getAttribute("aria-label"));

const diffViews = (region: HTMLElement) =>
  [...region.querySelectorAll<HTMLElement>(".cm-editor")].map(
    (dom) => EditorView.findFromDOM(dom) as EditorView,
  );

test("right_click_on_the_gutter_offers_soft_wrap", async () => {
  mockCommands();
  await open();
  await fireEvent.click(row("a.txt"));
  const diff = screen.getByRole("region", { name: "Diff" });
  await waitFor(() => expect(diff.querySelectorAll(".cm-gutters")).toHaveLength(2));
  const before = diffViews(diff);
  // The place `CommitDialog` starts off, as in IntelliJ.
  expect(before.map(wrapped)).toEqual([false, false]);

  await fireEvent.contextMenu(diff.querySelector(".cm-gutters .cm-gutterElement") as HTMLElement);
  const menu = await screen.findByRole("menu");
  expect(menuLabels(menu)).toEqual(["Soft-Wrap"]);
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Soft-Wrap" }));

  expect(diffViews(diff)).toEqual(before);
  expect(before.map(wrapped)).toEqual([true, true]);
  expect(localStorage.getItem("diff.wrap.CommitDialog")).toBe("true");
  expect(localStorage.getItem("diff.wrap.Default")).toBeNull();

  // The text keeps the menu it had.
  await fireEvent.contextMenu(diff.querySelector(".cm-line") as HTMLElement);
  expect(menuLabels(await screen.findByRole("menu"))).toEqual([
    "Rollback Lines",
    "Move Lines to Another Changelist...",
    "Show Diff for Lines",
  ]);
});

test("the Diff panel, the size and the splitters are remembered", async () => {
  mockCommands();
  const first = await open();

  expect(dialog().style.width).toBe("900px");
  expect(dialog().style.height).toBe("700px");
  expect(screen.getByRole("region", { name: "Diff" })).toBeTruthy();

  await fireEvent.click(screen.getByRole("button", { name: "Diff" }));
  expect(screen.queryByRole("region", { name: "Diff" })).toBeNull();
  const grip = dialog().querySelector(".grip") as HTMLElement;
  await fireEvent.pointerDown(grip, { clientX: 100, clientY: 100, pointerId: 1 });
  await fireEvent.pointerMove(grip, { clientX: 150, clientY: 120, pointerId: 1 });
  await fireEvent.pointerUp(grip, { pointerId: 1 });
  await fireEvent.click(within(dialog()).getByRole("button", { name: "Cancel" }));
  expect(first.onclose).toHaveBeenCalled();
  first.unmount();

  expect(JSON.parse(localStorage.getItem(SIZE_KEY) ?? "{}")).toEqual({ width: 1000, height: 740 });
  expect(localStorage.getItem(DETAILS_SHOWN_KEY)).toBe("false");
  expect(localStorage.getItem(SPLITTER_KEY)).toBe("0.5");
  expect(localStorage.getItem(DETAILS_PROPORTION_KEY)).toBe("0.6");

  localStorage.setItem(SPLITTER_KEY, "0.3");
  await open();

  expect(dialog().style.width).toBe("1000px");
  expect(dialog().style.height).toBe("740px");
  expect(screen.queryByRole("region", { name: "Diff" })).toBeNull();
  expect((dialog().querySelector(".browser") as HTMLElement).style.flexGrow).toBe("0.3");
});

test("a file that disappears while the dialog is open leaves the inclusion and a new one comes in unchecked", async () => {
  mockCommands();
  await open();
  expect(legendText()).toBe("2 modified");

  mockCommands({ repo_status: () => [change("a.txt"), change("d.txt"), change("c.txt")] });
  repoChanged();

  await screen.findByRole("treeitem", { name: "d.txt" });
  expect(screen.queryByRole("treeitem", { name: "b.txt" })).toBeNull();
  expect(checkOf("d.txt").getAttribute("aria-checked")).toBe("false");
  expect(legendText()).toBe("1 modified");

  // b.txt comes back: it left the inclusion, so it is not checked again.
  mockCommands();
  repoChanged();
  await screen.findByRole("treeitem", { name: "b.txt" });
  expect(checkOf("b.txt").getAttribute("aria-checked")).toBe("false");

  await type("Only a");
  await fireEvent.click(commitButton());
  await waitFor(() => expect(calls("commit_changes")).toHaveLength(1));
  expect(calls("commit_changes")[0][1]).toMatchObject({ request: { added: ["a.txt"] } });
});

test("Commit File of a directory includes the changes under it and only the chosen unversioned files", async () => {
  mockCommands({
    repo_status: () => [
      change("src/a.txt"),
      change("src/b.txt"),
      change("top.txt"),
      change("src/new.txt", "untracked"),
    ],
    changelists_get: () => ({ ...LISTS, assignments: {} }),
  });
  render(CommitDialog, {
    repo: REPO,
    request: { kind: "files", paths: ["src"], lists: [] },
    executor: "commit",
    branch: "main",
    hasHead: true,
    clearInitial: false,
    onclose: () => {},
  });
  await screen.findByRole("treeitem", { name: /^src/ });

  expect(legendText()).toBe("2 modified");
});

test("Cancel keeps the message as the list comment", async () => {
  mockCommands();
  const { onclose } = await open();
  await type("Work in progress");

  await fireEvent.keyDown(messageField(), { key: "Escape" });

  expect(onclose).toHaveBeenCalled();
  expect(calls("changelists_set_comment").at(-1)?.[1]).toEqual({
    id: "default",
    comment: "Work in progress",
    repo: "/repo",
  });
});

test("turning amend on saves the earlier text to the message history", async () => {
  const drafted = [{ ...LISTS.lists[0], comment: "Draft" }, LISTS.lists[1]];
  mockCommands({ changelists_get: () => ({ ...LISTS, lists: drafted }) });
  await open();
  await waitFor(() => expect((messageField() as HTMLTextAreaElement).value).toBe("Draft"));

  await fireEvent.click(screen.getByRole("checkbox", { name: "Amend commit" }));

  await waitFor(() => expect((messageField() as HTMLTextAreaElement).value).toBe("The last commit"));
  expect(calls("messages_push").map(([, args]) => args)).toEqual([{ message: "Draft" }]);
});

test("sign-off is remembered between openings", async () => {
  mockCommands();
  const first = await open();
  const signOff = () => screen.getByRole("checkbox", { name: "Sign-off commit" }) as HTMLInputElement;
  expect(signOff().checked).toBe(false);

  await fireEvent.click(signOff());
  await type("Signed");
  await fireEvent.click(commitButton());
  await waitFor(() => expect(calls("commit_changes")).toHaveLength(1));
  expect(calls("commit_changes")[0][1]).toMatchObject({ request: { options: { signOff: true } } });
  first.unmount();

  expect(localStorage.getItem(signOffKey("/repo"))).toBe("true");
  await open();
  expect(signOff().checked).toBe(true);

  // A Cancel saves it too, as a list-specific option.
  await fireEvent.click(signOff());
  await fireEvent.click(within(dialog()).getByRole("button", { name: "Cancel" }));
  expect(localStorage.getItem(signOffKey("/repo"))).toBe("false");
});

test("a file deleted while a warning is open is not sent", async () => {
  mockCommands({ commit_warnings: () => [{ kind: "detachedHead" }] });
  await open();
  await type("Message");
  await fireEvent.click(commitButton());
  const warning = await screen.findByRole("dialog", { name: "Commit in Detached HEAD" });

  mockCommands({
    commit_warnings: () => [{ kind: "detachedHead" }],
    repo_status: () => [change("a.txt"), change("b.txt", "deleted"), change("c.txt")],
  });
  repoChanged();
  await waitFor(() => expect(calls("repo_status").length).toBeGreaterThan(1));
  await waitFor(() => expect(legendText()).toContain("1 deleted"));
  await fireEvent.click(within(warning).getByRole("button", { name: "Commit" }));

  await waitFor(() => expect(calls("commit_changes")).toHaveLength(1));
  expect(calls("commit_changes")[0][1]).toMatchObject({
    request: { added: ["a.txt"], removed: ["b.txt"] },
  });
});

test("the chooser cuts long names as shortenTextWithEllipsis(name, 35, 0) does", async () => {
  const long = "A changelist with a really long name here";
  mockCommands({
    changelists_get: () => ({ ...LISTS, lists: [{ ...LISTS.lists[0], name: long }, LISTS.lists[1]] }),
  });
  await open();

  const shown = screen.getByRole("combobox", { name: "Changelist:" }).textContent?.trim();

  expect(shown).toBe(`${long.slice(0, 32)}...`);
  expect(shown).toHaveLength(35);
});

// The end of a task arrives through `task-changed`.
let deliverTask: (id: number) => void = () => {};
let nextTask = 800;

function task() {
  const id = nextTask++;
  setTimeout(() => deliverTask(id));
  return id;
}

async function listenToTasks() {
  vi.mocked(listen).mockImplementation(async (event, handler) => {
    if (event === "task-changed") {
      deliverTask = (id) =>
        handler({
          event: "task-changed",
          id: 0,
          payload: { id, title: "", text: "", fraction: null, state: "done", kind: null, value: null },
        } as never);
    }
    return () => {};
  });
  await trackTasks();
}

test("Add to VCS of the browser menu moves the file to the dialog's list and includes it in the commit", async () => {
  await listenToTasks();
  let status = STATUS;
  let assignments: Record<string, string> = { ...LISTS.assignments };
  mockCommands({
    repo_status: () => status,
    changelists_get: () => ({ ...LISTS, assignments: { ...assignments } }),
    changes_add: () => {
      status = [...STATUS.slice(0, 3), change("new.txt", "added")];
      return task();
    },
    changelists_move: ({ paths, id }: { paths: string[]; id: string }) => {
      for (const path of paths) assignments = { ...assignments, [path]: id };
      return { ...LISTS, assignments };
    },
  });
  await open();
  await fireEvent.click(screen.getByRole("combobox", { name: "Changelist:" }));
  await fireEvent.click(screen.getAllByRole("option")[1]);
  expect(row("c.txt")).toBeTruthy();

  await fireEvent.click(row(/^Unversioned Files/));
  await fireEvent.contextMenu(row(/^Unversioned Files/));
  const menu = screen.getByRole("menu", { name: "Changes" });
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Add to VCS" }));

  await waitFor(() => expect(checkOf("new.txt").getAttribute("aria-checked")).toBe("true"));
  expect(calls("changes_add").map(([, args]) => args)).toEqual([
    { repo: "/repo", paths: ["new.txt"] },
  ]);
  expect(calls("changelists_move").map(([, args]) => args)).toEqual([
    { paths: ["new.txt"], id: "other" },
  ]);
  expect(screen.queryByRole("treeitem", { name: /^Unversioned Files/ })).toBeNull();

  await type("Message");
  await fireEvent.click(commitButton());

  await waitFor(() => expect(calls("commit_changes")).toHaveLength(1));
  const sent = calls("commit_changes")[0][1] as unknown as { request: { added: string[] } };
  expect(sent.request.added).toEqual(["new.txt"]);
});

describe("the Create Patch executor", () => {
  const SPLIT: Changelists = {
    ...LISTS,
    hunkAssignments: { "a.txt": { h1: "default", h2: "other" } },
  };

  // `CreatePatchFromLocalChanges`: the dialog titled `Create Patch` with its `Create Patch…`
  // button, then Patch File Settings. The file is named after the message, the message is the
  // `Subject:` header, and a file split across lists goes with the hunks of this list only.
  test("create_patch_opens_the_dialog_with_create_patch_button_then_the_settings", async () => {
    mockCommands({ changelists_get: () => SPLIT });
    const { localEntries, patchFlow } = await import("../patch/flow");
    const settings = vi.fn(async (request: { file: string; base: string }) => {
      order.push("settings");
      return { file: request.file, toClipboard: false, base: request.base, reverse: false };
    });
    const create = vi.fn(async () => {
      order.push("create");
      return { file: "/repo/Fix_it_.patch", snapshot: null };
    });
    const flow = patchFlow(
      { settings, overwrite: async () => true, created: async () => ({ reveal: false, dontAsk: false }) },
      {
        create,
        createFromShelf: create,
        defaults: async () => ({ folder: null, toClipboard: false, showInFinder: false }),
        paths: async (file, base) => ({ file, fileExists: false, base, baseExists: true }),
        showInFinder: async () => {},
        reveal: async () => {},
        restore: async () => "",
      },
    );
    const onexecute = vi.fn(async (changes: import("./dialog").ExecutorChanges, context: import("./dialog").ExecutorContext) => {
      const selections = new Map(changes.selections.map((file) => [file.path, file.selection]));
      await flow.createPatch(
        { repo: changes.repo, entries: localEntries(changes.files, selections) },
        changes.message,
        context.close,
      );
    });
    await open({ executor: "patch", onexecute });

    expect(within(screen.getByRole("dialog", { name: "Create Patch" })).getByRole("heading", { name: "Create Patch" })).toBeTruthy();
    await type("Fix it.");
    await fireEvent.click(screen.getByRole("button", { name: "Create Patch…" }));

    await waitFor(() => expect(order).toEqual(["settings", "close", "create"]));
    expect(settings.mock.calls[0][0].file).toBe("/repo/Fix_it_.patch");
    const [entries, options, to, remember, repo] = create.mock.calls[0] as unknown as unknown[];
    expect(entries).toEqual([
      {
        path: "a.txt",
        oldPath: null,
        before: { kind: "head" },
        after: { kind: "selection", selection: [{ hunk: "h1", lines: null }] },
      },
      { path: "b.txt", oldPath: null, before: { kind: "head" }, after: { kind: "disk" } },
    ]);
    expect(options).toEqual({ basePath: "/repo", reverse: false, message: "Fix it." });
    expect(to).toEqual({ kind: "file", path: "/repo/Fix_it_.patch" });
    expect([remember, repo]).toEqual([true, "/repo"]);
  });

  test("cancelling_the_settings_leaves_the_create_patch_dialog_open", async () => {
    mockCommands();
    const onexecute = vi.fn(async () => {});
    const { onclose } = await open({ executor: "patch", onexecute });
    await type("Message");

    await fireEvent.click(screen.getByRole("button", { name: "Create Patch…" }));

    await waitFor(() => expect(onexecute).toHaveBeenCalledTimes(1));
    expect(onclose).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog", { name: "Create Patch" })).toBeTruthy();
  });

  // Decision 8: no commit checks; the executor is caught, and the draft is the list's comment and
  // goes to the history, as `saveCommitMessageBeforeCommit` does.
  test("executor_failure_is_notified_and_the_draft_is_kept", async () => {
    mockCommands();
    const onexecute = vi.fn(async (_changes, context: import("./dialog").ExecutorContext) => {
      context.close();
      throw { kind: "failed", message: "The disk is full" };
    });
    const { onclose } = await open({ executor: "patch", onexecute });
    await type("My patch");

    await fireEvent.click(screen.getByRole("button", { name: "Create Patch…" }));

    await waitFor(() =>
      expect(get(notifications).map((shown) => shown.title)).toContain("The disk is full"),
    );
    expect(onclose).toHaveBeenCalledTimes(1);
    expect(calls("commit_warnings")).toHaveLength(0);
    expect(calls("messages_push").map(([, args]) => args)).toEqual([{ message: "My patch" }]);
    expect(calls("changelists_set_comment").at(-1)?.[1]).toEqual({
      id: "default",
      comment: "My patch",
      repo: "/repo",
    });
  });

  // The executor learns which repository the changes are in, and whether it is still open.
  test("the_executor_is_tied_to_its_repository", async () => {
    mockCommands();
    let open_ = "/repo";
    const onexecute = vi.fn(async () => {});
    await open({ executor: "patch", onexecute, sameRepo: (path: string) => path === open_ });
    await type("m");

    await fireEvent.click(screen.getByRole("button", { name: "Create Patch…" }));

    await waitFor(() => expect(onexecute).toHaveBeenCalledTimes(1));
    const [changes, context] = onexecute.mock.calls[0] as unknown as [
      import("./dialog").ExecutorChanges,
      import("./dialog").ExecutorContext,
    ];
    expect(changes.repo).toBe("/repo");
    expect(changes.list).toBe("default");
    expect(context.here()).toBe(true);
    open_ = "/other";
    expect(context.here()).toBe(false);
  });
});

describe("the Shelve Changes executor", () => {
  // Decision 8: like Create Patch, Shelve Changes runs none of the commit checks
  // (`GitCheckinHandlerFactory` returns early for an executor that is not a VCS commit), so a
  // detached HEAD asks nothing. The executor gets the included files.
  test("shelve_changes_runs_no_commit_checks", async () => {
    mockCommands({ commit_warnings: () => [{ kind: "detachedHead" }] });
    const onexecute = vi.fn(async (_changes, context: import("./dialog").ExecutorContext) => context.close());
    const { onclose } = await open({ executor: "shelve", onexecute });
    await type("Later");

    await fireEvent.click(screen.getByRole("button", { name: "Shelve Changes" }));

    await waitFor(() => expect(onexecute).toHaveBeenCalledTimes(1));
    expect(calls("commit_warnings")).toHaveLength(0);
    expect(onclose).toHaveBeenCalledTimes(1);
    const [changes] = onexecute.mock.calls[0] as unknown as [import("./dialog").ExecutorChanges];
    expect(changes.files.map((file) => file.path)).toEqual(["a.txt", "b.txt"]);
  });

  // The dialog of Shelve Changes lists no unversioned files (`dialogTexts("shelve").unversioned`),
  // so none can be included, even when one was selected: rollback (`delete_added`) never deletes
  // a file the shelf did not get.
  test("shelve_changes_cannot_include_an_unversioned_file", async () => {
    mockCommands();
    await open({
      executor: "shelve",
      request: { kind: "files", paths: ["a.txt", "new.txt"], lists: [] },
    });

    const shelve = screen.getByRole("dialog", { name: "Shelve Changes" });
    expect(within(shelve).queryByRole("treeitem", { name: "new.txt" })).toBeNull();
    expect(within(shelve).queryByRole("treeitem", { name: /^Unversioned Files/ })).toBeNull();
  });
});

test("the_dialog_browser_does_not_drag", async () => {
  mockCommands();
  await open();

  const source = row("a.txt");
  const y = parseFloat(source.style.top) + 12;
  await fireEvent.pointerDown(source, { button: 0, clientX: 40, clientY: y });
  await fireEvent.pointerMove(window, { buttons: 1, clientX: 40, clientY: y + 40 });

  expect(document.querySelector(".drag-label")).toBeNull();
  await fireEvent.pointerUp(window, { clientX: 40, clientY: y + 40 });
});
