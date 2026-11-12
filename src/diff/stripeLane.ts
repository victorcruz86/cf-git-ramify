import type { EditorView } from "@codemirror/view";
import { lineAtLane, stripeMarks, type StripeBlock } from "./stripe";

// The lane's width, in points (plan decision 1).
export const LANE_WIDTH = 8;

// The change stripe of one editor, in a column of its own beside it: on top of the editor it would
// cover macOS's scrollbar. Left of the left editor, as 241 puts that editor's scrollbar on the left
// (`TwosideTextDiffViewer`, `ThreesideTextDiffViewer`); right of the others.
export class StripeLane {
  readonly dom: HTMLDivElement;
  private blocks: StripeBlock[] = [];

  constructor(
    private readonly view: EditorView,
    edge: "left" | "right",
  ) {
    const host = view.dom.parentElement;
    this.dom = document.createElement("div");
    this.dom.className = "rm-stripe";
    this.dom.dataset.edge = edge;
    this.dom.setAttribute("role", "presentation");
    Object.assign(this.dom.style, {
      position: "relative",
      flex: "none",
      width: `${LANE_WIDTH}px`,
      background: "var(--editor)",
    });
    if (host) {
      host.style.display = "flex";
      view.dom.style.flex = "1";
      view.dom.style.minWidth = "0";
      if (edge === "left") host.prepend(this.dom);
      else host.append(this.dom);
    }
    this.dom.addEventListener("mousedown", this.jump);
  }

  draw(blocks: StripeBlock[]) {
    this.blocks = blocks;
    this.redraw();
  }

  // Again, for a new height of the lane or a new number of lines.
  redraw() {
    const marks = stripeMarks(this.blocks, this.view.state.doc.lines, this.dom.clientHeight);
    this.dom.replaceChildren(
      ...marks.map((mark) => {
        const element = document.createElement("div");
        element.className = `rm-stripe-mark rm-stripe-${mark.kind}`;
        element.dataset.line = String(mark.line);
        Object.assign(element.style, {
          position: "absolute",
          left: "1px",
          right: "1px",
          top: `${mark.top * 100}%`,
          height: `${mark.height * 100}%`,
          background: `var(--diff-${mark.kind}-stripe)`,
        });
        return element;
      }),
    );
  }

  // A click on the lane scrolls its editor to the line at that height (plan decision 4).
  private readonly jump = (event: MouseEvent) => {
    if (event.button !== 0) return;
    event.preventDefault();
    const bounds = this.dom.getBoundingClientRect();
    const height = bounds.height || this.dom.clientHeight;
    const doc = this.view.state.doc;
    const line = lineAtLane(event.clientY - bounds.top, height, doc.lines);
    this.view.scrollDOM.scrollTop = this.view.lineBlockAt(doc.line(line + 1).from).top;
  };

  destroy() {
    this.dom.removeEventListener("mousedown", this.jump);
    this.dom.remove();
  }
}
