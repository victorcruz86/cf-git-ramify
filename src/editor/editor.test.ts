import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { EditorRequest } from "../lib/api";
import MessageEditorDialog from "./MessageEditorDialog.svelte";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({ label: "main" }),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
  emitTo: vi.fn(async () => {}),
}));

const handlers = new Map<string, (event: { payload: unknown }) => void>();
let nextId = 1;

beforeEach(() => {
  handlers.clear();
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockResolvedValue(undefined);
  vi.mocked(listen).mockImplementation(async (name, handler) => {
    handlers.set(name, handler as (event: { payload: unknown }) => void);
    return () => {};
  });
});

afterEach(cleanup);

async function open() {
  render(MessageEditorDialog, { root: "/Users/me/project" });
  await waitFor(() => expect(handlers.has("editor-request")).toBe(true));
}

async function request(
  text: string,
  kind: EditorRequest["kind"] = "message",
  form: EditorRequest["form"] = "text",
): Promise<number> {
  const id = nextId++;
  handlers.get("editor-request")!({ payload: { id, kind, form, text } satisfies EditorRequest });
  await Promise.resolve();
  return id;
}

const answers = () =>
  vi
    .mocked(invoke)
    .mock.calls.filter(([command]) => command === "editor_answer")
    .map(([, args]) => args);

const field = () => screen.getByRole("textbox") as HTMLTextAreaElement;
const button = (name: string) => screen.getByRole("button", { name });

test("a_message_request_opens_the_dialog_with_the_text", async () => {
  await open();
  expect(screen.queryByRole("dialog")).toBeNull();

  await request("Fix the login\n\nFix details\n");

  await screen.findByRole("dialog", { name: "Git Commit Message" });
  expect(screen.getByText("Git Root: /Users/me/project")).toBeTruthy();
  expect(field().value).toBe("Fix the login\n\nFix details\n");
  expect(document.activeElement).toBe(field());
  expect(button("Continue Rebasing")).toBeTruthy();
  expect(button("Cancel")).toBeTruthy();
  expect(answers()).toEqual([]);
});

test("continue_rebasing_answers_the_edited_text", async () => {
  await open();
  const id = await request("Old message\n");
  await screen.findByRole("dialog", { name: "Git Commit Message" });

  await fireEvent.input(field(), { target: { value: "New message\n\nwith body\n" } });
  await fireEvent.click(button("Continue Rebasing"));

  expect(answers()).toEqual([{ id, text: "New message\n\nwith body\n" }]);
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("cancel_answers_null", async () => {
  await open();
  const id = await request("Message\n");
  await screen.findByRole("dialog", { name: "Git Commit Message" });

  await fireEvent.click(button("Cancel"));

  expect(answers()).toEqual([{ id, text: null }]);
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("escape_answers_null", async () => {
  await open();
  const id = await request("Message\n");
  const shown = await screen.findByRole("dialog", { name: "Git Commit Message" });

  await fireEvent.keyDown(shown, { key: "Escape" });

  expect(answers()).toEqual([{ id, text: null }]);
});

test("requests_are_handled_one_at_a_time_without_carrying_the_text_over", async () => {
  await open();
  const first = await request("First\n");
  const second = await request("Second\n");
  await screen.findByRole("dialog", { name: "Git Commit Message" });
  await fireEvent.input(field(), { target: { value: "First edited\n" } });

  await fireEvent.click(button("Continue Rebasing"));

  await waitFor(() => expect(field().value).toBe("Second\n"));
  await fireEvent.click(button("Cancel"));
  expect(answers()).toEqual([
    { id: first, text: "First edited\n" },
    { id: second, text: null },
  ]);
});

test("the_list_of_a_rebase_without_its_own_window_is_edited_as_text", async () => {
  await open();
  const id = await request("pick abc1234 one\nreword def5678 two\n", "todo");
  await screen.findByRole("dialog", { name: "Git Commit Message" });

  expect(field().value).toBe("pick abc1234 one\nreword def5678 two\n");
  await fireEvent.click(button("Continue Rebasing"));

  expect(answers()).toEqual([{ id, text: "pick abc1234 one\nreword def5678 two\n" }]);
});

test("a_closed_request_closes_the_dialog", async () => {
  await open();
  const first = await request("First\n");
  const second = await request("Second\n");
  const third = await request("Third\n");
  await screen.findByRole("dialog", { name: "Git Commit Message" });
  await fireEvent.input(field(), { target: { value: "First half written\n" } });

  // One that is not yet visible is withdrawn: the dialog does not change.
  handlers.get("editor-dismissed")!({ payload: second });
  await Promise.resolve();
  expect(field().value).toBe("First half written\n");

  // The visible one is withdrawn: it moves to the next with its text, without answering anyone.
  handlers.get("editor-dismissed")!({ payload: first });
  await waitFor(() => expect(field().value).toBe("Third\n"));
  expect(answers()).toEqual([]);

  handlers.get("editor-dismissed")!({ payload: third });
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(answers()).toEqual([]);
});

// Without the log, the list that git generates is shown in `Rebasing Commits` (`handleInteractiveEditor`), and
// what was chosen comes back as JSON.
test("the_git_list_is_shown_in_rebasing_commits", async () => {
  await open();
  const commits = [
    { action: "pick", hash: "abc1234", subject: "# one", full: "a".repeat(40), message: "one" },
    { action: "pick", hash: "def5678", subject: "# two", full: "d".repeat(40), message: "two" },
  ];
  const id = await request(JSON.stringify(commits), "todo", "commits");
  const shown = await screen.findByRole("dialog", { name: "Rebasing Commits" });
  expect(screen.queryByRole("dialog", { name: "Git Commit Message" })).toBeNull();

  await fireEvent.click(within(shown).getByRole("button", { name: "Start Rebasing" }));

  expect(answers()).toEqual([
    {
      id,
      text: JSON.stringify({
        entries: [
          { action: "pick", hash: "abc1234", subject: "# one" },
          { action: "pick", hash: "def5678", subject: "# two" },
        ],
        messages: [],
      }),
    },
  ]);
  expect(screen.queryByRole("dialog")).toBeNull();

  const other = await request(JSON.stringify(commits), "todo", "commits");
  await screen.findByRole("dialog", { name: "Rebasing Commits" });
  await fireEvent.click(button("Cancel"));
  expect(answers()[1]).toEqual({ id: other, text: null });
});

test("noop_asks_whether_to_continue", async () => {
  await open();
  const id = await request("", "todo", "noop");
  const shown = await screen.findByRole("dialog", { name: "Git Rebase" });
  expect(
    within(shown).getByText(/There are no commits to rebase because the current branch is directly below the base branch/),
  ).toBeTruthy();

  await fireEvent.click(within(shown).getByRole("button", { name: "OK" }));
  expect(answers()).toEqual([{ id, text: "" }]);

  const other = await request("", "todo", "noop");
  await screen.findByRole("dialog", { name: "Git Rebase" });
  await fireEvent.click(button("Cancel"));
  expect(answers()[1]).toEqual({ id: other, text: null });
});
