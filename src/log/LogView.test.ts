import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { afterAll, afterEach, beforeAll, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { LogRow, Signature, TaskEvent } from "../lib/api";
import { trackTasks } from "../lib/tasks";
import { ROW_HEIGHT } from "./draw";
import LogView from "./LogView.svelte";
import { defaultOptions, toggleColumn, type LogColumn } from "./options";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({ label: "main" }),
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));

const HASH = "a".repeat(40);

const row: LogRow = {
  hash: HASH,
  subject: "one",
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

beforeAll(() => {
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
  HTMLCanvasElement.prototype.getContext = (() => null) as never;
});

afterAll(() => {
  vi.unstubAllGlobals();
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

// A tab switch (`{#key current.id}`) or the end of the panel destroys the table while it waits for
// git to say where a commit is: what comes back afterwards has no table to select or scroll in.
test("a_selection_that_arrives_after_the_table_is_gone_does_nothing", async () => {
  const answers: ((rows: (number | null)[]) => void)[] = [];
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "log_rows") return [row];
    if (command === "log_find_rows") return new Promise((resolve) => answers.push(resolve));
    throw new Error(`unexpected command: ${command}`);
  });
  const shown = render(LogView, {
    viewId: 1,
    count: 1,
    generation: 0,
    options: defaultOptions(),
    onselect: vi.fn(),
  });
  const table = shown.component as unknown as LogView;

  const selecting = table.selectHash(HASH);
  const restoring = table.restore(
    { hashes: [HASH], onTop: false, target: { hash: HASH, gap: 0 } },
    false,
  );
  shown.unmount();
  answers.forEach((answer) => answer([0]));

  await expect(selecting).resolves.toBe(false);
  await expect(restoring).resolves.toBeUndefined();
});

// One animation frame of the fake timers, and a bit.
const FRAME = 20;
const hashOf = (index: number) => index.toString(16).padStart(40, "0");
const rowOf = (index: number): LogRow => ({ ...row, hash: hashOf(index), subject: `commit ${index}` });
const hashesOf = (start: number, end: number) =>
  Array.from({ length: end - start }, (_, index) => hashOf(start + index));
const verified = (index: number): Signature => ({
  hash: hashOf(index),
  status: "verified",
  signer: "someone@example.com",
  fingerprint: "SHA256:abc",
});

// The size jsdom does not give: 10 rows on screen, 800 wide.
function withViewport(height: number, width: number) {
  // `Element`'s own getters stay: the ones defined here only hide them.
  for (const name of ["clientHeight", "clientWidth"] as const) {
    Object.defineProperty(HTMLElement.prototype, name, {
      configurable: true,
      get: () => (name === "clientHeight" ? height : width),
    });
  }
  return () => {
    delete (HTMLElement.prototype as { clientHeight?: number }).clientHeight;
    delete (HTMLElement.prototype as { clientWidth?: number }).clientWidth;
  };
}

// Review Focus 3. 241's `loadDataForVisibleRows`: nothing while the column is hidden; once shown,
// the visible rows ±15, 300 ms after the last change; each scroll asks again with the same loader,
// whose new request cancels the old one (the backend does it per requester, `signatures.rs`), and
// the old one's late answer is dropped. Hiding it cancels what it was asking for.
test("signatures_load_only_while_shown_for_the_visible_rows_plus_fifteen_and_a_new_request_cancels_the_old", async () => {
  vi.useFakeTimers();
  const restoreViewport = withViewport(10 * ROW_HEIGHT, 800);
  let onTask: ((event: { payload: TaskEvent }) => void) | null = null;
  vi.mocked(listen).mockImplementation(async (_, handler) => {
    onTask = handler as never;
    return () => {};
  });
  await trackTasks();
  const asked: { id: number; hashes: string[]; requester: number }[] = [];
  const cancelled: number[] = [];
  let nextTask = 1;
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    const values = args as Record<string, unknown>;
    if (command === "log_rows") {
      const start = values.start as number;
      return Array.from({ length: Math.max(0, Math.min(100 - start, values.count as number)) }, (_, index) =>
        rowOf(start + index),
      );
    }
    if (command === "log_signatures") {
      const id = nextTask++;
      asked.push({ id, hashes: values.hashes as string[], requester: values.requester as number });
      return id;
    }
    if (command === "task_cancel") {
      cancelled.push(values.id as number);
      return undefined;
    }
    throw new Error(`unexpected command: ${command}`);
  });
  const answer = (id: number, value: Signature[] | null) =>
    onTask!({
      payload: {
        id,
        title: "Loading git commit signatures",
        text: "",
        fraction: null,
        state: value ? "done" : "cancelled",
        kind: value ? null : "cancelled",
        value,
      },
    });
  const hidden = defaultOptions();
  const shown = { ...hidden, columns: toggleColumn(hidden.columns, "signature") };
  const view = render(LogView, { viewId: 1, count: 100, generation: 0, options: hidden, onselect: vi.fn() });
  const grid = screen.getByRole("grid", { name: "Commits" });

  try {
    await vi.advanceTimersByTimeAsync(1_000);
    expect(asked).toEqual([]);

    // The next frame paints and starts waiting.
    await view.rerender({ options: shown });
    await vi.advanceTimersByTimeAsync(FRAME);
    await vi.advanceTimersByTimeAsync(280);
    expect(asked).toEqual([]);
    await vi.advanceTimersByTimeAsync(FRAME + 20);
    expect(asked.map(({ hashes }) => hashes)).toEqual([hashesOf(0, 25)]);

    // Rows 40 to 49 on screen.
    grid.scrollTop = 40 * ROW_HEIGHT;
    await fireEvent.scroll(grid);
    await vi.advanceTimersByTimeAsync(FRAME);
    await vi.advanceTimersByTimeAsync(280);
    expect(asked).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(FRAME + 20);
    expect(asked.map(({ hashes }) => hashes)).toEqual([hashesOf(0, 25), hashesOf(25, 65)]);
    expect(asked[1].requester).toBe(asked[0].requester);

    // The old answer arrives late, then the new one.
    answer(asked[0].id, [verified(41)]);
    answer(asked[1].id, [verified(42)]);
    await vi.advanceTimersByTimeAsync(20);
    const tooltipOf = async (index: number) => {
      // The signature cell: after Subject, Author (150) and Date (100), 24 wide.
      await fireEvent.mouseMove(grid, { clientX: 790, clientY: (index - 40) * ROW_HEIGHT + 2 });
      return screen.queryByRole("tooltip")?.textContent?.trim();
    };
    expect(await tooltipOf(42)).toBe("Verified GPG signature");
    expect(await tooltipOf(41)).toBe("No GPG signature");
    expect(await tooltipOf(43)).toBe("No GPG signature");
    await fireEvent.mouseMove(grid, { clientX: 300, clientY: 2 });
    expect(screen.queryByRole("tooltip")).toBeNull();

    // A change that is still waiting, and one that is running, both stop when it is hidden.
    grid.scrollTop = 60 * ROW_HEIGHT;
    await fireEvent.scroll(grid);
    await vi.advanceTimersByTimeAsync(FRAME + 300 + FRAME);
    expect(asked).toHaveLength(3);
    await view.rerender({ options: hidden });
    await vi.advanceTimersByTimeAsync(20);
    expect(cancelled).toContain(asked[2].id);
    grid.scrollTop = 0;
    await fireEvent.scroll(grid);
    await vi.advanceTimersByTimeAsync(1_000);
    expect(asked).toHaveLength(3);
  } finally {
    restoreViewport();
  }
});

// Resizable columns (`InvisibleResizableHeader`, `VcsLogGraphTable.MyMouseAdapter`). Without a
// canvas the automatic widths are the initial ones: Author 150, Date 100. At 800 wide the main
// order puts Subject 0-550, Author 550-700, Date 700-800.
function renderColumns(options = defaultOptions(), extra: Record<string, unknown> = {}) {
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "log_rows") return [row];
    throw new Error(`unexpected command: ${command}`);
  });
  const onwidths = vi.fn();
  const onselect = vi.fn();
  const ongraphclick = vi.fn();
  render(LogView, { viewId: 1, count: 1, generation: 0, options, onselect, onwidths, ongraphclick, ...extra });
  return { grid: screen.getByRole("grid", { name: "Commits" }), onwidths, onselect, ongraphclick };
}

const history = () => ({ ...defaultOptions(), columns: ["author", "date", "subject"] as LogColumn[] });

test("hovering_a_border_shows_the_resize_cursor", async () => {
  const restoreViewport = withViewport(10 * ROW_HEIGHT, 800);
  try {
    const { grid } = renderColumns();
    await fireEvent.mouseMove(grid, { clientX: 552, clientY: 12 });
    expect(grid.style.cursor).toBe("col-resize");
    await fireEvent.mouseMove(grid, { clientX: 400, clientY: 12 });
    expect(grid.style.cursor).not.toBe("col-resize");
  } finally {
    restoreViewport();
  }
});

test("dragging_the_author_border_widens_author_and_reports_it_once_on_release", async () => {
  const restoreViewport = withViewport(10 * ROW_HEIGHT, 800);
  try {
    const { grid, onwidths } = renderColumns();
    await fireEvent.mouseDown(grid, { button: 0, clientX: 550, clientY: 12 });
    await fireEvent.mouseMove(window, { buttons: 1, clientX: 520, clientY: 12 });
    await fireEvent.mouseMove(window, { buttons: 1, clientX: 500, clientY: 12 });
    expect(onwidths).not.toHaveBeenCalled();
    expect(grid.style.cursor).toBe("col-resize");
    await fireEvent.mouseUp(window, { clientX: 500, clientY: 12 });

    expect(onwidths).toHaveBeenCalledTimes(1);
    expect(onwidths).toHaveBeenCalledWith({ author: 200 });
  } finally {
    restoreViewport();
  }
});

test("a_press_on_a_border_does_not_select_the_row", async () => {
  const restoreViewport = withViewport(10 * ROW_HEIGHT, 800);
  try {
    const { grid, onselect } = renderColumns();
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("log_rows", expect.anything()));
    await new Promise((resolve) => setTimeout(resolve, FRAME));
    onselect.mockClear();
    await fireEvent.mouseDown(grid, { button: 0, clientX: 550, clientY: 12 });
    await fireEvent.mouseUp(window, { clientX: 550, clientY: 12 });
    await fireEvent.click(grid, { clientX: 550, clientY: 12 });

    expect(onselect).not.toHaveBeenCalled();
  } finally {
    restoreViewport();
  }
});

test("a_double_click_on_a_border_resets_that_column", async () => {
  const restoreViewport = withViewport(10 * ROW_HEIGHT, 800);
  try {
    // Author 200 and Date 90: Subject 0-510, Author 510-710, Date 710-800.
    const { grid, onwidths } = renderColumns({ ...defaultOptions(), columnWidths: { author: 200, date: 90 } });
    await fireEvent.dblClick(grid, { clientX: 511, clientY: 12 });
    expect(onwidths).toHaveBeenCalledWith({ date: 90 });
  } finally {
    restoreViewport();
  }
});

test("a_double_click_away_from_a_border_still_selects_as_before", async () => {
  const restoreViewport = withViewport(10 * ROW_HEIGHT, 800);
  try {
    const { grid, onwidths, onselect } = renderColumns();
    await vi.waitFor(async () => {
      await fireEvent.click(grid, { clientX: 300, clientY: 12 });
      expect(onselect).toHaveBeenCalledWith([HASH]);
    });
    await fireEvent.dblClick(grid, { clientX: 300, clientY: 12 });
    expect(onwidths).not.toHaveBeenCalled();
  } finally {
    restoreViewport();
  }
});

test("saved_widths_are_drawn_and_move_the_graph_with_subject", async () => {
  const restoreViewport = withViewport(10 * ROW_HEIGHT, 800);
  try {
    // History order with Author 200: Subject starts at 300 instead of 250.
    const { grid, ongraphclick } = renderColumns({ ...history(), columnWidths: { author: 200 } });
    await vi.waitFor(async () => {
      await fireEvent.click(grid, { clientX: 308, clientY: 12 });
      expect(ongraphclick).toHaveBeenCalled();
    });
    ongraphclick.mockClear();
    await fireEvent.click(grid, { clientX: 258, clientY: 12 });
    expect(ongraphclick).not.toHaveBeenCalled();
  } finally {
    restoreViewport();
  }
});

test("a_saved_width_wider_than_the_table_draws_the_automatic_one", async () => {
  const restoreViewport = withViewport(10 * ROW_HEIGHT, 800);
  try {
    const { grid, ongraphclick } = renderColumns({ ...history(), columnWidths: { author: 900 } });
    await vi.waitFor(async () => {
      await fireEvent.click(grid, { clientX: 258, clientY: 12 });
      expect(ongraphclick).toHaveBeenCalled();
    });
  } finally {
    restoreViewport();
  }
});

test("without_onwidths_a_drag_still_resizes_until_the_view_goes", async () => {
  const restoreViewport = withViewport(10 * ROW_HEIGHT, 800);
  try {
    const { grid } = renderColumns(defaultOptions(), { onwidths: undefined });
    await fireEvent.mouseDown(grid, { button: 0, clientX: 550, clientY: 12 });
    await fireEvent.mouseMove(window, { buttons: 1, clientX: 500, clientY: 12 });
    await fireEvent.mouseUp(window, { clientX: 500, clientY: 12 });
    // The border is now at 500.
    await fireEvent.mouseMove(grid, { clientX: 501, clientY: 12 });
    expect(grid.style.cursor).toBe("col-resize");
  } finally {
    restoreViewport();
  }
});

test("the_click_that_ends_a_drag_away_from_the_border_does_not_select", async () => {
  const restoreViewport = withViewport(10 * ROW_HEIGHT, 800);
  try {
    const { grid, onselect, onwidths } = renderColumns();
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("log_rows", expect.anything()));
    await new Promise((resolve) => setTimeout(resolve, FRAME));
    onselect.mockClear();
    // Author stops at 15, so the border stops at 785 and the release at 795 is away from it.
    await fireEvent.mouseDown(grid, { button: 0, clientX: 550, clientY: 12 });
    await fireEvent.mouseMove(window, { buttons: 1, clientX: 795, clientY: 12 });
    await fireEvent.mouseUp(window, { clientX: 795, clientY: 12 });
    await fireEvent.click(grid, { clientX: 795, clientY: 12 });

    expect(onwidths).toHaveBeenCalledWith({ author: 15 });
    expect(onselect).not.toHaveBeenCalled();
  } finally {
    restoreViewport();
  }
});

test("a_move_without_the_button_held_ends_the_resize", async () => {
  const restoreViewport = withViewport(10 * ROW_HEIGHT, 800);
  try {
    const { grid, onwidths } = renderColumns();
    await fireEvent.mouseDown(grid, { button: 0, clientX: 550, clientY: 12 });
    await fireEvent.mouseMove(window, { buttons: 1, clientX: 500, clientY: 12 });
    await fireEvent.mouseMove(window, { buttons: 0, clientX: 450, clientY: 12 });
    await fireEvent.mouseMove(window, { buttons: 0, clientX: 400, clientY: 12 });

    expect(onwidths).toHaveBeenCalledTimes(1);
    expect(onwidths).toHaveBeenCalledWith({ author: 200 });
  } finally {
    restoreViewport();
  }
});

test("losing_the_window_focus_ends_the_resize", async () => {
  const restoreViewport = withViewport(10 * ROW_HEIGHT, 800);
  try {
    const { grid, onwidths } = renderColumns();
    await fireEvent.mouseDown(grid, { button: 0, clientX: 550, clientY: 12 });
    await fireEvent.mouseMove(window, { buttons: 1, clientX: 500, clientY: 12 });
    await fireEvent.blur(window);
    await fireEvent.mouseMove(window, { buttons: 1, clientX: 400, clientY: 12 });

    expect(onwidths).toHaveBeenCalledTimes(1);
    expect(onwidths).toHaveBeenCalledWith({ author: 200 });
  } finally {
    restoreViewport();
  }
});

test("a_press_on_a_border_focuses_the_table", async () => {
  const restoreViewport = withViewport(10 * ROW_HEIGHT, 800);
  try {
    const { grid } = renderColumns();
    await fireEvent.mouseDown(grid, { button: 0, clientX: 550, clientY: 12 });
    await fireEvent.mouseUp(window, { clientX: 550, clientY: 12 });

    expect(document.activeElement).toBe(grid);
  } finally {
    restoreViewport();
  }
});
