import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import type { Extension } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { tags } from "@lezer/highlight";

export const LINE_HEIGHT = 20;

// Colors of expUI_darkScheme.xml: DEFAULT_KEYWORD, DEFAULT_STRING and the rest.
const COMMENT = "#7A7E85";
const CONSTANT = "#C77DBB";

export const diffHighlight = HighlightStyle.define([
  { tag: [tags.keyword, tags.modifier, tags.self, tags.null, tags.bool], color: "#CF8E6D" },
  { tag: [tags.string, tags.special(tags.string), tags.regexp, tags.character], color: "#6AAB73" },
  { tag: [tags.number, tags.integer, tags.float], color: "#2AACB8" },
  { tag: [tags.lineComment, tags.blockComment, tags.comment], color: COMMENT },
  { tag: tags.docComment, color: "#5F826B", fontStyle: "italic" },
  { tag: tags.escape, color: "#CF8E6D" },
  { tag: [tags.meta, tags.annotation], color: "#B3AE60" },
  { tag: [tags.function(tags.definition(tags.variableName))], color: "#56A8F5" },
  { tag: tags.function(tags.definition(tags.propertyName)), color: "#56A8F5" },
  { tag: [tags.constant(tags.variableName), tags.propertyName], color: CONSTANT },
  { tag: tags.heading, fontWeight: "bold", color: "#CF8E6D" },
  { tag: tags.link, color: "#56A8F5" },
]);

const FONT = 'ui-monospace, "SF Mono", Menlo, monospace';

const editorStyles = EditorView.theme(
  {
    "&": {
      color: "var(--editor-text)",
      backgroundColor: "var(--editor)",
      fontSize: "13px",
    },
    "&.cm-focused": { outline: "none" },
    ".cm-scroller": { fontFamily: FONT, lineHeight: `${LINE_HEIGHT}px` },
    ".cm-content": { caretColor: "var(--caret)", padding: 0 },
    ".cm-line": { padding: "0 6px" },
    ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--caret)" },
    "&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection":
      { backgroundColor: "var(--selection)" },
    ".cm-activeLine": { backgroundColor: "transparent" },
    // The color of a change is still visible under the cursor.
    "&.cm-focused .cm-activeLine:not([class*='rm-'])": {
      backgroundColor: "var(--editor-current-line)",
    },
    ".cm-gutters": {
      color: "var(--editor-line-number)",
      backgroundColor: "var(--editor)",
      border: "none",
    },
    ".cm-activeLineGutter": { backgroundColor: "transparent" },
    "&.cm-focused .cm-activeLineGutter": { color: "var(--editor-line-number-current)" },
    "&.cm-focused .cm-activeLineGutter:not([class*='rm-'])": {
      backgroundColor: "var(--editor-current-line)",
    },
    ".cm-lineNumbers .cm-gutterElement": { padding: "0 4px 0 8px", minWidth: "20px" },
    // Change blocks. MergeView, in the diff of a commit, uses its own classes.
    ".rm-inserted": { backgroundColor: "var(--diff-inserted)" },
    ".rm-deleted": { backgroundColor: "var(--diff-deleted)" },
    ".rm-modified": { backgroundColor: "var(--diff-modified)" },
    ".rm-modified.rm-words": { backgroundColor: "var(--diff-modified-line)" },
    ".rm-word-inserted": { backgroundColor: "var(--diff-inserted)" },
    ".rm-word-deleted": { backgroundColor: "var(--diff-deleted)" },
    ".rm-word-modified": { backgroundColor: "var(--diff-modified)" },
    ".rm-gap-inserted": { boxShadow: "inset 0 1px 0 var(--diff-inserted-stripe)" },
    ".rm-gap-deleted": { boxShadow: "inset 0 1px 0 var(--diff-deleted-stripe)" },
    ".rm-gap-modified": { boxShadow: "inset 0 1px 0 var(--diff-modified-stripe)" },
    ".rm-spacer": {
      boxSizing: "border-box",
      backgroundColor: "var(--panel)",
      borderTop: "1px solid var(--divider)",
      borderBottom: "1px solid var(--divider)",
    },
    ".rm-fold": {
      display: "flex",
      alignItems: "center",
      height: `${LINE_HEIGHT}px`,
      padding: "0 8px",
      color: "var(--editor-folded-text)",
      cursor: "pointer",
      backgroundColor: "var(--editor-folded)",
      fontFamily: "-apple-system, BlinkMacSystemFont, sans-serif",
      fontSize: "12px",
    },
    ".rm-fold:hover": { color: "var(--text)" },
    ".rm-old-line": { whiteSpace: "pre" },
    ".rm-checks": { width: "16px" },
    ".rm-checks .cm-gutterElement": {
      display: "flex",
      alignItems: "center",
      justifyContent: "center",
    },
    ".rm-check": { display: "inline-flex", color: "var(--icon)", cursor: "pointer" },
    ".rm-check svg": { display: "block" },
    ".rm-old-numbers .cm-gutterElement": { padding: "0 4px 0 8px", textAlign: "right" },
    // A hunk of another list, without a checkbox: like what is excluded in DiffDrawUtil, with no fill.
    ".rm-inserted.rm-foreign": {
      backgroundColor: "transparent",
      boxShadow: "inset 3px 0 0 var(--diff-inserted-stripe)",
    },
    ".rm-deleted.rm-foreign": {
      backgroundColor: "transparent",
      boxShadow: "inset 3px 0 0 var(--diff-deleted-stripe)",
    },
    ".rm-modified.rm-foreign": {
      backgroundColor: "transparent",
      boxShadow: "inset 3px 0 0 var(--diff-modified-stripe)",
    },
    // Three-pane merge: the conflict, the lines with words of each kind and what is already
    // resolved, which keeps only the border, like DiffDrawUtil with `withResolved`.
    ".rm-conflict": { backgroundColor: "var(--diff-conflict)" },
    ".rm-conflict.rm-words": { backgroundColor: "var(--diff-conflict-line)" },
    ".rm-inserted.rm-words": { backgroundColor: "var(--diff-inserted-line)" },
    ".rm-deleted.rm-words": { backgroundColor: "var(--diff-deleted-line)" },
    ".rm-word-conflict": { backgroundColor: "var(--diff-conflict)" },
    ".rm-gap-conflict": { boxShadow: "inset 0 1px 0 var(--diff-conflict-stripe)" },
    ...Object.fromEntries(
      (["inserted", "deleted", "modified", "conflict"] as const).flatMap((kind) => {
        const stripe = `var(--diff-${kind}-stripe)`;
        return [
          [`.rm-gap-after-${kind}`, { boxShadow: `inset 0 -1px 0 ${stripe}` }],
          [`.rm-${kind}.rm-resolved`, { backgroundColor: "transparent" }],
          [`.rm-${kind}.rm-resolved.rm-first`, { boxShadow: `inset 0 1px 0 ${stripe}` }],
          [`.rm-${kind}.rm-resolved.rm-last`, { boxShadow: `inset 0 -1px 0 ${stripe}` }],
          [
            `.rm-${kind}.rm-resolved.rm-first.rm-last`,
            { boxShadow: `inset 0 1px 0 ${stripe}, inset 0 -1px 0 ${stripe}` },
          ],
        ];
      }),
    ),
    ".rm-merge-actions .cm-gutterElement": {
      display: "flex",
      alignItems: "center",
      gap: "2px",
      padding: "0 2px",
    },
    ".rm-operation": { display: "inline-flex", color: "var(--icon)", cursor: "pointer" },
    ".rm-operation svg": { display: "block" },
    // Diff of a commit, with MergeView.
    "&.cm-merge-a .cm-changedLine": { backgroundColor: "var(--diff-deleted)" },
    "&.cm-merge-b .cm-changedLine": { backgroundColor: "var(--diff-inserted)" },
    "&.cm-merge-a .cm-changedText, &.cm-merge-b .cm-changedText": {
      background: "var(--diff-modified)",
    },
    ".cm-deletedLine, .cm-insertedLine": { textDecoration: "none" },
    ".cm-changeGutter": { width: "3px", padding: 0 },
    "&.cm-merge-a .cm-changedLineGutter": { background: "var(--diff-deleted-stripe)" },
    "&.cm-merge-b .cm-changedLineGutter": { background: "var(--diff-inserted-stripe)" },
    ".cm-collapsedLines": {
      color: "var(--editor-folded-text)",
      background: "var(--editor-folded)",
    },
  },
  { dark: true },
);

export const diffTheme: Extension = [editorStyles, syntaxHighlighting(diffHighlight)];
