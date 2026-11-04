interface Row {
  node: { id: string };
}

export function rangeIds(rows: Row[], anchor: string | null, target: string): string[] {
  const end = rows.findIndex((row) => row.node.id === target);
  const start = rows.findIndex((row) => row.node.id === anchor);
  if (start < 0 || end < 0) return [target];
  return rows.slice(Math.min(start, end), Math.max(start, end) + 1).map((row) => row.node.id);
}

export function toggleId(selected: Set<string>, id: string): Set<string> {
  const next = new Set(selected);
  if (!next.delete(id)) next.add(id);
  return next;
}

export function sameIds(left: Set<string>, right: Set<string>): boolean {
  return left.size === right.size && [...left].every((id) => right.has(id));
}

// After a refresh the selection follows the rows that remain. If the reference row is gone, it
// moves to the one that now occupies its place, so the keyboard focus is not lost.
export function reconcileSelection(
  rows: Row[],
  selected: Set<string>,
  lead: string | null,
  leadIndex: number,
): { selected: Set<string>; lead: string | null } {
  const present = new Set(rows.map((row) => row.node.id));
  const kept = new Set([...selected].filter((id) => present.has(id)));
  if (lead !== null && present.has(lead)) return { selected: kept, lead };
  if (lead === null || rows.length === 0) return { selected: kept, lead: null };
  const next = rows[Math.min(leadIndex, rows.length - 1)].node.id;
  if (kept.size === 0) kept.add(next);
  return { selected: kept, lead: next };
}
