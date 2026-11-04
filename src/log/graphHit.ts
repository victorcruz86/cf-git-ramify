import type { GraphTarget, PrintRow } from "../lib/api";
import { center, graphElements, METRICS, nodeAt, type GraphElement, type GraphLine } from "./draw";

const distance = (x1: number, y1: number, x2: number, y2: number) => Math.hypot(x1 - x2, y1 - y2);

// `PositionUtil.overUpEdge` and `overDownEdge`: inside the ellipse with foci at the two centers.
function overLine(line: GraphLine, x: number, y: number): boolean {
  const { rowHeight, line: thickness } = METRICS;
  const middle = Math.trunc(rowHeight / 2);
  const fromX = center(line.x, METRICS);
  const toX = center(line.otherX, METRICS);
  const toY = line.up ? -middle : rowHeight + middle;
  const length = distance(fromX, middle, toX, toY);
  return distance(fromX, middle, x, y) + distance(toX, toY, x, y) < length + thickness;
}

// `SimpleGraphCellPainter.getElementUnderCursor`: first the node, and of the edges the first one.
export function elementAt(print: PrintRow, x: number, y: number): GraphElement | null {
  const elements = graphElements(print);
  if (nodeAt(print, x, y)) return elements.find(({ kind }) => kind === "node") ?? null;
  for (const element of elements) {
    if (element.kind === "line" && overLine(element, x, y)) return element;
  }
  return null;
}

// `VisibleGraphImpl.performArrowAction`: an arrow leads to the other end of its edge.
export const jumpRow = (element: GraphElement) =>
  element.kind === "line" ? element.jump : null;

export function targetOf(row: number, element: GraphElement): GraphTarget {
  if (element.kind === "node") return { kind: "node", row };
  return { kind: "edge", up: element.upRow, down: element.downRow, style: element.style };
}
