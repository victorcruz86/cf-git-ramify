import { EditorView } from "@codemirror/view";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { get } from "svelte/store";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import type { FileSides, RepoState, Side, StashChanges, StashEntry } from "../lib/api";
import { dismiss, notifications } from "../lib/notifications";
import { shortDate, shortTime } from "../log/draw";
import { showTabStep, stashFlow, type StashApi, type StashRequest, type StashUi } from "./flow";
import type { StashChangesApi } from "./stashChanges";
import StashTab from "./StashTab.svelte";

// jsdom does no layout: CodeMirror measures with this.
Range.prototype.getClientRects = () => [] as unknown as DOMRectList;
Element.prototype.scrollIntoView = () => {};

const HEAD = "h".repeat(40);
const TIME = 1_700_000_000_000;

function entry(index: number, subject: string, branch: string | null = "main"): StashEntry {
  const sha = String(index).repeat(40);
  return {
    sha,
    parents: [HEAD, "i".repeat(40), "u".repeat(40)],
    time: TIME,
    index,
    subject,
    branch,
    prefix: branch ? `On ${branch}` : null,
    message: subject,
  };
}

const NEWER = entry(0, "newer work");
const OLDER = entry(1, "older work", "feature");

const CHANGES: StashChanges = {
  files: [
    { path: "src/a.txt", oldPath: null, status: "M" },
    { path: "b.txt", oldPath: null, status: "D" },
  ],
  groups: [
    {
      title: "Index on main",
      sha: "i".repeat(40),
      files: [{ path: "src/a.txt", oldPath: null, status: "M" }],
    },
    {
      title: "Untracked files on main",
      sha: "u".repeat(40),
      files: [{ path: "new.txt", oldPath: null, status: "A" }],
    },
  ],
};

const text = (content: string): Side => ({ kind: "text", content });

function fakeApi(): StashChangesApi & { [K in keyof StashChangesApi]: ReturnType<typeof vi.fn> } {
  return {
    changes: vi.fn(async () => CHANGES),
    sides: vi.fn(async (): Promise<FileSides> => ({ old: text("old\n"), new: text("stashed\n") })),
    local: vi.fn(async () => text("local\n")),
  };
}

const RENAMED: StashChanges = {
  files: [{ path: "new/name.txt", oldPath: "old/name.txt", status: "R" }],
  groups: [],
};

const texts = (within: HTMLElement) =>
  [...within.querySelectorAll<HTMLElement>(".cm-editor")].map((dom) =>
    (EditorView.findFromDOM(dom) as EditorView).state.doc.toString(),
  );

const views = () =>
  [...document.querySelectorAll<HTMLElement>(".cm-editor")].map(
    (dom) => EditorView.findFromDOM(dom) as EditorView,
  );

function show(
  stashes: StashEntry[] = [NEWER, OLDER],
  handlers: Partial<{
    onunstash: (stash: StashEntry, pop: boolean) => unknown;
    onunstashas: (stash: StashEntry) => unknown;
    ondrop: (stash: StashEntry) => unknown;
    onvcs: (action: unknown) => unknown;
  }> = {},
) {
  const api = fakeApi();
  const props = {
    stashes,
    currentBranch: "main",
    onunstash: vi.fn(),
    onunstashas: vi.fn(),
    ondrop: vi.fn(),
    ...handlers,
    api,
  };
  const { rerender } = render(StashTab, props);
  return { api, props, rerender, list: screen.getByRole("tree", { name: "Stashes" }) };
}

const row = (name: string) => screen.getByRole("treeitem", { name });

beforeEach(() => localStorage.clear());
afterEach(() => {
  cleanup();
  get(notifications).forEach(({ id }) => dismiss(id));
});

describe("the list", () => {
  test("rows_show_subject_branch_and_tooltip", async () => {
    show();

    const rows = screen.getAllByRole("treeitem").filter((item) => item.closest('[aria-label="Stashes"]'));
    expect(rows.map((item) => item.getAttribute("aria-label"))).toEqual(["newer work", "older work"]);
    expect(row("newer work").title).toBe(
      `stash@{0} created on ${shortDate(TIME)} at ${shortTime(TIME)}`,
    );
    const label = row("newer work").querySelector(".branch") as HTMLElement;
    expect(label.textContent?.trim()).toBe("main");
    expect(label.classList.contains("current")).toBe(true);
    const other = row("older work").querySelector(".branch") as HTMLElement;
    expect(other.textContent?.trim()).toBe("feature");
    expect(other.classList.contains("current")).toBe(false);
    // The first one is selected (`TreeUtil.selectFirstNode`).
    expect(row("newer work").getAttribute("aria-selected")).toBe("true");
  });

  test("a_stash_without_a_branch_has_no_label", async () => {
    show([entry(0, "detached", null)]);

    expect(row("detached").querySelector(".branch")).toBeNull();
  });

  test("the_changes_of_a_stash_group_index_and_untracked", async () => {
    const { api } = show();

    const changes = screen.getByRole("tree", { name: "Changes" });
    await waitFor(() => expect(within(changes).getByText("Index on main")).toBeTruthy());
    expect(api.changes).toHaveBeenCalledWith(NEWER.sha);
    const lines = [...changes.querySelectorAll(".title, .folder, [role='treeitem']")].map(
      (line) => line.textContent?.trim(),
    );
    expect(lines).toEqual([
      "b.txt",
      "src",
      "a.txt",
      "Index on main",
      "src",
      "a.txt",
      "Untracked files on main",
      "new.txt",
    ]);
  });

  test("the_empty_texts_of_the_changes", async () => {
    let finish: (changes: StashChanges) => void = () => {};
    const { api } = show();
    api.changes.mockImplementationOnce(
      () => new Promise<StashChanges>((resolve) => (finish = resolve)),
    );

    await fireEvent.click(row("older work"));

    const changes = screen.getByRole("tree", { name: "Changes" });
    expect(within(changes).getByText("Loading...")).toBeTruthy();
    finish(CHANGES);
    await waitFor(() => expect(within(changes).queryByText("Loading...")).toBeNull());
    cleanup();

    // Without patches the changes and the bar are not shown (`treeChangesSplitter.secondComponent`).
    show([]);
    expect(screen.getByText("No stashes or shelves added.")).toBeTruthy();
    expect(screen.queryByRole("tree", { name: "Changes" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Apply" })).toBeNull();
  });

  test("the_preview_shows_the_first_change_of_the_stash", async () => {
    const { api } = show();

    await waitFor(() => expect(views()).toHaveLength(2));
    expect(api.sides).toHaveBeenCalledWith(NEWER.sha, "b.txt", null);
    expect(views().map((view) => view.state.doc.toString())).toEqual(["old\n", "stashed\n"]);
  });

  test("double_click_shows_the_diff_titled_stash_0_file", async () => {
    show();
    await waitFor(() => expect(screen.getAllByText("Index on main")).toHaveLength(1));

    await fireEvent.dblClick(row("newer work"));

    const dialog = await screen.findByRole("dialog", { name: "Stash@{0}: b.txt" });
    expect(dialog).toBeTruthy();
    await fireEvent.click(within(dialog).getByRole("button", { name: "Next File" }));
    expect(await screen.findByRole("dialog", { name: "Stash@{0}: a.txt" })).toBeTruthy();
  });

  // The list is read again on every change of the repository: the same stash keeps what was chosen.
  test("a_list_read_again_keeps_the_chosen_change", async () => {
    const { api, rerender } = show();
    const changes = screen.getByRole("tree", { name: "Changes" });
    await waitFor(() => expect(within(changes).getByText("new.txt")).toBeTruthy());
    await fireEvent.click(within(changes).getByRole("treeitem", { name: "new.txt" }));

    await rerender({ stashes: [{ ...NEWER }, { ...OLDER }] });

    expect(api.changes).toHaveBeenCalledTimes(1);
    const chosen = within(changes).getByRole("treeitem", { name: "new.txt" });
    expect(chosen.getAttribute("aria-selected")).toBe("true");
  });

  // `MySpeedSearch.matches`: the names of the changes already read count too.
  test("speed_search_matches_a_loaded_file_name", async () => {
    const { api, list } = show();
    api.changes.mockImplementation(async (sha: string) =>
      sha === OLDER.sha
        ? { files: [{ path: "zeta.txt", oldPath: null, status: "M" }], groups: [] }
        : CHANGES,
    );
    await waitFor(() => expect(api.changes).toHaveBeenCalledWith(NEWER.sha));
    await fireEvent.click(row("older work"));
    await waitFor(() => expect(api.changes).toHaveBeenCalledWith(OLDER.sha));

    await fireEvent.keyDown(list, { key: "b" });
    await fireEvent.keyDown(list, { key: "." });
    expect(row("newer work").getAttribute("aria-selected")).toBe("true");

    await fireEvent.keyDown(list, { key: "Escape" });
    await fireEvent.keyDown(list, { key: "z" });
    await fireEvent.keyDown(list, { key: "e" });
    expect(row("older work").getAttribute("aria-selected")).toBe("true");
  });

  // P6: a drag the system cancels ends as a release does: the proportion is kept and a later move
  // no longer resizes.
  test("a_cancelled_splitter_drag_ends_like_a_release", async () => {
    const box = { left: 0, top: 0, width: 1000, height: 1000, right: 1000, bottom: 1000 };
    const bounds = vi
      .spyOn(Element.prototype, "getBoundingClientRect")
      .mockReturnValue({ ...box, x: 0, y: 0, toJSON: () => box } as DOMRect);
    try {
      show();
      const columns = screen
        .getAllByRole("separator")
        .find((item) => item.getAttribute("aria-orientation") === "vertical")!;

      await fireEvent.pointerDown(columns, { clientX: 500, pointerId: 1 });
      await fireEvent.pointerMove(columns, { clientX: 400, pointerId: 1 });
      await fireEvent.pointerCancel(columns, { pointerId: 1 });
      await fireEvent.pointerMove(columns, { clientX: 200, pointerId: 1 });

      expect(localStorage.getItem("vcs.saved.patches.diff.splitter")).toBe("0.4");
      expect(columns.getAttribute("aria-valuenow")).toBe("40");
    } finally {
      bounds.mockRestore();
    }
  });

  // `vcs.saved.patches.changes.splitter.vertical` and `vcs.saved.patches.diff.splitter`, 0.5 each.
  test("the_splitters_resize_and_remember", async () => {
    const box = { left: 0, top: 0, width: 1000, height: 1000, right: 1000, bottom: 1000 };
    const bounds = vi
      .spyOn(Element.prototype, "getBoundingClientRect")
      .mockReturnValue({ ...box, x: 0, y: 0, toJSON: () => box } as DOMRect);
    try {
      show();
      const [rows, columns] = ["horizontal", "vertical"].map(
        (orientation) =>
          screen
            .getAllByRole("separator")
            .find((item) => item.getAttribute("aria-orientation") === orientation)!,
      );
      expect([rows, columns].map((item) => item.getAttribute("aria-valuenow"))).toEqual(["50", "50"]);

      await fireEvent.pointerDown(columns, { clientX: 500, pointerId: 1 });
      await fireEvent.pointerMove(columns, { clientX: 300, pointerId: 1 });
      await fireEvent.pointerUp(columns, { pointerId: 1 });
      await fireEvent.pointerDown(rows, { clientY: 500, pointerId: 1 });
      await fireEvent.pointerMove(rows, { clientY: 700, pointerId: 1 });
      await fireEvent.pointerUp(rows, { pointerId: 1 });

      expect(localStorage.getItem("vcs.saved.patches.diff.splitter")).toBe("0.3");
      expect(localStorage.getItem("vcs.saved.patches.changes.splitter.vertical")).toBe("0.7");
      cleanup();
      show();
      const again = screen
        .getAllByRole("separator")
        .map((item) => [item.getAttribute("aria-orientation"), item.getAttribute("aria-valuenow")]);
      expect(again).toEqual([
        ["horizontal", "70"],
        ["vertical", "30"],
      ]);
    } finally {
      bounds.mockRestore();
    }
  });

  // What was read for a stash that is gone is not kept.
  test("the_changes_of_a_dropped_stash_are_forgotten", async () => {
    const { api, rerender } = show();
    await fireEvent.click(row("older work"));
    await waitFor(() => expect(api.changes).toHaveBeenCalledWith(OLDER.sha));

    await rerender({ stashes: [NEWER] });
    await rerender({ stashes: [NEWER, OLDER] });
    await fireEvent.click(row("older work"));

    await waitFor(() =>
      expect(vi.mocked(api.changes).mock.calls.filter(([sha]) => sha === OLDER.sha)).toHaveLength(2),
    );
  });

  test("speed_search_selects_the_matching_stash", async () => {
    const { list } = show();

    await fireEvent.keyDown(list, { key: "o" });
    await fireEvent.keyDown(list, { key: "l" });

    expect(row("older work").getAttribute("aria-selected")).toBe("true");
    expect(within(list.parentElement!).getByText("ol")).toBeTruthy();
    await fireEvent.keyDown(list, { key: "Escape" });
    expect(within(list.parentElement!).queryByText("ol")).toBeNull();
  });
});

describe("actions", () => {
  const STATE: RepoState = { operation: null, branch: "main", label: "main", head: HEAD };

  function flowWith(answers: Partial<Record<StashRequest["kind"], unknown>> = {}) {
    const asked: StashRequest[] = [];
    const ui: StashUi = {
      ask: async <T,>(request: StashRequest) => {
        asked.push(request);
        return (answers[request.kind] ?? null) as T | null;
      },
      resolveConflicts: vi.fn(async () => "resolved" as const),
    };
    const api: StashApi = {
      state: vi.fn(async () => STATE),
      branches: vi.fn(async () => ({
        local: [],
        remote: [],
        recent: [],
        current: "main",
        previous: null,
      })),
      list: vi.fn(async () => [NEWER]),
      save: vi.fn(async () => {}),
      unstash: vi.fn(async () => ({ kind: "applied" as const })),
      drop: vi.fn(async () => {}),
      clear: vi.fn(async () => {}),
      changes: vi.fn(async () => CHANGES),
      sides: vi.fn(async () => ({ old: text("old\n"), new: text("stashed\n") })),
      local: vi.fn(async () => text("local\n")),
    };
    const flow = stashFlow(ui, api);
    flow.repositoryChanged({ path: "/work/rama" });
    return { flow, api, asked };
  }

  test("apply_and_pop_never_pass_index", async () => {
    const { flow, api } = flowWith();
    const { list } = show([NEWER, OLDER], { onunstash: flow.unstash });

    await fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    await waitFor(() => expect(api.unstash).toHaveBeenCalledTimes(1));
    await fireEvent.click(row("older work"));
    await fireEvent.click(screen.getByRole("button", { name: "Pop" }));
    await waitFor(() => expect(api.unstash).toHaveBeenCalledTimes(2));
    // ⌘⏎ is Apply (`DEFAULT_COMMIT_ACTION_SHORTCUT`).
    await fireEvent.keyDown(list, { key: "Enter", metaKey: true });
    await waitFor(() => expect(api.unstash).toHaveBeenCalledTimes(3));

    expect(vi.mocked(api.unstash).mock.calls).toEqual([
      [NEWER.sha, false, false, null, HEAD],
      [OLDER.sha, true, false, null, HEAD],
      [OLDER.sha, false, false, null, HEAD],
    ]);
  });

  test("the_apply_bar_explains_apply_and_pop", () => {
    show();

    const help = screen.getByLabelText("Help");
    expect(help.title).toBe(
      "Apply — apply and keep the stash\nin the list.\nPop — apply and remove the stash\nfrom the list.",
    );
    const apply = screen.getByRole("button", { name: "Apply" });
    expect(apply.classList.contains("default")).toBe(true);
  });

  test("the_context_menu_order", async () => {
    show();

    await fireEvent.contextMenu(row("older work"));

    const menu = await screen.findByRole("menu", { name: "Stash" });
    const items = [...menu.querySelectorAll('[role="menuitem"], [role="separator"]')].map(
      (item) => (item.getAttribute("role") === "separator" ? "-" : item.getAttribute("aria-label")),
    );
    expect(items).toEqual([
      "Pop",
      "Apply",
      "Unstash…",
      "Drop",
      "-",
      "Show Diff",
      "Show Diff in a New Tab or Window",
      "Compare with Local",
    ]);
    expect(row("older work").getAttribute("aria-selected")).toBe("true");
  });

  test("the_menu_runs_on_the_stash_it_was_opened_on", async () => {
    const { props } = show();

    await fireEvent.contextMenu(row("older work"));
    await fireEvent.click(await screen.findByRole("menuitem", { name: "Unstash…" }));
    await fireEvent.contextMenu(row("older work"));
    await fireEvent.click(await screen.findByRole("menuitem", { name: "Pop" }));

    expect(props.onunstashas).toHaveBeenCalledWith(OLDER);
    expect(props.onunstash).toHaveBeenCalledWith(OLDER, true);
  });

  test("compare_with_local_puts_the_stash_against_the_local_file", async () => {
    const { api } = show();
    await waitFor(() => expect(screen.getAllByText("Index on main")).toHaveLength(1));

    await fireEvent.contextMenu(row("newer work"));
    await fireEvent.click(await screen.findByRole("menuitem", { name: "Compare with Local" }));

    const dialog = await screen.findByRole("dialog", { name: "Stash@{0}: b.txt" });
    await waitFor(() => expect(within(dialog).getByText("Your version")).toBeTruthy());
    expect(api.local).toHaveBeenCalledWith("b.txt");
    // `Change(afterRevision, local)`: the stash's version on the left.
    await waitFor(() => expect(texts(dialog)).toEqual(["stashed\n", "local\n"]));
  });

  test("compare_before_with_local_takes_the_old_path_of_a_rename", async () => {
    const { api } = show();
    api.changes.mockResolvedValue(RENAMED);
    await fireEvent.click(row("older work"));
    const changes = screen.getByRole("tree", { name: "Changes" });
    const file = await within(changes).findByRole("treeitem", { name: "name.txt" });

    await fireEvent.contextMenu(file);
    await fireEvent.click(await screen.findByRole("menuitem", { name: "Compare Before with Local" }));

    const dialog = await screen.findByRole("dialog", { name: "Stash@{1}: name.txt" });
    await waitFor(() => expect(texts(dialog)).toEqual(["old\n", "local\n"]));
    expect(api.local).toHaveBeenCalledWith("old/name.txt");
    expect(api.local).not.toHaveBeenCalledWith("new/name.txt");
  });

  // `getLocalVirtualFileFor(revision) ?: getLocalVirtualFileFor(otherRevision)`.
  test("compare_with_local_falls_back_to_the_other_path_of_a_rename", async () => {
    const { api } = show();
    api.changes.mockResolvedValue(RENAMED);
    api.local.mockImplementation(async (path: string) =>
      path === "old/name.txt" ? text("there\n") : { kind: "absent" },
    );
    await fireEvent.click(row("older work"));
    await waitFor(() => expect(api.changes).toHaveBeenCalledWith(OLDER.sha));

    await fireEvent.contextMenu(row("older work"));
    await fireEvent.click(await screen.findByRole("menuitem", { name: "Compare with Local" }));

    const dialog = await screen.findByRole("dialog", { name: "Stash@{1}: name.txt" });
    await waitFor(() => expect(texts(dialog)).toEqual(["stashed\n", "there\n"]));
    expect(vi.mocked(api.local).mock.calls.map(([path]) => path)).toContain("new/name.txt");
  });

  // `getChangeWithLocal` returns null when neither the compared revision nor the local file exist.
  test("compare_with_local_leaves_out_a_change_with_both_sides_missing", async () => {
    const { api } = show();
    api.local.mockImplementation(async (path: string) =>
      path === "b.txt" ? { kind: "absent" } : text("local\n"),
    );
    await waitFor(() => expect(screen.getAllByText("Index on main")).toHaveLength(1));

    await fireEvent.contextMenu(row("newer work"));
    await fireEvent.click(await screen.findByRole("menuitem", { name: "Compare with Local" }));

    const dialog = await screen.findByRole("dialog", { name: "Stash@{0}: a.txt" });
    expect(within(dialog).getByText("1/3")).toBeTruthy();
  });

  test("the_menu_of_the_changes", async () => {
    show();
    const changes = screen.getByRole("tree", { name: "Changes" });
    const file = await within(changes).findByRole("treeitem", { name: "new.txt" });

    await fireEvent.contextMenu(file);

    const menu = await screen.findByRole("menu", { name: "Changes" });
    const items = [...menu.querySelectorAll('[role="menuitem"]')].map((item) =>
      item.getAttribute("aria-label"),
    );
    expect(items).toEqual([
      "Show Diff",
      "Show Diff in a New Tab or Window",
      "Compare with Local",
      "Compare Before with Local",
      "Cherry-Pick Selected Changes",
    ]);
  });

  // `Vcs.ApplySelectedChanges`, through the same `apply_paths` as the log.
  test("cherry_pick_selected_changes_goes_through_apply_paths", async () => {
    const onvcs = vi.fn();
    show([NEWER, OLDER], { onvcs });
    const changes = screen.getByRole("tree", { name: "Changes" });
    await within(changes).findByRole("treeitem", { name: "new.txt" });

    await fireEvent.contextMenu(within(changes).getAllByRole("treeitem", { name: "a.txt" })[0]);
    await fireEvent.click(await screen.findByRole("menuitem", { name: "Cherry-Pick Selected Changes" }));
    await fireEvent.contextMenu(within(changes).getByRole("treeitem", { name: "new.txt" }));
    await fireEvent.click(await screen.findByRole("menuitem", { name: "Cherry-Pick Selected Changes" }));

    expect(onvcs.mock.calls).toEqual([
      [
        {
          kind: "applyPaths",
          reverse: false,
          selection: [{ hash: NEWER.sha, base: HEAD, paths: ["src/a.txt"] }],
          hashes: [NEWER.sha],
        },
      ],
      [
        {
          kind: "applyPaths",
          reverse: false,
          selection: [{ hash: "u".repeat(40), base: null, paths: ["new.txt"] }],
          hashes: ["u".repeat(40)],
        },
      ],
    ]);
  });

  test("cmd_d_in_the_changes_shows_the_diff", async () => {
    show();
    const changes = screen.getByRole("tree", { name: "Changes" });
    await within(changes).findByRole("treeitem", { name: "new.txt" });
    await fireEvent.click(within(changes).getByRole("treeitem", { name: "new.txt" }));

    await fireEvent.keyDown(changes, { key: "d", metaKey: true });

    expect(await screen.findByRole("dialog", { name: "Stash@{0}: new.txt" })).toBeTruthy();
  });

  test("drop_with_delete_and_backspace_asks_first", async () => {
    const { flow, api, asked } = flowWith({ stashConfirm: false });
    const { list } = show([NEWER, OLDER], { ondrop: flow.drop });

    await fireEvent.keyDown(list, { key: "Delete" });
    await waitFor(() => expect(asked).toHaveLength(1));
    await fireEvent.keyDown(list, { key: "Backspace" });
    await waitFor(() => expect(asked).toHaveLength(2));

    expect(asked.map((request) => request.kind)).toEqual(["stashConfirm", "stashConfirm"]);
    expect(asked[0]).toMatchObject({ title: "Remove Stash stash@{0}?" });
    expect(api.drop).not.toHaveBeenCalled();

    const yes = flowWith({ stashConfirm: true });
    cleanup();
    const shown = show([NEWER, OLDER], { ondrop: yes.flow.drop });
    await fireEvent.keyDown(shown.list, { key: "Backspace", metaKey: true });
    await waitFor(() => expect(yes.api.drop).toHaveBeenCalledWith(NEWER.sha));
  });
});

describe("Show Git Stash and Unstash Changes…", () => {
  // `VcsShowToolWindowTabAction.actionPerformed`.
  test("show_git_stash_toggles", () => {
    const table: [boolean, boolean, string][] = [
      [true, true, "hide"],
      [true, false, "select"],
      [false, true, "activate"],
      [false, false, "activateAndSelect"],
    ];
    for (const [active, selected, step] of table) {
      expect(showTabStep(active, selected)).toBe(step);
    }
  });

  test("unstash_changes_opens_the_dialog_only_without_stashes", async () => {
    const asked: StashRequest[] = [];
    let visible = true;
    const showStash = vi.fn(() => visible);
    const ui: StashUi = {
      ask: async <T,>(request: StashRequest) => {
        asked.push(request);
        return null as T | null;
      },
      resolveConflicts: vi.fn(async () => "resolved" as const),
      showStash,
    };
    const api = {
      state: vi.fn(async () => ({ operation: null, branch: "main", label: "main", head: HEAD })),
      branches: vi.fn(async () => ({
        local: [],
        remote: [],
        recent: [],
        current: "main",
        previous: null,
      })),
    } as unknown as StashApi;
    const flow = stashFlow(ui, api);

    await flow.unstashChanges();
    expect(showStash).toHaveBeenCalledTimes(1);
    expect(asked).toEqual([]);

    visible = false;
    await flow.unstashChanges();
    expect(asked.map((request) => request.kind)).toEqual(["unstash"]);
  });
});
