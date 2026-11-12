import { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
import { language, type Language } from "@codemirror/language";
import { presentableDiff, type Change } from "@codemirror/merge";
import {
  Compartment,
  EditorSelection,
  EditorState,
  Facet,
  RangeSet,
  StateEffect,
  StateField,
  Transaction,
  type Extension,
  type Range,
  type Text,
} from "@codemirror/state";
import {
  Decoration,
  EditorView,
  GutterMarker,
  ViewPlugin,
  WidgetType,
  drawSelection,
  gutter,
  gutterLineClass,
  gutterWidgetClass,
  highlightActiveLine,
  highlightActiveLineGutter,
  keymap,
  lineNumbers,
  type DecorationSet,
  type ViewUpdate,
} from "@codemirror/view";
import { highlightTree } from "@lezer/highlight";
import type { Hunk, LineSelection } from "../lib/api";
import { changeType, foldRuns, mapLine, type ChangeType, type FoldRun } from "./blocks";
import { marginFor, type MarginCheck, type Side } from "./gutter";
import checkOff from "./icons/gutterCheckBox.svg?raw";
import checkOn from "./icons/gutterCheckBoxSelected.svg?raw";
import { languageFor, lineSeparator, separatorExtension } from "./language";
import { alignSpacers, boundariesOf, withoutSpacers } from "./spacers";
import { diffHighlight, diffTheme, LINE_HEIGHT, numberColumn } from "./theme";
import { stripeBlocks } from "./stripe";
import { StripeLane } from "./stripeLane";

export type HighlightMode = "words" | "lines" | "none";

export interface EditorOptions {
  highlight: HighlightMode;
  collapse: boolean;
  align: boolean;
  sync: boolean;
  // `Soft-Wrap`: each place remembers its own (`options.ts`).
  wrap: boolean;
}

export interface DiffData {
  oldText: string;
  newText: string;
  // What is drawn, and of that what has a checkbox (the rest belongs to another list, or is not split).
  hunks: Hunk[];
  selectable: Hunk[];
  selection: LineSelection[];
}

export interface Handlers {
  ontoggle: (hunk: Hunk, line: number | null) => void;
  onedit: () => void;
  onfocus: () => void;
}

export interface LineRange {
  side: Side;
  from: number;
  to: number;
  exact: boolean;
}

// IntelliJ: `DiffSettings.contextRange`, 4 by default.
const CONTEXT_LINES = 4;
export const WORD_DIFF_MAX_CHARS = 40_000;
const OLD_LINE_SYNTAX_MAX_CHARS = 1_000_000;

// A spacer of the aligned view, before the text of `line` (or after the end of the document).
// `blank` ones level unchanged text, before a hunk or at the end, and are not painted.
interface SpacerAt {
  line: number;
  height: number;
  blank: boolean;
}

interface Model {
  side: Side;
  unified: boolean;
  hunks: Hunk[];
  selectable: Hunk[];
  selection: LineSelection[];
  hover: { side: Side; line: number } | null;
  margin: boolean;
  highlight: HighlightMode;
  align: boolean;
  folds: FoldRun[];
  other: Text;
  // The document changed and the hunks of what was typed have not arrived yet.
  stale: boolean;
  // Measured with soft wrap on; null is one spacer per hunk of the lines the other side has more.
  spacers: SpacerAt[] | null;
}

const updateModel = StateEffect.define<Partial<Model>>();
// Only the measuring pass sends it, so it can tell its own transactions apart.
const measuredSpacers = StateEffect.define<SpacerAt[] | null>();
const handlersFacet = Facet.define<Handlers, Handlers>({ combine: (values) => values[0] });

const modelField = StateField.define<Model>({
  create: () => {
    throw new Error("the model is initialized with `modelField.init`");
  },
  update(model, transaction) {
    let next = model;
    if (transaction.docChanged && !model.stale) next = { ...next, stale: true };
    for (const effect of transaction.effects) {
      if (effect.is(updateModel)) {
        // Measured spacers belong to the hunks and the alignment they were measured for.
        const { hunks, align } = effect.value;
        const rehunked = hunks !== undefined && hunks !== next.hunks;
        const realigned = align !== undefined && align !== next.align;
        next = { ...next, ...(rehunked || realigned ? { spacers: null } : {}), ...effect.value };
      }
      if (effect.is(measuredSpacers)) next = { ...next, spacers: effect.value };
    }
    return next;
  },
});

const sideStart = (hunk: Hunk, side: Side) => (side === "old" ? hunk.oldStart : hunk.newStart);
const sideLines = (hunk: Hunk, side: Side) => (side === "old" ? hunk.oldLines : hunk.newLines);

// A hunk computed for another version of the text must not go outside the document.
function lineFrom(doc: Text, line: number): number {
  return line < doc.lines ? doc.line(line + 1).from : doc.length;
}

function sliceLines(doc: Text, start: number, lines: number): string {
  if (lines === 0) return "";
  const from = lineFrom(doc, start);
  const end = Math.min(start + lines, doc.lines);
  return doc.sliceString(from, doc.line(end).to);
}

const wordCache = new WeakMap<Hunk, readonly Change[]>();

function wordChanges(hunk: Hunk, oldDoc: Text, newDoc: Text): readonly Change[] {
  const known = wordCache.get(hunk);
  if (known) return known;
  const before = sliceLines(oldDoc, hunk.oldStart, hunk.oldLines);
  const after = sliceLines(newDoc, hunk.newStart, hunk.newLines);
  const changes =
    before.length + after.length > WORD_DIFF_MAX_CHARS
      ? []
      : presentableDiff(before, after, { scanLimit: 500 });
  wordCache.set(hunk, changes);
  return changes;
}

const showsWords = (model: Model, hunk: Hunk) =>
  model.highlight === "words" && !model.stale && changeType(hunk) === "modified";

function wordKind(change: Change): ChangeType {
  if (change.fromA === change.toA) return "inserted";
  return change.fromB === change.toB ? "deleted" : "modified";
}

// Heights closer than this are the same: what is measured comes in fractions of a pixel.
const SPACER_TOLERANCE = 1;

class SpacerWidget extends WidgetType {
  constructor(
    readonly height: number,
    readonly blank: boolean,
  ) {
    super();
  }
  // Exact, as MergeView's Spacer: an equal widget keeps the old DOM, which must then have the
  // height the model counts. The tolerance belongs to `sameSpacers`, before anything is sent.
  eq(other: SpacerWidget) {
    return other.blank === this.blank && other.height === this.height;
  }
  get estimatedHeight() {
    return this.height;
  }
  toDOM() {
    const dom = document.createElement("div");
    dom.className = this.blank ? "rm-spacer rm-spacer-blank" : "rm-spacer";
    dom.style.height = `${this.height}px`;
    return dom;
  }
  // Same class only: a measured height resizes the spacer instead of recreating it.
  updateDOM(dom: HTMLElement, _view: EditorView, from: this) {
    if (from.blank !== this.blank) return false;
    dom.style.height = `${this.height}px`;
    return true;
  }
}

// Without measuring, equal lines are taken as equally tall: the shorter side of each hunk gets the
// lines it lacks, after the hunk.
function countedSpacers(model: Model): SpacerAt[] {
  return model.hunks.flatMap((hunk) => {
    const lines = sideLines(hunk, model.side);
    const other = model.side === "old" ? hunk.newLines : hunk.oldLines;
    if (other <= lines) return [];
    const line = sideStart(hunk, model.side) + lines;
    return [{ line, height: (other - lines) * LINE_HEIGHT, blank: false }];
  });
}

const shownSpacers = (model: Model): SpacerAt[] => model.spacers ?? countedSpacers(model);

function sameSpacers(before: readonly SpacerAt[], after: readonly SpacerAt[]): boolean {
  return (
    before.length === after.length &&
    before.every(
      (spacer, index) =>
        spacer.line === after[index].line &&
        spacer.blank === after[index].blank &&
        Math.abs(spacer.height - after[index].height) <= SPACER_TOLERANCE,
    )
  );
}

class FoldWidget extends WidgetType {
  constructor(
    readonly run: FoldRun,
    readonly onexpand: (run: FoldRun) => void,
  ) {
    super();
  }
  eq(other: FoldWidget) {
    return other.run.old === this.run.old && other.run.lines === this.run.lines;
  }
  get estimatedHeight() {
    return LINE_HEIGHT;
  }
  toDOM() {
    const dom = document.createElement("div");
    dom.className = "rm-fold";
    dom.textContent = `${this.run.lines} unchanged lines`;
    dom.title = "Expand";
    dom.addEventListener("mousedown", (event) => {
      event.preventDefault();
      this.onexpand(this.run);
    });
    return dom;
  }
  ignoreEvent() {
    return true;
  }
}

// A line of the other side in the unified view: an old line, or a new one when the document is the
// left side (`MASTER_SIDE = LEFT`). A block of its own per line, so the margin has a checkbox and a
// number on each one.
class OldLineWidget extends WidgetType {
  constructor(
    readonly hunk: Hunk,
    readonly line: number,
    readonly text: string,
    readonly spans: readonly { from: number; to: number; className: string }[],
    readonly background: string,
    readonly side: Side = "old",
  ) {
    super();
  }
  eq(other: OldLineWidget) {
    return (
      other.side === this.side &&
      other.line === this.line &&
      other.text === this.text &&
      other.background === this.background &&
      other.spans.length === this.spans.length
    );
  }
  get estimatedHeight() {
    return LINE_HEIGHT;
  }
  toDOM() {
    const dom = document.createElement("div");
    dom.className = `cm-line rm-old-line ${this.background}`;
    const stops = new Set([0, this.text.length]);
    for (const span of this.spans) stops.add(span.from).add(span.to);
    const ordered = [...stops].sort((left, right) => left - right);
    ordered.slice(0, -1).forEach((from, index) => {
      const to = ordered[index + 1];
      const piece = document.createElement("span");
      piece.textContent = this.text.slice(from, to);
      piece.className = this.spans
        .filter((span) => span.from <= from && span.to >= to)
        .map((span) => span.className)
        .join(" ");
      dom.append(piece);
    });
    if (this.text === "") dom.append(document.createElement("br"));
    return dom;
  }
}

type SyntaxTree = ReturnType<Language["parser"]["parse"]>;
const syntaxTrees = new WeakMap<Text, { language: Language; tree: SyntaxTree }>();

function oldTree(doc: Text, current: Language) {
  if (doc.length > OLD_LINE_SYNTAX_MAX_CHARS) return null;
  const known = syntaxTrees.get(doc);
  if (known?.language === current) return known.tree;
  const tree = current.parser.parse(doc.toString());
  syntaxTrees.set(doc, { language: current, tree });
  return tree;
}

const otherSide = (side: Side): Side => (side === "old" ? "new" : "old");

function oldLineWidgets(state: EditorState, model: Model, hunk: Hunk): OldLineWidget[] {
  const doc = model.other;
  const side = otherSide(model.side);
  const current = state.facet(language);
  const tree = current && oldTree(doc, current);
  const [oldDoc, newDoc] = side === "old" ? [doc, state.doc] : [state.doc, doc];
  const words = showsWords(model, hunk) ? wordChanges(hunk, oldDoc, newDoc) : [];
  const first = sideStart(hunk, side);
  const hunkFrom = lineFrom(doc, first);
  const type = changeType(hunk);
  const widgets: OldLineWidget[] = [];
  for (let index = 0; index < sideLines(hunk, side); index++) {
    const lineNumber = first + index;
    if (lineNumber >= doc.lines) break;
    const line = doc.line(lineNumber + 1);
    const spans: { from: number; to: number; className: string }[] = [];
    if (tree) {
      highlightTree(
        tree,
        diffHighlight,
        (from, to, className) => {
          spans.push({
            from: Math.max(from, line.from) - line.from,
            to: Math.min(to, line.to) - line.from,
            className,
          });
        },
        line.from,
        line.to,
      );
    }
    for (const change of words) {
      const [start, end] =
        side === "old" ? [change.fromA, change.toA] : [change.fromB, change.toB];
      const from = Math.max(hunkFrom + start, line.from);
      const to = Math.min(hunkFrom + end, line.to);
      if (from < to) {
        spans.push({
          from: from - line.from,
          to: to - line.from,
          className: `rm-word-${wordKind(change)}`,
        });
      }
    }
    const lineBackground = lineClass(model, hunk, type);
    widgets.push(new OldLineWidget(hunk, lineNumber, line.text, spans, lineBackground, side));
  }
  return widgets;
}

const selectableSets = new WeakMap<Hunk[], Set<Hunk>>();

function isSelectable(model: Model, hunk: Hunk): boolean {
  let hunks = selectableSets.get(model.selectable);
  if (!hunks) {
    hunks = new Set(model.selectable);
    selectableSets.set(model.selectable, hunks);
  }
  return hunks.has(hunk);
}

function lineClass(model: Model, hunk: Hunk, type: ChangeType): string {
  if (model.highlight === "none") return "";
  const foreign = model.margin && hunk.id !== "" && !isSelectable(model, hunk);
  const words = showsWords(model, hunk) ? " rm-words" : "";
  return `rm-${type}${words}${foreign ? " rm-foreign" : ""}`;
}

function buildBlocks(state: EditorState, model: Model, onexpand: (run: FoldRun) => void) {
  const doc = state.doc;
  const ranges: Range<Decoration>[] = [];
  for (const hunk of model.hunks) {
    const side = model.side;
    const start = sideStart(hunk, side);
    const lines = sideLines(hunk, side);
    const type = changeType(hunk);
    const className = lineClass(model, hunk, type);
    if (className) {
      for (let index = 0; index < lines && start + index < doc.lines; index++) {
        ranges.push(Decoration.line({ class: className }).range(lineFrom(doc, start + index)));
      }
    }
    if (model.unified) {
      // The old lines go before the new ones: before the hunk on the new side, after it on the old.
      const at = side === "old" ? start + lines : start;
      for (const widget of oldLineWidgets(state, model, hunk)) {
        const atEnd = at >= doc.lines;
        ranges.push(
          Decoration.widget({ widget, block: true, side: atEnd ? 1 : -1 }).range(
            lineFrom(doc, at),
          ),
        );
      }
      continue;
    }
    const other = side === "old" ? hunk.newLines : hunk.oldLines;
    if (!(model.align && other > lines) && lines === 0 && model.highlight !== "none") {
      ranges.push(Decoration.line({ class: `rm-gap-${type}` }).range(lineFrom(doc, start)));
    }
  }
  if (model.align && !model.unified) {
    // In order: spacers of the same line stack as their boundaries come.
    for (const spacer of shownSpacers(model)) {
      const atEnd = spacer.line >= doc.lines;
      ranges.push(
        Decoration.widget({
          widget: new SpacerWidget(spacer.height, spacer.blank),
          block: true,
          side: atEnd ? 1 : -1,
        }).range(lineFrom(doc, spacer.line)),
      );
    }
  }
  for (const run of model.folds) {
    const first = model.side === "old" ? run.old : run.new;
    if (first + run.lines > doc.lines) continue;
    ranges.push(
      Decoration.replace({ block: true, widget: new FoldWidget(run, onexpand) }).range(
        doc.line(first + 1).from,
        doc.line(first + run.lines).to,
      ),
    );
  }
  return Decoration.set(ranges, true);
}

const expandFacet = Facet.define<(run: FoldRun) => void, (run: FoldRun) => void>({
  combine: (values) => values[0],
});

function blocksChanged(before: Model, after: Model): boolean {
  return (
    before.hunks !== after.hunks ||
    before.folds !== after.folds ||
    before.highlight !== after.highlight ||
    before.align !== after.align ||
    before.margin !== after.margin ||
    before.selectable !== after.selectable ||
    before.spacers !== after.spacers
  );
}

const blocksField = StateField.define<DecorationSet>({
  create: (state) => buildBlocks(state, state.field(modelField), state.facet(expandFacet)),
  update(blocks, transaction) {
    const before = transaction.startState.field(modelField);
    const after = transaction.state.field(modelField);
    const parsed = transaction.startState.facet(language) !== transaction.state.facet(language);
    if (blocksChanged(before, after) || (after.unified && parsed)) {
      return buildBlocks(transaction.state, after, transaction.state.facet(expandFacet));
    }
    return blocks.map(transaction.changes);
  },
  provide: (field) => EditorView.decorations.from(field),
});

// Word marks are computed only for what is visible: it is the only expensive thing to paint.
const wordMarks = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    constructor(view: EditorView) {
      this.decorations = this.build(view);
    }
    update(update: ViewUpdate) {
      const changed = update.startState.field(modelField) !== update.state.field(modelField);
      if (update.docChanged || update.viewportChanged || changed) {
        this.decorations = this.build(update.view);
      }
    }
    build(view: EditorView) {
      const model = view.state.field(modelField);
      if (model.highlight !== "words" || model.stale) return Decoration.none;
      const doc = view.state.doc;
      const oldDoc = model.side === "old" ? doc : model.other;
      const newDoc = model.side === "old" ? model.other : doc;
      const ranges: Range<Decoration>[] = [];
      const { from: top, to: bottom } = view.viewport;
      for (const hunk of model.hunks) {
        if (!showsWords(model, hunk)) continue;
        const start = lineFrom(doc, sideStart(hunk, model.side));
        const hunkEnd = sideStart(hunk, model.side) + sideLines(hunk, model.side);
        const lastLine = Math.min(hunkEnd, doc.lines);
        const end = doc.line(lastLine).to;
        if (end < top || start > bottom) continue;
        for (const change of wordChanges(hunk, oldDoc, newDoc)) {
          const [from, to] =
            model.side === "old" ? [change.fromA, change.toA] : [change.fromB, change.toB];
          if (from === to) continue;
          ranges.push(
            Decoration.mark({ class: `rm-word-${wordKind(change)}` }).range(
              start + from,
              start + to,
            ),
          );
        }
      }
      return Decoration.set(ranges, true);
    }
  },
  { decorations: (plugin) => plugin.decorations },
);

class CheckMarker extends GutterMarker {
  constructor(readonly check: MarginCheck) {
    super();
  }
  eq(other: CheckMarker) {
    return (
      other.check.checked === this.check.checked &&
      other.check.line === this.check.line &&
      other.check.hunk.id === this.check.hunk.id
    );
  }
  toDOM() {
    const dom = document.createElement("span");
    dom.className = "rm-check";
    dom.title = "Include into commit";
    dom.setAttribute("role", "checkbox");
    dom.setAttribute("aria-checked", String(this.check.checked));
    dom.setAttribute("aria-label", "Include into commit");
    dom.dataset.hunk = this.check.hunk.id;
    if (this.check.line !== null) dom.dataset.line = String(this.check.line);
    dom.innerHTML = (this.check.checked ? checkOn : checkOff).replace(/<!--.*?-->\s*/s, "");
    return dom;
  }
}

const marginCache = new WeakMap<Model, Map<Side, Map<number, MarginCheck>>>();

function marginOf(model: Model, side: Side): Map<number, MarginCheck> {
  let sides = marginCache.get(model);
  if (!sides) {
    sides = new Map();
    marginCache.set(model, sides);
  }
  let margin = sides.get(side);
  if (!margin) {
    margin =
      model.margin && !model.stale
        ? marginFor(model.selectable, model.selection, side, model.hover)
        : new Map();
    sides.set(side, margin);
  }
  return margin;
}

interface Target {
  side: Side;
  line: number;
}

// What is at the height of the mouse: a document line or an old line of the unified view.
function targetAt(view: EditorView, clientY: number): Target | null {
  const block = view.elementAtHeight(clientY - view.documentTop);
  if (block.widget instanceof OldLineWidget) {
    return { side: block.widget.side, line: block.widget.line };
  }
  if (block.widget) return null;
  const { side } = view.state.field(modelField);
  return { side, line: view.state.doc.lineAt(block.from).number - 1 };
}

const checkGutter = gutter({
  class: "rm-checks",
  lineMarker(view, block) {
    const model = view.state.field(modelField);
    const check = marginOf(model, model.side).get(view.state.doc.lineAt(block.from).number - 1);
    return check ? new CheckMarker(check) : null;
  },
  widgetMarker(view, widget) {
    if (!(widget instanceof OldLineWidget)) return null;
    const check = marginOf(view.state.field(modelField), widget.side).get(widget.line);
    return check ? new CheckMarker(check) : null;
  },
  lineMarkerChange: (update) =>
    update.startState.field(modelField) !== update.state.field(modelField),
  domEventHandlers: {
    mousedown(view, _block, event) {
      const element = (event.target as HTMLElement).closest<HTMLElement>(".rm-check");
      const { hunk: id, line } = element?.dataset ?? {};
      const hunk = view.state.field(modelField).selectable.find((item) => item.id === id);
      if (!hunk) return false;
      view.state.facet(handlersFacet).ontoggle(hunk, line === undefined ? null : Number(line));
      return true;
    },
    mousemove(view, _block, event) {
      const hover = targetAt(view, (event as MouseEvent).clientY);
      const model = view.state.field(modelField);
      if (hover?.line !== model.hover?.line || hover?.side !== model.hover?.side) {
        view.dispatch({ effects: updateModel.of({ hover }) });
      }
      return false;
    },
    mouseleave(view) {
      if (view.state.field(modelField).hover) {
        view.dispatch({ effects: updateModel.of({ hover: null }) });
      }
      return false;
    },
  },
});

class BackgroundMarker extends GutterMarker {
  constructor(readonly elementClass: string) {
    super();
  }
}

// The color of the change continues through the margin, like the LineMarkerRenderer of DiffDrawUtil.
const gutterBackground: Extension = [
  gutterLineClass.compute([modelField, "doc"], (state) => {
    const model = state.field(modelField);
    const ranges: Range<GutterMarker>[] = [];
    for (const hunk of model.hunks) {
      const className = lineClass(model, hunk, changeType(hunk));
      if (!className) continue;
      const start = sideStart(hunk, model.side);
      const end = Math.min(start + sideLines(hunk, model.side), state.doc.lines);
      const marker = new BackgroundMarker(className);
      for (let line = start; line < end; line++) {
        ranges.push(marker.range(lineFrom(state.doc, line)));
      }
    }
    return RangeSet.of(ranges, true);
  }),
  gutterWidgetClass.of((_view, widget) =>
    widget instanceof OldLineWidget ? new BackgroundMarker(widget.background) : null,
  ),
];

class NumberMarker extends GutterMarker {
  constructor(readonly value: number) {
    super();
  }
  eq(other: NumberMarker) {
    return other.value === this.value;
  }
  toDOM() {
    return document.createTextNode(String(this.value + 1));
  }
}

// The column of numbers of the other side in the unified view: the old version, or the new one
// when the document is the left side.
const oldNumbers = gutter({
  class: "rm-old-numbers",
  lineMarker(view, block) {
    const model = view.state.field(modelField);
    const line = view.state.doc.lineAt(block.from).number - 1;
    const inside = model.hunks.some(
      (hunk) =>
        line >= sideStart(hunk, model.side) &&
        line < sideStart(hunk, model.side) + sideLines(hunk, model.side),
    );
    return inside ? null : new NumberMarker(mapLine(model.hunks, line, model.side));
  },
  widgetMarker: (_view, widget) =>
    widget instanceof OldLineWidget ? new NumberMarker(widget.line) : null,
  lineMarkerChange: (update) =>
    update.startState.field(modelField).hunks !== update.state.field(modelField).hunks,
  initialSpacer: () => new NumberMarker(9999),
});

// Soft wrap, in a compartment so that it changes without rebuilding the editors. Each state keeps
// its own value: one compartment serves every editor.
export const wrapping = new Compartment();

export const softWrap = (on: boolean): Extension => (on ? EditorView.lineWrapping : []);

// Equal number columns in the aligned diff, so that equal lines wrap at the same width.
const numberWidth = new Compartment();

// Every editor of a viewer wraps together, as SetEditorSettingsActionGroup applies the toggle to all.
export function setWrap(views: readonly EditorView[], on: boolean) {
  for (const view of views) {
    const wraps = wrapping.get(view.state) === EditorView.lineWrapping;
    if (wraps !== on) view.dispatch({ effects: wrapping.reconfigure(softWrap(on)) });
  }
}

function commonExtensions(
  handlers: Handlers,
  onexpand: (run: FoldRun) => void,
  wrap: boolean,
): Extension {
  return [
    // Its rule wins by specificity: one more class, `.cm-gutters`. Ahead of the theme, it also
    // comes later in the sheet, which jsdom needs because it ignores specificity.
    numberWidth.of([]),
    diffTheme,
    wrapping.of(softWrap(wrap)),
    handlersFacet.of(handlers),
    expandFacet.of(onexpand),
    lineNumbers(),
    highlightActiveLine(),
    highlightActiveLineGutter(),
    drawSelection(),
    EditorView.contentAttributes.of({ spellcheck: "false", autocorrect: "off" }),
    EditorView.domEventHandlers({ focus: () => handlers.onfocus() }),
    wordMarks,
  ];
}

function editableExtensions(editable: boolean, handlers: Handlers): Extension {
  if (!editable) return [EditorState.readOnly.of(true), keymap.of(defaultKeymap)];
  return [
    history(),
    keymap.of([...defaultKeymap, ...historyKeymap]),
    EditorView.updateListener.of((update) => {
      const typed = update.transactions.some(
        (transaction) =>
          transaction.docChanged && transaction.annotation(Transaction.addToHistory) !== false,
      );
      if (typed) handlers.onedit();
    }),
  ];
}

function foldsFor(
  data: DiffData,
  oldLines: number,
  newLines: number,
  options: EditorOptions,
  expanded: Set<number>,
) {
  if (!options.collapse) return [];
  return foldRuns(data.hunks, oldLines, newLines, CONTEXT_LINES).filter(
    (run) => !expanded.has(run.old),
  );
}

// Two editors side by side, or one in the unified view. Everything that marks a change
// (backgrounds, alignment blocks, folding, checkboxes) comes from git's hunks and not from a
// comparison of the browser's own: MergeView recomputes the diff and with repetitive text it picks
// other lines, so a checkbox would point to a different change than the one that is seen. Only
// `presentableDiff` is left from the library, and only inside a hunk, for the word highlighting.
export class DiffEditors {
  readonly dom: HTMLDivElement;
  private oldView: EditorView | null = null;
  // The right editor, or the only one in the unified view, whose document is `mainSide`.
  private newView: EditorView;
  private readonly mainSide: Side;
  private divider: SVGSVGElement | null = null;
  private saved: Text;
  private data: DiffData;
  private options: EditorOptions;
  private readonly expanded = new Set<number>();
  private readonly cleanups: (() => void)[] = [];
  private frame = 0;
  private numberDigits = 0;
  // The change stripe of each editor and the side its document is; none when embedded.
  private lanes: { lane: StripeLane; view: EditorView; side: Side }[] = [];

  constructor(
    parent: HTMLElement,
    private readonly config: {
      path: string;
      data: DiffData;
      draft: string | null;
      editable: boolean;
      // The side that is typed into: the local one, on the left when a comparison puts it there
      // (`MASTER_SIDE`). The unified view then shows the left text, with the right lines as blocks.
      editSide?: Side;
      unified: boolean;
      embedded: boolean;
      options: EditorOptions;
      handlers: Handlers;
    },
  ) {
    this.data = config.data;
    this.options = config.options;
    this.dom = parent.appendChild(document.createElement("div"));
    this.dom.className = `rm-editors${config.embedded ? " rm-embedded" : ""}`;
    const onexpand = (run: FoldRun) => this.expand(run);
    const common = [
      commonExtensions(config.handlers, onexpand, this.options.wrap),
      this.geometry,
      this.remeasure,
    ];
    const oldDoc = documentOf(config.data.oldText);
    const newDoc = documentOf(config.data.newText);
    const editsOld = config.editSide === "old";
    this.mainSide = config.unified && editsOld ? "old" : "new";
    const oldText = (editsOld ? config.draft : null) ?? config.data.oldText;
    const newText = (editsOld ? null : config.draft) ?? config.data.newText;
    this.saved = editsOld ? oldDoc : newDoc;
    const mainOld = this.mainSide === "old";
    const folds = foldsFor(this.data, oldDoc.lines, newDoc.lines, this.options, this.expanded);
    const model = (side: Side, other: Text): Model => ({
      side,
      unified: config.unified,
      hunks: this.data.hunks,
      selectable: this.data.selectable,
      selection: this.data.selection,
      hover: null,
      margin: config.draft === null,
      highlight: this.options.highlight,
      align: this.alignment(),
      folds: config.draft === null ? folds : [],
      other,
      stale: false,
      spacers: null,
    });
    const newHost = document.createElement("div");
    newHost.className = "rm-editor";
    if (!config.unified) {
      const oldHost = this.dom.appendChild(document.createElement("div"));
      oldHost.className = "rm-editor";
      this.oldView = new EditorView({
        parent: oldHost,
        state: EditorState.create({
          doc: oldText,
          extensions: [
            separatorExtension(config.data.oldText),
            common,
            editableExtensions(editsOld && config.editable, config.handlers),
            modelField.init(() => model("old", newDoc)),
            blocksField,
            gutterBackground,
            checkGutter,
            languageFor(config.path),
          ],
        }),
      });
      if (!this.alignment() && !config.embedded) {
        this.divider = this.dom.appendChild(
          document.createElementNS("http://www.w3.org/2000/svg", "svg"),
        );
        this.divider.classList.add("rm-divider");
      }
    }
    this.dom.append(newHost);
    this.newView = new EditorView({
      parent: newHost,
      state: EditorState.create({
        doc: mainOld ? oldText : newText,
        extensions: [
          separatorExtension(mainOld ? config.data.oldText : config.data.newText),
          common,
          editableExtensions((mainOld || !editsOld) && config.editable, config.handlers),
          modelField.init(() => (mainOld ? model("old", newDoc) : model("new", oldDoc))),
          blocksField,
          gutterBackground,
          config.unified ? oldNumbers : [],
          checkGutter,
          languageFor(config.path),
        ],
      }),
    });
    const oldView = this.oldView;
    if (oldView) {
      this.cleanups.push(
        connectScroll([oldView, this.newView], (source, driving) => {
          this.drawDivider();
          const target = source === oldView ? this.newView : oldView;
          if (driving && this.options.sync) this.syncScroll(source, target);
        }),
      );
    }
    if (!config.embedded) {
      if (this.oldView) this.lanes.push({ lane: new StripeLane(this.oldView, "left"), view: this.oldView, side: "old" });
      this.lanes.push({ lane: new StripeLane(this.newView, "right"), view: this.newView, side: this.mainSide });
    }
    this.drawDivider();
    this.drawStripes();
    this.equalNumbers();
    this.scheduleSpacers();
  }

  private readonly geometry = EditorView.updateListener.of((update) => {
    if (!update.geometryChanged) return;
    this.drawDivider();
    for (const { lane, view } of this.lanes ?? []) if (view === update.view) lane.redraw();
  });

  // `DiffDrawUtil`: a block with no background (`Do not highlight`) has no stripe mark either.
  private drawStripes() {
    for (const { lane, view, side } of this.lanes) {
      const hunks = this.options.highlight === "none" ? [] : this.data.hunks;
      lane.draw(stripeBlocks(hunks, side, this.config.unified, view.state.doc.lines));
    }
  }

  // Heights change when lines wrap anew or come into view and are measured. The spacers' own
  // transactions change heights too, but measuring text only, they would change nothing.
  private readonly remeasure = EditorView.updateListener.of((update) => {
    if (!update.heightChanged && !update.viewportChanged) return;
    const own = update.transactions.some((transaction) =>
      transaction.effects.some((effect) => effect.is(measuredSpacers)),
    );
    if (!own) this.scheduleSpacers();
  });

  private alignment(): boolean {
    return this.config.embedded || this.options.align;
  }

  // With soft wrap off, equal lines are equally tall and the spacers are counted, not measured.
  private measures(): boolean {
    return this.oldView !== null && this.alignment() && this.options.wrap;
  }

  // One pass for both editors per frame, never from inside an update.
  private scheduleSpacers() {
    if (this.frame || !this.measures()) return;
    this.frame = requestAnimationFrame(() => {
      this.frame = 0;
      this.measureSpacers();
    });
  }

  // AlignedDiffModel.updateInlayHeights with CodeMirror's heights: the tops of each boundary,
  // without the spacers each side already has, give the spacers that level them.
  private measureSpacers() {
    const left = this.oldView;
    const right = this.newView;
    const stale = left?.state.field(modelField).stale || right.state.field(modelField).stale;
    if (!left || !this.measures() || stale) return;
    const boundaries = boundariesOf(this.data.hunks, left.state.doc.lines, right.state.doc.lines);
    const spacers = alignSpacers(
      boundaries,
      textTops(left, boundaries.map((boundary) => boundary.left)),
      textTops(right, boundaries.map((boundary) => boundary.right)),
      SPACER_TOLERANCE,
    );
    for (const [view, side] of [
      [left, "left"],
      [right, "right"],
    ] as const) {
      // Even boundaries are the hunks' starts and the end: there the spacer levels unchanged text.
      const next = spacers.flatMap((spacer, index) =>
        spacer?.side === side
          ? [{ line: spacer.line, height: spacer.height, blank: index % 2 === 0 }]
          : [],
      );
      if (!sameSpacers(shownSpacers(view.state.field(modelField)), next)) {
        view.dispatch({ effects: measuredSpacers.of(next) });
      }
    }
  }

  private equalNumbers() {
    const left = this.oldView;
    const lines = left ? Math.max(left.state.doc.lines, this.newView.state.doc.lines) : 0;
    const digits = this.measures() ? String(lines).length : 0;
    if (digits === this.numberDigits) return;
    this.numberDigits = digits;
    const extension = digits ? numberColumn(digits) : [];
    for (const view of this.views()) {
      view.dispatch({ effects: numberWidth.reconfigure(extension) });
    }
  }

  private views(): EditorView[] {
    return this.oldView ? [this.oldView, this.newView] : [this.newView];
  }

  private push(change: (side: Side) => Partial<Model>) {
    for (const view of this.views()) {
      const { side } = view.state.field(modelField);
      view.dispatch({ effects: updateModel.of(change(side)) });
    }
    this.scheduleSpacers();
  }

  // Aligned, the two columns have the same height. Unaligned, each line goes with the one that
  // corresponds to it on the other side.
  private syncScroll(source: EditorView, target: EditorView) {
    if (!this.alignment()) {
      const { side } = source.state.field(modelField);
      followScroll(source, target, (line) => mapLine(this.data.hunks, line, side));
      return;
    }
    const next = source.scrollDOM.scrollTop;
    if (Math.abs(target.scrollDOM.scrollTop - next) > 1) target.scrollDOM.scrollTop = next;
  }

  private drawDivider() {
    const svg = this.divider;
    const left = this.oldView;
    if (!svg || !left) return;
    if (this.options.highlight === "none") {
      svg.innerHTML = "";
      return;
    }
    drawDivider(
      svg,
      left,
      this.newView,
      this.data.hunks.map((hunk) => ({
        left: [hunk.oldStart, hunk.oldStart + hunk.oldLines],
        right: [hunk.newStart, hunk.newStart + hunk.newLines],
        className: `rm-${changeType(hunk)}-band`,
      })),
    );
  }

  private refreshFolds() {
    const main = this.newView.state;
    const other = main.field(modelField).other.lines;
    const oldLines =
      this.oldView?.state.doc.lines ?? (this.mainSide === "old" ? main.doc.lines : other);
    const lines = this.mainSide === "old" ? other : main.doc.lines;
    const folds = this.dirty ? [] : foldsFor(this.data, oldLines, lines, this.options, this.expanded);
    this.push(() => ({ folds }));
    this.drawDivider();
  }

  private expand(run: FoldRun) {
    this.expanded.add(run.old);
    this.refreshFolds();
  }

  // The editor that is typed into.
  private get editView(): EditorView {
    return this.config.editSide === "old" && this.oldView ? this.oldView : this.newView;
  }

  get dirty(): boolean {
    return !this.editView.state.doc.eq(this.saved);
  }

  content(): string {
    return this.editView.state.sliceDoc();
  }

  // What is sent to disk: what was typed afterwards is not saved.
  snapshot(): Text {
    return this.editView.state.doc;
  }

  textOf(doc: Text): string {
    return doc.sliceString(0, doc.length, this.editView.state.lineBreak);
  }

  markSaved(doc: Text) {
    this.saved = doc;
  }

  // Whether this content fits in these editors: the line separator decides the numbering, and being
  // editable, whether it can be typed into. Neither of the two can be changed in an editor that
  // already exists.
  fits(data: DiffData, editable: boolean): boolean {
    return (
      editable === this.config.editable &&
      this.newView.state.lineBreak ===
        lineSeparator(this.mainSide === "old" ? data.oldText : data.newText) &&
      (!this.oldView || this.oldView.state.lineBreak === lineSeparator(data.oldText))
    );
  }

  focus() {
    this.editView.focus();
  }

  setSelection(selection: LineSelection[]) {
    this.data = { ...this.data, selection };
    this.push(() => ({ selection }));
  }

  // The hunks of what was typed, not yet saved. No checkboxes: the selection speaks of the file on
  // disk.
  setDraftHunks(hunks: Hunk[]) {
    const clean = !this.dirty;
    this.data = { ...this.data, hunks };
    this.push(() => ({ hunks, stale: false, margin: clean, folds: [] }));
    this.equalNumbers();
    this.drawDivider();
    this.drawStripes();
  }

  setOptions(options: EditorOptions) {
    if (options === this.options) return;
    this.options = options;
    setWrap(this.views(), options.wrap);
    const counted = this.measures() ? {} : { spacers: null };
    this.push(() => ({ highlight: options.highlight, align: this.alignment(), ...counted }));
    this.equalNumbers();
    this.refreshFolds();
    this.updateDivider();
    this.drawStripes();
  }

  // The separator between columns exists only when unaligned.
  private updateDivider() {
    if (!this.oldView) return;
    const needed = !this.alignment();
    if (needed && !this.divider) {
      this.divider = document.createElementNS("http://www.w3.org/2000/svg", "svg");
      this.divider.classList.add("rm-divider");
      this.dom.insertBefore(this.divider, this.newView.dom.parentElement);
    } else if (!needed && this.divider) {
      this.divider.remove();
      this.divider = null;
    }
    this.drawDivider();
  }

  // Replaces the content without losing the position: only what differs is replaced.
  setData(data: DiffData) {
    this.data = data;
    const mainOld = this.mainSide === "old";
    if (this.oldView) replaceText(this.oldView, data.oldText);
    replaceText(this.newView, mainOld ? data.oldText : data.newText);
    const oldDoc =
      this.oldView?.state.doc ?? (mainOld ? this.newView.state.doc : documentOf(data.oldText));
    const newDoc = mainOld ? documentOf(data.newText) : this.newView.state.doc;
    this.saved = this.editView.state.doc;
    const folds = foldsFor(data, oldDoc.lines, newDoc.lines, this.options, this.expanded);
    for (const view of this.views()) {
      const { side } = view.state.field(modelField);
      view.dispatch({
        effects: updateModel.of({
          hunks: data.hunks,
          selectable: data.selectable,
          selection: data.selection,
          margin: true,
          stale: false,
          folds,
          other: side === "old" ? newDoc : oldDoc,
        }),
      });
    }
    this.equalNumbers();
    this.scheduleSpacers();
    this.drawDivider();
    this.drawStripes();
  }

  // The lines chosen in the editor, or the cursor's line.
  selectedLines(view: EditorView = this.activeView()): LineRange {
    const { side } = view.state.field(modelField);
    const range = view.state.selection.main;
    const doc = view.state.doc;
    return {
      side,
      from: doc.lineAt(range.from).number - 1,
      to: doc.lineAt(range.empty ? range.from : range.to - 1).number - 1,
      exact: !range.empty,
    };
  }

  private activeView(): EditorView {
    return this.oldView?.hasFocus ? this.oldView : this.newView;
  }

  // The lines of a right click: if it falls inside the selection, the whole selection.
  linesAt(event: MouseEvent): LineRange | null {
    const view = this.views().find((candidate) => candidate.dom.contains(event.target as Node));
    if (!view) return null;
    const target = targetAt(view, event.clientY);
    if (!target) return null;
    if (target.side === view.state.field(modelField).side) {
      const selected = this.selectedLines(view);
      if (selected.exact && target.line >= selected.from && target.line <= selected.to) {
        return selected;
      }
    }
    return { side: target.side, from: target.line, to: target.line, exact: false };
  }

  // Jumps to the next or previous change; false if none is left in that direction.
  navigate(direction: 1 | -1): boolean {
    const view = this.activeView();
    const { side } = view.state.field(modelField);
    const doc = view.state.doc;
    const line = doc.lineAt(view.state.selection.main.head).number - 1;
    const starts = this.data.hunks.map((hunk) => sideStart(hunk, side));
    const target =
      direction > 0
        ? starts.find((start) => start > line)
        : starts.filter((start) => start < line).at(-1);
    if (target === undefined) return false;
    const position = lineFrom(doc, target);
    view.dispatch({
      selection: EditorSelection.cursor(position),
      effects: EditorView.scrollIntoView(position, { y: "center" }),
    });
    view.focus();
    return true;
  }

  // Moves the cursor to the first change when the file is opened.
  showFirstChange() {
    const first = this.data.hunks[0];
    if (!first) return;
    const position = lineFrom(this.newView.state.doc, sideStart(first, this.mainSide));
    this.newView.dispatch({
      selection: EditorSelection.cursor(position),
      effects: EditorView.scrollIntoView(position, { y: "center" }),
    });
  }

  destroy() {
    if (this.frame) cancelAnimationFrame(this.frame);
    this.cleanups.forEach((cleanup) => cleanup());
    this.lanes.forEach(({ lane }) => lane.destroy());
    this.oldView?.destroy();
    this.newView.destroy();
    this.dom.remove();
  }
}

export function lineTop(view: EditorView, line: number): number {
  return view.lineBlockAt(lineFrom(view.state.doc, line)).top;
}

// The top of each line without the view's spacers; past the last line, the end of the document.
function textTops(view: EditorView, lines: number[]): number[] {
  const doc = view.state.doc;
  const tops = lines.map((line) =>
    line >= doc.lines ? view.lineBlockAt(doc.length).bottom : lineTop(view, line),
  );
  return withoutSpacers(lines, tops, shownSpacers(view.state.field(modelField)), doc.lines);
}

// Each editor reports when it scrolls; `driving` says whether it is the one that moves the others:
// the one under the mouse or with the focus. Those that follow it also scroll, and must not drag it
// along.
export function connectScroll(
  views: EditorView[],
  onscroll: (source: EditorView, driving: boolean) => void,
): () => void {
  let driver: EditorView | null = null;
  const cleanups = views.map((source) => {
    const claim = () => (driver = source);
    const scroll = () => onscroll(source, driver === source);
    source.scrollDOM.addEventListener("scroll", scroll, { passive: true });
    source.dom.addEventListener("pointerenter", claim);
    source.dom.addEventListener("focusin", claim);
    return () => {
      source.scrollDOM.removeEventListener("scroll", scroll);
      source.dom.removeEventListener("pointerenter", claim);
      source.dom.removeEventListener("focusin", claim);
    };
  });
  return () => cleanups.forEach((cleanup) => cleanup());
}

// Brings `target` to the line that corresponds to the first visible one of `source`, interpolating
// within the line so the movement is continuous.
export function followScroll(
  source: EditorView,
  target: EditorView,
  map: (line: number) => number,
) {
  const scrolled = source.scrollDOM.scrollTop;
  const block = source.lineBlockAtHeight(scrolled);
  const line = source.state.doc.lineAt(block.from).number - 1;
  const fraction = block.height > 0 ? (scrolled - block.top) / block.height : 0;
  const top = lineTop(target, map(line));
  const next = top + fraction * (lineTop(target, map(line + 1)) - top);
  if (Math.abs(target.scrollDOM.scrollTop - next) > 1) target.scrollDOM.scrollTop = next;
}

// A stretch of lines on each side, `[from, to)`. `className` is fixed, never text from the file.
export interface Band {
  left: [number, number];
  right: [number, number];
  className: string;
}

// The bands that join each change with its place on the other side, like DiffDividerDrawUtil.
export function drawDivider(
  svg: SVGSVGElement,
  left: EditorView,
  right: EditorView,
  bands: Iterable<Band>,
) {
  const width = svg.clientWidth || 30;
  const height = svg.clientHeight;
  const middle = width / 2;
  const shapes: string[] = [];
  for (const band of bands) {
    const a1 = lineTop(left, band.left[0]) - left.scrollDOM.scrollTop;
    const a2 = lineTop(left, band.left[1]) - left.scrollDOM.scrollTop;
    const b1 = lineTop(right, band.right[0]) - right.scrollDOM.scrollTop;
    const b2 = lineTop(right, band.right[1]) - right.scrollDOM.scrollTop;
    if (Math.max(a2, b2) < 0 || Math.min(a1, b1) > height) continue;
    const shape =
      `M0 ${a1} C${middle} ${a1} ${middle} ${b1} ${width} ${b1} ` +
      `L${width} ${b2} C${middle} ${b2} ${middle} ${a2} 0 ${a2} Z`;
    shapes.push(`<path d="${shape}" class="${band.className}"/>`);
  }
  svg.innerHTML = shapes.join("");
}

function documentOf(text: string): Text {
  return EditorState.create({ doc: text, extensions: separatorExtension(text) }).doc;
}

// `Text.toString` joins with one character per break, just as the document positions count.
function replaceText(view: EditorView, text: string) {
  const replacement = documentOf(text);
  const before = view.state.doc.toString();
  const after = replacement.toString();
  if (before === after) return;
  const shortest = Math.min(before.length, after.length);
  let start = 0;
  while (start < shortest && before[start] === after[start]) start++;
  let end = 0;
  while (
    end < shortest - start &&
    before[before.length - 1 - end] === after[after.length - 1 - end]
  ) {
    end++;
  }
  view.dispatch({
    changes: {
      from: start,
      to: before.length - end,
      insert: replacement.slice(start, after.length - end),
    },
    annotations: Transaction.addToHistory.of(false),
  });
}
