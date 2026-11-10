# Phase 3: decisions and pending items

Record of what was decided during execution and of the minor findings that were left unattended. Each decision says what it costs if it turns out to be wrong. The 10 decisions of the plan (Decisions section of `docs/superpowers/plans/2026-11-02-phase-3-conflicts-and-rewriting.md`) are noted in section 4 of the design and are not repeated among the execution ones.

The labels come from the execution log (`.superpowers/sdd/2026-11-02-phase-3-conflicts-and-rewriting/progress.md`), with these changes: a `Ruling` is a line of Decisions (rulings R1 to R5 of the plan's previous review are `Preparation`; R6 belongs to Task 4), and `minor (deferred)` is a line of Minor findings. `Final` is the final review of the branch, which is done after this report. A finding with `[PRIORITY in the final review]` or `[review in the final]` is one that the task's reviewer asked to look at again. What the log marks as resolved within its task is not repeated here.

## Where the app differs from IntelliJ on purpose

Only what was decided: the plan's decisions and the task rulings. What the validation found without anyone deciding it is in `phase-3-validation.md`.

From the plan's decisions (the user approved them together with it):

- Everything is saved with `git stash`: there is no Shelve until phase 4. Rebase, Smart Merge and editing from the log use IntelliJ's messages (decision 1).
- The git editor receives `GIT_SEQUENCE_EDITOR` in addition to `GIT_EDITOR`, so a user's `sequence.editor` does not win over it (decision 2).
- A single merge window: asking for another file while it is open brings it to the front without changing file (decision 3).
- The diff from base to each side is Myers (`similar`), not IntelliJ's "fair" one; in ambiguous cases a block may start or end one line earlier or later. The 20 cases of `LineComparisonMergeUtilTest` match and the 2 of `testRegressions` are `#[ignore]` because of this decision (decision 4 and Task 4).
- Recoverable copy before Accept Yours, Accept Theirs and Apply (decision 5).
- `Show Details` and `Filter by conflicted file` go to phase 4 (decision 6). There is no `Conflicts` window and no "Resolving merge conflicts is in progress" bar (decision 7).
- Cherry-Pick and Revert Selected Changes use `git apply`, and `--3way` if it does not go in cleanly (decision 8).
- The classification of the `git4idea.actions.branch.*` rows changed (decision 9). The `using log` mode of the interactive rebase is the only one that opens the dialog before launching git (decision 10).

From the task rulings:

- **Sides of a conflict.** `conflicts::accept` receives the side as the user sees it and is the only place where it is inverted in a rebase (R1). The conflict type is labeled from the user's side: a deletion in a normal merge says `deleted by you` where IntelliJ says `deleted by them` (Task 3). Short hashes are 8 characters; the rebase description does not carry `revision <hash>`; a file that is not UTF-8 is binary and only admits Accept Left/Right (Task 3). `accept` skips without error what is no longer in conflict (Task 3).
- **Merge window.** It normalizes CRLF to LF when loading and writes with the working file's separator (R6). `Resolve Automatically` also applies the non-conflicting changes, like `applyResolvableConflictedChanges` (Task 5). Ranges follow the text with a port of `updateRangeOnModification`; deleting a block at line 0 does not leave it one line down, although IntelliJ does (Task 5). `Automatically apply non-conflicting changes` is off by default and lives in Settings > Merge, not in the window (Task 6). A file resolved from outside shows `Conflict is not valid…`, and opening another repository or quitting asks the merge window (Task 6).
- **Merge and rebase from the popup.** Merge and Smart Merge refuse during a rebase with `Cannot merge during rebase`: IntelliJ lets git decide and git allows it (Task 9). `Process` returns the saved changes as soon as the rebase is not left halfway, also after a failure, because there is no Retry here (Task 9). The `Rebase…` dialog goes through `rebase::Process` and saves the local changes (Task 8).
- **Cherry-pick and revert.** `CHERRY_PICK_HEAD` is not removed before committing after conflicts: git keeps the author and a hook that rejects the commit does not break Abort (Task 10). Cherry-Pick/Revert Selected Changes show the error and the success as a notification, not with IntelliJ's modal dialog (Task 11).
- **Rewriting from the log.** The card with Undo does not close by itself: it lasts until the repository changes (Task 12). Drop does not ask for confirmation because that Undo exists. The message dialog is modal, unlike IntelliJ (Task 12). The reword messages of the interactive rebase live only in memory (Task 14). No drag and drop, no quick search and no diff preview in the interactive rebase dialog (Task 14). After the Commit and Rebase… proposal is spent with a commit, Amend is not restored (Task 13).
- **Three panes of HEAD, staged and local.** The title of the rename follows IntelliJ (new → old). The Stage, Unstage and Revert arrows save at once. No word highlighting, folding, unified view, combined view or chain of files in three panes; with `autocrlf` or filters it goes back to two panes (Task 15).

## Decisions

- Preparation: R1: conflicts::accept receives the side as the user sees it (Yours = left pane, already inverted in a rebase by Sides/Session); Task 6 translates Accept Left→Yours and Accept Right→Theirs without inverting again — it is the only way for Review Focus 2 to be met — cost if wrong: Accept Left in a rebase would take the opposite side; the test in_rebase_sides_and_columns_are_swapped catches it.
- Preparation: R2: Task 5 may modify src/diff/editor.ts to generalize connectScroll and drawDivider — the plan forbids copying them — cost if wrong: one more file in the task's diff.
- Preparation: R3: Task 1 may touch pull.rs and update.rs so that pull and rebase with an editor go through git_editing with editor::Ask — its own test demands it — cost if wrong: none.
- Preparation: R4: Task 7 may touch vcs_commands.rs and lib.rs for merge_commit and merge_message — they are new commands — cost if wrong: none.
- Preparation: R5: in Task 10, the setting goes in the component that currently draws the Git settings (the one that shows Git Settings in the interface) — the plan does not give the path — cost if wrong: none.
- Task 1: GIT_EDITOR is '<exe>' --ramify-editor and the path arrives in argv[2] — without the mark, editor::client_main would take an askpass request for a path, because the task carries both variables — cost if wrong: none visible.
- Task 1: git_editing does not add REBASE_CONFIG; those who launch rebase set it — in pull it would leave \u{1} lines in MERGE_MSG after a conflict — cost if wrong: a rebase that forgets the -c would use '#' as the comment character.
- Task 1: the editor tests with real git live in src-tauri/tests/editor.rs (they need CARGO_BIN_EXE_ramify, like tests/askpass.rs); they are run with --test editor — cost if wrong: none.
- Task 1: the orphan dialog after cancelling the task is a correctness matter (Review Focus 3); it is fixed before the review, with an editor-dismissed event like askpass-dismissed.
- Task 2: the card of the test cannot_complete_update_comes_back_with_abort_even_without_an_entry stays, but only for Merging/Rebasing (the only things Update leaves) and its Abort without a saved entry goes through operationFlow.abortOperation, with IntelliJ's confirmation and the HEAD seen — the global constraints on confirmations and HEAD take precedence over the letter of the test — cost if wrong: a merge or rebase started in the terminal still shows a card that talks about Update.
- Task 3: the conflict type is labeled from the user's side (DU in merge = deleted by you, in rebase = deleted by them); IntelliJ always passes isReversed=true to getConflictType and in a normal merge calls "deleted by them" what git calls "deleted by us" — cost if wrong: a label different from IntelliJ's in deletion conflicts during a merge.
- Task 3: short hashes of 8 characters, like getShortHash and operation.rs — cost: none.
- Task 3: the rebase description without ", revision <hash>", like GitRebaseMergeDialogCustomizer (the text of the brief) — cost: the hash is missing in a rebase already in progress.
- Task 3: a file that is not UTF-8 is treated as Version::Binary — cost: a Latin-1 text in conflict only admits Accept Left/Right.
- Task 3: accept skips without error the paths that are no longer in conflict — cost: the interface must reread the session after accepting (Tasks 6 and 7).
- Task 4: the merge window (Task 6) normalizes CRLF to LF when loading the three versions and writes the result with the separator the working file had, like IntelliJ's Document (always \n, separator detected on saving) — otherwise a CRLF side and an LF side differ on every line — cost if wrong: a file with mixed separators is written with a single one.
- Task 5: Resolve Automatically also applies the non-conflicting changes, because MergeThreesideViewer.applyResolvableConflictedChanges (232, l.858-867) calls resolveChangeAutomatically(change, BASE) for all of them and canResolveChangeAutomatically accepts the non-conflicting ones with isChange(BASE) — parity wins over the name of the plan's test (resolve_automatically_only_touches_conflicts_with_auto_not_edited) — cost if wrong: one line.
- Task 5: the ranges of the blocks follow the text with a port of DiffUtil.updateRangeOnModification (per-line rule) instead of ChangeSet.mapPos — the expectations ported from MergeTest demand it — cost: none visible.
- Task 5: it is fixed even though IntelliJ's DiffUtil did the same — a merge that leaves the text in the wrong order loses work; if IntelliJ does it the same, it is noted as a divergence — cost if wrong: a difference in behavior in an edge case.
- Task 5: IntelliJ has the same fault (DiffUtil.deleteLines/getAffectedLineRange in 232); the fix of Task 5 is a deliberate divergence, commented in MergeEdit.modify — cost if wrong: the center pane behaves differently from IntelliJ only when deleting whole lines from a merge action.
- Task 6: before the review three correctness points are fixed: (a) repo_open destroys the merge window without asking and loses the half-done merge — it must go through the same close request as Cancel; (b) a file that stops being in conflict (external git add) must show the Conflict is not valid… bar; (c) the Automatically apply non-conflicting changes setting is missing in the window (DiffBundle settings.automatically.apply.non.conflicting.changes) — parity.
- Task 6: a new command merge_working_file (hash and dominant separator), a merge label in the capabilities, task events also to the merge window, close_main hides the merge window — accepted, necessary — cost: none.
- Task 6: the Automatically apply non-conflicting changes setting goes in the settings dialog (Merge group) and not in the window — it is where 2023.2.3 puts it (DiffSettingsConfigurable.kt); my earlier indication was wrong — cost: none.
- Task 7: "Pending Unresolved conflicts" is the title of the notification (GitConflictResolver.java:198-201). (confirmed by the reviewer)
- Task 8: the Rebase… dialog goes through rebase::Process in Task 9, same as rebaseOnto — GitRebase.java uses GitRebaseUtils.rebase → GitRebaseProcess, which saves the local changes — pointer for Task 9 — cost if wrong: until Task 9, Rebase… with local changes fails instead of saving them.
- Task 9: Process returns the saved changes as soon as the rebase is not left halfway, also after a failure; IntelliJ leaves them saved with Retry — it follows decision 4 of phase 2 (restore if the repository is clean again) and there is no Retry here — cost if wrong: after a failure the user cannot retry with the changes still saved.
- Task 9: interfaces different from the brief accepted: branch_rebase_remote(remote_branch, name, drop_commits, expected_head); smart_merge/merge_into return BranchMerge { outcome, saved, restore }; published receives Repo (the patterns live in the data folder); rebase_published is a read; branch_pull_into as TaskKind::Read just like update_project — cost: none visible.
- Task 9: Merge and Smart Merge of the popup refuse during a rebase with "Cannot merge during rebase", from IntelliJ's pattern rebase.log.commit.editing.action.prohibit.state.rebasing (B:240, "Cannot {0} during rebase") — IntelliJ lets git decide and git allows it, so it is a divergence that avoids corrupting the rebase — cost if wrong: a text that IntelliJ does not show in the popup.
- Task 10: CHERRY_PICK_HEAD is not removed before committing after conflicts (IntelliJ deletes it to use commit --only) — this way git keeps the author and, if a hook rejects the commit, the resolution and Abort Cherry-pick keep working — cost if wrong: none visible.
- Task 12: before the review, the card with Undo does not close by itself after 10 s; it lasts until a change of the repository invalidates it (HEAD different from new_head), like GitCommitEditingNotifications — Drop does not ask for confirmation because that Undo exists — cost if wrong: one more card that stays on screen.
- Task 12: the protected branch validation of the dialog (GitNewCommitMessageActionDialog.validate:47-65) is ported with its text "Cannot edit selected … was pushed to protected branch '{2}'", inside the dialog and without losing the typed message — parity and not losing what the user types — enters in round 1.
- Task 12: the message dialog stays modal (IntelliJ: isModal = false) — the app's dialogs are modal within their window; non-modal would allow changing the log selection with the dialog open — cost if wrong: the log cannot be looked at while typing the message.
- Task 13: after the proposal is spent with a commit, Amend is not restored (HEAD is already another): only the message draft comes back, and if Amend was active, the draft before Amend (amendRestore.before) — otherwise the next commit would amend the new commit with the old HEAD's message — cost if wrong: the user has to tick Amend again.
- Task 14: the reword messages of the dialog live only in memory (IntelliJ: GitRewordedCommitMessages in the workspace) — if the app is closed with the rebase stopped, git will ask for the message through the free editor and it will have to be rewritten; nothing in the repository is lost — cost: rewriting a message in that case.
- Task 14: no drag and drop, no quick search and no diff preview in the dialog's changes panel — for the list of gaps of Task 16 — cost: three conveniences fewer than IntelliJ.
- Task 15: the title of the rename follows IntelliJ (getTitle(path, origPath): new -> old) even though the name of the plan's test says orig -> new — cost: none visible.
- Task 15: the Stage/Unstage/Revert arrows save at once (IntelliJ edits the document and saves afterwards) — no half-saved edits — cost: an arrow cannot be undone with ⌘Z before saving.
- Task 15: no word highlighting, folding, unified view, combined view or chain of files in three panes; autocrlf/filters go back to two panes — for the list of gaps of Task 16.
- Final review: the app's own texts are in Spanish only where IntelliJ has no text (internal guards and warnings that exist only in the app); every IntelliJ text is in literal English — cost: on error paths the interface mixes two languages.

## Findings of phases 1 and 2 that this phase fixed

They were minor findings that had been deferred. Each has its test, which was written first.

- Phase 1, Task 2 ("conflict uses a direct Command; a tolerant helper is missing in TempRepo"): `TempRepo::conflict` now leaves a merge with a conflict without failing on git's exit code, and checks `MERGE_HEAD` (Task 3).
- Phase 1, Task 8 ("lines_rollback on a file in conflict breaks the markers; the has_unmerged guard is missing"): test `rolling_back_lines_rejects_a_conflicted_file`, with the `has_unmerged` guard in `lines_rollback` (Task 3; the guard has no RED of its own, see below).
- Phase 1, Task 11 ("unresolved ancestor: conflicted does not see it with pathspec d/f"): test `conflict_in_a_child_folder_is_seen_with_a_folder_pathspec` (Task 3).
- Phase 2, Task 9: `rebase_abort` copied against `ORIG_HEAD`, which another command can move. Test `abort_rebase_copies_against_the_rebases_orig_head`, now against `rebase-merge/orig-head` (Task 2).
- Phase 2, Task 11: the `Cannot complete update` card without a saved entry did not come back and its Abort was left without an entry. Test `cannot_complete_update_comes_back_with_abort_even_without_an_entry`, restricted to Merging/Rebasing and with the Abort confirmation (Task 2).
- Phase 2, Task 12: `pull.rebase=interactive` left git waiting for an editor without a terminal. Test `pull_rebase_interactive_does_not_wait_for_a_terminal` (Task 1). `update::abort` did `reset --merge` with any state without `MERGE_HEAD` and said `Pull abort succeeded`: test `abort_without_operation_does_nothing` (Task 2). A `--no-commit` left `MERGE_HEAD` without Abort or warning: test `merge_no_commit_stays_as_a_merge_in_progress` (Task 2).

## Open points left by the execution

Noted by the implementers and reviewers with a warning. Those that ask to be looked at by hand are in the list of `phase-3-validation.md`; those of the final review are done by whoever does it.

- Task 1: todo title in the free dialog: resolved — GitUnstructuredEditor uses "Git Commit Message"/"Continue Rebasing" also for unknown actions (IntelliJ report, GitInteractiveRebaseEditorHandler.java:115-116).
- Task 2: (a) git_menu_enable and the Rebase submenu without an automatic test — for the manual list of Task 16.
- Task 2: (b) resolved: watch.rs::relevant counts all of .git except objects/ and *.lock, so MERGE_HEAD, rebase-merge/ and CHERRY_PICK_HEAD trigger repo-changed.
- Task 2: (c) resolved: LocalCommitsDialog needs no changes; the adaptation is in actions.ts.
- Task 2: (d) real gap: during a rebase newBranch(null, "HEAD", branch) suggests the rebased branch, which already exists; before, it suggested nothing with a detached HEAD (App.svelte:186, 448) — enters in round 1.
- Task 3: the plan's text for Task 6 ("taking reversed into account") would invert twice under R1 — carry R1 explicitly in the submission of Task 6.
- Task 4: Implementer's concern: no size limit like DiffTooBigException — for Task 6 (huge file in the merge window).
- Task 4: confirmed as a real gap: with a Version::Missing side, Sides.blocks is left empty (conflicts.rs:595); IntelliJ uses empty bytes for the missing stage (loadRevisionCatchingErrors) and shows the merge with blocks — enters in round 1 (Missing counts as empty text; Binary still has no blocks).
- Task 5: Implementer's concern: the layout CSS and the padding of the bands are left for Task 6; only tested in jsdom.
- Task 5: navigation rule not compared with PrevNextDifferenceIterableBase (it is not in the copy) — for the final review.
- Task 6: that merge_window_done destroys the window inside its own IPC call without hanging on macOS — manual check (Task 16).
- Task 7: commitMerge (src/conflicts/flow.ts:58) has no caller — pointer for Task 8: the merge started by the app must pass onresolved = commitMerge (except --no-commit).
- Task 7: the Merge Conflicts node takes the files out of their changelist; it cannot be confirmed in IntelliJ's copy (ChangesBrowserConflictsNode is not built in 232) — manual check in Task 16.
- Task 8: For the final review: the MERGE_MSG of a merge with conflicts keeps the "# Conflicts:" block with -F (same as GitMerger).
- Task 9: preserve::restore facing "cannot apply a stash in the middle of a merge" — the implementer checks it in round 1.
- Task 10: resolved: ongoingActions already includes Git.CherryPick.Abort and Git.Revert.Abort (operation.ts:59-60, 84-87).
- Task 10: separator between Cherry-Pick and Checkout in the log menu — manual check in Task 16.
- Task 11: suggested name with several commits: IntelliJ probably proposes Revert: <subject of the first commit> (CHANGE_LISTS of the log), not the active list — cannot be confirmed without vcs-log.
- Task 12: order of commitDetails(hashes) for the squash fill-in, menu order and whether rewrite_menu/rewrite_undo_check show a visible task on every right click — manual check in Task 16.
- Task 13: resolved: start(.., TaskKind::Write, ..) takes RepoLocks (phase 2 task infrastructure).
- Task 13: IntelliJ commits only the changes of the default changelist (GitAutoSquashCommitAction: changeList.changes); here the Commit view's inclusion is not touched — enters in round 1 to check and match.
- Task 14: reopening after a restart with the rebase stopped at edit (detection of Task 2) and the look of the real dialog — manual check in Task 16.
- Task 15: done in the final review: no command holds RepoLocks across flush_diff_window.
- Task 15: For the final review: "Reload and Discard Edits" is the app's own text in Spanish from phase 1 (FilePane.svelte:570) reused in ThreeVersionsPane — review the language of the app's own texts.

## Minor findings not attended

- Task 1: an EditorHandler that wraps Ask and only overrides edit leaves the dialog orphaned (editor.rs:92,121-133) — pointer for Task 12 (Rewriter must delegate edit_request).
- Task 1: race between shown=true and the emission of editor-request (editor.rs:206-207 vs 459-463).
- Task 1: time gap between canonicalize and the write, without O_NOFOLLOW (editor.rs:482 vs 498).
- Task 1: any regular file of the git dir can be edited through the dialog; limit to known editor names (editor.rs:411).
- Task 1: accept() and the length framing copied from askpass.rs:260-273 (editor.rs:381-394).
- Task 1: aria-label="Commit message" fixed for everything too (MessageEditorDialog.svelte:67).
- Task 1: no test of a path with .. that leaves the git dir.
- Task 1: the report says ~480 lines; editor.rs has 764.
- Task 1: a panic inside the serve scope would leave waiting>0 and a cancel would wait for the 5 s grace (editor.rs:463-484).
- Task 2: Rebase failed and Untracked Files Prevent Rebase always offer Abort; IntelliJ only if something was rebased (GitRebaseProcess.java:446-448,461-463); and Retry/Abort act on someone else's rebase that was already in progress.
- Task 2: progress titles Continuing rebase process… / Aborting rebase process… (vcs_commands.rs:2347,2376).
- Task 2: onconflicts by default uses the current generation, not the one captured in resume (operation.ts:4652-4655).
- Task 2: Merging <hash> with a detached HEAD (IntelliJ leaves it empty) and cherry-pick/revert with a detached HEAD give CherryPicking/Reverting (IntelliJ: DETACHED) — note as execution decisions.
- Task 2: Group(id, children) fixes the text "Rebase" in append_entries (commands.rs:626).
- Task 2: the native menu disables instead of hiding (IntelliJ: isEnabledAndVisible=false); Tauri allows removing and inserting.
- Task 2: test name the_git_menu_abort_merge_asks_for_confirmation_and_aborts cancels and does not abort (App.test.ts:2989); assert !ran("rebase -x y") can hardly fail (rebase.rs:1954).
- Task 2: rebase_onto saves a copy before git refuses because of a rebase in progress (rebase.rs:1667-1677).
- Task 2: suggestedBranch looks at operation === "rebasing"; with --rebase-merges stopped at a merge the operation is merging and it suggests the rebased branch again (App.svelte:80) — better to look at the detached HEAD.
- Task 2: abortOperation returns true if the operation is no longer there, and the card says "The update was aborted" without aborting anything (operation.ts:~257-264, flow.ts:266).
- Task 2: confirmable swallows the repo_state error and aborts without confirming (flow.ts:244-256).
- Task 2: with --rebase-merges stopped at a merge, the widget and Abort Merge offer merge --abort — it follows IntelliJ's precedence (MERGE_HEAD first), but it does not cancel the rebase.
- Task 3: microsecond TOCTOU between the second unchanged() and the rename in resolve; no test of a change during the copy (conflicts.rs:937-945).
- Task 3: picked_of uses git show without -c log.showSignature=false; with signatures, the gpg lines end up in the description's author (conflicts.rs:613-614).
- Task 3: the stage mode is ignored: a symbolic link in conflict is shown/resolved as a normal file, a gitlink gives Missing (conflicts.rs:458-478,759-773,940-945).
- Task 3: a non-UTF-8 path in the index brings down the whole session (conflicts.rs:468).
- Task 3: stages repeats the ls-files splitting of commit::records/parse_entry (conflicts.rs:459-478).
- Task 3: NOT_IN_CONFLICT and OUTDATED in English against the module's errors in Spanish (conflicts.rs:375,386).
- Task 3: no --sparse in add/rm for conflicts outside the cone (GitMergeUtil.java:385-388).
- Task 3: path_with_leading_dash_is_not_an_option does not test resolve; TempRepo::conflict does not check that the file is in conflict (test_support.rs:1552-1568).
- Task 3: the lines_rollback guard has no RED; it adds one more ls-files -u (commands.rs:386-390, 2812-2826).
- Task 4: merge3.rs 2,422 lines; the word engine (~678-1572) could go to merge3/words.rs.
- Task 4: ignoring_whitespace_merges_blocks checks something other than its name (the plan's name): Ignore turns a conflict into a Modified without conflict; with the same alignment it never gives fewer blocks.
- Task 4: parse_line_matching of the tests loses the * mark (merge3.rs:1865-1875).
- Task 4: Whitespace::Trim without a test through blocks(); fixed tests of empty texts, without a final newline and CRLF are missing.
- Task 4: is_continuous_script approximate (is_numeric accepts No; fixed BMP tables) (merge3.rs:712).
- Task 4: automatic resolution depends on time: after the 1 s budget, the following blocks lose auto even if they are trivial (merge3.rs ~414, ~463).
- Task 4: the differences > threshold branch after the diff has no test (merge3.rs:340).
- Task 4: similar 3.2.1 uses a Myers with a bounded heuristic that may not be minimal, even without a deadline (add to decision 4).
- Task 4: smoke test with a 60 s cap (blocks_of_huge_files_finish_in_time) and expired() subtracts from Instant::now().
- Task 5: pasting CRLF in the center pane leaves \r in result() (editors.ts:338) — pointer for Task 6: strip \r on saving (R6).
- Task 5: global wordCache survives destroy() with file contents (editors.ts:148-162).
- Task 5: a divider band that reaches the end of the document is drawn one short line (editor.ts:115, 1038-1058), inherited from the two-pane diff.
- Task 5: deleting the whole document (base "a", left "L", right "") and applying right and then left leaves "L\n" instead of "L" — "" is an empty line in the document and zero lines in the block (model.ts:259-275).
- Task 6: with Settings open, changing the whitespace from the merge window and pressing OK in Settings writes the old value.
- Task 6: if merge_window_done fails, busy stays true: dead buttons and Quit/repo_open rejected until the exit is forced (MergeWindow.svelte:114-117,173-177,385).
- Task 6: two exit requests at once cross each other; an exit during an Apply is rejected with a misleading text (MergeWindow.svelte:225-231).
- Task 6: opening_another_repository_with_a_half_done_merge_asks looks for the order of lines in the source itself with include_str! (commands.rs:~3938-3946).
- Task 6: the "same hash without a red bar" half of a test checks nothing (MergeWindow.test.ts:290-291).
- Task 6: changeWhitespace does not set busy while waiting for mergeBlocks (MergeWindow.svelte:186-209).
- Task 6: repo_open destroys the window without emitting merge-done — pointer for Task 7.
- Task 6: no test that check() is skipped while busy.
- Task 6: a duplicated Received makes wait_for_merge answer "half-done merge that was wanted to be kept" (commands.rs:123-131); fails on the safe side.
- Task 6: must_ask_merge(true, ready) with a fixed true and a test that repeats exists && ready (commands.rs:142).
- Task 7: a merge-done from another file does not reread the session (flow.ts:206-211,233).
- Task 7: merge_commit forces -c i18n.commitEncoding=UTF-8 and merge_message decodes as UTF-8; IntelliJ uses the repository's encoding (commit.rs:1111,1134).
- Task 7: merge_commit without an expected HEAD; the rejection with unresolved files depends on git's error and has no test.
- Task 7: the separator after Accept Yours is missing in the Staging menu (Git.Stage.Tree.Menu, plugin.xml:384-388) (CommitView.svelte:948-960).
- Task 7: after conflicts_session fails, conflictKey is not retried (CommitView.svelte:383).
- Task 7: proposedMerge is noted even though amend prevented filling (CommitView.svelte:401-405).
- Task 7: resolveRebase(false) onnothing shows the rebase card; IntelliJ shows showFatalError with git's output (GitRebaseProcess.java:291-299).
- Task 7: no quick search in the dialog's table (MultipleFileMergeDialog.kt:120) nor conflict type per row in Staging.
- Task 7: if the rebase stops again while there is a Conflicts opened by the user, stopped() joins that dialog and its onresolved is lost (flow.ts:167).
- Task 7: a rejection of dialogPhase would be left unhandled in the derived promise (flow.ts:177); ask does not reject today.
- Task 8: rebase flags in a fixed order, not the user's (plan: RebaseParams with booleans) (rebase.rs:322-343).
- Task 8: repeated keys in {#each refs} if a tag is named like a branch (RebaseDialog.svelte:~175, flow.ts:~387).
- Task 8: squash without conflicts: IntelliJ probably shows All files are up to date (HEAD does not move); here N files updated (flow.ts:213-231).
- Task 8: .catch(() => {}) when saving settings (flow.ts:320,395).
- Task 8: with an unknown git version, TS and Rust choose --preserve-merges/--rebase-merges differently (options.ts:101).
- Task 8: duplicated head/other binding in a test (merge.rs:498-499); a test of -m that starts with - is missing.
- Task 8: an old SQUASH_MSG would let merge_commit commit someone else's changes if the user aborts outside with the dialog open; harden by passing squash: bool or requiring MERGE_HEAD except for squash.
- Task 8: summarize does not check the repository's generation after its awaits (src/git/flow.ts).
- Task 8: squash resolved without changes → git commit fails with nothing to commit and comes out as an error.
- Task 9: with two stashes each has its Restore Changes and they can be returned in the opposite order (git refuses to overwrite; flow.ts:257-265).
- Task 9: after switching repository saved/restore are discarded without a warning (actions.ts:368-374, merging.ts:242,346); the journal recovers them when reopening.
- Task 9: the published commits question goes before isRebaseAllowed (merging.ts:363,415; git/flow.ts:579).
- Task 9: the Overwrite existing branch checkbox is ignored in askName (merging.ts:368-379).
- Task 9: the "always dirty" half of rebase_retries_once_if_git_sees_a_dirty_tree does not check the second stash; date +%N on macOS (rebase.rs:1459-1472).
- Task 9: it is not checked that the Merged x to y card closes after Delete x (IntelliJ: notification.expire()).
- Task 9: preserve::UNMERGED_FIRST repeats update::FINISH_FIRST (preserve.rs:246, update.rs:28).
- Task 9: after returning through repo-changed, the stopped rebase's card still has Continue/Abort and gives Cannot continue rebase.
- Task 9: preserve::restore is only saved for unresolved paths, not for a stopped rebase with a clean index (only restore_requested looks at stopped_midway).
- Task 9: a stopped git am (rebase-apply) also gives "Cannot merge during rebase".
- Task 10: the description of Conflicts during a cherry-pick from the log does not use apply.conflict.dialog.description.label.text, and the title Cherry-pick was performed with conflicts is not passed as options.title.
- Task 10: invented Retry button in commitConflicted (IntelliJ uses its commit dialog) — note as a divergence.
- Task 10: the author test after conflicts does not check the date (%at).
- Task 10: Cherry-Pick is not disabled while that commit is in progress (VcsCherryPickAction.java:67-68); empty root until repositoryChanged.
- Task 10: expectedHead is read when the flow starts; commit_after_conflicts without an expected HEAD (CHERRY_PICK_HEAD == hash replaces it).
- Task 10: is_published and message_of use git::run instead of repo.ctx.git (apply_commits.rs:130-139,279,289).
- Task 10: REPOSITORY_CHANGED and "No … in progress" as GitError::Io (apply_commits.rs:313,316).
- Task 10: merge_message now depends on the settings and fails if they are not read.
- Task 10: detection of local changes broader than IntelliJ's ("would be overwritten by") (apply_commits.rs:209).
- Task 11: error as a notification instead of IntelliJ's modal dialog.
- Task 11: Cmd-click leaves current on a deselected file; Shift-click does not move current (ChangesPanel.svelte click()); indentation of the folder's <p>.
- Task 11: apply_paths is only used in tests.
- Task 11: new files (??) are also sent to changelistsMove.
- Task 11: when the guard denies the --3way, the user only sees the error of the first attempt, with no explanation of why it was not retried.
- Task 12: two reads of HEAD (expect_head and planned); compare plan.head with expected_head (rewrite.rs:545,578,605).
- Task 12: first_changed uses merge-base(old,new) instead of the parent of the oldest rewritten commit (rewrite.rs:662-674).
- Task 12: the todo round-trip test does not fix break, exec make test or p a1b2 x (todo.rs:668-716).
- Task 12: with rebase.abbreviateCommands=true, Reword fails with an internal text; pass -c rebase.abbreviateCommands=false (rewrite.rs:519-536). Fixed in the final review (editor::REBASE_CONFIG).
- Task 12: quick counts hashes before removing duplicates (rewrite.rs:316-358).
- Task 13: menu_checks repeats the full check twice per right click (rewrite.rs:867).
- Task 13: title and body "Undo commit failed" repeated (rewrite.rs:811).
- Task 13: the proposal is silently discarded if committing or if the Commit view is not mounted (CommitView.svelte:647, App.svelte:460).
- Task 13: moveToChangelist with an empty list (editing.ts:130).
- Task 13: autosquash_does_not_accept_a_base_that_starts_with_a_dash only checks is_err (rewrite.rs:1507); no test of abandonment on generation change with the selector open.
- Task 13: dropStaleProposal because HEAD moved from outside restores Amend with the old HEAD's message — the same risk as the Amend ruling after a commit. Fixed in the final review: with HEAD moved, Amend does not come back.
- Task 13: with a half-done merge, Fixup… and then Commit restores the already concluded merge message (CommitView.svelte:268,294-297).
- Task 13: emptying the box abandons the proposal and brings back the old draft (CommitView.svelte:434-438).
- Task 13: a change of the repository during the commit warnings dialog is not seen by checkProposal (CommitView.svelte:320-326,408); Rust stops it.
- Task 14: the log mode comparison accepts a git todo longer than the generated one and the extra lines would be overwritten by the plan (rewrite.rs diff 1218): require == in the length. Fixed in the final review.
- Task 14: the plan's subjects are written as they are in the todo; take them from the lines generated by hash (rewrite.rs diff 1008-1042, 1190). Fixed in the final review, also in native mode, and a row with a line break is rejected.
- Task 14: model.moveDown without checking the limit (model.ts 5006-5007).
- Task 14: forget does not run after rebase::rebase interactive nor after autosquash; abort does forget even if --abort fails (rebase.rs:505-507,531-533).
- Task 14: menu_checks repeats single_plan (rewrite.rs diff 822).
- Task 14: test gaps of the model (canSimpleAction with a todo of the same type, drop of a fixup child, unite of another group, exchangeIndices of a child); misleading name of cancelling_the_dialog_does_not_launch_git in Rust.
- Task 14: INVALID_TODO duplicated and Retry by exact text equality (rewrite.rs diff 849, editing.ts 3074,3137).
- Task 14: InteractiveRebaseDialog.svelte of 689 lines.
- Task 15: with the panel dirty and !fitsAll, load() does not warn; the next save fails with a generic warning (ThreeVersionsPane.svelte:686-690).
- Task 15: discard() throws away the text of both panes even though only one had a conflict (ThreeVersionsPane.svelte:790).
- Task 15: the return to two panes chooses staged/local by file.unstaged and not by the node, like GitStageDiffUtil.kt:58-66 (DiffWindow.svelte:253-256).
- Task 15: the staged blob is not checked as a fixed point of the clean filter (worktree_diff.rs:369-389).
- Task 15: the index_write lock test does not test the lock (vcs_commands.rs:1744); no test of 120000/160000 or an unresolved entry (worktree_diff.rs:1138).
- Task 15: ThreeVersionsPane.svelte of 955 lines with ~550 of model in <script module>; repeated merge CSS.
- Task 15: head_path does diff-index --cached -M of the whole repository on every load and write (worktree_diff.rs:322).
- Task 15: fallBack depends on saveAll succeeding; in combined view another panel that fails leaves a clean panel on three old versions (DiffWindow.svelte:254-259); no tests of flush during "gone" nor of removing the warning when the three versions come back.
