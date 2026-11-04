// The model of the `Rebasing Commits` dialog, without DOM. Ported from `GitRebaseTodoModel`,
// `GitRebaseTodoModelConverter`, `GitRebaseCommitsTableModel` (with its ten Undo states),
// `ChangeEntryStateActions` (`getIndicesToUnite`, `findNewRoot` and when each action is enabled),
// `TableToolbarDecorator` (`MoveRunnable`), `CommonActionsPanel` (Up and Down) and
// `IndexedDetails.getSubject`.

import type { RebaseCommit, RebaseEntry, RebasePlan } from "../lib/api";

export type { RebaseCommit, RebaseEntry, RebasePlan };

export type EntryAction = "pick" | "edit" | "reword" | "squash" | "fixup" | "drop";
export type TableAction = EntryAction | "moveUp" | "moveDown";
export type ShortcutAction = TableAction | "undo" | "redo";

type KeepCommit = { kind: "pick" } | { kind: "edit" } | { kind: "reword"; message: string };
type NonUnite = KeepCommit | { kind: "drop" };

const PICK: KeepCommit = { kind: "pick" };
const EDIT: KeepCommit = { kind: "edit" };
const DROP: NonUnite = { kind: "drop" };
// `MAX_SIZE` of `SavedStates`.
const MAX_STATES = 10;
// `"\n".repeat(3)`.
const MESSAGE_SEPARATOR = "\n\n\n";

const keepsCommit = (type: NonUnite): type is KeepCommit => type.kind !== "drop";

function check(condition: boolean): asserts condition {
  if (!condition) throw new Error("The rebase list would be malformed");
}

class Simple {
  constructor(
    public index: number,
    public type: NonUnite,
    readonly entry: RebaseCommit,
  ) {}
}

class Root {
  private own: Child[] = [];

  constructor(
    public index: number,
    public type: KeepCommit,
    readonly entry: RebaseCommit,
  ) {}

  get children(): readonly Child[] {
    return this.own;
  }

  get group(): Element[] {
    return [this, ...this.own];
  }

  addChild(child: Child) {
    check(child.index > this.index && child.index <= this.newChildIndex());
    if (child.index === this.newChildIndex()) this.own.push(child);
    else this.own.splice(child.index - this.index - 1, 0, child);
  }

  lastChildIndex() {
    return this.own.at(-1)?.index ?? this.index;
  }

  newChildIndex() {
    return this.lastChildIndex() + 1;
  }

  removeChild(child: Child) {
    this.own = this.own.filter((own) => own !== child);
  }

  childrenIndicesChanged() {
    this.own.sort((a, b) => a.index - b.index);
  }

  unitedMessage(message: (entry: RebaseCommit) => string) {
    return this.group.map(({ entry }) => message(entry)).join(MESSAGE_SEPARATOR);
  }
}

class Child {
  constructor(
    public index: number,
    readonly entry: RebaseCommit,
    readonly root: Root,
  ) {}
}

type Element = Simple | Root | Child;

const commandOf = (element: Element) => (element instanceof Child ? "fixup" : element.type.kind);

// `ElementList` and `MutableElementList`: a single list, validated after every change.
class Elements {
  constructor(readonly items: Element[]) {}

  get size() {
    return this.items.length;
  }

  at(index: number): Element {
    const element = this.items[index];
    check(element !== undefined);
    return element;
  }

  modify(change: () => void) {
    change();
    this.validate();
  }

  private validate() {
    this.items.forEach((element, index) => {
      check(element.index === index);
      if (element instanceof Root) {
        check(element.children.length > 0);
        for (let at = element.index + 1; at < element.index + element.children.length; at += 1) {
          const child = this.items[at];
          check(child instanceof Child && child.root === element);
        }
      } else if (element instanceof Child) {
        check(element.root === this.items[element.root.index]);
      }
    });
  }

  forceChange(element: Element, next: Element) {
    check(element.index === next.index);
    this.items[element.index] = next;
  }

  move(group: Element[], position: number) {
    if (group.length === 0) return;
    const first = group[0];
    const last = group[group.length - 1];
    const min = Math.min(first.index, position);
    const max = Math.max(last.index, Math.min(position + group.length, this.size - 1));
    const firstAfter = Math.min(Math.max(position, 0), this.size - group.length);
    const moved = new Set(group.map(({ index }) => index));
    const kept: Element[] = [];
    for (let index = min; index <= max; index += 1) {
      if (!moved.has(index)) kept.push(this.items[index]);
    }
    const before = firstAfter - min;
    const changed = [...kept.slice(0, before), ...group, ...kept.slice(before)];
    changed.forEach((element, offset) => {
      this.items[min + offset] = element;
      element.index = min + offset;
    });
    const roots = new Set<Root>();
    for (const element of changed) {
      const root = element instanceof Child ? element.root : element;
      if (root instanceof Root) roots.add(root);
    }
    roots.forEach((root) => root.childrenIndicesChanged());
  }
}

// `GitRebaseTodoModel`.
class TodoModel {
  readonly rows: Elements;

  constructor(elements: Element[]) {
    this.rows = new Elements(elements);
  }

  get elements(): readonly Element[] {
    return this.rows.items;
  }

  private canSimple(indices: number[], kind: NonUnite["kind"]) {
    return indices.some((index) => {
      const element = this.rows.at(index);
      return element instanceof Child || element.type.kind !== kind;
    });
  }

  canPick = (indices: number[]) => this.canSimple(indices, "pick");
  canEdit = (indices: number[]) => this.canSimple(indices, "edit");
  canDrop = (indices: number[]) => this.canSimple(indices, "drop");
  canReword = (index: number) => !(this.rows.at(index) instanceof Child);

  pick(indices: number[]) {
    this.keepCommit(indices, PICK);
  }

  edit(indices: number[]) {
    this.keepCommit(indices, EDIT);
  }

  reword(index: number, message: string) {
    this.keepCommit([index], { kind: "reword", message });
  }

  drop(indices: number[]) {
    const elements = indices.map((index) => this.rows.at(index));
    elements.forEach((element) => {
      if (element instanceof Simple) element.type = DROP;
    });
    this.rows.modify(() => {
      elements.forEach((element) => {
        if (element instanceof Root) this.groupToSimple(element, DROP);
      });
    });
    // A dropped child leaves its group.
    const children = [...indices]
      .sort((a, b) => b - a)
      .map((index) => this.rows.at(index))
      .filter((element): element is Child => element instanceof Child);
    this.rows.modify(() => children.forEach((child) => this.removeAndMoveChild(child, DROP)));
  }

  canUnite(indices: number[]) {
    if (indices.length < 2) return false;
    const first = this.rows.at(indices[0]);
    if (first instanceof Simple) return true;
    const root = first instanceof Root ? first : first.root;
    return indices.slice(1).some((index) => {
      const element = this.rows.at(index);
      return !(element instanceof Child) || element.root !== root;
    });
  }

  unite(indices: number[]): Root {
    let root: Root | undefined;
    this.rows.modify(() => {
      const united = this.toRoot(indices[0]);
      root = united;
      const children: Element[] = [];
      for (const index of indices.slice(1)) {
        const element = this.rows.at(index);
        if (element instanceof Child && element.root === united) continue;
        for (const member of element instanceof Root ? element.group : [element]) {
          if (!children.includes(member)) children.push(member);
        }
      }
      this.rows.move(children, united.newChildIndex());
      children.forEach((child) => this.addToGroup(child.index, united));
    });
    check(root !== undefined);
    return root;
  }

  exchangeIndices(oldIndex: number, newIndex: number) {
    this.rows.modify(() => {
      const element = this.rows.at(oldIndex);
      let moved: Element[];
      if (element instanceof Root) moved = element.group;
      else if (element instanceof Simple) moved = [element];
      else {
        const simple = this.removeAndMoveChild(element, PICK);
        if (simple.index === newIndex) {
          // Moved to the end of its own group.
          this.addToGroup(simple.index, element.root);
          moved = [];
        } else {
          moved = [simple];
        }
      }
      this.rows.move(moved, newIndex);
      this.addToGroupIfNeeded(moved);
    });
  }

  private addToGroupIfNeeded(elements: Element[]) {
    const last = elements.at(-1);
    if (!last) return;
    const next = this.rows.items[last.index + 1];
    if (next instanceof Child) {
      elements.forEach((element) => this.addToGroup(element.index, next.root));
    }
  }

  private groupToSimple(root: Root, type: NonUnite) {
    root.group.forEach((element) => {
      this.rows.forceChange(element, new Simple(element.index, type, element.entry));
    });
  }

  private changeChild(child: Child, next: Element) {
    const root = child.root;
    root.removeChild(child);
    if (root.children.length === 0) {
      this.changeRoot(root, new Simple(root.index, root.type, root.entry));
    }
    this.rows.forceChange(child, next);
  }

  private changeRoot(root: Root, next: Element) {
    this.groupToSimple(root, PICK);
    this.rows.forceChange(root, next);
  }

  private addToGroup(index: number, root: Root) {
    const element = this.rows.at(index);
    const child = new Child(index, element.entry, root);
    root.addChild(child);
    if (element instanceof Simple) this.rows.forceChange(element, child);
    else if (element instanceof Root) this.changeRoot(element, child);
    else this.changeChild(element, child);
  }

  private toRoot(index: number): Root {
    const element = this.rows.at(index);
    if (element instanceof Root) return element;
    if (element instanceof Child) return element.root;
    const type = keepsCommit(element.type) ? element.type : PICK;
    const root = new Root(index, type, element.entry);
    this.rows.forceChange(element, root);
    return root;
  }

  private keepCommit(indices: number[], type: KeepCommit) {
    this.rows.modify(() => {
      [...indices]
        .sort((a, b) => b - a)
        .forEach((index) => {
          const element = this.rows.at(index);
          if (element instanceof Child) this.removeAndMoveChild(element, type);
          else element.type = type;
        });
    });
  }

  private removeAndMoveChild(child: Child, type: NonUnite): Simple {
    const target = child.root.lastChildIndex();
    const simple = new Simple(child.index, type, child.entry);
    this.changeChild(child, simple);
    this.rows.move([simple], target);
    return simple;
  }
}

// `convertToModel`: the `drop`s go at the end, and `fixup` and `squash` unite with the previous one.
function toModel(commits: RebaseCommit[]): TodoModel {
  const result: Element[] = [];
  for (const commit of commits) {
    const index = result.length;
    switch (commit.action) {
      case "pick":
      case "reword":
        result.push(new Simple(index, PICK, commit));
        break;
      case "edit":
        result.push(new Simple(index, EDIT, commit));
        break;
      case "drop":
        break;
      case "fixup":
      case "squash": {
        const last = result.at(-1);
        if (!last) throw new Error("Couldn't unite with non-existed commit");
        let root: Root;
        if (last instanceof Child) root = last.root;
        else if (last instanceof Root) root = last;
        else {
          if (!keepsCommit(last.type)) throw new Error("Couldn't unite with a dropped commit");
          root = new Root(last.index, last.type, last.entry);
          result[root.index] = root;
        }
        const child = new Child(index, commit, root);
        root.addChild(child);
        result.push(child);
        break;
      }
      default:
        throw new Error("Couldn't convert unknown action to the model");
    }
  }
  for (const commit of commits.filter(({ action }) => action === "drop")) {
    result.push(new Simple(result.length, DROP, commit));
  }
  return new TodoModel(result);
}

// `SavedStates.copyElements`.
function copied(elements: readonly Element[]): Element[] {
  const result: Element[] = [];
  for (const element of elements) {
    if (element instanceof Simple) {
      result.push(new Simple(element.index, element.type, element.entry));
    } else if (element instanceof Root) {
      result.push(new Root(element.index, element.type, element.entry));
    } else {
      const root = result[element.root.index];
      check(root instanceof Root);
      const child = new Child(element.index, element.entry, root);
      root.addChild(child);
      result.push(child);
    }
  }
  return result;
}

// `SavedStates`: up to ten states, with the current one.
class SavedStates {
  private current = 0;
  private states: Element[][];

  constructor(initial: readonly Element[]) {
    this.states = [copied(initial)];
  }

  private stateOrNull(): Element[] | null {
    if (this.current < 0 || this.current >= this.states.length) {
      this.current = Math.min(Math.max(this.current, 0), this.states.length - 1);
      return null;
    }
    return copied(this.states[this.current]);
  }

  previous() {
    this.current -= 1;
    return this.stateOrNull();
  }

  next() {
    this.current += 1;
    return this.stateOrNull();
  }

  add(state: readonly Element[]) {
    this.current += 1;
    if (this.current === MAX_STATES) {
      this.current = MAX_STATES - 1;
      this.states.shift();
    }
    this.states.length = this.current;
    this.states.push(copied(state));
  }
}

// `IndexedDetails.getSubject`.
export function subjectOf(message: string): string {
  const end = message.indexOf("\n\n");
  return (end > 0 ? message.slice(0, end) : message).replaceAll("\n", " ");
}

// `NodeType` of `GitRebaseCommitsTableView`: the simple node, the double one of a group's root, the
// pause of `edit`, and nothing for `drop` or for the children.
export type NodeType = "simple" | "double" | "edit" | "none";

export interface RebaseRow {
  commit: RebaseCommit;
  node: NodeType;
  // The subject, from the new message if there is a `reword`.
  subject: string;
  dropped: boolean;
  reworded: boolean;
  // A child of a group: whether it is the first (the arrow) and the last of the group.
  child: { first: boolean; last: boolean } | null;
}

// `GitRebaseCommitsTableModel` with the dialog's actions. The rows go from the oldest to the most
// recent. Each action returns the selection that remains, like `restoreSelection`.
export class RebaseModel {
  private todo: TodoModel;
  private saved: SavedStates;
  // How many times a change was announced (`fireTableRowsUpdated`): `Reset` and the question when cancelling.
  changes = 0;

  constructor(private readonly initial: RebaseCommit[]) {
    this.todo = toModel(initial);
    this.saved = new SavedStates(this.todo.elements);
  }

  get size() {
    return this.todo.elements.length;
  }

  private element(row: number): Element {
    return this.todo.rows.at(row);
  }

  // `updateModel`. If the list would end up malformed, nothing changes.
  private update(change: (todo: TodoModel) => void): boolean {
    const before = copied(this.todo.elements);
    try {
      change(this.todo);
    } catch {
      this.todo = new TodoModel(before);
      return false;
    }
    this.saved.add(this.todo.elements);
    this.changes += 1;
    return true;
  }

  private selectionOf(entries: RebaseCommit[]): number[] {
    const wanted = new Set(entries);
    return this.todo.elements.flatMap((element, index) => (wanted.has(element.entry) ? [index] : []));
  }

  private acting(selection: number[], change: (todo: TodoModel) => void): number[] {
    const entries = selection.map((row) => this.element(row).entry);
    this.update(change);
    return this.selectionOf(entries);
  }

  // `getCommitMessage`: the new one if there is a `reword`.
  message(row: number): string {
    const element = this.element(row);
    if (!(element instanceof Child) && element.type.kind === "reword") return element.type.message;
    return element.entry.message;
  }

  rows(): RebaseRow[] {
    return this.todo.elements.map((element, row) => {
      const type = element instanceof Child ? null : element.type;
      let node: NodeType = "simple";
      if (type?.kind === "edit") node = "edit";
      else if (!type || type.kind === "drop") node = "none";
      else if (element instanceof Root) node = "double";
      const children = element instanceof Child ? element.root.children : [];
      return {
        commit: element.entry,
        node,
        subject: subjectOf(this.message(row)),
        dropped: type?.kind === "drop",
        reworded: type?.kind === "reword",
        child:
          element instanceof Child
            ? { first: children[0] === element, last: children.at(-1) === element }
            : null,
      };
    });
  }

  // `isEntryActionEnabled` with `updateButton`: nothing without a selection or while a message is being edited.
  // Up and Down, like `CommonActionsPanel`.
  enabled(action: TableAction, selection: number[], editing = false): boolean {
    if (selection.length === 0) return false;
    const sorted = [...selection].sort((a, b) => a - b);
    if (action === "moveUp") return sorted[0] >= 1;
    if (action === "moveDown") return sorted[sorted.length - 1] < this.size - 1;
    if (editing) return false;
    switch (action) {
      case "pick":
        return this.todo.canPick(sorted);
      case "edit":
        return this.todo.canEdit(sorted);
      case "drop":
        return this.todo.canDrop(sorted);
      case "reword":
        return sorted.length === 1 && this.todo.canReword(sorted[0]);
      case "squash":
      case "fixup":
        return this.toUnite(sorted) !== null;
    }
  }

  // `getIndicesToUnite`: the selection, or with a single row, it and the previous one that keeps its
  // commit (`findNewRoot`).
  private toUnite(selection: number[]): number[] | null {
    if (this.todo.canUnite(selection)) return selection;
    if (selection.length !== 1) return null;
    const [index] = selection;
    const element = this.element(index);
    const before = this.todo.elements.slice(0, element.index);
    const root = before.findLast(
      (candidate) =>
        candidate instanceof Root || (candidate instanceof Simple && keepsCommit(candidate.type)),
    );
    if (!root) return null;
    const united = [root.index, index];
    return this.todo.canUnite(united) ? united : null;
  }

  pick(selection: number[]) {
    return this.acting(selection, (todo) => todo.pick(selection));
  }

  edit(selection: number[]) {
    return this.acting(selection, (todo) => todo.edit(selection));
  }

  drop(selection: number[]) {
    return this.acting(selection, (todo) => todo.drop(selection));
  }

  fixup(selection: number[]) {
    const united = this.toUnite([...selection].sort((a, b) => a - b));
    if (!united) return selection;
    return this.acting(selection, (todo) => todo.unite(united));
  }

  // `SquashAction`: fixup and, on the root, the united messages. Returns the root's row, whose
  // message is edited next.
  squash(selection: number[]): number | null {
    const united = this.toUnite([...selection].sort((a, b) => a - b));
    if (!united) return null;
    let root: number | null = null;
    this.update((todo) => {
      const first = todo.rows.at(united[0]);
      const current = first instanceof Root ? first : first instanceof Child ? first.root : null;
      const count = current?.children.length ?? 0;
      const grouped = todo.unite(united);
      root = grouped.index;
      if (current) {
        const added = grouped.children.slice(count);
        const message = [grouped, ...added]
          .map(({ index }) => this.message(index))
          .join(MESSAGE_SEPARATOR);
        todo.reword(grouped.index, message);
      } else {
        todo.reword(grouped.index, grouped.unitedMessage(({ message }) => message));
      }
    });
    return root;
  }

  // `RewordAction`: it only announces the change; the message is edited in the table.
  rewordStarted() {
    this.update(() => {});
  }

  // `setValueAt`, when finishing editing in the table: the original message goes back to `pick`. It does not save
  // state or announce, like IntelliJ.
  reword(row: number, message: string) {
    if (!this.todo.canReword(row)) return;
    if (message === this.element(row).entry.message) this.todo.pick([row]);
    else this.todo.reword(row, message);
  }

  // `MoveRunnable`: row by row, each one a change.
  private moved(selection: number[], delta: 1 | -1): number[] {
    const sorted = [...selection].sort((a, b) => a - b);
    const rows = delta > 0 ? sorted.reverse() : sorted;
    if (rows.length === 0 || rows[0] + delta < 0) return selection;
    if (rows[rows.length - 1] + delta > this.size) return selection;
    return rows.map((row) => {
      this.update((todo) => todo.exchangeIndices(row, row + delta));
      return row + delta;
    });
  }

  moveUp(selection: number[]) {
    return this.moved(selection, -1);
  }

  moveDown(selection: number[]) {
    return this.moved(selection, 1);
  }

  private restored(state: Element[] | null) {
    if (state) this.todo = new TodoModel(state);
    this.changes += 1;
  }

  undo() {
    this.restored(this.saved.previous());
  }

  redo() {
    this.restored(this.saved.next());
  }

  // `resetEntries`: the starting list, as one more state.
  reset() {
    this.todo = toModel(this.initial);
    this.saved.add(this.todo.elements);
    this.changes += 1;
  }

  entries(): RebasePlan {
    const elements = this.todo.elements;
    return {
      entries: elements.map((element) => ({
        action: commandOf(element),
        hash: element.entry.hash,
        subject: element.entry.subject,
      })),
      messages: elements.flatMap((element) =>
        !(element instanceof Child) && element.type.kind === "reword"
          ? [{ hash: element.entry.hash, message: element.type.message }]
          : [],
      ),
    };
  }
}

// `GitRebaseEntry.Action.mnemonic`: the initial of the command, with ⌥.
const MNEMONICS: Record<string, EntryAction> = {
  KeyP: "pick",
  KeyE: "edit",
  KeyD: "drop",
  KeyR: "reword",
  KeyS: "squash",
  KeyF: "fixup",
};

type Keys = Pick<KeyboardEvent, "key" | "code" | "altKey" | "metaKey" | "ctrlKey" | "shiftKey">;

// The table's shortcuts with the macOS keymap: ⌥ and the initial; `$Delete` (⌦, ⌫ and ⌘⌫) for Drop;
// `Git.Reword.Commit` (F2 and ⇧F6) for Reword; ⌥↑ and ⌥↓ (`CommonShortcuts.MOVE_UP` and `MOVE_DOWN`);
// `$Undo` (⌘Z) and `$Redo` (⇧⌘Z and ⌥⇧⌫).
export function shortcutAction(event: Keys): ShortcutAction | null {
  const { key, code, altKey, metaKey, ctrlKey, shiftKey } = event;
  if (ctrlKey) return null;
  if (altKey && !metaKey && !shiftKey) {
    if (code in MNEMONICS) return MNEMONICS[code];
    if (key === "ArrowUp") return "moveUp";
    if (key === "ArrowDown") return "moveDown";
  }
  if (!altKey && !shiftKey && (key === "Delete" || key === "Backspace")) return "drop";
  if (!altKey && !metaKey && ((key === "F2" && !shiftKey) || (key === "F6" && shiftKey))) {
    return "reword";
  }
  if (metaKey && !altKey && code === "KeyZ") return shiftKey ? "redo" : "undo";
  if (altKey && shiftKey && !metaKey && key === "Backspace") return "redo";
  return null;
}
