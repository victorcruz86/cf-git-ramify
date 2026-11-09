import { describe, expect, test, vi } from "vitest";
import type { AnnotatedCommit, Annotation } from "../lib/api";
import type { MenuAction, MenuItem } from "../lib/menu";
import {
  annotationColumns,
  annotationPopup,
  background,
  DEFAULT_OPTIONS,
  prettyDate,
  shortenName,
  commitDiffPairs,
  tooltipText,
  type AnnotationHandlers,
} from "./annotation";

const OLD = "1111111111111111111111111111111111111111";
const NEW = "2222222222222222222222222222222222222222";

function commit(hash: string, change: Partial<AnnotatedCommit> = {}): AnnotatedCommit {
  return {
    hash,
    path: "src/a.txt",
    author: "Ann Smith",
    email: "ann@example.com",
    authorTime: new Date(2020, 0, 1, 10, 0).getTime() / 1000,
    committerTime: new Date(2020, 0, 1, 10, 0).getTime() / 1000,
    subject: "first",
    message: "first\n\nits body",
    previous: null,
    number: 1,
    orderColor: "#242f45",
    authorColor: "#2f5194",
    ...change,
  };
}

// Line 1 and 3 from the old commit, line 2 inserted locally, line 4 from the revision annotated:
// the revision blamed has three lines, and line 2 would go where it was inserted, its line 2.
const annotation: Annotation = {
  revision: NEW,
  path: "src/a.txt",
  commits: [
    commit(OLD),
    commit(NEW, {
      author: "Bob Jones",
      email: "bob@example.com",
      number: 12,
      previous: { hash: OLD, path: "src/old.txt" },
      orderColor: "#2f5194",
      authorColor: "#2d4473",
    }),
  ],
  lines: [0, null, 0, 1],
  origins: [0, 1, 1, 2],
  owners: [0, 0, 1],
  suspicious: false,
  historySize: 12,
  line: null,
};

const shape = (items: MenuItem[]): unknown[] =>
  items.map((item) => {
    if (item === null) return "—";
    if ("header" in item) return `# ${item.header}`;
    const label = `${item.checked ? "✓ " : ""}${item.disabled ? `(${item.label})` : item.label}`;
    return item.submenu ? { [label]: shape(item.submenu) } : label;
  });

const find = (items: MenuItem[], label: string) =>
  items.find((item): item is MenuAction => item !== null && "label" in item && item.label === label)!;

function handlers(): AnnotationHandlers {
  return { close: vi.fn(), showDiff: vi.fn(), copy: vi.fn(), annotate: vi.fn(), options: vi.fn() };
}

describe("short_names_follow_shortnametype", () => {
  test.each([
    ["Vasya Pupkin <vasya.pupkin@jetbrains.com>", "lastname", "Pupkin"],
    ["Vasya Pupkin <vasya.pupkin@jetbrains.com>", "firstname", "Vasya"],
    ["Vasya Pupkin <vasya.pupkin@jetbrains.com>", "initials", "VP"],
    ["Vasya Pupkin <vasya.pupkin@jetbrains.com>", "full", "Vasya Pupkin"],
    ["Vasya Pupkin <vasya.pupkin@jetbrains.com>", "email", "vasya.pupkin@jetbrains.com"],
    ["Vasya <vasya.pupkin@jetbrains.com> Pupkin", "full", "Vasya Pupkin"],
    // Only an email: its name part, capitalized for first and last names.
    ["<vasya.pupkin@jetbrains.com>", "lastname", "Pupkin"],
    ["vasya.pupkin@email.com", "firstname", "Vasya"],
    ["vasya.pupkin@email.com", "initials", "VP"],
    // Whitespace collapses; initials split on the delimiters too.
    ["  Jean-Luc \t Picard ", "initials", "JLP"],
    ["  Jean-Luc \t Picard ", "lastname", "Picard"],
    // The email inside a name keeps the space before it, as `substring` leaves it.
    ["Vasya vasya@x.com Pupkin", "email", " vasya@x.com"],
    ["Vasya Pupkin", "email", "Vasya Pupkin"],
    ["", "lastname", ""],
  ] as const)("%s as %s is %s", (input, type, expected) => {
    expect(shortenName(input, type)).toBe(expected);
  });
});

test("the date is pretty: today, yesterday, else the short date", () => {
  const now = new Date(2026, 10, 9, 15, 0).getTime();
  expect(prettyDate(new Date(2026, 10, 9, 0, 5).getTime(), now)).toBe("Today");
  expect(prettyDate(new Date(2026, 10, 8, 23, 59).getTime(), now)).toBe("Yesterday");
  const older = new Date(2026, 10, 7, 12, 0).getTime();
  expect(prettyDate(older, now)).toBe(new Intl.DateTimeFormat(undefined, { dateStyle: "short" }).format(older));
});

test("the tooltip is text: commit, author, date, a moved path, then the message", () => {
  const moved = commit(OLD, { path: "src/old.txt" });
  const date = new Intl.DateTimeFormat(undefined, { dateStyle: "short", timeStyle: "short" }).format(
    moved.authorTime * 1000,
  );

  expect(tooltipText(annotation, annotation.commits[1])).toBe(
    `commit ${NEW}\nAuthor: Bob Jones <bob@example.com>\nDate: ${date}\n\nfirst\n\nits body`,
  );
  expect(tooltipText(annotation, moved)).toBe(
    `commit ${OLD}\nAuthor: Ann Smith <ann@example.com>\nDate: ${date}\nPath: src/old.txt\n\nfirst\n\nits body`,
  );
});

test("the default columns are Date and Author, blank on a line changed locally", () => {
  const now = new Date(2026, 10, 9).getTime();
  const date = prettyDate(annotation.commits[0].authorTime * 1000, now);

  const columns = annotationColumns(annotation, DEFAULT_OPTIONS, now);

  expect(columns.map((column) => column.aspect)).toEqual(["date", "author"]);
  expect(columns[0].texts).toEqual([date, "", date, date]);
  expect(columns[1].texts).toEqual(["Smith", "", "Smith", "Jones"]);
  expect(columns[1].width).toBe(5);
});

test("revision and commit number show when asked, the number padded to the largest", () => {
  const shown = { revision: true, date: false, author: false, number: true };

  const columns = annotationColumns(annotation, { ...DEFAULT_OPTIONS, shown }, 0);

  expect(columns.map((column) => column.aspect)).toEqual(["revision", "number"]);
  expect(columns[0].texts).toEqual(["11111111", "", "11111111", "22222222"]);
  expect(columns[1].texts).toEqual([" 1", "", " 1", "12"]);
  // Without a history of two or more there is no number column.
  const unnumbered = annotationColumns({ ...annotation, historySize: 0 }, { ...DEFAULT_OPTIONS, shown }, 0);
  expect(unnumbered.map((column) => column.aspect)).toEqual(["revision"]);
});

test("the background follows the colour mode", () => {
  const latest = annotation.commits[1];
  expect(background(latest, "order")).toBe("#2f5194");
  expect(background(latest, "author")).toBe("#2d4473");
  expect(background(latest, "none")).toBeNull();
});

test("the gutter popup has 241's entries in 241's order", () => {
  expect(shape(annotationPopup(annotation, 4, DEFAULT_OPTIONS, handlers()))).toEqual([
    "Close Annotations",
    "Show Diff",
    {
      View: [
        "Revision",
        "✓ Date",
        "✓ Author",
        "Commit number",
        "—",
        { Colors: ["Author", "✓ Order", "Hide"] },
        { Names: ["Initials", "✓ Last Name", "First Name", "Full Name", "Email"] },
      ],
    },
    "Copy Revision Number",
    "—",
    "Annotate Revision",
    "Annotate Previous Revision",
  ]);
});

test("on a line changed locally only the previous revision is the one annotated", () => {
  const run = handlers();
  const items = annotationPopup(annotation, 2, DEFAULT_OPTIONS, run);

  expect(shape(items).filter((item) => typeof item === "string")).toEqual([
    "Close Annotations",
    "(Show Diff)",
    "—",
    "(Annotate Revision)",
    "Annotate Previous Revision",
  ]);
  find(items, "Annotate Previous Revision").run!();
  // `getAnnotationLineNumber(.., approximate = true)`: where the insertion is in the revision.
  expect(run.annotate).toHaveBeenCalledWith({ hash: NEW, path: "src/a.txt" }, 2);
});

test("the popup's entries carry the line's commit", () => {
  const run = handlers();
  const items = annotationPopup(annotation, 4, DEFAULT_OPTIONS, run);

  find(items, "Copy Revision Number").run!();
  find(items, "Annotate Revision").run!();
  find(items, "Annotate Previous Revision").run!();
  find(items, "Show Diff").run!();
  find(items, "Close Annotations").run!();

  expect(run.copy).toHaveBeenCalledWith(NEW);
  // 241's quirk: the annotation's line (3), not the editor's (4), with the editor's text.
  expect(run.annotate).toHaveBeenNthCalledWith(1, { hash: NEW, path: "src/a.txt" }, 3);
  expect(run.annotate).toHaveBeenNthCalledWith(2, { hash: OLD, path: "src/old.txt" }, 3);
  expect(run.showDiff).toHaveBeenCalledWith(annotation.commits[1]);
  expect(run.close).toHaveBeenCalled();
  // The first commit has no previous revision.
  expect(find(annotationPopup(annotation, 1, DEFAULT_OPTIONS, run), "Annotate Previous Revision").disabled).toBe(true);
});

test("the view menu changes the options", () => {
  const run = handlers();
  const view = find(annotationPopup(annotation, 4, DEFAULT_OPTIONS, run), "View").submenu!;

  find(view, "Revision").run!();
  find(find(view, "Colors").submenu!, "Author").run!();
  find(find(view, "Names").submenu!, "Initials").run!();

  expect(run.options).toHaveBeenNthCalledWith(1, {
    ...DEFAULT_OPTIONS,
    shown: { ...DEFAULT_OPTIONS.shown, revision: true },
  });
  expect(run.options).toHaveBeenNthCalledWith(2, { ...DEFAULT_OPTIONS, colors: "author" });
  expect(run.options).toHaveBeenNthCalledWith(3, { ...DEFAULT_OPTIONS, names: "initials" });
});

// `ShowDiffFromAnnotation.loadRequests`: the whole commit, sorted by `ChangesComparator(true)`,
// positioned on the file (`findSelfInList`).
test("show_diff_opens_the_whole_commit_on_the_annotated_file", () => {
  const change = (path: string, status: string, oldPath: string | null = null) => ({ path, oldPath, status });
  const files = [
    change("src/z/b.txt", "M"),
    change("src/a.txt", "R100", "src/old.txt"),
    change("lib/file10.txt", "A"),
    change("lib/file9.txt", "D"),
    change("docs/a.txt", "M"),
  ];

  const { files: pairs, selected } = commitDiffPairs(annotation.commits[1], OLD, files);

  // By name, naturally (file9 before file10), then by path.
  expect(pairs.map((pair) => pair.right.path)).toEqual([
    "docs/a.txt",
    "src/a.txt",
    "src/z/b.txt",
    "lib/file9.txt",
    "lib/file10.txt",
  ]);
  expect(selected).toBe(1);
  expect(pairs[1]).toEqual({
    left: { revision: OLD, path: "src/old.txt", title: OLD },
    right: { revision: NEW, path: "src/a.txt", title: NEW },
  });
  expect(pairs[3].right).toEqual({ revision: null, path: "lib/file9.txt", title: NEW });
  expect(pairs[4].left).toEqual({ revision: null, path: "lib/file10.txt", title: "" });
  // A root commit has nothing on the left; a file not in the list is looked for by name.
  const root = commitDiffPairs({ ...annotation.commits[1], path: "moved/z/b.txt" }, null, files);
  expect(root.selected).toBe(2);
  expect(root.files.every((pair) => pair.left.revision === null)).toBe(true);
});
