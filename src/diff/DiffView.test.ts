import { EditorView } from "@codemirror/view";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { FileSides } from "../lib/api";
import DiffView from "./DiffView.svelte";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

// jsdom does no layout: CodeMirror measures with this.
Range.prototype.getClientRects = () => [] as unknown as DOMRectList;

// What `EditorView.lineWrapping` puts on the content; `view.lineWrapping` only follows it after a
// measure, which jsdom does not do.
const wrapped = (view: EditorView) => view.contentDOM.classList.contains("cm-lineWrapping");

const SIDES: FileSides = {
  old: { kind: "text", content: "a\nb\nc\n" },
  new: { kind: "text", content: "a\nB\nc\n" },
};

beforeEach(() => {
  localStorage.clear();
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "file_sides") return SIDES;
    throw new Error(`unexpected command: ${command}`);
  });
});

afterEach(cleanup);

async function show() {
  render(DiffView, {
    hash: "abc",
    file: { path: "a.txt", oldPath: null, status: "M" },
    base: "parent",
  });
  await waitFor(() => expect(document.querySelectorAll(".cm-editor")).toHaveLength(2));
}

const views = () =>
  [...document.querySelectorAll<HTMLElement>(".cm-editor")].map(
    (dom) => EditorView.findFromDOM(dom) as EditorView,
  );

test("vcs_log_view_wraps_from_its_own_place_default_off", async () => {
  localStorage.setItem("diff.wrap.Default", "true");
  await show();

  expect(views().map(wrapped)).toEqual([false, false]);
  expect(document.querySelector(".cm-lineWrapping")).toBeNull();

  cleanup();
  localStorage.setItem("diff.wrap.VcsLogView", "true");
  await show();

  expect(views().map(wrapped)).toEqual([true, true]);
});

test("right_click_on_the_gutter_offers_soft_wrap_and_stores_it_for_the_log", async () => {
  await show();
  const before = views();

  await fireEvent.contextMenu(document.querySelector(".cm-gutters .cm-gutterElement") as HTMLElement);
  const menu = await screen.findByRole("menu");
  expect(within(menu).getAllByRole("menuitem").map((item) => item.getAttribute("aria-label"))).toEqual([
    "Soft-Wrap",
  ]);
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Soft-Wrap" }));

  expect(views()).toEqual(before);
  expect(before.map(wrapped)).toEqual([true, true]);
  expect(localStorage.getItem("diff.wrap.VcsLogView")).toBe("true");
  expect(localStorage.getItem("diff.wrap.Default")).toBeNull();

  await fireEvent.contextMenu(document.querySelector(".cm-gutters .cm-gutterElement") as HTMLElement);
  await fireEvent.click(await screen.findByRole("menuitem", { name: "Soft-Wrap" }));

  expect(views()).toEqual(before);
  expect(before.map(wrapped)).toEqual([false, false]);
  expect(localStorage.getItem("diff.wrap.VcsLogView")).toBe("false");

  // The text has no menu of its own.
  await fireEvent.contextMenu(document.querySelector(".cm-line") as HTMLElement);
  expect(screen.queryByRole("menu")).toBeNull();
});
