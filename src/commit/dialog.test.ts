import { describe, expect, test } from "vitest";
import type { CheckinRequest } from "./checkin";
import {
  canCommit,
  dialogTexts,
  expandPaths,
  initialInclusion,
  initialList,
  initialMessage,
  knownMessage,
  onClosed,
  onListChanged,
  type ListWithFiles,
} from "./dialog";

const request = (
  kind: CheckinRequest["kind"],
  paths: string[] = [],
  lists: string[] = [],
): CheckinRequest => ({ kind, paths, lists });

const list = (id: string, name: string, files: string[], comment = ""): ListWithFiles => ({
  id,
  name,
  comment,
  files,
});

const standard = list("default", "Changes", ["a.txt", "b.txt"]);
const other = list("other", "Other", ["c.txt", "d.txt"]);

describe("initial inclusion", () => {
  test("with nothing selected the whole initial list is included", () => {
    expect(initialInclusion(request("project"), standard, ["u.txt"])).toEqual(
      new Set(["a.txt", "b.txt"]),
    );
  });

  test("with files selected only those are included", () => {
    const selected = request("project", ["b.txt", "u.txt"]);

    expect(initialInclusion(selected, standard, ["u.txt"])).toEqual(new Set(["b.txt", "u.txt"]));
  });

  test("what the list does not show is not included", () => {
    expect(initialInclusion(request("files", ["c.txt"]), standard, [])).toEqual(new Set());
  });
});

describe("initial list", () => {
  const lists = [standard, other];

  test("the initial list is the selected list, else the list of the first selected change, else the active list", () => {
    expect(initialList(request("project", ["a.txt"], ["other"]), lists, "default")).toBe("other");
    expect(initialList(request("project", ["u.txt", "d.txt", "a.txt"]), lists, "default")).toBe(
      "other",
    );
    expect(initialList(request("project"), lists, "other")).toBe("other");
    expect(initialList(request("project", ["u.txt"], ["gone"]), lists, "default")).toBe("default");
  });

  test("Commit File picks the active list if it holds one of the files, else the only affected list", () => {
    expect(initialList(request("files", ["c.txt", "a.txt"]), lists, "default")).toBe("default");
    expect(initialList(request("files", ["c.txt", "d.txt"]), lists, "default")).toBe("other");
    expect(initialList(request("files", ["a.txt", "c.txt"]), lists, "third")).toBe("third");
    expect(initialList(request("files", ["u.txt"]), lists, "default")).toBe("default");
  });
});

describe("message", () => {
  test("the message is the list comment, then a non-default name, then MERGE_MSG, then the last commit message", () => {
    const commented = list("o", "Other", [], "draft");
    const named = list("o", "Other", []);
    const plain = list("default", "Changes", []);
    const input = { mergeMessage: "Merge branch 'x'", lastMessage: "previous", clearInitial: false };

    expect(initialMessage({ ...input, list: commented })).toBe("draft");
    expect(initialMessage({ ...input, list: { ...plain, comment: "  \n" } })).toBe(
      "Merge branch 'x'",
    );
    expect(initialMessage({ ...input, list: named })).toBe("Other");
    expect(initialMessage({ ...input, list: plain })).toBe("Merge branch 'x'");
    expect(initialMessage({ ...input, mergeMessage: undefined, list: plain })).toBe("previous");
    expect(initialMessage({ list: plain, clearInitial: false })).toBe("");
  });

  test("the old default names are default names", () => {
    for (const name of ["Default Changelist", "Default"]) {
      const old = list("d", name, []);

      expect(initialMessage({ list: old, lastMessage: "previous", clearInitial: false })).toBe(
        "previous",
      );
    }
  });

  test("a prefilled text wins over everything but nothing wins over Clear initial", () => {
    const commented = list("o", "Other", [], "draft");

    expect(initialMessage({ prefilled: "fixup! x", list: commented, clearInitial: false })).toBe(
      "fixup! x",
    );
  });

  test("Clear initial commit message leaves only the prefilled text", () => {
    const commented = list("o", "Other", [], "draft");
    const input = { list: commented, mergeMessage: "merge", lastMessage: "previous" };

    expect(initialMessage({ ...input, prefilled: "squash! x", clearInitial: true })).toBe(
      "squash! x",
    );
    expect(initialMessage({ ...input, clearInitial: true })).toBe("");
  });

  test("changing the list saves the draft as the old list's comment", () => {
    const from = list("default", "Changes", []);
    const to = list("other", "Other", [], "its draft");

    expect(onListChanged("my draft", from, to)).toEqual({
      save: { id: "default", comment: "my draft" },
      message: "its draft",
    });
    // Without a comment, the name; failing that, the text known when the dialog opened.
    expect(onListChanged("x", from, list("o", "Named", [])).message).toBe("Named");
    expect(onListChanged("x", to, from, "known").message).toBe("known");
    expect(onListChanged("x", to, from).message).toBe("");
  });

  test("cancel keeps the message as the list comment", () => {
    const current = list("other", "Other", [], "old");

    expect(onClosed({ committed: false, draft: "kept", list: current, fullyIncluded: true })).toEqual(
      { id: "other", comment: "kept" },
    );
    expect(onClosed({ committed: false, draft: "kept", list: current, fullyIncluded: false })).toEqual(
      { id: "other", comment: "kept" },
    );
  });

  test("a full commit of the default list clears its comment", () => {
    const standardList = list("default", "Changes", [], "old");

    expect(
      onClosed({ committed: true, draft: "done", list: standardList, fullyIncluded: true }),
    ).toEqual({ id: "default", comment: "" });
    // A named list keeps the message; a partial commit keeps the original comment.
    expect(
      onClosed({ committed: true, draft: "done", list: other, fullyIncluded: true }),
    ).toEqual({ id: "other", comment: "done" });
    expect(
      onClosed({ committed: true, draft: "done", list: standardList, fullyIncluded: false }),
    ).toBeNull();
  });
});

describe("executors", () => {
  test("the shelve and patch variants have their own title and one button and no options", () => {
    expect(dialogTexts("commit")).toEqual({
      title: "Commit Changes",
      primary: "Commit",
      dropdown: ["Commit and Push…"],
      options: true,
      unversioned: true,
    });
    expect(dialogTexts("shelve")).toEqual({
      title: "Shelve Changes",
      primary: "Shelve Changes",
      dropdown: [],
      options: false,
      unversioned: false,
    });
    expect(dialogTexts("patch")).toEqual({
      title: "Create Patch",
      primary: "Create Patch…",
      dropdown: [],
      options: false,
      unversioned: false,
    });
  });

  test("nothing included disables the buttons even with amend", () => {
    expect(canCommit(new Set())).toBe(false);
    expect(canCommit(new Set(["a.txt"]))).toBe(true);
  });
});

describe("Commit File roots", () => {
  test("a directory stands for the changed files under it, and unversioned ones only if chosen", () => {
    const changed = ["src/a.txt", "src/deep/b.txt", "srcx.txt", "top.txt"];
    const unversioned = ["src/new.txt", "picked.txt"];

    const expanded = expandPaths(
      request("files", ["src", "picked.txt", "top.txt"]),
      changed,
      unversioned,
    );

    expect(expanded.paths).toEqual(["src/a.txt", "src/deep/b.txt", "picked.txt", "top.txt"]);
    expect(initialInclusion(expanded, list("l", "Changes", changed), unversioned)).toEqual(
      new Set(["src/a.txt", "src/deep/b.txt", "picked.txt", "top.txt"]),
    );
    expect(expandPaths(request("project", ["src"]), changed, unversioned).paths).toEqual(["src"]);
    // The repository's own folder, the root of the Project tree.
    expect(expandPaths(request("files", [""]), changed, unversioned).paths).toEqual(changed);
  });
});

describe("the text a new list falls back to", () => {
  test("is the prefilled one, else the last commit message once the list had none", () => {
    const named = list("n", "Feature", []);

    expect(knownMessage({ prefilled: "fixup! x", list: named, lastMessage: "last" })).toBe(
      "fixup! x",
    );
    expect(knownMessage({ list: standard, lastMessage: "last" })).toBe("last");
    expect(knownMessage({ list: named, lastMessage: "last" })).toBeUndefined();
    expect(knownMessage({ list: { ...standard, comment: "draft" }, lastMessage: "last" })).toBe(
      undefined,
    );
  });
});
