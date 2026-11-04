import { beforeEach, describe, expect, test, vi } from "vitest";
import { get } from "svelte/store";
import type { Conflict, ConflictSession, MergeDone } from "../lib/api";
import { dismiss, notifications, type Shown } from "../lib/notifications";
import {
  conflictsFlow,
  mergeFiles,
  type ConflictsApi,
  type ConflictsRequest,
  type ConflictsUi,
  type MergeApi,
} from "./flow";

const conflict = (path: string): Conflict => ({
  path,
  yours: "modified",
  theirs: "modified",
  kind: "both modified",
});

const session = (...paths: string[]): ConflictSession => ({
  conflicts: paths.map(conflict),
  reversed: false,
  yoursColumn: "Yours (main)",
  theirsColumn: "Theirs (other)",
  description: "Merging branch other into branch main",
});

// Each read returns the next session; the last one repeats.
function api(...sessions: ConflictSession[]): ConflictsApi & { session: ReturnType<typeof vi.fn> } {
  let next = 0;
  return {
    session: vi.fn(async () => sessions[Math.min(next++, sessions.length - 1)]),
  };
}

function ui(close: () => void = () => {}) {
  const asked: ConflictsRequest[] = [];
  return {
    asked,
    ask: vi.fn(async (request: ConflictsRequest) => {
      asked.push(request);
      close();
      return null;
    }),
  } as unknown as ConflictsUi & { asked: ConflictsRequest[]; ask: ReturnType<typeof vi.fn> };
}

const shown = () => get(notifications);
const last = (): Shown => shown()[shown().length - 1];
const labels = (notification: Shown) => notification.actions?.map(({ label }) => label);
const click = (notification: Shown, label: string) =>
  notification.actions?.find((action) => action.label === label)?.run();

beforeEach(() => {
  for (const { id } of shown()) dismiss(id);
});

describe("resolveConflicts", () => {
  test("resolving_everything_closes_by_itself_and_calls_onresolved", async () => {
    const dialogs = ui();
    const onresolved = vi.fn(async () => {});
    const flow = conflictsFlow(dialogs, api(session("a.txt"), session()));

    const result = await flow.resolveConflicts({ onresolved });

    expect(result).toBe("resolved");
    expect(onresolved).toHaveBeenCalledTimes(1);
    expect(dialogs.asked).toEqual([{ kind: "conflicts", session: session("a.txt") }]);
    expect(shown()).toEqual([]);
  });

  test("with_nothing_to_resolve_it_neither_opens_the_dialog_nor_calls_onresolved", async () => {
    const dialogs = ui();
    const onresolved = vi.fn(async () => {});
    const onnothing = vi.fn(async () => {});
    const flow = conflictsFlow(dialogs, api(session()));

    const result = await flow.resolveConflicts({ onresolved, onnothing });

    expect(result).toBe("resolved");
    expect(dialogs.ask).not.toHaveBeenCalled();
    expect(onresolved).not.toHaveBeenCalled();
    expect(onnothing).toHaveBeenCalledTimes(1);
  });

  test("closing_with_pending_notifies_and_resolve_reopens_with_another_title", async () => {
    const dialogs = ui();
    const onresolved = vi.fn(async () => {});
    const flow = conflictsFlow(dialogs, api(session("a.txt", "b.txt"), session("b.txt")));

    const result = await flow.resolveConflicts({ onresolved });

    expect(result).toBe("pending");
    expect(onresolved).not.toHaveBeenCalled();
    expect(last()).toMatchObject({
      kind: "warning",
      title: "There are pending unresolved conflicts.",
    });
    expect(labels(last())).toEqual(["Resolve…"]);

    click(last(), "Resolve…");
    await vi.waitFor(() => expect(dialogs.ask).toHaveBeenCalledTimes(2));
    await vi.waitFor(() => expect(last().title).toBe("Pending Unresolved conflicts"));

    expect(shown()).toHaveLength(1);
    expect(labels(last())).toEqual(["Resolve…"]);
    expect(onresolved).not.toHaveBeenCalled();
  });

  test("what_is_resolved_from_the_notification_does_not_continue_the_flow", async () => {
    const dialogs = ui();
    const onresolved = vi.fn(async () => {});
    const backend = api(session("a.txt"), session("a.txt"), session("a.txt"), session());
    const flow = conflictsFlow(dialogs, backend);

    await flow.resolveConflicts({ onresolved });
    click(last(), "Resolve…");
    await vi.waitFor(() => expect(backend.session).toHaveBeenCalledTimes(4));

    expect(dialogs.ask).toHaveBeenCalledTimes(2);
    expect(onresolved).not.toHaveBeenCalled();
    expect(shown()).toEqual([]);
  });

  test("with_the_flow_title_and_text_it_uses_them", async () => {
    const flow = conflictsFlow(ui(), api(session("a.txt"), session("a.txt")));

    await flow.resolveConflicts({ title: "Merge failed", additional: "\nContinue later." });

    expect(last()).toMatchObject({
      title: "Merge failed",
      body: "There are pending unresolved conflicts.\nContinue later.",
    });
  });

  test("onpending_replaces_the_notification", async () => {
    const onpending = vi.fn();
    const flow = conflictsFlow(ui(), api(session("a.txt"), session("a.txt")));

    const result = await flow.resolveConflicts({ onpending });

    expect(result).toBe("pending");
    expect(onpending).toHaveBeenCalledTimes(1);
    expect(shown()).toEqual([]);
  });

  test("a_failure_reading_the_conflicts_is_reported", async () => {
    const failing: ConflictsApi = {
      session: vi.fn().mockRejectedValue({ kind: "failed", message: "git is missing" }),
    };
    const dialogs = ui();

    const result = await conflictsFlow(dialogs, failing).resolveConflicts({ title: "Merge failed" });

    expect(result).toBe("pending");
    expect(dialogs.ask).not.toHaveBeenCalled();
    expect(last()).toMatchObject({ kind: "error", title: "Merge failed" });
    expect(last().body).toContain("Cannot check the working tree for unmerged files");
    expect(last().body).toContain("git is missing");
  });

  test("if_the_repository_changes_with_the_dialog_open_it_does_not_continue", async () => {
    let flow: ReturnType<typeof conflictsFlow>;
    const dialogs = ui(() => flow.repositoryChanged());
    const onresolved = vi.fn(async () => {});
    flow = conflictsFlow(dialogs, api(session("a.txt"), session()));

    const result = await flow.resolveConflicts({ onresolved });

    expect(result).toBe("pending");
    expect(onresolved).not.toHaveBeenCalled();
    expect(shown()).toEqual([]);
  });

  test("changing_repository_withdraws_the_notification", async () => {
    const flow = conflictsFlow(ui(), api(session("a.txt"), session("a.txt")));
    await flow.resolveConflicts({});
    expect(shown()).toHaveLength(1);

    flow.repositoryChanged();

    expect(shown()).toEqual([]);
  });

  test("while_a_dialog_is_open_another_request_waits_for_the_same_one", async () => {
    let close: () => void = () => {};
    const dialogs = {
      ask: vi.fn(() => new Promise<null>((resolve) => (close = () => resolve(null)))),
    } as unknown as ConflictsUi;
    const flow = conflictsFlow(dialogs, api(session("a.txt"), session()));

    const first = flow.resolveConflicts({});
    await vi.waitFor(() => expect(dialogs.ask).toHaveBeenCalled());
    const second = flow.resolveConflicts({});
    close();

    expect(await first).toBe("resolved");
    expect(await second).toBe("resolved");
    expect(dialogs.ask).toHaveBeenCalledTimes(1);
  });
});

test("while_onresolved_runs_a_new_request_opens_its_own_dialog", async () => {
  const dialogs = ui();
  const backend = api(session("a.txt"), session(), session("b.txt"), session());
  const flow = conflictsFlow(dialogs, backend);
  let second: Promise<unknown> = Promise.resolve();

  const first = await flow.resolveConflicts({
    onresolved: async () => {
      second = flow.resolveConflicts({});
      await second;
    },
  });

  expect(first).toBe("resolved");
  await second;
  expect(dialogs.ask).toHaveBeenCalledTimes(2);
});

describe("mergeFiles", () => {
  function windows() {
    let deliver: (done: MergeDone) => void = () => {};
    const opened: string[] = [];
    const backend: MergeApi = {
      open: vi.fn(async (path: string) => {
        opened.push(path);
      }),
      onDone: async (handler) => {
        deliver = handler;
        return () => {};
      },
    };
    return { backend, opened, done: (path: string, result: MergeDone["result"] = "resolved") => deliver({ path, result }) };
  }

  test("opens_the_next_one_when_the_previous_one_finishes", async () => {
    const merge = windows();
    const finished: string[] = [];

    const running = mergeFiles(["a.txt", "b.txt"], { ondone: (done) => void finished.push(done.path) }, merge.backend);
    await vi.waitFor(() => expect(merge.opened).toEqual(["a.txt"]));
    merge.done("a.txt");
    await vi.waitFor(() => expect(merge.opened).toEqual(["a.txt", "b.txt"]));
    merge.done("b.txt", "cancelled");
    await running;

    expect(finished).toEqual(["a.txt", "b.txt"]);
  });

  test("skips_what_is_no_longer_in_conflict", async () => {
    const merge = windows();

    const running = mergeFiles(["a.txt", "b.txt"], { skip: (path) => path === "a.txt" }, merge.backend);
    await vi.waitFor(() => expect(merge.opened).toEqual(["b.txt"]));
    merge.done("b.txt");
    await running;
  });

  test("if_the_window_was_for_another_file_it_does_not_go_on", async () => {
    const merge = windows();

    const running = mergeFiles(["a.txt", "b.txt"], {}, merge.backend);
    await vi.waitFor(() => expect(merge.opened).toEqual(["a.txt"]));
    merge.done("other.txt");
    await running;

    expect(merge.opened).toEqual(["a.txt"]);
  });

  test("a_failure_when_opening_propagates", async () => {
    const backend: MergeApi = {
      open: vi.fn().mockRejectedValue({ kind: "failed", message: "no window" }),
      onDone: async () => () => {},
    };

    await expect(mergeFiles(["a.txt"], {}, backend)).rejects.toMatchObject({ message: "no window" });
  });

  test("aborting_stops_waiting", async () => {
    const merge = windows();
    const controller = new AbortController();

    const running = mergeFiles(["a.txt", "b.txt"], { signal: controller.signal }, merge.backend);
    await vi.waitFor(() => expect(merge.opened).toEqual(["a.txt"]));
    controller.abort();
    await running;

    expect(merge.opened).toEqual(["a.txt"]);
  });
});
