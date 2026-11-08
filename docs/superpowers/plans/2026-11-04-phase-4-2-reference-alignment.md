# Phase 4, part 2: alignment with IntelliJ 2024.1. Implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** what phases 1 to 3 built against IntelliJ 2023.2 behaves as IntelliJ 2024.1, before the user validates it on real repositories. This covers:
- the copied texts that changed;
- the new confirmations and options;
- the branches popup search;
- the merge and rebase toolbar widget;
- soft wraps in the diff.

**Architecture:** small, independent changes to existing flows. Each task touches one area and carries its own tests. The two soft-wrap tasks are the only large ones:
- Task 11 adds the option, the toggles and plain CodeMirror wrapping;
- Task 12 replaces the line-count spacers of the aligned diff with measured pixel heights.

**Tech Stack:** Tauri 2.12, Rust 1.99.1, Svelte 5, Vitest 5, CodeMirror 6. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-10-31-intellij-git-client-design.md`, and the phase 4 overview `docs/superpowers/plans/2026-11-03-phase-4-overview.md`: its Global Constraints and Decisions are part of this plan.
- Starting point: `docs/reports/phase-4-reference-change.md` sections 2 to 4 and 6.
- Research (not in the repository): `.superpowers/phase-4/research-part-2.md`, items 1 to 20, and `.superpowers/phase-4/research-soft-wraps.md`. Each task names the items it implements. The implementer reads those items first: they hold the exact texts, the `241:` citations and the current rama lines.

**Reference:** tag `idea/241.14494.240`. Paths are relative to `intellij-community`:
- `G/` = `plugins/git4idea/src/git4idea/`;
- `D/` = `platform/diff-impl/src/com/intellij/diff/`.

Bundles: `GB:` = GitBundle, `VB:` = VcsBundle, `LB:` = VcsLogBundle, `AB:` = ActionsBundle. Whoever implements copies the literal texts from the cited file or bundle line, not from this plan.

**Branch:** `phase-4`, after part 1 (`8a99d59`).

**Commands** (the user asked to keep the machine load low; suites never run at the same time):
- TS: `PATH=$HOME/.cache/rama-toolchain/node-v22.23.4-darwin-arm64/bin:$PATH pnpm exec vitest run --maxWorkers=2 <files>` and `pnpm exec tsc --noEmit -p .`
- Rust: `PATH=$HOME/.cargo/bin:$PATH CARGO_BUILD_JOBS=4 cargo test --manifest-path src-tauri/Cargo.toml --lib <filter> -- --test-threads=4`
- Python: `python3 -I -m unittest discover -s scripts`

## Decisions of this part

They add to those of the overview. The user approves them with this plan. Each has its cost if it is wrong.

1. **`Rebasing Published Commit` asks before saving the changes.** 241 saves the local changes first, then asks, and Cancel leaves them in the stash with `Local changes were not restored`. rama asks before saving, as it already does for `Rebasing Merge Commits`, so Cancel leaves the working tree untouched. The texts, buttons and default button are 241's. `Pull…` is not affected, as in 241, where `Pull…` runs `git pull` directly. Cost: the order differs from IntelliJ, and the user sees no "not restored" card after Cancel.
2. **The unstash conflict card follows 241's text and drops rama's stash message line.** It gets the `Resolve conflicts…` action now. `View saved changes…` comes in part 3, with the Stash tab (overview decision 10), on this card and on `Local changes were not restored`. Cost: until part 3 the card does not name the stash; `git stash list` shows it.
3. **Protected branch patterns are validated while typing, and the notification on save goes away.** The error text is 241's (`GB:700`). Its cause comes from the Rust `regex` crate, which is the dialect rama already matches with, so it is not Java's text. Save is not blocked, as far as 241 can be read. Cost: the cause reads differently from IntelliJ's.
4. **`INTELLIJ_GIT_EXECUTABLE=true` is set for every git command,** as 241 does. Its only readers are the user's own hooks. A hook written to behave differently under IntelliJ will behave the same way under rama, which is what someone replacing IntelliJ wants. Cost: a hook that identifies "IntelliJ" by this variable takes rama for IntelliJ.
5. **Fetch is disabled while any fetch of the app runs:** an explicit Fetch, an Update or a Pull. 241 disables it only while a fetch request is open, and rama's Update does its fetch inside one Rust command. rama keeps Fetch disabled for the whole Update. There is no description text, because the native menu cannot show it. Cost: during the merge or rebase phase of an Update, Fetch stays disabled for a few more seconds.
6. **`Add to VCS` runs `git add --ignore-errors -A -- <paths>`, as 241 does.** The files land in the active changelist, and in the dialog they are included in the commit. The commit pipeline must commit another changelist without the added file. Task 9 pins that with a test. Cost: if the pipeline mishandles it, the task fails review instead of shipping.
7. **The merge and rebase widget is a row of icon buttons after the branch widget** in rama's main toolbar, with 241's tooltips and colours. rama's toolbar copies the New UI, which is where 241 has the widget. Icons are drawn for rama and listed in `NOTICE` if they come from IntelliJ's jar. Cost: none expected.
8. **The branches popup takes the New UI minimum size, 375×300.** rama copies the New UI. The classic UI's 300×200 is not used. Cost: none.
9. **Soft wrap is stored per diff place, as in 241:** `Default` (the diff window), `CommitDialog` (the dialog's `Diff` panel), `ChangesView` (the Local Changes preview) and `Merge` (the merge window). `Align Changes Highlighting` stays one shared value, as in 241. The toggle is `Soft-Wrap`:
   - in the `Settings` gear of the diff window, before `Align Changes Highlighting`;
   - in the right-click menu of the line-number gutter of every diff and merge pane, as in 241's gutter popup.

   Defaults:
   - `Default` is on, because the user's 2024.1 has it on;
   - the other places are off, which is IntelliJ's default, because the user's values for them were not read.

   Cost: if the user had wraps on in the other places, they turn them on once per place.
10. **Wrapping details that rama does not copy:**
    - no sticky line number when the top of the view is inside a wrapped line;
    - no forced wrap of lines over 100000 characters;
    - wrapped rows start at column 0, as CodeMirror does.

    Both line-number gutters of an aligned diff get the same width, so equal lines wrap at the same width. Cost: small visual differences on long lines.
11. **Rows that stay out of this part, with their reason in the matrix:**
    - `Git.Stage.Configure` and `Git.Stage.ToggleCommitAll`: non-modal staging only, not this user's mode;
    - `Git.Stage.Stash.Files`: needs the stash of part 3;
    - `Vcs.Log.OpenAnotherTabInEditor`, `Vcs.Diff.EnableDisableCombinedDiff` and `Vcs.Diff.ToggleSearchInChanges`: rama has no editor tabs and no search in the diff.

    Cost: those rows stay pending or not applicable.

## Review Focus

1. **The user clicks `Rebase Anyway` or `Cancel` during an Update while Devin edits files.** Nothing has been saved yet. Cancel changes nothing, and `Rebase Anyway` goes through the normal save and restore path. Test in Task 5.
2. **A branch and a tag with the same name, merged from the branches popup.** The popup merges the item that was clicked: `refs/heads/x` for the branch and `refs/tags/x` for the tag. Test in Task 6.
3. **Typing in the branches popup a word that matches an action and a branch,** for example `push` with a branch `push-fix`. The action row comes first and is selected, Enter runs it, and the arrows go through actions and branches. With the gear toggle off, the popup behaves as today. Test in Task 7.
4. **`Add to VCS` on a new file, then a commit of another changelist.** The new file stays added in the index and is not in the commit. Test in Task 9.
5. **Soft wrap on, aligned diff, a long unchanged line that wraps only on one side.** The rows after it stay level. The spacers converge in one pass, and feeding back their own heights changes nothing. Pure-function test in Task 12. The real layout goes to the manual checklist.

---

### Task 1: Copied texts and documentation rows

Research items 1, 14, 15, 16 and 19.

**Files:**
- Modify:
  - `src/log/editing.ts`;
  - `src-tauri/src/rebase.rs`, `src-tauri/src/update.rs`, `src-tauri/src/git.rs`, `src-tauri/src/vcs_commands.rs`;
  - `src/branches/actions.ts`, `src/branches/merging.ts`;
  - `src/log/LogActions.svelte`, `src/log/FilterBar.svelte`;
  - `docs/superpowers/plans/2026-11-01-phase-2-log-branches-and-remotes.md` (decision 3 wording), `docs/reports/phase-2-commands.md` (row 9), the `settings.rs` comment on the credential helper.
- Test: the existing tests that assert the old texts, listed per row in research item 1 (`src/log/editing.test.ts`, `src/branches/actions.test.ts`, `src/branches/merging.test.ts`, `src/log/LogPanel.test.ts`, `src/log/FilterBar.test.ts`, the Rust tests of each constant).

Not in this task:
- the branches popup texts, which go to Task 7;
- `Push failed to start`, which goes to Task 6;
- the drop success body, which goes to Task 2.

**Interfaces:**
- Produces: nothing new. The unstash conflict card (`restoredWithConflicts` in `src/branches/actions.ts`) gets the action `Resolve conflicts…`, which calls the existing `ui.resolveConflicts`.

- [ ] **Step 1: Change the assertions first.** Every test that asserts one of the old texts in research item 1 asserts the 241 text. Examples:
  - `Could not Drop Commits`;
  - `Dropped 1 commit` and `Dropped 3 commits`;
  - `Cannot undo drop`;
  - `Could not save uncommitted changes`, with no period;
  - `Cannot identify version of Git executable {0}`;
  - `View and reset filters`.

  Add:
  - `unmerged_files_text_and_action_follow_241`: the card says `You need to resolve all merge conflicts before merge.` and the second 241 line, separated by `\n`, and its action is `Resolve conflicts…`;
  - `unstash_with_conflicts_says_241s_text_and_offers_resolve`: the body is exactly the 241 message with `\n` for `<br/>` (decision 2), and the only action is `Resolve conflicts…`;
  - `the_text_filter_shows_text_or_hash`: the input has `placeholder="Text or hash"`, and its accessible name and title are `Filter by Commit Message or Hashes`.
- [ ] **Step 2: Check that they fail.** Run the TS and Rust commands on those files. Expected: FAIL on the old texts.
- [ ] **Step 3: Change the texts** as research item 1 lists them, row by row.
- [ ] **Step 4: Change the documents.** Rewrite the "unlike IntelliJ" wording of the credential helper as research item 19 lists it: IntelliJ's own default is off, and the user's IntelliJ has it on. In `docs/parity-matrix.md`:
  - `HTTP login dialog` and `Vcs.Diff.IncludeWholeChangedLinesIntoCommit` are `done` with the test or reason of research items 14 and 15;
  - the `LocalChangesView.ShowOnDoubleClick.*` rows leave the table if part 1 left them (research item 16);
  - `merge.operation.merged.to` is done, because rama's notifications are plain text.
- [ ] **Step 5: Check that they pass.** TS, tsc and Rust on the touched files. Expected: PASS. Then `git grep -n "Couldn't" src src-tauri/src` returns only texts that 241 still has.
- [ ] **Step 6: Commit.** `git commit -m "fix: copied texts follow IntelliJ 2024.1"`

### Task 2: Drop Commits asks for confirmation

Research item 2. Reference: `G/rebase/log/drop/GitDropLogAction.kt:24-71`, `G/rebase/log/GitLogCommitEditingConfigurable.kt`, `G/config/GitVcsApplicationSettings.java:29`.

**Files:**
- Modify:
  - `src-tauri/src/settings.rs`: `show_drop_commit_dialog: bool`, default `true`, camelCase `showDropCommitDialog`;
  - `src/lib/api.ts` (`Settings`);
  - `src/log/editing.ts`;
  - `src/branches/BranchDialogs.svelte`;
  - `src/settings/SettingsView.svelte`.
- Create: `src/log/DropConfirmDialog.svelte`, or a `doNotAsk` option on `src/operation/ConfirmDialog.svelte`, whichever keeps `ConfirmDialog`'s existing callers unchanged.
- Test: `src/log/editing.test.ts`, `src/settings/SettingsView.test.ts`, `src-tauri/src/settings.rs` tests.

**Interfaces:**
- Produces:
  - `EditingUi.confirmDrop(count: number): Promise<{ ok: boolean; doNotAsk: boolean }>`;
  - `succeeded()` and `finish()` in `editing.ts` take an optional plain-text `body`.

- [ ] **Step 1: Write the failing tests:**
  - `drop_asks_drop_commits_with_ok_and_cancel`: dropping 3 commits shows the warning with:
    - title `Drop Commits`;
    - message `Do you want to drop 3 commits?`;
    - buttons `OK` and `Cancel`, with OK as the default;
    - the `Don't ask again` checkbox, unchecked.

    Cancel runs nothing. This replaces `drop_without_confirmation`.
  - `drop_asks_after_the_check_and_before_reading_the_state`: `api.check` runs first and `api.state()` only after OK.
  - `dont_ask_again_is_saved_only_on_ok`: checked plus OK saves `showDropCommitDialog: false` and drops. Checked plus Cancel saves nothing.
  - `without_the_setting_drop_does_not_ask`.
  - `drop_success_lists_the_subjects`:
    - 12 commits give 10 quoted subjects, one per line, then a line `...`;
    - a 45-character subject becomes its first 37 characters plus `...`;
    - a subject of exactly 40 characters stays whole.
  - SettingsView: a `Confirmation` section shows `Ask for confirmation to drop commits`, checked by default, with the comment `Applicable to Git`. OK saves its change.
  - Rust: `show_drop_commit_dialog_defaults_to_true`, also when the field is missing from an older `settings.json`.
- [ ] **Step 2: Check that they fail.** Expected: FAIL.
- [ ] **Step 3: Implement.**
  - The question goes between `api.check` and `api.state()`. Split `prepare()` if it fuses them.
  - The dialog uses the warning icon.
  - Use the texts of `GB:250-251`, `GB:719-720` and `IdeCoreBundle` `dialog.options.do.not.ask`.
  - The `Confirmation` section goes after the Git section of `SettingsView`.
- [ ] **Step 4: Check that they pass.** Expected: PASS.
- [ ] **Step 5: Commit.** `git commit -m "feat: Drop Commits asks for confirmation as in IntelliJ 2024.1"`

### Task 3: `--allow-unrelated-histories` in Merge and `--update-refs` in Rebase

Research items 3 and 4. Reference: `G/merge/GitMergeOption.kt:17,26`, `G/rebase/GitRebaseOption.kt:19,34-38`, `G/rebase/GitRebaseDialog.kt:468,482`.

**Files:**
- Modify: `src/git/options.ts`, `src/lib/api.ts` (`MergeOption`, `RebaseFlag`, `RebaseParams`), `src/git/RebaseDialog.svelte`, `src/git/flow.ts`, `src-tauri/src/merge.rs`, `src-tauri/src/rebase.rs`, `src-tauri/src/settings.rs` (`REBASE_FLAGS`).
- Test: `src/git/options.test.ts`, `src/git/dialogs.test.ts`, `src-tauri/src/merge.rs`, `src-tauri/src/rebase.rs`, `src-tauri/src/settings.rs` tests.

**Interfaces:**
- Produces:
  - `MergeOption` gains `"allowUnrelatedHistories"`;
  - `RebaseFlag` gains `"updateRefs"`;
  - Rust `RebaseParams.update_refs: bool`.

- [ ] **Step 1: Write the failing tests:**
  - the option-order tests list `--allow-unrelated-histories` last in Merge, with `Allow merging histories that do not share a common ancestor.`, and `Auto-update branches pointing to rebased commits` last in Rebase, after `Edit commits before rebasing`, with no separator of its own;
  - neither option disables any other one;
  - `merge_with_each_option_launches_the_intellij_command` covers the new flag;
  - `rebase_with_update_refs_passes_the_flag`: `git rebase --update-refs <upstream>`;
  - `update_refs_is_remembered_per_repository`: the flag survives `save_rebase`.
- [ ] **Step 2: Check that they fail.** Expected: FAIL.
- [ ] **Step 3: Implement.** There is no git version check, as in 241.
- [ ] **Step 4: Check that they pass.** Expected: PASS.
- [ ] **Step 5: Commit.** `git commit -m "feat: allow unrelated histories in Merge and update refs in Rebase"`

### Task 4: `Cancel Reword`, and message history in the Edit Commit Message dialog

Research items 5 and 6. Reference: `G/rebase/interactive/dialog/GitRebaseCommitsTableView.kt:110-130`, `G/rebase/log/GitNewCommitMessageActionDialog.kt:68-85,121-131`.

**Files:**
- Modify: `src/rebase/InteractiveRebaseDialog.svelte`, `src/log/CommitMessageDialog.svelte`, `src/commit/CommitDialog.svelte` (only to move its history menu into a shared helper).
- Create: `src/commit/messageHistory.ts` with `historyMenu(messages: string[], choose: (message: string) => void): MenuItem[]`. With no messages it returns one disabled `History is empty` item.
- Test: `src/rebase/InteractiveRebaseDialog.test.ts`, `src/log/editing.test.ts` (the `CommitMessageDialog` block), `src/commit/CommitDialog.test.ts`.

**Interfaces:**
- Consumes: `messagesList()` and `messagesPush(message)` from `src/lib/api.ts` (part 1).
- Produces: `historyMenu`, used by both dialogs.

- [ ] **Step 1: Write the failing tests:**
  - `escape_with_a_changed_reword_asks_cancel_reword`: the confirmation has title `Cancel Reword`, message `Entered commit message will be lost`, and the buttons `Cancel Reword` and `Keep Editing`.
    - `Cancel Reword` restores the old message.
    - `Keep Editing` keeps the typed text in the editor, with the focus in it.
    - Esc inside the confirmation keeps editing.
  - `escape_with_an_unchanged_reword_closes_silently`.
  - `reset_with_a_changed_reword_asks_first`.
  - `the_reword_dialog_has_the_history_button`:
    - `Commit Message History` with ⌃M lists the history, newest first;
    - choosing an entry replaces the message;
    - with an empty history the button is disabled.
  - `a_changed_message_goes_to_history_on_any_close`:
    - OK, Cancel and Esc each call `messagesPush` with the message, trimmed at the end;
    - an unchanged message calls nothing;
    - an OK that validation refuses calls nothing.
  - `CommitDialog`'s existing history tests still pass through `historyMenu`.
- [ ] **Step 2: Check that they fail.** Expected: FAIL.
- [ ] **Step 3: Implement.** The history push runs before `onanswer`, and its failure is ignored, as in the commit dialog.
- [ ] **Step 4: Check that they pass.** Expected: PASS.
- [ ] **Step 5: Commit.** `git commit -m "feat: Cancel Reword and message history in the Edit Commit Message dialog"`

### Task 5: `Rebasing Published Commit` in Update

Research item 7, decision 1. Reference: `G/update/GitRebaseUpdater.java:60-83`, `G/rebase/GitRebaseProcess.java:515-544`.

**Files:**
- Modify:
  - `src-tauri/src/update.rs`: `Outcome::RebasingPublishedCommits`, `UpdateOptions.published_confirmed`;
  - `src-tauri/src/push.rs`: the auto-update after a rejected push treats the new outcome as "did not finish";
  - `src/lib/api.ts`;
  - `src/update/flow.ts`: a new `UpdateRequest { kind: "publishedCommits" }` that reuses `src/branches/PublishedCommitsDialog.svelte` through `BranchDialogs.ask`;
  - `src/push/model.ts`, if it switches on the outcome.
- Test: `src-tauri/src/update.rs`, `src-tauri/src/push.rs`, `src/update/flow.test.ts`.

**Interfaces:**
- Consumes: `rebase::published(repo, upstream, branch)` (`src-tauri/src/rebase.rs`). An `Err` counts as published, as in 241.
- Produces: `Outcome::RebasingPublishedCommits { branch: String, protected: String }`, which serialises as the other outcomes do.

- [ ] **Step 1: Write the failing tests:**
  - `update_with_rebase_over_a_published_commit_asks_before_saving` (Rust):
    - the setup is a local commit already in `origin/main`, `main` protected, and a dirty file;
    - the outcome is `RebasingPublishedCommits`;
    - no stash, shelf or snapshot was created;
    - the dirty file is untouched, and HEAD has not moved.
  - `confirmed_update_rebases_as_before`: with `published_confirmed: true`, the update runs as today.
  - `merge_updates_and_pull_do_not_ask`: the merge method and `Pull…` with rebase never return the outcome.
  - `an_update_that_does_not_end_well_does_not_push_again` lists the new outcome.
  - TS:
    - the outcome opens `Rebasing Published Commit` with `Rebase Anyway` and `Cancel`, Cancel being the default;
    - `Rebase Anyway` repeats with `published_confirmed: true`;
    - Cancel shows `Update canceled`;
    - Update Selected behaves the same.
- [ ] **Step 2: Check that they fail.** Expected: FAIL.
- [ ] **Step 3: Implement.** The check sits next to `rebases_resolved_merges`, before the save, and only when `rebase && !options.published_confirmed`. `Pull…` passes `published_confirmed: true`.
- [ ] **Step 4: Check that they pass.** Expected: PASS.
- [ ] **Step 5: Commit.** `git commit -m "feat: Update with rebase asks before rebasing published commits"`

### Task 6: `git merge` with the full ref, and `Push All up to Here…` without a local branch

Research items 8 and 9. Reference: `G/branch/GitMergeOperation.java:84,228`, `G/actions/GitPushUpToCommitAction.kt:26-61`, `G/push/GitPushSupport.java:79-103`.

**Files:**
- Modify: `src/branches/merging.ts`, `src/lib/api.ts` (`merge` takes the ref kind), `src-tauri/src/vcs_commands.rs` (`branch_merge`), `src-tauri/src/merge.rs`, `src/log/contextMenu.ts`, `src/push/model.ts`, `docs/reports/phase-3-commands.md` (row 1).
- Test: `src-tauri/src/merge.rs`, `src/branches/merging.test.ts`, `src/log/contextMenu.test.ts`, `src/log/LogPanel.test.ts`, `src/push/model.test.ts`.

**Interfaces:**
- Produces: `api.merge(branch: string, kind: "local" | "remote" | "tag", smart: boolean, head: string)`, where `kind` is the popup item's existing ref kind (`src/lib/api.ts`). The Rust command validates `kind` against `local | remote | tag` and builds:
  - `refs/heads/<name>` for a local branch;
  - `refs/remotes/<name>` for a remote branch, whose name already includes the remote;
  - `refs/tags/<name>` for a tag.

  Every title and notification keeps the short name. The `Merge…` dialog keeps the short name too.

- [ ] **Step 1: Write the failing tests:**
  - `merge_into_is_merge_without_arguments` expects `merge refs/heads/side`.
  - `popup_merge_of_a_tag_with_a_branchs_name_merges_the_tag`: a branch `x` and a tag `x` on different commits; merging the tag from the popup gives `git merge refs/tags/x` and the tag's commit.
  - `an_unknown_ref_kind_is_rejected`.
  - `push_all_up_to_here_is_enabled_on_detached_head`: it replaces `push_all_up_to_here_only_on_the_current_branch` and is enabled with exactly one commit.
  - `push_all_up_to_here_outside_local_branches_opens_the_push_dialog`: it replaces `push_all_up_to_here_without_a_local_branch_is_not_possible`.
    - The source is the short hash.
    - With a current branch, the target is `<remote>/<current>`.
    - On detached HEAD, the target is a new `detached` branch.
- [ ] **Step 2: Check that they fail.** Expected: FAIL.
- [ ] **Step 3: Implement.** Read `UniqueNameGenerator.generateUniqueName` in 241 (fetch it with `fetch_ic.py` if it is missing), and make rama's `unique_detached` (`src-tauri/src/push.rs`) number the same way. Remove the `Push failed to start` notification of this path.
- [ ] **Step 4: Check that they pass.** Expected: PASS. Update the row in `phase-3-commands.md` to the new command.
- [ ] **Step 5: Commit.** `git commit -m "fix: merge the full ref from the popup and push up to any commit"`

### Task 7: Branches popup search with actions

Research item 20, decision 8. Reference: `G/ui/branch/popup/GitBranchesTreePopupStep.kt:116-125`, `G/ui/branch/tree/GitBranchesTreeModelUtil.kt:212-258`, `G/ui/branch/tree/GitBranchesTreeSingleRepoModel.kt:93-134`, `G/ui/branch/popup/GitBranchesTreePopup.kt:110,166-228`, `G/config/GitVcsOptions.kt:53-54`.

**Files:**
- Modify: `src/branches/BranchesPopup.svelte`, `src/branches/state.ts` (`BranchOptions.filterByAction`, default `true`), `src/branches/model.ts` if the matching helpers move there.
- Test: `src/branches/BranchesPopup.test.ts`, `src/branches/model.test.ts`.

**Interfaces:**
- Consumes: `speedMatch` and `highlighted` (`src/branches/model.ts`, `src/lib/speedSearch.ts`).
- Produces: `filterActions(rows: ActionRow[], query: string): { rows: ActionRow[]; best: ActionRow | null }`, a pure function, so the degree rule can be tested alone.

- [ ] **Step 1: Write the failing tests:**
  - `the_placeholder_says_branches_and_actions`. With the toggle off, it says `Search for branches`.
  - `the_gear_starts_with_show_actions_in_search_results`: a checked toggle, then a separator, then `Group by Prefix`, `Show Recent Branches` and `Restore Popup Size`. Toggling it is saved in `branch-options.json`.
  - `typing_filters_the_actions_too`: `pus` leaves `Push…` with its highlight, plus the matching branches.
  - The separator rules:
    - the separator between actions and branches exists only when both are present;
    - with only actions matching, there is no separator and no empty text.
  - `a_matching_action_is_selected_before_a_matching_branch`: with `push` typed and a branch `push-fix`, `Push…` is selected and Enter opens the push dialog. The arrows go through actions and branches.
  - `with_the_toggle_off_actions_stay_and_are_skipped`: today's behaviour.
  - `without_results_says_not_found`: with the toggle on, `'zzz' not found`; with it off, `Branch 'zzz' not found`.
  - `the_minimum_width_is_375`.
- [ ] **Step 2: Check that they fail.** Expected: FAIL.
- [ ] **Step 3: Implement.**
  - The ongoing rows and both action blocks match on their displayed label.
  - Trailing spaces are trimmed, and a query of `/` alone filters nothing, as in 241.
  - The popup keeps its current debounce.
- [ ] **Step 4: Check that they pass.** Expected: PASS.
- [ ] **Step 5: Commit.** `git commit -m "feat: the branches popup searches actions too, as in IntelliJ 2024.1"`

### Task 8: Fetch while fetching, and protected patterns while typing

Research items 10 and 13, decisions 3 and 5. Reference: `G/actions/GitFetch.java:23-44`, `G/config/GitVcsPanel.kt:132-175`.

**Files:**
- Modify:
  - `src/update/flow.ts`: a `fetching` flag set around the explicit Fetch, Update and Pull, exposed as a subscribable value;
  - `src/App.svelte`: pushes the enabled ids again when the flag changes;
  - `src-tauri/src/commands.rs`: `Git.Fetch` joins the state-driven ids;
  - `src/branches/BranchesPopup.svelte`: the fetch button reads the shared flag instead of its own;
  - `src-tauri/src/push.rs` and `src-tauri/src/vcs_commands.rs`: a command `protected_patterns_check(text) -> Option<String>`;
  - `src/settings/SettingsView.svelte`.
- Test: `src/update/flow.test.ts`, `src/App.test.ts`, `src-tauri/src/commands.rs`, `src/branches/BranchesPopup.test.ts`, `src/settings/SettingsView.test.ts`, `src-tauri/src/push.rs`.

**Interfaces:**
- Produces: `protected_patterns_check(text: String) -> Option<String>`. It takes the `;`-separated field text and returns the first error as `Skipped invalid protected branch pattern '<pattern>': <cause>`. `<cause>` is the last `error:` line of the `regex` error, without the `error: ` prefix.

- [ ] **Step 1: Write the failing tests:**
  - `fetch_is_disabled_while_any_fetch_runs`: during an explicit Fetch, an Update and a Pull, the menu id `Git.Fetch` is not enabled and the popup button is disabled. Both come back when the fetch ends, also after a failure.
  - `merge_and_rebase_start_enabled_and_the_state_governs_them` now expects `Git.Fetch` to be state-driven.
  - Rust: `a_bad_pattern_reports_the_first_error`: `main;(` gives `Skipped invalid protected branch pattern '(': unclosed group`. Valid text gives `None`.
  - SettingsView:
    - `an_invalid_pattern_is_shown_while_typing`: the error appears under the field after typing, and also on opening with a bad saved value;
    - Save is not blocked;
    - no notification after Save. This replaces `an_invalid_pattern_is_reported_when_saving`.
- [ ] **Step 2: Check that they fail.** Expected: FAIL.
- [ ] **Step 3: Implement.** The settings field calls the check with the same debounce the app uses for its other text fields, or 300 ms if it has none.
- [ ] **Step 4: Check that they pass.** Expected: PASS.
- [ ] **Step 5: Commit.** `git commit -m "feat: Fetch waits for the running fetch and protected patterns are checked while typing"`

### Task 9: `Add to VCS` with changelists

Research item 17, decision 6. Reference: `platform/vcs-impl/src/com/intellij/openapi/vcs/changes/actions/ScheduleForAdditionAction.kt:15-27`, `G/util/GitFileUtils.java:188-203`. Texts: `AB:1553-1554`, `VB:962`, `VB:327`.

**Files:**
- Modify: `src-tauri/src/staging.rs` or a new function next to it, `src-tauri/src/vcs_commands.rs` (`changes_add(repo, paths)`), `src/lib/api.ts`, `src/commit/ChangesPanel.svelte` (the popup in changelist mode and ⌥⌘A), `src/commit/CommitDialog.svelte` (the browser menu, which includes the added files), `src/lib/keymap.ts` if ⌥⌘A is not bound in changelist mode.
- Test: `src-tauri/src/staging.rs` or `src-tauri/src/commit.rs`, `src/commit/LocalChangesView.test.ts`, `src/commit/CommitDialog.test.ts`.

**Interfaces:**
- Produces: `changes_add(repo: String, paths: Vec<String>)`.
  - It validates the paths as the other path commands do.
  - It calls `flush_diff_window` and takes `RepoLocks`.
  - It runs `git add --ignore-errors -A -- <paths>` under the title `Adding Files to VCS…`.
  - On failure, the notification title is `Failed to add files`.

- [ ] **Step 1: Write the failing tests:**
  - `add_to_vcs_adds_the_new_file_to_the_index` (Rust).
  - `committing_another_changelist_leaves_the_added_file_out`: the setup is a new file added with `changes_add` and a change in another file of another list. Committing only the other file leaves the new file out of the commit and still added in the index. This is Review Focus 4.
  - `mutating_without_saving_the_diff_is_not_allowed` and `whatever_touches_the_repository_saves_the_diff_window_first` count the new command.
  - Local Changes: `Add to VCS` with ⌥⌘A is visible only when the selection has unversioned files, after `Delete` and before `Remove from VCS` if that item exists. After it runs, the file shows in the active list.
  - Dialog: `Add to VCS` from the browser menu includes the file in the commit.
- [ ] **Step 2: Check that they fail.** Expected: FAIL.
- [ ] **Step 3: Implement.** If the commit test shows that the pipeline commits or loses the added file, fix the pipeline in this task and say so in the report.
- [ ] **Step 4: Check that they pass.** Expected: PASS.
- [ ] **Step 5: Commit.** `git commit -m "feat: Add to VCS with changelists"`

### Task 10: Small Rust alignments, and the merge and rebase widget

Research items 11, 12 and 18, decisions 4 and 7. Reference: `G/branch/GitBranchIncomingOutgoingManager.java:104-124`, `G/commands/GitHandler.java:491`, `plugins/git4idea/resources/META-INF/plugin.xml:496-510`, `G/ui/toolbar/GitMergeRebaseWidget.kt:45-102`.

**Files:**
- Modify: `src-tauri/src/branch_info.rs` (`has_external_agent`), `src-tauri/src/git.rs` (`command()`), `src/App.svelte` (main toolbar), `NOTICE` if icons are copied.
- Create: `src/operation/MergeRebaseWidget.svelte`, plus the icons it needs in `src/lib/icons/`.
- Test: `src-tauri/src/branch_info.rs`, `src-tauri/src/git.rs`, `src/operation/MergeRebaseWidget.test.ts`, `src/App.test.ts`.

**Interfaces:**
- Consumes: `ongoingActions(state, handlers)` and `RESOLVE_ID` from `src/operation/operation.ts`.
- Produces: `has_external_agent(agent: Option<&OsStr>) -> bool`. An agent path whose canonical path differs from itself is external. An error counts as not external.

- [ ] **Step 1: Write the failing tests:**
  - `a_symlinked_launchd_agent_counts_as_external`: a symlink `…/com.apple.launchd.x/Listeners` under a canonicalised temp dir points elsewhere. A missing launchd path is not external, and any other path is external.
  - `every_git_command_carries_intellij_git_executable`: `git -c alias.x='!echo $INTELLIJ_GIT_EXECUTABLE' x` prints `true`.
  - Widget:
    - nothing shows with no operation in progress;
    - during a rebase: `Continue Rebase` and `Abort Rebase`;
    - during a merge with conflicts: `Resolve Conflicts…` and `Abort Merge`;
    - `Abort Cherry-Pick` and `Abort Revert` in theirs;
    - each button's accessible name is the 241 text, and clicking runs the same handler as the Git menu;
    - Resolve and Continue have the "running" colour, and the aborts the "stop" colour.
  - App: the widget sits after the branch widget.
- [ ] **Step 2: Check that they fail.** Expected: FAIL.
- [ ] **Step 3: Implement.** Take the colours from the run-widget keys that `GitMergeRebaseWidget.kt` names, read from the installed IntelliJ's theme files, never written. If a value cannot be found, say so in the report and use the app's success and error colours.
- [ ] **Step 4: Check that they pass.** Expected: PASS.
- [ ] **Step 5: Commit.** `git commit -m "feat: merge and rebase widget, and two small git environment alignments"`

### Task 11: Soft-Wrap: the option, the toggles and plain wrapping

`research-soft-wraps.md` sections 1 to 4, decisions 9 and 10. Reference: `D/actions/impl/SetEditorSettingsActionGroup.java:47-235`, `D/tools/util/base/TextDiffSettingsHolder.kt:37-56,174-211`. Text `Soft-Wrap` (`AB:186`).

**Files:**
- Modify:
  - `src/diff/options.ts`: a per-place `wrap`, stored under `diff.wrap.<place>`;
  - `src/diff/editor.ts`: `EditorView.lineWrapping` in a `Compartment`, reconfigured by `setOptions`;
  - `src/diff/theme.ts`: `.rm-old-line` wraps when wrapping, and the checkbox and merge-arrow gutter cells align to the first row;
  - `src/diff/DiffWindow.svelte`: the gear item;
  - `src/diff/FilePane.svelte`: the gutter right-click menu;
  - `src/diff/ThreeVersionsPane.svelte` and `src/merge/editors.ts`, `src/merge/MergeWindow.svelte`: the three panes wrap together;
  - `src/commit/CommitDialog.svelte`, `src/commit/LocalChangesView.svelte`: they pass their place.
- Test: `src/diff/editor.test.ts`, `src/diff/DiffWindow.test.ts`, `src/diff/options` tests (create `src/diff/options.test.ts` if missing), `src/merge/editors.test.ts`, `src/commit/CommitDialog.test.ts`.

**Interfaces:**
- Produces:
  - `type DiffPlace = "Default" | "CommitDialog" | "ChangesView" | "Merge"`;
  - `loadWrap(place: DiffPlace): boolean` and `storeWrap(place: DiffPlace, on: boolean)`. The defaults are `Default` true and the others false (decision 9);
  - `EditorOptions.wrap: boolean`.

  Task 12 reads `EditorOptions.wrap`.

- [ ] **Step 1: Write the failing tests:**
  - `wrap_is_stored_per_place_with_its_default`: `Default` reads true and the other places false. Each place round-trips on its own.
  - `the_gear_has_soft_wrap_before_align`: the `Settings` menu lists `Soft-Wrap`, then `Align Changes Highlighting`. Toggling it updates both sides of the open diff without rebuilding the views (the same `EditorView` instances) and stores it for `Default`.
  - `right_click_on_the_gutter_offers_soft_wrap`: in the dialog's panel it stores `CommitDialog`, and in the Local Changes preview `ChangesView`. A right click on the text keeps today's menu.
  - `unified_old_lines_wrap_too`: with wrap on, `.rm-old-line` computes `white-space: pre-wrap`.
  - `the_three_merge_panes_wrap_together` and store `Merge`.
  - `wrap_off_keeps_todays_configuration`: no `.cm-lineWrapping` class.
- [ ] **Step 2: Check that they fail.** Expected: FAIL.
- [ ] **Step 3: Implement.** In this task the aligned diff keeps its line-count spacers. Task 12 replaces them.
- [ ] **Step 4: Check that they pass.** Expected: PASS.
- [ ] **Step 5: Commit.** `git commit -m "feat: Soft-Wrap in the diff and merge windows, per place as in IntelliJ"`

### Task 12: Soft-Wrap in the aligned diff: measured spacers

`research-soft-wraps.md` sections 2 ("Two-side, aligned"), 4 ("Aligned two-side and the embedded diffs") and 5. Reference: `D/tools/simple/AlignedDiffModel.kt:84-481`. Working model: `updateSpacers` in `node_modules/@codemirror/merge/dist/index.js:900-983`. It is MIT-licensed; if code is ported from it, `NOTICE` says so.

**Files:**
- Create: `src/diff/spacers.ts`.
- Modify: `src/diff/editor.ts` (`SpacerWidget` in pixels, the top and end spacers, the measuring pass), `src/diff/FilePane.svelte` (equal number-gutter widths when aligned), `NOTICE` if code was ported.
- Test: `src/diff/spacers.test.ts`, `src/diff/editor.test.ts`.

**Interfaces:**
- Consumes: `EditorOptions.wrap` (Task 11).
- Produces: `alignSpacers(boundaries: Boundary[], left: Measured, right: Measured, tolerance = 1): Spacers`, a pure function:
  - `Boundary` is a pair of line numbers, one per side, where the two sides must be level: each hunk's start and end, and the end of the document;
  - `Measured` is the text-only top of each boundary on one side, in px, with that side's own spacers subtracted;
  - `Spacers` lists, for each boundary, the side and the height in px of the spacer that levels it. That is IntelliJ's top inlay before a hunk, its bottom inlay after it, and one spacer at the end.

- [ ] **Step 1: Write the failing tests** in `src/diff/spacers.test.ts`:
  - `equal_heights_need_no_spacers`;
  - `a_hunk_taller_on_the_left_puts_its_spacer_on_the_right_after_the_hunk`;
  - `an_unchanged_line_wrapping_only_on_one_side_gets_a_top_spacer_before_the_next_hunk`;
  - `an_insertion_at_the_end_levels_the_end`;
  - `differences_within_the_tolerance_are_ignored`;
  - `feeding_its_output_back_changes_nothing`, which shows idempotence.

  In `src/diff/editor.test.ts`:
  - `with_wrap_off_spacers_are_line_counts_times_20`: today's output, as in `each_aligned_side_has_the_gap_of_the_lines_it_lacks`;
  - `with_wrap_on_spacers_follow_the_measured_heights`: `lineBlockAt` is stubbed with `vi.spyOn` to fake a wrapped line, and `.rm-spacer` gets the px heights;
  - `a_second_measuring_pass_dispatches_nothing`;
  - `the_two_number_gutters_have_the_same_width_when_aligned`.
- [ ] **Step 2: Check that they fail.** Expected: FAIL.
- [ ] **Step 3: Implement.**
  - The pass runs from an `updateListener` on `heightChanged` or `viewportChanged`.
  - It skips the transactions that carry its own spacer effect.
  - It is coalesced into one `requestAnimationFrame` for both editors.
  - It measures text heights only, never totals with spacers. That breaks the loop, as IntelliJ does.
  - `SpacerWidget` takes `height` in px. It has `eq` with the 1 px tolerance, `updateDOM` and `estimatedHeight`.
  - With wrap off, no measuring pass runs.
- [ ] **Step 4: Check that they pass.** Expected: PASS.
- [ ] **Step 5: Commit.** `git commit -m "feat: aligned diff stays level with soft wraps"`

### Task 13: Matrix, report and full verification

**Files:**
- Modify: `docs/parity-matrix.md`. Every row this part touched gets its result and its test, and the rows of decision 11 get their reason.
- Modify: `docs/reports/phase-4-validation.md`, with a section for part 2. It lists what was proven with tests, the decisions of this part, and the manual checklist:
  - Soft-Wrap in a real window, as listed in `research-soft-wraps.md` section 5 "Only in a real window";
  - the widget during a real rebase;
  - the popup search typed on a physical keyboard;
  - `Rebasing Published Commit` against a protected branch of a local remote;
  - `Add to VCS` followed by a commit.
- Modify: `docs/reports/phase-4-reference-change.md`: one line at the top saying which rows part 2 closed and which ones stay open.

- [ ] **Step 1:** Each `done` cites its test, as in the previous phases.
- [ ] **Step 2: Full verification, one suite at a time.**
  - `pnpm exec tsc --noEmit -p .`
  - `pnpm exec vitest run --maxWorkers=2`
  - `cargo test --lib -- --test-threads=4`
  - `cargo clippy --all-targets -- -D warnings`
  - `python3 -I -m unittest discover -s scripts`

  Expected: all of them pass. The report writes the exact counts.
- [ ] **Step 3: Commit.** `git commit -m "docs: phase 4 part 2 results in the matrix and the report"`
