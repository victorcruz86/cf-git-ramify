import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { afterAll, afterEach, beforeAll, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { LogRow, Signature, TaskEvent } from "../lib/api";
import { trackTasks } from "../lib/tasks";
import { ROW_HEIGHT } from "./draw";
import LogView from "./LogView.svelte";
import { defaultOptions, toggleColumn } from "./options";

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
