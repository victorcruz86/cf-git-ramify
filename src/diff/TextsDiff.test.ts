import { EditorView } from "@codemirror/view";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test } from "vitest";
import TextsDiff from "./TextsDiff.svelte";

// jsdom does no layout: CodeMirror measures with this.
Range.prototype.getClientRects = () => [] as unknown as DOMRectList;

const wrapped = (view: EditorView) => view.contentDOM.classList.contains("cm-lineWrapping");

const views = () =>
  [...document.querySelectorAll<HTMLElement>(".cm-editor")].map(
    (dom) => EditorView.findFromDOM(dom) as EditorView,
  );

beforeEach(() => localStorage.clear());
afterEach(cleanup);

async function show(titles?: [string, string]) {
  render(TextsDiff, { oldText: "a\nb\nc\n", newText: "a\nB\nc\n", path: "a.txt", titles });
  await waitFor(() => expect(document.querySelectorAll(".cm-editor")).toHaveLength(2));
}

test("texts_diff_shows_both_texts_with_their_titles", async () => {
  await show(["Base", "Shelved version"]);

  expect(views().map((view) => view.state.doc.toString())).toEqual(["a\nb\nc\n", "a\nB\nc\n"]);
  expect(views().every((view) => view.state.readOnly)).toBe(true);
  expect([...document.querySelectorAll(".title")].map((title) => title.textContent)).toEqual([
    "Base",
    "Shelved version",
  ]);
});

test("texts_diff_without_titles_has_no_header", async () => {
  await show();

  expect(document.querySelector(".titles")).toBeNull();
});

test("texts_diff_wraps_from_the_saved_patches_place", async () => {
  localStorage.setItem("diff.wrap.Default", "true");
  await show();
  expect(views().map(wrapped)).toEqual([false, false]);

  await fireEvent.contextMenu(document.querySelector(".cm-gutters .cm-gutterElement") as HTMLElement);
  await fireEvent.click(await screen.findByRole("menuitem", { name: "Soft-Wrap" }));

  expect(views().map(wrapped)).toEqual([true, true]);
  expect(localStorage.getItem("diff.wrap.SavedPatchesUiPlace")).toBe("true");
  expect(localStorage.getItem("diff.wrap.Default")).toBe("true");
});

// Apply Patch's `Show Difference` opens in `DiffPlaces.DEFAULT`, which the user has wrapped.
test("texts_diff_takes_the_wrap_of_the_place_it_is_given", async () => {
  localStorage.setItem("diff.wrap.Default", "true");
  render(TextsDiff, { oldText: "a\n", newText: "b\n", path: "a.txt", place: "Default" });
  await waitFor(() => expect(document.querySelectorAll(".cm-editor")).toHaveLength(2));

  expect(views().map(wrapped)).toEqual([true, true]);
});
