#!/usr/bin/env python3
"""Lists the actions and shortcuts of IntelliJ's XML files for the parity matrix."""

import argparse
import re
import sys
import xml.etree.ElementTree as ElementTree
from typing import NamedTuple, Optional

MAC_KEYMAPS = ("Mac OS X 10.5+", "Mac OS X", "$default")
OUT_OF_SCOPE = (
    r"^addQuickAction|^main\.toolbar|Toolbar|^git4idea\.|^Start\.Use\.Vcs|ShowOnDoubleClick"
    r"|CommittedChanges|IncomingChanges|IntegrateFiles|IntegrateProject|CheckStatusForFiles"
    r"|AnnotateStackTrace|CloseAllUnmodifiedEditors|MarkFileDirty|RunCommitChecks"
    r"|SwitchToCommitDialog|QuickListPopup|CopyPathFromRepositoryRoot"
    r"|CheckIgnoredAndNotExcludedDirectories"
    r"|^Git\.Log\.(GitDumpLogIndexDataAction|GitUseSharedLogAction|Show\.Command)$"
)
# From a file's history and from comparisons: phase 4 even though the name is a `Vcs.Log.` one.
LOG_HISTORY = (
    r"^Vcs\.Log\.(ShowAllAffected|ShowOtherBranches|AnnotateRevisionAction|CompareRevisions"
    r"|OpenRepositoryVersion)$"
)
# The branches popup actions carry `git4idea.` in the id, but they are not internal to the IDE
# (phase 3 decision 9): they are classified before the out-of-scope rule.
BRANCH_POPUP = r"^git4idea\.actions\.branch\."
RULES = (
    (BRANCH_POPUP + r"(GitMergeBranchAction|GitRebaseBranchAction|GitCheckoutWithRebaseAction"
     r"|GitPullBranchAction\$With(Merge|Rebase))$", "Conflicts and rebase", 3, "include"),
    (BRANCH_POPUP + r"(GitCompareWithBranchAction|GitShowDiffWithBranchAction)$",
     "History and comparison", 4, "include"),
    (BRANCH_POPUP, "Log and branches", 2, "include"),
    # The search toggles of the branches popup gear (241).
    (r"^git\.branches\.popup\.", "Log and branches", 2, "include"),
    (OUT_OF_SCOPE, "Out of scope", None, "out"),
    (LOG_HISTORY, "History and comparison", 4, "include"),
    (r"^Vcs\.Log\.|PushUpToCommit|CopyCommitSubject", "Log and branches", 2, "include"),
    (
        r"Rebase|Merge|Squash|Fixup|Drop\.Commits|Reword|CherryPick|ResolveConflicts"
        r"|AcceptTheirs|AcceptYours|ThreeSide|Three\.Versions|Revert\.|Uncommit|SelectedChanges",
        "Conflicts and rebase",
        3,
        "include",
    ),
    (r"Stash|Unstash|Shelv|Shelf|Patch", "Stash and shelf", 4, "include"),
    (
        r"Annotate|History|Show\.Current\.Revision|Compare|OpenRepositoryVersion"
        r"|GetVersion|BrowseRepo|ChangesView\.Browse",
        "History and comparison",
        4,
        "include",
    ),
    (
        r"Checkin|Commit|Stage|ChangesView|ChangedLines|MessageHistory|Git\.Add|Rollback"
        r"|Diff|ChangeMarker|EditSource|Show\.Local\.Changes",
        "Commit and diff",
        1,
        "include",
    ),
    (
        r"Vcs\.Log|Git\.Log|Branch|Tag|Push|Pull|Fetch|Update|Reset|Remote|Clone|Git\.Init"
        r"|Vcs\.Copy|Git\.Show|CheckoutRevision|SelectInGitLog|Vcs\.Show\.Log|ShowConsoleTab"
        r"|OpenExcludeFile",
        "Log and branches",
        2,
        "include",
    ),
)


# In 241 the staging area only exists in the non-modal commit interface; the user commits in the modal one.
NON_MODAL_ONLY = (r"^Git\.Stage\.", "only in non-modal mode with the staging area")
SECTION_FOUR = "## 4. "
HEADER = "| Area | Phase | Status | Feature | Action | Shortcut |"
PREVIOUS_ROW = re.compile(
    r"^\| (?P<area>[^|]*) \| (?P<phase>[^|]*) \| (?P<status>[^|]*) \| (?P<feature>.*?) \| `(?P<id>[^`]+)` "
    r"\| (?P<shortcut>.*?) \| (?P<result>.*) \|$"
)


class Action(NamedTuple):
    id: str
    cls: str
    group: Optional[str]


class PreviousRow(NamedTuple):
    area: str
    phase: str
    status: str
    feature: str
    shortcut: str
    result: str


def extract_actions(xml):
    actions = []

    def visit(element, group):
        for child in element:
            if child.tag == "action":
                actions.append(Action(child.get("id") or child.get("class"), child.get("class"), group))
            elif child.tag == "group":
                visit(child, child.get("id"))
            else:
                visit(child, group)

    visit(ElementTree.fromstring(xml), None)
    return actions


def extract_shortcuts(xml):
    shortcuts = {}
    for action in ElementTree.fromstring(xml).iter("action"):
        strokes = [
            " ".join(filter(None, [shortcut.get("first-keystroke"), shortcut.get("second-keystroke")]))
            for shortcut in action.iter("keyboard-shortcut")
        ]
        if strokes:
            shortcuts[action.get("id")] = strokes
    return shortcuts


def extract_inline_shortcuts(xml):
    shortcuts = {}
    for action in ElementTree.fromstring(xml).iter("action"):
        by_keymap = {}
        removed = set()
        for shortcut in action.findall("keyboard-shortcut"):
            stroke = " ".join(
                filter(None, [shortcut.get("first-keystroke"), shortcut.get("second-keystroke")])
            )
            keymap = shortcut.get("keymap")
            if shortcut.get("remove") != "true":
                by_keymap.setdefault(keymap, []).append(stroke)
            elif keymap in MAC_KEYMAPS:
                removed.add(stroke)
        for keymap in MAC_KEYMAPS:
            if keymap in by_keymap:
                strokes = by_keymap[keymap]
                if keymap == "$default":
                    strokes = [convert_from_parent(stroke) for stroke in strokes]
                strokes = [stroke for stroke in strokes if stroke not in removed]
                if strokes:
                    shortcuts[action.get("id")] = strokes
                break
    return shortcuts


def convert_from_parent(stroke):
    """Rule of MacOSDefaultKeymap.convertShortcutFromParent: swaps control and meta."""
    swapped = {"control": "meta", "ctrl": "meta", "meta": "control"}
    return " ".join(swapped.get(word, word) for word in stroke.split())


def inherit_shortcuts(keymaps):
    """Takes the keymaps from the most general to the most specific."""
    merged = {}
    for xml in keymaps:
        if ElementTree.fromstring(xml).get("parent"):
            merged = {
                action_id: [convert_from_parent(stroke) for stroke in strokes]
                for action_id, strokes in merged.items()
            }
        merged.update(extract_shortcuts(xml))
    return merged


def extract_bindings(xml):
    return {
        action.get("id"): action.get("use-shortcut-of")
        for action in ElementTree.fromstring(xml).iter("action")
        if action.get("use-shortcut-of")
    }


def shortcut_of(action_id, shortcuts, bindings):
    while action_id not in shortcuts and action_id in bindings:
        action_id = bindings[action_id]
    return shortcuts.get(action_id, [])


def classify(action_id):
    for pattern, area, phase, status in RULES:
        if re.search(pattern, action_id):
            return area, phase, status
    return "Unclassified", None, "unsure"


def action_names(bundle_paths):
    names = {}
    for path in bundle_paths:
        with open(path, encoding="utf-8") as bundle:
            for line in bundle:
                match = re.match(r"(?:action|group)\.(.+)\.text=(.*)", line)
                if match:
                    text = match.group(2).encode().decode("unicode_escape", "ignore")
                    names.setdefault(match.group(1), text.replace("_", "").replace("&", ""))
    return names


def pretty(strokes):
    replacements = (
        ("meta", "⌘"),
        ("control", "⌃"),
        ("ctrl", "⌃"),
        ("shift", "⇧"),
        ("Shift", "⇧"),
        ("alt", "⌥"),
    )
    text = ", ".join(strokes).replace("pressed ", "")
    for name, symbol in replacements:
        text = text.replace(name, symbol)
    return text


def parse_previous(matrix):
    """Reads the rows of section 4 of a matrix, by action id. The Result column is written by hand."""
    previous = {}
    in_section = False
    for line in matrix.splitlines():
        if line.startswith("## "):
            in_section = line.startswith(SECTION_FOUR)
        match = PREVIOUS_ROW.match(line) if in_section else None
        if match:
            previous[match["id"]] = PreviousRow(
                *(match[key] for key in ("area", "phase", "status", "feature", "shortcut", "result"))
            )
    return previous


def feature_text(action_id, name):
    pattern, note = NON_MODAL_ONLY
    if not re.search(pattern, action_id):
        return name
    return f"{name} ({note})" if name else f"({note})"


def render_table(rows, previous=None):
    """The lines of the table. With a previous matrix it adds the Result column, kept by action id."""
    header = HEADER if previous is None else HEADER + " Result |"
    lines = [header, "|---|---|---|---|---|---|" + ("" if previous is None else "---|")]
    order = lambda item: (item[1][0][1] or 9, item[1][0][0], item[0])
    for action_id, ((area, phase, status), name, strokes) in sorted(rows.items(), key=order):
        cells = f"| {area} | {phase or '-'} | {status} | {feature_text(action_id, name)} | `{action_id}` | {pretty(strokes)} |"
        if previous is not None:
            old = previous.get(action_id)
            cells += f" {old.result if old else ''} |"
        lines.append(cells)
    return lines


def removed_rows(rows, previous):
    return sorted((action_id, old) for action_id, old in previous.items() if action_id not in rows)


def render_removed(removed):
    if not removed:
        return []
    lines = [
        "### Removed in 241",
        "",
        "Actions of the previous reference that 2024.1 no longer declares. They left the table above; their old rows are kept here.",
        "",
        HEADER + " Result |",
        "|---|---|---|---|---|---|---|",
    ]
    for action_id, old in removed:
        lines.append(
            f"| {old.area} | {old.phase} | {old.status} | {old.feature} | `{action_id}` | {old.shortcut} | {old.result} |"
        )
    return lines


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--actions", nargs="+", required=True)
    parser.add_argument("--keymaps", nargs="+", required=True, help="from the most general to the most specific")
    parser.add_argument("--bundles", nargs="+", required=True)
    parser.add_argument("--previous", help="the current matrix: its Result column is kept by action id")
    args = parser.parse_args()

    keymaps = []
    for path in args.keymaps:
        with open(path, encoding="utf-8") as keymap:
            keymaps.append(keymap.read())
    action_xmls = []
    for path in args.actions:
        with open(path, encoding="utf-8") as actions:
            action_xmls.append(actions.read())

    shortcuts = {}
    bindings = {}
    for xml in action_xmls:
        shortcuts.update(extract_inline_shortcuts(xml))
        bindings.update(extract_bindings(xml))
    shortcuts.update(inherit_shortcuts(keymaps))

    names = action_names(args.bundles)
    rows = {}
    for xml in action_xmls:
        for action in extract_actions(xml):
            strokes = shortcut_of(action.id, shortcuts, bindings)
            rows[action.id] = (classify(action.id), names.get(action.id, ""), strokes)

    previous = None
    if args.previous:
        with open(args.previous, encoding="utf-8") as matrix:
            previous = parse_previous(matrix.read())
    print("\n".join(render_table(rows, previous)))
    if previous is not None:
        removed = removed_rows(rows, previous)
        if removed:
            print("\n" + "\n".join(render_removed(removed)))
            print("removed: " + ", ".join(action_id for action_id, _ in removed), file=sys.stderr)


if __name__ == "__main__":
    main()
