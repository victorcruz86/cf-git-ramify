import { beforeEach, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { Annotation } from "../lib/api";
import { taskResult } from "../lib/tasks";
import type { ViewerTab } from "./model";
import { TabAnnotations } from "./annotations.svelte";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(), emitTo: vi.fn() }));
// The task's result arrives at once.
vi.mock("../lib/tasks", () => ({ taskResult: vi.fn() }));

const HASH = "1234567890abcdef1234567890abcdef12345678";
const TEXT = { kind: "text", text: "a\n", previewOf: null, previewSize: null } as const;

const annotation: Annotation = {
  revision: HASH,
  path: "a.txt",
  commits: [],
  lines: [null],
  origins: [0],
  owners: [],
  suspicious: false,
  historySize: 0,
  line: null,
};

function tab(content: ViewerTab["content"], path = "a.txt"): ViewerTab {
  return { key: "k", path, revision: HASH, content, failed: false, stamp: null };
}

// A controller over one tab whose content the test changes.
function over(start: ViewerTab) {
  let current = start;
  const annotations = new TabAnnotations({ tab: () => current, view: () => undefined });
  return { annotations, set: (next: ViewerTab) => (current = next) };
}

const starts = () => vi.mocked(invoke).mock.calls.filter(([name]) => name === "annotate_start");

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockResolvedValue(1);
  vi.mocked(taskResult).mockReset();
  vi.mocked(taskResult).mockResolvedValue(annotation);
});

// A target that asks for annotations but is not a text keeps Annotate off: the Git menu does not
// show it checked.
test("a_binary_target_asked_to_annotate_stays_off", () => {
  const { annotations } = over(tab({ kind: "binary", size: 24, tooLarge: false }, "logo.png"));

  annotations.want("k");
  annotations.textArrived("k");

  expect(annotations.isOn("k")).toBe(false);
  expect(invoke).not.toHaveBeenCalled();
});

test("a_text_target_asked_to_annotate_starts_once_its_text_is_read", async () => {
  const { annotations } = over(tab(TEXT));

  annotations.want("k");
  annotations.textArrived("k");

  expect(annotations.isOn("k")).toBe(true);
  await vi.waitFor(() => expect(annotations.shown.k).toBe(annotation));
  expect(starts()).toEqual([
    ["annotate_start", { request: { path: "a.txt", revision: HASH, localText: null, from: null } }],
  ]);
  // The want is served: another arrival of the same revision's text does not annotate again.
  annotations.textArrived("k");
  await new Promise((done) => setTimeout(done, 10));
  expect(starts()).toHaveLength(1);
  expect(annotations.shown.k).toBe(annotation);
});

// `AnnotateRevisionActionBase.annotate` (the history menu's Annotate, `annotate: true` on an open
// tab): an annotated revision tab is annotated again, not closed.
test("annotate_on_an_annotated_revision_annotates_again", async () => {
  const { annotations } = over(tab(TEXT));
  annotations.want("k");
  annotations.textArrived("k");
  await vi.waitFor(() => expect(annotations.shown.k).toBe(annotation));

  annotations.want("k", { text: "x\na\n", line: 2 });
  annotations.textArrived("k");

  expect(annotations.isOn("k")).toBe(true);
  await vi.waitFor(() => expect(starts()).toHaveLength(2));
  expect(starts()[1][1]).toMatchObject({ request: { from: { text: "x\na\n", line: 2 } } });
  await vi.waitFor(() => expect(annotations.shown.k).toBe(annotation));
});

// Annotate asked while the tab still reads its text: kept until the text arrives.
test("annotate_asked_while_the_text_loads_is_kept", async () => {
  const loading = over(tab(null));

  loading.annotations.want("k", { text: "b\n", line: 1 });
  loading.annotations.textArrived("k");
  expect(starts()).toHaveLength(0);

  loading.set(tab(TEXT));
  loading.annotations.textArrived("k");

  await vi.waitFor(() => expect(starts()).toHaveLength(1));
  expect(starts()[0][1]).toMatchObject({ request: { from: { text: "b\n", line: 1 } } });
  await vi.waitFor(() => expect(loading.annotations.shown.k).toBe(annotation));
});
