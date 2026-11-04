import type { Arrow, EdgeStyle, LogRow, PrintRow } from "../lib/api";
import { TOKENS } from "../lib/theme";
import { graphColor } from "./graphColor";
import {
  availableLabelWidth,
  iconWidth,
  LABEL_COMPACT_GAP,
  LABEL_GAP,
  LABEL_ICON_HEIGHT,
  LABEL_LEFT,
  LABEL_RIGHT,
  LABEL_ICON_GAP,
  labelGroups,
  layoutLabels,
  type Label,
} from "./labels";
import type { LogColumn, LogOptions } from "./options";
import type { Range } from "./rowCache";

export const ROW_HEIGHT = 24;

// `PaintParameters`, at a row height of 22.
const BASE_ROW_HEIGHT = 22;
const NODE_WIDTH = 15;
const CIRCLE_RADIUS = 4;
const SELECTED_CIRCLE_RADIUS = 5;
const LINE = 1.5;
const SELECTED_LINE = 2.5;
// `SimpleGraphCellPainter`: `JBColor.BLACK`, which in the dark theme is `Label.foreground`. The reference
// does not include `JBColor`: checked against the class of the installed app.
const MARK_COLOR = TOKENS.text;
const ARROW_ANGLE_COS2 = 0.7;
const ARROW_LENGTH = 0.3;

// `VcsLogGraphTable.MAX_DEFAULT_DYNAMIC_COLUMN_WIDTH` and `MAX_ROWS_TO_CALC_WIDTH`.
const MAX_COLUMN_WIDTH = 300;
export const MAX_ROWS_TO_MEASURE = 1000;
const CELL_PADDING = 8;
const HASH_LENGTH = 8;
const SHORT_HASH_SAMPLE = "e".repeat(HASH_LENGTH);
const FONT_FAMILY = '-apple-system, BlinkMacSystemFont, "Inter", sans-serif';
const FONT = `13px ${FONT_FAMILY}`;
const BOLD_FONT = `bold ${FONT}`;
// `LabelPainter.getReferenceFont`: one point less than the table.
const LABEL_FONT = `12px ${FONT_FAMILY}`;
// `currentBranch.svg`, in a box of 16.
const TAG_ICON_SIZE = 16;
const TAG_LINES: [number, number][] = [
  [13.45, 7.5],
  [7.01, 13.94],
  [6.3, 13.94],
  [2.06, 9.7],
  [2.06, 8.99],
  [8.5, 2.55],
  [8.92, 2.41],
  [12.64, 2.94],
  [13.06, 3.36],
  [13.59, 7.08],
];

const COLORS = {
  background: TOKENS.editor,
  selection: TOKENS.selection,
  text: TOKENS.text,
  muted: TOKENS["text-secondary"],
  merge: TOKENS["log-merge"],
  currentBranch: TOKENS["log-current-branch"],
};

const timeFormat = new Intl.DateTimeFormat(undefined, { timeStyle: "short" });
const dateFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "short" });
const dateTimeFormat = new Intl.DateTimeFormat(undefined, {
  dateStyle: "short",
  timeStyle: "short",
});
const MINUTE = 60_000;
const HOUR = 60 * MINUTE;

// `DateFormatUtil.formatDate` and `formatTime`.
export const shortDate = (time: number) => dateFormat.format(time);
export const shortTime = (time: number) => timeFormat.format(time);

export interface GraphMetrics {
  rowHeight: number;
  nodeWidth: number;
  radius: number;
  selectedRadius: number;
  line: number;
  selectedLine: number;
}

// An edge segment of the row, with its arrow if it has one. `terminal` is an arrow without a segment.
export interface GraphLine {
  kind: "line";
  x: number;
  otherX: number;
  up: boolean;
  color: number;
  style: EdgeStyle;
  arrow: boolean;
  terminal: boolean;
  upRow: number;
  downRow: number;
  // Where its arrow leads, if it has one.
  jump: number | null;
}

export interface GraphNode {
  kind: "node";
  x: number;
  color: number;
}

export type GraphElement = GraphLine | GraphNode;

// What is highlighted on hover: `Selection.FromPrintElement`, a single segment, or
// `Selection.FromNodeIds`, some rows and the edges that join two of them.
export type GraphHighlight =
  | { kind: "element"; row: number; element: GraphElement }
  | { kind: "rows"; rows: ReadonlySet<number> };

export interface ColumnWidths {
  author: number;
  date: number;
  hash: number;
}

export interface DrawOptions {
  context: CanvasRenderingContext2D;
  width: number;
  height: number;
  scrollTop: number;
  range: Range;
  rowAt: (index: number) => LogRow | undefined;
  selected: ReadonlySet<number>;
  options: LogOptions;
  widths: ColumnWidths;
  now: number;
  highlight?: GraphHighlight | null;
  // Filled in with where the labels of each painted row end up.
  labelBounds?: Map<number, LabelBounds>;
}

export interface LabelBounds {
  x: number;
  width: number;
}

export interface TableCell {
  column: LogColumn;
  x: number;
  width: number;
}

export interface TableLayout {
  subjectWidth: number;
  cells: TableCell[];
}

const scaled = (value: number, rowHeight: number) => (value * rowHeight) / BASE_ROW_HEIGHT;

export function graphMetrics(rowHeight: number): GraphMetrics {
  return {
    rowHeight,
    nodeWidth: Math.trunc(scaled(NODE_WIDTH, rowHeight)),
    radius: Math.trunc(scaled(CIRCLE_RADIUS, rowHeight)),
    selectedRadius: Math.trunc(scaled(SELECTED_CIRCLE_RADIUS, rowHeight)),
    line: scaled(LINE, rowHeight),
    selectedLine: scaled(SELECTED_LINE, rowHeight),
  };
}

export const METRICS = graphMetrics(ROW_HEIGHT);

export function graphCellWidth(print: PrintRow, metrics: GraphMetrics): number {
  return Math.trunc((print.width / 2) * metrics.nodeWidth);
}

// `Subject` takes what the other columns leave, and is always the first.
export function tableLayout(
  width: number,
  columns: LogColumn[],
  widths: ColumnWidths,
): TableLayout {
  const shown = columns.filter((column) => column !== "subject") as (keyof ColumnWidths)[];
  const used = shown.reduce((total, column) => total + widths[column], 0);
  let x = Math.max(0, width - used);
  const subjectWidth = x;
  const cells = shown.map((column) => {
    const cell = { column, x, width: widths[column] };
    x += widths[column];
    return cell;
  });
  return { subjectWidth, cells };
}

export const authorText = (row: LogRow) => row.author + (row.samePerson ? "" : "*");

// `Math.rint`: halves round to the even number.
function roundHalfEven(value: number): number {
  const rounded = Math.round(value);
  return rounded - value === 0.5 && rounded % 2 !== 0 ? rounded - 1 : rounded;
}

function minutesAgo(minutes: number): string {
  if (minutes === 0) return "Moments ago";
  if (minutes === 1) return "A minute ago";
  return minutes < 60 ? `${minutes} minutes ago` : "1 hour ago";
}

const sameDay = (a: Date, b: Date) =>
  a.getFullYear() === b.getFullYear() &&
  a.getMonth() === b.getMonth() &&
  a.getDate() === b.getDate();

// `DateFormatUtil.doFormatPretty`, which `JBDateFormat.formatPrettyDateTime` uses with the default
// setting. Its source is not in the reference: the texts are from `UtilBundle`, and the cases were
// checked against the class of the installed app.
export function prettyDateTime(time: number, now: number): string {
  const delta = now - time;
  if (delta >= 0 && delta <= HOUR) return minutesAgo(roundHalfEven(delta / MINUTE));
  const date = new Date(time);
  const today = new Date(now);
  if (sameDay(date, today)) return `Today ${timeFormat.format(time)}`;
  const yesterday = new Date(today.getFullYear(), today.getMonth(), today.getDate() - 1);
  if (sameDay(date, yesterday)) return `Yesterday ${timeFormat.format(time)}`;
  return dateTimeFormat.format(time);
}

const dateText = (row: LogRow, preferCommitDate: boolean, now: number) =>
  prettyDateTime((preferCommitDate ? row.commitTime : row.time) * 1000, now);

export const shortHash = (hash: string) => hash.slice(0, HASH_LENGTH);

// `VcsLogGraphTable.getColumnWidthFromData`.
export function columnWidths(
  context: CanvasRenderingContext2D,
  rows: LogRow[],
  now: number,
): ColumnWidths {
  const measure = (text: string) => Math.ceil(context.measureText(text).width);
  context.font = FONT;
  const widest = (text: (row: LogRow) => string) =>
    rows.reduce((widest, row) => Math.max(widest, measure(text(row))), 0);
  const author = widest(authorText);
  const date = Math.max(
    widest((row) => dateText(row, false, now)),
    widest((row) => dateText(row, true, now)),
  );
  context.font = BOLD_FONT;
  const hash = measure(SHORT_HASH_SAMPLE) + CELL_PADDING;
  context.font = FONT;
  return {
    author: Math.min(author + CELL_PADDING, MAX_COLUMN_WIDTH),
    date: Math.min(date + CELL_PADDING, MAX_COLUMN_WIDTH),
    hash,
  };
}

// `PositionUtil.overNode`, inside the row.
export function nodeAt(print: PrintRow, x: number, y: number): boolean {
  const distance = Math.hypot(
    x - center(print.nodeX, METRICS),
    y - Math.trunc(METRICS.rowHeight / 2),
  );
  return distance <= METRICS.radius;
}

export function drawRows(draw: DrawOptions): void {
  const { context, width, height, scrollTop, range, rowAt, selected, options, widths, now } = draw;
  const { highlight, labelBounds } = draw;
  const layout = tableLayout(width, options.columns, widths);

  context.fillStyle = COLORS.background;
  context.fillRect(0, 0, width, height);
  context.font = FONT;
  context.textBaseline = "middle";
  labelBounds?.clear();

  for (let index = range.start; index < range.end; index++) {
    const row = rowAt(index);
    if (!row) continue;
    const top = index * ROW_HEIGHT - scrollTop;
    const isSelected = selected.has(index);
    const currentBranch = options.highlights.currentBranch && row.highlight.branch;
    if (isSelected || currentBranch) {
      context.fillStyle = isSelected ? COLORS.selection : COLORS.currentBranch;
      context.fillRect(0, top, width, ROW_HEIGHT);
    }
    const labels = drawText(context, row, top, layout, isSelected, options, now);
    if (labels) labelBounds?.set(index, labels);
    paintGraphRow(
      context,
      row.print,
      METRICS,
      top,
      layout.subjectWidth,
      highlight ? (element) => isHighlighted(highlight, index, element) : undefined,
    );
  }
}

function sameElement(a: GraphElement, b: GraphElement): boolean {
  if (a.kind === "node" || b.kind === "node") return a.kind === b.kind;
  return sameSegment(a, b) && a.terminal === b.terminal;
}

export function isHighlighted(
  highlight: GraphHighlight,
  row: number,
  element: GraphElement,
): boolean {
  if (highlight.kind === "element") {
    return highlight.row === row && sameElement(highlight.element, element);
  }
  if (element.kind === "node") return highlight.rows.has(row);
  return highlight.rows.has(element.upRow) && highlight.rows.has(element.downRow);
}

type Segment = Pick<Arrow, "x" | "otherX" | "up">;

function sameSegment(a: Segment, b: Segment): boolean {
  return a.x === b.x && a.otherX === b.otherX && a.up === b.up;
}

// In the order of `PrintElementBuilder`, with the node last so that it ends up over the edges.
export function graphElements(print: PrintRow): GraphElement[] {
  const lines = print.edges.map((edge): GraphLine => {
    const arrow = print.arrows.find((arrow) => !arrow.terminal && sameSegment(arrow, edge));
    return {
      kind: "line",
      ...edge,
      arrow: arrow !== undefined,
      terminal: false,
      jump: arrow?.targetRow ?? null,
    };
  });
  const terminals = print.arrows
    .filter((arrow) => arrow.terminal)
    .map(({ x, otherX, up, color, upRow, downRow, targetRow }): GraphLine => ({
      kind: "line",
      x,
      otherX,
      up,
      color,
      style: "usual",
      arrow: true,
      terminal: true,
      upRow,
      downRow,
      jump: targetRow,
    }));
  return [...lines, ...terminals, { kind: "node", x: print.nodeX, color: print.nodeColor }];
}

// `SimpleGraphCellPainter.draw`. As in a table cell, the row clips what it draws:
// each diagonal is traced whole and only its half shows here, and the neighboring row draws the other.
export function paintGraphRow(
  context: CanvasRenderingContext2D,
  print: PrintRow,
  metrics: GraphMetrics,
  top: number,
  clipWidth: number,
  isSelected: (element: GraphElement) => boolean = () => false,
): void {
  context.save();
  context.beginPath();
  context.rect(0, top, clipWidth, metrics.rowHeight);
  context.clip();
  context.translate(0, top);
  context.lineCap = "round";
  context.lineJoin = "bevel";
  const selected: GraphElement[] = [];
  for (const element of graphElements(print)) {
    if (isSelected(element)) selected.push(element);
    else paintElement(context, element, metrics, false);
  }
  for (const element of selected) paintElement(context, element, metrics, true);
  for (const element of selected) paintElement(context, element, metrics, false);
  context.restore();
}

function paintElement(
  context: CanvasRenderingContext2D,
  element: GraphElement,
  metrics: GraphMetrics,
  mark: boolean,
): void {
  const color = mark ? MARK_COLOR : graphColor(element.color);
  if (element.kind === "node") paintCircle(context, element.x, color, metrics, mark);
  else paintLine(context, element, color, metrics, mark);
}

export const center = (position: number, metrics: GraphMetrics) =>
  metrics.nodeWidth * position + Math.trunc(metrics.nodeWidth / 2);

// Java2D shifts lines by half a pixel and the circle compensates with 0.5. The canvas shifts
// neither of them.
function paintCircle(
  context: CanvasRenderingContext2D,
  position: number,
  color: string,
  metrics: GraphMetrics,
  mark: boolean,
): void {
  context.fillStyle = color;
  context.beginPath();
  context.arc(
    center(position, metrics),
    Math.trunc(metrics.rowHeight / 2),
    mark ? metrics.selectedRadius : metrics.radius,
    0,
    Math.PI * 2,
  );
  context.fill();
}

// `paintUpLine` and `paintDownLine`, with the arrowhead at the end of the stroke or, on a
// diagonal, halfway between the two rows.
function lineGeometry(line: GraphLine, metrics: GraphMetrics) {
  const { rowHeight, radius } = metrics;
  const middle = Math.trunc(rowHeight / 2);
  const x1 = center(line.x, metrics);
  const x2 = center(line.otherX, metrics);
  const short = Math.trunc(radius / 2) + 1;
  if (line.x === line.otherX) {
    if (line.up) {
      const y2 = line.terminal ? short : 0;
      return { x1, y1: middle - 1, x2, y2, tipX: x2, tipY: y2 };
    }
    const y2 = rowHeight - (line.terminal ? short : 0);
    return { x1, y1: middle, x2, y2, tipX: x2, tipY: y2 };
  }
  const y2 = line.up ? -middle : rowHeight + middle;
  return {
    x1,
    y1: middle,
    x2,
    y2,
    tipX: Math.trunc((x1 + x2) / 2),
    tipY: Math.trunc((middle + y2) / 2),
  };
}

function setDash(context: CanvasRenderingContext2D, length: number, rowHeight: number): void {
  const count = Math.max(1, Math.floor(length / rowHeight));
  const space = rowHeight / 2 - 2;
  const dash = length / count - space;
  context.setLineDash([dash, space]);
  context.lineDashOffset = dash / 2;
}

function segment(
  context: CanvasRenderingContext2D,
  x1: number,
  y1: number,
  x2: number,
  y2: number,
): void {
  context.beginPath();
  context.moveTo(x1, y1);
  context.lineTo(x2, y2);
  context.stroke();
}

function paintLine(
  context: CanvasRenderingContext2D,
  line: GraphLine,
  color: string,
  metrics: GraphMetrics,
  mark: boolean,
): void {
  const { x1, y1, x2, y2, tipX, tipY } = lineGeometry(line, metrics);
  context.strokeStyle = color;
  context.lineWidth = mark ? metrics.selectedLine : metrics.line;
  if (line.style === "usual" || line.arrow) {
    context.setLineDash([]);
  } else {
    const length = x1 === x2 ? metrics.rowHeight : Math.hypot(x1 - x2, y1 - y2);
    setDash(context, length, metrics.rowHeight);
  }
  segment(context, x1, y1, x2, y2);
  if (!line.arrow) return;

  const size = ARROW_LENGTH * metrics.rowHeight;
  const cos = Math.sqrt(ARROW_ANGLE_COS2);
  const sin = Math.sqrt(1 - ARROW_ANGLE_COS2);
  const distance = Math.hypot(x1 - tipX, y1 - tipY);
  const scaleX = (size * (x1 - tipX)) / distance;
  const scaleY = (size * (y1 - tipY)) / distance;
  for (const side of [sin, -sin]) {
    segment(
      context,
      tipX,
      tipY,
      Math.round(scaleX * cos - scaleY * side + tipX),
      Math.round(scaleX * side + scaleY * cos + tipY),
    );
  }
}

// `TagPainter.paintTag`, with straight lines: at this size its corner curves are not visible.
function paintTag(
  context: CanvasRenderingContext2D,
  x: number,
  y: number,
  color: string,
  background: string,
  withDot: boolean,
): void {
  const scale = LABEL_ICON_HEIGHT / TAG_ICON_SIZE;
  context.beginPath();
  TAG_LINES.forEach(([tagX, tagY], index) => {
    const pointX = tagX * scale + x;
    const pointY = tagY * scale + y;
    if (index === 0) context.moveTo(pointX, pointY);
    else context.lineTo(pointX, pointY);
  });
  context.closePath();
  context.fillStyle = background;
  context.fill();
  context.strokeStyle = color;
  context.lineWidth = scale;
  context.setLineDash([]);
  context.stroke();
  if (!withDot) return;
  context.fillStyle = color;
  context.beginPath();
  context.arc(10.5 * scale + x, 5.5 * scale + y, scale, 0, Math.PI * 2);
  context.fill();
}

// `LabelIcon.paintTagsForNewUi`: each color peeks out from under the next one, and the dot goes on the
// top one.
function paintIcon(
  context: CanvasRenderingContext2D,
  x: number,
  y: number,
  colors: string[],
  background: string,
) {
  const count = colors.length;
  for (let index = 0; index < count; index++) {
    const offset = x + (iconWidth(index + 1) - iconWidth(1));
    paintTag(context, offset, y, colors[count - 1 - index], background, index === count - 1);
  }
}

// `LabelPainter.paint`.
function paintLabels(
  context: CanvasRenderingContext2D,
  labels: Label[],
  x: number,
  top: number,
  compact: boolean,
  background: string,
): void {
  const y = top + (ROW_HEIGHT - LABEL_ICON_HEIGHT) / 2;
  const gap = compact ? LABEL_COMPACT_GAP : LABEL_GAP;
  let left = x + LABEL_LEFT;
  for (const { text, colors } of labels) {
    paintIcon(context, left, y, colors, background);
    left += iconWidth(colors.length) + LABEL_ICON_GAP;
    context.fillStyle = COLORS.text;
    context.font = LABEL_FONT;
    context.fillText(text, left, top + ROW_HEIGHT / 2);
    left += context.measureText(text).width + gap;
  }
}

function drawText(
  context: CanvasRenderingContext2D,
  row: LogRow,
  top: number,
  layout: TableLayout,
  isSelected: boolean,
  options: LogOptions,
  now: number,
): LabelBounds | null {
  const y = top + ROW_HEIGHT / 2;
  const merge = options.highlights.mergeCommits && row.highlight.muted && !isSelected;
  const bold = options.highlights.myCommits && row.highlight.bold;
  const graph = graphCellWidth(row.print, METRICS);
  const subject = row.subject.replaceAll("\t", " ").trim();
  const cell = Math.max(0, layout.subjectWidth - graph);

  context.save();
  context.beginPath();
  context.rect(graph, top, cell, ROW_HEIGHT);
  context.clip();
  const labelWidth = (text: string) => {
    context.font = LABEL_FONT;
    const width = context.measureText(text).width;
    context.font = FONT;
    return width;
  };
  const available = availableLabelWidth(cell, context.measureText(subject).width, options.compactRefs);
  const { labels, width } = layoutLabels(
    labelGroups(row, options),
    options.compactRefs,
    available,
    labelWidth,
  );
  const color = merge ? COLORS.merge : COLORS.text;
  const left = options.refsOnLeft;
  const textX = left && width > 0 ? graph + width + LABEL_RIGHT : graph;
  const labelsX = left ? graph : Math.max(graph, layout.subjectWidth - width);
  context.font = FONT;
  context.fillStyle = color;
  context.fillText(subject, textX, y);
  if (labels.length > 0) {
    const background = isSelected ? COLORS.selection : rowBackground(row, options);
    // As in the cell: the label covers the end of the subject.
    context.fillStyle = background;
    context.fillRect(labelsX, top, width, ROW_HEIGHT);
    paintLabels(context, labels, labelsX, top, options.compactRefs, background);
  }
  context.restore();

  // In the New UI, with the author column in view, only that one is bold.
  context.fillStyle = merge ? COLORS.merge : COLORS.muted;
  context.font = FONT;
  for (const { column, x, width: columnWidth } of layout.cells) {
    const max = columnWidth - CELL_PADDING;
    if (column === "author") {
      if (bold) context.font = BOLD_FONT;
      context.fillText(authorText(row), x, y, max);
      context.font = FONT;
    } else if (column === "date") {
      context.fillText(dateText(row, options.preferCommitDate, now), x, y, max);
    } else {
      context.fillText(shortHash(row.hash), x, y, max);
    }
  }
  return labels.length > 0 ? { x: labelsX, width } : null;
}

const rowBackground = (row: LogRow, options: LogOptions) =>
  options.highlights.currentBranch && row.highlight.branch
    ? COLORS.currentBranch
    : COLORS.background;
