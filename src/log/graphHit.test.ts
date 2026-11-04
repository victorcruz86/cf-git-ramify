import { expect, test } from "vitest";
import type { Arrow, PrintRow, ShortEdge } from "../lib/api";
import { isHighlighted, ROW_HEIGHT, type GraphLine, type GraphNode } from "./draw";
import { elementAt, jumpRow, targetOf } from "./graphHit";

const WIDTH = 16;
const MIDDLE = ROW_HEIGHT / 2;

function edge(overrides: Partial<ShortEdge> = {}): ShortEdge {
  return {
    x: 0,
    otherX: 0,
    up: false,
    color: 1,
    style: "usual",
    upRow: 3,
    downRow: 4,
    ...overrides,
  };
}

function arrow(overrides: Partial<Arrow> = {}): Arrow {
  return {
    x: 0,
    otherX: 0,
    up: false,
    color: 1,
    targetRow: 40,
    terminal: true,
    upRow: 3,
    downRow: 40,
    ...overrides,
  };
}

function print(overrides: Partial<PrintRow> = {}): PrintRow {
  return { nodeX: 5, nodeColor: 1, width: 12, edges: [], arrows: [], ...overrides };
}

test("click_on_the_node_beats_the_edges", () => {
  const row = print({ nodeX: 0, edges: [edge()] });

  expect(elementAt(row, 8, MIDDLE)?.kind).toBe("node");
  expect(elementAt(row, 8, MIDDLE + 8)?.kind).toBe("line");
});

test("outside_the_node_and_the_edges_there_is_nothing", () => {
  expect(elementAt(print({ edges: [edge()] }), 8 + WIDTH, MIDDLE)).toBeNull();
});

test("lower_edge_is_only_touchable_below_the_center", () => {
  const row = print({ edges: [edge()] });

  expect(elementAt(row, 8, MIDDLE + 4)?.kind).toBe("line");
  expect(elementAt(row, 8, MIDDLE - 4)).toBeNull();
});

test("upper_edge_is_only_touchable_above_the_center", () => {
  const row = print({ edges: [edge({ up: true })] });

  expect(elementAt(row, 8, MIDDLE - 4)?.kind).toBe("line");
  expect(elementAt(row, 8, MIDDLE + 4)).toBeNull();
});

test("diagonal_is_touched_on_its_line", () => {
  // From column 0 to 1 of the row below: at the row boundary it goes through the middle.
  const row = print({ edges: [edge({ otherX: 1 })] });

  expect(elementAt(row, 16, ROW_HEIGHT - 1)?.kind).toBe("line");
  expect(elementAt(row, 8, ROW_HEIGHT - 1)).toBeNull();
});

test("the_first_of_two_touching_edges_wins", () => {
  const first = edge({ color: 2 });
  const second = edge({ color: 3 });

  const found = elementAt(print({ edges: [first, second] }), 8, MIDDLE + 4) as GraphLine;

  expect(found.color).toBe(2);
});

test("terminal_arrow_has_the_touch_area_of_the_whole_edge", () => {
  const found = elementAt(print({ arrows: [arrow()] }), 8, ROW_HEIGHT - 1) as GraphLine;

  expect(found.terminal).toBe(true);
  expect(jumpRow(found)).toBe(40);
});

test("an_edge_with_an_arrow_jumps_and_the_others_do_not", () => {
  const withArrow = print({
    edges: [edge()],
    arrows: [arrow({ terminal: false, targetRow: 4, downRow: 4 })],
  });
  const plain = print({ edges: [edge()] });

  expect(jumpRow(elementAt(withArrow, 8, MIDDLE + 4)!)).toBe(4);
  expect(jumpRow(elementAt(plain, 8, MIDDLE + 4)!)).toBeNull();
  expect(jumpRow(elementAt(print({ nodeX: 0 }), 8, MIDDLE)!)).toBeNull();
});

test("what_is_sent_to_the_backend", () => {
  const node = elementAt(print({ nodeX: 0 }), 8, MIDDLE)!;
  const line = elementAt(print({ edges: [edge({ style: "dotted" })] }), 8, MIDDLE + 4)!;

  expect(targetOf(7, node)).toEqual({ kind: "node", row: 7 });
  expect(targetOf(7, line)).toEqual({ kind: "edge", up: 3, down: 4, style: "dotted" });
});

test("highlighting_an_arrow_marks_only_that_segment", () => {
  const row = print({
    edges: [edge(), edge({ up: true })],
    arrows: [arrow({ terminal: false, targetRow: 4, downRow: 4 })],
  });
  const hovered = elementAt(row, 8, MIDDLE + 4) as GraphLine;
  const highlight = { kind: "element", row: 9, element: hovered } as const;
  const up = elementAt(row, 8, MIDDLE - 4) as GraphLine;

  expect(isHighlighted(highlight, 9, hovered)).toBe(true);
  expect(isHighlighted(highlight, 8, hovered)).toBe(false);
  expect(isHighlighted(highlight, 9, up)).toBe(false);
});

test("highlighting_rows_marks_nodes_and_edges_with_both_ends_inside", () => {
  const highlight = { kind: "rows", rows: new Set([3, 4]) } as const;
  const node: GraphNode = { kind: "node", x: 0, color: 1 };
  const inside = elementAt(print({ edges: [edge()] }), 8, MIDDLE + 4) as GraphLine;
  const leaving = elementAt(print({ edges: [edge({ downRow: 5 })] }), 8, MIDDLE + 4) as GraphLine;

  expect(isHighlighted(highlight, 4, node)).toBe(true);
  expect(isHighlighted(highlight, 5, node)).toBe(false);
  expect(isHighlighted(highlight, 4, inside)).toBe(true);
  expect(isHighlighted(highlight, 4, leaving)).toBe(false);
});
