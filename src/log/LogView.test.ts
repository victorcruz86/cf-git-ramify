import { cleanup, render } from "@testing-library/svelte";
import { afterAll, afterEach, beforeAll, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { LogRow } from "../lib/api";
import LogView from "./LogView.svelte";
import { defaultOptions } from "./options";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

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

afterEach(cleanup);

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
