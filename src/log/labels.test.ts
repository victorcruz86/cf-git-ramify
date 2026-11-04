import { expect, test } from "vitest";
import type { LogRow, RefKind } from "../lib/api";
import {
  collapseLabel,
  expandLabel,
  labelGroups,
  iconWidth,
  layoutLabels,
  shortenRefName,
  type LabelGroup,
} from "./labels";

const HEAD = "#e1c731";
const LOCAL = "#3cb45c";
const REMOTE = "#9f79b5";
const TAG = "#999999";

const textWidth = (text: string) => text.length * 7;

function row(
  compact: { name: string; kinds: RefKind[] }[],
  normal: { name: string; kinds: RefKind[] }[] = compact,
): LogRow {
  return { labels: { compact, normal } } as unknown as LogRow;
}

const TRACKED = row(
  [{ name: "origin & main", kinds: ["head", "local", "remote", "tag"] }],
  [
    { name: "origin & main", kinds: ["head", "local", "remote"] },
    { name: "v1", kinds: ["tag"] },
  ],
);

test("compact_label_joins_groups", () => {
  const groups = labelGroups(TRACKED, { compactRefs: true, tagNames: false });

  expect(groups).toEqual([{ name: "origin & main", colors: [HEAD, LOCAL, REMOTE, TAG] }]);
});

test("normal_label_one_per_group", () => {
  const groups = labelGroups(TRACKED, { compactRefs: false, tagNames: false });

  expect(groups).toEqual([
    { name: "origin & main", colors: [HEAD, LOCAL, REMOTE] },
    { name: "", colors: [TAG] },
  ]);
});

test("several_references_of_a_type_are_two_colors", () => {
  const groups = labelGroups(row([{ name: "a", kinds: ["local", "local", "local", "tag"] }]), {
    compactRefs: true,
    tagNames: false,
  });

  expect(groups[0].colors).toEqual([LOCAL, LOCAL, TAG]);
});

test("long_name_is_shortened_to_22", () => {
  const name = "feature/a-branch-with-a-long-name";

  const short = shortenRefName(name, textWidth, 100);

  expect(short).toHaveLength(22);
  expect(short.startsWith("../a-branch")).toBe(true);
  expect(short.endsWith("…")).toBe(true);
});

test("long_name_that_fits_or_of_22_is_not_shortened", () => {
  const name = "feature/a-branch-with-a-long-name";

  expect(shortenRefName(name, textWidth, 1000)).toBe(name);
  expect(shortenRefName("a".repeat(22), textWidth, 10)).toBe("a".repeat(22));
});

test("show_tag_names_shows_the_name", () => {
  const groups = labelGroups(TRACKED, { compactRefs: false, tagNames: true });

  expect(groups[1].name).toBe("v1");
  const tagOnly = row([{ name: "v2", kinds: ["tag"] }]);
  expect(labelGroups(tagOnly, { compactRefs: true, tagNames: false })[0].name).toBe("");
  expect(labelGroups(tagOnly, { compactRefs: true, tagNames: true })[0].name).toBe("v2");
});

const group = (name: string, colors = [LOCAL]): LabelGroup => ({ name, colors });

test("compact_with_icons_and_label_painter_measures", () => {
  const { labels, width } = layoutLabels([group("main", [HEAD, LOCAL])], true, 500, textWidth);

  expect(labels).toEqual([{ text: "main", colors: [HEAD, LOCAL] }]);
  // Margin of 4 on each side, two-color icon, separation of 1 and the text.
  expect(width).toBe(4 + 4 + iconWidth(2) + 1 + 28);
});

test("normal_what_does_not_fit_is_joined_in_an_icon_without_a_name", () => {
  const { labels } = layoutLabels([group("one"), group("two"), group("three")], false, 60, textWidth);

  expect(labels).toEqual([{ text: "one", colors: [LOCAL, LOCAL] }]);
});

test("in_linear_order_the_buttons_talk_about_merges", () => {
  expect(collapseLabel("linear")).toBe("Collapse Merges");
  expect(expandLabel("linear")).toBe("Expand Merges");
});

test("in_the_other_orders_they_talk_about_linear_branches", () => {
  for (const sort of ["off", "standard"] as const) {
    expect(collapseLabel(sort)).toBe("Collapse Linear Branches");
    expect(expandLabel(sort)).toBe("Expand Linear Branches");
  }
});
