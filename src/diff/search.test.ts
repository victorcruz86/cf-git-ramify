import { SearchQuery } from "@codemirror/search";
import { EditorSelection, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { fireEvent, within } from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import { DiffEditors, type DiffData } from "./editor";
import { searchStatus } from "./search";

// jsdom does no layout: CodeMirror measures with this.
Range.prototype.getClientRects = () => [] as unknown as DOMRectList;

let editors: DiffEditors | null = null;
const away = vi.fn();

afterEach(() => {
  editors?.destroy();
  editors = null;
  document.body.textContent = "";
  window.removeEventListener("keydown", away);
  away.mockReset();
});

const OLD = "alpha line\nbeta Line\ngamma line\n";
const NEW = "alpha line\nbeta changed\ngamma line\n";

function open(unified = false) {
  const data: DiffData = {
    oldText: OLD,
    newText: NEW,
    hunks: [{ id: "h1", oldStart: 2, oldLines: 1, newStart: 2, newLines: 1 }],
    selectable: [],
    selection: [],
  };
  const parent = document.body.appendChild(document.createElement("div"));
  editors = new DiffEditors(parent, {
    path: "a.txt",
    data,
    draft: null,
    editable: false,
    unified,
    embedded: false,
    options: { highlight: "words", collapse: false, align: true, sync: true, wrap: false },
    handlers: { ontoggle: vi.fn(), onedit: vi.fn(), onfocus: vi.fn() },
  });
  // The diff window closes on Escape: what the editor takes must not reach it.
  window.addEventListener("keydown", away);
  const hosts = [...parent.querySelectorAll<HTMLElement>(".rm-editor")];
  const view = (host: HTMLElement) =>
    EditorView.findFromDOM(host.querySelector(".cm-content") as HTMLElement) as EditorView;
  return { hosts, views: hosts.map(view) };
}

const statusOf = (host: HTMLElement) => within(host).getByRole("status").textContent;
const field = (host: HTMLElement) => within(host).getByRole("textbox", { name: "Search" }) as HTMLInputElement;
const selected = (view: EditorView) => {
  const { from, to } = view.state.selection.main;
  return view.state.sliceDoc(from, to);
};

async function type(host: HTMLElement, text: string) {
  await fireEvent.input(field(host), { target: { value: text } });
}

test("the_status_is_intellijs", () => {
  const state = EditorState.create({ doc: "one two one\none" });
  const query = (search: string, extra: Partial<{ regexp: boolean; caseSensitive: boolean }> = {}) =>
    new SearchQuery({ search, literal: true, ...extra });

  expect(searchStatus(state, query(""))).toEqual({ text: "0 results", notFound: false });
  expect(searchStatus(state, query("one"))).toEqual({ text: "3 results", notFound: false });
  expect(searchStatus(state, query("two"))).toEqual({ text: "1 result", notFound: false });
  expect(searchStatus(state, query("zzz"))).toEqual({ text: "0 results", notFound: true });
  expect(searchStatus(state, query("(", { regexp: true }))).toEqual({ text: "Bad pattern", notFound: true });
  const onSecond = state.update({ selection: EditorSelection.single(8, 11) }).state;
  expect(searchStatus(onSecond, query("one"))).toEqual({ text: "2/3", notFound: false });
  const many = EditorState.create({ doc: "a".repeat(10_001) });
  expect(searchStatus(many, query("a")).text).toBe("10000+");
});

test("cmd_f_opens_the_bar_over_the_side_with_the_caret_and_typing_goes_to_the_first_match", async () => {
  const { hosts, views } = open();
  const [left, right] = hosts;

  await fireEvent.keyDown(views[0].contentDOM, { key: "f", metaKey: true });

  expect(within(left).getByRole("search")).toBeTruthy();
  expect(within(right).queryByRole("search")).toBeNull();
  expect(document.activeElement).toBe(field(left));
  expect(statusOf(left)).toBe("0 results");
  await type(left, "line");
  expect(statusOf(left)).toBe("1/3");
  expect(selected(views[0])).toBe("line");
  expect(away).not.toHaveBeenCalled();
});

test("enter_and_cmd_g_go_down_shift_goes_up_and_the_options_change_the_matches", async () => {
  const { hosts, views } = open();
  const [left] = hosts;
  await fireEvent.keyDown(views[0].contentDOM, { key: "f", metaKey: true });
  await type(left, "line");

  await fireEvent.keyDown(field(left), { key: "Enter" });
  expect(statusOf(left)).toBe("2/3");
  await fireEvent.keyDown(field(left), { key: "g", metaKey: true });
  expect(statusOf(left)).toBe("3/3");
  await fireEvent.keyDown(field(left), { key: "Enter", shiftKey: true });
  expect(statusOf(left)).toBe("2/3");

  const matchCase = within(left).getByRole("button", { name: "Match Case" });
  await fireEvent.click(matchCase);
  expect(matchCase.getAttribute("aria-pressed")).toBe("true");
  // `Line` of the second line no longer counts, and the search starts again where the bar opened.
  expect(statusOf(left)).toBe("1/2");
  await fireEvent.click(within(left).getByRole("button", { name: "Regex" }));
  await type(left, "(");
  expect(statusOf(left)).toBe("Bad pattern");
  expect(left.querySelector(".rm-find")?.classList.contains("rm-find-not-found")).toBe(true);
  expect(away).not.toHaveBeenCalled();
});

test("the_selected_text_starts_the_search_and_each_side_keeps_its_own", async () => {
  const { hosts, views } = open();
  const [left, right] = hosts;
  views[1].dispatch({ selection: EditorSelection.single(16, 23) });

  await fireEvent.keyDown(views[1].contentDOM, { key: "f", metaKey: true });

  expect(field(right).value).toBe("changed");
  expect(statusOf(right)).toBe("1/1");
  expect(within(left).queryByRole("search")).toBeNull();
});

test("escape_closes_the_bar_and_only_then_reaches_the_window", async () => {
  const { hosts, views } = open();
  const [left] = hosts;
  await fireEvent.keyDown(views[0].contentDOM, { key: "f", metaKey: true });

  await fireEvent.keyDown(field(left), { key: "Escape" });
  expect(within(left).queryByRole("search")).toBeNull();
  expect(document.activeElement).toBe(views[0].contentDOM);

  await fireEvent.keyDown(views[0].contentDOM, { key: "f", metaKey: true });
  await fireEvent.keyDown(views[0].contentDOM, { key: "Escape" });
  expect(within(left).queryByRole("search")).toBeNull();
  expect(away).not.toHaveBeenCalled();

  await fireEvent.keyDown(views[0].contentDOM, { key: "Escape" });
  expect(away).toHaveBeenCalledTimes(1);
});

test("the_unified_view_searches_its_one_editor", async () => {
  const { hosts, views } = open(true);

  await fireEvent.keyDown(views[0].contentDOM, { key: "f", metaKey: true });
  await type(hosts[0], "gamma");

  expect(statusOf(hosts[0])).toBe("1/1");
});
