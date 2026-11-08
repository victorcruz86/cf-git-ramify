<script module lang="ts">
  import { defaultKeymap, history, historyKeymap, isolateHistory } from "@codemirror/commands";
  import {
    Annotation,
    EditorSelection,
    EditorState,
    Facet,
    RangeSet,
    StateEffect,
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
    drawSelection,
    gutterLineClass,
    highlightActiveLine,
    highlightActiveLineGutter,
    keymap,
    lineNumbers,
    type DecorationSet,
    type ViewUpdate,
  } from "@codemirror/view";
  import type { Hunk, MergeBlock, MergeBlockKind, ThreeSides, Whitespace } from "../lib/api";
  import { ThreePanes } from "../merge/editors";
  import { actionsGutter, type GutterAction } from "../merge/gutter";
  import {
    lineChanges,
    linesModification,
    linesOf,
    paneFacet,
    updateRangeOnModification,
    type MergePane,
    type MergeSide,
  } from "../merge/model";
  import { mapLine, type IgnorePolicy } from "./blocks";
  import type { Band } from "./editor";
  import { languageFor, lineSeparator, separatorExtension } from "./language";
  import { diffTheme } from "./theme";

  // Ported from GitStageDiffUtil.compareThreeVersions and SimpleThreesideDiffViewer: read-only HEAD
  // on the left, the stage in the center and the disk on the right, both editable.

  const SEPARATOR = " -> ";

  // DiffRequestFactoryImpl.getContentTitle, with the repository path as `presentableUrl`.
  function contentTitle(name: string, path: string, parent: string | null): string {
    return parent !== null ? `${name} (${parent})` : path;
  }

  function split(path: string): { name: string; parent: string | null } {
    const slash = path.lastIndexOf("/");
    return slash < 0
      ? { name: path, parent: null }
      : { name: path.slice(slash + 1), parent: path.slice(0, slash) };
  }

  // DiffRequestFactoryImpl.getTitle(status.path, status.origPath, DIFF_TITLE_RENAME_SEPARATOR).
  export function requestTitle(path: string, oldPath: string | null): string {
    const first = split(path);
    if (oldPath === null || oldPath === path) return contentTitle(first.name, path, first.parent);
    const second = split(oldPath);
    if (first.parent === second.parent) {
      return first.parent !== null
        ? `${first.name}${SEPARATOR}${second.name} (${first.parent})`
        : `${path}${SEPARATOR}${oldPath}`;
    }
    if (first.parent === null || second.parent === null) return `${path}${SEPARATOR}${oldPath}`;
    const parents = `(${first.parent}${SEPARATOR}${second.parent})`;
    return first.name === second.name
      ? `${first.name} ${parents}`
      : `${first.name}${SEPARATOR}${second.name} ${parents}`;
  }

  // The whitespace policy of the toolbar in the one of the three-way diff.
  export function policyOf(ignore: IgnorePolicy): Whitespace {
    if (ignore === "none") return "default";
    return ignore === "trim" ? "trim" : "ignore";
  }

  type Ranges = Record<MergePane, [number, number]>;

  // SimpleThreesideDiffChange: the ranges follow the text of each pane, and one that an edit breaks
  // stops being valid (no arrows or bands) until the next diff.
  interface Change {
    ranges: Ranges;
    kind: MergeBlockKind;
    leftChanged: boolean;
    rightChanged: boolean;
    valid: boolean;
  }

  // MergeRangeUtil.getLineLeftToRightThreeSideDiffType: THREESIDE_DIFF_COLORS_MODE is
  // LEFT_TO_RIGHT, and from HEAD to the disk there are no conflicts.
  function leftToRightKind(block: MergeBlock): MergeBlockKind {
    const empty = (range: [number, number]) => range[0] === range[1];
    const [left, base, right] = [empty(block.left), empty(block.base), empty(block.right)];
    if (base) {
      if (left) return "inserted";
      return right ? "deleted" : "modified";
    }
    if (left && right) return "modified";
    if (!block.leftChanged) return right ? "deleted" : "modified";
    if (!block.rightChanged) return left ? "inserted" : "modified";
    return "modified";
  }

  function changesOf(blocks: MergeBlock[]): Change[] {
    return blocks.map((block) => ({
      ranges: { left: block.left, base: block.base, right: block.right },
      kind: leftToRightKind(block),
      leftChanged: block.leftChanged,
      rightChanged: block.rightChanged,
      valid: true,
    }));
  }

  // MergeConflictType.isChange: the center is always part of the change.
  function changedIn(change: Change, pane: MergePane): boolean {
    if (pane === "base") return true;
    return pane === "left" ? change.leftChanged : change.rightChanged;
  }

  const setChanges = StateEffect.define<readonly Change[]>();
  // What arrives from another pane is not sent back; what is loaded does not count as an edit.
  const fromPeer = Annotation.define<boolean>();
  const loaded = Annotation.define<boolean>();

  // SimpleThreesideDiffViewer.onBeforeDocumentChange, for the pane that changes.
  function mapChanges(
    changes: readonly Change[],
    pane: MergePane,
    transaction: Transaction,
  ): readonly Change[] {
    if (changes.length === 0) return changes;
    let current = changes;
    const edits = lineChanges(transaction.changes, transaction.startState.doc, transaction.state.doc);
    for (const { start, end, shift } of edits) {
      current = current.map((change) => {
        const [from, to] = change.ranges[pane];
        const range = updateRangeOnModification(from, to, start, end, shift);
        if (range.from === from && range.to === to && !range.damaged) return change;
        return {
          ...change,
          ranges: { ...change.ranges, [pane]: [range.from, range.to] },
          valid: change.valid && !range.damaged,
        };
      });
    }
    return current;
  }

  const changesField = StateField.define<readonly Change[]>({
    create: () => [],
    update(changes, transaction) {
      const given = transaction.effects.filter((effect) => effect.is(setChanges)).at(-1);
      if (given) return given.value as readonly Change[];
      if (!transaction.docChanged) return changes;
      return mapChanges(changes, transaction.state.facet(paneFacet), transaction);
    },
  });

  function buildLines(state: EditorState): DecorationSet {
    const pane = state.facet(paneFacet);
    const doc = state.doc;
    const ranges: Range<Decoration>[] = [];
    for (const change of state.field(changesField)) {
      if (!changedIn(change, pane)) continue;
      const [from, to] = change.ranges[pane];
      if (from === to) {
        const atEnd = from >= doc.lines;
        const line = doc.line(atEnd ? doc.lines : from + 1);
        const className = `rm-gap${atEnd ? "-after" : ""}-${change.kind}`;
        ranges.push(Decoration.line({ class: className }).range(line.from));
        continue;
      }
      for (let line = from; line < to && line < doc.lines; line++) {
        ranges.push(Decoration.line({ class: `rm-${change.kind}` }).range(doc.line(line + 1).from));
      }
    }
    return Decoration.set(ranges, true);
  }

  const linesField = StateField.define<DecorationSet>({
    create: buildLines,
    update(lines, transaction) {
      if (transaction.startState.field(changesField) !== transaction.state.field(changesField)) {
        return buildLines(transaction.state);
      }
      return lines.map(transaction.changes);
    },
    provide: (field) => EditorView.decorations.from(field),
  });

  class BackgroundMarker extends GutterMarker {
    readonly elementClass: string;
    constructor(elementClass: string) {
      super();
      this.elementClass = elementClass;
    }
  }

  const gutterBackground = gutterLineClass.compute([changesField, "doc"], (state) => {
    const pane = state.facet(paneFacet);
    const ranges: Range<GutterMarker>[] = [];
    for (const change of state.field(changesField)) {
      if (!changedIn(change, pane)) continue;
      const [from, to] = change.ranges[pane];
      const marker = new BackgroundMarker(`rm-${change.kind}`);
      for (let line = from; line < to && line < state.doc.lines; line++) {
        ranges.push(marker.range(state.doc.line(line + 1).from));
      }
    }
    return RangeSet.of(ranges, true);
  });

  export type VersionActionName = "stage" | "unstage" | "revert";

  export interface VersionAction extends GutterAction {
    action: VersionActionName;
    side: null;
  }

  // SimpleThreesideDiffChange.createAcceptOperation with the texts of compareThreeVersions. HEAD is
  // not edited: of the four, three remain, each one in the pane the lines come from.
  function actionsFor(
    changes: readonly Change[],
    pane: MergePane,
    lines: number,
  ): Map<number, VersionAction[]> {
    const result = new Map<number, VersionAction[]>();
    changes.forEach((change, index) => {
      if (!change.valid) return;
      let action: VersionAction | null = null;
      if (pane === "left" && change.leftChanged) {
        action = { action: "unstage", index, side: null, icon: "arrowRight", tooltip: "Unstage" };
      } else if (pane === "base" && change.rightChanged) {
        action = { action: "revert", index, side: null, icon: "arrowRight", tooltip: "Revert" };
      } else if (pane === "right" && change.rightChanged) {
        action = { action: "stage", index, side: null, icon: "arrow", tooltip: "Stage" };
      }
      if (!action) return;
      const line = Math.min(change.ranges[pane][0], lines - 1);
      result.set(line, [...(result.get(line) ?? []), action]);
    });
    return result;
  }

  const marginCache = new WeakMap<readonly Change[], Map<string, Map<number, VersionAction[]>>>();

  function marginOf(view: EditorView): Map<number, VersionAction[]> {
    const changes = view.state.field(changesField);
    const pane = view.state.facet(paneFacet);
    let panes = marginCache.get(changes);
    if (!panes) {
      panes = new Map();
      marginCache.set(changes, panes);
    }
    const key = `${pane}:${view.state.doc.lines}`;
    let margin = panes.get(key);
    if (!margin) {
      margin = actionsFor(changes, pane, view.state.doc.lines);
      panes.set(key, margin);
    }
    return margin;
  }

  type ActionHandler = (action: VersionAction) => void;

  const actionFacet = Facet.define<ActionHandler, ActionHandler>({
    combine: (values) => values[0],
  });

  const versionsGutter = actionsGutter<VersionAction>({
    margin: marginOf,
    changed: (update) => update.startState.field(changesField) !== update.state.field(changesField),
    run: (view, action) => view.state.facet(actionFacet)(action),
  });

  // From where to where each arrow copies.
  const DIRECTIONS: Record<VersionActionName, [MergePane, EditablePane]> = {
    stage: ["right", "base"],
    unstage: ["left", "base"],
    revert: ["base", "right"],
  };

  export type EditablePane = "base" | "right";

  function documentOf(text: string): Text {
    return EditorState.create({ doc: text, extensions: separatorExtension(text) }).doc;
  }

  // What was loaded anew, changing only what differs so the cursor and the scroll position are not
  // lost.
  function replaceText(view: EditorView, text: string) {
    const before = view.state.doc.toString();
    const after = documentOf(text).toString();
    if (before === after) return;
    const shortest = Math.min(before.length, after.length);
    let start = 0;
    while (start < shortest && before[start] === after[start]) start++;
    let end = 0;
    while (end < shortest - start && before[before.length - 1 - end] === after[after.length - 1 - end]) {
      end++;
    }
    view.dispatch({
      changes: {
        from: start,
        to: before.length - end,
        insert: Text.of(after.slice(start, after.length - end).split("\n")),
      },
      annotations: [loaded.of(true), Transaction.addToHistory.of(false)],
    });
  }

  interface VersionConfig {
    path: string;
    sides: ThreeSides;
    blocks: MergeBlock[];
    synced: () => boolean;
    wrap: boolean;
    onedit: () => void;
    onaction: (action: VersionAction) => void;
  }

  export class VersionEditors {
    private readonly panes: ThreePanes;
    private readonly saved: Record<EditablePane, Text>;
    private readonly config: VersionConfig;

    constructor(parent: HTMLElement, config: VersionConfig) {
      this.config = config;
      const changes = changesOf(config.blocks);
      const { head, staged, local } = config.sides;
      const common = (text: string): Extension => [
        separatorExtension(text),
        diffTheme,
        changesField.init(() => changes),
        linesField,
        lineNumbers(),
        versionsGutter,
        gutterBackground,
        highlightActiveLine(),
        highlightActiveLineGutter(),
        drawSelection(),
        EditorView.contentAttributes.of({ spellcheck: "false", autocorrect: "off" }),
        actionFacet.of(config.onaction),
        languageFor(config.path),
        EditorView.updateListener.of((update) => this.onupdate(update)),
      ];
      const editable = [history(), keymap.of([...defaultKeymap, ...historyKeymap])];
      this.panes = new ThreePanes(parent, {
        className: "rm-merge",
        titles: ["HEAD", "Staged", "Local"],
        docs: [head, staged, local],
        extensions: [
          [common(head), EditorState.readOnly.of(true), keymap.of(defaultKeymap)],
          [common(staged), editable],
          [common(local), editable],
        ],
        mapLine: (from, to, line) => this.mapLine(from, to, line),
        bands: (side) => this.bands(side),
        synced: config.synced,
        wrap: config.wrap,
      });
      this.saved = {
        base: this.panes.view("base").state.doc,
        right: this.panes.view("right").state.doc,
      };
      this.panes.drawDividers();
    }

    get dom(): HTMLElement {
      return this.panes.dom;
    }

    view(pane: MergePane): EditorView {
      return this.panes.view(pane);
    }

    changes(): readonly Change[] {
      return this.panes?.view("base")?.state.field(changesField) ?? [];
    }

    private onupdate(update: ViewUpdate) {
      const before = update.startState.field(changesField);
      const after = update.state.field(changesField);
      const peer = update.transactions.some((transaction) => transaction.annotation(fromPeer));
      if (before !== after && !peer && this.panes) {
        for (const view of this.panes.views) {
          if (view !== update.view) view.dispatch({ effects: setChanges.of(after), annotations: fromPeer.of(true) });
        }
      }
      const reloaded = update.transactions.some((transaction) => transaction.annotation(loaded));
      if (update.docChanged && !reloaded) this.config.onedit();
      if (before !== after || update.docChanged) this.panes?.drawDividers();
    }

    private hunks(side: MergeSide): Hunk[] {
      return this.changes().map((change) => ({
        id: "",
        oldStart: change.ranges[side][0],
        oldLines: change.ranges[side][1] - change.ranges[side][0],
        newStart: change.ranges.base[0],
        newLines: change.ranges.base[1] - change.ranges.base[0],
      }));
    }

    private mapLine(from: MergePane, to: MergePane, line: number): number {
      const base = from === "base" ? line : mapLine(this.hunks(from), line, "old");
      return to === "base" ? base : mapLine(this.hunks(to), base, "new");
    }

    // MyDividerPaintable: the valid changes of that side.
    private bands(side: MergeSide): Band[] {
      return this.changes()
        .filter((change) => change.valid && changedIn(change, side))
        .map((change) => {
          const className = `rm-${change.kind}-band`;
          return side === "left"
            ? { left: change.ranges.left, right: change.ranges.base, className }
            : { left: change.ranges.base, right: change.ranges.right, className };
        });
    }

    private setAll(changes: readonly Change[]) {
      for (const view of this.panes.views) {
        view.dispatch({ effects: setChanges.of(changes), annotations: fromPeer.of(true) });
      }
      this.panes.drawDividers();
    }

    // The diff of the current texts.
    setBlocks(blocks: MergeBlock[]) {
      this.setAll(changesOf(blocks));
    }

    // SimpleThreesideDiffViewer.replaceChange: the lines of the change in the source pane move to the
    // other one, and the change stops being valid until the next diff. Returns the changed pane.
    perform(action: VersionAction): EditablePane | null {
      const change = this.changes()[action.index];
      if (!change?.valid) return null;
      const [source, target] = DIRECTIONS[action.action];
      const view = this.view(target);
      const [sourceFrom, sourceTo] = change.ranges[source];
      const [from, to] = change.ranges[target];
      const lines = linesOf(this.view(source).state.doc, sourceFrom, sourceTo);
      const found = linesModification(view.state.doc, from, to, lines);
      if (found) {
        view.dispatch({
          changes: { from: found.from, to: found.to, insert: Text.of(found.insert.split("\n")) },
          annotations: [isolateHistory.of("full"), Transaction.userEvent.of("input.replace")],
        });
      }
      const invalid = this.changes().map((each, index) =>
        index === action.index ? { ...each, valid: false } : each,
      );
      this.setAll(invalid);
      if (!found) this.config.onedit();
      return target;
    }

    // The texts for the diff, split only by `\n` like the blocks.
    texts(): { head: string; staged: string; local: string } {
      return {
        head: this.view("left").state.doc.toString(),
        staged: this.view("base").state.doc.toString(),
        local: this.view("right").state.doc.toString(),
      };
    }

    dirty(pane: EditablePane): boolean {
      return !this.view(pane).state.doc.eq(this.saved[pane]);
    }

    // What is sent: what was typed afterwards is not saved.
    snapshot(pane: EditablePane): Text {
      return this.view(pane).state.doc;
    }

    textOf(pane: EditablePane, doc: Text): string {
      return doc.sliceString(0, doc.length, this.view(pane).state.lineBreak);
    }

    markSaved(pane: EditablePane, doc: Text) {
      this.saved[pane] = doc;
    }

    savedText(pane: EditablePane): string {
      return this.textOf(pane, this.saved[pane]);
    }

    // Whether a text fits in the editor of that pane: the line separator cannot be changed in an
    // existing one.
    fits(pane: MergePane, text: string): boolean {
      return this.view(pane).state.lineBreak === lineSeparator(text);
    }

    // A pane with nothing typed moves to what exists now.
    reload(pane: MergePane, text: string) {
      replaceText(this.view(pane), text);
      if (pane !== "left") this.saved[pane] = this.view(pane).state.doc;
    }

    setDirtyTitles() {
      this.panes.setTitle("base", `Staged${this.dirty("base") ? " *" : ""}`);
      this.panes.setTitle("right", `Local${this.dirty("right") ? " *" : ""}`);
    }

    // doScrollToChange: the cursor of each pane at the start of the change.
    private show(index: number, focus?: EditorView) {
      const change = this.changes()[index];
      for (const view of this.panes.views) {
        const doc = view.state.doc;
        const line = change.ranges[view.state.facet(paneFacet)][0];
        const position = line < doc.lines ? doc.line(line + 1).from : doc.length;
        view.dispatch({
          selection: EditorSelection.cursor(position),
          effects: EditorView.scrollIntoView(position, { y: "center" }),
        });
      }
      focus?.focus();
    }

    showFirstChange() {
      if (this.changes().length > 0) this.show(0);
    }

    // The next or previous change of the last pane with the focus; false if there is none.
    navigate(direction: 1 | -1): boolean {
      const view = this.panes.current ?? this.view("base");
      const pane = view.state.facet(paneFacet);
      const line = view.state.doc.lineAt(view.state.selection.main.head).number - 1;
      const starts = this.changes().map((change, index) => ({ index, start: change.ranges[pane][0] }));
      const target =
        direction > 0
          ? starts.find(({ start }) => start > line)
          : starts.filter(({ start }) => start < line).at(-1);
      if (!target) return false;
      this.show(target.index, view);
      return true;
    }

    setWrap(on: boolean) {
      this.panes.setWrap(on);
    }

    destroy() {
      this.panes.destroy();
    }
  }
</script>

<script lang="ts">
  import { onDestroy, untrack } from "svelte";
  import {
    fileWrite,
    indexWrite,
    mergeBlocks,
    notifyRepoChanged,
    snapshotsRestore,
    stagingRevertRange,
    stagingThreeSides,
    type ApiError,
  } from "../lib/api";
  import { errorText } from "../lib/errors";
  import Menu, { type MenuItem } from "../lib/Menu.svelte";
  import type { NoticeKind } from "./FilePane.svelte";
  import { inGutter, softWrapItem, type DiffOptions } from "./options";

  const REDIFF_DELAY_MS = 150;

  interface Props {
    path: string;
    oldPath: string | null;
    // Goes up when something may have changed on disk or in the index.
    version: number;
    options: DiffOptions;
    onnotice: (kind: NoticeKind, text: string, action?: { label: string; run: () => void }) => void;
    // One of the three versions is missing: they are compared two at a time, like ThreeSidesProducer.
    onfallback: () => void;
    // `Soft-Wrap` of the gutter menu: the diff window stores it.
    onwrap: (on: boolean) => void;
  }

  let { path, oldPath, version, options, onnotice, onfallback, onwrap }: Props = $props();

  // The last thing read or written on each side: their hashes are what is expected when writing.
  let sides: ThreeSides | null = null;
  let failure = $state<string | null>(null);
  let dirty = $state(false);
  // What has unsaved changes here changed outside: saving it would fail.
  // `gone`: the three versions no longer exist; what was typed stays until the user discards it.
  let conflict = $state<"staged" | "local" | "gone" | null>(null);
  let host = $state<HTMLDivElement>();
  let menu = $state.raw<{ x: number; y: number; items: MenuItem[] } | null>(null);
  let editors: VersionEditors | null = null;
  let request = 0;
  let rediffs = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  // Saves go one at a time: each one needs the hash left by the previous one.
  let saving: Promise<boolean> = Promise.resolve(true);
  let writing = false;
  // There was a `Revert` on the disk since it was saved: the next save carries a snapshot.
  let reverted = false;

  const title = $derived(requestTitle(path, oldPath));
  const policy = $derived(policyOf(options.ignore));

  function edited() {
    dirty = !!editors && (editors.dirty("base") || editors.dirty("right"));
    editors?.setDirtyTitles();
    clearTimeout(timer);
    timer = setTimeout(rediff, REDIFF_DELAY_MS);
  }

  async function rediff() {
    const target = editors;
    if (!target) return;
    const token = ++rediffs;
    const texts = target.texts();
    try {
      const blocks = await mergeBlocks(texts.head, texts.staged, texts.local, policy);
      const now = target.texts();
      const same = now.head === texts.head && now.staged === texts.staged && now.local === texts.local;
      if (token === rediffs && editors === target && same) target.setBlocks(blocks);
    } catch (error) {
      onnotice("error", errorText(error));
    }
  }

  async function build(fresh: ThreeSides, token: number) {
    const blocks = await mergeBlocks(fresh.head, fresh.staged, fresh.local, policy);
    if (!host || token !== request) return;
    editors?.destroy();
    editors = new VersionEditors(host, {
      path,
      sides: fresh,
      blocks,
      synced: () => options.sync,
      wrap: options.wrap,
      onedit: edited,
      onaction: run,
    });
    editors.showFirstChange();
    dirty = false;
    conflict = null;
  }

  // What comes from outside enters the sides with nothing typed. One with unsaved changes is not
  // touched: if its hash changed, saving it would fail, and that is reported.
  function absorb(fresh: ThreeSides, target: VersionEditors) {
    const known = sides as ThreeSides;
    const next = { ...known, head: fresh.head };
    let changed = false;
    if (target.texts().head !== documentOf(fresh.head).toString()) {
      target.reload("left", fresh.head);
      changed = true;
    }
    conflict = null;
    const panes = [
      ["base", "staged", "stagedHash"],
      ["right", "local", "localHash"],
    ] as const;
    for (const [pane, key, hash] of panes) {
      if (target.dirty(pane)) {
        if (fresh[hash] !== known[hash]) conflict ??= key;
        continue;
      }
      if (target.savedText(pane) !== fresh[key]) {
        target.reload(pane, fresh[key]);
        changed = true;
      }
      next[key] = fresh[key];
      next[hash] = fresh[hash];
    }
    sides = next;
    if (changed) edited();
  }

  function fitsAll(fresh: ThreeSides, target: VersionEditors): boolean {
    return (
      target.fits("left", fresh.head) &&
      (target.dirty("base") || target.fits("base", fresh.staged)) &&
      (target.dirty("right") || target.fits("right", fresh.local))
    );
  }

  async function load() {
    const token = ++request;
    try {
      const fresh = await stagingThreeSides(path);
      if (token !== request || writing) return;
      failure = null;
      if (editors && fitsAll(fresh, editors)) absorb(fresh, editors);
      else if (!editors || !dirty) {
        sides = fresh;
        await build(fresh, token);
      }
    } catch (error) {
      if (token !== request) return;
      if ((error as ApiError).kind === "no_three_sides") {
        // Switching to two panes would remove these editors: with something unsaved, a notice is
        // shown and nothing is touched.
        if (editors && dirty) conflict = "gone";
        else onfallback();
        return;
      }
      failure = errorText(error);
    }
  }

  function offerCopy(copy: string, text: string) {
    onnotice("info", text, { label: "Restore", run: () => restore(copy) });
  }

  async function restore(id: string) {
    try {
      const previous = await snapshotsRestore(id);
      onnotice("info", "Snapshot restored.", { label: "Undo", run: () => restore(previous) });
      await notifyRepoChanged();
    } catch (error) {
      onnotice("error", errorText(error));
    }
  }

  // One side: the stage with `indexWrite`, and the disk with `fileWrite`, or with a snapshot after a
  // `Revert`. What is marked as saved is what was sent. False if it could not be saved.
  async function writePane(pane: EditablePane): Promise<boolean> {
    const target = editors;
    const known = sides;
    if (!target?.dirty(pane) || !known) return true;
    const sent = target.snapshot(pane);
    const content = target.textOf(pane, sent);
    writing = true;
    try {
      if (pane === "base") {
        const written = await indexWrite(path, content, known.stagedHash);
        sides = { ...known, staged: content, stagedHash: written.hash };
        if (written.copy) offerCopy(written.copy, "A snapshot was saved of what existed only in the stage.");
      } else if (reverted) {
        const written = await stagingRevertRange(path, content, known.localHash);
        sides = { ...known, local: content, localHash: written.hash };
        reverted = false;
        if (written.copy) offerCopy(written.copy, "Revert of lines. A snapshot was saved.");
      } else {
        const hash = await fileWrite(path, content, known.localHash);
        sides = { ...known, local: content, localHash: hash };
      }
      target.markSaved(pane, sent);
      // What was read before writing is no longer valid.
      request++;
      return true;
    } catch (error) {
      onnotice("error", errorText(error));
      writing = false;
      // If it changed outside, the notice above says so with the option to reload.
      await load();
      return false;
    } finally {
      writing = false;
      edited();
    }
  }

  async function writeDrafts(): Promise<boolean> {
    const ok = (await writePane("base")) && (await writePane("right"));
    if (ok && editors && !editors.dirty("base") && !editors.dirty("right")) await load();
    return ok;
  }

  export function isDirty(): boolean {
    return dirty;
  }

  export function save(): Promise<boolean> {
    saving = saving.then(writeDrafts);
    return saving;
  }

  // One pass: what was typed during this write is picked up by the next one of `saveAll`.
  export async function flush(): Promise<boolean> {
    if (dirty && !(await save())) return false;
    await saving;
    return true;
  }

  export function navigate(direction: 1 | -1): boolean {
    return editors?.navigate(direction) ?? false;
  }

  // An arrow changes one side and it is saved right away: the stage or the disk end up as they look.
  async function run(action: VersionAction) {
    const target = editors?.perform(action);
    if (!target) return;
    if (action.action === "revert") reverted = true;
    edited();
    saving = saving.then(() => writePane(target));
    if (await saving) await notifyRepoChanged();
  }

  function discard() {
    reverted = false;
    dirty = false;
    conflict = null;
    editors?.destroy();
    editors = null;
    sides = null;
    load();
  }

  $effect(() => {
    void version;
    untrack(() => load());
  });

  $effect(() => {
    void policy;
    untrack(() => editors && rediff());
  });

  // The three panes wrap together, without being rebuilt.
  $effect(() => {
    const wrap = options.wrap;
    untrack(() => editors?.setWrap(wrap));
  });

  // The gutter popup of SetEditorSettingsActionGroup. The text has no menu here.
  function contextmenu(event: MouseEvent) {
    if (!editors || !inGutter(event)) return;
    event.preventDefault();
    menu = { x: event.clientX, y: event.clientY, items: [softWrapItem(options.wrap, onwrap)] };
  }

  onDestroy(() => {
    clearTimeout(timer);
    editors?.destroy();
  });
</script>

<div class="pane">
  <div class="request-title">{title}</div>

  {#if conflict}
    <div class="banner" role="alert">
      <span>
        {conflict === "staged"
          ? "The stage has changed outside and there are unsaved changes here."
          : conflict === "local"
            ? "The file has changed on disk and there are unsaved changes here."
            : "The file no longer has HEAD, stage and disk at the same time and there are unsaved changes here."}
      </span>
      <button type="button" class="link" onclick={discard}>Reload and Discard Edits</button>
    </div>
  {/if}

  {#if failure}
    <p class="message" role="alert">{failure}</p>
  {/if}

  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="host" bind:this={host} oncontextmenu={contextmenu}></div>
</div>

{#if menu}
  <Menu x={menu.x} y={menu.y} label="Diff" items={menu.items} onclose={() => (menu = null)} />
{/if}

<style>
  .pane {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    background: var(--editor);
  }

  .request-title {
    flex: none;
    padding: 4px 12px;
    overflow: hidden;
    color: var(--text);
    text-overflow: ellipsis;
    white-space: nowrap;
    background: var(--panel);
    border-bottom: 1px solid var(--divider);
  }

  .banner {
    display: flex;
    flex: none;
    gap: 12px;
    align-items: center;
    padding: 6px 12px;
    background: var(--notice-warning);
    border-bottom: 1px solid var(--notice-warning-border);
  }

  .link {
    padding: 0;
    font: inherit;
    color: var(--link);
    background: none;
    border: 0;
  }

  .message {
    padding: 16px;
    margin: 0;
    color: var(--text-secondary);
  }

  .host {
    display: flex;
    flex: 1;
    min-height: 0;
  }

  .host :global(.rm-merge) {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }

  .host :global(.rm-merge-titles) {
    display: flex;
    flex: none;
    height: 28px;
    color: var(--text-secondary);
    background: var(--panel);
    border-bottom: 1px solid var(--divider);
  }

  .host :global(.rm-merge-title) {
    display: flex;
    flex: 1;
    align-items: center;
    min-width: 0;
    padding: 0 8px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .host :global(.rm-merge-title-gap) {
    flex: none;
    width: 30px;
  }

  .host :global(.rm-editors) {
    display: flex;
    flex: 1;
    min-width: 0;
    min-height: 0;
  }

  .host :global(.rm-editor) {
    flex: 1;
    min-width: 0;
    min-height: 0;
  }

  .host :global(.rm-editor .cm-editor) {
    height: 100%;
  }

  .host :global(.rm-divider) {
    flex: none;
    width: 30px;
    background: var(--panel);
  }

  .host :global(.rm-inserted-band) {
    fill: var(--diff-inserted);
  }

  .host :global(.rm-deleted-band) {
    fill: var(--diff-deleted);
  }

  .host :global(.rm-modified-band) {
    fill: var(--diff-modified);
  }
</style>
