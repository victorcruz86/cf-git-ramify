import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { BlockLines, SelectionRequest, SelectionRevision, SelectionUpdate } from "../lib/api";
import { createTextsView } from "../diff/textsView";
import SelectionHistoryWindow from "./SelectionHistoryWindow.svelte";
import {
  blockDiff,
  emptyData,
  keptSelection,
  statusText,
  visibleRows,
  withUpdate,
  type HistoryData,
} from "./selection";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(), emit: vi.fn() }));
// The block diff's views, counted.
vi.mock("../diff/textsView", async (original) => {
  const real = await original<typeof import("../diff/textsView")>();
  return { createTextsView: vi.fn(real.createTextsView) };
});

// jsdom does no layout: CodeMirror measures with this.
Range.prototype.getClientRects = () => [] as unknown as DOMRectList;

// Each stream's task id, never reused: `taskResult` keeps the ends of finished tasks.
let lastTask = 100;
const NEWEST = "1111111111111111111111111111111111111111";
const MIDDLE = "2222222222222222222222222222222222222222";
const OLDEST = "3333333333333333333333333333333333333333";

const handlers = new Map<string, (payload: unknown) => void>();
let pending: SelectionRequest | null = null;
let more: Record<string, (args: never) => unknown> = {};
// What `github_links` answers: the GitHub repositories of the remotes.
let links: { name: string; url: string }[] = [];

const target = (): SelectionRequest => ({
  path: "src/a.txt",
  startLine: 1,
  endLine: 1,
  localText: "a\nb3\nc\n",
  stream: 0,
});

const revision = (hash: string, message: string, change: Partial<SelectionRevision> = {}) => ({
  hash,
  path: "src/a.txt",
  commitTime: 1_700_000_000,
  author: "Ann",
  authorEmail: "ann@example.com",
  committer: "Ann",
  committerEmail: "ann@example.com",
  message,
  ...change,
});

const block = (start: number, ...lines: string[]): BlockLines => ({ start, lines });

function calls(command: string) {
  return vi.mocked(invoke).mock.calls.filter(([name]) => name === command);
}

// The stream the window started: the `stream` it was given.
const started = () => (calls("selection_history_start").at(-1)?.[1] as { request: SelectionRequest })
  .request;

// An update without its stream, which `update` adds.
type Unstreamed<T> = T extends unknown ? Omit<T, "stream"> : never;

function update(change: Unstreamed<SelectionUpdate>, stream = started().stream) {
  handlers.get("selection-history-update")?.({ ...change, stream });
}

function finishTask(state: "done" | "failed" | "cancelled", text = "") {
  handlers.get("task-changed")?.({
    id: lastTask,
    title: "",
    text,
    fraction: null,
    state,
    kind: state === "done" ? null : state,
    value: null,
  });
}

// Three revisions whose block changes in each one, and the local text, which changes it again.
function streamAll() {
  update({ kind: "block", index: 0, block: block(1, "b3") });
  update({
    kind: "revisions",
    localTime: 1_700_000_500,
    revisions: [
      revision(NEWEST, "third\n\nwith a body", { committer: "Bob", committerEmail: "bob@example.com" }),
      revision(MIDDLE, "second"),
      revision(OLDEST, "first"),
    ],
  });
  update({ kind: "block", index: 1, block: block(1, "b2") });
  update({ kind: "block", index: 2, block: block(1, "b1") });
  update({ kind: "block", index: 3, block: block(1, "b0") });
}

const table = () => screen.getByRole("grid", { name: "Revisions" });
const rows = () => within(table()).queryAllByRole("row").slice(1);
const cells = () =>
  rows().map((row) => within(row).getAllByRole("gridcell").map((cell) => cell.textContent));

async function openWindow() {
  pending = target();
  render(SelectionHistoryWindow);
  await waitFor(() => expect(calls("selection_history_start")).toHaveLength(1));
}

beforeEach(() => {
  localStorage.clear();
  handlers.clear();
  more = {};
  links = [];
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    switch (command) {
      case "selection_history_target": {
        const taken = pending;
        pending = null;
        return taken;
      }
      case "selection_history_start":
        return ++lastTask;
      case "show_window":
      case "task_cancel":
      case "selection_history_close":
        return undefined;
      case "github_links":
        return links;
      default:
        if (more[command]) return more[command](args as never);
        throw new Error(`unexpected command: ${command}`);
    }
  });
  vi.mocked(listen).mockReset();
  vi.mocked(listen).mockImplementation(async (event, handler) => {
    handlers.set(event as string, (payload) =>
      handler({ event: event as string, id: 0, payload } as never),
    );
    return () => {};
  });
});

afterEach(cleanup);

// `VcsSelectionHistoryDialog`: the table `Version | Date | Author | Commit Message` with `Local
// Changes` first, `Changes only` on, the status `Loading revision {0}… (n/total)` while it reads,
// and `Commit Message:` of the row selected.
test("the_window_shows_the_revisions_while_their_blocks_load", async () => {
  await openWindow();
  expect(started()).toMatchObject({ ...target(), stream: expect.any(Number) });
  expect(screen.getByText("Loading affected revisions…")).toBeTruthy();
  expect(screen.getByRole("status").textContent).toBe("Loading...");
  expect(within(table()).getAllByRole("columnheader").map((cell) => cell.textContent)).toEqual([
    "Version",
    "Date",
    "Author",
    "Commit Message",
  ]);
  expect((screen.getByRole("checkbox", { name: "Changes only" }) as HTMLInputElement).checked).toBe(
    true,
  );

  update({ kind: "block", index: 0, block: block(1, "b3") });
  update({
    kind: "revisions",
    localTime: null,
    revisions: [revision(NEWEST, "third"), revision(MIDDLE, "second")],
  });
  await waitFor(() =>
    expect(screen.getByRole("status").textContent).toBe("Loading revision 11111111… (1/3)"),
  );

  update({ kind: "block", index: 1, block: block(1, "b2") });
  await waitFor(() =>
    expect(screen.getByRole("status").textContent).toBe("Loading revision 22222222… (2/3)"),
  );
  // With `Changes only`, a row shows once the older block says it changed.
  expect(cells().map((row) => row[0])).toEqual(["Local Changes"]);

  update({ kind: "block", index: 2, block: block(1, "b1") });
  finishTask("done");
  await waitFor(() => expect(screen.getByRole("status").textContent).toBe(""));
  expect(cells()).toEqual([
    ["Local Changes", "", "", "[Current]"],
    ["11111111", expect.any(String), "Ann", "third"],
    ["22222222", expect.any(String), "Ann", "second"],
  ]);
  // The first row is selected (`getNearestVisibleRevision` with nothing selected).
  expect(rows()[0].getAttribute("aria-selected")).toBe("true");
  expect(screen.getByLabelText("Commit message").textContent).toBe("[Current]");
});

describe("changes_only_keeps_revisions_whose_block_differs", () => {
  const data = (blocks: (BlockLines | null)[]): HistoryData => ({
    ...withUpdate(emptyData(), {
      kind: "revisions",
      stream: 1,
      localTime: null,
      revisions: [revision(NEWEST, "3"), revision(MIDDLE, "2"), revision(OLDEST, "1")],
    }),
    blocks,
    loading: false,
  });

  test("a row whose block differs from the older one's, and the oldest with a block", () => {
    // Local = newest, newest ≠ middle, middle = oldest.
    const same = data([block(0, "x"), block(0, "x"), block(0, "y"), block(0, "y")]);
    expect(visibleRows(same, true)).toEqual([1, 3]);
    expect(visibleRows(same, false)).toEqual([0, 1, 2, 3]);
  });

  test("the block gone in a revision ends the list there, and the oldest is not added", () => {
    const gone = data([block(0, "x"), block(0, "y"), null, null]);
    expect(visibleRows(gone, true)).toEqual([0, 1]);
  });

  test("while blocks are missing, only the rows they already tell", () => {
    expect(visibleRows(data([block(0, "x"), block(0, "y")]), true)).toEqual([0]);
  });

  test("a selection keeps what is still shown, else the nearest newer row shown", () => {
    expect(keptSelection([1, 2], [1, 3])).toEqual([1]);
    expect(keptSelection([2], [0, 1, 3])).toEqual([1]);
    expect(keptSelection([2], [0, 3])).toEqual([0]);
    expect(keptSelection([], [2, 3])).toEqual([2]);
    expect(keptSelection([1], [])).toEqual([]);
  });

  test("the window's checkbox shows every row and is remembered", async () => {
    await openWindow();
    update({ kind: "block", index: 0, block: block(1, "b") });
    update({
      kind: "revisions",
      localTime: null,
      revisions: [revision(NEWEST, "third"), revision(MIDDLE, "second"), revision(OLDEST, "first")],
    });
    update({ kind: "block", index: 1, block: block(1, "b") });
    update({ kind: "block", index: 2, block: block(1, "a") });
    update({ kind: "block", index: 3, block: block(1, "a") });
    finishTask("done");
    await waitFor(() => expect(cells().map((row) => row[0])).toEqual(["11111111", "33333333"]));

    await fireEvent.click(screen.getByRole("checkbox", { name: "Changes only" }));

    expect(cells().map((row) => row[0])).toEqual([
      "Local Changes",
      "11111111",
      "22222222",
      "33333333",
    ]);
    expect(localStorage.getItem("ramify.selectionHistory.changesOnly")).toBe("false");
  });
});

// `closeOnEsc`: the window closes and its stream is cancelled; unmounting cancels it too.
test("closing_the_window_cancels_the_stream", async () => {
  await openWindow();
  update({ kind: "block", index: 0, block: block(1, "b3") });

  await fireEvent.keyDown(window, { key: "Escape" });

  await waitFor(() => expect(calls("selection_history_close")).toHaveLength(1));
  expect(calls("task_cancel")).toEqual([["task_cancel", { id: lastTask }]]);
});

test("unmounting_cancels_the_stream", async () => {
  await openWindow();
  await new Promise((resolve) => setTimeout(resolve, 0));
  cleanup();
  expect(calls("task_cancel")).toEqual([["task_cancel", { id: lastTask }]]);
});

// `updateDiff`: the block after the selection on the left, the first selected on the right, titled
// `Revision {0}` with the full hash; line numbers from where each block starts.
test("the_block_diff_compares_the_selection_with_the_row_after_it", async () => {
  await openWindow();
  streamAll();
  finishTask("done");
  await waitFor(() => expect(rows()).toHaveLength(4));

  await fireEvent.click(rows()[1]);
  const titles = [...document.querySelectorAll(".titles .title")].map((title) => title.textContent);
  expect(titles).toEqual([`Revision ${MIDDLE}`, `Revision ${NEWEST}`]);
  expect(screen.getByLabelText("Commit message").textContent).toBe("third\n\nwith a body");
  // Someone else committed it: `*`, and the tooltip names both.
  const author = within(rows()[1]).getAllByRole("gridcell")[2];
  expect(author.textContent).toBe("Ann*");
  expect(author.getAttribute("title")).toBe("Ann <ann@example.com>, via Bob <bob@example.com>");

  await fireEvent.click(rows()[3]);
  const last = [...document.querySelectorAll(".titles .title")].map((title) => title.textContent);
  expect(last).toEqual(["", `Revision ${OLDEST}`]);

  await fireEvent.click(rows()[0]);
  await fireEvent.click(rows()[3], { metaKey: true });
  expect(rows().map((row) => row.getAttribute("aria-selected"))).toEqual([
    "true",
    "false",
    "false",
    "true",
  ]);
  expect(screen.getByLabelText("Commit message").textContent).toBe("");
});

// `setRequest(request, IntPair)` and the 300 ms queue: the stream's updates for other revisions
// leave the diff of the selected row as it is built; another selection builds it again.
test("the_block_diff_is_built_once_per_pair_while_the_stream_runs", async () => {
  await openWindow();
  vi.mocked(createTextsView).mockClear();
  update({ kind: "block", index: 0, block: block(1, "b3") });
  update({
    kind: "revisions",
    localTime: null,
    revisions: [1, 2, 3, 4, 5, 6].map((at) => revision(String(at).repeat(40), `commit ${at}`)),
  });
  update({ kind: "block", index: 1, block: block(1, "b2") });
  await waitFor(() => expect(createTextsView).toHaveBeenCalledTimes(1));
  expect(rows()[0].getAttribute("aria-selected")).toBe("true");

  for (const index of [2, 3, 4, 5]) {
    update({ kind: "block", index, block: block(1, `b${index}`) });
    await new Promise((resolve) => setTimeout(resolve, 120));
  }
  await waitFor(() =>
    expect(screen.getByRole("status").textContent).toBe("Loading revision 66666666… (6/7)"),
  );
  update({ kind: "block", index: 6, block: block(1, "b6") });
  finishTask("done");
  await waitFor(() => expect(screen.getByRole("status").textContent).toBe(""));
  expect(createTextsView).toHaveBeenCalledTimes(1);

  await fireEvent.click(rows()[1]);
  expect(createTextsView).toHaveBeenCalledTimes(2);
});

test("the_diff_says_what_it_cannot_show", () => {
  const loading = withUpdate(emptyData(), { kind: "block", stream: 1, index: 0, block: block(0) });
  expect(blockDiff(loading, [])).toEqual({ kind: "message", text: "Nothing to show" });
  expect(blockDiff(loading, [0])).toEqual({ kind: "message", text: "Loading..." });
  const listed = withUpdate(loading, {
    kind: "revisions",
    stream: 1,
    localTime: null,
    revisions: [revision(NEWEST, "1")],
  });
  expect(blockDiff(listed, [0])).toEqual({ kind: "message", text: "Loading..." });
  expect(blockDiff({ ...listed, loading: false, error: "broken" }, [0])).toEqual({
    kind: "message",
    text: "Cannot load revision contents: broken",
  });
  expect(statusText({ ...listed, loading: false })).toBe("");
});

// `VcsSelectionHistoryDialog.Popup`: `Compare` (`MyDiffAction`), `Compare with Local`,
// `Show All Affected Files` and `Copy Revision Number`, by what is selected.
test("the_popup_acts_on_the_selected_revisions", async () => {
  more = {
    compare_local_state: () => ({ root: "/repo", modified: true, current: null, headLabel: "main" }),
    diff_window_open_request: () => undefined,
    selection_history_show_affected: () => undefined,
    clipboard_write: () => undefined,
    annotation_show_in_log: () => undefined,
  };
  await openWindow();
  streamAll();
  finishTask("done");
  await waitFor(() => expect(rows()).toHaveLength(4));
  const popup = async (row: HTMLElement) => {
    await fireEvent.contextMenu(row);
    const menu = await screen.findByRole("menu", { name: "Revisions" });
    return [...menu.querySelectorAll('[role="menuitem"]')].map((item) => [
      item.getAttribute("aria-label"),
      (item as HTMLButtonElement).disabled,
    ]);
  };

  expect(await popup(rows()[0])).toEqual([
    ["Compare", true],
    ["Compare with Local", true],
    ["Show All Affected Files", true],
    ["Copy Revision Number", true],
    ["Show in Git Log", true],
  ]);
  await fireEvent.keyDown(screen.getByRole("menu", { name: "Revisions" }), { key: "Escape" });

  expect(await popup(rows()[2])).toEqual([
    ["Compare", false],
    ["Compare with Local", false],
    ["Show All Affected Files", false],
    ["Copy Revision Number", false],
    ["Show in Git Log", false],
  ]);
  await fireEvent.click(screen.getByRole("menuitem", { name: "Compare" }));
  await waitFor(() =>
    expect(calls("diff_window_open_request").at(-1)?.[1]).toEqual({
      request: {
        kind: "revisions",
        files: [
          {
            left: { revision: OLDEST, path: "src/a.txt", title: OLDEST },
            right: { revision: MIDDLE, path: "src/a.txt", title: MIDDLE },
          },
        ],
        title: "a.txt (/repo/src)",
      },
    }),
  );

  await popup(rows()[2]);
  await fireEvent.click(screen.getByRole("menuitem", { name: "Compare with Local" }));
  await waitFor(() =>
    expect(calls("diff_window_open_request").at(-1)?.[1]).toMatchObject({
      request: {
        files: [
          {
            left: { revision: MIDDLE, title: MIDDLE },
            right: { revision: "local", path: "src/a.txt", title: "Local Changes (Local)" },
          },
        ],
      },
    }),
  );

  await popup(rows()[2]);
  await fireEvent.click(screen.getByRole("menuitem", { name: "Show in Git Log" }));
  await waitFor(() =>
    expect(calls("annotation_show_in_log")).toEqual([["annotation_show_in_log", { hash: MIDDLE }]]),
  );

  await popup(rows()[2]);
  await fireEvent.click(screen.getByRole("menuitem", { name: "Show All Affected Files" }));
  await waitFor(() =>
    expect(calls("selection_history_show_affected")).toEqual([
      ["selection_history_show_affected", { affected: { hash: MIDDLE, path: "src/a.txt" } }],
    ]),
  );

  // Local Changes and two revisions: compared from the oldest's next row to Local; copied without
  // Local, oldest first.
  await fireEvent.click(rows()[0]);
  await fireEvent.click(rows()[2], { shiftKey: true });
  expect(await popup(rows()[1])).toEqual([
    ["Compare", false],
    ["Compare with Local", true],
    ["Show All Affected Files", true],
    ["Copy Revision Number", false],
    ["Show in Git Log", true],
  ]);
  await fireEvent.click(screen.getByRole("menuitem", { name: "Copy Revision Number" }));
  await waitFor(() =>
    expect(calls("clipboard_write")).toEqual([
      ["clipboard_write", { text: `${MIDDLE} ${NEWEST}` }],
    ]),
  );
  await fireEvent.keyDown(window, { key: "d", metaKey: true });
  await waitFor(() =>
    expect(calls("diff_window_open_request").at(-1)?.[1]).toMatchObject({
      request: {
        files: [
          {
            left: { revision: OLDEST, title: OLDEST },
            right: { revision: "local", title: "Local Changes (Local)" },
          },
        ],
        title: "a.txt (/repo/src)",
      },
    }),
  );
});

// One task per window: a new selection cancels the stream before it, whose late updates and end
// are dropped; a failure says `Cannot load revision contents: …`.
test("a_new_selection_replaces_the_stream_and_a_failure_is_shown", async () => {
  await openWindow();
  const first = started().stream;
  const firstTask = lastTask;
  update({ kind: "block", index: 0, block: block(1, "old") });

  pending = { ...target(), path: "src/b.txt" };
  handlers.get("selection-history-open")?.(null);
  await waitFor(() => expect(calls("selection_history_start")).toHaveLength(2));
  expect(calls("task_cancel")).toEqual([["task_cancel", { id: firstTask }]]);
  expect(started()).toMatchObject({ path: "src/b.txt" });
  expect(started().stream).not.toBe(first);

  update(
    {
      kind: "revisions",
      localTime: null,
      revisions: [revision(NEWEST, "from the old stream")],
    },
    first,
  );
  expect(screen.getByText("Loading affected revisions…")).toBeTruthy();

  update({ kind: "block", index: 0, block: block(1, "b") });
  update({ kind: "revisions", localTime: null, revisions: [revision(MIDDLE, "second")] });
  finishTask("failed", "fatal: bad object");
  expect(await screen.findByRole("alert")).toHaveProperty(
    "textContent",
    expect.stringContaining("Cannot load revision contents: fatal: bad object"),
  );
  expect(screen.getByRole("status").textContent).toBe("");
});

// `VcsSelectionHistoryDialog.Popup` ends with `Git.Hosting.Open.In.Browser.Group` (`g4x:522`): the
// commit URL of the first selected row. With no GitHub repository it is not there.
test("each_menu_has_open_on_github_at_241s_place_selection_history", async () => {
  links = [{ name: "https://github.com/o/r", url: `https://github.com/o/r/commit/${MIDDLE}` }];
  more = {
    compare_local_state: () => ({ root: "/repo", modified: true, current: null, headLabel: "main" }),
    open_url: () => undefined,
  };
  await openWindow();
  streamAll();
  finishTask("done");
  await waitFor(() => expect(rows()).toHaveLength(4));

  await fireEvent.contextMenu(rows()[2]);
  const menu = await screen.findByRole("menu", { name: "Revisions" });
  const labels = [...menu.querySelectorAll('[role="menuitem"]')].map((item) =>
    item.getAttribute("aria-label"),
  );
  expect(labels.at(-2)).toBe("Show in Git Log");
  expect(labels.at(-1)).toBe("Open on GitHub");

  await fireEvent.click(screen.getByRole("menuitem", { name: "Open on GitHub" }));
  expect(calls("github_links").at(-1)?.[1]).toEqual({ target: { kind: "commit", hash: MIDDLE } });
  expect(calls("open_url")).toEqual([["open_url", { url: links[0].url }]]);
});
