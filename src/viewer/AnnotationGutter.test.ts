import type { EditorView } from "@codemirror/view";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { AnnotatedCommit, Annotation } from "../lib/api";
import AnnotationGutter from "./AnnotationGutter.svelte";
import { annotationOptions, DEFAULT_OPTIONS, prettyDate } from "./annotation";
import { createViewerEditor } from "./editor";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(), emit: vi.fn() }));

// jsdom does no layout: CodeMirror measures with this.
Range.prototype.getClientRects = () => [] as unknown as DOMRectList;

const OLD = "1111111111111111111111111111111111111111";
const NEW = "2222222222222222222222222222222222222222";
const TIME = new Date(2020, 0, 1, 10, 0).getTime() / 1000;

function commit(hash: string, change: Partial<AnnotatedCommit> = {}): AnnotatedCommit {
  return {
    hash,
    path: "src/a.txt",
    author: "Ann Smith",
    email: "ann@example.com",
    authorTime: TIME,
    committerTime: TIME,
    subject: "first",
    message: "first <b>not html</b>",
    previous: null,
    number: 1,
    orderColor: "#242f45",
    authorColor: "#2f5194",
    ...change,
  };
}

const annotation: Annotation = {
  revision: NEW,
  path: "src/a.txt",
  commits: [
    commit(OLD),
    commit(NEW, {
      author: "Bob Jones",
      email: "bob@example.com",
      number: 2,
      previous: { hash: OLD, path: "src/a.txt" },
      orderColor: "#2f5194",
      authorColor: "#2d4473",
    }),
  ],
  lines: [0, null, 1],
  origins: [0, 1, 1],
  owners: [0, 1],
  suspicious: false,
  historySize: 2,
  line: null,
};

let view: EditorView;
let host: HTMLElement;
const handlers = () => ({ close: vi.fn(), showDiff: vi.fn(), annotate: vi.fn() });

// The cells of the lines, without the hidden spacer that keeps the gutter as wide as its widest.
function cells(): HTMLElement[] {
  return [...view.dom.querySelectorAll<HTMLElement>(".cm-annotations .cm-annotation")].filter(
    (cell) => (cell.parentElement as HTMLElement).style.visibility !== "hidden",
  );
}
const texts = () =>
  cells().map((cell) =>
    [...cell.querySelectorAll(".cm-annotation-column")].map((column) => column.textContent),
  );

beforeEach(() => {
  localStorage.clear();
  annotationOptions.set(DEFAULT_OPTIONS);
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockResolvedValue(undefined);
  host = document.createElement("div");
  document.body.append(host);
  view = createViewerEditor(host, "one\ntwo\nthree\n", "src/a.txt", false);
});

afterEach(() => {
  cleanup();
  view.destroy();
  host.remove();
});

test("the gutter shows date and author left of the numbers, blank where the line changed", async () => {
  render(AnnotationGutter, { view, annotation, ...handlers() });

  await waitFor(() => expect(cells()).toHaveLength(4));
  const date = prettyDate(TIME * 1000, Date.now());
  expect(texts()).toEqual([[date, "Smith"], ["", ""], [date, "Jones"], ["", ""]]);
  const gutters = [...view.dom.querySelectorAll(".cm-gutter")];
  expect(gutters.indexOf(view.dom.querySelector(".cm-annotations")!)).toBeLessThan(
    gutters.indexOf(view.dom.querySelector(".cm-lineNumbers")!),
  );
  // The revision annotated is bold and lighter; the order colours are the background.
  const [first, blank, latest] = cells();
  expect(latest.style.fontWeight).toBe("bold");
  expect(latest.style.color).toBe("rgb(206, 208, 214)");
  expect(latest.style.backgroundColor).toBe("rgb(47, 81, 148)");
  expect(first.style.fontWeight).toBe("");
  expect(first.style.color).toBe("rgb(141, 145, 153)");
  expect(first.style.backgroundColor).toBe("rgb(36, 47, 69)");
  expect(blank.style.backgroundColor).toBe("");
});

test("the tooltip is plain text with the commit", async () => {
  render(AnnotationGutter, { view, annotation, ...handlers() });

  await waitFor(() => expect(cells()).toHaveLength(4));
  const title = cells()[2].title;
  expect(title.startsWith(`commit ${NEW}\nAuthor: Bob Jones <bob@example.com>\nDate: `)).toBe(true);
  expect(title.endsWith("\n\nfirst <b>not html</b>")).toBe(true);
  expect(cells()[2].querySelector("b")).toBeNull();
  expect(cells()[1].title).toBe("");
});

test("a click selects the commit in the main log", async () => {
  render(AnnotationGutter, { view, annotation, ...handlers() });
  await waitFor(() => expect(cells()).toHaveLength(4));

  await fireEvent.click(cells()[2]);
  await fireEvent.click(cells()[1]);

  expect(vi.mocked(invoke).mock.calls).toEqual([["annotation_show_in_log", { hash: NEW }]]);
});

test("the popup changes the columns for every annotation and closes them", async () => {
  const run = handlers();
  render(AnnotationGutter, { view, annotation, ...run });
  await waitFor(() => expect(cells()).toHaveLength(4));

  await fireEvent.contextMenu(cells()[0]);
  const menu = await screen.findByRole("menu", { name: "Annotations" });
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "View" }));
  const viewMenu = await screen.findByRole("menu", { name: "View" });
  await fireEvent.click(within(viewMenu).getByRole("menuitem", { name: "Revision" }));

  await waitFor(() => expect(texts()[0]).toEqual(["11111111", expect.any(String), "Smith"]));
  expect(JSON.parse(localStorage.getItem("rama.annotation.options")!).shown.revision).toBe(true);

  await fireEvent.contextMenu(cells()[2]);
  await fireEvent.click(
    within(await screen.findByRole("menu", { name: "Annotations" })).getByRole("menuitem", {
      name: "Annotate Previous Revision",
    }),
  );
  // The annotation's line of the third line shown is its second (`getAnnotationLineNumber`).
  expect(run.annotate).toHaveBeenCalledWith({ hash: OLD, path: "src/a.txt" }, 2);

  await fireEvent.contextMenu(cells()[2]);
  await fireEvent.click(
    within(await screen.findByRole("menu", { name: "Annotations" })).getByRole("menuitem", {
      name: "Close Annotations",
    }),
  );
  expect(run.close).toHaveBeenCalled();
});

test("copy revision number copies the line's full hash", async () => {
  render(AnnotationGutter, { view, annotation, ...handlers() });
  await waitFor(() => expect(cells()).toHaveLength(4));

  await fireEvent.contextMenu(cells()[2]);
  await fireEvent.click(
    within(await screen.findByRole("menu", { name: "Annotations" })).getByRole("menuitem", {
      name: "Copy Revision Number",
    }),
  );

  expect(invoke).toHaveBeenCalledWith("clipboard_write", { text: NEW });
});

test("unmounting takes the gutter away", async () => {
  const { unmount } = render(AnnotationGutter, { view, annotation, ...handlers() });
  await waitFor(() => expect(cells()).toHaveLength(4));

  unmount();

  expect(view.dom.querySelector(".cm-annotations")).toBeNull();
});
