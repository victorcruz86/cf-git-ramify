import type { PullStateFilter, ReviewFilter, SavedSearch, SearchValue } from "../lib/api";

// The list's search: GHPRListSearchValue, GHPRSearchPanelViewModel (the quick filters),
// GHPRSearchPanelFactory (the filters and the short text) and ReviewListSearchPanelViewModelBase
// (when a search goes into the history), at idea/241.14494.240.

export const EMPTY_SEARCH: SearchValue = {
  text: null,
  state: null,
  assignee: null,
  review: null,
  author: null,
  label: null,
};

// `GB:176-178`.
export const STATES: { value: PullStateFilter; label: string }[] = [
  { value: "OPEN", label: "Open" },
  { value: "CLOSED", label: "Closed" },
  { value: "MERGED", label: "Merged" },
];

// `GB:181-194`: the value shows the short text, the popup the full one.
export const REVIEWS: { value: ReviewFilter; short: string; full: string }[] = [
  { value: "NO_REVIEW", short: "No reviews", full: "No reviews" },
  { value: "REQUIRED", short: "Review required", full: "Review required" },
  { value: "APPROVED", short: "Approved review", full: "Approved review" },
  { value: "CHANGES_REQUESTED", short: "Changes requested", full: "Changes requested" },
  { value: "REVIEWED_BY_ME", short: "Reviewed", full: "Reviewed by you" },
  { value: "NOT_REVIEWED_BY_ME", short: "Not reviewed", full: "Not reviewed by you" },
  { value: "AWAITING_REVIEW", short: "Awaiting review", full: "Awaiting review from you" },
];

export type FilterField = "state" | "author" | "label" | "assignee" | "review";

// The dropdowns in their on-screen order (`GB:175`, `195`, `196`, `179`, `180`). `chooser` is the
// REST list an async chooser reads its values from.
export const FILTERS: {
  field: FilterField;
  name: string;
  chooser: "collaborators" | "labels" | "assignees" | null;
}[] = [
  { field: "state", name: "State", chooser: null },
  { field: "author", name: "Author", chooser: "collaborators" },
  { field: "label", name: "Label", chooser: "labels" },
  { field: "assignee", name: "Assignee", chooser: "assignees" },
  { field: "review", name: "Review", chooser: null },
];

const stateLabel = (state: PullStateFilter) => STATES.find((entry) => entry.value === state)?.label ?? state;
const reviewShort = (review: ReviewFilter) => REVIEWS.find((entry) => entry.value === review)?.short ?? review;

// What a set dropdown shows.
export function filterText(value: SearchValue, field: FilterField): string | null {
  switch (field) {
    case "state":
      return value.state && stateLabel(value.state);
    case "review":
      return value.review && reviewShort(value.review);
    default:
      return value[field];
  }
}

// The quick filters (`GB:197-200`), with `me` the connection's login.
export function quickFilters(me: string): { title: string; value: SearchValue }[] {
  return [
    { title: "Open", value: { ...EMPTY_SEARCH, state: "OPEN" } },
    { title: "Your pull requests", value: { ...EMPTY_SEARCH, state: "OPEN", author: me } },
    { title: "Assigned to you", value: { ...EMPTY_SEARCH, state: "OPEN", assignee: me } },
    { title: "Review requests", value: { ...EMPTY_SEARCH, state: "OPEN", review: "AWAITING_REVIEW" } },
  ];
}

// `defaultQuickFilter`: `Assigned to you`.
export const defaultSearch = (me: string): SearchValue => quickFilters(me)[2].value;

// `historyModel.lastFilter ?: defaultQuickFilter.filter`.
export const initialSearch = (saved: SavedSearch | null, me: string): SearchValue =>
  saved?.last ?? defaultSearch(me);

const FIELDS = ["text", "state", "assignee", "review", "author", "label"] as const;

export const sameSearch = (left: SearchValue, right: SearchValue) =>
  FIELDS.every((field) => (left[field] ?? null) === (right[field] ?? null));

// `filterCount`: one for each value that is set.
export const filterCount = (value: SearchValue) =>
  FIELDS.filter((field) => value[field] !== null && value[field] !== undefined).length;

// `CTB:52`.
export const clearFiltersLabel = (count: number) => `Clear ${count} Filters`;

// `GHPRSearchPanelFactory.getShortText`, with its trailing space.
export function shortText(value: SearchValue): string {
  let text = "";
  if (value.text !== null) text += `"${value.text}" `;
  if (value.state !== null) text += `state:"${stateLabel(value.state)}" `;
  if (value.label !== null) text += `label:${value.label} `;
  if (value.assignee !== null) text += `assignee:${value.assignee} `;
  if (value.review !== null) text += `reviewState:"${reviewShort(value.review)}" `;
  if (value.author !== null) text += `author:${value.author} `;
  return text;
}

// `updateHistoryOnSearchChanges`: a search goes into the history when it differs from the
// previous one only by its filters, 10 s later; else at once.
export const HISTORY_DELAY = 10_000;

type Save = (value: SearchValue, remember: boolean) => Promise<SavedSearch>;

// Every change becomes the last search. A search with no filter, or the default one, never goes
// into the history; another change, or `dispose`, drops the one still waiting.
export function searchHistory(save: Save, me: string, onsaved: (saved: SavedSearch) => void) {
  let waiting: ReturnType<typeof setTimeout> | null = null;
  const run = (value: SearchValue, remember: boolean) =>
    save(value, remember).then(onsaved, (failure) =>
      console.warn("Could not save the pull requests search:", failure),
    );
  const cancel = () => {
    if (waiting !== null) clearTimeout(waiting);
    waiting = null;
  };
  return {
    changed(old: SearchValue, next: SearchValue) {
      cancel();
      const remembered = filterCount(next) > 0 && !sameSearch(next, defaultSearch(me));
      if (remembered && (old.text ?? null) !== (next.text ?? null)) {
        void run(next, true);
        return;
      }
      void run(next, false);
      if (remembered) {
        waiting = setTimeout(() => {
          waiting = null;
          void run(next, true);
        }, HISTORY_DELAY);
      }
    },
    dispose: cancel,
  };
}
