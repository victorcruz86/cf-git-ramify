import { cleanup, fireEvent, render, screen, within } from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import type { OpenProjectIn } from "../lib/api";
import OpenProjectDialog from "./OpenProjectDialog.svelte";
import {
  isOpenElsewhere,
  openProject,
  projectName,
  type OpenAnswer,
  type OpenProjectDeps,
} from "./open";

afterEach(() => cleanup());

const OPEN_ROOT = "/work/app";

// The backend's resolver: a subfolder, a trailing slash and a symlink of the open repository all
// give its root.
const RESOLVED: Record<string, string> = {
  "/work/app": OPEN_ROOT,
  "/work/app/src/deep": OPEN_ROOT,
  "/work/app/": OPEN_ROOT,
  "/links/app": OPEN_ROOT,
  "/work/other": "/work/other",
  "/work/other/sub": "/work/other",
};

function deps(overrides: Partial<OpenProjectDeps> = {}) {
  const fake = {
    resolve: vi.fn(async (target: string) => {
      const root = RESOLVED[target];
      if (!root) throw { kind: "not_a_repository", message: `${target} is not a Git repository` };
      return root;
    }),
    // Another window has `/work/app` open.
    focusOpen: vi.fn(async (root: string) => root === OPEN_ROOT),
    current: vi.fn((): string | null => "/work/mine"),
    setting: vi.fn((): OpenProjectIn => "ask"),
    openHere: vi.fn(async () => {}),
    newWindow: vi.fn(async (root: string) => ({ label: `window of ${root}`, created: true })),
    ask: vi.fn(async (): Promise<OpenAnswer> => ({ choice: "here", remember: false })),
    remember: vi.fn(async () => {}),
    focusHere: vi.fn(async () => {}),
    ...overrides,
  };
  return fake;
}

function nothingOpened(fake: ReturnType<typeof deps>) {
  expect(fake.openHere).not.toHaveBeenCalled();
  expect(fake.newWindow).not.toHaveBeenCalled();
  expect(fake.ask).not.toHaveBeenCalled();
}

test("an already open repository focuses its window and opens nothing", async () => {
  for (const target of ["/work/app", "/work/app/src/deep", "/work/app/", "/links/app"]) {
    for (const current of [null, "/work/mine"]) {
      const fake = deps({ current: vi.fn(() => current) });

      await openProject(target, fake);

      expect(fake.resolve, target).toHaveBeenCalledWith(target);
      expect(fake.focusOpen, target).toHaveBeenCalledWith(OPEN_ROOT);
      nothingOpened(fake);
    }
  }
});

test("without a repository it opens here without asking", async () => {
  const fake = deps({ current: vi.fn(() => null), setting: vi.fn(() => "new" as const) });

  await openProject("/work/other/sub", fake);

  expect(fake.openHere).toHaveBeenCalledWith("/work/other");
  expect(fake.newWindow).not.toHaveBeenCalled();
  expect(fake.ask).not.toHaveBeenCalled();
});

// `findAndFocusExistingProjectForPath` also matches the calling frame: the window is focused and
// nothing is opened again (its auxiliary windows and its commit draft stay).
test("the repository this window already has only focuses this window", async () => {
  for (const setting of ["ask", "current", "new"] as const) {
    const fake = deps({ current: vi.fn(() => "/work/other"), setting: vi.fn(() => setting) });

    await openProject("/work/other/sub", fake);

    expect(fake.focusHere, setting).toHaveBeenCalledTimes(1);
    nothingOpened(fake);
  }
});

test("current opens here", async () => {
  const fake = deps({ setting: vi.fn(() => "current" as const) });

  await openProject("/work/other", fake);

  expect(fake.openHere).toHaveBeenCalledWith("/work/other");
  expect(fake.newWindow).not.toHaveBeenCalled();
  expect(fake.ask).not.toHaveBeenCalled();
});

test("new opens a new window", async () => {
  const fake = deps({ setting: vi.fn(() => "new" as const) });

  await openProject("/work/other", fake);

  expect(fake.newWindow).toHaveBeenCalledWith("/work/other");
  expect(fake.openHere).not.toHaveBeenCalled();
  expect(fake.ask).not.toHaveBeenCalled();
});

test("ask shows the dialog and does what is chosen", async () => {
  const here = deps();
  await openProject("/work/other", here);
  expect(here.ask).toHaveBeenCalledWith("other");
  expect(here.openHere).toHaveBeenCalledWith("/work/other");
  expect(here.newWindow).not.toHaveBeenCalled();
  expect(here.remember).not.toHaveBeenCalled();

  const elsewhere = deps({ ask: vi.fn(async () => ({ choice: "new" as const, remember: false })) });
  await openProject("/work/other", elsewhere);
  expect(elsewhere.newWindow).toHaveBeenCalledWith("/work/other");
  expect(elsewhere.openHere).not.toHaveBeenCalled();
  expect(elsewhere.remember).not.toHaveBeenCalled();
});

test("Don't ask again saves the choice", async () => {
  const here = deps({ ask: vi.fn(async () => ({ choice: "here" as const, remember: true })) });
  await openProject("/work/other", here);
  expect(here.remember).toHaveBeenCalledWith("current");
  expect(here.openHere).toHaveBeenCalledWith("/work/other");

  const elsewhere = deps({ ask: vi.fn(async () => ({ choice: "new" as const, remember: true })) });
  await openProject("/work/other", elsewhere);
  expect(elsewhere.remember).toHaveBeenCalledWith("new");
  expect(elsewhere.newWindow).toHaveBeenCalledWith("/work/other");
});

test("Cancel does nothing", async () => {
  const fake = deps({ ask: vi.fn(async () => ({ choice: "cancel" as const, remember: true })) });

  await openProject("/work/other", fake);

  expect(fake.openHere).not.toHaveBeenCalled();
  expect(fake.newWindow).not.toHaveBeenCalled();
  expect(fake.remember).not.toHaveBeenCalled();
});

test("what cannot be resolved fails and opens nothing", async () => {
  const fake = deps();

  await expect(openProject("/nowhere", fake)).rejects.toEqual({
    kind: "not_a_repository",
    message: "/nowhere is not a Git repository",
  });
  expect(fake.focusOpen).not.toHaveBeenCalled();
  nothingOpened(fake);
});

test("the project name is the root's folder", () => {
  expect(projectName("/work/other")).toBe("other");
  expect(projectName("/work/other/")).toBe("other");
  expect(projectName("/")).toBe("/");
});

test("the dialog texts and buttons", async () => {
  const onanswer = vi.fn();
  render(OpenProjectDialog, { name: "other", onanswer });

  const dialog = screen.getByRole("dialog", { name: "Open Project" });
  expect(dialog.textContent).toContain("Where would you like to open the project 'other'?");
  const buttons = within(dialog)
    .getAllByRole("button")
    .map((button) => button.textContent?.trim());
  // As macOS lays out IntelliJ's message dialogs: the default button on the right.
  expect(buttons).toEqual(["Cancel", "New Window", "This Window"]);
  const thisWindow = within(dialog).getByRole("button", { name: "This Window" });
  expect(thisWindow.classList.contains("default")).toBe(true);
  expect(document.activeElement).toBe(thisWindow);
  const remember = within(dialog).getByRole("checkbox", { name: "Don't ask again" });
  expect((remember as HTMLInputElement).checked).toBe(false);

  // Enter is the default button.
  await fireEvent.submit(thisWindow.closest("form")!);
  expect(onanswer).toHaveBeenLastCalledWith({ choice: "here", remember: false });
});

test("the dialog answers each button, the checkbox and Escape", async () => {
  const onanswer = vi.fn();
  render(OpenProjectDialog, { name: "other", onanswer });
  const dialog = screen.getByRole("dialog", { name: "Open Project" });

  await fireEvent.click(within(dialog).getByRole("checkbox", { name: "Don't ask again" }));
  await fireEvent.click(within(dialog).getByRole("button", { name: "New Window" }));
  expect(onanswer).toHaveBeenLastCalledWith({ choice: "new", remember: true });

  await fireEvent.click(within(dialog).getByRole("button", { name: "This Window" }));
  expect(onanswer).toHaveBeenLastCalledWith({ choice: "here", remember: true });

  await fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
  expect(onanswer).toHaveBeenLastCalledWith({ choice: "cancel", remember: true });

  await fireEvent.keyDown(dialog, { key: "Escape" });
  expect(onanswer).toHaveBeenLastCalledWith({ choice: "cancel", remember: true });
});

test("a root another window started opening meanwhile is not an error to show", () => {
  expect(isOpenElsewhere({ kind: "open_elsewhere", message: "x" })).toBe(true);
  expect(isOpenElsewhere({ kind: "not_a_repository", message: "x" })).toBe(false);
  expect(isOpenElsewhere(null)).toBe(false);
});

test("a new window that was not built because another window has the root opens nothing here", async () => {
  const fake = deps({
    setting: vi.fn(() => "new" as const),
    newWindow: vi.fn(async () => ({ label: "main-2", created: false })),
  });

  await openProject("/work/other", fake);

  expect(fake.newWindow).toHaveBeenCalledWith("/work/other");
  expect(fake.openHere).not.toHaveBeenCalled();
});
