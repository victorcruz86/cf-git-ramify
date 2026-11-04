import { expect, test } from "vitest";
import type { LogRow, PrintRow, ShortEdge } from "../lib/api";
import {
  columnWidths,
  drawRows,
  graphCellWidth,
  graphMetrics,
  nodeAt,
  paintGraphRow,
  prettyDateTime,
  ROW_HEIGHT,
  tableLayout,
  type GraphHighlight,
  type GraphMetrics,
} from "./draw";
import { graphColor } from "./graphColor";
import { defaultOptions, toggleColumn } from "./options";
import { TOKENS } from "../lib/theme";

interface Stroke {
  color: string;
  width: number;
  dash: number[];
  offset: number;
  points: number[][];
}

interface Circle {
  color: string;
  x: number;
  y: number;
  radius: number;
}

// Only what the drawing uses, and in the order it arrives.
function recorder() {
  const painted: (Stroke | Circle)[] = [];
  const texts: { text: string; x: number }[] = [];
  let dash: number[] = [];
  let points: number[][] = [];
  let circle: { x: number; y: number; radius: number } | null = null;
  const context = {
    fillStyle: "",
    strokeStyle: "",
    lineWidth: 0,
    lineDashOffset: 0,
    lineCap: "",
    lineJoin: "",
    font: "",
    textBaseline: "",
    save() {},
    restore() {},
    translate() {},
    clip() {},
    rect() {},
    fillRect() {},
    beginPath() {
      points = [];
      circle = null;
    },
    closePath() {},
    moveTo(x: number, y: number) {
      points.push([x, y]);
    },
    lineTo(x: number, y: number) {
      points.push([x, y]);
    },
    arc(x: number, y: number, radius: number) {
      circle = { x, y, radius };
    },
    setLineDash(next: number[]) {
      dash = next;
    },
    stroke() {
      painted.push({
        color: context.strokeStyle,
        width: context.lineWidth,
        dash,
        offset: context.lineDashOffset,
        points,
      });
    },
    fill() {
      if (circle) painted.push({ color: context.fillStyle, ...circle });
    },
    fillText(text: string, x: number) {
      texts.push({ text, x });
    },
    measureText: (text: string) => ({ width: text.length }),
  };
  return { context: context as unknown as CanvasRenderingContext2D, painted, texts };
}

const strokes = (painted: (Stroke | Circle)[]) =>
  painted.filter((item): item is Stroke => "points" in item);

function print(overrides: Partial<PrintRow> = {}): PrintRow {
  return { nodeX: 0, nodeColor: 1, width: 2, edges: [], arrows: [], ...overrides };
}

function edge(overrides: Partial<ShortEdge> = {}): ShortEdge {
  return { x: 0, otherX: 0, up: false, color: 1, style: "usual", upRow: 0, downRow: 1, ...overrides };
}

const metrics22: GraphMetrics = graphMetrics(22);

test("row_height", () => {
  expect(ROW_HEIGHT).toBe(24);
});

test("measures_scale_with_the_row", () => {
  expect(metrics22).toMatchObject({
    nodeWidth: 15,
    radius: 4,
    selectedRadius: 5,
    line: 1.5,
    selectedLine: 2.5,
  });
  const at24 = graphMetrics(24);
  expect(at24.nodeWidth).toBe(16);
  expect(at24.radius).toBe(4);
  expect(at24.selectedRadius).toBe(5);
  expect(at24.line).toBeCloseTo((1.5 * 24) / 22);
  expect(at24.selectedLine).toBeCloseTo((2.5 * 24) / 22);
});

test("cell_width_in_half_columns", () => {
  expect(graphCellWidth(print({ width: 3 }), metrics22)).toBe(22);
  expect(graphCellWidth(print({ width: 4 }), metrics22)).toBe(30);
});

test("node_at_the_center_of_its_column", () => {
  const { context, painted } = recorder();
  paintGraphRow(context, print({ nodeX: 2, nodeColor: 5 }), metrics22, 22, 0);
  expect(painted).toEqual([{ color: graphColor(5), x: 37, y: 11, radius: 4 }]);
});

test("straight_edge_reaches_the_row_border", () => {
  const { context, painted } = recorder();
  paintGraphRow(
    context,
    print({ edges: [edge({ x: 1, otherX: 1 }), edge({ x: 1, otherX: 1, up: true })] }),
    metrics22,
    22,
    0,
  );
  expect(strokes(painted).map((stroke) => stroke.points)).toEqual([
    [[22, 11], [22, 22]],
    [[22, 10], [22, 0]],
  ]);
});

test("diagonal_goes_from_center_to_center", () => {
  const { context, painted } = recorder();
  paintGraphRow(
    context,
    print({ edges: [edge({ x: 0, otherX: 1 }), edge({ x: 1, otherX: 0, up: true })] }),
    metrics22,
    22,
    0,
  );
  expect(strokes(painted).map((stroke) => stroke.points)).toEqual([
    [[7, 11], [22, 33]],
    [[22, 11], [7, -11]],
  ]);
});

test("terminal_arrow_stops_short_and_has_a_head", () => {
  const { context, painted } = recorder();
  const arrow = {
    x: 0,
    otherX: 0,
    up: false,
    color: 2,
    targetRow: 40,
    terminal: true,
    upRow: 0,
    downRow: 40,
  };
  paintGraphRow(context, print({ arrows: [arrow] }), metrics22, 22, 0);

  const [line, left, right] = strokes(painted);
  expect(line.points).toEqual([[7, 11], [7, 19]]);
  expect(line.color).toBe(graphColor(2));
  expect(left.points[0]).toEqual([7, 19]);
  expect(right.points[0]).toEqual([7, 19]);
  // Two sides of 0.3 rows on each side, symmetrical.
  expect(left.points[1][1]).toBe(right.points[1][1]);
  expect(left.points[1][0] + right.points[1][0]).toBe(14);
  expect(left.points[1]).toEqual([11, 13]);
});

test("arrow_over_a_segment_is_not_another_line_and_sits_at_the_border", () => {
  const { context, painted } = recorder();
  const arrow = {
    x: 0,
    otherX: 0,
    up: false,
    color: 1,
    targetRow: 40,
    terminal: false,
    upRow: 0,
    downRow: 40,
  };
  paintGraphRow(context, print({ edges: [edge()], arrows: [arrow] }), metrics22, 22, 0);

  const [line, left] = strokes(painted);
  expect(strokes(painted)).toHaveLength(3);
  expect(line.points).toEqual([[7, 11], [7, 22]]);
  expect(left.points[0]).toEqual([7, 22]);
});

test("dashed_edge_spreads_the_stroke_and_arrow_is_always_solid", () => {
  const { context, painted } = recorder();
  const dashed = edge({ style: "dashed" });
  const dotted = edge({ x: 1, otherX: 1, style: "dotted" });
  const withArrow = edge({ x: 2, otherX: 2, style: "dashed" });
  const arrow = {
    x: 2,
    otherX: 2,
    up: false,
    color: 1,
    targetRow: 9,
    terminal: false,
    upRow: 0,
    downRow: 9,
  };
  paintGraphRow(
    context,
    print({ edges: [dashed, dotted, withArrow], arrows: [arrow] }),
    metrics22,
    22,
    0,
  );

  const [first, second, third] = strokes(painted);
  expect(first.dash).toEqual([13, 9]);
  expect(first.offset).toBe(6.5);
  expect(second.dash).toEqual([13, 9]);
  expect(third.dash).toEqual([]);
});

test("selection_paints_the_brand_color_and_then_its_own", () => {
  const { context, painted } = recorder();
  const other = edge({ x: 1, otherX: 1, color: 3 });
  const selected = edge({ color: 2 });
  paintGraphRow(
    context,
    print({ edges: [selected, other], nodeColor: 4 }),
    metrics22,
    22,
    0,
    (element) => element.kind === "node" || element.color === 2,
  );

  const summary = painted.map((item) =>
    "points" in item
      ? { color: item.color, size: item.width, kind: "edge" }
      : { color: item.color, size: item.radius, kind: "node" },
  );
  expect(summary).toEqual([
    { color: graphColor(3), size: 1.5, kind: "edge" },
    { color: TOKENS.text, size: 2.5, kind: "edge" },
    { color: TOKENS.text, size: 5, kind: "node" },
    { color: graphColor(2), size: 1.5, kind: "edge" },
    { color: graphColor(4), size: 4, kind: "node" },
  ]);
});

const WIDTHS = { author: 150, date: 100, hash: 64 };
const NOW = new Date(2026, 10, 1, 13, 0, 0).getTime();
const SECOND = 1000;
const MINUTE = 60 * SECOND;
const HOUR = 60 * MINUTE;
const time = (date: number) => new Intl.DateTimeFormat(undefined, { timeStyle: "short" }).format(date);
const dateTime = (date: number) =>
  new Intl.DateTimeFormat(undefined, { dateStyle: "short", timeStyle: "short" }).format(date);
const SUBJECT_END = 800 - 150 - 100;

function logRow(overrides: Partial<LogRow> = {}): LogRow {
  return {
    hash: "abcdef0123456789",
    subject: "subject",
    author: "Ana",
    time: 1_000,
    commitTime: 1_000,
    samePerson: true,
    parentCount: 1,
    refs: [],
    labels: { compact: [], normal: [] },
    highlight: { bold: false, muted: false, branch: false },
    print: print({ width: 3 }),
    ...overrides,
  };
}

function drawn(
  row: LogRow,
  change: Partial<ReturnType<typeof defaultOptions>> = {},
  selected: number[] = [],
) {
  const fills: { color: string; x: number; y: number; width: number }[] = [];
  const texts: { text: string; x: number; color: string; font: string }[] = [];
  const { context } = recorder();
  context.fillRect = (x: number, y: number, width: number) =>
    fills.push({ color: context.fillStyle as string, x, y, width });
  context.fillText = (text: string, x: number) =>
    texts.push({ text, x, color: context.fillStyle as string, font: context.font });
  drawRows({
    context,
    width: 800,
    height: 48,
    scrollTop: 0,
    range: { start: 0, end: 1 },
    rowAt: () => row,
    selected: new Set(selected),
    options: { ...defaultOptions(), ...change },
    widths: WIDTHS,
    now: NOW,
  });
  const at = (text: string) => texts.find((item) => item.text === text);
  return { fills, texts, at };
}

function drawGraph(highlight: GraphHighlight) {
  const { context, painted } = recorder();
  const row = logRow({ print: print({ nodeX: 0, nodeColor: 4, edges: [edge({ upRow: 0, downRow: 1 })] }) });
  drawRows({
    context,
    width: 800,
    height: 48,
    scrollTop: 0,
    range: { start: 0, end: 1 },
    rowAt: () => row,
    selected: new Set(),
    options: defaultOptions(),
    widths: WIDTHS,
    now: NOW,
    highlight,
  });
  return painted.filter((item) => item.color === TOKENS.text);
}

test("hover_highlight_is_painted_with_the_brand_color", () => {
  expect(drawGraph({ kind: "rows", rows: new Set([0, 1]) })).toHaveLength(2);
  expect(drawGraph({ kind: "rows", rows: new Set([5]) })).toHaveLength(0);
});

test("remembers_where_the_labels_of_each_row_end_up", () => {
  const { context } = recorder();
  const labelBounds = new Map([[9, { x: 0, width: 0 }]]);
  const labels = { compact: [{ name: "main", kinds: ["local" as const] }], normal: [] };
  drawRows({
    context,
    width: 800,
    height: 48,
    scrollTop: 0,
    range: { start: 0, end: 2 },
    rowAt: (index) => logRow(index === 0 ? { labels } : {}),
    selected: new Set(),
    options: defaultOptions(),
    widths: WIDTHS,
    now: NOW,
    labelBounds,
  });

  expect([...labelBounds.keys()]).toEqual([0]);
  expect(labelBounds.get(0)?.width).toBeGreaterThan(0);
});

test("columns_subject_author_date_by_default", () => {
  expect(defaultOptions().columns).toEqual(["subject", "author", "date"]);
  const { cells, subjectWidth } = tableLayout(800, defaultOptions().columns, WIDTHS);

  expect(subjectWidth).toBe(SUBJECT_END);
  expect(cells.map((cell) => [cell.column, cell.x])).toEqual([
    ["author", SUBJECT_END],
    ["date", SUBJECT_END + 150],
  ]);
});

test("hash_is_enabled_from_columns", () => {
  const columns = toggleColumn(defaultOptions().columns, "hash");

  expect(columns).toEqual(["subject", "author", "date", "hash"]);
  expect(drawn(logRow(), { columns }).at("abcdef01")).toBeTruthy();
  expect(drawn(logRow()).at("abcdef01")).toBeUndefined();
  expect(toggleColumn(columns, "hash")).toEqual(defaultOptions().columns);
});

test("hidden_author_is_not_painted", () => {
  const columns = toggleColumn(defaultOptions().columns, "author");

  expect(drawn(logRow(), { columns }).at("Ana")).toBeUndefined();
});

test("author_with_asterisk_if_the_committer_is_someone_else", () => {
  expect(drawn(logRow({ samePerson: false })).at("Ana*")).toBeTruthy();
  expect(drawn(logRow()).at("Ana")).toBeTruthy();
});

test("prefer_commit_timestamp_changes_the_date", () => {
  const row = logRow({ time: 0, commitTime: 86_400 * 400 });
  const dates = (preferCommitDate: boolean) =>
    drawn(row, { preferCommitDate }).texts.at(-1)?.text;

  expect(dates(false)).toBe(dateTime(0));
  expect(dates(true)).toBe(dateTime(86_400 * 400 * 1000));
});

// What `DateFormatUtil.formatPrettyDateTime` of the installed app gives for those distances.
test("date_as_format_pretty_date_time", () => {
  const ago = (delta: number) => prettyDateTime(NOW - delta, NOW);

  expect(ago(0)).toBe("Moments ago");
  expect(ago(20 * SECOND)).toBe("Moments ago");
  expect(ago(40 * SECOND)).toBe("A minute ago");
  expect(ago(MINUTE)).toBe("A minute ago");
  expect(ago(90 * SECOND)).toBe("2 minutes ago");
  expect(ago(59 * MINUTE)).toBe("59 minutes ago");
  expect(ago(HOUR)).toBe("1 hour ago");
  expect(ago(HOUR + MINUTE)).toBe(`Today ${time(NOW - HOUR - MINUTE)}`);
  expect(ago(-5 * MINUTE)).toBe(`Today ${time(NOW + 5 * MINUTE)}`);
  expect(ago(20 * HOUR)).toBe(`Yesterday ${time(NOW - 20 * HOUR)}`);
  expect(ago(60 * HOUR)).toBe(dateTime(NOW - 60 * HOUR));
});

test("yesterday_across_a_year_change", () => {
  const now = new Date(2027, 0, 1, 9, 0, 0).getTime();
  const before = new Date(2026, 11, 31, 18, 0, 0).getTime();

  expect(prettyDateTime(before, now)).toBe(`Yesterday ${time(before)}`);
});

test("the_same_day_last_year_is_not_today", () => {
  const before = new Date(2025, 10, 1, 9, 0, 0).getTime();

  expect(prettyDateTime(before, NOW)).toBe(dateTime(before));
});

test("date_width_counts_the_pretty_text", () => {
  const { context } = recorder();
  const today = logRow({ time: (NOW - 2 * HOUR) / 1000, commitTime: (NOW - 2 * HOUR) / 1000 });

  const widths = columnWidths(context, [today], NOW);

  expect(widths.date).toBeGreaterThanOrEqual(`Today ${time(NOW - 2 * HOUR)}`.length);
});

const TAGGED = {
  compact: [{ name: "main", kinds: ["local" as const] }],
  normal: [{ name: "main", kinds: ["local" as const] }],
};

test("labels_at_the_end_of_the_subject", () => {
  const { at } = drawn(logRow({ labels: TAGGED }));

  expect(at("subject")!.x).toBeLessThan(at("main")!.x);
  expect(at("main")!.x).toBeGreaterThan(SUBJECT_END - 80);
  expect(at("main")!.font).toContain("12px");
});

test("references_on_the_left_moves_the_label", () => {
  const { at } = drawn(logRow({ labels: TAGGED }), { refsOnLeft: true });

  expect(at("main")!.x).toBeLessThan(at("subject")!.x);
});

test("column_widths_come_from_the_data", () => {
  const { context } = recorder();
  const long = "x".repeat(500);
  const rows = [logRow({ author: "Ana" }), logRow({ author: long })];

  const widths = columnWidths(context, rows, NOW);

  expect(widths.author).toBe(300);
  const marked = logRow({ author: "Ana", samePerson: false });
  expect(columnWidths(context, [marked], NOW).author).toBeLessThan(20);
  expect(widths.hash).toBeGreaterThanOrEqual(8);
});

test("node_under_the_cursor", () => {
  const row = print({ nodeX: 1 });
  const { nodeWidth, rowHeight } = graphMetrics(ROW_HEIGHT);
  const x = nodeWidth + Math.trunc(nodeWidth / 2);

  expect(nodeAt(row, x, Math.trunc(rowHeight / 2))).toBe(true);
  expect(nodeAt(row, x + 20, Math.trunc(rowHeight / 2))).toBe(false);
});

const NONE = { bold: false, muted: false, branch: false };

function highlighted(highlight: LogRow["highlight"], selected: number[] = [], change = {}) {
  return drawn(logRow({ subject: "one", highlight }), change, selected);
}

test("current_branch_with_its_background", () => {
  const { fills } = highlighted({ ...NONE, branch: true });

  expect(fills.map((fill) => fill.color)).toEqual([TOKENS.editor, "#1c2433"]);
});

test("selection_takes_precedence_over_the_current_branch", () => {
  const { fills } = highlighted({ ...NONE, branch: true }, [0]);

  expect(fills.map((fill) => fill.color)).toEqual([TOKENS.editor, TOKENS.selection]);
});

test("merge_in_gray", () => {
  const { texts } = highlighted({ ...NONE, muted: true });

  expect(texts.map((text) => text.color)).toEqual(Array(3).fill(TOKENS["log-merge"]));
});

test("selected_merge_without_gray", () => {
  const { texts } = highlighted({ ...NONE, muted: true }, [0]);

  expect(texts[0].color).toBe(TOKENS.text);
});

test("my_commits_in_bold_only_in_the_author", () => {
  const { at } = highlighted({ ...NONE, bold: true });

  expect(at("one")!.font.startsWith("bold")).toBe(false);
  expect(at("Ana")!.font.startsWith("bold")).toBe(true);
});

test("each_highlight_is_turned_off_from_view_options", () => {
  const highlights = { myCommits: false, mergeCommits: false, currentBranch: false };
  const { fills, at } = highlighted(
    { bold: true, muted: true, branch: true },
    [],
    { highlights },
  );

  expect(fills.map((fill) => fill.color)).toEqual([TOKENS.editor]);
  expect(at("one")!.color).toBe(TOKENS.text);
  expect(at("Ana")!.font.startsWith("bold")).toBe(false);
});
