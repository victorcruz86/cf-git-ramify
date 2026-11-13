import type { BranchList, FavoriteSet, LogRef } from "../lib/api";
import type { MenuAction, MenuItem } from "../lib/menu";
import { COPY_BRANCH_NAME } from "./copyName";
import { speedMatch, type SpeedMatch } from "../lib/speedSearch";
import { shortHash } from "../log/draw";
import {
  comparisonItems,
  isProtected,
  truncated,
  type RefOperation,
  type VcsAction,
} from "../log/contextMenu";

export type Section = "recent" | "local" | "remote";

export interface BranchNode {
  kind: "branch";
  id: string;
  section: Section;
  name: string;
  label: string;
  current: boolean;
  favorite: boolean;
  tracked: string | null;
  match: SpeedMatch | null;
}

export interface GroupNode {
  kind: "section" | "folder";
  id: string;
  label: string;
  children: TreeNode[];
}

export type TreeNode = BranchNode | GroupNode;

export interface TreeOptions {
  favorites: FavoriteSet;
  // `GROUPING_BY_DIRECTORY`.
  grouping: boolean;
  // `showRecentBranches`.
  recent: boolean;
  // The log panel sorts with `BranchTreeNodeComparator`.
  panel?: boolean;
}

interface Branch {
  name: string;
  tracked: string | null;
  current: boolean;
  favorite: boolean;
  match: SpeedMatch | null;
}

const SEPARATOR = null;
const SLASH = "/";
const SECTION_LABELS: Record<Section, string> = {
  recent: "Recent",
  local: "Local",
  remote: "Remote",
};

const byCodeUnits = (a: string, b: string) => (a < b ? -1 : a > b ? 1 : 0);

// `Comparator.then`: `false` goes first.
const compareFlags = (flags: (branch: Branch) => boolean[]) => (a: Branch, b: Branch) => {
  const [first, second] = [flags(a), flags(b)];
  const index = first.findIndex((flag, position) => flag !== second[position]);
  return index < 0 ? byCodeUnits(a.name, b.name) : Number(first[index]) - Number(second[index]);
};

// `getBranchComparator`: `!(grouping && contains('/'))` puts what contains a slash first.
const branchComparator = (grouping: boolean) =>
  compareFlags((branch) => [
    !branch.current,
    !branch.favorite,
    !(grouping && branch.name.includes(SLASH)),
  ]);

const branchId = (section: Section, name: string) => `${section}:${name}`;

function leaf(section: Section, branch: Branch, label: string): BranchNode {
  return { kind: "branch", id: branchId(section, branch.name), section, label, ...branch };
}

// `getSubTreeComparator`: favorites first, then folders, then the rest. It is stable.
function subTreeWeight(node: TreeNode): number {
  if (node.kind !== "branch") return 1;
  return node.current || node.favorite ? 0 : 2;
}

// `buildSubTree`: each name takes the place of its first appearance.
function subTree(
  section: Section,
  branches: { parts: string[]; branch: Branch }[],
  prefix: string[],
  panel: boolean,
): TreeNode[] {
  const level = new Map<string, Branch | typeof branches>();
  for (const { parts, branch } of branches) {
    const [first, ...rest] = parts;
    if (rest.length === 0) level.set(first, branch);
    else {
      const known = level.get(first);
      const group = Array.isArray(known) ? known : [];
      group.push({ parts: rest, branch });
      level.set(first, group);
    }
  }
  const nodes = [...level].map(([name, value]): TreeNode => {
    if (!Array.isArray(value)) return leaf(section, value, name);
    const path = [...prefix, name];
    return {
      kind: "folder",
      id: `${section}${SLASH}${path.join(SLASH)}`,
      label: name,
      children: subTree(section, value, path, panel),
    };
  });
  if (panel) return nodes.sort(panelOrder);
  return prefix.length > 0
    ? nodes.sort((a, b) => subTreeWeight(a) - subTreeWeight(b))
    : nodes;
}

// `BranchTreeNodeComparator`.
function panelWeight(node: TreeNode): number {
  if (node.kind !== "branch") return 2;
  if (node.current) return 0;
  return node.favorite ? 1 : 3;
}

const panelOrder = (a: TreeNode, b: TreeNode) =>
  panelWeight(a) - panelWeight(b) || byCodeUnits(a.label, b.label);

function sectionNode(
  id: Section,
  branches: Branch[],
  grouping: boolean,
  panel: boolean,
): GroupNode | null {
  if (branches.length === 0) return null;
  const parts = branches.map((branch) => ({
    parts: grouping ? branch.name.split(SLASH) : [branch.name],
    branch,
  }));
  const children = subTree(id, parts, [], panel);
  return { kind: "section", id, label: SECTION_LABELS[id], children };
}

// `setSearchPattern`: a lone slash does not filter.
const patternOf = (query: string) => {
  const pattern = query.trim();
  return pattern === SLASH ? "" : pattern;
};

// A row of the popup that runs something: the top actions and those of the ongoing operation.
export interface ActionRow {
  id: string;
  label: string;
  shortcut?: string;
  run: (() => void) | undefined;
}

// `ActionItem` filtering of `GitBranchesTreeModelUtil.match`: the same matcher as the branches, and the
// best one is the highest degree (the first on a tie).
export function filterActions(
  rows: ActionRow[],
  query: string,
): { rows: ActionRow[]; best: ActionRow | null } {
  const pattern = patternOf(query);
  if (!pattern) return { rows, best: null };
  let best: ActionRow | null = null;
  let top = -Infinity;
  const matching = rows.filter((row) => {
    const match = speedMatch(row.label, pattern);
    if (match && match.degree > top) {
      top = match.degree;
      best = row;
    }
    return match !== null;
  });
  return { rows: matching, best };
}

export function buildTree(list: BranchList, query: string, options: TreeOptions): TreeNode[] {
  const { favorites, grouping, recent, panel = false } = options;
  const pattern = patternOf(query);
  const make = (name: string, tracked: string | null, local: boolean): Branch => ({
    name,
    tracked,
    current: local && name === list.current,
    favorite: (local ? favorites.local : favorites.remote).includes(name),
    match: pattern ? speedMatch(name, pattern) : null,
  });
  const matches = (branch: Branch) => !pattern || branch.match !== null;
  const local = list.local.map(({ name, tracked }) => make(name, tracked, true));
  const remote = list.remote.map((name) => make(name, null, false));
  const recentNames = recent ? list.recent : [];
  const recentBranches = recentNames.flatMap(
    (name) => local.filter((branch) => branch.name === name).slice(0, 1),
  );
  const comparator = branchComparator(grouping);
  // They only leave Local when filtering: `LazyBranchesSubtreeHolder.match` is what checks it.
  const inLocal = (branch: Branch) => !pattern || !recentNames.includes(branch.name);
  return [
    sectionNode("recent", recentBranches.filter(matches), grouping, panel),
    sectionNode("local", [...local].sort(comparator).filter(matches).filter(inLocal), grouping, panel),
    sectionNode("remote", [...remote].sort(comparator).filter(matches), grouping, panel),
  ].filter((node) => node !== null);
}

function walk(nodes: TreeNode[], visit: (node: TreeNode, path: GroupNode[]) => void, path: GroupNode[] = []) {
  for (const node of nodes) {
    visit(node, path);
    if (node.kind !== "branch") walk(node.children, visit, [...path, node]);
  }
}

export function branchesOf(nodes: TreeNode[]): BranchNode[] {
  const found: BranchNode[] = [];
  walk(nodes, (node) => {
    if (node.kind === "branch") found.push(node);
  });
  return found;
}

export const findBranch = (tree: TreeNode[], id: string) =>
  branchesOf(tree).find((branch) => branch.id === id) ?? null;

// `createTreePathFor` looks for a local branch in Recent first.
function located(tree: TreeNode[], name: string): BranchNode | null {
  const all = branchesOf(tree).filter((branch) => branch.name === name);
  return all.find(({ section }) => section === "recent") ?? all[0] ?? null;
}

// `getPreferredBranch`. When filtering, the best one in Recent wins, then Local, then Remote;
// without a filter, the branch we came from or the current one.
export function preferredId(tree: TreeNode[], list: BranchList, query: string): string | null {
  if (patternOf(query)) {
    for (const section of tree) {
      let top: BranchNode | null = null;
      for (const branch of branchesOf([section])) {
        if ((branch.match?.degree ?? 0) > (top?.match?.degree ?? -1)) top = branch;
      }
      if (top) return top.id;
    }
    return null;
  }
  const wanted = list.previous ? located(tree, list.previous) : null;
  const current = list.current ? located(tree, list.current) : null;
  return (wanted ?? current)?.id ?? null;
}

function ancestors(tree: TreeNode[], id: string): string[] {
  let found: string[] = [];
  walk(tree, (node, path) => {
    if (node.id === id) found = path.map((group) => group.id);
  });
  return found;
}

// `traverseNodesAndExpand` and `selectPreferred`: what opens by itself. Since `createTreePathFor` takes
// every local branch to Recent if that section exists, Local only opens when there are no recent ones.
export function autoExpanded(tree: TreeNode[], list: BranchList, query: string): Set<string> {
  const wanted = branchesOf(tree)
    .filter(({ section }) => section === "recent")
    .map(({ id }) => id);
  const current = list.current ? located(tree, list.current) : null;
  if (current) wanted.push(current.id);
  const preferred = preferredId(tree, list, query);
  if (preferred) wanted.push(preferred);
  return new Set(wanted.flatMap((id) => ancestors(tree, id)));
}

export function pathTo(tree: TreeNode[], id: string): string[] {
  return ancestors(tree, id);
}

export interface VisibleRow {
  node: TreeNode;
  depth: number;
}

export function visibleRows(tree: TreeNode[], expanded: Set<string>, depth = 0): VisibleRow[] {
  return tree.flatMap((node) => [
    { node, depth },
    ...(node.kind !== "branch" && expanded.has(node.id)
      ? visibleRows(node.children, expanded, depth + 1)
      : []),
  ]);
}

// `selectNextLeaf`: when filtering, the arrows go from branch to branch even if they are collapsed.
export function nextLeaf(tree: TreeNode[], id: string | null, step: number): BranchNode | null {
  const leaves = branchesOf(tree);
  if (leaves.length === 0) return null;
  const position = leaves.findIndex((branch) => branch.id === id);
  if (position < 0) return leaves[step > 0 ? 0 : leaves.length - 1];
  return leaves[(position + step + leaves.length) % leaves.length];
}

export interface BranchState {
  current: string | null;
  remotes: boolean;
  onvcs?: (action: VcsAction) => void;
  protectedBranches?: string[];
  // With a detached HEAD, `current` is `null` and the current one is named with the short hash.
  head?: string | null;
  // `Copy Branch Name`, which IntelliJ does not have (`src/branches/copyName.ts`).
  oncopy?: (name: string) => void;
}

const quoted = (name: string) => `'${truncated(name)}'`;
// `branches.rebase.is.not.possible.in.the.detached.head.state`.
const DETACHED = "Rebase is not possible in the detached HEAD state";

// `GitBranchPopupActions`, group `Git.Branch` (`X:236-254`).
export function branchActions(branch: BranchNode, state: BranchState): MenuItem[] {
  const { current, remotes, onvcs, oncopy, protectedBranches = [], head = null } = state;
  const { name, section, tracked } = branch;
  const remote = section === "remote";
  const ref: LogRef = { name, kind: remote ? "remote" : "local" };
  const item = (
    label: string,
    operation: RefOperation,
    off = false,
    title?: string,
  ): MenuAction => ({
    label,
    disabled: off || !onvcs,
    title: off ? title : undefined,
    run: () => onvcs?.({ kind: "ref", operation, ref }),
  });
  // `getCurrentBranchTruncatedPresentation`. Without the hash at hand, `HEAD`.
  const target = quoted(current ?? (head ? shortHash(head) : "HEAD"));
  const selected = quoted(name);
  const newBranch = item(`New Branch from '${truncated(name)}'…`, "newBranch");
  const isCurrent = !remote && name === current;
  // `disabledForCurrent`: Checkout and Rebase, Rebase and Merge.
  const withCurrent: MenuItem[] = isCurrent
    ? []
    : [
        item(`Rebase ${target} onto ${selected}`, "rebase", current === null, DETACHED),
        item(`Merge ${selected} into ${target}`, "merge"),
        SEPARATOR,
      ];
  const checkoutAndRebase = isCurrent
    ? []
    : [item(`Checkout and Rebase onto ${target}`, "checkoutAndRebase")];
  // `getCurrentBranchFullPresentation` names a detached HEAD by its short hash; the range starts
  // at `HEAD` then.
  const comparisons = comparisonItems(name, {
    start: current ?? "HEAD",
    current: target,
    currentName: current ?? (head ? shortHash(head) : "HEAD"),
    compare: !isCurrent,
    // Without commits HEAD has no hash (`repo_state` gives it empty).
    fresh: head === "",
  });
  // Ramify's own last row: copying needs no repository operation, so `onvcs` does not turn it off.
  const copy: MenuItem[] = oncopy ? [SEPARATOR, { label: COPY_BRANCH_NAME, run: () => oncopy(name) }] : [];
  if (remote) {
    return [
      item("Checkout", "checkout"),
      newBranch,
      ...checkoutAndRebase,
      SEPARATOR,
      ...comparisons,
      ...withCurrent,
      item(`Pull into ${target} Using Merge`, "pullMerge"),
      item(`Pull into ${target} Using Rebase`, "pullRebase"),
      SEPARATOR,
      item("Delete", "delete", isProtected(ref, protectedBranches)),
      ...copy,
    ];
  }
  const update = item("Update", "update", tracked === null);
  return [
    ...(isCurrent ? [] : [item("Checkout", "checkout")]),
    newBranch,
    ...checkoutAndRebase,
    SEPARATOR,
    ...comparisons,
    ...withCurrent,
    ...(remotes ? [update] : []),
    item("Push…", "push"),
    SEPARATOR,
    item("Rename…", "rename"),
    ...(isCurrent ? [] : [item("Delete", "delete")]),
    ...copy,
  ];
}

// `constructIncomingOutgoingTooltip`.
export function syncTooltip(incoming: boolean, outgoing: boolean): string {
  if (incoming && outgoing) return "There are incoming and outgoing commits";
  if (incoming) return "There are incoming commits";
  return outgoing ? "There are outgoing commits" : "";
}

// The row in pieces, with what matches marked: `applySpeedSearchHighlightingFiltered`.
export function highlighted(label: string, query: string): { text: string; hit: boolean }[] {
  const pattern = patternOf(query);
  const match = pattern ? speedMatch(label, pattern) : null;
  if (!match) return [{ text: label, hit: false }];
  const parts: { text: string; hit: boolean }[] = [];
  let position = 0;
  for (const [start, end] of match.fragments) {
    if (start > position) parts.push({ text: label.slice(position, start), hit: false });
    parts.push({ text: label.slice(start, end), hit: true });
    position = end;
  }
  if (position < label.length) parts.push({ text: label.slice(position), hit: false });
  return parts;
}

export const refOf = (branch: BranchNode): LogRef => ({
  name: branch.name,
  kind: branch.section === "remote" ? "remote" : "local",
});
