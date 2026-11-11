import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { ProjectsList } from "../lib/api";
import { homeRelative, openProject, type OpenProjectDeps } from "./open";
import { PROJECT_PALETTE, projectColorIndex } from "./projectIcon";
import ProjectWidget from "./ProjectWidget.svelte";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const HOME = "/Users/me";
const LIST: ProjectsList = {
  open: [{ path: "/Users/me/work/other", name: "other", homePath: "~/work/other", exists: true }],
  recent: [
    { path: "/Users/me/work/old-one", name: "old-one", homePath: "~/work/old-one", exists: true },
    { path: "/srv/shared/lib", name: "lib", homePath: "/srv/shared/lib", exists: true },
  ],
};

let list: ProjectsList;
let focused: boolean;

beforeEach(() => {
  list = LIST;
  focused = true;
  vi.mocked(invoke).mockImplementation(async (command: string) => {
    if (command === "projects_list") return list;
    if (command === "project_focus_open") return focused;
    return undefined;
  });
});

afterEach(() => {
  cleanup();
  vi.mocked(invoke).mockReset();
});

const calls = (command: string) =>
  vi.mocked(invoke).mock.calls.filter(([name]) => name === command);

function widget(props: Partial<Parameters<typeof render<typeof ProjectWidget>>[1]> = {}) {
  const handlers = { open: vi.fn(async () => {}), choose: vi.fn(), missing: vi.fn(), clone: vi.fn() };
  render(ProjectWidget, {
    path: "/Users/me/work/ramify",
    home: HOME,
    onopen: handlers.open,
    onmissing: handlers.missing,
    onchoose: handlers.choose,
    onclone: handlers.clone,
    ...props,
  });
  return handlers;
}

const pop = async () => {
  await fireEvent.click(screen.getByRole("button", { name: /ramify/ }));
  return await screen.findByRole("menu");
};

test("the button shows the icon and the name, and its tooltip is the path from ~", () => {
  widget();
  const button = screen.getByRole("button", { name: /ramify/ });
  expect(button.textContent).toContain("ramify");
  expect(button.getAttribute("title")).toBe("~/work/ramify");
  const icon = button.querySelector(".project-icon") as HTMLElement;
  expect(icon.textContent).toBe("R");
  const [start, end] = PROJECT_PALETTE[projectColorIndex("/Users/me/work/ramify")];
  expect(icon.style.getPropertyValue("--icon-start").toUpperCase()).toBe(start);
  expect(icon.style.getPropertyValue("--icon-end").toUpperCase()).toBe(end);
});

test("a path outside the home folder is the tooltip as it is", () => {
  widget({ path: "/srv/shared/lib" });
  expect(screen.getByRole("button", { name: /lib/ }).getAttribute("title")).toBe("/srv/shared/lib");
});

test("without a repository the widget is not shown", () => {
  widget({ path: null });
  expect(screen.queryByRole("button")).toBeNull();
});

test("the popup shows Open…, Get from Version Control…, the open projects and the recent ones", async () => {
  widget();
  const menu = await pop();
  await within(menu).findByText("Recent Projects");
  const texts = Array.from(menu.querySelectorAll("[role=menuitem], .section")).map((node) =>
    node.textContent?.replace(/\s+/g, " ").trim(),
  );
  expect(texts).toEqual([
    "Open…",
    "Get from Version Control…",
    "Open Projects",
    "O other ~/work/other",
    "Recent Projects",
    "OO old-one ~/work/old-one",
    "L lib /srv/shared/lib",
  ]);
});

test("a section without projects is not shown", async () => {
  list = { open: [], recent: [] };
  widget();
  const menu = await pop();
  await waitFor(() => expect(calls("projects_list")).toHaveLength(1));
  expect(within(menu).queryByText("Open Projects")).toBeNull();
  expect(within(menu).queryByText("Recent Projects")).toBeNull();
  expect(within(menu).getByText("Open…")).toBeTruthy();
});

test("a path is text, never markup", async () => {
  list = {
    open: [],
    recent: [{ path: "/x/<b>bold</b>", name: "<b>bold</b>", homePath: "/x/<b>bold</b>", exists: true }],
  };
  widget();
  const menu = await pop();
  await within(menu).findByText("Recent Projects");
  expect(menu.querySelector("b")).toBeNull();
  expect(menu.textContent).toContain("<b>bold</b>");
});

test("choosing an open project focuses its window", async () => {
  const handlers = widget();
  const menu = await pop();
  await fireEvent.click(await within(menu).findByText("other"));
  expect(calls("project_focus_open")).toEqual([["project_focus_open", { root: "/Users/me/work/other" }]]);
  expect(handlers.open).not.toHaveBeenCalled();
  await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
});

test("an open project whose window is gone goes through the open flow", async () => {
  focused = false;
  const handlers = widget();
  const menu = await pop();
  await fireEvent.click(await within(menu).findByText("other"));
  await waitFor(() => expect(handlers.open).toHaveBeenCalledWith("/Users/me/work/other"));
});

test("choosing a recent project goes through the open flow", async () => {
  const deps = {
    resolve: vi.fn(async (target: string) => target),
    focusOpen: vi.fn(async () => false),
    current: vi.fn(() => "/Users/me/work/ramify"),
    setting: vi.fn(() => "current" as const),
    openHere: vi.fn(async () => {}),
    newWindow: vi.fn(),
    ask: vi.fn(),
    remember: vi.fn(),
    focusHere: vi.fn(async () => {}),
  } satisfies OpenProjectDeps;
  widget({ onopen: (target: string) => openProject(target, deps) });
  const menu = await pop();
  await fireEvent.click(await within(menu).findByText("old-one"));
  await waitFor(() => expect(deps.resolve).toHaveBeenCalledWith("/Users/me/work/old-one"));
  expect(deps.openHere).toHaveBeenCalledWith("/Users/me/work/old-one");
  expect(calls("project_focus_open")).toHaveLength(0);
});

test("Open… runs the chooser and Get from Version Control… the clone dialog", async () => {
  const handlers = widget();
  let menu = await pop();
  await fireEvent.click(within(menu).getByText("Open…"));
  expect(handlers.choose).toHaveBeenCalledTimes(1);
  expect(screen.queryByRole("menu")).toBeNull();

  menu = await pop();
  await fireEvent.click(within(menu).getByText("Get from Version Control…"));
  expect(handlers.clone).toHaveBeenCalledTimes(1);
});

test("Escape closes the popup", async () => {
  widget();
  const menu = await pop();
  await fireEvent.keyDown(menu, { key: "Escape" });
  expect(screen.queryByRole("menu")).toBeNull();
});

test("the home-relative path is the one of IntelliJ", () => {
  expect(homeRelative("/Users/me/work/a", "/Users/me")).toBe("~/work/a");
  expect(homeRelative("/Users/me", "/Users/me")).toBe("/Users/me");
  expect(homeRelative("/Users/meow/a", "/Users/me")).toBe("/Users/meow/a");
  expect(homeRelative("/srv/a", null)).toBe("/srv/a");
});

test("a recent project whose folder is gone asks about it instead of opening", async () => {
  list = {
    open: [],
    recent: [{ path: "/gone/app", name: "app", homePath: "/gone/app", exists: false }],
  };
  const handlers = widget();
  const menu = await pop();
  await fireEvent.click(await within(menu).findByText("app"));
  expect(handlers.missing).toHaveBeenCalledWith("/gone/app");
  expect(handlers.open).not.toHaveBeenCalled();
});

test("Escape closes the popup while the focus is still on the widget button", async () => {
  widget();
  await pop();
  (document.activeElement as HTMLElement | null)?.blur();
  screen.getByRole("button", { name: /ramify/ }).focus();
  await fireEvent.keyDown(window, { key: "Escape" });
  expect(screen.queryByRole("menu")).toBeNull();
});
