export interface SelectionSnapshot {
  hashes: string[];
  onTop: boolean;
  // The commit that must stay at the same height, and how many pixels from the top edge.
  target: { hash: string; gap: number } | null;
}

export interface SnapshotInput {
  // In row order.
  selected: number[];
  // Those of `selected`: a selected row may have already left the cache.
  hashes: string[];
  hashAt: (row: number) => string | undefined;
  scrollTop: number;
  viewportHeight: number;
  rowHeight: number;
  count: number;
}

export interface RestoredSelection {
  rows: number[];
  // Null if there is nothing to hold on to: the scroll stays where it is.
  scrollTop: number | null;
}

// `SelectionSnapshot`: it stores commits and not rows, which change with each reload.
export function snapshotSelection(input: SnapshotInput): SelectionSnapshot {
  const { selected, hashes, hashAt, scrollTop, viewportHeight, rowHeight, count } = input;
  if (count === 0) return { hashes, onTop: false, target: null };
  const first = Math.floor(scrollTop / rowHeight);
  const last = Math.min(count - 1, Math.ceil((scrollTop + viewportHeight) / rowHeight) - 1);
  const row = selected.find((candidate) => candidate >= first && candidate <= last) ?? first;
  const hash = hashAt(row);
  return {
    hashes,
    onTop: first === 0,
    target: hash ? { hash, gap: row * rowHeight - scrollTop } : null,
  };
}

// `rows` says where each of the remaining commits ended up. `changed`: the commit list changed.
export function restoreSelection(
  snapshot: SelectionSnapshot,
  rows: Map<string, number>,
  changed: boolean,
  rowHeight: number,
): RestoredSelection {
  const selected = snapshot.hashes.flatMap((hash) => rows.get(hash) ?? []);
  if (snapshot.onTop && changed) return { rows: selected, scrollTop: 0 };
  const row = snapshot.target ? rows.get(snapshot.target.hash) : undefined;
  if (row === undefined || !snapshot.target) return { rows: selected, scrollTop: null };
  return { rows: selected, scrollTop: Math.max(row * rowHeight - snapshot.target.gap, 0) };
}
