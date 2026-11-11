import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { TaskEvent } from "./api";
import { isOutdated } from "./errors";
import { runLocal, taskResult } from "./tasks";
import TaskStatus from "./TaskStatus.svelte";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({ label: "main" }),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
  emitTo: vi.fn(async () => {}),
}));

let send: (event: TaskEvent) => void;

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockResolvedValue(undefined);
  vi.mocked(listen).mockImplementation(async (name, handler) => {
    if (name === "task-changed") {
      send = (event) => (handler as (event: { payload: TaskEvent }) => void)({ payload: event });
    }
    return () => {};
  });
});

afterEach(cleanup);

const FETCH: TaskEvent = {
  id: 7,
  title: "Fetching origin",
  text: "Receiving objects: 45%",
  fraction: 0.45,
  state: "running",
  kind: null,
  value: null,
};

async function shown() {
  render(TaskStatus);
  await vi.waitFor(() => expect(send).toBeDefined());
}

test("running_task_shows_title_and_cancel", async () => {
  await shown();

  send(FETCH);

  expect(await screen.findByText("Fetching origin")).toBeTruthy();
  expect(screen.getByText("Receiving objects: 45%")).toBeTruthy();
  expect(screen.getByRole("progressbar").getAttribute("aria-valuenow")).toBe("45");
  await fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(vi.mocked(invoke)).toHaveBeenCalledWith("task_cancel", { id: 7 });
});

test("without_tasks_there_is_no_bar", async () => {
  await shown();

  expect(screen.queryByRole("button", { name: "Cancel" })).toBeNull();
  expect(screen.queryByRole("progressbar")).toBeNull();
});

test("the_bar_does_not_show_without_a_fraction", async () => {
  await shown();

  send({ ...FETCH, text: "", fraction: null });

  await screen.findByText("Fetching origin");
  expect(screen.queryByRole("progressbar")).toBeNull();
});

test("a_task_that_finishes_leaves_the_bar", async () => {
  await shown();
  send(FETCH);
  await screen.findByText("Fetching origin");

  for (const state of ["done", "failed", "cancelled"] as const) {
    send({ ...FETCH, id: 8, title: "Another", state: "running" });
    await screen.findByText("Another");
    send({ ...FETCH, id: 8, title: "Another", state });
    await vi.waitFor(() => expect(screen.queryByText("Another")).toBeNull());
  }

  expect(screen.getByText("Fetching origin")).toBeTruthy();
});

test("two_tasks_are_seen_at_once_and_cancelled_separately", async () => {
  await shown();
  send(FETCH);
  send({ ...FETCH, id: 9, title: "Pushing" });
  await screen.findByText("Pushing");

  await fireEvent.click(screen.getAllByRole("button", { name: "Cancel" })[1]);

  expect(vi.mocked(invoke)).toHaveBeenCalledWith("task_cancel", { id: 9 });
});

const finished = (id: number, patch: Partial<TaskEvent>): TaskEvent => ({
  ...FETCH,
  id,
  fraction: null,
  state: "done",
  ...patch,
});

test("the_result_of_a_task_reaches_whoever_waits_for_it", async () => {
  await shown();
  const result = taskResult<{ files: number }>(21);

  send(finished(21, { value: { files: 3 } }));

  await expect(result).resolves.toEqual({ files: 3 });
});

test("a_result_that_arrived_before_being_requested_is_not_lost", async () => {
  await shown();

  send(finished(22, { value: "ready" }));

  await expect(taskResult(22)).resolves.toBe("ready");
  await expect(taskResult(22)).resolves.toBe("ready");
});

test("a_failed_task_rejects_with_the_shape_of_ApiError", async () => {
  await shown();
  const result = taskResult(23);

  send(finished(23, { state: "failed", kind: "git_not_found", text: "git was not found" }));

  await expect(result).rejects.toEqual({ kind: "git_not_found", message: "git was not found" });
});

test("a_cancelled_task_rejects_with_kind_cancelled", async () => {
  await shown();
  const result = taskResult(24);

  send(finished(24, { state: "cancelled", kind: "cancelled" }));

  await expect(result).rejects.toMatchObject({ kind: "cancelled" });
});

test("progress_updates_do_not_resolve_the_wait", async () => {
  await shown();
  let settled = false;
  taskResult(25).finally(() => (settled = true));

  send({ ...FETCH, id: 25 });
  await Promise.resolve();

  expect(settled).toBe(false);
  send(finished(25, { value: 1 }));
  await vi.waitFor(() => expect(settled).toBe(true));
});

test("a_resolution_with_a_changed_file_arrives_with_kind_outdated", async () => {
  await shown();
  const result = taskResult(26);

  send(finished(26, { state: "failed", kind: "outdated", text: "The file has changed" }));

  const failure = await result.catch((error: unknown) => error);
  expect(failure).toMatchObject({ kind: "outdated" });
  expect(isOutdated(failure)).toBe(true);
  expect(isOutdated({ kind: "failed", message: "The file has changed" })).toBe(false);
  expect(isOutdated(new Error("outdated"))).toBe(false);
});

test("work of this window shows while it runs, without Cancel", async () => {
  render(TaskStatus);
  let finish: () => void = () => {};
  const work = runLocal(
    "Commit Changes",
    "Committing…",
    () => new Promise<string>((done) => (finish = () => done("abc"))),
  );

  expect(await screen.findByText("Commit Changes")).toBeTruthy();
  expect(screen.getByText("Committing…")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Cancel" })).toBeNull();

  finish();

  expect(await work).toBe("abc");
  await vi.waitFor(() => expect(screen.queryByText("Commit Changes")).toBeNull());
});
