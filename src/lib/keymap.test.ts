import { expect, test } from "vitest";
import { matchBranchPanelShortcut, matchDiffShortcut, matchShortcut } from "./keymap";

function press(init: KeyboardEventInit) {
  return matchShortcut(new KeyboardEvent("keydown", init));
}

test("with_option_the_physical_key_counts_even_if_the_keyboard_gives_a_space", () => {
  // On Spanish - ISO, ⌥⇧C gives a space, not "Ç".
  expect(press({ key: " ", code: "KeyC", metaKey: true, altKey: true, shiftKey: true })).toBe(
    "Vcs.CopyRevisionNumberAction",
  );
  expect(press({ key: " ", code: "Space", ctrlKey: true })).toBe("ChangesView.SetDefault");
});

test("matrix_shortcuts", () => {
  expect(press({ key: "k", code: "KeyK", metaKey: true })).toBe("CheckinProject");
  expect(press({ key: "˚", code: "KeyK", metaKey: true, altKey: true })).toBe(
    "Git.Commit.And.Push.Executor",
  );
  expect(press({ key: "1", code: "Digit1", metaKey: true })).toBe("ActivateProjectToolWindow");
  expect(press({ key: "0", code: "Digit0", metaKey: true })).toBe("ActivateCommitToolWindow");
  expect(press({ key: "z", code: "KeyZ", metaKey: true, altKey: true })).toBe("ChangesView.Revert");
  expect(press({ key: "Ω", code: "KeyZ", ctrlKey: true, altKey: true })).toBeNull();
  expect(press({ key: "å", code: "KeyA", metaKey: true, altKey: true })).toBe("ChangesView.AddUnversioned");
  expect(press({ key: "M", code: "KeyM", metaKey: true, shiftKey: true })).toBe("ChangesView.Move");
  expect(press({ key: "m", code: "KeyM", ctrlKey: true })).toBe("Vcs.ShowMessageHistory");
  expect(press({ key: "µ", code: "KeyM", ctrlKey: true, altKey: true })).toBe(
    "Vcs.ToggleAmendCommitMode",
  );
  expect(press({ key: "F2", code: "F2" })).toBe("ChangesView.Rename");
  expect(press({ key: "F6", code: "F6", shiftKey: true })).toBe("ChangesView.Rename");
  expect(press({ key: " ", code: "Space", ctrlKey: true })).toBe("ChangesView.SetDefault");
  expect(press({ key: "d", code: "KeyD", metaKey: true })).toBe("Diff.ShowDiff");
  expect(press({ key: "f", code: "KeyF", metaKey: true })).toBe("Vcs.Log.GoToRef");
  expect(press({ key: "l", code: "KeyL", metaKey: true })).toBe("Vcs.Log.FocusTextFilter");
  expect(press({ key: "r", code: "KeyR", metaKey: true })).toBe("Vcs.Log.Refresh");
  expect(press({ key: "ArrowRight", code: "ArrowRight" })).toBe("Vcs.Log.GoToParent");
  expect(press({ key: "ArrowLeft", code: "ArrowLeft" })).toBe("Vcs.Log.GoToChild");
  expect(press({ key: "F1", code: "F1" })).toBe("Vcs.Log.ShowTooltip");
  expect(press({ key: "j", code: "KeyJ", ctrlKey: true })).toBe("Vcs.Log.ShowTooltip");
  expect(press({ key: "Ç", code: "KeyC", metaKey: true, altKey: true, shiftKey: true })).toBe(
    "Vcs.CopyRevisionNumberAction",
  );
});

test("view_shortcuts", () => {
  expect(press({ key: "r", code: "KeyR", metaKey: true })).toBe("Vcs.Log.Refresh");
  expect(press({ key: "9", code: "Digit9", metaKey: true })).toBe(
    "ActivateVersionControlToolWindow",
  );
  expect(press({ key: "p", code: "KeyP", ctrlKey: true })).toBe("ChangesView.GroupBy.Directory");
  expect(press({ key: "Enter", code: "Enter", metaKey: true })).toBe("CommitView.Commit");
  expect(press({ key: "Enter", code: "Enter", ctrlKey: true })).toBe("CommitView.Commit");
  expect(press({ key: "=", code: "Equal", metaKey: true })).toBe("ExpandAll");
  expect(press({ key: "+", code: "BracketRight", metaKey: true })).toBe("ExpandAll");
  expect(press({ key: "-", code: "Minus", metaKey: true })).toBe("CollapseAll");
});

test("update_project_with_cmd_t", () => {
  expect(press({ key: "t", code: "KeyT", metaKey: true })).toBe("Vcs.UpdateProject");
  expect(press({ key: "k", code: "KeyK", metaKey: true, shiftKey: true })).toBe("Vcs.Push");
});

test("git_branches_has_no_shortcut_on_mac", () => {
  const chord = { ctrlKey: true, shiftKey: true, code: "Backquote" };

  expect(press({ key: "~", ...chord })).toBeNull();
  expect(press({ key: "º", ...chord })).toBeNull();
});

test("branches_panel_shortcuts", () => {
  const panel = (init: KeyboardEventInit) =>
    matchBranchPanelShortcut(new KeyboardEvent("keydown", init));

  expect(panel({ key: "F2", code: "F2" })).toBe("Git.Rename.Local.Branch");
  expect(panel({ key: "F6", code: "F6", shiftKey: true })).toBe("Git.Rename.Local.Branch");
  expect(panel({ key: "F1", code: "F1", altKey: true })).toBe(
    "Git.Log.Branches.Navigate.Log.To.Selected.Branch",
  );
  expect(panel({ key: "˜", code: "KeyN", altKey: true, metaKey: true })).toBe(
    "Git.New.Branch.In.Log",
  );
  expect(panel({ key: "F1", code: "F1" })).toBeNull();
  expect(panel({ key: "n", code: "KeyN", metaKey: true })).toBeNull();
});

test("f2_outside_the_panel_is_still_the_commit_views", () => {
  expect(press({ key: "F2", code: "F2" })).toBe("ChangesView.Rename");
  expect(press({ key: "F6", code: "F6", shiftKey: true })).toBe("ChangesView.Rename");
  expect(press({ key: "F1", code: "F1", altKey: true })).toBeNull();
  expect(press({ key: "˜", code: "KeyN", altKey: true, metaKey: true })).toBeNull();
});

test("without_a_shortcut_there_is_no_action", () => {
  expect(press({ key: "k", code: "KeyK" })).toBeNull();
  expect(press({ key: "K", code: "KeyK", ctrlKey: true, shiftKey: true })).toBeNull();
  expect(press({ key: "d", code: "KeyD", ctrlKey: true })).toBeNull();
  expect(press({ key: "ArrowRight", code: "ArrowRight", shiftKey: true })).toBeNull();
  expect(press({ key: "ArrowLeft", code: "ArrowLeft", metaKey: true })).toBeNull();
});

test("with_option_the_physical_key_is_read_and_without_it_the_keyboards", () => {
  expect(press({ key: "k", code: "KeyD", metaKey: true })).toBe("CheckinProject");
});

test("diff_window_shortcuts", () => {
  const press = (init: KeyboardEventInit) => matchDiffShortcut(new KeyboardEvent("keydown", init));

  expect(press({ key: "F7" })).toBe("NextDiff");
  expect(press({ key: "F7", shiftKey: true })).toBe("PreviousDiff");
  expect(press({ key: "ArrowRight", ctrlKey: true, shiftKey: true })).toBe("Diff.NextChange");
  expect(press({ key: "ArrowLeft", ctrlKey: true, shiftKey: true })).toBe("Diff.PrevChange");
  expect(press({ key: "s", metaKey: true })).toBe("SaveAll");
  expect(press({ key: "z", code: "KeyZ", metaKey: true, altKey: true })).toBe(
    "Vcs.RollbackChangedLines",
  );
  expect(press({ key: "M", metaKey: true, shiftKey: true })).toBe(
    "Vcs.MoveChangedLinesToChangelist",
  );
  expect(press({ key: "a" })).toBeNull();
});

test("⌘, opens the settings", () => {
  expect(matchShortcut(new KeyboardEvent("keydown", { key: ",", metaKey: true }))).toBe(
    "ShowSettings",
  );
});
