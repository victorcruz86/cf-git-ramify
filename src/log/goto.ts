import type { LogRef, LogRow, RefKind, RowOf } from "../lib/api";
import { authorText, shortDate, shortHash, shortTime } from "./draw";

export interface Target {
  kind: "ref" | "hash" | "invalid";
  value: string;
}

// `VcsLogUtil.HASH_PREFIX_REGEX`.
const HASH_PREFIX = /^[a-fA-F0-9]{4,40}$/;
// `jumpToHash` lets through what usually surrounds a copied hash.
const AROUND_HASH = /^[()'"`]+|[()'"`]+$/g;
const ELLIPSIS = "...";
// `GoToParentOrChildAction.getActionText` and `GraphCommitCellController.getArrowTooltipText`.
const MENU_SUBJECT_LENGTH = 40;
const TOOLTIP_SUBJECT_LENGTH = 50;

// `VcsLogNavigationUtil.jumpToRefOrHash`. `refs` arrives in the order of the labels.
export function resolveTarget(text: string, refs: LogRef[]): Target {
  const ref = refs.find(({ name }) => name.startsWith(text));
  if (ref) return { kind: "ref", value: ref.name };
  const hash = text.replace(AROUND_HASH, "");
  return HASH_PREFIX.test(hash) ? { kind: "hash", value: hash } : { kind: "invalid", value: text };
}

function presentation(target: Target, found: RowOf): string {
  if (target.kind === "ref") return `Commit '${shortHash(found.hash ?? target.value)}'`;
  const { value } = target;
  return `Commit or reference '${HASH_PREFIX.test(value) ? shortHash(value) : value}'`;
}

// `getCommitNotFoundMessage`.
export function jumpFailure(target: Target, found: RowOf): string {
  if (target.kind === "invalid") return `'${target.value}' does not look like a commit hash`;
  const commit = presentation(target, found);
  return found.hash ? `${commit} doesn't match the filters` : `${commit} not found`;
}

// `StringUtil.shortenTextWithEllipsis`, without a suffix.
function shortened(text: string, length: number): string {
  return text.length > length ? text.slice(0, length - ELLIPSIS.length) + ELLIPSIS : text;
}

function authored(row: LogRow): string {
  const time = row.time * 1000;
  return `by ${authorText(row)} on ${shortDate(time)} at ${shortTime(time)}`;
}

export function adjacentLabel(row: LogRow): string {
  const subject = shortened(row.subject, MENU_SUBJECT_LENGTH);
  return `${shortHash(row.hash)} "${subject}" ${authored(row)}`;
}

// `TooltipReferencesPanel.REFS_LIMIT`.
const TOOLTIP_REFS = 10;

export interface TooltipRefs {
  rows: { text: string; kind: RefKind; first: boolean }[];
  rest: string | null;
}

// `ReferencesPanel.update`: the icon goes on the first of each type, and those of a type go with a comma.
export function tooltipRefs(refs: LogRef[]): TooltipRefs {
  const visible = refs.slice(0, TOOLTIP_REFS);
  const hidden = refs.length - visible.length;
  return {
    rows: visible.map(({ name, kind }, index) => ({
      text: visible[index + 1]?.kind === kind ? `${name},` : name,
      kind,
      first: visible[index - 1]?.kind !== kind,
    })),
    rest: hidden > 0 ? `... ${hidden} more in details pane` : null,
  };
}

export interface ArrowTooltip {
  subject: string;
  rest: string;
}

// The text `Jump to {0} by {1} on {2} at {3}`, with the subject apart because it is bold.
export const arrowTooltip = (row: LogRow): ArrowTooltip => ({
  subject: `"${shortened(row.subject, TOOLTIP_SUBJECT_LENGTH)}"`,
  rest: ` ${authored(row)}`,
});
