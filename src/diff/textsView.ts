import { MergeView } from "@codemirror/merge";
import { EditorState, type Extension } from "@codemirror/state";
import { EditorView, lineNumbers } from "@codemirror/view";
import { softWrap, wrapping } from "./editor";
import { languageFor } from "./language";
import { diffTheme } from "./theme";

// Two read-only texts side by side, unchanged runs collapsed. `path` only picks the highlighting.
export function createTextsView(
  parent: HTMLElement,
  oldText: string,
  newText: string,
  path: string,
  wrap: boolean,
): MergeView {
  const extensions: Extension[] = [
    lineNumbers(),
    diffTheme,
    EditorState.readOnly.of(true),
    EditorView.editable.of(false),
    wrapping.of(softWrap(wrap)),
    languageFor(path),
  ];
  return new MergeView({
    a: { doc: oldText, extensions },
    b: { doc: newText, extensions },
    parent,
    collapseUnchanged: { margin: 3, minSize: 8 },
  });
}
