import type { TreeEntry } from "../lib/api";
import { speedMatch } from "../lib/speedSearch";
import { matchingRows, parentOf, type TreeRow } from "./model";

// What a tree gives its navigation: the rows that show and where each one is, how to bring a row
// into sight, and how many rows a page has.
export interface TreeRowsSource {
  rows: () => TreeRow[];
  indexOf: () => Map<string, number>;
  reveal: (index: number) => void;
  pageSize: () => number;
}

// What the keys do to the tree itself: open or close a folder, and Enter (on the lead row, if any).
export interface TreeKeyActions {
  isOpen: (path: string) => boolean;
  setOpen: (path: string, open: boolean) => void;
  enter: (row: TreeRow | undefined) => void;
}

// The selection, the speed search and the keys of a JTree, as the Project and Repositories trees
// have them. `selected` keeps the order of `DefaultTreeSelectionModel`: a path is added at the end,
// a range in row order (`JTree.getPathBetweenRows`).
export class TreeNavigation {
  selected = $state.raw(new Set<string>());
  lead = $state<string | null>(null);
  anchor: string | null = null;
  query = $state("");

  private readonly source: TreeRowsSource;

  constructor(source: TreeRowsSource, initial: string | null = null) {
    this.source = source;
    if (initial !== null) this.selectOnly(initial);
  }

  get leadIndex(): number {
    const lead = this.lead;
    return lead === null ? -1 : (this.source.indexOf().get(lead) ?? -1);
  }

  // The selected rows, in row order.
  selectedRows(): TreeRow[] {
    return this.source.rows().filter((row) => this.selected.has(row.entry.path));
  }

  // The selected entries, in the order they were selected (`getSelectionPaths`).
  selectedInOrder(): TreeEntry[] {
    const rows = this.source.rows();
    const indexOf = this.source.indexOf();
    return [...this.selected].flatMap((path) => {
      const index = indexOf.get(path);
      return index === undefined ? [] : [rows[index].entry];
    });
  }

  selectOnly(path: string) {
    this.selected = new Set([path]);
    this.lead = path;
    this.anchor = path;
  }

  clear() {
    this.selected = new Set();
    this.lead = null;
    this.anchor = null;
    this.query = "";
  }

  rangeOf(from: string | null, to: string): Set<string> {
    const indexOf = this.source.indexOf();
    const start = from === null ? -1 : (indexOf.get(from) ?? -1);
    const end = indexOf.get(to) ?? 0;
    if (start < 0) return new Set([to]);
    const [low, high] = start <= end ? [start, end] : [end, start];
    return new Set(
      this.source
        .rows()
        .slice(low, high + 1)
        .map((row) => row.entry.path),
    );
  }

  moveTo(index: number, extend: boolean) {
    const rows = this.source.rows();
    const row = rows[Math.max(0, Math.min(index, rows.length - 1))];
    if (!row) return;
    if (extend) {
      this.selected = this.rangeOf(this.anchor ?? this.lead, row.entry.path);
      this.lead = row.entry.path;
    } else this.selectOnly(row.entry.path);
    this.source.reveal(this.source.indexOf().get(row.entry.path) ?? -1);
  }

  // `DISCONTIGUOUS_TREE_SELECTION`: ⇧ a range, ⌘ one more or one less.
  click(event: MouseEvent, path: string) {
    if (event.shiftKey) {
      this.selected = this.rangeOf(this.anchor ?? this.lead, path);
      this.lead = path;
    } else if (event.metaKey || event.ctrlKey) {
      const next = new Set(this.selected);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      this.selected = next;
      this.lead = path;
      this.anchor = path;
    } else this.selectOnly(path);
  }

  // `JTree.setExpandedState` on a collapse: the selected rows under the folder leave the selection
  // (`removeDescendantSelectedPaths`), and the folder is selected if any was there; the lead follows.
  collapsed(folder: string) {
    const under = (path: string) => path.startsWith(`${folder}/`);
    const removed = [...this.selected].filter(under);
    if (removed.length > 0) {
      const next = new Set([...this.selected].filter((path) => !under(path)));
      next.add(folder);
      this.selected = next;
    }
    if (this.lead !== null && under(this.lead)) {
      this.lead = folder;
      this.anchor = folder;
    }
  }

  // `SpeedSearchBase.findElement`: the first row that matches from the selected one on, round the end.
  search(text: string) {
    this.query = text;
    const pattern = text.trim();
    if (!pattern) return;
    const rows = this.source.rows();
    const start = Math.max(0, this.leadIndex);
    const ordered = [...rows.slice(start), ...rows.slice(0, start)];
    const found = ordered.find((row) => speedMatch(row.entry.name, pattern) !== null);
    if (found) this.moveTo(this.source.indexOf().get(found.entry.path) ?? 0, false);
  }

  // ↑ and ↓ while searching go between the matches, round the end (`cycleScrolling`); Home and End
  // to the first and the last.
  private jump(key: string) {
    const matches = matchingRows(this.source.rows(), this.query.trim());
    if (matches.length === 0) return;
    const indexOf = this.source.indexOf();
    const leadIndex = this.leadIndex;
    const at = (row: TreeRow) => indexOf.get(row.entry.path) ?? 0;
    let target: TreeRow;
    if (key === "Home") target = matches[0];
    else if (key === "End") target = matches[matches.length - 1];
    else if (key === "ArrowDown") target = matches.find((row) => at(row) > leadIndex) ?? matches[0];
    else target = [...matches].reverse().find((row) => at(row) < leadIndex) ?? matches[matches.length - 1];
    this.moveTo(at(target), false);
  }

  private navigate(event: KeyboardEvent, actions: TreeKeyActions): boolean {
    const rows = this.source.rows();
    const leadIndex = this.leadIndex;
    const row = leadIndex >= 0 ? rows[leadIndex] : undefined;
    const page = this.source.pageSize();
    switch (event.key) {
      case "ArrowDown":
        this.moveTo(leadIndex + 1, event.shiftKey);
        break;
      case "ArrowUp":
        this.moveTo(leadIndex < 0 ? 0 : leadIndex - 1, event.shiftKey);
        break;
      case "Home":
        this.moveTo(0, event.shiftKey);
        break;
      case "End":
        this.moveTo(rows.length - 1, event.shiftKey);
        break;
      case "PageDown":
        this.moveTo(leadIndex + page, event.shiftKey);
        break;
      case "PageUp":
        this.moveTo(leadIndex - page, event.shiftKey);
        break;
      case "ArrowRight":
        if (row?.entry.dir && !actions.isOpen(row.entry.path)) actions.setOpen(row.entry.path, true);
        else if (row?.entry.dir && rows[leadIndex + 1]?.depth > row.depth) this.moveTo(leadIndex + 1, false);
        break;
      case "ArrowLeft":
        if (row?.entry.dir && actions.isOpen(row.entry.path)) actions.setOpen(row.entry.path, false);
        else if (row) {
          // A parent that is not a row (the hidden root) leaves the selection where it is.
          const parent = parentOf(row.entry.path);
          const parentIndex = parent === null ? undefined : this.source.indexOf().get(parent);
          if (parentIndex !== undefined) this.moveTo(parentIndex, false);
        }
        break;
      case "Enter":
        actions.enter(row);
        break;
      default:
        return false;
    }
    return true;
  }

  // The speed search and the navigation keys; true when the key was used. The tree's own shortcuts
  // go before this.
  keydown(event: KeyboardEvent, actions: TreeKeyActions): boolean {
    const plain = !event.metaKey && !event.ctrlKey && !event.altKey;
    const searching = this.query !== "";
    if (searching && plain && ["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key) && !event.shiftKey) {
      this.jump(event.key);
    } else if (searching && event.key === "Escape") {
      this.query = "";
    } else if (searching && plain && event.key === "Backspace") {
      this.search(this.query.slice(0, -1));
    } else if (plain && event.key.length === 1 && (searching || event.key !== " ")) {
      this.search(this.query + event.key);
    } else if (plain) {
      // Enter ends the search and acts on the row it found.
      if (event.key === "Enter") this.query = "";
      return this.navigate(event, actions);
    } else {
      return false;
    }
    return true;
  }
}
