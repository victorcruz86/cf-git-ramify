<script lang="ts">
  import { EditorView, gutter, GutterMarker, type BlockInfo } from "@codemirror/view";
  import { onDestroy } from "svelte";
  import {
    annotationShowInLog,
    clipboardWrite,
    type AnnotatedCommit,
    type Annotation,
    type AnnotationTarget,
  } from "../lib/api";
  import { hostingItems, OPEN_ON_GITHUB } from "../hosting/links";
  import Menu, { type MenuItem } from "../lib/Menu.svelte";
  import {
    annotationColumns,
    annotationOptions,
    annotationPopup,
    background,
    isLatest,
    setAnnotationOptions,
    tooltipText,
    type AnnotationColumn,
    type AnnotationOptions,
  } from "./annotation";
  import { annotationGutter } from "./editor";

  // The annotations of one tab (`AnnotateToggleAction.doAnnotate`): a gutter left of the line
  // numbers whose cells are plain text, and its popup. The tab says what its entries open.
  interface Props {
    view: EditorView;
    annotation: Annotation;
    close: () => void;
    showDiff: (commit: AnnotatedCommit) => void;
    // `line` is the line (from 1) the popup was opened on.
    annotate: (target: AnnotationTarget, line: number) => void;
    // `Open on GitHub` failed: the window shows it.
    onerror?: (failure: unknown) => void;
  }

  let { view, annotation, close, showDiff, annotate, onerror }: Props = $props();
  let menu = $state.raw<{ x: number; y: number; items: MenuItem[] } | null>(null);

  // `DS:13-14`: `VCS_ANNOTATIONS_LAST_COMMIT_COLOR`, bold (`AnnotationPresentation.getFontType`),
  // and `ANNOTATIONS_COLOR` for the others.
  const LATEST_COLOR = "#ced0d6";
  const OTHER_COLOR = "#8d9199";
  // `FillerColumn` in the New UI.
  const FILLER = "8px";
  // The gap a column with margins adds to its widest text, half on each side.
  const GAP = 5;

  interface CellLook {
    // The annotation the cell was made from: the prop may already be another one.
    annotation: Annotation;
    // From 1; 0 for the spacer.
    line: number;
    texts: string[];
    commit: AnnotatedCommit | null;
  }

  class Cell extends GutterMarker {
    look: CellLook;
    columns: AnnotationColumn[];
    options: AnnotationOptions;

    constructor(look: CellLook, columns: AnnotationColumn[], options: AnnotationOptions) {
      super();
      this.look = look;
      this.columns = columns;
      this.options = options;
    }

    eq(other: GutterMarker): boolean {
      return (
        other instanceof Cell &&
        other.look.line === this.look.line &&
        other.look.commit === this.look.commit &&
        other.columns === this.columns &&
        other.look.texts.join("\0") === this.look.texts.join("\0")
      );
    }

    toDOM(): Node {
      const cell = document.createElement("div");
      cell.className = "cm-annotation";
      const { commit, texts, line, annotation } = this.look;
      if (line > 0) cell.dataset.line = String(line);
      if (commit) {
        const latest = isLatest(annotation, commit);
        cell.style.color = latest ? LATEST_COLOR : OTHER_COLOR;
        if (latest) cell.style.fontWeight = "bold";
        const color = background(commit, this.options.colors);
        if (color) cell.style.backgroundColor = color;
        cell.title = tooltipText(annotation, commit);
      }
      this.columns.forEach((column, index) => {
        const text = document.createElement("span");
        text.className = "cm-annotation-column";
        text.style.width = `calc(${column.width}ch + ${GAP}px)`;
        text.textContent = texts[index] ?? "";
        cell.append(text);
      });
      return cell;
    }
  }

  // The cells of every line; a line past the annotation, like the empty one after the last
  // newline, is blank.
  function extension(annotation: Annotation, options: AnnotationOptions) {
    const columns = annotationColumns(annotation, options, Date.now());
    const cells = new Map<number, Cell>();
    const cellOf = (line: number) => {
      let cell = cells.get(line);
      if (!cell) {
        const index = annotation.lines[line - 1];
        const commit = index === null || index === undefined ? null : annotation.commits[index];
        const texts = commit ? columns.map((column) => column.texts[line - 1]) : [];
        cell = new Cell({ annotation, line, texts, commit: commit ?? null }, columns, options);
        cells.set(line, cell);
      }
      return cell;
    };
    // `GHOpenInBrowserFromAnnotationActionGroup`: the commit of the clicked line, when it has one.
    async function popup(line: number, x: number, y: number) {
      const commit = cellOf(line).look.commit;
      const hosting = commit
        ? await hostingItems({ kind: "commit", hash: commit.hash }, "open", OPEN_ON_GITHUB, (failure) =>
            onerror?.(failure),
          )
        : [];
      menu = {
        x,
        y,
        items: annotationPopup(annotation, line, options, {
          close,
          showDiff,
          copy: (text) => void clipboardWrite(text).catch(() => {}),
          annotate: (target, from) => annotate(target, from),
          options: setAnnotationOptions,
          hosting,
        }),
      };
    }
    // The cell clicked says its line; the block of the event is the fallback.
    const lineOf = (block: BlockInfo, event?: Event) => {
      const cell = (event?.target as Element | null)?.closest?.<HTMLElement>(".cm-annotation");
      const line = Number(cell?.dataset.line);
      return line > 0 ? line : view.state.doc.lineAt(block.from).number;
    };
    return [
      gutter({
        class: "cm-annotations",
        lineMarker: (_view, block) => cellOf(lineOf(block)),
        initialSpacer: () =>
          new Cell({ annotation, line: 0, texts: [], commit: null }, columns, options),
        domEventHandlers: {
          // `GitAnnotationAspect.showAffectedPaths`: the commit in the main window's Log.
          click: (_view, block, event) => {
            const commit = cellOf(lineOf(block, event)).look.commit;
            if (commit) annotationShowInLog(commit.hash).catch(() => {});
            return true;
          },
          contextmenu: (_view, block, event) => {
            const line = lineOf(block, event);
            const { clientX: x, clientY: y } = event as MouseEvent;
            void popup(line, x, y);
            return true;
          },
        },
      }),
      EditorView.theme({
        ".cm-annotations .cm-gutterElement": { padding: "0" },
        ".cm-annotation": {
          height: "100%",
          boxSizing: "border-box",
          paddingLeft: FILLER,
          whiteSpace: "pre",
          cursor: "default",
        },
        ".cm-annotation-column": {
          display: "inline-block",
          boxSizing: "border-box",
          paddingLeft: `${GAP / 2}px`,
          textAlign: "left",
        },
      }),
    ];
  }

  $effect(() => {
    const built = extension(annotation, $annotationOptions);
    view.dispatch({ effects: annotationGutter.reconfigure(built) });
  });

  onDestroy(() => {
    try {
      view.dispatch({ effects: annotationGutter.reconfigure([]) });
    } catch {
      // The editor went first, with its gutter.
    }
  });
</script>

{#if menu}
  <Menu x={menu.x} y={menu.y} label="Annotations" items={menu.items} onclose={() => (menu = null)} />
{/if}
