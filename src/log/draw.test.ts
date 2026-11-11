import { expect, test } from "vitest";
import type { LogRow, PrintRow, ShortEdge } from "../lib/api";
import signedSvg from "../lib/icons/signatureSigned.svg?raw";
import verifiedSvg from "../lib/icons/signatureVerified.svg?raw";
import {
  BADGE,
  BARS,
  borderAt,
  CHECK,
  columnWidths,
  dragWidths,
  effectiveWidths,
  readSavedWidths,
  drawRows,
  graphCellWidth,
  graphMetrics,
  nodeAt,
  formatPrettyDate,
  paintGraphRow,
  prettyDateTime,
  ROW_HEIGHT,
  tableLayout,
  type GraphHighlight,
  type GraphMetrics,
} from "./draw";
import type { Signature } from "../lib/api";
import { graphColor } from "./graphColor";
import { defaultOptions, historyOptions, toggleColumn, type LogColumn } from "./options";
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

const WIDTHS = { author: 150, date: 100, hash: 64, signature: 24 };
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
  notPicked: ReadonlySet<string> | null = null,
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
    notPicked,
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

// `FileHistoryUiProperties`: `Author, Date, Commit`. The Commit column (graph, references and
// subject) goes after the other two, and the graph is drawn there.
test("history_columns_are_author_date_then_commit", () => {
  const columns = historyOptions().columns;
  expect(columns).toEqual(["author", "date", "subject"]);
  const layout = tableLayout(800, columns, WIDTHS);
  expect(layout.subjectX).toBe(250);
  expect(layout.subjectWidth).toBe(SUBJECT_END);
  expect(layout.cells.map((cell) => [cell.column, cell.x])).toEqual([
    ["author", 0],
    ["date", 150],
  ]);

  const shown = drawn(logRow({ subject: "fix it" }), { columns });
  expect(shown.at("Ana")?.x).toBe(0);
  expect(shown.at("fix it")!.x).toBeGreaterThanOrEqual(250);
  const { context, painted } = recorder();
  let offset = 0;
  const translate = context.translate.bind(context);
  context.translate = (x: number, y: number) => {
    offset += x;
    translate(x, y);
  };
  const arc = context.arc.bind(context);
  context.arc = (x: number, y: number, radius: number, start: number, end: number) =>
    arc(x + offset, y, radius, start, end);
  paintGraphRow(context, print({ nodeX: 0 }), metrics22, 0, 100, undefined, 250);
  const node = painted.find((item): item is Circle => "radius" in item)!;
  expect(node.x).toBeGreaterThanOrEqual(250);
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

// `DateFormatUtil.doFormatPretty` at idea/241.14494.240 (`DateFormatUtil.java:167`): "minutes ago"
// up to `HOUR + MINUTE` included, which `UtilBundle`'s `60#1 hour` names for 60 and 61 minutes.
test("date_as_format_pretty_date_time", () => {
  const ago = (delta: number) => prettyDateTime(NOW - delta, NOW);

  expect(ago(0)).toBe("Moments ago");
  expect(ago(20 * SECOND)).toBe("Moments ago");
  expect(ago(40 * SECOND)).toBe("A minute ago");
  expect(ago(MINUTE)).toBe("A minute ago");
  expect(ago(90 * SECOND)).toBe("2 minutes ago");
  expect(ago(59 * MINUTE)).toBe("59 minutes ago");
  expect(ago(HOUR)).toBe("1 hour ago");
  expect(ago(HOUR + 30 * SECOND)).toBe("1 hour ago");
  expect(ago(HOUR + MINUTE)).toBe("1 hour ago");
  expect(ago(HOUR + MINUTE + SECOND)).toBe(`Today ${time(NOW - HOUR - MINUTE - SECOND)}`);
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

// `DateFormatUtil.formatPrettyDate`: no time and no "minutes ago", only Today, Yesterday or the
// short date.
test("format_pretty_date_has_no_time", () => {
  const shortDate = (date: number) => new Intl.DateTimeFormat(undefined, { dateStyle: "short" }).format(date);
  const ago = (delta: number) => formatPrettyDate(NOW - delta, NOW);

  expect(ago(0)).toBe("Today");
  expect(ago(5 * MINUTE)).toBe("Today");
  expect(ago(-5 * MINUTE)).toBe("Today");
  expect(ago(20 * HOUR)).toBe("Yesterday");
  expect(ago(60 * HOUR)).toBe(shortDate(NOW - 60 * HOUR));
  const newYear = new Date(2027, 0, 1, 9, 0, 0).getTime();
  expect(formatPrettyDate(new Date(2026, 11, 31, 18, 0, 0).getTime(), newYear)).toBe("Yesterday");
  const lastYear = new Date(2025, 10, 1, 9, 0, 0).getTime();
  expect(formatPrettyDate(lastYear, NOW)).toBe(shortDate(lastYear));
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

test("not_cherry_picked_greys_every_other_row_even_selected", () => {
  const row = logRow({ subject: "one" });
  const colors = (notPicked: ReadonlySet<string> | null, selected: number[] = []) =>
    drawn(row, {}, selected, notPicked).texts.map((text) => text.color);

  const normal = [TOKENS.text, TOKENS["text-secondary"], TOKENS["text-secondary"]];
  expect(colors(new Set([row.hash]))).toEqual(normal);
  expect(colors(new Set(["other"]))).toEqual(Array(3).fill(TOKENS["log-merge"]));
  expect(colors(new Set(["other"]), [0])).toEqual(Array(3).fill(TOKENS["log-merge"]));
  // While it is computed nothing changes.
  expect(colors(null)).toEqual(normal);
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

// The canvas and `Icon` draw the same badge: the SVGs carry the points the canvas uses.
test("the_signature_icons_on_the_canvas_are_the_svgs", () => {
  const points = (svg: string, tag: string) =>
    svg.match(new RegExp(`<${tag} points="([^"]+)"`))![1].split(" ").map((pair) => pair.split(",").map(Number));
  expect(points(verifiedSvg, "polygon")).toEqual(BADGE);
  expect(points(signedSvg, "polygon")).toEqual(BADGE);
  expect(points(verifiedSvg, "polyline")).toEqual(CHECK);
  const bars = BARS.map(([left, top, right]) => `M${left} ${top}H${right}`).join("");
  expect(signedSvg).toContain(`d="${bars}"`);
  expect(verifiedSvg).toContain(`fill="${TOKENS["signature-verified"]}"`);
  expect(signedSvg).toContain(`stroke="${TOKENS["signature-signed"]}"`);
});

// `VcsLogIconCellRenderer`: a fixed cell after the others, with the verified badge, the amber one
// for every other signed state, and nothing for an unsigned or not yet loaded commit.
test("the_signature_cell_draws_the_icon_of_each_state", () => {
  const columns = toggleColumn(defaultOptions().columns, "signature");
  const layout = tableLayout(800, columns, WIDTHS);
  expect(layout.cells.at(-1)).toEqual({ column: "signature", x: 776, width: 24 });
  expect(columnWidths(recorder().context, [], NOW).signature).toBe(24);

  const strokesOf = (status: Signature["status"] | null, shown = columns) => {
    const { context, painted } = recorder();
    const row = logRow();
    const signature: Signature | undefined = status
      ? { hash: row.hash, status, signer: "", fingerprint: "" }
      : undefined;
    drawRows({
      context,
      width: 800,
      height: 48,
      scrollTop: 0,
      range: { start: 0, end: 1 },
      rowAt: () => row,
      selected: new Set(),
      options: { ...defaultOptions(), columns: shown },
      widths: WIDTHS,
      now: NOW,
      signatures: new Map(signature ? [[row.hash, signature]] : []),
    });
    return strokes(painted).filter((stroke) => stroke.points.every(([x]) => x >= 776));
  };
  const verified = strokesOf("verified");
  expect(verified.map((stroke) => stroke.color)).toEqual(["#FFFFFF"]);
  expect(verified[0].points[0]).toEqual([776 + 4 + CHECK[0][0], 4 + CHECK[0][1]]);
  for (const status of ["unknown", "expired", "expiredKey", "revokedKey", "cannotVerify", "bad"] as const) {
    const signed = strokesOf(status);
    expect(signed.map((stroke) => stroke.color)).toEqual(Array(4).fill(TOKENS["signature-signed"]));
  }
  expect(strokesOf(null)).toEqual([]);
  expect(strokesOf("verified", defaultOptions().columns)).toEqual([]);
});

// Resizable columns: `VcsLogGraphTable.updateDynamicColumnsWidth`, `isOnLeftBorder` and
// `isOnRightBorder`, with `BORDER_THICKNESS = 3`.
const AUTO = { author: 100, date: 80, hash: 70, signature: 24 };
const AUTO_NARROW = { author: 150, date: 100, hash: 70, signature: 24 };
const MAIN: LogColumn[] = ["subject", "author", "date"];
const HISTORY: LogColumn[] = ["author", "date", "subject"];

test("a_saved_width_wins_over_the_automatic_one", () => {
  expect(effectiveWidths(AUTO, { author: 150 }, 800)).toEqual({ ...AUTO, author: 150 });
});

test("a_saved_width_wider_than_the_table_is_ignored", () => {
  expect(effectiveWidths(AUTO, { author: 900 }, 800)).toEqual(AUTO);
});

test("zero_negative_and_nan_are_ignored", () => {
  expect(effectiveWidths(AUTO, { author: 0, date: -5, hash: Number.NaN }, 800)).toEqual(AUTO);
  expect(effectiveWidths(AUTO, undefined, 800)).toEqual(AUTO);
});

test("the_border_right_of_subject_resizes_the_column_after_it", () => {
  // Subject 0-620, Author 620-720, Date 720-800.
  const layout = tableLayout(800, MAIN, AUTO);
  expect(borderAt(layout, MAIN, 620)).toMatchObject({ column: "author", left: "subject", right: "author", x: 620 });
  expect(borderAt(layout, MAIN, 721)).toMatchObject({ column: "date", left: "author", right: "date", x: 720 });
  expect(borderAt(layout, MAIN, 800)).toBeNull();
  expect(borderAt(layout, MAIN, 400)).toBeNull();
});

test("a_column_left_of_subject_resizes_by_its_right_border", () => {
  // Author 0-100, Date 100-180, Subject 180-800.
  const layout = tableLayout(800, HISTORY, AUTO);
  expect(borderAt(layout, HISTORY, 100)).toMatchObject({ column: "author", left: "author", right: "date" });
  expect(borderAt(layout, HISTORY, 180)).toMatchObject({ column: "date", left: "date", right: "subject" });
  expect(borderAt(layout, HISTORY, 0)).toBeNull();
});

test("three_points_from_a_border_is_on_it_and_four_is_not", () => {
  const layout = tableLayout(800, MAIN, AUTO);
  expect(borderAt(layout, MAIN, 617)).not.toBeNull();
  expect(borderAt(layout, MAIN, 623)).not.toBeNull();
  expect(borderAt(layout, MAIN, 616)).toBeNull();
  expect(borderAt(layout, MAIN, 624)).toBeNull();
});

test("the_signature_column_has_no_border", () => {
  const columns: LogColumn[] = ["subject", "author", "signature"];
  // Subject 0-676, Author 676-776, Signature 776-800.
  const layout = tableLayout(800, columns, AUTO);
  expect(borderAt(layout, columns, 776)).toBeNull();
  expect(borderAt(layout, columns, 676)).toMatchObject({ column: "author" });
});

test("dragging_the_subject_border_changes_only_that_column", () => {
  const layout = tableLayout(800, MAIN, AUTO);
  const border = borderAt(layout, MAIN, 620)!;
  expect(dragWidths(AUTO, border, -50, 800, MAIN)).toEqual({ author: 150 });
  expect(dragWidths(AUTO, border, 30, 800, MAIN)).toEqual({ author: 70 });
});

test("dragging_between_author_and_date_moves_width_from_one_to_the_other", () => {
  const main = tableLayout(800, MAIN, AUTO);
  expect(dragWidths(AUTO, borderAt(main, MAIN, 720)!, 20, 800, MAIN)).toEqual({ author: 120, date: 60 });
  const history = tableLayout(800, HISTORY, AUTO);
  expect(dragWidths(AUTO, borderAt(history, HISTORY, 100)!, 20, 800, HISTORY)).toEqual({ author: 120, date: 60 });
  expect(dragWidths(AUTO, borderAt(history, HISTORY, 180)!, 20, 800, HISTORY)).toEqual({ date: 100 });
});

test("a_column_does_not_go_under_15_and_subject_keeps_15", () => {
  const layout = tableLayout(800, MAIN, AUTO);
  expect(dragWidths(AUTO, borderAt(layout, MAIN, 620)!, 500, 800, MAIN)).toEqual({ author: 15 });
  // 800 - 80 (date) - 15 (subject) = 705 at most for Author.
  expect(dragWidths(AUTO, borderAt(layout, MAIN, 620)!, -2000, 800, MAIN)).toEqual({ author: 705 });
  expect(dragWidths(AUTO, borderAt(layout, MAIN, 720)!, 500, 800, MAIN)).toEqual({ author: 165, date: 15 });
});

test("read_saved_widths_keeps_only_known_positive_numbers", () => {
  expect(readSavedWidths({ author: 120, date: "80", hash: -1, subject: 50, extra: 3 })).toEqual({ author: 120 });
  expect(readSavedWidths(null)).toEqual({});
  expect(readSavedWidths({ date: Number.POSITIVE_INFINITY, hash: 64 })).toEqual({ hash: 64 });
});

test("a_narrow_table_does_not_snap_a_column_on_the_first_pixel", () => {
  // File history 250 wide: Author 0-150, Date 150-250, Subject 0 wide.
  const layout = tableLayout(250, HISTORY, AUTO_NARROW);
  const border = borderAt(layout, HISTORY, 250)!;
  expect(dragWidths(AUTO_NARROW, border, 1, 250, HISTORY)).toEqual({ date: 100 });
  expect(dragWidths(AUTO_NARROW, border, -1, 250, HISTORY)).toEqual({ date: 99 });
});

test("between_two_columns_neither_gets_wider_than_the_table", () => {
  const wide = { ...AUTO, author: 300, date: 300 };
  const layout = tableLayout(400, HISTORY, wide);
  const border = borderAt(layout, HISTORY, 300)!;
  const dragged = dragWidths(wide, border, 250, 400, HISTORY);
  expect(dragged.author).toBeLessThanOrEqual(400);
  expect((dragged.author ?? 0) + (dragged.date ?? 0)).toBe(600);
});
