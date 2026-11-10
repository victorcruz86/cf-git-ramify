import { defaultKeymap } from "@codemirror/commands";
import { Compartment, EditorSelection, EditorState } from "@codemirror/state";
import {
  drawSelection,
  EditorView,
  highlightActiveLine,
  highlightActiveLineGutter,
  keymap,
  lineNumbers,
} from "@codemirror/view";
import { softWrap, wrapping } from "../diff/editor";
import { languageFor, separatorExtension } from "../diff/language";
import { diffTheme } from "../diff/theme";

// The gutter left of the line numbers, empty until Annotate fills it (`AnnotationGutter`), as
// IntelliJ puts the annotations before the numbers.
export const annotationGutter = new Compartment();
// The line separator follows the text, which a new read can change.
const separator = new Compartment();

export interface LineRange {
  // From 1, both included.
  startLine: number;
  endLine: number;
}

// A read-only editor: it has a caret, a selection and the keys that move them, and typing does
// nothing (`isWritable()` false). `path` only picks the highlighting.
export function createViewerEditor(
  parent: HTMLElement,
  text: string,
  path: string,
  wrap: boolean,
): EditorView {
  return new EditorView({
    parent,
    state: EditorState.create({
      doc: text,
      extensions: [
        annotationGutter.of([]),
        lineNumbers(),
        diffTheme,
        wrapping.of(softWrap(wrap)),
        highlightActiveLine(),
        highlightActiveLineGutter(),
        drawSelection(),
        EditorState.readOnly.of(true),
        keymap.of(defaultKeymap),
        separator.of(separatorExtension(text)),
        languageFor(path),
        EditorView.contentAttributes.of({ spellcheck: "false", autocorrect: "off" }),
      ],
    }),
  });
}

// The lines of the main selection, or of the caret without one.
export function selectedLines(view: EditorView): LineRange {
  const { from, to } = view.state.selection.main;
  const doc = view.state.doc;
  return { startLine: doc.lineAt(from).number, endLine: doc.lineAt(to).number };
}

// The lines of a GitHub link, 0-based (`HostedGitRepositoryReferenceUtil`): the line of the selection's
// start and of its end, with the end one line up when it sits exactly at the start of its line. 241's
// quirk is kept: with no selection and the caret at column 0 of line k the end is k-1, so the URL
// ends `#Lk-L(k-1)`; with the caret mid-line it is `#Lk`.
export function linkLines(view: EditorView): [number, number] {
  const { from, to } = view.state.selection.main;
  const { doc } = view.state;
  const first = doc.lineAt(from).number - 1;
  const end = doc.lineAt(to);
  return [first, to === end.from ? end.number - 2 : end.number - 1];
}

// `OpenFileDescriptor` with a line: the caret at its start, the line in the middle of the view.
// A line past the end goes to the last one.
export function goToLine(view: EditorView, line: number) {
  const doc = view.state.doc;
  const target = doc.line(Math.min(Math.max(line, 1), doc.lines));
  view.dispatch({
    selection: EditorSelection.cursor(target.from),
    effects: EditorView.scrollIntoView(target.from, { y: "center" }),
  });
}

// A new read of the same file: the text changes in place, so the caret, the selection and the
// scroll stay where they were (clamped to the new text).
export function replaceText(view: EditorView, text: string) {
  const { scrollTop, scrollLeft } = view.scrollDOM;
  const { anchor, head } = view.state.selection.main;
  view.dispatch({
    changes: { from: 0, to: view.state.doc.length, insert: text },
    effects: separator.reconfigure(separatorExtension(text)),
  });
  const clamp = (position: number) => Math.min(position, view.state.doc.length);
  view.dispatch({ selection: EditorSelection.range(clamp(anchor), clamp(head)) });
  view.scrollDOM.scrollTop = scrollTop;
  view.scrollDOM.scrollLeft = scrollLeft;
}
