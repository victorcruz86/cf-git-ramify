import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import type { RebaseCommit } from "../lib/api";
import InteractiveRebaseDialog from "./InteractiveRebaseDialog.svelte";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn() }));

const commit = (name: string): RebaseCommit => ({
  action: "pick",
  hash: name.toLowerCase().repeat(7),
  subject: name,
  full: name.toLowerCase().repeat(40),
  message: `${name}\n\nbody of ${name}`,
});

const COMMITS = ["A", "B", "C"].map(commit);

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "commit_changes_of") {
      return [
        {
          parent: null,
          subject: null,
          base: null,
          merge: false,
          size: 1,
          files: [{ path: "src/b.txt", status: "M", oldPath: null }],
        },
      ];
    }
    if (command === "commit_details") {
      const { hashes } = args as { hashes: string[] };
      return hashes.map((hash) => ({
        hash,
        subject: `detail of ${hash.slice(0, 1)}`,
        body: "",
        author: "Ana",
        authorEmail: "",
        authorTime: 0,
        committer: "Ana",
        committerEmail: "",
        commitTime: 0,
        samePerson: true,
        refs: [],
        links: {},
        parents: [],
      }));
    }
    return null;
  });
});

afterEach(cleanup);

function open() {
  const onanswer = vi.fn();
  render(InteractiveRebaseDialog, { commits: COMMITS, onanswer });
  return { onanswer };
}

const table = () => screen.getByRole("grid", { name: "Commits" });
const rows = () => within(table()).getAllByRole("row");
const start = () => screen.getByRole("button", { name: "Start Rebasing" }) as HTMLButtonElement;
const button = (name: string) => screen.getByRole("button", { name }) as HTMLButtonElement;

test("table_with_the_oldest_on_top_and_title_rebasing_commits", () => {
  open();
  expect(screen.getByRole("dialog", { name: "Rebasing Commits" })).toBeTruthy();
  expect(rows().map((row) => row.textContent?.trim())).toEqual(["A", "B", "C"]);
  expect(start().className).toContain("default");
  // Up, Down, Pick and Stop to Edit, and then Reword, Squash (with Fixup) and Drop.
  for (const name of ["Up", "Down", "Pick", "Stop to Edit", "Reword", "Squash", "Drop"]) {
    expect(button(name).disabled).toBe(true);
  }
});

test("start_rebasing_disabled_while_a_message_is_edited", async () => {
  open();
  await fireEvent.click(rows()[1]);
  await fireEvent.dblClick(rows()[1]);

  const editor = screen.getByRole("textbox", { name: "Commit message" }) as HTMLTextAreaElement;
  expect(editor.value).toBe("B\n\nbody of B");
  expect(start().disabled).toBe(true);
  // The row's actions neither, in the meantime.
  expect(button("Drop").disabled).toBe(true);

  // Escape leaves the edit without changing anything and without closing the dialog.
  await fireEvent.keyDown(editor, { key: "Escape" });
  expect(screen.queryByRole("textbox", { name: "Commit message" })).toBeNull();
  expect(screen.getByRole("dialog", { name: "Rebasing Commits" })).toBeTruthy();
  expect(start().disabled).toBe(false);
});

const editorField = () => screen.getByRole("textbox", { name: "Commit message" }) as HTMLTextAreaElement;
const confirmation = () => screen.getByRole("dialog", { name: "Cancel Reword" });

async function typeIntoReword() {
  await fireEvent.click(rows()[1]);
  await fireEvent.click(button("Reword"));
  await fireEvent.input(editorField(), { target: { value: "typed" } });
}

test("escape_with_a_changed_reword_asks_cancel_reword", async () => {
  open();
  await typeIntoReword();
  await fireEvent.keyDown(editorField(), { key: "Escape" });

  const asking = confirmation();
  expect(within(asking).getByText("Entered commit message will be lost")).toBeTruthy();
  expect(within(asking).getByRole("button", { name: "Cancel Reword" }).className).toContain("default");
  expect(within(asking).getByRole("button", { name: "Keep Editing" })).toBeTruthy();

  // Esc inside the confirmation keeps editing.
  await fireEvent.keyDown(asking, { key: "Escape" });
  expect(screen.queryByRole("dialog", { name: "Cancel Reword" })).toBeNull();
  expect(editorField().value).toBe("typed");
  expect(document.activeElement).toBe(editorField());

  // Keep Editing keeps the typed text, with the focus in the editor.
  await fireEvent.keyDown(editorField(), { key: "Escape" });
  await fireEvent.click(within(confirmation()).getByRole("button", { name: "Keep Editing" }));
  expect(editorField().value).toBe("typed");
  expect(document.activeElement).toBe(editorField());

  // Cancel Reword restores the old message.
  await fireEvent.keyDown(editorField(), { key: "Escape" });
  await fireEvent.click(within(confirmation()).getByRole("button", { name: "Cancel Reword" }));
  expect(screen.queryByRole("textbox", { name: "Commit message" })).toBeNull();
  expect(screen.getByRole("dialog", { name: "Rebasing Commits" })).toBeTruthy();
  expect(rows()[1].textContent?.trim()).toBe("B");
});

test("escape_with_an_unchanged_reword_closes_silently", async () => {
  open();
  await fireEvent.click(rows()[1]);
  await fireEvent.click(button("Reword"));
  await fireEvent.keyDown(editorField(), { key: "Escape" });

  expect(screen.queryByRole("dialog", { name: "Cancel Reword" })).toBeNull();
  expect(screen.queryByRole("textbox", { name: "Commit message" })).toBeNull();
});

test("reset_with_a_changed_reword_asks_first", async () => {
  open();
  await typeIntoReword();
  await fireEvent.click(button("Reset"));

  const asking = confirmation();
  // Keep Editing leaves everything as it was.
  await fireEvent.click(within(asking).getByRole("button", { name: "Keep Editing" }));
  expect(editorField().value).toBe("typed");
  expect(screen.getByRole("button", { name: "Reset" })).toBeTruthy();

  await fireEvent.click(button("Reset"));
  await fireEvent.click(within(confirmation()).getByRole("button", { name: "Cancel Reword" }));
  expect(screen.queryByRole("textbox", { name: "Commit message" })).toBeNull();
  expect(screen.queryByRole("button", { name: "Reset" })).toBeNull();
  expect(rows()[1].textContent?.trim()).toBe("B");
});

test("editing_inline_with_cmd_enter_applies", async () => {
  const { onanswer } = open();
  await fireEvent.click(rows()[1]);
  await fireEvent.click(button("Reword"));

  const editor = screen.getByRole("textbox", { name: "Commit message" }) as HTMLTextAreaElement;
  expect(screen.getByText("Press ⌘⏎ to apply changes")).toBeTruthy();
  await fireEvent.input(editor, { target: { value: "B new\n\nanother body" } });
  await fireEvent.keyDown(editor, { key: "Enter", metaKey: true });

  expect(screen.queryByRole("textbox", { name: "Commit message" })).toBeNull();
  expect(rows()[1].textContent?.trim()).toBe("B new");
  expect(rows()[1].querySelector(".subject")?.classList.contains("reworded")).toBe(true);
  await fireEvent.click(start());
  expect(onanswer).toHaveBeenCalledWith({
    entries: [
      { action: "pick", hash: "aaaaaaa", subject: "A" },
      { action: "reword", hash: "bbbbbbb", subject: "B" },
      { action: "pick", hash: "ccccccc", subject: "C" },
    ],
    messages: [{ hash: "bbbbbbb", message: "B new\n\nanother body" }],
  });
});

test("cancelling_with_changes_asks_discard_changes_and_cancel_rebase", async () => {
  const first = open();
  // Without changes, it cancels without asking.
  await fireEvent.click(button("Cancel"));
  expect(first.onanswer).toHaveBeenCalledWith(null);
  cleanup();

  const { onanswer } = open();
  await fireEvent.click(rows()[2]);
  await fireEvent.click(button("Drop"));
  expect(rows()[2].querySelector(".subject")?.classList.contains("dropped")).toBe(true);
  await fireEvent.click(button("Cancel"));

  const question = screen.getByRole("dialog", { name: "Cancel Rebase" });
  expect(within(question).getByText("Discard changes and cancel rebase?")).toBeTruthy();
  await fireEvent.click(within(question).getByRole("button", { name: "Continue Rebasing" }));
  expect(onanswer).not.toHaveBeenCalled();
  expect(screen.queryByRole("dialog", { name: "Cancel Rebase" })).toBeNull();

  await fireEvent.click(button("Cancel"));
  const again = screen.getByRole("dialog", { name: "Cancel Rebase" });
  await fireEvent.click(within(again).getByRole("button", { name: "Discard" }));
  expect(onanswer).toHaveBeenCalledWith(null);
});

test("reset_only_after_a_change", async () => {
  const { onanswer } = open();
  expect(screen.queryByRole("button", { name: "Reset" })).toBeNull();

  await fireEvent.click(rows()[0]);
  await fireEvent.keyDown(table(), { key: "∂", code: "KeyD", altKey: true });
  expect(rows()[0].querySelector(".subject")?.classList.contains("dropped")).toBe(true);
  await fireEvent.click(button("Reset"));

  expect(screen.queryByRole("button", { name: "Reset" })).toBeNull();
  expect(rows()[0].querySelector(".subject")?.classList.contains("dropped")).toBe(false);
  await fireEvent.click(start());
  expect(onanswer.mock.calls[0][0].entries.map(({ action }: { action: string }) => action)).toEqual([
    "pick",
    "pick",
    "pick",
  ]);
});

test("view_git_commands_shows_the_todo", async () => {
  open();
  await fireEvent.click(rows()[1]);
  await fireEvent.click(screen.getByRole("button", { name: "More squash options" }));
  await fireEvent.click(screen.getByRole("menuitem", { name: "Fixup" }));
  await fireEvent.contextMenu(rows()[0]);
  await fireEvent.click(screen.getByRole("menuitem", { name: "View Git Commands" }));

  const commands = screen.getByRole("dialog", { name: "Git Commands" });
  const lines = within(commands)
    .getAllByRole("row")
    .map((row) => [...row.querySelectorAll("th, td")].map((cell) => cell.textContent?.trim()));
  // With a single row, Fixup unites it with the previous one.
  expect(lines).toEqual([
    ["Action", "Hash", "Subject"],
    ["pick", "aaaaaaa", "A"],
    ["fixup", "bbbbbbb", "B"],
    ["pick", "ccccccc", "C"],
  ]);
  await fireEvent.click(within(commands).getByRole("button", { name: "OK" }));
  expect(screen.queryByRole("dialog", { name: "Git Commands" })).toBeNull();
});

test("details_of_the_selected_commit", async () => {
  open();
  expect(screen.getByText("No commits selected")).toBeTruthy();

  await fireEvent.click(rows()[1]);

  await waitFor(() => expect(screen.getByText("detail of b")).toBeTruthy());
  const asked = vi.mocked(invoke).mock.calls.filter(([name]) => name === "commit_details");
  expect(asked.at(-1)?.[1]).toEqual({ hashes: ["b".repeat(40)] });
  await waitFor(() => expect(screen.getByText("b.txt")).toBeTruthy());
});
