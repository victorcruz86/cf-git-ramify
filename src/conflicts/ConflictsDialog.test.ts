import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Conflict, ConflictKind, ConflictSession, SideStatus } from "../lib/api";
import { trackTasks } from "../lib/tasks";
import ConflictsDialog from "./ConflictsDialog.svelte";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(), emit: vi.fn() }));

const conflict = (
  path: string,
  yours: SideStatus = "modified",
  theirs: SideStatus = "modified",
  kind: ConflictKind = "both modified",
): Conflict => ({ path, yours, theirs, kind });

const session = (...conflicts: Conflict[]): ConflictSession => ({
  conflicts,
  reversed: false,
  yoursColumn: "Yours (main)",
  theirsColumn: "Theirs (other)",
  description: "Merging branch other into branch main",
});

const handlers = new Map<string, (payload: unknown) => void>();
const deliver = (event: string, payload?: unknown) => handlers.get(event)?.(payload);

// Commands that launch a task return its id, and the result arrives later.
let nextTask = 1;

function task(value: unknown = null, failure?: { kind: string; text: string }) {
  return () => {
    const id = nextTask++;
    setTimeout(() =>
      deliver("task-changed", {
        id,
        title: "",
        text: failure?.text ?? "",
        fraction: null,
        state: failure ? "failed" : "done",
        kind: failure?.kind ?? null,
        value,
      }),
    );
    return id;
  };
}

// The session that `conflicts_session` reads in turn: the last one repeats.
function mockCommands(
  sessions: ConflictSession[],
  commands: Record<string, (args: never) => unknown> = {},
) {
  let read = 0;
  const all: Record<string, (args: never) => unknown> = {
    conflicts_session: () => task(sessions[Math.min(read++, sessions.length - 1)])(),
    conflicts_accept: task(),
    merge_window_open: () => undefined,
    ...commands,
  };
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    const handler = all[command];
    if (!handler) throw new Error(`unexpected command: ${command}`);
    return handler(args as never);
  });
}

const calls = (command: string) =>
  vi.mocked(invoke).mock.calls.filter(([name]) => name === command);

beforeEach(async () => {
  handlers.clear();
  localStorage.clear();
  vi.mocked(invoke).mockReset();
  vi.mocked(listen).mockReset();
  vi.mocked(listen).mockImplementation(async (event, handler) => {
    handlers.set(event as string, (payload) =>
      handler({ event: event as string, id: 0, payload } as never),
    );
    return () => {};
  });
  await trackTasks();
});

afterEach(cleanup);

function open(initial: ConflictSession, onanswer = vi.fn()) {
  render(ConflictsDialog, { session: initial, onanswer });
  return onanswer;
}

const row = (name: string | RegExp) => screen.getByRole("row", { name });
const button = (name: string | RegExp) => screen.getByRole("button", { name });
const select = (name: string | RegExp) => fireEvent.click(row(name));

test("conflicts_dialog_columns_name_yours_theirs_with_branches", () => {
  mockCommands([]);
  open(session(conflict("a.txt")));

  expect(screen.getByRole("dialog", { name: "Conflicts" })).toBeTruthy();
  expect(screen.getByText("Merging branch other into branch main")).toBeTruthy();
  const headers = screen.getAllByRole("columnheader").map((header) => header.textContent);
  expect(headers).toEqual(["Name", "Yours (main)", "Theirs (other)"]);
  expect(button("Accept Yours")).toBeTruthy();
  expect(button("Accept Theirs")).toBeTruthy();
  expect(button("Merge…")).toBeTruthy();
  expect(button("Close")).toBeTruthy();
});

test("modified_and_deleted_states", () => {
  mockCommands([]);
  open(
    session(
      conflict("a.txt"),
      conflict("b.txt", "deleted", "modified", "deleted by you"),
      conflict("c.txt", "modified", "deleted", "deleted by them"),
    ),
  );

  const cells = (name: string) =>
    within(row(new RegExp(`^${name}`)))
      .getAllByRole("gridcell")
      .map((cell) => cell.textContent?.trim());
  expect(cells("a.txt")).toEqual(["a.txt", "Modified", "Modified"]);
  expect(cells("b.txt")).toEqual(["b.txt", "Deleted", "Modified"]);
  expect(cells("c.txt")).toEqual(["c.txt", "Modified", "Deleted"]);
});

test("the_first_file_starts_chosen_and_the_name_carries_its_folder", () => {
  mockCommands([]);
  open(session(conflict("src/a.txt"), conflict("b.txt")));

  expect(row(/^a.txt/).getAttribute("aria-selected")).toBe("true");
  expect(row(/^a.txt/).textContent).toContain("src");
  expect(row(/^b.txt/).getAttribute("aria-selected")).toBe("false");
});

test("merge_is_the_default_button_and_double_click_launches_it", async () => {
  mockCommands([]);
  open(session(conflict("a.txt"), conflict("b.txt")));

  const merge = button("Merge…");
  expect(merge.classList.contains("default")).toBe(true);
  expect((merge as HTMLButtonElement).type).toBe("submit");
  expect(button("Accept Yours").classList.contains("default")).toBe(false);

  await fireEvent.dblClick(row(/^b.txt/));

  await waitFor(() => expect(calls("merge_window_open")).toHaveLength(1));
  expect(calls("merge_window_open")[0][1]).toEqual({ path: "b.txt" });
});

test("enter_launches_merge", async () => {
  mockCommands([]);
  open(session(conflict("a.txt")));

  await fireEvent.submit(screen.getByRole("dialog").querySelector("form") as HTMLFormElement);

  await waitFor(() => expect(calls("merge_window_open")).toHaveLength(1));
});

test("enter_on_the_table_launches_merge_like_the_default_button", async () => {
  mockCommands([]);
  open(session(conflict("a.txt"), conflict("b.txt")));
  await select(/^b.txt/);

  await fireEvent.keyDown(screen.getByRole("treegrid"), { key: "Enter" });

  await waitFor(() => expect(calls("merge_window_open")).toHaveLength(1));
  expect(calls("merge_window_open")[0][1]).toEqual({ path: "b.txt" });
});

test("arrows_move_the_selection_and_shift_extends_it", async () => {
  mockCommands([]);
  open(session(conflict("a.txt"), conflict("b.txt"), conflict("c.txt")));
  const table = screen.getByRole("treegrid");

  await fireEvent.keyDown(table, { key: "ArrowDown" });
  expect(row(/^b.txt/).getAttribute("aria-selected")).toBe("true");
  expect(row(/^a.txt/).getAttribute("aria-selected")).toBe("false");

  await fireEvent.keyDown(table, { key: "ArrowDown", shiftKey: true });
  expect(row(/^b.txt/).getAttribute("aria-selected")).toBe("true");
  expect(row(/^c.txt/).getAttribute("aria-selected")).toBe("true");
});

test("merge_disabled_if_both_deleted", async () => {
  mockCommands([]);
  open(
    session(
      conflict("a.txt"),
      conflict("b.txt", "deleted", "deleted", "both deleted"),
    ),
  );

  expect((button("Merge…") as HTMLButtonElement).disabled).toBe(false);
  await select(/^b.txt/);

  expect((button("Merge…") as HTMLButtonElement).disabled).toBe(true);
  expect((button("Accept Yours") as HTMLButtonElement).disabled).toBe(false);
  await fireEvent.dblClick(row(/^b.txt/));
  expect(calls("merge_window_open")).toHaveLength(0);
});

test("without_selection_the_three_buttons_are_off", async () => {
  mockCommands([]);
  open(session(conflict("a.txt")));
  await fireEvent.click(row(/^a.txt/), { metaKey: true });

  for (const name of ["Accept Yours", "Accept Theirs", "Merge…"]) {
    expect((button(name) as HTMLButtonElement).disabled).toBe(true);
  }
});

test("group_files_by_directory_is_remembered", async () => {
  mockCommands([]);
  const initial = session(conflict("src/a.txt"), conflict("b.txt"));
  const first = render(ConflictsDialog, { session: initial, onanswer: vi.fn() });
  const group = screen.getByRole("checkbox", { name: "Group files by directory" }) as HTMLInputElement;
  expect(group.checked).toBe(false);
  expect(screen.queryByRole("row", { name: /^src/ })).toBeNull();

  await fireEvent.click(group);

  expect(screen.getByRole("row", { name: /^src/ })).toBeTruthy();
  expect(row(/^a.txt/).getAttribute("aria-level")).toBe("2");
  first.unmount();

  render(ConflictsDialog, { session: initial, onanswer: vi.fn() });
  const again = screen.getByRole("checkbox", { name: "Group files by directory" }) as HTMLInputElement;
  expect(again.checked).toBe(true);
  expect(screen.getByRole("row", { name: /^src/ })).toBeTruthy();
});

test("accept_yours_resolves_and_removes_the_row", async () => {
  mockCommands([session(conflict("b.txt"))]);
  open(session(conflict("a.txt"), conflict("b.txt")));
  await select(/^a.txt/);

  await fireEvent.click(button("Accept Yours"));

  await waitFor(() => expect(screen.queryByRole("row", { name: /^a.txt/ })).toBeNull());
  expect(calls("conflicts_accept")[0][1]).toEqual({ paths: ["a.txt"], side: "yours" });
  expect(row(/^b.txt/).getAttribute("aria-selected")).toBe("true");
});

test("accept_theirs_of_several_chosen_files", async () => {
  mockCommands([session(conflict("c.txt"))]);
  open(session(conflict("a.txt"), conflict("b.txt"), conflict("c.txt")));
  await select(/^a.txt/);
  await fireEvent.click(row(/^b.txt/), { shiftKey: true });

  await fireEvent.click(button("Accept Theirs"));

  await waitFor(() => expect(calls("conflicts_accept")).toHaveLength(1));
  expect(calls("conflicts_accept")[0][1]).toEqual({ paths: ["a.txt", "b.txt"], side: "theirs" });
});

test("resolving_everything_closes_by_itself", async () => {
  mockCommands([session()]);
  const onanswer = open(session(conflict("a.txt")));

  await fireEvent.click(button("Accept Theirs"));

  await waitFor(() => expect(onanswer).toHaveBeenCalledTimes(1));
});

test("in_rebase_the_columns_are_inverted", async () => {
  // The backend already gives the session from the user's side: `yours` is the branch being rebased, and the
  // dialog inverts nothing when accepting (`conflicts_accept` is the one that does).
  const rebasing: ConflictSession = {
    ...session(conflict("a.txt", "deleted", "modified", "deleted by you")),
    reversed: true,
    yoursColumn: "Yours (feature)",
    theirsColumn: "Theirs (main)",
    description: "Rebasing branch feature onto branch main",
  };
  mockCommands([rebasing]);
  open(rebasing);

  expect(screen.getAllByRole("columnheader").map((header) => header.textContent)).toEqual([
    "Name",
    "Yours (feature)",
    "Theirs (main)",
  ]);
  expect(screen.getByText("Rebasing branch feature onto branch main")).toBeTruthy();

  await fireEvent.click(button("Accept Yours"));

  await waitFor(() => expect(calls("conflicts_accept")).toHaveLength(1));
  expect(calls("conflicts_accept")[0][1]).toEqual({ paths: ["a.txt"], side: "yours" });
});

test("merge_done_rereads_the_session", async () => {
  mockCommands([session(conflict("b.txt"))]);
  open(session(conflict("a.txt"), conflict("b.txt")));
  await fireEvent.click(button("Merge…"));
  await waitFor(() => expect(calls("merge_window_open")).toHaveLength(1));
  expect((button("Accept Yours") as HTMLButtonElement).disabled).toBe(true);

  deliver("merge-done", { path: "a.txt", result: "resolved" });

  await waitFor(() => expect(screen.queryByRole("row", { name: /^a.txt/ })).toBeNull());
  expect(calls("conflicts_session")).toHaveLength(1);
  await waitFor(() => expect((button("Accept Yours") as HTMLButtonElement).disabled).toBe(false));
});

test("a_repo_changed_rereads_the_session", async () => {
  mockCommands([session(conflict("b.txt"))]);
  open(session(conflict("a.txt"), conflict("b.txt")));
  await waitFor(() => expect(handlers.has("repo-changed")).toBe(true));

  deliver("repo-changed");

  await waitFor(() => expect(screen.queryByRole("row", { name: /^a.txt/ })).toBeNull());
});

test("if_nothing_is_left_when_rereading_it_closes", async () => {
  mockCommands([session()]);
  const onanswer = open(session(conflict("a.txt")));
  await waitFor(() => expect(handlers.has("repo-changed")).toBe(true));

  deliver("repo-changed");

  await waitFor(() => expect(onanswer).toHaveBeenCalledTimes(1));
});

test("a_failure_when_accepting_is_reported_and_leaves_the_rows", async () => {
  mockCommands([session(conflict("a.txt"))], {
    conflicts_accept: task(null, { kind: "failed", text: "disk full" }),
  });
  const onanswer = open(session(conflict("a.txt")));

  await fireEvent.click(button("Accept Yours"));

  const alert = await screen.findByRole("alert");
  expect(alert.textContent).toBe("Error saving merged data: disk full");
  expect(row(/^a.txt/)).toBeTruthy();
  expect(onanswer).not.toHaveBeenCalled();
});

test("a_failure_when_opening_the_window_is_reported", async () => {
  mockCommands([session(conflict("a.txt"))], {
    merge_window_open: () => {
      throw { kind: "failed", message: "no window" };
    },
  });
  open(session(conflict("a.txt")));

  await fireEvent.click(button("Merge…"));

  const alert = await screen.findByRole("alert");
  expect(alert.textContent).toBe("Error loading revisions to merge: no window");
  expect((button("Merge…") as HTMLButtonElement).disabled).toBe(false);
});

test("close_and_escape_close", async () => {
  mockCommands([]);
  const onanswer = open(session(conflict("a.txt")));

  await fireEvent.click(button("Close"));
  expect(onanswer).toHaveBeenCalledTimes(1);

  await fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
  expect(onanswer).toHaveBeenCalledTimes(2);
});

test("the_repository_text_is_shown_as_text", () => {
  mockCommands([]);
  const hostile = '<img src=x onerror="window.pwned=1">.txt';
  open({ ...session(conflict(hostile)), description: "<b>Merging</b>" });

  expect(screen.getByText("<b>Merging</b>")).toBeTruthy();
  expect(document.querySelector("img")).toBeNull();
  expect(document.querySelector("b")).toBeNull();
});
