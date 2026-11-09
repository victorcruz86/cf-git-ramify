import { fireEvent, screen, waitFor } from "@testing-library/svelte";
import { get } from "svelte/store";
import { beforeEach, describe, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { CurrentRevision } from "../lib/api";
import { tasks } from "../lib/tasks";
import {
  LOADING_CURRENT_REVISION,
  currentRevisionLines,
  showCurrentRevision,
} from "./currentRevision";
import { revisionNumbersText } from "../lib/revisions";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn() }));

const HASH = "a".repeat(40);
const NOW = Date.UTC(2026, 10, 8, 12, 0, 0);
const REVISION: CurrentRevision = {
  hash: HASH,
  commitTime: NOW / 1000 - 120,
  author: "Ann (Bob)",
  message: "Fix the parser\n\nIt read one line too many.",
};

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  document.body.innerHTML = "";
});

describe("show_current_revision_shows_the_last_commit_on_any_branch_as_text", () => {
  test("`V:611` as lines: name, hash, author and committer, pretty date, whole message", () => {
    expect(currentRevisionLines("parser.ts", REVISION, NOW)).toEqual([
      "parser.ts",
      "",
      `Current version is ${HASH}.`,
      "Modified by Ann (Bob)",
      "2 minutes ago",
      "Fix the parser",
      "",
      "It read one line too many.",
    ]);
  });

  test("it is read in the background, then shown until Escape", async () => {
    let answer!: (revision: CurrentRevision) => void;
    vi.mocked(invoke).mockImplementation(
      (command) =>
        new Promise((resolve, reject) => {
          if (command !== "compare_current_revision") reject(new Error(command));
          answer = resolve as (revision: CurrentRevision) => void;
        }),
    );

    const shown = showCurrentRevision("src/parser.ts", NOW);
    await waitFor(() => expect(answer).toBeDefined());
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("compare_current_revision", {
      path: "src/parser.ts",
    });
    expect(get(tasks).map((task) => task.title)).toEqual([LOADING_CURRENT_REVISION]);

    answer(REVISION);
    await shown;
    const popup = await screen.findByRole("dialog", { name: "Current Revision" });
    expect(popup.textContent).toContain("parser.ts");
    expect(popup.textContent).toContain(`Current version is ${HASH}.`);
    expect(popup.textContent).toContain("It read one line too many.");
    expect(get(tasks)).toEqual([]);

    await fireEvent.keyDown(popup, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  test("a second one replaces the popup that is open", async () => {
    vi.mocked(invoke).mockResolvedValue(REVISION);
    await showCurrentRevision("src/parser.ts", NOW);
    await screen.findByRole("dialog", { name: "Current Revision" });
    await showCurrentRevision("src/other.ts", NOW);
    await waitFor(() => expect(screen.getAllByRole("dialog")).toHaveLength(1));
    expect(screen.getByRole("dialog").textContent).toContain("other.ts");
  });

  test("a file no commit touched shows nothing", async () => {
    vi.mocked(invoke).mockResolvedValue(null);
    await showCurrentRevision("new.txt", NOW);
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});

describe("copy_revision_number_is_oldest_first_space_separated", () => {
  test("the selection comes newest first and is copied the other way round", () => {
    expect(revisionNumbersText(["c".repeat(40), "b".repeat(40), HASH])).toBe(
      `${HASH} ${"b".repeat(40)} ${"c".repeat(40)}`,
    );
    expect(revisionNumbersText([HASH])).toBe(HASH);
  });
});
