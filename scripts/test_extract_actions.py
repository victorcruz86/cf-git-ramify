import unittest

from extract_actions import (
    Action,
    classify,
    convert_from_parent,
    extract_actions,
    extract_bindings,
    extract_inline_shortcuts,
    extract_shortcuts,
    inherit_shortcuts,
    parse_previous,
    pretty,
    removed_rows,
    render_removed,
    render_table,
    shortcut_of,
)

PLUGIN_XML = """
<idea-plugin>
  <actions>
    <action id="Git.Top" class="git4idea.actions.Top"/>
    <group id="Git.Menu">
      <action id="Git.Init" class="git4idea.actions.GitInit"/>
      <group id="Git.Sub">
        <action id="Git.Nested" class="git4idea.actions.Nested"/>
      </group>
    </group>
  </actions>
</idea-plugin>
"""

KEYMAP_XML = """
<keymap>
  <action id="Git.Init"><keyboard-shortcut first-keystroke="meta K"/></action>
  <action id="Git.Other"><mouse-shortcut keystroke="button1"/></action>
</keymap>
"""


class ExtractActionsTest(unittest.TestCase):
    def test_extracts_actions(self):
        actions = extract_actions(PLUGIN_XML)
        self.assertIn(Action("Git.Init", "git4idea.actions.GitInit", "Git.Menu"), actions)

    def test_nested_group_keeps_the_immediate_one(self):
        actions = extract_actions(PLUGIN_XML)
        self.assertIn(Action("Git.Nested", "git4idea.actions.Nested", "Git.Sub"), actions)

    def test_action_without_id_uses_its_class(self):
        actions = extract_actions('<a><action class="x.Y"/></a>')
        self.assertEqual(actions, [Action("x.Y", "x.Y", None)])

    def test_action_without_group(self):
        actions = extract_actions(PLUGIN_XML)
        self.assertIn(Action("Git.Top", "git4idea.actions.Top", None), actions)


class ExtractShortcutsTest(unittest.TestCase):
    def test_only_keyboard_shortcuts(self):
        self.assertEqual(extract_shortcuts(KEYMAP_XML), {"Git.Init": ["meta K"]})


class InlineShortcutsTest(unittest.TestCase):
    def test_prefers_the_mac_keymap(self):
        xml = """<a><action id="X" class="c">
          <keyboard-shortcut first-keystroke="control K" keymap="$default"/>
          <keyboard-shortcut first-keystroke="meta shift K" keymap="Mac OS X 10.5+"/>
        </action></a>"""
        self.assertEqual(extract_inline_shortcuts(xml), {"X": ["meta shift K"]})

    def test_converts_control_to_meta_from_the_default_keymap(self):
        xml = """<a><action id="X" class="c">
          <keyboard-shortcut first-keystroke="control K" keymap="$default"/>
        </action></a>"""
        self.assertEqual(extract_inline_shortcuts(xml), {"X": ["meta K"]})


    def test_the_default_keymap_swaps_meta_and_control(self):
        xml = """<a><action id="X" class="c">
          <keyboard-shortcut first-keystroke="meta K" keymap="$default"/>
        </action></a>"""
        self.assertEqual(extract_inline_shortcuts(xml), {"X": ["control K"]})

    def test_removed_on_mac_is_not_inherited(self):
        xml = """<a><action id="Git.Branches" class="c">
          <keyboard-shortcut first-keystroke="control shift BACK_QUOTE" keymap="$default"/>
          <keyboard-shortcut first-keystroke="meta shift BACK_QUOTE" keymap="Mac OS X" remove="true"/>
          <keyboard-shortcut first-keystroke="meta shift BACK_QUOTE" keymap="Mac OS X 10.5+" remove="true"/>
        </action></a>"""
        self.assertEqual(extract_inline_shortcuts(xml), {})

    def test_removed_in_another_keymap_does_not_affect_mac(self):
        xml = """<a><action id="X" class="c">
          <keyboard-shortcut first-keystroke="control L" keymap="$default"/>
          <keyboard-shortcut first-keystroke="meta L" keymap="Visual Studio" remove="true"/>
        </action></a>"""
        self.assertEqual(extract_inline_shortcuts(xml), {"X": ["meta L"]})


DEFAULT_KEYMAP = """
<keymap name="$default">
  <action id="Rollback"><keyboard-shortcut first-keystroke="control alt Z"/></action>
  <action id="Own"><keyboard-shortcut first-keystroke="control P"/></action>
  <action id="Meta"><keyboard-shortcut first-keystroke="meta alt N"/></action>
  <action id="Rename"><keyboard-shortcut first-keystroke="F2"/></action>
</keymap>
"""

MAC_KEYMAP = """
<keymap name="Mac OS X 10.5+" parent="$default">
  <action id="Own"><keyboard-shortcut first-keystroke="control P"/></action>
</keymap>
"""


class InheritShortcutsTest(unittest.TestCase):
    def test_converts_like_the_mac_keymap(self):
        self.assertEqual(convert_from_parent("control alt Z"), "meta alt Z")
        self.assertEqual(convert_from_parent("meta alt N"), "control alt N")
        self.assertEqual(convert_from_parent("control shift H control K"), "meta shift H meta K")
        self.assertEqual(convert_from_parent("shift ENTER"), "shift ENTER")

    def test_what_is_inherited_from_default_is_converted(self):
        merged = inherit_shortcuts([DEFAULT_KEYMAP, MAC_KEYMAP])
        self.assertEqual(merged["Rollback"], ["meta alt Z"])
        self.assertEqual(merged["Meta"], ["control alt N"])
        self.assertEqual(merged["Rename"], ["F2"])

    def test_what_belongs_to_the_mac_keymap_is_not_converted(self):
        merged = inherit_shortcuts([DEFAULT_KEYMAP, MAC_KEYMAP])
        self.assertEqual(merged["Own"], ["control P"])

    def test_without_a_child_keymap_there_is_no_conversion(self):
        self.assertEqual(inherit_shortcuts([DEFAULT_KEYMAP])["Rollback"], ["control alt Z"])


class BindingsTest(unittest.TestCase):
    def test_reads_use_shortcut_of(self):
        xml = '<a><action id="Git.Add" class="c" use-shortcut-of="ChangesView.AddUnversioned"/></a>'
        self.assertEqual(extract_bindings(xml), {"Git.Add": "ChangesView.AddUnversioned"})

    def test_takes_the_shortcut_of_the_linked_action(self):
        shortcuts = {"B": ["meta K"]}
        self.assertEqual(shortcut_of("A", shortcuts, {"A": "B"}), ["meta K"])

    def test_own_shortcut_wins(self):
        shortcuts = {"A": ["meta J"], "B": ["meta K"]}
        self.assertEqual(shortcut_of("A", shortcuts, {"A": "B"}), ["meta J"])

    def test_follows_the_chain(self):
        self.assertEqual(shortcut_of("A", {"C": ["F4"]}, {"A": "B", "B": "C"}), ["F4"])

    def test_without_shortcut(self):
        self.assertEqual(shortcut_of("A", {}, {}), [])


class PrettyTest(unittest.TestCase):
    def test_symbols(self):
        self.assertEqual(pretty(["meta alt Z"]), "⌘ ⌥ Z")
        self.assertEqual(pretty(["F2", "Shift F6"]), "F2, ⇧ F6")


class ClassifyTest(unittest.TestCase):
    def test_rules_by_area(self):
        self.assertEqual(classify("Git.Interactive.Rebase"), ("Conflicts and rebase", 3, "include"))
        self.assertEqual(classify("Git.Stash.Pop"), ("Stash and shelf", 4, "include"))

    def test_actions_with_ambiguous_name(self):
        self.assertEqual(classify("Git.Unstash")[0], "Stash and shelf")
        self.assertEqual(classify("Vcs.ShowConsoleTab")[0], "Log and branches")
        self.assertEqual(classify("Vcs.RevertSelectedChanges")[0], "Conflicts and rebase")
        self.assertEqual(classify("Vcs.RunCommitChecks")[2], "out")

    def test_log_actions_do_not_go_to_commit(self):
        self.assertEqual(classify("Vcs.Log.ShowDiffPreview")[0], "Log and branches")
        self.assertEqual(classify("Git.PushUpToCommit")[0], "Log and branches")
        self.assertEqual(classify("Vcs.CopyCommitSubjectAction")[0], "Log and branches")

    def test_show_all_affected_in_phase_4(self):
        # Phase 2 decision 11: they only act on a file's history.
        for action in ("Vcs.Log.ShowAllAffected", "Vcs.Log.ShowOtherBranches"):
            self.assertEqual(classify(action), ("History and comparison", 4, "include"), action)

    def test_the_history_log_actions_are_in_phase_4(self):
        # They were already in phase 4 of the matrix; the rules took them to phase 2.
        for action in (
            "Vcs.Log.AnnotateRevisionAction",
            "Vcs.Log.CompareRevisions",
            "Vcs.Log.OpenRepositoryVersion",
        ):
            self.assertEqual(classify(action), ("History and comparison", 4, "include"), action)

    def test_the_rest_of_vcs_log_stays_in_phase_2(self):
        for action in ("Vcs.Log.ShowDetailsAction", "Vcs.Log.ShowTooltip", "Vcs.Log.Refresh"):
            self.assertEqual(classify(action), ("Log and branches", 2, "include"), action)

    def test_branches_popup_actions_in_their_phase(self):
        # Phase 3 decision 9: `^git4idea\.` left them out, but they are the branches popup actions.
        prefix = "git4idea.actions.branch."
        phase_3 = (
            "GitMergeBranchAction",
            "GitRebaseBranchAction",
            "GitCheckoutWithRebaseAction",
            "GitPullBranchAction$WithMerge",
            "GitPullBranchAction$WithRebase",
        )
        phase_2 = (
            "GitCheckoutAction",
            "GitCheckoutAsNewBranch",
            "GitCheckoutFromInputAction",
            "GitNewBranchAction",
            "GitDeleteBranchAction",
            "GitRenameBranchAction",
            "GitPushBranchAction",
            "GitUpdateSelectedBranchAction",
        )
        phase_4 = ("GitCompareWithBranchAction", "GitShowDiffWithBranchAction")
        for action in phase_3:
            self.assertEqual(classify(prefix + action), ("Conflicts and rebase", 3, "include"), action)
        for action in phase_2:
            self.assertEqual(classify(prefix + action), ("Log and branches", 2, "include"), action)
        for action in phase_4:
            self.assertEqual(classify(prefix + action), ("History and comparison", 4, "include"), action)

    def test_the_rest_of_git4idea_stays_out_of_scope(self):
        for action in (
            "git4idea.actions.GitDumpLogIndexDataAction",
            "git4idea.commands.TestGitHttpLoginDialogAction",
            "git4idea.ui.branch.popup.GitBranchesTreePopupResizeAction",
        ):
            self.assertEqual(classify(action), ("Out of scope", None, "out"), action)

    def test_the_internal_log_actions_of_241_stay_out_of_scope(self):
        # 241 gave ids of the Git.Log. family to the internal actions that 232 declared as `git4idea.actions.*`.
        for action in (
            "Git.Log.GitDumpLogIndexDataAction",
            "Git.Log.GitUseSharedLogAction",
            "Git.Log.Show.Command",
        ):
            self.assertEqual(classify(action), ("Out of scope", None, "out"), action)

    def test_the_branches_popup_search_toggles_are_in_phase_2(self):
        for action in ("git.branches.popup.filter.by.action", "git.branches.popup.filter.by.repository"):
            self.assertEqual(classify(action), ("Log and branches", 2, "include"), action)

    def test_without_a_rule_it_stays_unsure(self):
        self.assertEqual(classify("Something.Unknown"), ("Unclassified", None, "unsure"))


PREVIOUS_MATRIX = """# Parity matrix

## 2. Parts of the interface without an action of their own

| Area | Phase | Status | Feature | Result |
|---|---|---|---|---|
| Commit and diff | 1 | include | Commit window | done: Test `commit_window`. |

## 4. IntelliJ actions

| Area | Phase | Status | Feature | Action | Shortcut | Result |
|---|---|---|---|---|---|---|
| Commit and diff | 1 | include | Add | `Git.Add` | ⌘ ⌥ A | partial: Same as `ChangesView.AddUnversioned`. |
| Commit and diff | 1 | include |  | `Git.Stage.Add` | ⌘ ⌥ A | done: Test `stage_add`. |
| Commit and diff | 1 | include | Refresh | `ChangesView.Refresh` |  |  |
| History and comparison | 4 | include | Show Other Branches | `Vcs.Log.ShowOtherBranches` |  | pending: It should be dropped. |
"""

NEW_ROWS = {
    "Git.Add": (("Commit and diff", 1, "include"), "Add", ["meta alt A"]),
    "Git.Stage.Add": (("Commit and diff", 1, "include"), "", ["meta alt A"]),
    "ChangesView.Refresh": (("Commit and diff", 1, "include"), "Refresh", []),
    "Vcs.Log.Refresh": (("Log and branches", 2, "include"), "Refresh", []),
}


class PreviousMatrixTest(unittest.TestCase):
    def test_reads_the_result_of_each_action_of_section_4_only(self):
        previous = parse_previous(PREVIOUS_MATRIX)
        self.assertEqual(
            set(previous), {"Git.Add", "Git.Stage.Add", "ChangesView.Refresh", "Vcs.Log.ShowOtherBranches"}
        )
        self.assertEqual(previous["Git.Stage.Add"].result, "done: Test `stage_add`.")
        self.assertEqual(previous["ChangesView.Refresh"].result, "")

    def test_results_are_kept_by_action_id(self):
        table = render_table(NEW_ROWS, parse_previous(PREVIOUS_MATRIX))
        self.assertEqual(table[0], "| Area | Phase | Status | Feature | Action | Shortcut | Result |")
        self.assertIn(
            "| Commit and diff | 1 | include | Add | `Git.Add` | ⌘ ⌥ A | partial: Same as `ChangesView.AddUnversioned`. |",
            table,
        )
        self.assertIn("| Commit and diff | 1 | include | Refresh | `ChangesView.Refresh` |  |  |", table)

    def test_a_new_action_has_an_empty_result(self):
        table = render_table(NEW_ROWS, parse_previous(PREVIOUS_MATRIX))
        self.assertIn("| Log and branches | 2 | include | Refresh | `Vcs.Log.Refresh` |  |  |", table)

    def test_without_a_previous_matrix_the_table_has_no_result_column(self):
        table = render_table(NEW_ROWS)
        self.assertEqual(table[0], "| Area | Phase | Status | Feature | Action | Shortcut |")
        self.assertIn("| Log and branches | 2 | include | Refresh | `Vcs.Log.Refresh` |  |", table)

    def test_removed_actions_are_listed_apart(self):
        previous = parse_previous(PREVIOUS_MATRIX)
        table = render_table(NEW_ROWS, previous)
        self.assertFalse([line for line in table if "Vcs.Log.ShowOtherBranches" in line])
        removed = removed_rows(NEW_ROWS, previous)
        self.assertEqual([action_id for action_id, _ in removed], ["Vcs.Log.ShowOtherBranches"])
        listing = render_removed(removed)
        self.assertIn("### Removed in 241", listing)
        self.assertIn("| Area | Phase | Status | Feature | Action | Shortcut | Result |", listing)
        self.assertIn("`Vcs.Log.ShowOtherBranches`", "\n".join(listing))
        self.assertIn("pending: It should be dropped.", "\n".join(listing))

    def test_nothing_removed_prints_no_list(self):
        self.assertEqual(render_removed([]), [])

    def test_stage_actions_are_marked_non_modal(self):
        table = render_table(NEW_ROWS, parse_previous(PREVIOUS_MATRIX))
        stage = [line for line in table if "`Git.Stage.Add`" in line][0]
        self.assertIn("only in non-modal mode with the staging area", stage)
        # The hand-written Result stays as it was.
        self.assertTrue(stage.endswith("| done: Test `stage_add`. |"))
        add = [line for line in table if "`Git.Add`" in line][0]
        self.assertNotIn("non-modal", add)

    def test_the_mark_goes_after_the_name_of_the_feature(self):
        rows = {"Git.Stage.Add.All": (("Commit and diff", 1, "include"), "Stage All", [])}
        table = render_table(rows)
        self.assertIn("| Stage All (only in non-modal mode with the staging area) |", table[2])


if __name__ == "__main__":
    unittest.main()
