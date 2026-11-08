import { defaultKeymap, history, historyKeymap, invertedEffects, isolateHistory } from "@codemirror/commands";
import { presentableDiff, type Change } from "@codemirror/merge";
import {
  Annotation,
  Compartment,
  EditorSelection,
  EditorState,
  Facet,
  RangeSet,
  StateField,
  Text,
  Transaction,
  type Extension,
  type Range,
} from "@codemirror/state";
import {
  Decoration,
  EditorView,
  GutterMarker,
  ViewPlugin,
  drawSelection,
  gutterLineClass,
  highlightActiveLine,
  highlightActiveLineGutter,
  keymap,
  lineNumbers,
  type DecorationSet,
  type ViewUpdate,
} from "@codemirror/view";
import { mapLine } from "../diff/blocks";
import {
  WORD_DIFF_MAX_CHARS,
  connectScroll,
  drawDivider,
  followScroll,
  setWrap,
  softWrap,
  wrapping,
  type Band,
} from "../diff/editor";
import { languageFor } from "../diff/language";
import { diffTheme } from "../diff/theme";
import type { Hunk, MergeBlock, Whitespace } from "../lib/api";
import { operationFacet, operationsGutter, type Operation } from "./gutter";
import {
  MergeEdit,
  baseFacet,
  canResolveAutomatically,
  counts,
  initialBlocks,
  isChange,
  isConflict,
  isResolved,
  isResolvedIn,
  linesContent,
  mergeCommand,
  paneFacet,
  rangeIn,
  setStates,
  statesField,
  type BlockState,
  type Counts,
  type MergePane,
  type MergeSide,
  type Texts,
} from "./model";

// Ported from MergeThreesideViewer: the two read-only sides and the result in the center, which
// starts with the text of the base and is changed with the actions of each block.

export interface MergeData {
  left: string;
  base: string;
  right: string;
  blocks: MergeBlock[];
  // Left, center and right.
  titles: [string, string, string];
  // Only for syntax highlighting.
  path?: string;
  // `Soft-Wrap` of the `Merge` place.
  wrap?: boolean;
}

export interface MergeHandlers {
  onchange: () => void;
  onallprocessed: () => void;
}

const SIDES: MergeSide[] = ["left", "right"];

// Loading the base or recalculating the blocks does not count as a change, like `setInitialOutputContent`.
const reset = Annotation.define<boolean>();

const textOf = (text: string) => Text.of(text.split("\n"));

// The center of each pane, for the word highlighting of the sides.
const peersFacet = Facet.define<() => Texts, () => Texts>({ combine: (values) => values[0] });

function buildLines(state: EditorState): DecorationSet {
  const pane = state.facet(paneFacet);
  const doc = state.doc;
  const ranges: Range<Decoration>[] = [];
  for (const block of state.field(statesField)) {
    if (!isChange(block, pane)) continue;
    const [from, to] = rangeIn(block, pane);
    const kind = block.block.kind;
    if (from === to) {
      const atEnd = from >= doc.lines;
      const line = doc.line(atEnd ? doc.lines : from + 1);
      ranges.push(Decoration.line({ class: `rm-gap${atEnd ? "-after" : ""}-${kind}` }).range(line.from));
      continue;
    }
    const resolved = isResolvedIn(block, pane);
    for (let line = from; line < to && line < doc.lines; line++) {
      let className = `rm-${kind}`;
      if (resolved) {
        className += " rm-resolved";
        if (line === from) className += " rm-first";
        if (line === to - 1) className += " rm-last";
      }
      ranges.push(Decoration.line({ class: className }).range(doc.line(line + 1).from));
    }
  }
  return Decoration.set(ranges, true);
}

const linesField = StateField.define<DecorationSet>({
  create: buildLines,
  update(lines, transaction) {
    if (transaction.startState.field(statesField) !== transaction.state.field(statesField)) {
      return buildLines(transaction.state);
    }
    return lines.map(transaction.changes);
  },
  provide: (field) => EditorView.decorations.from(field),
});

class BackgroundMarker extends GutterMarker {
  constructor(readonly elementClass: string) {
    super();
  }
}

// The color of what is pending continues along the margin, like the LineMarkerRenderer of DiffDrawUtil.
const gutterBackground = gutterLineClass.compute([statesField, "doc"], (state) => {
  const pane = state.facet(paneFacet);
  const ranges: Range<GutterMarker>[] = [];
  for (const block of state.field(statesField)) {
    if (!isChange(block, pane) || isResolvedIn(block, pane)) continue;
    const [from, to] = rangeIn(block, pane);
    const marker = new BackgroundMarker(`rm-${block.block.kind}`);
    for (let line = from; line < to && line < state.doc.lines; line++) {
      ranges.push(marker.range(state.doc.line(line + 1).from));
    }
  }
  return RangeSet.of(ranges, true);
});

const WORD_CACHE_SIZE = 64;
const wordCache = new Map<string, readonly Change[] | null>();

function wordChanges(centre: string, side: string): readonly Change[] | null {
  const key = `${centre}\u0000${side}`;
  const known = wordCache.get(key);
  if (known !== undefined) return known;
  const changes =
    centre.length + side.length > WORD_DIFF_MAX_CHARS
      ? null
      : presentableDiff(centre, side, { scanLimit: 500 });
  if (wordCache.size >= WORD_CACHE_SIZE) wordCache.delete(wordCache.keys().next().value as string);
  wordCache.set(key, changes);
  return changes;
}

// The words of each pending block against the center, like MyInnerDiffWorker: on a side if
// that side changed and is still pending, and in the center against each of those sides. Only what is
// visible: it is the only expensive thing to paint.
const wordMarks = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    constructor(view: EditorView) {
      this.decorations = this.build(view);
    }
    update(update: ViewUpdate) {
      const changed = update.startState.field(statesField) !== update.state.field(statesField);
      if (update.docChanged || update.viewportChanged || changed) {
        this.decorations = this.build(update.view);
      }
    }
    build(view: EditorView) {
      const pane = view.state.facet(paneFacet);
      const doc = view.state.doc;
      const peers = view.state.facet(peersFacet)();
      const centre = pane === "base" ? doc : peers.base;
      const { from: top, to: bottom } = view.viewport;
      const ranges: Range<Decoration>[] = [];
      for (const state of view.state.field(statesField)) {
        if (isResolved(state)) continue;
        const [from, to] = rangeIn(state, pane);
        if (from === to || to > doc.lines) continue;
        const start = doc.line(from + 1).from;
        if (doc.line(to).to < top || start > bottom) continue;
        let marked = false;
        for (const side of SIDES) {
          if ((pane !== "base" && pane !== side) || !isChange(state, side) || state[side]) continue;
          const [sideFrom, sideTo] = state.block[side];
          const output = state.output;
          if (sideFrom === sideTo || output.from === output.to) continue;
          const changes = wordChanges(
            linesContent(centre, output.from, output.to),
            linesContent(peers[side], sideFrom, sideTo),
          );
          if (!changes) continue;
          marked = true;
          for (const change of changes) {
            const [markFrom, markTo] =
              pane === "base" ? [change.fromA, change.toA] : [change.fromB, change.toB];
            if (markFrom === markTo) continue;
            ranges.push(
              Decoration.mark({ class: `rm-word-${state.block.kind}` }).range(
                start + markFrom,
                start + markTo,
              ),
            );
          }
        }
        if (!marked) continue;
        for (let line = from; line < to; line++) {
          ranges.push(Decoration.line({ class: "rm-words" }).range(doc.line(line + 1).from));
        }
      }
      return Decoration.set(ranges, true);
    }
  },
  { decorations: (plugin) => plugin.decorations },
);

// Each state that changes keeps the previous one for undo, and each command keeps it even if nothing changes,
// like `registerUndoRedo`: undo goes command by command.
const undoStates = invertedEffects.of((transaction) => {
  const before = transaction.startState.field(statesField);
  const stated = transaction.effects.some((effect) => effect.is(setStates));
  if (!stated && before === transaction.state.field(statesField)) return [];
  return [setStates.of(before)];
});

const pending = (states: readonly BlockState[]) => states.some((state) => !isResolved(state));

const SIDE_TO_BASE = new WeakMap<readonly BlockState[], Record<MergeSide, Hunk[]>>();

// The blocks as hunks of each side against the center, for `mapLine`.
function hunksOf(states: readonly BlockState[], side: MergeSide): Hunk[] {
  let known = SIDE_TO_BASE.get(states);
  if (!known) {
    const of = (which: MergeSide) =>
      states.map((state) => ({
        id: "",
        oldStart: state.block[which][0],
        oldLines: state.block[which][1] - state.block[which][0],
        newStart: state.output.from,
        newLines: state.output.to - state.output.from,
      }));
    known = { left: of("left"), right: of("right") };
    SIDE_TO_BASE.set(states, known);
  }
  return known[side];
}

// What is common to the three panes: that of MergeThreesideViewer and that of SimpleThreesideDiffViewer. The
// titles, three editors with two dividers, the scroll that is followed from one to another and the
// bands of each divider, which are repainted when any of them moves.
export interface ThreePanesConfig {
  className: string;
  titles: [string, string, string];
  docs: [string, string, string];
  // What is specific to each pane, in the order left, center and right.
  extensions: [Extension, Extension, Extension];
  // The line of a pane that corresponds to one of another.
  mapLine: (from: MergePane, to: MergePane, line: number) => number;
  // The bands of the divider on that side of the center.
  bands: (side: MergeSide) => Band[];
  // Whether scrolling one scrolls the others.
  synced?: () => boolean;
  // `Soft-Wrap`, for the three at once.
  wrap?: boolean;
}

const PANES: MergePane[] = ["left", "base", "right"];

export class ThreePanes {
  readonly dom: HTMLDivElement;
  readonly views: EditorView[] = [];
  private readonly titles: HTMLDivElement[] = [];
  private readonly dividers: SVGSVGElement[] = [];
  private readonly cleanups: (() => void)[] = [];
  // getCurrentEditor: the last pane with focus, even if the button bar has it now.
  current: EditorView | null = null;

  constructor(
    parent: HTMLElement,
    private readonly config: ThreePanesConfig,
  ) {
    this.dom = parent.appendChild(document.createElement("div"));
    this.dom.className = config.className;
    const titles = this.dom.appendChild(document.createElement("div"));
    titles.className = "rm-merge-titles";
    config.titles.forEach((title, index) => {
      if (index > 0) titles.appendChild(document.createElement("div")).className = "rm-merge-title-gap";
      const element = titles.appendChild(document.createElement("div"));
      element.className = "rm-merge-title";
      element.textContent = title;
      this.titles.push(element);
    });
    const row = this.dom.appendChild(document.createElement("div"));
    row.className = "rm-editors";
    PANES.forEach((pane, index) => {
      if (index > 0) {
        const divider = row.appendChild(document.createElementNS("http://www.w3.org/2000/svg", "svg"));
        divider.classList.add("rm-divider");
        this.dividers.push(divider);
      }
      const host = row.appendChild(document.createElement("div"));
      host.className = "rm-editor";
      host.dataset.pane = pane;
      this.views.push(
        new EditorView({
          parent: host,
          state: EditorState.create({
            doc: config.docs[index],
            extensions: [
              paneFacet.of(pane),
              EditorView.domEventHandlers({
                focus: (_event, view) => {
                  this.current = view;
                },
              }),
              EditorView.updateListener.of((update) => {
                if (update.geometryChanged) this.drawDividers();
              }),
              wrapping.of(softWrap(config.wrap ?? false)),
              config.extensions[index],
            ],
          }),
        }),
      );
    });
    this.cleanups.push(
      connectScroll(this.views, (source, driving) => {
        this.drawDividers();
        if (!driving || !(config.synced?.() ?? true)) return;
        const from = source.state.facet(paneFacet);
        for (const target of this.views) {
          if (target === source) continue;
          const to = target.state.facet(paneFacet);
          followScroll(source, target, (line) => config.mapLine(from, to, line));
        }
      }),
    );
    this.drawDividers();
  }

  view(pane: MergePane): EditorView {
    return this.views[PANES.indexOf(pane)];
  }

  // The three wrap together, without being rebuilt.
  setWrap(on: boolean) {
    setWrap(this.views, on);
  }

  setTitle(pane: MergePane, title: string) {
    this.titles[PANES.indexOf(pane)].textContent = title;
  }

  // MyDividerPaintable of each side of the center.
  drawDividers() {
    if (this.views.length < PANES.length) return;
    const [left, centre, right] = this.views;
    drawDivider(this.dividers[0], left, centre, this.config.bands("left"));
    drawDivider(this.dividers[1], centre, right, this.config.bands("right"));
  }

  destroy() {
    this.cleanups.forEach((cleanup) => cleanup());
    this.views.forEach((view) => view.destroy());
    this.dom.remove();
  }
}

export class MergeEditors {
  readonly dom: HTMLDivElement;
  private readonly panes: ThreePanes;
  private readonly left: EditorView;
  private readonly centre: EditorView;
  private readonly right: EditorView;
  private readonly texts: Texts;
  private readonly historySlot = new Compartment();
  private policy: Whitespace = "default";
  // `myContentModified`: the center changed since it was loaded.
  private changed = false;

  constructor(
    parent: HTMLElement,
    data: MergeData,
    private readonly handlers: MergeHandlers,
  ) {
    this.texts = { left: textOf(data.left), base: textOf(data.base), right: textOf(data.right) };
    const states = initialBlocks(data.blocks);
    const language = data.path ? languageFor(data.path) : [];
    const readOnly = [EditorState.readOnly.of(true), keymap.of(defaultKeymap)];
    const pane = (extensions: Extension) => [this.common(states), language, extensions];
    this.panes = new ThreePanes(parent, {
      className: "rm-merge",
      titles: data.titles,
      docs: [data.left, data.base, data.right],
      extensions: [
        pane(readOnly),
        pane([
          baseFacet.of(this.texts.base),
          this.historySlot.of(history()),
          undoStates,
          keymap.of([...defaultKeymap, ...historyKeymap]),
          EditorView.updateListener.of((update) => this.onupdate(update)),
        ]),
        pane(readOnly),
      ],
      mapLine: (from, to, line) => this.mapLine(from, to, line),
      bands: (side) => this.bands(side),
      wrap: data.wrap,
    });
    this.dom = this.panes.dom;
    [this.left, this.centre, this.right] = this.panes.views;
    this.panes.drawDividers();
  }

  private common(states: readonly BlockState[]): Extension {
    return [
      EditorState.lineSeparator.of("\n"),
      diffTheme,
      statesField.init(() => states),
      linesField,
      lineNumbers(),
      operationsGutter,
      gutterBackground,
      highlightActiveLine(),
      highlightActiveLineGutter(),
      drawSelection(),
      EditorView.contentAttributes.of({ spellcheck: "false", autocorrect: "off" }),
      peersFacet.of(() => ({
        left: this.texts.left,
        base: this.centre?.state.doc ?? this.texts.base,
        right: this.texts.right,
      })),
      wordMarks,
      operationFacet.of((operation, modifier) => this.perform(operation, modifier)),
    ];
  }

  private perform(operation: Operation, modifier: boolean) {
    if (operation.action === "resolve") this.resolve(operation.index);
    else if (operation.action === "apply") this.apply(operation.index, operation.side!, modifier);
    else this.ignore(operation.index, operation.side!, modifier);
  }

  private onupdate(update: ViewUpdate) {
    const before = update.startState.field(statesField);
    const after = update.state.field(statesField);
    const loading = update.transactions.some((transaction) => transaction.annotation(reset));
    if (update.docChanged && !loading) this.changed = true;
    if (before !== after) {
      for (const side of [this.left, this.right]) side.dispatch({ effects: setStates.of(after) });
    }
    if (before === after && !update.docChanged) return;
    this.panes?.drawDividers();
    this.handlers.onchange();
    // onChangeResolved: when everything is processed. Recalculating the blocks resolves none.
    if (!loading && pending(before) && !pending(after)) this.handlers.onallprocessed();
  }

  // A command with its name, which is undone at once.
  private run(name: string, action: (edit: MergeEdit) => void) {
    const state = this.centre.state;
    const edit = new MergeEdit(state.doc, state.field(statesField), this.texts);
    action(edit);
    this.changed = true;
    this.centre.dispatch({
      changes: edit.changes,
      effects: setStates.of(edit.states),
      annotations: [mergeCommand.of(name), isolateHistory.of("full")],
    });
  }

  // The line of a pane that corresponds to one of another, going through the center.
  private mapLine(from: MergePane, to: MergePane, line: number): number {
    const states = this.states();
    const base = from === "base" ? line : mapLine(hunksOf(states, from), line, "old");
    return to === "base" ? base : mapLine(hunksOf(states, to), base, "new");
  }

  // MyDividerPaintable: each block that changes on that side, only the border if it was already resolved.
  private bands(side: MergeSide): Band[] {
    if (!this.centre) return [];
    return this.states()
      .filter((state) => isChange(state, side))
      .map((state) => {
        const output: [number, number] = [state.output.from, state.output.to];
        const className = `rm-${state.block.kind}-band${state[side] ? " rm-resolved-band" : ""}`;
        return side === "left"
          ? { left: state.block.left, right: output, className }
          : { left: output, right: state.block.right, className };
      });
  }

  // doScrollToChange: the cursor of each pane at the start of the block.
  private show(index: number, focus?: EditorView) {
    const state = this.states()[index];
    for (const view of [this.left, this.centre, this.right]) {
      const doc = view.state.doc;
      const line = rangeIn(state, view.state.facet(paneFacet))[0];
      const position = line < doc.lines ? doc.line(line + 1).from : doc.length;
      view.dispatch({
        selection: EditorSelection.cursor(position),
        effects: EditorView.scrollIntoView(position, { y: "center" }),
      });
    }
    focus?.focus();
  }

  private showFirstPending() {
    const index = this.states().findIndex((state) => !isResolved(state));
    if (index >= 0) this.show(index);
  }

  states(): readonly BlockState[] {
    return this.centre.state.field(statesField);
  }

  get whitespace(): Whitespace {
    return this.policy;
  }

  // TextMergeChange: the margin arrow. `whole` is the Ctrl click.
  apply(index: number, side: MergeSide, whole: boolean) {
    this.run("Accept change", (edit) => edit.replace(index, side, whole));
  }

  // TextMergeChange: the margin cross.
  ignore(index: number, side: MergeSide, whole: boolean) {
    this.run("Ignore change", (edit) => edit.ignore(index, side, whole));
  }

  // TextMergeChange: the margin wand.
  resolve(index: number) {
    this.run("Resolve conflict", (edit) => edit.resolve(index, "base"));
  }

  // hasNonConflictedChanges: what enables `Left`, `All` and `Right`.
  hasNonConflicting(which: "left" | "all" | "right"): boolean {
    const pane: MergePane = which === "all" ? "base" : which;
    return this.states().some((state) => !isConflict(state) && canResolveAutomatically(state, pane));
  }

  // hasResolvableConflictedChanges: what enables `Resolve Automatically`.
  hasResolvable(): boolean {
    return this.states().some((state) => canResolveAutomatically(state, "base"));
  }

  // ApplyNonConflictsAction. `all` takes each change from the side that has it, the left one if it is
  // both.
  applyNonConflicting(which: "left" | "all" | "right") {
    if (!this.hasNonConflicting(which)) return;
    const pane: MergePane = which === "all" ? "base" : which;
    this.run("Apply non conflicted changes", (edit) => {
      edit.states.forEach((state, index) => {
        if (!isConflict(state)) edit.resolve(index, pane);
      });
    });
    this.showFirstPending();
  }

  // MagicResolvedConflictsAction: everything that resolves by itself from the center, also what is not
  // a conflict.
  resolveAutomatically() {
    if (!this.hasResolvable()) return;
    this.run("Resolve simple conflicted changes", (edit) => {
      edit.states.forEach((_state, index) => edit.resolve(index, "base"));
    });
    this.showFirstPending();
  }

  // Jumps to the next or previous pending block of the last pane with focus; false if there is none.
  navigate(direction: 1 | -1): boolean {
    const view = this.panes.current ?? this.centre;
    const pane = view.state.facet(paneFacet);
    const line = view.state.doc.lineAt(view.state.selection.main.head).number - 1;
    const starts = this.states()
      .map((state, index) => ({ state, index, start: rangeIn(state, pane)[0] }))
      .filter(({ state }) => !isResolved(state));
    const target =
      direction > 0
        ? starts.find(({ start }) => start > line)
        : starts.filter(({ start }) => start < line).at(-1);
    if (!target) return false;
    this.show(target.index, view);
    return true;
  }

  result(): string {
    return this.centre.state.doc.toString();
  }

  modified(): boolean {
    return this.changed;
  }

  counts(): Counts {
    return counts(this.states());
  }

  // Another whitespace policy brings other blocks: the center goes back to the base without being able to undo it,
  // like the `doRediff` of MergeThreesideViewer.
  setWhitespace(policy: Whitespace, blocks: MergeBlock[]) {
    this.policy = policy;
    const doc = this.centre.state.doc;
    this.centre.dispatch({ effects: this.historySlot.reconfigure([]) });
    this.centre.dispatch({
      changes: doc.eq(this.texts.base) ? [] : { from: 0, to: doc.length, insert: this.texts.base },
      effects: setStates.of(initialBlocks(blocks)),
      annotations: [reset.of(true), Transaction.addToHistory.of(false)],
    });
    this.centre.dispatch({ effects: this.historySlot.reconfigure(history()) });
    this.changed = false;
  }

  setWrap(on: boolean) {
    this.panes.setWrap(on);
  }

  destroy() {
    this.panes.destroy();
  }
}
