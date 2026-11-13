import { describe, expect, it } from "vitest";
import {
  dividers,
  focusAfterClosing,
  MAX_RATIO,
  MIN_RATIO,
  nextTitle,
  panes,
  ratioAt,
  rects,
  remove,
  setRatio,
  split,
  type Layout,
} from "./layout";

const pane = (id: number): Layout => ({ kind: "pane", id });

describe("titles", () => {
  it("names the first session Local and the next ones with the first free number", () => {
    expect(nextTitle([])).toBe("Local");
    expect(nextTitle(["Local"])).toBe("Local (2)");
    expect(nextTitle(["Local", "Local (2)"])).toBe("Local (3)");
    expect(nextTitle(["Local", "Local (3)"])).toBe("Local (2)");
    expect(nextTitle(["Local (2)"])).toBe("Local");
  });
});

describe("splits", () => {
  it("splitting right puts the new pane to the right, half and half", () => {
    const layout = split(pane(1), 1, "right", 2, 10);
    expect(panes(layout)).toEqual([1, 2]);
    expect(rects(layout)).toEqual([
      { id: 1, rect: { x: 0, y: 0, width: 0.5, height: 1 } },
      { id: 2, rect: { x: 0.5, y: 0, width: 0.5, height: 1 } },
    ]);
  });

  it("splitting down puts the new pane below", () => {
    const layout = split(pane(1), 1, "down", 2, 10);
    expect(rects(layout)[1]).toEqual({ id: 2, rect: { x: 0, y: 0.5, width: 1, height: 0.5 } });
  });

  it("a split inside a split only divides the pane it was asked for", () => {
    const layout = split(split(pane(1), 1, "right", 2, 10), 2, "down", 3, 11);
    expect(panes(layout)).toEqual([1, 2, 3]);
    expect(rects(layout)).toEqual([
      { id: 1, rect: { x: 0, y: 0, width: 0.5, height: 1 } },
      { id: 2, rect: { x: 0.5, y: 0, width: 0.5, height: 0.5 } },
      { id: 3, rect: { x: 0.5, y: 0.5, width: 0.5, height: 0.5 } },
    ]);
  });

  it("splitting a pane that is not there changes nothing", () => {
    expect(split(pane(1), 7, "right", 2, 10)).toEqual(pane(1));
  });
});

describe("closing", () => {
  it("the sibling takes the place of the split", () => {
    const layout = split(split(pane(1), 1, "right", 2, 10), 2, "down", 3, 11);
    const left = remove(layout, 2)!;
    expect(panes(left)).toEqual([1, 3]);
    expect(rects(left)[1]).toEqual({ id: 3, rect: { x: 0.5, y: 0, width: 0.5, height: 1 } });
  });

  it("closing the only pane leaves nothing", () => {
    expect(remove(pane(1), 1)).toBeNull();
  });

  it("the keyboard goes to the pane before the closed one, or the next one", () => {
    const layout = split(split(pane(1), 1, "right", 2, 10), 2, "right", 3, 11);
    expect(focusAfterClosing(layout, 2)).toBe(1);
    expect(focusAfterClosing(layout, 1)).toBe(2);
    expect(focusAfterClosing(pane(1), 1)).toBeNull();
  });
});

describe("dividers", () => {
  it("there is one per split, where its two sides meet", () => {
    const layout = split(split(pane(1), 1, "right", 2, 10), 2, "down", 3, 11);
    expect(dividers(layout)).toEqual([
      { split: 10, direction: "right", area: { x: 0, y: 0, width: 1, height: 1 }, at: 0.5 },
      { split: 11, direction: "down", area: { x: 0.5, y: 0, width: 0.5, height: 1 }, at: 0.5 },
    ]);
  });

  it("dragging a divider sets its split's ratio within its own area, within limits", () => {
    const layout = split(split(pane(1), 1, "right", 2, 10), 2, "right", 3, 11);
    const inner = dividers(layout).find((divider) => divider.split === 11)!;
    expect(ratioAt(inner, 0.75)).toBeCloseTo(0.5);
    const moved = setRatio(layout, 11, ratioAt(inner, 0.6));
    expect(rects(moved)[1].rect.width).toBeCloseTo(0.1);
    expect(setRatio(layout, 10, 2)).toMatchObject({ ratio: MAX_RATIO });
    expect(setRatio(layout, 10, -1)).toMatchObject({ ratio: MIN_RATIO });
  });
});
