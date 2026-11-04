import { LanguageDescription } from "@codemirror/language";
import { languages } from "@codemirror/language-data";
import { Compartment, EditorState, type Extension } from "@codemirror/state";
import { ViewPlugin } from "@codemirror/view";

function describe(path: string): LanguageDescription | null {
  return LanguageDescription.matchFilename(languages, path.slice(path.lastIndexOf("/") + 1));
}

// The grammar is loaded the first time it is needed and enters the editor when it arrives:
// opening a file does not wait for it nor does it bring in those of the other languages.
export function languageFor(path: string): Extension {
  const description = describe(path);
  if (!description) return [];
  if (description.support) return description.support;
  const slot = new Compartment();
  const loader = ViewPlugin.define((view) => {
    let alive = true;
    description.load().then(
      (support) => {
        if (alive) view.dispatch({ effects: slot.reconfigure(support) });
      },
      // Without a grammar the text looks the same, just without colors.
      () => {},
    );
    return {
      destroy() {
        alive = false;
      },
    };
  });
  return [slot.of([]), loader];
}

// The lines of a hunk are split only by `\n`. By default CodeMirror also splits by `\r`, and when
// saving it would lose the `\r\n`: with a single separator the numbering matches and the bytes come
// back identical. With mixed endings, `\r` stays in the text of the line.
export function lineSeparator(text: string): string {
  const crlf = text.includes("\r\n");
  const mixed = /(^|[^\r])\n|\r(?!\n)/.test(text);
  return crlf && !mixed ? "\r\n" : "\n";
}

export function separatorExtension(text: string): Extension {
  return EditorState.lineSeparator.of(lineSeparator(text));
}
