// `Find` (⌘F) in the editors of the diff window: the search bar of IntelliJ's editor
// (`EditorSearchSession`, `SearchReplaceComponent`) over the side that has the caret. The matching,
// the highlighting and the panel slot are CodeMirror's (`@codemirror/search`); the bar, its texts and
// its keys are IntelliJ's. Each side has its own bar and its own query, as each of IntelliJ's editors.

import {
  SearchQuery,
  closeSearchPanel,
  findNext,
  findPrevious,
  getSearchQuery,
  openSearchPanel,
  search,
  searchPanelOpen,
  setSearchQuery,
} from "@codemirror/search";
import { EditorSelection, type EditorState, type Extension } from "@codemirror/state";
import { EditorView, type Panel, type ViewUpdate } from "@codemirror/view";

// `LivePreviewController.MATCHES_LIMIT`.
export const MATCHES_LIMIT = 10_000;

// `ApplicationBundle`: `editorsearch.search.hint`, `.matches`, `.current.cursor.position`,
// `.toomuch`; `FindBundle`: `find.incorrect.regexp`, `find.case.sensitive`, `find.whole.words`,
// `find.regex`.
export const SEARCH_HINT = "Search";
export const BAD_PATTERN = "Bad pattern";
export const MATCH_CASE = "Match Case";
export const WORDS = "Words";
export const REGEX = "Regex";
export const PREVIOUS = "Previous Occurrence";
export const NEXT = "Next Occurrence";
export const CLOSE = "Close";

export function resultsText(count: number): string {
  return count === 1 ? "1 result" : `${count} results`;
}

type Toggle = "caseSensitive" | "wholeWord" | "regexp";

export interface SearchStatus {
  text: string;
  // `setNotFoundBackground`: nothing found, or a pattern that does not compile.
  notFound: boolean;
}

// `EditorSearchSession.searchResultsUpdated`: the place of the match under the selection, or the count.
export function searchStatus(state: EditorState, query: SearchQuery): SearchStatus {
  if (!query.search) return { text: resultsText(0), notFound: false };
  if (!query.valid) return { text: BAD_PATTERN, notFound: true };
  const selection = state.selection.main;
  const cursor = query.getCursor(state);
  let count = 0;
  let current = -1;
  for (let next = cursor.next(); !next.done; next = cursor.next()) {
    count += 1;
    if (next.value.from === selection.from && next.value.to === selection.to) current = count;
    if (count > MATCHES_LIMIT) return { text: `${MATCHES_LIMIT}+`, notFound: false };
  }
  if (count === 0) return { text: resultsText(0), notFound: true };
  return { text: current > 0 ? `${current}/${count}` : resultsText(count), notFound: false };
}

// While typing, the first match at or after where the search began, or else the first one, as
// IntelliJ's live search moves to it.
function selectFrom(view: EditorView, query: SearchQuery, start: number) {
  if (!query.valid) return;
  const cursor = query.getCursor(view.state);
  let first: { from: number; to: number } | null = null;
  for (let next = cursor.next(); !next.done; next = cursor.next()) {
    first ??= next.value;
    if (next.value.from >= start) {
      first = next.value;
      break;
    }
  }
  if (!first) return;
  view.dispatch({
    selection: EditorSelection.single(first.from, first.to),
    effects: EditorView.scrollIntoView(first.from, { y: "center" }),
    userEvent: "select.search",
  });
}

function button(label: string, text: string, onclick: () => void, toggle = false) {
  const element = document.createElement("button");
  element.type = "button";
  element.className = toggle ? "rm-find-toggle" : "rm-find-button";
  element.textContent = text;
  element.title = label;
  element.setAttribute("aria-label", label);
  if (toggle) element.setAttribute("aria-pressed", "false");
  element.tabIndex = -1;
  element.addEventListener("mousedown", (event) => event.preventDefault());
  element.addEventListener("click", onclick);
  return element;
}

class FindBar implements Panel {
  readonly dom: HTMLElement;
  readonly top = true;
  private readonly field: HTMLInputElement;
  private readonly status: HTMLElement;
  private readonly toggles: Record<Toggle, HTMLButtonElement>;
  private query: SearchQuery;
  // Where the caret was when the bar opened, or when the query changed from outside.
  private start: number;

  constructor(private readonly view: EditorView) {
    this.query = getSearchQuery(view.state);
    this.start = view.state.selection.main.from;
    this.dom = document.createElement("div");
    this.dom.className = "rm-find";
    this.dom.setAttribute("role", "search");
    this.field = document.createElement("input");
    this.field.type = "text";
    this.field.className = "rm-find-field";
    this.field.placeholder = SEARCH_HINT;
    this.field.setAttribute("aria-label", SEARCH_HINT);
    this.field.setAttribute("main-field", "true");
    this.field.spellcheck = false;
    this.field.value = this.query.search;
    this.field.addEventListener("input", () => this.typed());
    this.field.addEventListener("keydown", (event) => this.keydown(event));
    this.status = document.createElement("span");
    this.status.className = "rm-find-status";
    this.status.setAttribute("role", "status");
    const previous = button(`${PREVIOUS} (⇧⌘G)`, "↑", () => findPrevious(view));
    const next = button(`${NEXT} (⌘G)`, "↓", () => findNext(view));
    this.toggles = {
      caseSensitive: button(MATCH_CASE, "Cc", () => this.flip("caseSensitive"), true),
      wholeWord: button(WORDS, "W", () => this.flip("wholeWord"), true),
      regexp: button(REGEX, ".*", () => this.flip("regexp"), true),
    };
    const close = button(`${CLOSE} (Escape)`, "×", () => this.close());
    const options = document.createElement("span");
    options.className = "rm-find-options";
    options.append(this.toggles.caseSensitive, this.toggles.wholeWord, this.toggles.regexp);
    this.dom.append(this.field, options, this.status, previous, next, close);
    this.show();
  }

  mount() {
    this.field.focus();
    this.field.select();
  }

  update(update: ViewUpdate) {
    const query = getSearchQuery(update.state);
    if (!query.eq(this.query)) {
      this.query = query;
      if (this.field.value !== query.search) {
        this.field.value = query.search;
        this.start = update.state.selection.main.from;
      }
    }
    if (update.docChanged || update.selectionSet || !query.eq(getSearchQuery(update.startState))) {
      this.show();
    }
  }

  private show() {
    const { text, notFound } = searchStatus(this.view.state, this.query);
    this.status.textContent = text;
    this.dom.classList.toggle("rm-find-not-found", notFound);
    for (const [key, element] of Object.entries(this.toggles)) {
      element.setAttribute("aria-pressed", String(this.query[key as Toggle]));
    }
  }

  private commit(spec: Partial<ConstructorParameters<typeof SearchQuery>[0]>) {
    const query = new SearchQuery({
      search: this.query.search,
      caseSensitive: this.query.caseSensitive,
      wholeWord: this.query.wholeWord,
      regexp: this.query.regexp,
      literal: true,
      ...spec,
    });
    this.query = query;
    this.view.dispatch({ effects: setSearchQuery.of(query) });
    selectFrom(this.view, query, this.start);
  }

  private typed() {
    this.commit({ search: this.field.value });
  }

  private flip(key: Toggle) {
    this.commit({ [key]: !this.query[key] });
  }

  private close() {
    closeSearchPanel(this.view);
    this.view.focus();
  }

  // `EditorSearchSession`: Enter and ⌘G go down, ⇧Enter and ⇧⌘G up, Escape closes the bar and goes
  // back to the editor. The window never sees them: Escape there would close the diff.
  private keydown(event: KeyboardEvent) {
    const key = event.key.toLowerCase();
    const down = (event.key === "Enter" && !event.shiftKey) || (event.metaKey && key === "g" && !event.shiftKey);
    const up = (event.key === "Enter" && event.shiftKey) || (event.metaKey && key === "g" && event.shiftKey);
    if (down) findNext(this.view);
    else if (up) findPrevious(this.view);
    else if (event.key === "Escape") this.close();
    else if (event.metaKey && key === "f") this.field.select();
    else return;
    event.preventDefault();
    event.stopPropagation();
  }
}

const findTheme = EditorView.theme({
  ".cm-panels": { backgroundColor: "var(--panel)", color: "var(--text)" },
  ".cm-panels-top": { borderBottom: "1px solid var(--border)" },
  ".rm-find": {
    display: "flex",
    alignItems: "center",
    gap: "4px",
    padding: "3px 6px",
    fontFamily: "system-ui, sans-serif",
    fontSize: "13px",
  },
  ".rm-find-field": {
    flex: "0 1 280px",
    minWidth: "120px",
    height: "22px",
    padding: "0 6px",
    border: "1px solid var(--border)",
    borderRadius: "4px",
    background: "var(--input-background)",
    color: "var(--editor-text)",
    outline: "none",
  },
  ".rm-find-field:focus": { borderColor: "var(--focus)" },
  "&.cm-editor .rm-find-not-found .rm-find-field": { background: "#5E3838" },
  ".rm-find-options": { display: "inline-flex", gap: "1px" },
  ".rm-find-toggle, .rm-find-button": {
    minWidth: "22px",
    height: "22px",
    padding: "0 4px",
    border: "1px solid transparent",
    borderRadius: "4px",
    background: "transparent",
    color: "inherit",
    fontSize: "12px",
    cursor: "default",
  },
  ".rm-find-toggle:hover, .rm-find-button:hover": { background: "var(--hover)" },
  ".rm-find-toggle[aria-pressed='true']": { background: "var(--pressed)", borderColor: "var(--focus)" },
  ".rm-find-status": { minWidth: "70px", padding: "0 6px", color: "var(--muted)" },
  // TEXT_SEARCH_RESULT_ATTRIBUTES of expUI_darkScheme.xml; the current match is the selection.
  ".cm-searchMatch": { backgroundColor: "#114957", outline: "1px solid #165e70" },
  ".cm-searchMatch.cm-searchMatch-selected": { backgroundColor: "var(--selection)" },
});

// `Find`, `FindNext` and `FindPrevious` of the Mac keymap: ⌘F, ⌘G and ⇧⌘G (and F3, ⇧F3); Escape closes
// an open bar. The editor takes them before anything around it: ⌘F is also `Vcs.Log.GoToRef` in the
// Log, which a preview diff sits in, and Escape closes the diff window.
function command(event: KeyboardEvent, view: EditorView): ((view: EditorView) => boolean) | null {
  if (event.altKey || event.ctrlKey) return null;
  const key = event.key.toLowerCase();
  if (event.key === "Escape" && !event.metaKey && searchPanelOpen(view.state)) return closeSearchPanel;
  if (event.metaKey && !event.shiftKey && key === "f") return openSearchPanel;
  if (event.metaKey && key === "g") return event.shiftKey ? findPrevious : findNext;
  if (!event.metaKey && event.key === "F3") return event.shiftKey ? findPrevious : findNext;
  return null;
}

export function diffSearch(): Extension {
  return [
    search({ top: true, literal: true, createPanel: (view) => new FindBar(view) }),
    EditorView.domEventHandlers({
      keydown(event, view) {
        const run = command(event, view);
        if (!run) return false;
        run(view);
        event.preventDefault();
        event.stopPropagation();
        return true;
      },
    }),
    findTheme,
  ];
}
