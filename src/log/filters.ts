import type { FavoriteSet, LogFilter, LogRef, Person } from "../lib/api";
import { speedMatches } from "../lib/speedSearch";

// `VcsLogFilterObject.ME`.
export const ME = "*";
const ME_LABEL = "me";
// `MultipleValueFilterPopupComponent`.
const MAX_VALUE_LENGTH = 20;
const SEPARATORS = ["|", "\n"];
const TWO_DOTS = "..";
const LOCAL = "Local";
// `VcsLogUtil.isFullHash` and `isRegexp`.
const FULL_HASH = /^[a-fA-F0-9]{40}$/;
const REGEX_CHARS = /[()[\]{}.*?+^$\\|]/;
// `VcsUserUtil.NAME_PATTERN`.
const NAME_PATTERN = /^(\w+)[!-/:-@[-`{-~\s](\w+)$/;
const PRINTABLE_ASCII = /^[ -~]*$/;

const dateFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "short" });

export interface FilterState {
  text: string;
  regex: boolean;
  matchCase: boolean;
  // What the branches popup separates: IntelliJ's `BranchFilters`.
  branches: string[];
  revisions: string[];
  ranges: [string, string][];
  users: string[];
  since: number | null;
  until: number | null;
  paths: string[];
}

export const emptyFilters = (): FilterState => ({
  text: "",
  regex: false,
  matchCase: false,
  branches: [],
  revisions: [],
  ranges: [],
  users: [],
  since: null,
  until: null,
  paths: [],
});

export function toLogFilter(state: FilterState): LogFilter {
  const { text, regex, matchCase, branches, revisions, ranges, users, since, until, paths } = state;
  return {
    branches,
    revisions,
    ranges,
    users,
    since,
    until,
    paths,
    text: text.trim() ? { text, regex, matchCase } : null,
    hashes: [],
  };
}

// `BranchFilterModel.createFilterFromPresentation`.
export function splitBranchValues(
  values: string[],
): Pick<FilterState, "branches" | "revisions" | "ranges"> {
  const parts: Pick<FilterState, "branches" | "revisions" | "ranges"> = {
    branches: [],
    revisions: [],
    ranges: [],
  };
  for (const value of values) {
    const dots = value.indexOf(TWO_DOTS);
    if (dots > 0 && dots === value.lastIndexOf(TWO_DOTS)) {
      parts.ranges.push([value.slice(0, dots), value.slice(dots + TWO_DOTS.length)]);
    } else if (FULL_HASH.test(value)) {
      parts.revisions.push(value);
    } else {
      parts.branches.push(value);
    }
  }
  return parts;
}

// `BranchFilterModel.getFilterPresentation`.
export const branchValues = ({ branches, revisions, ranges }: FilterState) => [
  ...[...branches].sort(),
  ...revisions,
  ...ranges.map(([exclusive, inclusive]) => `${exclusive}${TWO_DOTS}${inclusive}`),
];

export const userLabel = (value: string) => (value === ME ? ME_LABEL : value);
// `getLocalizedValues` and `parseLocalizedValues`.
export const localizedUsers = (values: string[]) => values.map(userLabel);
export const parseUsers = (values: string[]) =>
  values.map((value) => (value === ME_LABEL ? ME : value));

const seconds = (date: Date) => Math.floor(date.getTime() / 1000);

function daysBefore(now: Date, days: number): number {
  const date = new Date(now);
  date.setDate(date.getDate() - days);
  return seconds(date);
}

export const lastDay = (now: Date) => daysBefore(now, 1);
export const lastWeek = (now: Date) => daysBefore(now, 7);

export function dateText(since: number | null, until: number | null): string {
  const format = (value: number) => dateFormat.format(value * 1000);
  if (since !== null && until !== null) return `${format(since)}-${format(until)}`;
  if (since !== null) return `Since ${format(since)}`;
  return until !== null ? `Until ${format(until)}` : "";
}

export function parseValues(text: string, separators = SEPARATORS): string[] {
  const split = separators.reduce(
    (parts, separator) => parts.flatMap((part) => part.split(separator)),
    [text],
  );
  return split.map((value) => value.trim()).filter(Boolean);
}

// `MultipleValueFilterPopupComponent.displayableText`.
export function displayText(values: string[], maxLength = MAX_VALUE_LENGTH): string {
  const text = values.join("|");
  return text.length > maxLength ? `${text.slice(0, maxLength - 1)}…` : text;
}

// `StructureFilterPopupComponent.getText`: the first one in order and how many more there are.
export function pathsText(paths: string[]): string {
  const [first] = [...paths].sort();
  return paths.length > 1 ? `${first} + ${paths.length - 1}` : (first ?? "");
}

const capitalized = (name: string) =>
  PRINTABLE_ASCII.test(name) ? name.charAt(0).toUpperCase() + name.slice(1) : name;

// `UserFilterPopupComponent.collectUsers`.
export function collectUsers(people: Person[]): string[] {
  const names = people.map(({ name, email }) => {
    const short = name || (email.indexOf("@") > 0 ? email.slice(0, email.indexOf("@")) : email);
    const parts = NAME_PATTERN.exec(short);
    return parts ? `${capitalized(parts[1])} ${capitalized(parts[2])}` : short;
  });
  return [...new Set(names)].sort();
}

// `BranchFilterPopupComponent.getCompletionPrefixProvider`.
export function branchCompletionPrefix(text: string, offset: number): string {
  let start = 0;
  for (const separator of [...SEPARATORS, TWO_DOTS]) {
    const found = text.lastIndexOf(separator, offset - separator.length);
    if (found > start) start = found + separator.length;
  }
  return text.slice(start, offset).replace(/^-+/, "");
}

// `TextCompletionProviderBase.getPrefix`.
export function completionPrefix(text: string, offset: number): string {
  const found = SEPARATORS.map((separator) => text.lastIndexOf(separator, offset - 1));
  return text.slice(Math.max(...found) + 1, offset);
}

export function completions(values: string[], prefix: string): string[] {
  if (!prefix) return [];
  return values.filter((value) => value !== prefix && speedMatches(value, prefix));
}

export interface BranchGroup {
  label: string;
  values: string[];
}

export interface BranchMenu {
  recent: string[][];
  favorites: string[];
  groups: BranchGroup[];
}

// `processRecentBranchFilters`: `fromBranchPatterns` with `excludeNotMatched`, with nothing left over.
function matchesSomething(values: string[], existing: Set<string>): boolean {
  return values.some((value) => {
    const name = value.startsWith("-") ? value.slice(1) : value;
    if (existing.has(name)) return true;
    if (!REGEX_CHARS.test(name)) return false;
    try {
      new RegExp(name);
      return true;
    } catch {
      return false;
    }
  });
}

// `BranchPopupBuilder` with `GitRefManager.groupForBranchFilter`. A TreeMap sorts by UTF-16 units,
// like `sort` without a comparator.
export function branchMenu(
  refs: LogRef[],
  recent: string[][],
  favorites: FavoriteSet,
): BranchMenu {
  const branches = refs.filter((ref) => ref.kind !== "tag");
  const names = new Set(branches.map((ref) => ref.name));
  const grouped = new Map<string, string[]>();
  for (const { name, kind } of branches) {
    if (kind === "head") continue;
    const label = kind === "local" ? LOCAL : name.split("/", 1)[0];
    grouped.set(label, [...(grouped.get(label) ?? []), name]);
  }
  // `GitRefManager.isFavorite` treats HEAD as a favorite.
  const favorite = ({ name, kind }: LogRef) =>
    kind === "head" || (kind === "local" ? favorites.local : favorites.remote).includes(name);
  return {
    recent: names.size > 0 ? recent.filter((values) => matchesSomething(values, names)) : recent,
    favorites: branches.filter(favorite).map((ref) => ref.name).sort(),
    groups: [...grouped.keys()]
      .sort()
      .map((label) => ({ label, values: grouped.get(label)!.sort() })),
  };
}
