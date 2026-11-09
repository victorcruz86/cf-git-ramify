import { tick } from "svelte";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { afterAll, afterEach, beforeAll, beforeEach, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  ChangeGroup,
  CommitDetails,
  LogRow,
  SelectedChanges,
  TaskEvent,
} from "../lib/api";
import { trackTasks } from "../lib/tasks";
import { get } from "svelte/store";
import { dismiss, notifications } from "../lib/notifications";
import { zipChanges } from "./changes";
import { emptyFilters } from "./filters";
import LogPanel from "./LogPanel.svelte";
import { openLogTab } from "./tabKinds";
import { ROW_HEIGHT } from "./draw";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
  emit: vi.fn(async () => {}),
}));

const HASH_A = "a".repeat(40);
const HASH_B = "b".repeat(40);
const HASH_C = "c".repeat(40);
const X = 300;

function row(hash: string, subject: string): LogRow {
  return {
    hash,
    subject,
    author: "Ana",
    time: 100,
    commitTime: 100,
    samePerson: true,
    parentCount: 1,
    refs: [],
    labels: { compact: [], normal: [] },
    highlight: { bold: false, muted: false, branch: false },
    print: { nodeX: 0, nodeColor: 1, width: 2, edges: [], arrows: [] },
  };
}

const ROWS = [row(HASH_A, "one"), row(HASH_B, "two"), row(HASH_C, "three")];

function details(overrides: Partial<CommitDetails> = {}): CommitDetails {
  return {
    hash: HASH_A,
    subject: "one",
    body: "body",
    author: "Ana",
    authorEmail: "ana@example.com",
    authorTime: 100,
    committer: "Ana",
    committerEmail: "ana@example.com",
    commitTime: 100,
    samePerson: true,
    refs: [],
    parents: [],
    links: {},
    ...overrides,
  };
}

const file = (path: string, status = "M", oldPath: string | null = null) => ({
  path,
  oldPath,
  status,
});

const group = (files: ChangeGroup["files"], overrides: Partial<ChangeGroup> = {}): ChangeGroup => ({
  parent: null,
  subject: null,
  base: "f".repeat(40),
  merge: false,
  files,
  size: files.length,
  ...overrides,
});

type Handler = (args: Record<string, unknown>) => unknown;

let commands: Record<string, Handler>;
let handlers: Map<string, ((event: { payload: unknown }) => void)[]>;
let nextTask = 100;

function deliverTask(value: unknown): number {
  const id = nextTask++;
  const event: TaskEvent = {
    id,
    title: "",
    text: "",
    fraction: null,
    state: "done",
    kind: null,
    value,
  };
  handlers.get("task-changed")?.forEach((handler) => handler({ payload: event }));
  return id;
}

function calls(command: string) {
  return vi.mocked(invoke).mock.calls.filter(([name]) => name === command);
}

function mock(overrides: Record<string, Handler> = {}) {
  commands = { ...commands, ...overrides };
}

function deferred<T>() {
  let resolve: (value: T) => void = () => {};
  const promise = new Promise<T>((done) => (resolve = done));
  return { promise, resolve };
}

const selected = (commits: SelectedChanges["commits"], maxSize = 1): SelectedChanges => ({
  commits,
  maxSize,
});

const scroller = () => screen.getByRole("grid", { name: "Commits" });

async function selectRow(index: number, init: Record<string, unknown> = {}) {
  await fireEvent.click(scroller(), { clientX: X, clientY: index * ROW_HEIGHT + 2, ...init });
}

async function panel(props: Record<string, unknown> = {}) {
  const view = render(LogPanel, { active: true, ...props });
  await screen.findByRole("grid", { name: "Commits" });
  await waitFor(() => expect(calls("log_rows").length).toBeGreaterThan(0));
  await new Promise((resolve) => setTimeout(resolve));
  return view;
}

// The new view has already painted its rows, and a click lands on them.
async function viewShown(viewId: number) {
  await waitFor(() =>
    expect(calls("log_rows").some(([, args]) => (args as { viewId: number }).viewId === viewId)).toBe(
      true,
    ),
  );
  await new Promise((resolve) => setTimeout(resolve));
}

function commitsRegion() {
  return screen.getByRole("region", { name: "Commit details" });
}

async function openViewOptions() {
  await fireEvent.click(screen.getByRole("button", { name: "View Options" }));
  return screen.getByRole("menu", { name: "View Options" });
}

function repoChanged() {
  handlers.get("repo-changed")?.forEach((handler) => handler({ payload: undefined }));
}

beforeAll(async () => {
  vi.stubGlobal(
    "ResizeObserver",
    class {
      constructor(private run: () => void) {}
      observe() {
        this.run();
      }
      unobserve() {}
      disconnect() {}
    },
  );
  HTMLCanvasElement.prototype.getContext = (() => null) as never;
});

afterAll(() => {
  vi.unstubAllGlobals();
});

beforeEach(async () => {
  handlers = new Map();
  vi.mocked(listen).mockImplementation(async (event, handler) => {
    handlers.set(event as string, [...(handlers.get(event as string) ?? []), handler as never]);
    return () => {};
  });
  commands = {
    // Every chosen file is on disk.
    viewer_openable: ({ targets }) => targets,
    log_options_get: () => ({}),
    log_options_set: () => undefined,
    log_open: () => ({ viewId: 1, count: 3, collapsible: true }),
    log_rows: ({ start }) => (start === 0 ? ROWS : []),
    log_refs: () => [{ name: "main", kind: "local" }],
    log_users: () => [],
    log_current_branch: () => "main",
    log_graph_hover: () => null,
    log_recent_filters: () => ({}),
    log_refs_changed: () => false,
    branch_list: () => ({
      local: [{ name: "main", tracked: null }, { name: "lateral", tracked: null }],
      remote: [],
      recent: [],
      current: "main",
      previous: null,
    }),
    branch_incoming_outgoing: () => ({}),
    log_find_rows: ({ hashes }) =>
      (hashes as string[]).map((hash) => {
        const index = ROWS.findIndex((item) => item.hash === hash);
        return index < 0 ? null : index;
      }),
    commit_details: ({ hashes }) =>
      (hashes as string[]).map((hash) => details({ hash, subject: `detail ${hash[0]}` })),
    branches_containing: () => ["main"],
    commit_changes_of: () => [group([file("src/a.txt")])],
    commit_changes_start: ({ hashes }) =>
      deliverTask(
        selected((hashes as string[]).map((hash) => ({ hash, group: group([file("src/a.txt")]) }))),
      ),
    task_cancel: () => undefined,
    file_sides: () => ({
      old: { kind: "text", content: "a\n" },
      new: { kind: "text", content: "b\n" },
    }),
  };
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    const handler = commands[command];
    if (!handler) throw new Error(`unexpected command: ${command}`);
    return handler((args ?? {}) as Record<string, unknown>);
  });
  await trackTasks();
});

afterEach(() => {
  cleanup();
  get(notifications).forEach(({ id }) => dismiss(id));
});

test("details_in_order_message_author_references_branches", async () => {
  mock({
    commit_details: () => [
      details({
        refs: [
          { name: "main", kind: "local" },
          { name: "v1", kind: "tag" },
        ],
      }),
    ],
    branches_containing: () => ["main", "other"],
  });
  await panel();

  await selectRow(0);

  const region = commitsRegion();
  await within(region).findByText(/In 2 branches:/);
  const text = region.textContent ?? "";
  const order = ["one", "body", "aaaaaaaa Ana <ana@example.com> on", "main", "v1", "In 2 branches:"];
  const positions = order.map((part) => text.indexOf(part));
  expect(positions).not.toContain(-1);
  expect(positions).toEqual([...positions].sort((a, b) => a - b));
});

test("subject_in_bold_unless_it_has_several_lines", async () => {
  mock({ commit_details: () => [details({ subject: "one\ntwo", body: "" })] });
  await panel();

  await selectRow(0);

  const message = await within(commitsRegion()).findByText(/one/);
  expect(message.closest("strong")).toBeNull();
});

test("details_with_a_different_committer_say_so", async () => {
  mock({
    commit_details: () => [
      details({
        committer: "Other",
        committerEmail: "other@example.com",
        commitTime: 200_000,
        samePerson: false,
      }),
    ],
  });
  await panel();

  await selectRow(0);

  expect(
    await within(commitsRegion()).findByText(/committed by Other <other@example\.com> on .* at /),
  ).toBeTruthy();
});

test("details_same_moment_other_person_does_not_repeat_the_date", async () => {
  mock({
    commit_details: () => [
      details({ committer: "Other", committerEmail: "", samePerson: false }),
    ],
  });
  await panel();

  await selectRow(0);

  const line = await within(commitsRegion()).findByText(/committed by/);
  expect(line.textContent?.trim()).toBe("committed by Other");
});

test("details_same_person_other_time", async () => {
  mock({ commit_details: () => [details({ commitTime: 200_000 })] });
  await panel();

  await selectRow(0);

  const line = await within(commitsRegion()).findByText(/^committed on .* at /);
  expect(line.textContent).not.toContain("by");
});

test("details_hash_in_the_message_is_a_link", async () => {
  const short = HASH_C.slice(0, 9);
  mock({
    commit_details: ({ hashes }) =>
      (hashes as string[])[0] === HASH_A
        ? [details({ body: `reverts ${short} and deadbee0`, links: { [short]: HASH_C } })]
        : [details({ hash: HASH_C })],
  });
  await panel();
  await selectRow(0);

  const link = await within(commitsRegion()).findByRole("button", { name: short });
  expect(within(commitsRegion()).queryByRole("button", { name: "deadbee0" })).toBeNull();

  await fireEvent.click(link);

  await waitFor(() =>
    expect(calls("commit_details").at(-1)?.[1]).toEqual({ hashes: [HASH_C] }),
  );
});

test("in_n_branches_with_show_all", async () => {
  mock({ branches_containing: () => ["main", "other"] });
  await panel();
  await selectRow(0);

  const region = commitsRegion();
  const show = await within(region).findByRole("button", { name: "Show all" });
  expect(within(region).getByText(/In 2 branches:/)).toBeTruthy();

  await fireEvent.click(show);

  expect(within(region).getByRole("button", { name: "Hide" })).toBeTruthy();
  expect(within(region).getAllByRole("listitem").map((item) => item.textContent)).toEqual([
    "main",
    "other",
  ]);
});

test("one_branch_is_singular_and_loading_is_announced", async () => {
  let answer: (value: string[]) => void = () => {};
  mock({ branches_containing: () => new Promise((resolve) => (answer = resolve)) });
  await panel();
  await selectRow(0);

  expect(await within(commitsRegion()).findByText("In branches: loading…")).toBeTruthy();

  answer(["main"]);

  expect(await within(commitsRegion()).findByText(/In 1 branch:/)).toBeTruthy();
});

test("not_in_any_branch", async () => {
  mock({ branches_containing: () => [] });
  await panel();

  await selectRow(0);

  expect(await within(commitsRegion()).findByText("Not in any branch")).toBeTruthy();
});

test("more_than_50_commits_warns", async () => {
  const many = Array.from({ length: 60 }, (_, index) =>
    row(String(index).padStart(40, "0"), `commit ${index}`),
  );
  mock({
    log_open: () => ({ viewId: 1, count: 60, collapsible: true }),
    log_rows: () => many,
    commit_details: ({ hashes }) =>
      (hashes as string[]).map((hash) => details({ hash })),
  });
  await panel();

  await selectRow(0);
  await selectRow(59, { shiftKey: true });

  await within(commitsRegion()).findByText("(showing 50 of 60 selected commits)");
  expect((calls("commit_details").at(-1)?.[1] as { hashes: string[] }).hashes).toHaveLength(50);
});

const changes = () => screen.getByRole("tree", { name: "Changes" });

test("without_selection_both_panels_say_so", async () => {
  await panel();

  expect(screen.getByText("Select commit to view changes")).toBeTruthy();
  expect(screen.getByText("No commits selected")).toBeTruthy();
});

test("changes_are_grouped_by_folder", async () => {
  mock({ commit_changes_of: () => [group([file("src/a.txt"), file("b.txt", "A")])] });
  await panel();

  await selectRow(0);

  await within(changes()).findByRole("treeitem", { name: /a\.txt/ });
  expect(within(changes()).getByText("src")).toBeTruthy();
  expect(calls("commit_changes_of")[0][1]).toEqual({ hash: HASH_A, mode: "merged" });
});

test("several_commits_join_changes", async () => {
  mock({
    commit_changes_start: () =>
      deliverTask(
        selected([
          {
            hash: HASH_A,
            group: group([file("a.txt"), file("new.txt", "D")], { base: HASH_B }),
          },
          {
            hash: HASH_B,
            group: group([file("a.txt"), file("b.txt"), file("new.txt", "A")], { base: HASH_C }),
          },
        ]),
      ),
  });
  await panel();

  await selectRow(0);
  await selectRow(1, { metaKey: true });

  await waitFor(() =>
    expect(
      within(changes())
        .getAllByRole("treeitem")
        .map((item) => item.getAttribute("aria-label")),
    ).toEqual(["a.txt", "b.txt"]),
  );
  expect(calls("commit_changes_start")).toHaveLength(1);
  expect(calls("commit_changes_start")[0][1]).toEqual({
    hashes: [HASH_A, HASH_B],
    limit: 50000,
  });
  expect(calls("commit_changes_of")).toHaveLength(1);
});

const zip = (...commits: [string, string | null, ReturnType<typeof file>[]][]) =>
  zipChanges(
    commits.map(([hash, base, files]) => ({ hash, group: group(files, { base }) })),
  );

test("zip_modify_and_rename_is_a_rename_from_the_origin", () => {
  const [change] = zip(
    ["one", "base", [file("a.txt")]],
    ["two", "one", [file("b.txt", "R100", "a.txt")]],
  );

  expect(change.file).toEqual({ path: "b.txt", oldPath: "a.txt", status: "R" });
  expect(change.hash).toBe("two");
  expect(change.base).toBe("base");
});

test("zip_two_renames_go_from_the_first_origin_to_the_last_destination", () => {
  const [change] = zip(
    ["one", "base", [file("b.txt", "R100", "a.txt")]],
    ["two", "one", [file("c.txt", "R100", "b.txt")]],
  );

  expect(change.file).toEqual({ path: "c.txt", oldPath: "a.txt", status: "R" });
});

test("zip_commit_without_parents_leaves_the_old_side_absent", async () => {
  mock({
    commit_changes_start: () =>
      deliverTask(
        selected([
          { hash: HASH_A, group: group([file("a.txt")], { base: HASH_B }) },
          { hash: HASH_B, group: group([file("a.txt", "A")], { base: null }) },
        ]),
      ),
  });
  await panel();
  await selectRow(0);
  await selectRow(1, { metaKey: true });
  await within(changes()).findByRole("treeitem", { name: "a.txt" });

  await fireEvent.keyDown(scroller(), { key: "d", code: "KeyD", metaKey: true });

  await waitFor(() => expect(calls("file_sides")).toHaveLength(1));
  expect(calls("file_sides")[0][1]).toEqual({
    hash: HASH_A,
    path: "a.txt",
    oldPath: null,
    base: null,
    noBase: true,
  });
});

test("changing_the_selection_empties_the_list_and_cancels_the_previous_one", async () => {
  const later = deferred<number>();
  mock({ commit_changes_start: () => later.promise });
  await panel();
  await selectRow(0);
  await within(changes()).findByRole("treeitem", { name: "a.txt" });
  await selectRow(1, { metaKey: true });
  await waitFor(() => expect(calls("commit_changes_start")).toHaveLength(1));

  expect(within(changes()).queryByRole("treeitem")).toBeNull();
  expect(screen.getByRole("status", { name: "Loading Commit Changes" })).toBeTruthy();

  await selectRow(2, { metaKey: true });
  await waitFor(() => expect(calls("commit_changes_start")).toHaveLength(2));
  later.resolve(77);

  await waitFor(() => expect(calls("task_cancel")[0]?.[1]).toEqual({ id: 77 }));
});

test("too_many_changes_in_one_commit_says_so_and_offers_show_anyway", async () => {
  mock({ commit_changes_of: () => [group([file("a.txt")], { size: 60_000 })] });
  await panel();
  await selectRow(0);

  expect(await screen.findByText("This commit has 60K changes")).toBeTruthy();
  expect(within(changes()).queryByRole("treeitem")).toBeNull();

  await fireEvent.click(screen.getByRole("button", { name: "Show anyway" }));

  expect(await within(changes()).findByRole("treeitem", { name: "a.txt" })).toBeTruthy();
  await selectRow(1);
  expect(await screen.findByText("This commit has 60K changes")).toBeTruthy();
});

test("too_many_changes_in_one_of_several_asks_for_everything_with_show_anyway", async () => {
  mock({
    commit_changes_start: ({ limit }) =>
      deliverTask(
        limit === null
          ? selected([{ hash: HASH_A, group: group([file("a.txt")], { size: 1_234_567 }) }], 1_234_567)
          : selected([], 1_234_567),
      ),
  });
  await panel();
  await selectRow(0);
  await selectRow(1, { metaKey: true });

  expect(await screen.findByText("One of the selected commits has 1.2M changes")).toBeTruthy();

  await fireEvent.click(screen.getByRole("button", { name: "Show anyway" }));

  expect(await within(changes()).findByRole("treeitem", { name: "a.txt" })).toBeTruthy();
  expect(calls("commit_changes_start").at(-1)?.[1]).toMatchObject({ limit: null });
});

test("changing_an_option_or_a_divider_does_not_launch_git_again", async () => {
  await panel();
  await selectRow(0);
  await within(changes()).findByRole("treeitem", { name: "a.txt" });
  await waitFor(() => expect(calls("commit_details")).toHaveLength(1));

  await layoutItem("Show Diff Preview");
  await layoutItem("Show Details");
  await layoutItem("Show Details");
  const divider = screen.getAllByRole("separator").find(
    (item) => item.getAttribute("aria-orientation") === "vertical",
  )!;
  await fireEvent.pointerDown(divider, { clientX: 100, pointerId: 1 });
  await fireEvent.pointerMove(divider, { clientX: 120, pointerId: 1 });
  await fireEvent.pointerUp(divider, { pointerId: 1 });
  await new Promise((resolve) => setTimeout(resolve, 20));

  expect(calls("commit_changes_of")).toHaveLength(1);
  expect(calls("commit_details").length).toBeLessThanOrEqual(2);
});

test("refresh_requests_the_selected_commit_details_again", async () => {
  await panel();
  await selectRow(0);
  await waitFor(() => expect(calls("branches_containing")).toHaveLength(1));

  await fireEvent.click(screen.getByRole("button", { name: "Refresh" }));

  await waitFor(() => expect(calls("commit_details")).toHaveLength(2));
  await waitFor(() => expect(calls("branches_containing")).toHaveLength(2));
  expect(calls("commit_changes_of")).toHaveLength(1);
});

test("a_selection_that_left_the_cache_survives_the_refresh", async () => {
  const many = (start: number) =>
    Array.from({ length: 200 }, (_, index) =>
      row(String(start + index).padStart(40, "0"), `commit ${start + index}`),
    );
  mock({
    log_open: () => ({ viewId: 1, count: 10000, collapsible: true }),
    log_rows: ({ start }) => many(start as number),
    log_find_rows: ({ hashes }) => (hashes as string[]).map((hash) => Number(hash)),
  });
  await panel();
  await selectRow(0);
  await waitFor(() => expect(calls("commit_details")).toHaveLength(1));

  for (let page = 1; page <= 22; page++) {
    const before = calls("log_rows").length;
    scroller().scrollTop = page * 200 * ROW_HEIGHT;
    await fireEvent.scroll(scroller());
    await waitFor(() => expect(calls("log_rows").length).toBeGreaterThan(before));
    await new Promise((resolve) => setTimeout(resolve));
  }

  await fireEvent.click(screen.getByRole("button", { name: "Refresh" }));

  await waitFor(() => expect(calls("log_find_rows").length).toBeGreaterThan(0));
  const { hashes } = calls("log_find_rows").at(-1)?.[1] as { hashes: string[] };
  expect(hashes).toContain("0".repeat(40));
});

test("another_view_while_restoring_does_not_lose_the_selection", async () => {
  const first = deferred<(number | null)[]>();
  let finds = 0;
  mock({
    log_find_rows: () => (++finds === 1 ? first.promise : [1, 0]),
    log_view_open: () => deliverTask({ viewId: 2, summary: { viewId: 2, count: 3, collapsible: true } }),
  });
  const { component } = await panel();
  await selectRow(1);
  await waitFor(() => expect(calls("commit_details")).toHaveLength(1));

  await fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
  await waitFor(() => expect(calls("log_find_rows")).toHaveLength(1));
  component.applyFilters({ users: ["Ana"] });
  await waitFor(() => expect(calls("log_find_rows")).toHaveLength(2));

  expect((calls("log_find_rows")[1][1] as { hashes: string[] }).hashes).toContain(HASH_B);
  // The old response arrives late: with row 2 it would be another commit.
  first.resolve([2, 0]);
  await new Promise((resolve) => setTimeout(resolve, 20));

  expect(calls("commit_details").at(-1)?.[1]).toEqual({ hashes: [HASH_B] });
});

test("changing_the_order_with_a_filter_in_progress_reopens_the_view", async () => {
  const slow = deferred<void>();
  let opened = 0;
  mock({
    log_view_open: async () => {
      if (++opened === 1) await slow.promise;
      return deliverTask({ viewId: 2, summary: { viewId: 2, count: 3, collapsible: true } });
    },
  });
  const { component } = await panel();
  component.applyFilters({ users: ["Ana"] });
  await waitFor(() => expect(calls("log_view_open")).toHaveLength(1));

  await openViewOptions();
  await fireEvent.pointerEnter(screen.getByRole("menuitem", { name: "IntelliSort" }));
  await fireEvent.click(await screen.findByRole("menuitem", { name: "Standard" }));

  await waitFor(() => expect(calls("log_view_open")).toHaveLength(2));
  expect(calls("log_view_open")[1][1]).toMatchObject({ spec: { sort: "standard" } });
  expect(calls("log_view_set")).toHaveLength(0);
  slow.resolve();
});

test("a_late_view_set_response_does_not_install_a_dead_view", async () => {
  const slow = deferred<unknown>();
  mock({
    log_view_set: () => slow.promise,
    log_view_open: () => deliverTask({ viewId: 2, summary: { viewId: 2, count: 3, collapsible: true } }),
  });
  const { component } = await panel();
  await openViewOptions();
  await fireEvent.pointerEnter(screen.getByRole("menuitem", { name: "IntelliSort" }));
  await fireEvent.click(await screen.findByRole("menuitem", { name: "Standard" }));
  await waitFor(() => expect(calls("log_view_set")).toHaveLength(1));

  component.applyFilters({ users: ["Ana"] });
  await viewShown(2);
  slow.resolve({ viewId: 1, count: 3, collapsible: true });
  await new Promise((resolve) => setTimeout(resolve, 20));
  const before = calls("log_rows").length;
  await fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
  await waitFor(() => expect(calls("log_open")).toHaveLength(2));

  const views = calls("log_rows").slice(0, before).map(([, args]) => (args as { viewId: number }).viewId);
  expect(views.at(-1)).toBe(2);
});

test("view_set_failing_because_of_a_reload_does_not_warn", async () => {
  const slow = deferred<unknown>();
  mock({
    log_view_set: () => slow.promise.then(() => {
      throw { kind: "failed", message: "View 1 is not open" };
    }),
    log_view_open: () => deliverTask({ viewId: 2, summary: { viewId: 2, count: 3, collapsible: true } }),
  });
  await panel();
  await openViewOptions();
  await fireEvent.click(screen.getByRole("menuitem", { name: "Long Edges" }));
  await waitFor(() => expect(calls("log_view_set")).toHaveLength(1));

  await fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
  await waitFor(() => expect(calls("log_open")).toHaveLength(2));
  slow.resolve(null);
  await new Promise((resolve) => setTimeout(resolve, 20));

  expect(get(notifications)).toEqual([]);
});

test("graph_click_failing_because_of_a_reload_does_not_warn", async () => {
  const slow = deferred<unknown>();
  mock({
    log_graph_click: () => slow.promise.then(() => {
      throw { kind: "failed", message: "View 1 is not open" };
    }),
  });
  await panel();
  await fireEvent.click(scroller(), { clientX: 8, clientY: 12 });
  await waitFor(() => expect(calls("log_graph_click")).toHaveLength(1));

  await fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
  await waitFor(() => expect(calls("log_open")).toHaveLength(2));
  slow.resolve(null);
  await new Promise((resolve) => setTimeout(resolve, 20));

  expect(get(notifications)).toEqual([]);
});

test("changing_the_order_while_the_log_is_re_read_applies_it_when_done", async () => {
  const slow = deferred<unknown>();
  let opens = 0;
  mock({
    log_open: () =>
      ++opens === 1 ? { viewId: 1, count: 3, collapsible: true } : slow.promise,
    log_view_open: () => deliverTask({ viewId: 2, summary: { viewId: 2, count: 3, collapsible: true } }),
  });
  await panel();
  await fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
  await waitFor(() => expect(calls("log_open")).toHaveLength(2));

  await openViewOptions();
  await fireEvent.pointerEnter(screen.getByRole("menuitem", { name: "IntelliSort" }));
  await fireEvent.click(await screen.findByRole("menuitem", { name: "Linear" }));
  slow.resolve({ viewId: 1, count: 3, collapsible: true });

  await waitFor(() => expect(calls("log_view_open")).toHaveLength(1));
  expect(calls("log_view_open")[0][1]).toMatchObject({ spec: { sort: "linear" } });
  expect(calls("log_view_set")).toHaveLength(0);
});

test("merge_without_own_changes_offers_show_changes_to_parents", async () => {
  mock({
    commit_changes_of: ({ mode }) =>
      mode === "merged"
        ? [group([], { merge: true })]
        : [
            group([file("a.txt")], { merge: true, parent: HASH_B, subject: "parent one", base: HASH_B }),
            group([file("b.txt")], { merge: true, parent: HASH_C, subject: "parent two", base: HASH_C }),
          ],
  });
  await panel();
  await selectRow(0);

  expect(await screen.findByText("No merged conflicts.")).toBeTruthy();

  await fireEvent.click(screen.getByRole("button", { name: "Show changes to parents" }));

  expect(await screen.findByText("No merged conflicts")).toBeTruthy();
  expect(await screen.findByText("Changes to bbbbbbbb parent one")).toBeTruthy();
  expect(screen.getByText("Changes to cccccccc parent two")).toBeTruthy();
  expect(within(changes()).getByRole("treeitem", { name: "b.txt" })).toBeTruthy();
});

test("show_only_affected_changes_filters_by_paths", async () => {
  mock({
    commit_changes_of: () => [group([file("src/a.txt"), file("another/b.txt")])],
    log_view_open: () => deliverTask({ viewId: 2, summary: { viewId: 2, count: 3, collapsible: false } }),
  });
  const { component } = await panel();
  expect(screen.queryByRole("button", { name: "Show Only Affected Changes" })).toBeNull();

  component.applyFilters({ paths: ["src"] });
  await viewShown(2);
  await selectRow(0);
  await within(changes()).findByRole("treeitem", { name: "b.txt" });

  await fireEvent.click(await screen.findByRole("button", { name: "Show Only Affected Changes" }));

  await waitFor(() =>
    expect(within(changes()).queryByRole("treeitem", { name: "b.txt" })).toBeNull(),
  );
  expect(within(changes()).getByRole("treeitem", { name: "a.txt" })).toBeTruthy();
});

test("without_changes_in_the_paths_says_so_and_offers_to_go_back", async () => {
  mock({
    commit_changes_of: () => [group([file("another/b.txt")])],
    log_view_open: () => deliverTask({ viewId: 2, summary: { viewId: 2, count: 3, collapsible: false } }),
  });
  const { component } = await panel();
  component.applyFilters({ paths: ["src"] });
  await viewShown(2);
  await selectRow(0);
  await fireEvent.click(await screen.findByRole("button", { name: "Show Only Affected Changes" }));

  expect(await screen.findByText("No changes that affect selected paths.")).toBeTruthy();

  await fireEvent.click(screen.getByRole("button", { name: "Show changes for all paths" }));

  expect(await within(changes()).findByRole("treeitem", { name: "b.txt" })).toBeTruthy();
});

async function layoutItem(name: string) {
  await fireEvent.click(screen.getByRole("button", { name: "Changes options" }));
  await fireEvent.click(screen.getByRole("menuitem", { name }));
}

test("show_details_hides_the_panel", async () => {
  await panel();
  expect(commitsRegion()).toBeTruthy();

  await layoutItem("Show Details");

  await waitFor(() => expect(screen.queryByRole("region", { name: "Commit details" })).toBeNull());
  expect(changes()).toBeTruthy();
  expect(JSON.stringify(calls("log_options_set").at(-1)?.[1])).toContain('"showDetails":false');
});

test("diff_preview_below_or_on_the_right", async () => {
  await panel();
  await selectRow(0);
  await within(changes()).findByRole("treeitem", { name: "a.txt" });
  expect(screen.queryByRole("region", { name: "Diff Preview" })).toBeNull();

  await layoutItem("Show Diff Preview");

  const preview = await screen.findByRole("region", { name: "Diff Preview" });
  expect(preview.parentElement?.dataset.location).toBe("bottom");
  await waitFor(() => expect(calls("file_sides")).toHaveLength(1));

  await fireEvent.click(screen.getByRole("button", { name: "Changes options" }));
  await fireEvent.pointerEnter(screen.getByRole("menuitem", { name: "Diff Preview Location" }));
  await fireEvent.click(await screen.findByRole("menuitem", { name: "Right" }));

  await waitFor(() =>
    expect(
      screen.getByRole("region", { name: "Diff Preview" }).parentElement?.dataset.location,
    ).toBe("right"),
  );
});

test("opening_the_log_moves_focus_to_the_table", async () => {
  await panel();

  expect(document.activeElement).toBe(scroller());
});

test("returning_to_the_log_moves_focus_to_the_table", async () => {
  const view = await panel();
  await view.rerender({ active: false });
  scroller().blur();

  await view.rerender({ active: true });

  expect(document.activeElement).toBe(scroller());
});

test("closing_the_preview_returns_focus_to_the_table", async () => {
  mock({ commit_changes_of: () => [group([file("a.txt", "M")])] });
  await panel();
  await selectRow(0);
  await within(changes()).findByRole("treeitem", { name: "a.txt" });
  await fireEvent.keyDown(scroller(), { key: "d", code: "KeyD", metaKey: true });
  const close = await screen.findByRole("button", { name: "Close" });
  close.focus();

  await fireEvent.click(close);
  await tick();

  expect(document.activeElement).toBe(scroller());
});

test("opening_a_log_commit_opens_its_diff", async () => {
  mock({ commit_changes_of: () => [group([file("a.txt", "R100", "old.txt")], { base: HASH_B })] });
  await panel();
  await selectRow(0);
  await within(changes()).findByRole("treeitem", { name: "a.txt" });

  await fireEvent.keyDown(scroller(), { key: "d", code: "KeyD", metaKey: true });

  await waitFor(() => expect(calls("file_sides")).toHaveLength(1));
  expect(calls("file_sides")[0][1]).toEqual({
    hash: HASH_A,
    path: "a.txt",
    oldPath: "old.txt",
    base: HASH_B,
    noBase: false,
  });

  await fireEvent.click(
    within(screen.getByRole("region", { name: "Diff Preview" })).getByRole("button", {
      name: "Close",
    }),
  );

  expect(screen.queryByRole("region", { name: "Diff Preview" })).toBeNull();
});

test("double_click_on_a_file_opens_its_diff", async () => {
  await panel();
  await selectRow(0);

  await fireEvent.dblClick(await within(changes()).findByRole("treeitem", { name: "a.txt" }));

  expect(await screen.findByRole("region", { name: "Diff Preview" })).toBeTruthy();
});

test("open_another_tab_starts_without_filters", async () => {
  mock({
    log_view_open: ({ tab }) =>
      deliverTask({
        viewId: 10 + (tab as number),
        summary: { viewId: 10 + (tab as number), count: 3, collapsible: true },
      }),
    log_view_close: () => undefined,
  });
  const { component } = await panel();
  component.applyFilters({ users: ["Ana"] });
  await waitFor(() => expect(calls("log_view_open")).toHaveLength(1));
  expect(screen.getByRole("button", { name: "User: Ana" })).toBeTruthy();

  await fireEvent.click(screen.getByRole("button", { name: "Open New Git Log Tab" }));

  await waitFor(() => expect(calls("log_view_open")).toHaveLength(2));
  const spec = calls("log_view_open")[1][1] as { tab: number; spec: { filter: { users: string[] } } };
  expect(spec.tab).toBe(1);
  expect(spec.spec.filter.users).toEqual([]);
  expect(await screen.findByRole("button", { name: "User" })).toBeTruthy();

  await fireEvent.click(screen.getByRole("tab", { name: "Log" , selected: false }));
  expect(await screen.findByRole("button", { name: "User: Ana" })).toBeTruthy();
});

const START = "a".repeat(40);
const END = "b".repeat(40);

function updateTabs() {
  mock({
    log_view_open: ({ tab }) =>
      deliverTask({
        viewId: 10 + (tab as number),
        summary: { viewId: 10 + (tab as number), count: 3, collapsible: true },
      }),
    log_view_close: () => undefined,
  });
}

test("view_commits_opens_a_tab_with_the_range", async () => {
  updateTabs();
  const { component } = await panel();

  await component.openUpdateInfo([START, END], ["src"]);

  await waitFor(() => expect(calls("log_view_open")).toHaveLength(1));
  const opened = calls("log_view_open")[0][1] as {
    tab: number;
    spec: { filter: { ranges: [string, string][]; paths: string[]; branches: string[] } };
  };
  expect(opened.tab).toBe(1);
  expect(opened.spec.filter.ranges).toEqual([[START, END]]);
  expect(opened.spec.filter.paths).toEqual(["src"]);
  expect(opened.spec.filter.branches).toEqual([]);
  const selected = screen.getByRole("tab", { selected: true });
  expect(selected.textContent?.trim()).toMatch(/^Update Info: .+/);
  expect(screen.getAllByRole("tab")).toHaveLength(2);
  expect(screen.getByRole("button", { name: /^Branch: / })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Paths: src" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Close Log Tab" })).toBeTruthy();
});

test("view_commits_with_the_same_range_returns_to_the_existing_tab", async () => {
  updateTabs();
  const { component } = await panel();
  await component.openUpdateInfo([START, END], []);
  await waitFor(() => expect(calls("log_view_open")).toHaveLength(1));
  await fireEvent.click(screen.getByRole("tab", { name: "Log", selected: false }));

  await component.openUpdateInfo([START, END], []);

  expect(screen.getAllByRole("tab")).toHaveLength(2);
  expect(calls("log_view_open")).toHaveLength(1);
  expect(screen.getByRole("tab", { selected: true }).textContent?.trim()).toMatch(
    /^Update Info: /,
  );

  await component.openUpdateInfo([END, START], []);
  await waitFor(() => expect(screen.getAllByRole("tab")).toHaveLength(3));
});

test("the_update_info_tab_is_not_remembered_for_next_time", async () => {
  updateTabs();
  const { component } = await panel();

  await component.openUpdateInfo([START, END], []);
  await waitFor(() => expect(calls("log_view_open")).toHaveLength(1));
  await fireEvent.click(screen.getByRole("button", { name: "Open New Git Log Tab" }));
  await waitFor(() => expect(calls("log_view_open")).toHaveLength(2));

  const saved = calls("log_options_set").at(-1)?.[1] as { options: Record<string, unknown> };
  expect(Object.keys(saved.options).sort()).toEqual(["0", "2"]);
});

// `GitUpdateInfoAsLog.findOrCreateLogUi` through the tab kinds: an `update` request with the key of an
// open tab returns to it, another key opens a new one, a `log` request always opens one, and only
// `log` tabs are remembered.
test("update_info_tabs_keep_their_find_or_create_rule", async () => {
  updateTabs();
  await panel();
  const init = vi.fn(() => ({ name: "today", filters: { ...emptyFilters(), ranges: [[START, END]] as [string, string][] } }));

  await openLogTab("update", `${START}..${END}`, init);
  await waitFor(() => expect(calls("log_view_open")).toHaveLength(1));
  expect(screen.getByRole("tab", { selected: true }).textContent?.trim()).toBe("Update Info: today");
  await fireEvent.click(screen.getByRole("tab", { name: "Log", selected: false }));

  await openLogTab("update", `${START}..${END}`, init);

  expect(screen.getAllByRole("tab")).toHaveLength(2);
  expect(calls("log_view_open")).toHaveLength(1);
  expect(init).toHaveBeenCalledTimes(1);
  expect(screen.getByRole("tab", { selected: true }).textContent?.trim()).toBe("Update Info: today");

  await openLogTab("update", `${END}..${START}`, init);
  await waitFor(() => expect(screen.getAllByRole("tab")).toHaveLength(3));
  const plain = () => ({ name: "", filters: emptyFilters() });
  await openLogTab("log", "any", plain);
  await openLogTab("log", "any", plain);
  await waitFor(() => expect(screen.getAllByRole("tab")).toHaveLength(5));
  const saved = calls("log_options_set").at(-1)?.[1] as { options: Record<string, unknown> };
  expect(Object.keys(saved.options).sort()).toEqual(["0", "3", "4"]);
});

test("view_commits_before_the_log_loads_waits_for_it_to_load", async () => {
  updateTabs();
  const opening = deferred<unknown>();
  mock({ log_open: () => opening.promise });
  const view = render(LogPanel, { active: true });
  await waitFor(() => expect(calls("log_open")).toHaveLength(1));

  const shown = view.component.openUpdateInfo([START, END], []);
  await new Promise((resolve) => setTimeout(resolve, 20));
  expect(calls("log_view_open")).toHaveLength(0);
  opening.resolve({ viewId: 1, count: 3, collapsible: true });
  await shown;

  await waitFor(() => expect(calls("log_view_open")).toHaveLength(1));
  expect(screen.getAllByRole("tab")).toHaveLength(2);
});

test("closing_a_tab_returns_to_the_main_one_and_releases_its_view", async () => {
  mock({
    log_view_open: () => deliverTask({ viewId: 5, summary: { viewId: 5, count: 3, collapsible: true } }),
    log_view_close: () => undefined,
  });
  await panel();
  await fireEvent.click(screen.getByRole("button", { name: "Open New Git Log Tab" }));
  await waitFor(() => expect(screen.getAllByRole("tab")).toHaveLength(2));

  await fireEvent.click(screen.getByRole("button", { name: "Close Log Tab" }));

  await waitFor(() => expect(screen.getAllByRole("tab")).toHaveLength(1));
  expect(calls("log_view_close")[0][1]).toEqual({ tab: 1 });
});

test("view_options_in_order", async () => {
  await panel();

  const menu = await openViewOptions();

  expect(
    within(menu)
      .getAllByRole("menuitem")
      .map((item) => item.getAttribute("aria-label")),
  ).toEqual([
    "Compact References View",
    "Tag Names",
    "Long Edges",
    "Prefer Commit Timestamp",
    "References on the Left",
    "Columns",
    "My Commits",
    "Merge Commits",
    "Current Branch",
    "Not Cherry-Picked Commits",
    "IntelliSort",
    "Collapse Linear Branches",
    "Expand Linear Branches",
  ]);
});

test("collapse_hidden_with_a_details_filter", async () => {
  mock({ log_open: () => ({ viewId: 1, count: 3, collapsible: false }) });
  await panel();

  await openViewOptions();

  expect(screen.queryByRole("menuitem", { name: "Collapse Linear Branches" })).toBeNull();
  expect(screen.queryByRole("menuitem", { name: "Expand Linear Branches" })).toBeNull();
});

test("collapse_disabled_if_there_are_no_rows", async () => {
  mock({
    log_view_open: () => deliverTask({ viewId: 2, summary: { viewId: 2, count: 0, collapsible: true } }),
  });
  const { component } = await panel();
  component.applyFilters({ branches: ["nothing"] });
  await screen.findByText("No commits matching filters");

  await openViewOptions();

  const item = screen.getByRole("menuitem", { name: "Collapse Linear Branches" }) as HTMLButtonElement;
  expect(item.disabled).toBe(true);
});

test("without_results_offers_to_remove_the_filters", async () => {
  mock({
    log_view_open: () => deliverTask({ viewId: 2, summary: { viewId: 2, count: 0, collapsible: true } }),
  });
  const { component } = await panel();
  component.applyFilters({ branches: ["nothing"] });

  await fireEvent.click(await screen.findByRole("button", { name: "Reset filters" }));

  await waitFor(() => expect(screen.getByRole("button", { name: "Branch" })).toBeTruthy());
});

test("changing_the_order_requests_the_rows_again_and_is_remembered", async () => {
  mock({ log_view_set: () => ({ viewId: 1, count: 3, collapsible: true }) });
  await panel();
  const before = calls("log_rows").length;

  await openViewOptions();
  await fireEvent.pointerEnter(screen.getByRole("menuitem", { name: "IntelliSort" }));
  await fireEvent.click(await screen.findByRole("menuitem", { name: "Standard" }));

  await waitFor(() =>
    expect(calls("log_view_set")[0][1]).toEqual({ viewId: 1, patch: { sort: "standard" } }),
  );
  await waitFor(() => expect(calls("log_rows").length).toBeGreaterThan(before));
  expect(JSON.stringify(calls("log_options_set").at(-1)?.[1])).toContain('"sort":"standard"');
});

test("saved_options_open_their_view", async () => {
  mock({
    log_options_get: () => ({ "0": { sort: "linear", tagNames: true } }),
    log_view_open: () => deliverTask({ viewId: 2, summary: { viewId: 2, count: 3, collapsible: true } }),
  });

  await panel();

  await waitFor(() => expect(calls("log_view_open")).toHaveLength(1));
  expect(calls("log_view_open")[0][1]).toMatchObject({ tab: 0, spec: { sort: "linear" } });
  const menu = await openViewOptions();
  expect(within(menu).getByRole("menuitem", { name: "Tag Names" }).textContent).toContain("✓");
  expect(within(menu).getByRole("menuitem", { name: "Collapse Merges" })).toBeTruthy();
});

test("click_on_a_folding_node_requests_the_rows_and_does_not_select", async () => {
  mock({
    log_graph_click: () => ({
      changed: true,
      summary: { viewId: 1, count: 2, collapsible: true },
    }),
  });
  await panel();
  const before = calls("log_rows").length;

  await fireEvent.click(scroller(), { clientX: 8, clientY: 12 });

  await waitFor(() =>
    expect(calls("log_graph_click")[0][1]).toEqual({
      viewId: 1,
      target: { kind: "node", row: 0 },
    }),
  );
  await waitFor(() => expect(calls("log_rows").length).toBeGreaterThan(before));
  expect(calls("commit_details")).toHaveLength(0);
});

test("click_on_a_node_without_effect_does_not_request_rows", async () => {
  mock({
    log_graph_click: () => ({
      changed: false,
      summary: { viewId: 1, count: 3, collapsible: true },
    }),
  });
  await panel();
  const before = calls("log_rows").length;

  await fireEvent.click(scroller(), { clientX: 8, clientY: 12 });
  await waitFor(() => expect(calls("log_graph_click")).toHaveLength(1));
  await new Promise((resolve) => setTimeout(resolve, 20));

  expect(calls("log_rows")).toHaveLength(before);
});

test("references_change_with_the_log_visible_reloads", async () => {
  mock({ log_refs_changed: () => true });
  await panel();

  repoChanged();

  await waitFor(() => expect(calls("log_open")).toHaveLength(2));
});

test("without_fingerprint_change_does_not_reload", async () => {
  await panel();

  repoChanged();
  await waitFor(() => expect(calls("log_refs_changed")).toHaveLength(1));
  await new Promise((resolve) => setTimeout(resolve, 20));

  expect(calls("log_open")).toHaveLength(1);
});

test("with_the_log_hidden_the_reload_waits_until_it_is_shown", async () => {
  mock({ log_refs_changed: () => true });
  const { rerender } = await panel();
  await rerender({ active: false });

  repoChanged();
  await waitFor(() => expect(calls("log_refs_changed")).toHaveLength(1));
  await new Promise((resolve) => setTimeout(resolve, 20));
  expect(calls("log_open")).toHaveLength(1);

  await rerender({ active: true });

  await waitFor(() => expect(calls("log_open")).toHaveLength(2));
});

test("the_refresh_button_always_reloads", async () => {
  await panel();

  await fireEvent.click(screen.getByRole("button", { name: "Refresh" }));

  await waitFor(() => expect(calls("log_open")).toHaveLength(2));
});

test("reloading_reopens_the_view_with_filters_and_keeps_the_selection", async () => {
  mock({
    log_view_open: () => deliverTask({ viewId: 2, summary: { viewId: 2, count: 3, collapsible: true } }),
  });
  const { component } = await panel();
  await selectRow(1);
  await waitFor(() => expect(calls("commit_details")).toHaveLength(1));
  component.applyFilters({ users: ["Ana"] });
  await viewShown(2);

  await fireEvent.click(screen.getByRole("button", { name: "Refresh" }));

  await waitFor(() => expect(calls("log_view_open")).toHaveLength(2));
  await waitFor(() => expect(calls("log_find_rows").length).toBeGreaterThanOrEqual(2));
  expect(calls("log_find_rows").at(-1)?.[1]).toMatchObject({ viewId: 2 });
  expect(calls("commit_details").at(-1)?.[1]).toEqual({ hashes: [HASH_B] });
});

test("a_cancelled_task_is_not_an_error", async () => {
  mock({
    log_view_open: () => {
      const id = nextTask++;
      handlers.get("task-changed")?.forEach((handler) =>
        handler({
          payload: {
            id,
            title: "",
            text: "",
            fraction: null,
            state: "cancelled",
            kind: "cancelled",
            value: null,
          },
        }),
      );
      return id;
    },
  });
  const { component } = await panel();

  component.applyFilters({ users: ["Ana"] });
  await waitFor(() => expect(calls("log_view_open")).toHaveLength(1));
  await new Promise((resolve) => setTimeout(resolve, 10));

  expect(get(notifications)).toEqual([]);
});

const hashesAsked = () => calls("commit_details").at(-1)?.[1];

function withPrint(index: number, print: Partial<LogRow["print"]>) {
  const rows = ROWS.map((item) => ({ ...item, print: { ...item.print } }));
  rows[index].print = { ...rows[index].print, ...print };
  mock({ log_rows: ({ start }) => (start === 0 ? rows : []) });
}

const edge = (overrides: Partial<LogRow["print"]["edges"][number]> = {}) => ({
  x: 0,
  otherX: 0,
  up: false,
  color: 1,
  style: "usual" as const,
  upRow: 0,
  downRow: 1,
  ...overrides,
});

const arrow = (overrides: Partial<LogRow["print"]["arrows"][number]> = {}) => ({
  x: 0,
  otherX: 0,
  up: false,
  color: 1,
  targetRow: 2,
  terminal: true,
  upRow: 0,
  downRow: 2,
  ...overrides,
});

// Below the node of the first row, over what leaves it going down.
const BELOW_NODE = { clientX: 8, clientY: 20 };
const shortcut = (element: Element, init: KeyboardEventInit) => fireEvent.keyDown(element, init);

test("arrows_move_the_selection_and_shift_extends_it", async () => {
  await panel();
  await selectRow(0);

  await shortcut(scroller(), { key: "ArrowDown" });
  await waitFor(() => expect(hashesAsked()).toEqual({ hashes: [HASH_B] }));

  await shortcut(scroller(), { key: "ArrowDown", shiftKey: true });
  await waitFor(() => expect(hashesAsked()).toEqual({ hashes: [HASH_B, HASH_C] }));

  await shortcut(scroller(), { key: "ArrowUp" });
  await waitFor(() => expect(hashesAsked()).toEqual({ hashes: [HASH_B] }));
});

test("right_arrow_goes_to_the_parent", async () => {
  mock({ log_adjacent_rows: ({ parent }) => (parent ? [2] : []) });
  await panel();
  await selectRow(0);

  await shortcut(scroller(), { key: "ArrowRight" });

  await waitFor(() => expect(hashesAsked()).toEqual({ hashes: [HASH_C] }));
  expect(calls("log_adjacent_rows")[0][1]).toEqual({ viewId: 1, row: 0, parent: true });
});

test("left_arrow_goes_to_the_child", async () => {
  mock({ log_adjacent_rows: ({ parent }) => (parent ? [] : [0]) });
  await panel();
  await selectRow(2);

  await shortcut(scroller(), { key: "ArrowLeft" });

  await waitFor(() => expect(hashesAsked()).toEqual({ hashes: [HASH_A] }));
  expect(calls("log_adjacent_rows")[0][1]).toEqual({ viewId: 1, row: 2, parent: false });
});

test("with_focus_outside_the_table_arrows_do_not_navigate", async () => {
  mock({ log_adjacent_rows: () => [2] });
  await panel();
  await selectRow(0);

  await shortcut(screen.getByRole("searchbox"), { key: "ArrowRight" });
  await new Promise((resolve) => setTimeout(resolve));

  expect(calls("log_adjacent_rows")).toHaveLength(0);
});

test("several_parents_open_a_numbered_popup", async () => {
  mock({ log_adjacent_rows: () => [1, 2] });
  await panel();
  await selectRow(0);

  await shortcut(scroller(), { key: "ArrowRight" });

  const popup = await screen.findByRole("menu", { name: "Select Parent to Navigate" });
  const rows = within(popup).getAllByRole("menuitem");
  expect(rows[0].textContent).toMatch(/1\.\s+bbbbbbbb "two" by Ana on/);
  expect(rows[1].textContent).toMatch(/2\.\s+cccccccc "three" by Ana on/);

  await shortcut(popup, { key: "2" });

  await waitFor(() => expect(hashesAsked()).toEqual({ hashes: [HASH_C] }));
  expect(screen.queryByRole("menu")).toBeNull();
});

test("several_children_open_their_popup", async () => {
  mock({ log_adjacent_rows: () => [0, 1] });
  await panel();
  await selectRow(2);

  await shortcut(scroller(), { key: "ArrowLeft" });

  expect(await screen.findByRole("menu", { name: "Select Child to Navigate" })).toBeTruthy();
});

test("with_several_selected_there_is_no_navigation", async () => {
  mock({ log_adjacent_rows: () => [1] });
  await panel();
  await selectRow(0);
  await selectRow(1, { shiftKey: true });

  await shortcut(scroller(), { key: "ArrowRight" });
  await new Promise((resolve) => setTimeout(resolve));

  expect(calls("log_adjacent_rows")).toHaveLength(0);
});

test("cmd_l_toggles_filter_and_table", async () => {
  await panel();
  scroller().focus();

  await shortcut(scroller(), { key: "l", code: "KeyL", metaKey: true });
  expect(document.activeElement).toBe(screen.getByRole("searchbox"));

  await shortcut(screen.getByRole("searchbox"), { key: "l", code: "KeyL", metaKey: true });
  expect(document.activeElement).toBe(scroller());
});

async function goTo(text: string) {
  await shortcut(scroller(), { key: "f", code: "KeyF", metaKey: true });
  const field = screen.getByRole("textbox", { name: "Enter hash or branch/tag name:" });
  await fireEvent.input(field, { target: { value: text } });
  return field;
}

test("go_to_jumps_to_the_row_and_leaves_focus_on_the_table", async () => {
  mock({ log_row_of: () => ({ row: 2, hash: HASH_C }) });
  await panel();

  const field = await goTo("mai");
  await shortcut(field, { key: "Enter" });

  await waitFor(() => expect(hashesAsked()).toEqual({ hashes: [HASH_C] }));
  expect(calls("log_row_of")[0][1]).toEqual({
    viewId: 1,
    target: { kind: "ref", value: "main" },
  });
  expect(screen.queryByRole("textbox", { name: /Enter hash/ })).toBeNull();
  expect(document.activeElement).toBe(scroller());
});

test("go_to_also_opens_from_the_bar", async () => {
  await panel();

  await fireEvent.click(screen.getByRole("button", { name: "Go to Hash/Branch/Tag" }));

  expect(screen.getByRole("textbox", { name: "Enter hash or branch/tag name:" })).toBeTruthy();
});

test("go_to_completes_references_and_chooses_with_the_arrows", async () => {
  mock({
    log_refs: () => [
      { name: "main", kind: "local" },
      { name: "mainline", kind: "local" },
    ],
    log_row_of: () => ({ row: 1, hash: HASH_B }),
  });
  await panel();

  const field = await goTo("mai");
  const options = screen.getAllByRole("option").map((option) => option.textContent?.trim());
  expect(options).toEqual(["main", "mainline"]);

  await shortcut(field, { key: "ArrowDown" });
  await shortcut(field, { key: "ArrowDown" });
  await shortcut(field, { key: "Enter" });

  await waitFor(() =>
    expect(calls("log_row_of")[0][1]).toMatchObject({
      target: { kind: "ref", value: "mainline" },
    }),
  );
});

test("go_to_empty_or_cancelled_does_nothing", async () => {
  await panel();

  let field = await goTo("   ");
  await shortcut(field, { key: "Enter" });
  expect(screen.queryByRole("textbox", { name: /Enter hash/ })).toBeNull();

  field = await goTo("main");
  await shortcut(field, { key: "Escape" });
  expect(screen.queryByRole("textbox", { name: /Enter hash/ })).toBeNull();
  expect(calls("log_row_of")).toHaveLength(0);
  expect(get(notifications)).toEqual([]);
});

test("go_to_that_does_not_look_like_a_hash_warns_without_asking", async () => {
  await panel();

  const field = await goTo("xyz");
  await shortcut(field, { key: "Enter" });

  await waitFor(() => expect(get(notifications)).toHaveLength(1));
  expect(get(notifications)[0]).toMatchObject({
    kind: "warning",
    title: "'xyz' does not look like a commit hash",
  });
  expect(calls("log_row_of")).toHaveLength(0);
});

test("go_to_without_a_commit_warns", async () => {
  mock({ log_row_of: () => ({ row: null, hash: null }) });
  await panel();

  const field = await goTo("abcd1234ef");
  await shortcut(field, { key: "Enter" });

  await waitFor(() => expect(get(notifications)).toHaveLength(1));
  expect(get(notifications)[0].title).toBe("Commit or reference 'abcd1234' not found");
  expect(get(notifications)[0].actions).toBeUndefined();
});

test("go_to_without_filters_does_not_offer_to_remove_them", async () => {
  mock({ log_row_of: () => ({ row: null, hash: HASH_B }) });
  await panel();

  const field = await goTo("bbbb");
  await shortcut(field, { key: "Enter" });

  await waitFor(() => expect(get(notifications)).toHaveLength(1));
  expect(get(notifications)[0].title).toBe("Commit or reference 'bbbb' doesn't match the filters");
  expect(get(notifications)[0].actions).toBeUndefined();
});

test("go_to_filtered_offers_to_view_and_remove_the_filters", async () => {
  mock({
    log_view_open: () => deliverTask({ viewId: 2, summary: { viewId: 2, count: 3, collapsible: true } }),
  });
  const view = await panel();
  await view.component.applyFilters({ text: "one" });
  await viewShown(2);
  mock({ log_row_of: () => ({ row: null, hash: HASH_B }) });

  const field = await goTo("bbbb");
  await shortcut(field, { key: "Enter" });

  await waitFor(() => expect(get(notifications)).toHaveLength(1));
  const { actions } = get(notifications)[0];
  expect(actions?.map(({ label }) => label)).toEqual(["View and reset filters", "View in new tab"]);

  mock({ log_row_of: () => ({ row: 1, hash: HASH_B }) });
  actions?.[0].run();

  await waitFor(() => expect(hashesAsked()).toEqual({ hashes: [HASH_B] }));
  expect(calls("log_view_open").at(-1)?.[1]).toMatchObject({ spec: { filter: { text: null } } });
});

test("copy_hash_with_the_shortcut", async () => {
  mock({ clipboard_write: () => undefined });
  await panel();
  await selectRow(0);
  await selectRow(1, { shiftKey: true });

  await shortcut(scroller(), { key: "Ç", code: "KeyC", metaKey: true, altKey: true, shiftKey: true });

  // `CopyRevisionNumberAction`: the rows come newest first and are copied oldest first.
  await waitFor(() =>
    expect(calls("clipboard_write")[0][1]).toEqual({ text: `${HASH_B} ${HASH_A}` }),
  );
});

test("context_menu_selects_the_row_and_copies_the_subject", async () => {
  mock({ clipboard_write: () => undefined, log_adjacent_rows: ({ parent }) => (parent ? [2] : [0]) });
  await panel();

  await fireEvent.contextMenu(scroller(), { clientX: X, clientY: ROW_HEIGHT + 2 });

  const menu = await screen.findByRole("menu", { name: "Commit" });
  await waitFor(() => expect(hashesAsked()).toEqual({ hashes: [HASH_B] }));
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Copy Commit Subject" }));

  await waitFor(() => expect(calls("clipboard_write")[0][1]).toEqual({ text: "two" }));
});

test("context_menu_keeps_a_selection_of_several", async () => {
  mock({ log_adjacent_rows: () => [] });
  await panel();
  await selectRow(0);
  await selectRow(1, { shiftKey: true });

  await fireEvent.contextMenu(scroller(), { clientX: X, clientY: 2 * ROW_HEIGHT + 2 });

  const menu = await screen.findByRole("menu", { name: "Commit" });
  expect(hashesAsked()).toEqual({ hashes: [HASH_A, HASH_B] });
  const items = within(menu).getAllByRole("menuitem") as HTMLButtonElement[];
  expect(items.find((item) => item.getAttribute("aria-label") === "New Tag…")?.disabled).toBe(true);
});

test("push_all_up_to_here_is_enabled_on_detached_head", async () => {
  mock({ log_adjacent_rows: () => [], log_current_branch: () => null });
  await panel({ onvcs: vi.fn() });
  await selectRow(0);

  await fireEvent.contextMenu(scroller(), { clientX: X, clientY: 2 });

  const menu = await screen.findByRole("menu", { name: "Commit" });
  const push = within(menu).getByRole("menuitem", { name: "Push All up to Here…" });
  expect((push as HTMLButtonElement).disabled).toBe(false);
});

test("push_all_up_to_here_reaches_whoever_performs_the_operations", async () => {
  mock({ log_adjacent_rows: () => [], log_current_branch: () => "main" });
  const onvcs = vi.fn();
  await panel({ onvcs });
  await selectRow(0);

  await fireEvent.contextMenu(scroller(), { clientX: X, clientY: 2 });

  const menu = await screen.findByRole("menu", { name: "Commit" });
  const push = within(menu).getByRole("menuitem", { name: "Push All up to Here…" });
  expect((push as HTMLButtonElement).disabled).toBe(false);
  await fireEvent.click(push);
  await waitFor(() => expect(onvcs).toHaveBeenCalledWith({ kind: "pushUpTo", hash: HASH_A }));
});

test("go_to_parent_from_the_context_menu", async () => {
  mock({ log_adjacent_rows: ({ parent }) => (parent ? [2] : []) });
  await panel();
  await fireEvent.contextMenu(scroller(), { clientX: X, clientY: 2 });
  const menu = await screen.findByRole("menu", { name: "Commit" });

  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Go to Parent Commit" }));

  await waitFor(() => expect(hashesAsked()).toEqual({ hashes: [HASH_C] }));
});

test("commit_tooltip_shows_its_references", async () => {
  const rows = ROWS.map((item) => ({
    ...item,
    refs: [
      { name: "main", kind: "local" as const },
      { name: "other", kind: "local" as const },
      { name: "v1", kind: "tag" as const },
    ],
  }));
  mock({ log_rows: ({ start }) => (start === 0 ? rows : []) });
  await panel();
  Object.defineProperty(scroller(), "clientHeight", { value: 240 });
  await selectRow(1);

  await shortcut(scroller(), { key: "F1" });

  const tooltip = await screen.findByRole("tooltip");
  expect([...tooltip.children].map((item) => item.textContent)).toEqual(["main,", "other", "v1"]);

  await shortcut(scroller(), { key: "ArrowDown" });
  await waitFor(() => expect(screen.queryByRole("tooltip")).toBeNull());
});

test("commit_tooltip_with_ctrl_j_and_a_limit_of_ten", async () => {
  const refs = Array.from({ length: 12 }, (_, index) => ({
    name: `r${index}`,
    kind: "local" as const,
  }));
  const rows = ROWS.map((item) => ({ ...item, refs }));
  mock({ log_rows: ({ start }) => (start === 0 ? rows : []) });
  await panel();
  Object.defineProperty(scroller(), "clientHeight", { value: 240 });
  await selectRow(0);

  await shortcut(scroller(), { key: "j", code: "KeyJ", ctrlKey: true });

  const tooltip = await screen.findByRole("tooltip");
  expect(tooltip.textContent).toContain("r9");
  expect(tooltip.textContent).not.toContain("r10");
  expect(tooltip.textContent).toContain("... 2 more in details pane");
});

test("commit_tooltip_needs_a_visible_commit_with_references", async () => {
  await panel();
  Object.defineProperty(scroller(), "clientHeight", { value: 240 });
  await selectRow(0);

  await shortcut(scroller(), { key: "F1" });
  await new Promise((resolve) => setTimeout(resolve));

  expect(screen.queryByRole("tooltip")).toBeNull();
});

test("click_on_an_arrow_jumps_to_its_row", async () => {
  withPrint(0, { arrows: [arrow()] });
  await panel();
  await selectRow(1);

  await fireEvent.click(scroller(), BELOW_NODE);

  await waitFor(() => expect(hashesAsked()).toEqual({ hashes: [HASH_C] }));
  expect(calls("log_graph_click")).toHaveLength(0);
  expect(document.activeElement).toBe(scroller());
});

test("hovering_an_arrow_shows_the_hand_and_the_tooltip", async () => {
  withPrint(0, { arrows: [arrow()] });
  await panel();

  await fireEvent.mouseMove(scroller(), BELOW_NODE);

  const tooltip = await screen.findByRole("tooltip");
  expect(tooltip.textContent).toMatch(/^Jump to "three" by Ana on .+ at .+$/);
  expect(tooltip.querySelector("b")?.textContent).toBe('"three"');
  expect(scroller().style.cursor).toBe("pointer");
  expect(calls("log_graph_hover")).toHaveLength(0);

  await fireEvent.mouseMove(scroller(), { clientX: X, clientY: 12 });

  await waitFor(() => expect(screen.queryByRole("tooltip")).toBeNull());
  expect(scroller().style.cursor).toBe("");
});

test("click_on_an_arrow_removes_the_tooltip", async () => {
  withPrint(0, { arrows: [arrow()] });
  await panel();
  await fireEvent.mouseMove(scroller(), BELOW_NODE);
  await screen.findByRole("tooltip");

  await fireEvent.click(scroller(), BELOW_NODE);

  await waitFor(() => expect(screen.queryByRole("tooltip")).toBeNull());
  expect(scroller().style.cursor).toBe("");
});

test("leaving_the_table_removes_the_tooltip", async () => {
  withPrint(0, { arrows: [arrow()] });
  await panel();
  await fireEvent.mouseMove(scroller(), BELOW_NODE);
  await screen.findByRole("tooltip");

  await fireEvent.mouseLeave(scroller());

  await waitFor(() => expect(screen.queryByRole("tooltip")).toBeNull());
});

test("hovering_a_fragment_shows_the_hand", async () => {
  mock({ log_graph_hover: () => [0, 1] });
  await panel();

  await fireEvent.mouseMove(scroller(), { clientX: 8, clientY: 12 });

  await waitFor(() => expect(scroller().style.cursor).toBe("pointer"));
  expect(calls("log_graph_hover")[0][1]).toEqual({
    viewId: 1,
    target: { kind: "node", row: 0 },
  });
});

test("hovering_what_does_not_fold_leaves_the_cursor", async () => {
  mock({ log_graph_hover: () => null });
  await panel();

  await fireEvent.mouseMove(scroller(), { clientX: 8, clientY: 12 });
  await waitFor(() => expect(calls("log_graph_hover")).toHaveLength(1));
  await new Promise((resolve) => setTimeout(resolve));

  expect(scroller().style.cursor).toBe("");
});

test("moving_the_mouse_over_the_same_element_does_not_repeat_the_question", async () => {
  mock({ log_graph_hover: () => [0] });
  await panel();

  await fireEvent.mouseMove(scroller(), { clientX: 8, clientY: 12 });
  await fireEvent.mouseMove(scroller(), { clientX: 9, clientY: 12 });
  await new Promise((resolve) => setTimeout(resolve));

  expect(calls("log_graph_hover")).toHaveLength(1);
});

test("an_old_hover_response_does_not_overwrite_the_new_one", async () => {
  const slow = deferred<number[]>();
  mock({ log_graph_hover: () => slow.promise });
  await panel();

  await fireEvent.mouseMove(scroller(), { clientX: 8, clientY: 12 });
  await fireEvent.mouseMove(scroller(), { clientX: X, clientY: 12 });
  slow.resolve([0]);
  await new Promise((resolve) => setTimeout(resolve));

  expect(scroller().style.cursor).toBe("");
});

test("only_one_hover_question_in_progress_and_then_the_last", async () => {
  const slow = deferred<number[]>();
  mock({ log_graph_hover: () => (calls("log_graph_hover").length === 1 ? slow.promise : [2]) });
  await panel();

  for (const clientY of [12, 36, 60]) await fireEvent.mouseMove(scroller(), { clientX: 8, clientY });
  await new Promise((resolve) => setTimeout(resolve));
  expect(calls("log_graph_hover")).toHaveLength(1);

  slow.resolve([0]);

  await waitFor(() => expect(calls("log_graph_hover")).toHaveLength(2));
  expect(calls("log_graph_hover")[1][1]).toEqual({ viewId: 1, target: { kind: "node", row: 2 } });
  await waitFor(() => expect(scroller().style.cursor).toBe("pointer"));
});

test("leaving_the_graph_with_a_question_in_progress_discards_the_next", async () => {
  const slow = deferred<number[]>();
  mock({ log_graph_hover: () => slow.promise });
  await panel();
  await fireEvent.mouseMove(scroller(), { clientX: 8, clientY: 12 });
  await fireEvent.mouseMove(scroller(), { clientX: 8, clientY: 36 });

  await fireEvent.mouseMove(scroller(), { clientX: X, clientY: 12 });
  slow.resolve([0]);
  await new Promise((resolve) => setTimeout(resolve));

  expect(calls("log_graph_hover")).toHaveLength(1);
  expect(scroller().style.cursor).toBe("");
});

test("click_on_a_dotted_edge_expands_it_and_does_not_select", async () => {
  withPrint(0, { edges: [edge({ style: "dotted", downRow: 2 })] });
  mock({
    log_graph_click: () => ({ changed: true, summary: { viewId: 1, count: 3, collapsible: true } }),
  });
  await panel();

  await fireEvent.click(scroller(), BELOW_NODE);

  await waitFor(() =>
    expect(calls("log_graph_click")[0][1]).toEqual({
      viewId: 1,
      target: { kind: "edge", up: 0, down: 2, style: "dotted" },
    }),
  );
  expect(calls("commit_details")).toHaveLength(0);
});

test("graph_click_during_a_sort_does_not_lose_the_order", async () => {
  const sort = deferred<{ viewId: number; count: number; collapsible: boolean }>();
  mock({
    log_view_set: () => sort.promise,
    log_graph_click: () => ({ changed: false, summary: { viewId: 1, count: 3, collapsible: true } }),
  });
  await panel();
  const menu = await openViewOptions();
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "IntelliSort" }));
  await fireEvent.click(await screen.findByRole("menuitem", { name: "Standard" }));
  await waitFor(() => expect(calls("log_view_set")).toHaveLength(1));

  await fireEvent.click(scroller(), { clientX: 8, clientY: 12 });
  await waitFor(() => expect(calls("log_graph_click")).toHaveLength(1));
  sort.resolve({ viewId: 1, count: 2, collapsible: true });

  await waitFor(() => expect(scroller().getAttribute("aria-rowcount")).toBe("2"));
});

test("branches_panel_open_on_the_main_tab_and_can_be_hidden", async () => {
  mock();
  await panel();

  expect(await screen.findByRole("tree", { name: "Branches" })).toBeTruthy();

  await fireEvent.click(screen.getByRole("button", { name: "Hide Git Branches" }));

  expect(screen.queryByRole("tree", { name: "Branches" })).toBeNull();
  await fireEvent.click(screen.getByRole("button", { name: "Branches" }));
  expect(await screen.findByRole("tree", { name: "Branches" })).toBeTruthy();
});

test("double_click_on_a_panel_branch_filters_the_log", async () => {
  mock({
    log_view_open: () =>
      deliverTask({ viewId: 2, summary: { viewId: 2, count: 3, collapsible: true } }),
  });
  await panel();

  const branch = await screen.findByRole("treeitem", { name: "lateral" });
  await fireEvent.click(branch);
  await fireEvent.dblClick(branch);

  await waitFor(() => expect(calls("log_view_open")).toHaveLength(1));
  expect(calls("log_view_open")[0][1]).toMatchObject({ spec: { filter: { branches: ["lateral"] } } });
});

test("the_panel_arrows_do_not_move_the_log", async () => {
  mock();
  await panel();
  const tree = await screen.findByRole("tree", { name: "Branches" });
  await selectRow(0);
  vi.mocked(invoke).mockClear();

  await fireEvent.keyDown(tree, { key: "ArrowRight" });

  expect(calls("log_adjacent_rows")).toEqual([]);
});

// Cherry-Pick Selected Changes and Revert Selected Changes: the menu of the panel's files.
async function fileMenu(name: string) {
  const item = await within(changes()).findByRole("treeitem", { name });
  await fireEvent.contextMenu(item);
  return screen.findByRole("menu");
}

test("a_file_menu_offers_revert_and_cherry_pick_selected_changes", async () => {
  const onvcs = vi.fn();
  await panel({ onvcs });
  await selectRow(0);

  const menu = await fileMenu("a.txt");

  const labels = within(menu).getAllByRole("menuitem").map((item) => item.textContent?.trim());
  expect(labels).toEqual([
    "Compare with Local",
    "Compare Before with Local",
    "Edit Source ⌘↓",
    "Open Repository Version",
    "Revert Selected Changes",
    "Cherry-Pick Selected Changes",
    "Create Patch...",
    "Get from Revision",
    "History Up to Here",
  ]);
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Revert Selected Changes" }));
  expect(onvcs).toHaveBeenCalledWith({
    kind: "applyPaths",
    reverse: true,
    selection: [{ hash: HASH_A, base: "f".repeat(40), paths: ["src/a.txt"] }],
    hashes: [HASH_A],
  });
});

// `Vcs.RepositoryChangesBrowserMenu` has `Create Patch...` (no Copy as Patch): the chosen files with
// the base the panel shows.
test("create_patch_of_the_changes_tree_takes_the_chosen_files", async () => {
  mock({
    commit_changes_of: () => [group([file("src/a.txt"), file("src/b.txt", "R", "src/old.txt")])],
  });
  const oncreatepatch = vi.fn();
  await panel({ oncreatepatch });
  await selectRow(0);
  await within(changes()).findByRole("treeitem", { name: "a.txt" });

  await fireEvent.click(within(changes()).getByRole("treeitem", { name: "a.txt" }));
  await fireEvent.click(within(changes()).getByRole("treeitem", { name: "b.txt" }), { metaKey: true });
  const menu = await fileMenu("b.txt");
  expect(within(menu).queryByRole("menuitem", { name: "Copy as Patch to Clipboard" })).toBeNull();
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Create Patch..." }));

  const chosen = oncreatepatch.mock.calls[0][0] as { file: { path: string; oldPath: string | null }; hash: string; base: string }[];
  expect(chosen.map(({ file, hash, base }) => [file.path, file.oldPath, hash, base])).toEqual([
    ["src/a.txt", null, HASH_A, "f".repeat(40)],
    ["src/b.txt", "src/old.txt", HASH_A, "f".repeat(40)],
  ]);
});

// `VcsDataKeys.CHANGES` of the log is `myChangesBrowser.getDirectChanges()` (`MainFrame.java:267`):
// the menu's Create Patch... takes what the changes panel has loaded and never waits for git; while
// nothing is loaded it is off.
test("the_log_menu_takes_the_loaded_changes_and_never_waits", async () => {
  let answer: (groups: unknown) => void = () => {};
  mock({
    commit_changes_of: () => new Promise((resolve) => (answer = resolve)),
    log_adjacent_rows: () => [],
  });
  const oncreatepatch = vi.fn();
  await panel({ oncreatepatch });
  await selectRow(0);
  await waitFor(() => expect(calls("commit_changes_of")).toHaveLength(1));

  // The changes are still loading: the menu opens at once, with Create Patch... off.
  await fireEvent.contextMenu(scroller(), { clientX: X, clientY: 2 });
  let menu = await screen.findByRole("menu", { name: "Commit" });
  expect((within(menu).getByRole("menuitem", { name: "Create Patch..." }) as HTMLButtonElement).disabled).toBe(true);
  await fireEvent.keyDown(menu, { key: "Escape" });

  answer([group([file("src/a.txt")])]);
  await within(changes()).findByRole("treeitem", { name: "a.txt" });
  await fireEvent.contextMenu(scroller(), { clientX: X, clientY: 2 });
  menu = await screen.findByRole("menu", { name: "Commit" });
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Create Patch..." }));

  expect(calls("commit_changes_of")).toHaveLength(1);
  expect(calls("commit_changes_start")).toHaveLength(0);
  const chosen = oncreatepatch.mock.calls[0][0] as { file: { path: string }; hash: string }[];
  expect(chosen.map(({ file, hash }) => [file.path, hash])).toEqual([["src/a.txt", HASH_A]]);
});

// `directChanges = commitModel.changes` (`VcsLogChangesBrowser.kt:263-264`): with a Paths filter
// and Show Only Affected Changes on, the tree shows only the affected files, but the menu's patch
// takes all of the commit's changes.
test("the_log_menu_patch_takes_all_the_changes_whatever_the_tree_shows", async () => {
  mock({
    commit_changes_of: () => [group([file("src/a.txt"), file("other.txt")])],
    log_adjacent_rows: () => [],
    log_view_open: () => deliverTask({ viewId: 2, summary: { viewId: 2, count: 3, collapsible: true } }),
  });
  const oncreatepatch = vi.fn();
  await panel({ oncreatepatch });
  await openLogTab("log", "paths", () => ({
    name: "",
    filters: { ...emptyFilters(), paths: ["src"] },
    options: { showOnlyAffected: true },
  }));
  await waitFor(() => expect(calls("log_view_open")).toHaveLength(1));
  await selectRow(0);
  await within(changes()).findByRole("treeitem", { name: "a.txt" });
  expect(within(changes()).queryByRole("treeitem", { name: "other.txt" })).toBeNull();

  await fireEvent.contextMenu(scroller(), { clientX: X, clientY: 2 });
  const menu = await screen.findByRole("menu", { name: "Commit" });
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Create Patch..." }));

  const chosen = oncreatepatch.mock.calls[0][0] as { file: { path: string } }[];
  expect(chosen.map(({ file }) => file.path)).toEqual(["src/a.txt", "other.txt"]);
});

test("cherry_pick_selected_changes_does_not_revert", async () => {
  const onvcs = vi.fn();
  await panel({ onvcs });
  await selectRow(0);

  await fileMenu("a.txt");
  await fireEvent.click(screen.getByRole("menuitem", { name: "Cherry-Pick Selected Changes" }));

  expect(onvcs).toHaveBeenCalledWith(expect.objectContaining({ kind: "applyPaths", reverse: false }));
});

test("with_several_files_chosen_the_menu_carries_all_and_the_rename_with_both_paths", async () => {
  mock({
    commit_changes_of: () => [
      group([file("src/a.txt"), file("src/b.txt", "R", "src/old.txt"), file("c.txt")]),
    ],
  });
  const onvcs = vi.fn();
  await panel({ onvcs });
  await selectRow(0);
  await within(changes()).findByRole("treeitem", { name: "a.txt" });

  await fireEvent.click(within(changes()).getByRole("treeitem", { name: "a.txt" }));
  await fireEvent.click(within(changes()).getByRole("treeitem", { name: "b.txt" }), {
    metaKey: true,
  });
  await fileMenu("b.txt");
  await fireEvent.click(screen.getByRole("menuitem", { name: "Revert Selected Changes" }));

  // c.txt was not chosen. Each file with the base the panel shows.
  expect(onvcs.mock.calls[0][0].selection).toEqual([
    { hash: HASH_A, base: "f".repeat(40), paths: ["src/a.txt", "src/b.txt", "src/old.txt"] },
  ]);
});

test("right_click_on_an_unchosen_file_chooses_only_it", async () => {
  mock({ commit_changes_of: () => [group([file("src/a.txt"), file("c.txt")])] });
  const onvcs = vi.fn();
  await panel({ onvcs });
  await selectRow(0);
  await within(changes()).findByRole("treeitem", { name: "a.txt" });

  await fileMenu("c.txt");
  await fireEvent.click(screen.getByRole("menuitem", { name: "Revert Selected Changes" }));

  expect(onvcs.mock.calls[0][0].selection[0].paths).toEqual(["c.txt"]);
  expect(
    within(changes()).getByRole("treeitem", { name: "a.txt" }).getAttribute("aria-selected"),
  ).toBe("false");
});
