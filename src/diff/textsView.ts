import { MergeView } from "@codemirror/merge";
import { EditorState, type Extension } from "@codemirror/state";
import { EditorView, lineNumbers } from "@codemirror/view";
import { softWrap, wrapping } from "./editor";
import { languageFor } from "./language";
import { diffTheme } from "./theme";

// Two read-only texts side by side, unchanged runs collapsed. `path` only picks the highlighting.
// `starts` are the lines (from 0) of whole files where the texts start, which their line numbers
// count from (`DiffUserDataKeysEx.LINE_NUMBER_CONVERTOR`).
export function createTextsView(
  parent: HTMLElement,
  oldText: string,
  newText: string,
  path: string,
  wrap: boolean,
  starts: [number, number] = [0, 0],
): MergeView {
  const extensions = (start: number): Extension[] => [
    start === 0 ? lineNumbers() : lineNumbers({ formatNumber: (line) => String(line + start) }),
    diffTheme,
    EditorState.readOnly.of(true),
    EditorView.editable.of(false),
    wrapping.of(softWrap(wrap)),
    languageFor(path),
  ];
  return new MergeView({
    a: { doc: oldText, extensions: extensions(starts[0]) },
    b: { doc: newText, extensions: extensions(starts[1]) },
    parent,
    collapseUnchanged: { margin: 3, minSize: 8 },
  });
}
