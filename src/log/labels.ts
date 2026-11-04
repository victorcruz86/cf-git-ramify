import type { LogRow, RefKind, SortType } from "../lib/api";

// `LabelPainter`. Its top and bottom margins and its radius belong to the label's gray background, which
// is not painted in the New UI.
export const LABEL_LEFT = 4;
export const LABEL_RIGHT = 4;
export const LABEL_COMPACT_GAP = 2;
export const LABEL_GAP = 12;
export const LABEL_ICON_GAP = 1;
const MAX_LENGTH = 22;
const TWO_DOTS = "..";
const SEPARATOR = "/";
const ELLIPSIS = "…";
// `LabelIcon.SIZE`, and the height of the labels' font, which is what the icon takes as its side.
const ICON_SIZE = 6.25;
export const LABEL_ICON_HEIGHT = 15;
// `GraphCommitCellRenderer.DISPLAYED_MESSAGE_PART`.
const DISPLAYED_MESSAGE_PART = 80;

// `VcsLogStandardColors`, in the dark theme.
const REF_COLORS: Record<RefKind, string> = {
  head: "#e1c731",
  local: "#3cb45c",
  remote: "#9f79b5",
  tag: "#999999",
};

export const refColor = (kind: RefKind) => REF_COLORS[kind];

export interface LabelGroup {
  name: string;
  colors: string[];
}

export interface Label {
  text: string;
  colors: string[];
}

export interface LabelLayout {
  labels: Label[];
  width: number;
}

type TextWidth = (text: string) => number;

// In Linear order what gets folded are the merges.
export const collapseLabel = (sort: SortType) =>
  sort === "linear" ? "Collapse Merges" : "Collapse Linear Branches";
export const expandLabel = (sort: SortType) =>
  sort === "linear" ? "Expand Merges" : "Expand Linear Branches";

// `SimpleRefGroup.getColors`.
function groupColors(kinds: RefKind[]): string[] {
  const unique = [...new Set(kinds)];
  return unique.flatMap((kind) => {
    const repeated = kinds.filter((other) => other === kind).length > 1;
    return repeated ? [REF_COLORS[kind], REF_COLORS[kind]] : [REF_COLORS[kind]];
  });
}

export function labelGroups(
  row: LogRow,
  { compactRefs, tagNames }: { compactRefs: boolean; tagNames: boolean },
): LabelGroup[] {
  const groups = compactRefs ? row.labels.compact : row.labels.normal;
  return groups.map(({ name, kinds }) => ({
    name: !tagNames && kinds[0] === "tag" ? "" : name,
    colors: groupColors(kinds),
  }));
}

// `StringUtil.shortenTextWithEllipsis` without a suffix.
const shortened = (text: string, length: number) =>
  text.length <= length ? text : text.slice(0, length - ELLIPSIS.length) + ELLIPSIS;

// `VcsLogUiUtil.shortenTextToFit`.
function shortenToFit(text: string, width: TextWidth, available: number): string {
  if (width(text) <= available) return text;
  for (let length = text.length; length > MAX_LENGTH; length--) {
    const result = shortened(text, length);
    if (width(result) <= available) return result;
  }
  return shortened(text, MAX_LENGTH);
}

// `LabelPainter.shortenRefName`.
export function shortenRefName(name: string, width: TextWidth, available: number): string {
  if (width(name) <= available || name.length <= MAX_LENGTH) return name;
  const separator = name.indexOf(SEPARATOR);
  const text = separator > TWO_DOTS.length ? TWO_DOTS + name.slice(separator) : name;
  return available > 0 ? shortenToFit(text, width, available) : shortened(text, MAX_LENGTH);
}

// `LabelIcon.getWidth`.
export const iconWidth = (colors: number) =>
  Math.round((ICON_SIZE + 2 * (colors - 1)) * (LABEL_ICON_HEIGHT / ICON_SIZE));

// `LabelPainter.getColors`: the colors that several groups bring together, repeating the one that repeats.
function mergedColors(groups: LabelGroup[]): string[] {
  const counts = new Map<string, number>();
  for (const color of groups.flatMap((group) => group.colors)) {
    counts.set(color, (counts.get(color) ?? 0) + 1);
  }
  return [...counts].flatMap(([color, count]) => (count > 1 ? [color, color] : [color]));
}

function compactLayout(groups: LabelGroup[], available: number, width: TextWidth): LabelLayout {
  let used = LABEL_LEFT + LABEL_RIGHT;
  const labels = groups.map((group, index) => {
    const gap = index === groups.length - 1 ? 0 : LABEL_COMPACT_GAP;
    const next = used + iconWidth(group.colors.length) + gap;
    const text = shortenRefName(group.name, width, available - next);
    used = next + width(text) + LABEL_ICON_GAP;
    return { text, colors: group.colors };
  });
  return { labels, width: used };
}

function longLayout(groups: LabelGroup[], available: number, width: TextWidth): LabelLayout {
  let used = LABEL_LEFT + LABEL_RIGHT;
  const labels: Label[] = [];
  for (let index = 0; index < groups.length; index++) {
    const group = groups[index];
    const last = index === groups.length - 1;
    const rest = last ? 0 : iconWidth(mergedColors(groups.slice(index + 1)).length);
    let next = used + iconWidth(group.colors.length) + (last ? 0 : LABEL_GAP);
    const text = shortenRefName(group.name, width, available - next - rest);
    next += width(text) + LABEL_ICON_GAP;
    if (available - next - rest < 0) {
      const colors = mergedColors(groups.slice(index));
      const name = labels.length === 0 ? text : "";
      labels.push({ text: name, colors });
      used += width(name) + iconWidth(colors.length);
      break;
    }
    labels.push({ text, colors: group.colors });
    used = next;
  }
  return { labels, width: used };
}

// `LabelPainter.calculatePresentation`.
export function layoutLabels(
  groups: LabelGroup[],
  compact: boolean,
  available: number,
  width: TextWidth,
): LabelLayout {
  if (groups.length === 0) return { labels: [], width: 0 };
  return compact ? compactLayout(groups, available, width) : longLayout(groups, available, width);
}

// `GraphCommitCellRenderer.getAvailableWidth`.
export function availableLabelWidth(cell: number, text: number, compact: boolean): number {
  const free = cell - text;
  const allowed = compact
    ? Math.min(free, Math.trunc(cell / 3))
    : Math.max(free, Math.max(Math.trunc(cell / 2), cell - DISPLAYED_MESSAGE_PART));
  return Math.max(0, allowed);
}
