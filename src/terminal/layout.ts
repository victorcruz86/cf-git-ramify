// The Terminal tab's model, without xterm: tabs of sessions, each tab a tree of splits whose
// leaves are panes. A pane is one shell session. The panes are drawn absolutely, from the
// rectangles of the tree, so that splitting never moves a pane to another parent and its terminal
// is never built again.

export type Direction = "right" | "down";

export type Layout =
  | { kind: "pane"; id: number }
  | { kind: "split"; id: number; direction: Direction; ratio: number; first: Layout; second: Layout };

export interface Tab {
  id: number;
  title: string;
  layout: Layout;
  // The pane that gets the keyboard.
  focused: number;
}

// A rectangle in fractions of the tab's area.
export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface Divider {
  // The split it resizes, and the area that split covers, to turn a pointer into a ratio.
  split: number;
  direction: Direction;
  area: Rect;
  // Where the line between the two sides is, in fractions of the tab's area.
  at: number;
}

export const MIN_RATIO = 0.1;
export const MAX_RATIO = 0.9;
// The first session's name, as IntelliJ's local terminal names it.
export const BASE_TITLE = "Local";

// `Local`, then `Local (2)`, `Local (3)`… with the first number nobody uses.
export function nextTitle(taken: readonly string[]): string {
  if (!taken.includes(BASE_TITLE)) return BASE_TITLE;
  for (let n = 2; ; n++) {
    const title = `${BASE_TITLE} (${n})`;
    if (!taken.includes(title)) return title;
  }
}

export function panes(layout: Layout): number[] {
  return layout.kind === "pane" ? [layout.id] : [...panes(layout.first), ...panes(layout.second)];
}

// `target` becomes two panes: itself, and `added` to its right or below it, half and half.
export function split(
  layout: Layout,
  target: number,
  direction: Direction,
  added: number,
  splitId: number,
): Layout {
  if (layout.kind === "pane") {
    if (layout.id !== target) return layout;
    return {
      kind: "split",
      id: splitId,
      direction,
      ratio: 0.5,
      first: layout,
      second: { kind: "pane", id: added },
    };
  }
  return {
    ...layout,
    first: split(layout.first, target, direction, added, splitId),
    second: split(layout.second, target, direction, added, splitId),
  };
}

// The tree without `target`: its sibling takes the place of their split. `null` when it was the
// only pane.
export function remove(layout: Layout, target: number): Layout | null {
  if (layout.kind === "pane") return layout.id === target ? null : layout;
  const first = remove(layout.first, target);
  const second = remove(layout.second, target);
  if (first === null) return second;
  if (second === null) return first;
  return { ...layout, first, second };
}

export function setRatio(layout: Layout, splitId: number, ratio: number): Layout {
  if (layout.kind === "pane") return layout;
  const clamped = Math.min(MAX_RATIO, Math.max(MIN_RATIO, ratio));
  if (layout.id === splitId) return { ...layout, ratio: clamped };
  return {
    ...layout,
    first: setRatio(layout.first, splitId, ratio),
    second: setRatio(layout.second, splitId, ratio),
  };
}

const WHOLE: Rect = { x: 0, y: 0, width: 1, height: 1 };

function halves(area: Rect, direction: Direction, ratio: number): [Rect, Rect] {
  if (direction === "right") {
    const left = area.width * ratio;
    return [
      { ...area, width: left },
      { ...area, x: area.x + left, width: area.width - left },
    ];
  }
  const top = area.height * ratio;
  return [
    { ...area, height: top },
    { ...area, y: area.y + top, height: area.height - top },
  ];
}

// Where each pane goes, in tree order.
export function rects(layout: Layout, area: Rect = WHOLE): { id: number; rect: Rect }[] {
  if (layout.kind === "pane") return [{ id: layout.id, rect: area }];
  const [first, second] = halves(area, layout.direction, layout.ratio);
  return [...rects(layout.first, first), ...rects(layout.second, second)];
}

export function dividers(layout: Layout, area: Rect = WHOLE): Divider[] {
  if (layout.kind === "pane") return [];
  const [first, second] = halves(area, layout.direction, layout.ratio);
  const at = layout.direction === "right" ? second.x : second.y;
  return [
    { split: layout.id, direction: layout.direction, area, at },
    ...dividers(layout.first, first),
    ...dividers(layout.second, second),
  ];
}

// The ratio a pointer at `position` (a fraction of the tab's area, along the divider's axis) gives.
export function ratioAt(divider: Divider, position: number): number {
  const [start, size] =
    divider.direction === "right"
      ? [divider.area.x, divider.area.width]
      : [divider.area.y, divider.area.height];
  return size > 0 ? (position - start) / size : 0.5;
}

// The pane that takes the keyboard when `closed` goes: the one before it, or the next one.
export function focusAfterClosing(layout: Layout, closed: number): number | null {
  const order = panes(layout);
  const index = order.indexOf(closed);
  const rest = order.filter((id) => id !== closed);
  if (rest.length === 0) return null;
  return rest[Math.max(0, index - 1)];
}
