import { beforeEach, describe, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { backend as applying } from "./applying";
import { backend as editing } from "./editing";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const MODIFIED = { path: "a.txt", oldPath: null, state: "modified", staged: false, unstaged: true };

function mockSettings(settings: { nonModalCommit: boolean; stagingArea: boolean }) {
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "settings_get") return settings;
    if (command === "repo_status") return [MODIFIED];
    // Without staging area the changelists are read, and nothing is on the active list.
    if (command === "changelists_get") {
      return {
        lists: [{ id: "default", name: "Changes", comment: "" }],
        active: "default",
        assignments: {},
        hunkAssignments: {},
      };
    }
    throw new Error(`unexpected command: ${String(command)}`);
  });
}

beforeEach(() => {
  vi.mocked(invoke).mockReset();
});

describe("the staging area of the log flows is the effective one", () => {
  test("a saved staging area is not in effect while the commit interface is modal", async () => {
    mockSettings({ nonModalCommit: false, stagingArea: true });

    expect(await editing.stagingArea()).toBe(false);
    expect(await applying.stagingArea()).toBe(false);
    // The changelists are read, as in the commit view.
    expect(await editing.activeChanges()).toEqual({ kind: "project", paths: ["a.txt"], lists: ["default"] });
    expect(vi.mocked(invoke).mock.calls.map(([name]) => name)).toContain("changelists_get");
  });

  test("it is in effect in the non-modal interface", async () => {
    mockSettings({ nonModalCommit: true, stagingArea: true });

    expect(await editing.stagingArea()).toBe(true);
    expect(await applying.stagingArea()).toBe(true);
    expect(await editing.activeChanges()).toEqual({ kind: "project", paths: ["a.txt"], lists: [] });
    expect(vi.mocked(invoke).mock.calls.map(([name]) => name)).not.toContain("changelists_get");
  });
});
