import { Facet } from "@codemirror/state";
import { GutterMarker, gutter, type EditorView, type ViewUpdate } from "@codemirror/view";
import {
  canResolveAutomatically,
  isChange,
  isConflict,
  isResolvedIn,
  paneFacet,
  rangeIn,
  statesField,
  type BlockState,
  type MergePane,
  type MergeSide,
} from "./model";

// The actions of TextMergeChange's margin: accept and ignore on each side and resolve in the
// center.

export type OperationAction = "apply" | "ignore" | "resolve";

// A margin action, DiffGutterOperation: on the first line of its block, with its icon.
export interface GutterAction {
  action: string;
  index: number;
  side: string | null;
  icon: IconName;
  tooltip: string;
}

export interface Operation extends GutterAction {
  action: OperationAction;
  side: MergeSide | null;
}

// Icons from platform/icons/src/diff/ (dark version), with the theme's color.
// Copyright 2000-2021 JetBrains s.r.o. Apache License 2.0.
const rect = (attributes: string) => `<rect fill="currentColor" ${attributes}/>`;
const ICONS = {
  arrow: [
    rect('width="1" height="7" x="3.328" y=".38" transform="rotate(45 3.828 3.88)"'),
    rect('width="7" height="1" x=".328" y="7.622" transform="rotate(45 3.828 8.122)"'),
    rect('width="1" height="7" x="7.571" y=".38" transform="rotate(45 8.071 3.88)"'),
    rect('width="7" height="1" x="4.571" y="7.622" transform="rotate(45 8.071 8.122)"'),
  ],
  arrowRight: [
    rect('width="1" height="7" x="7.674" y=".38" transform="scale(-1 1) rotate(45 0 -15.853)"'),
    rect('width="7" height="1" x="4.674" y="7.622" transform="scale(-1 1) rotate(45 0 -11.61)"'),
    rect('width="1" height="7" x="3.431" y=".38" transform="scale(-1 1) rotate(45 0 -5.61)"'),
    rect('width="7" height="1" x=".431" y="7.622" transform="scale(-1 1) rotate(45 0 -1.368)"'),
  ],
  arrowLeftDown: [
    rect('width="1" height="7" x="1" y="4"'),
    rect('width="7" height="1" x="1" y="10"'),
    rect('width="1" height="7" x="4" y="1"'),
    rect('width="7" height="1" x="4" y="7"'),
  ],
  arrowRightDown: [
    rect('width="1" height="7" x="10" y="4" transform="matrix(-1 0 0 1 21 0)"'),
    rect('width="7" height="1" x="3" y="10" transform="matrix(-1 0 0 1 13 0)"'),
    rect('width="1" height="7" x="7" y="1" transform="matrix(-1 0 0 1 15 0)"'),
    rect('width="7" height="1" x="1" y="7" transform="matrix(-1 0 0 1 9 0)"'),
  ],
  remove: [
    rect('width="1" height="11" x="5.498" y=".498" transform="rotate(45 5.998 5.998)"'),
    rect('width="1" height="11" x="5.498" y=".498" transform="rotate(135 5.998 5.998)"'),
  ],
  magicResolve: [
    '<polygon fill="currentColor" points="10.723 7.987 10.106 8.603 8.873 7.37 9.489 6.753"/>',
    '<polygon fill="currentColor" points="7.5 1.714 8.357 1.714 8.357 0 7.5 0"/>',
    '<polygon fill="currentColor" points="3.323 1.82 3.939 1.202 5.173 2.436 4.556 3.052"/>',
    '<path fill="currentColor" d="M0,10.2857143 L1.71428571,12 L9.42857143,4.28571429 L7.71428571,2.57142857 L0,10.2857143 Z M6,5.14285714 L7.71428571,3.42857143 L8.57142857,4.28571429 L6.85714286,6 L6,5.14285714 Z"/>',
    '<polygon fill="currentColor" points="10.286 4.357 12 4.357 12 3.5 10.286 3.5"/>',
  ],
};

export type IconName = keyof typeof ICONS;

const iconSvg = (name: IconName) =>
  `<svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 12 12">${ICONS[name].join("")}</svg>`;

// DiffUtil.createTooltipText, as text: the appendix goes on another line.
const CTRL_CLICK = "Ctrl+click to resolve conflict";
const tooltip = (text: string, ctrl: boolean) => (ctrl ? `${text}\n${CTRL_CLICK}` : text);

// DiffUtil.getArrowIcon and getArrowDownIcon: from the side towards the center, and downwards if the
// other side of the conflict was already applied.
function arrow(side: MergeSide, oneside: boolean): IconName {
  if (side === "left") return oneside ? "arrowRightDown" : "arrowRight";
  return oneside ? "arrowLeftDown" : "arrow";
}

// TextMergeChange.installOperations: each operation on the first line of the block in its pane.
export function operationsFor(
  states: readonly BlockState[],
  pane: MergePane,
  lines: number,
): Map<number, Operation[]> {
  const result = new Map<number, Operation[]>();
  const add = (state: BlockState, operation: Operation) => {
    const line = Math.min(rangeIn(state, pane)[0], lines - 1);
    const known = result.get(line);
    if (known) known.push(operation);
    else result.set(line, [operation]);
  };
  states.forEach((state, index) => {
    if (isResolvedIn(state, pane)) return;
    if (pane === "base") {
      if (isConflict(state) && canResolveAutomatically(state, "base")) {
        add(state, { action: "resolve", index, side: null, icon: "magicResolve", tooltip: "Resolve" });
      }
      return;
    }
    if (!isChange(state, pane)) return;
    const conflict = isConflict(state);
    add(state, {
      action: "apply",
      index,
      side: pane,
      icon: arrow(pane, state.oneside),
      tooltip: tooltip("Accept", conflict),
    });
    add(state, { action: "ignore", index, side: pane, icon: "remove", tooltip: tooltip("Ignore", conflict) });
  });
  return result;
}

class ActionsMarker extends GutterMarker {
  constructor(readonly operations: GutterAction[]) {
    super();
  }
  eq(other: ActionsMarker) {
    return (
      other.operations.length === this.operations.length &&
      other.operations.every((operation, index) => {
        const mine = this.operations[index];
        return (
          operation.action === mine.action &&
          operation.index === mine.index &&
          operation.icon === mine.icon &&
          operation.tooltip === mine.tooltip
        );
      })
    );
  }
  toDOM() {
    const dom = document.createElement("span");
    dom.style.display = "contents";
    for (const operation of this.operations) {
      const element = document.createElement("span");
      element.className = "rm-operation";
      element.title = operation.tooltip;
      element.setAttribute("role", "button");
      element.setAttribute("aria-label", operation.tooltip.split("\n")[0]);
      element.dataset.action = operation.action;
      element.dataset.index = String(operation.index);
      element.dataset.icon = operation.icon;
      if (operation.side) element.dataset.side = operation.side;
      // The icons are fixed; nothing from the file gets in here.
      element.innerHTML = iconSvg(operation.icon);
      dom.append(element);
    }
    return dom;
  }
}

// The actions margin of the three-pane editors: `margin` gives those of each line, `changed` says when
// to redo them and `run` launches them. The Ctrl click (VK_CONTROL, also on macOS) arrives as `modifier`.
export function actionsGutter<T extends GutterAction>(config: {
  margin: (view: EditorView) => Map<number, T[]>;
  changed: (update: ViewUpdate) => boolean;
  run: (view: EditorView, action: T, modifier: boolean) => void;
}) {
  const actionAt = (view: EditorView, event: Event): T | null => {
    const element = (event.target as HTMLElement).closest<HTMLElement>(".rm-operation");
    if (!element) return null;
    const index = Number(element.dataset.index);
    const action = element.dataset.action;
    for (const actions of config.margin(view).values()) {
      const found = actions.find((item) => item.index === index && item.action === action);
      if (found) return found;
    }
    return null;
  };
  return gutter({
    class: "rm-merge-actions",
    lineMarker(view, block) {
      const actions = config.margin(view).get(view.state.doc.lineAt(block.from).number - 1);
      return actions ? new ActionsMarker(actions) : null;
    },
    lineMarkerChange: config.changed,
    domEventHandlers: {
      mousedown(view, _block, event) {
        const mouse = event as MouseEvent;
        if (mouse.button !== 0) return false;
        const action = actionAt(view, event);
        if (!action) return false;
        config.run(view, action, mouse.ctrlKey);
        return true;
      },
      // On macOS Ctrl+click also opens the context menu.
      contextmenu: (view, _block, event) => actionAt(view, event) !== null,
    },
  });
}

export type OperationHandler = (operation: Operation, modifier: boolean) => void;

export const operationFacet = Facet.define<OperationHandler, OperationHandler>({
  combine: (values) => values[0],
});

// The three panes start with the same state: each one keeps its own.
const marginCache = new WeakMap<readonly BlockState[], Map<MergePane, Map<number, Operation[]>>>();

function marginOf(view: EditorView): Map<number, Operation[]> {
  const states = view.state.field(statesField);
  const pane = view.state.facet(paneFacet);
  let panes = marginCache.get(states);
  if (!panes) {
    panes = new Map();
    marginCache.set(states, panes);
  }
  let margin = panes.get(pane);
  if (!margin) {
    margin = operationsFor(states, pane, view.state.doc.lines);
    panes.set(pane, margin);
  }
  return margin;
}

export const operationsGutter = actionsGutter<Operation>({
  margin: marginOf,
  changed: (update) => update.startState.field(statesField) !== update.state.field(statesField),
  run: (view, operation, modifier) => view.state.facet(operationFacet)(operation, modifier),
});
