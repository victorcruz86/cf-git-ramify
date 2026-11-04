import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { language } from "@codemirror/language";
import { expect, test, vi } from "vitest";
import { languageFor, lineSeparator } from "./language";

test.each(["a.rs", "a.ts", "a.py", "a.go", "a.json", "a.c", "src/b.h", "Makefile", "no_extension"])(
  "%s_returns_an_extension_without_failing",
  (path) => {
    expect(() => EditorState.create({ extensions: [languageFor(path)] })).not.toThrow();
  },
);

test("loads_the_needed_grammar_and_applies_it", async () => {
  const view = new EditorView({
    state: EditorState.create({ doc: "fn main() {}", extensions: [languageFor("a.rs")] }),
  });

  await vi.waitFor(() => expect(view.state.facet(language)?.name).toBe("rust"));
  view.destroy();
});

test("a_file_without_a_language_loads_nothing", () => {
  const state = EditorState.create({ extensions: [languageFor("no_extension")] });

  expect(state.facet(language)).toBeNull();
});

test("line_separator", () => {
  expect(lineSeparator("a\nb\n")).toBe("\n");
  expect(lineSeparator("a\r\nb\r\n")).toBe("\r\n");
  expect(lineSeparator("a\r\nb")).toBe("\r\n");
  expect(lineSeparator("a\r\nb\n")).toBe("\n");
  expect(lineSeparator("a\rb\r\n")).toBe("\n");
  expect(lineSeparator("")).toBe("\n");
});
