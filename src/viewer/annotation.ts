import { writable } from "svelte/store";
import type {
  AnnotatedCommit,
  Annotation,
  AnnotationTarget,
  ChangedFile,
  RevisionPair,
} from "../lib/api";
import type { MenuAction, MenuItem } from "../lib/menu";
import { SHORT_HASH } from "./model";

// The annotation gutter of the viewer (decision 7): `AnnotateToggleAction`'s columns with 241's
// defaults, and its popup. Every text here comes from the repository and is shown as text.

// `ShortNameType`, in its order (`Names ▸`), with `VcsBundle` texts (`V:920-924`).
export type ShortNameType = "initials" | "lastname" | "firstname" | "full" | "email";
export const SHORT_NAME_TYPES: [ShortNameType, string][] = [
  ["initials", "Initials"],
  ["lastname", "Last Name"],
  ["firstname", "First Name"],
  ["full", "Full Name"],
  ["email", "Email"],
];

// `ColorMode`, in its order (`Colors ▸`, `V:916-918`).
export type ColorMode = "author" | "order" | "none";
export const COLOR_MODES: [ColorMode, string][] = [
  ["author", "Author"],
  ["order", "Order"],
  ["none", "Hide"],
];

// The columns after the filler (`AnnotateToggleAction.doAnnotate`): the aspects of
// `GitFileAnnotation` (`V:683-685`), then `HistoryIdColumn` (`V:686`).
export type Aspect = "revision" | "date" | "author" | "number";
const ASPECTS: [Aspect, string][] = [
  ["revision", "Revision"],
  ["date", "Date"],
  ["author", "Author"],
  ["number", "Commit number"],
];

export interface AnnotationOptions {
  names: ShortNameType;
  colors: ColorMode;
  shown: Record<Aspect, boolean>;
}

// 241's defaults (the user has none set, G§13): `ShowShortenNames.getType` is `Last Name`,
// `ShowAnnotationColorsAction.getType` is `Order`, and `Revision` and `Commit number` are hidden
// (`showByDefault` false).
export const DEFAULT_OPTIONS: AnnotationOptions = {
  names: "lastname",
  colors: "order",
  shown: { revision: false, date: true, author: true, number: false },
};

// `annotate.short.names.type`, `annotate.color.mode` and the aspects' availability: global, as
// `PropertiesComponent` keeps them, so every annotated tab follows a change.
const OPTIONS_KEY = "rama.annotation.options";

function loadOptions(): AnnotationOptions {
  try {
    const stored = JSON.parse(localStorage.getItem(OPTIONS_KEY) ?? "{}");
    return {
      ...DEFAULT_OPTIONS,
      ...stored,
      shown: { ...DEFAULT_OPTIONS.shown, ...stored.shown },
    };
  } catch {
    return DEFAULT_OPTIONS;
  }
}

export const annotationOptions = writable<AnnotationOptions>(loadOptions());

export function setAnnotationOptions(options: AnnotationOptions) {
  annotationOptions.set(options);
  try {
    localStorage.setItem(OPTIONS_KEY, JSON.stringify(options));
  } catch {
    // Without storage, only what is remembered between sessions is lost.
  }
}

// `StringUtil.collapseWhiteSpace`: runs of spaces, tabs and newlines become one space, none at the ends.
const collapseWhiteSpace = (text: string) => text.split(/[ \t\n]+/).filter(Boolean).join(" ");

const DELIMITERS = /[.,<>()":_-]/g;
const isLowerCase = (char: string) => char !== char.toUpperCase() && char === char.toLowerCase();

// `ShortNameType.shorten`.
export function shortenName(input: string, type: ShortNameType): string {
  const raw = collapseWhiteSpace(input);
  let name = raw;
  let fromEmail = false;
  const [emailStart, emailEnd, at] = [name.indexOf("<"), name.indexOf(">"), name.indexOf("@")];
  if (emailStart >= 0 && emailStart < at && at < emailEnd) {
    // "Vasya <vasya.pupkin@jetbrains.com> Pupkin" -> "vasya.pupkin@jetbrains.com"
    const email = name.slice(emailStart + 1, emailEnd).trim();
    if (type === "email") return email;
    // "Vasya <vasya.pupkin@jetbrains.com> Pupkin" -> "Vasya Pupkin"
    const prefix = name.slice(0, emailStart).trim();
    const suffix = name.slice(emailEnd + 1).trim();
    if (prefix && suffix) name = `${prefix} ${suffix}`;
    else if (prefix || suffix) name = prefix || suffix;
    else {
      fromEmail = true;
      name = email;
    }
  }
  if (type === "full") return name;
  if (type === "email") {
    const atIndex = name.indexOf("@");
    if (atIndex === -1) return name;
    // "Vasya vasya.pupkin@jetbrains.com Pupkin" -> "vasya.pupkin@jetbrains.com", with the space
    // before it as `substring(lastIndexOf(" ", at), …)` leaves it.
    const start = Math.max(name.lastIndexOf(" ", atIndex), 0);
    const end = name.indexOf(" ", atIndex);
    return name.slice(start, end === -1 ? name.length : end);
  }
  const atIndex = name.indexOf("@");
  if (atIndex > 0 && !name.includes(" ")) {
    // "vasya.pupkin@email.com" -> "vasya pupkin"
    fromEmail = true;
    name = name.slice(0, atIndex).replace(DELIMITERS, " ");
  }
  const parts = name.split(" ").filter((part) => part.trim() !== "");
  if (parts.length === 0) return raw;
  if (type === "initials") {
    return parts
      .flatMap((part) => part.split(DELIMITERS))
      .filter((part) => part.trim() !== "")
      .map((part) => part[0].toUpperCase())
      .join("");
  }
  let picked = type === "firstname" ? parts[0] : parts[parts.length - 1];
  // Emails are written in lowercase and names capitalized.
  if (fromEmail && picked !== "" && isLowerCase(picked[0])) {
    picked = picked[0].toUpperCase() + picked.slice(1);
  }
  return picked;
}

// `VcsUserUtil.toExactString`.
export function exactUser({ author, email }: Pick<AnnotatedCommit, "author" | "email">): string {
  if (!author) return email;
  if (!email) return author;
  return `${author} <${email}>`;
}

const sameDay = (a: Date, b: Date) =>
  a.getFullYear() === b.getFullYear() &&
  a.getMonth() === b.getMonth() &&
  a.getDate() === b.getDate();
const dateFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "short" });
const dateTimeFormat = new Intl.DateTimeFormat(undefined, {
  dateStyle: "short",
  timeStyle: "short",
});

// `FileAnnotation.formatDate` = `DateFormatUtil.formatPrettyDate`: the date only, `Today` and
// `Yesterday` by name (`UtilBundle`). Milliseconds.
export function prettyDate(time: number, now: number): string {
  const date = new Date(time);
  const today = new Date(now);
  if (sameDay(date, today)) return "Today";
  const yesterday = new Date(today.getFullYear(), today.getMonth(), today.getDate() - 1);
  if (sameDay(date, yesterday)) return "Yesterday";
  return dateFormat.format(time);
}

// `GitFileAnnotation.getToolTip` without HTML (`V:927-930`): the commit, its author and date, the
// path when the commit had the file elsewhere, and the message after an empty line.
export function tooltipText(annotation: Annotation, commit: AnnotatedCommit): string {
  const lines = [
    `commit ${commit.hash}`,
    `Author: ${exactUser(commit)}`,
    `Date: ${dateTimeFormat.format(commit.authorTime * 1000)}`,
  ];
  if (commit.path !== annotation.path) lines.push(`Path: ${commit.path}`);
  return `${lines.join("\n")}\n\n${commit.message}`;
}

// The commit of line `line` (from 1), `null` on a line changed locally or past the annotation.
export function commitAt(annotation: Annotation, line: number): AnnotatedCommit | null {
  const index = annotation.lines[line - 1];
  return index === null || index === undefined ? null : (annotation.commits[index] ?? null);
}

// `AnnotationPresentation.isLastCommit`: the lines of the revision annotated are bold.
export const isLatest = (annotation: Annotation, commit: AnnotatedCommit) =>
  commit.hash === annotation.revision;

// `AnnotationFieldGutter.getBgColor`.
export function background(commit: AnnotatedCommit, mode: ColorMode): string | null {
  if (mode === "author") return commit.authorColor;
  if (mode === "order") return commit.orderColor;
  return null;
}

// The texts of a shown column, one per annotated line; empty on a blank line.
export interface AnnotationColumn {
  aspect: Aspect;
  texts: string[];
  // The widest text, in characters: every row of the column is that wide.
  width: number;
}

// `HistoryIdColumn.getLineText`: the number, padded on the left to the width of the largest.
function numberText(annotation: Annotation, commit: AnnotatedCommit): string {
  if (commit.number === null) return "";
  return String(commit.number).padStart(String(annotation.historySize).length, " ");
}

function aspectText(
  aspect: Aspect,
  annotation: Annotation,
  commit: AnnotatedCommit,
  options: AnnotationOptions,
  now: number,
): string {
  if (aspect === "revision") return commit.hash.slice(0, SHORT_HASH);
  if (aspect === "date") return prettyDate(commit.authorTime * 1000, now);
  if (aspect === "author") return shortenName(exactUser(commit), options.names);
  return numberText(annotation, commit);
}

// `Commit number` exists with a history of two or more (`computeLineNumbers`).
const available = (aspect: Aspect, annotation: Annotation) =>
  aspect !== "number" || annotation.historySize > 0;

// The columns shown, left to right, as `AspectAnnotationFieldGutter` and `HistoryIdColumn` fill them.
export function annotationColumns(
  annotation: Annotation,
  options: AnnotationOptions,
  now: number,
): AnnotationColumn[] {
  return ASPECTS.filter(
    ([aspect]) => options.shown[aspect] && available(aspect, annotation),
  ).map(([aspect]) => {
    const texts = annotation.lines.map((index) => {
      const commit = index === null ? undefined : annotation.commits[index];
      return commit ? aspectText(aspect, annotation, commit, options, now) : "";
    });
    return { aspect, texts, width: Math.max(0, ...texts.map((text) => text.length)) };
  });
}

const names = new Intl.Collator("en", { numeric: true, sensitivity: "base" });
const fileNameOf = (path: string) => path.slice(path.lastIndexOf("/") + 1);

// `ChangesComparator.getInstance(true)`: by file name (`FileNameComparator`), then by path
// (`HierarchicalFilePathComparator.NATURAL`: folder by folder, naturally).
export function compareChanges(left: ChangedFile, right: ChangedFile): number {
  const byName = names.compare(fileNameOf(left.path), fileNameOf(right.path));
  if (byName !== 0) return byName;
  const [a, b] = [left.path.split("/"), right.path.split("/")];
  for (let index = 0; index < Math.min(a.length, b.length); index++) {
    const part = names.compare(a[index], b[index]) || (a[index] < b[index] ? -1 : a[index] > b[index] ? 1 : 0);
    if (part !== 0) return part;
  }
  return a.length - b.length;
}

// `ShowDiffFromAnnotation.loadRequests` (88-104): every change of the line's commit against its
// first parent (`getCommittedChangeList`), sorted, as one request positioned on the annotated file
// (`findSelfInList`: by path, else by name, else the first). `parent` is `null` for a root commit.
export function commitDiffPairs(
  commit: AnnotatedCommit,
  parent: string | null,
  files: ChangedFile[],
): { files: RevisionPair[]; selected: number } {
  const sorted = [...files].sort(compareChanges);
  const pairs = sorted.map((file): RevisionPair => {
    const kind = file.status[0];
    return {
      left:
        kind === "A" || parent === null
          ? { revision: null, path: file.path, title: "" }
          : { revision: parent, path: file.oldPath ?? file.path, title: parent },
      right:
        kind === "D"
          ? { revision: null, path: file.path, title: commit.hash }
          : { revision: commit.hash, path: file.path, title: commit.hash },
    };
  });
  let selected = sorted.findIndex((file) => file.path === commit.path);
  if (selected < 0) {
    selected = Math.max(sorted.findIndex((file) => fileNameOf(file.path) === fileNameOf(commit.path)), 0);
  }
  return { files: pairs, selected };
}

// What the popup's entries do; the viewer carries them out.
export interface AnnotationHandlers {
  close: () => void;
  showDiff: (commit: AnnotatedCommit) => void;
  copy: (text: string) => void;
  // `Annotate Revision` and `Annotate Previous Revision`: that file at that revision, annotated,
  // at `line` (from 1) of the text shown translated into it.
  annotate: (target: AnnotationTarget, line: number) => void;
  options: (options: AnnotationOptions) => void;
  // `Open on GitHub` (`GHOpenInBrowserFromAnnotationActionGroup`), for the commit of the line: given
  // empty or not at all, there is none. It follows `Annotate Previous Revision` after a separator.
  hosting?: MenuItem[];
}

// `AnnotateActionGroup` (`V:766`): the columns, then `Colors ▸` and `Names ▸`. `Show Diff On Hover`
// is not copied (decision 8).
function viewMenu(
  annotation: Annotation,
  options: AnnotationOptions,
  set: (options: AnnotationOptions) => void,
): MenuAction {
  const aspects: MenuItem[] = ASPECTS.filter(([aspect]) => available(aspect, annotation)).map(
    ([aspect, label]) => ({
      label,
      checked: options.shown[aspect],
      run: () => set({ ...options, shown: { ...options.shown, [aspect]: !options.shown[aspect] } }),
    }),
  );
  return {
    label: "View",
    submenu: [
      ...aspects,
      null,
      {
        label: "Colors",
        submenu: COLOR_MODES.map(([mode, label]) => ({
          label,
          checked: options.colors === mode,
          run: () => set({ ...options, colors: mode }),
        })),
      },
      {
        label: "Names",
        submenu: SHORT_NAME_TYPES.map(([type, label]) => ({
          label,
          checked: options.names === type,
          run: () => set({ ...options, names: type }),
        })),
      },
    ],
  };
}

// `EditorGutterComponentImpl.getTextAnnotationPopupActions`: `Close Annotations` (`EB:76`) first,
// then `AnnotationPresentation`'s actions: `Show Diff`, `View ▸`, `Copy Revision Number`
// (`V:914`, only on an annotated line), a separator, `Annotate Revision` (`V:1002`) and `Annotate
// Previous Revision` (`V:1004`; on a line changed locally, the revision annotated), then a separator and
// the GitHub item (`handlers.hosting`). `line` is from 1.
// Both pass the annotation's line, not the editor's, with the editor's text
// (`ShowAnnotateOperationsPopup.getAnnotationLineNumber`; `approximate` for the previous one, so a
// changed line gives where its change starts): 241's own quirk, kept.
export function annotationPopup(
  annotation: Annotation,
  line: number,
  options: AnnotationOptions,
  handlers: AnnotationHandlers,
): MenuItem[] {
  const commit = commitAt(annotation, line);
  const origin = annotation.origins[line - 1];
  const annotationLine = origin === undefined ? line : origin + 1;
  const previous: AnnotationTarget | null = commit
    ? commit.previous
    : { hash: annotation.revision, path: annotation.path };
  const items: MenuItem[] = [
    { label: "Close Annotations", run: handlers.close },
    { label: "Show Diff", disabled: !commit, run: () => commit && handlers.showDiff(commit) },
    viewMenu(annotation, options, handlers.options),
  ];
  if (commit) {
    items.push({ label: "Copy Revision Number", run: () => handlers.copy(commit.hash) });
  }
  items.push(
    null,
    {
      label: "Annotate Revision",
      disabled: !commit,
      run: () => commit && handlers.annotate({ hash: commit.hash, path: commit.path }, annotationLine),
    },
    {
      label: "Annotate Previous Revision",
      disabled: !previous,
      run: () => previous && handlers.annotate(previous, annotationLine),
    },
  );
  // The extension actions of `AnnotateToggleAction` come after one more separator.
  const hosting = commit ? (handlers.hosting ?? []) : [];
  if (hosting.length > 0) items.push(null, ...hosting);
  return items;
}
