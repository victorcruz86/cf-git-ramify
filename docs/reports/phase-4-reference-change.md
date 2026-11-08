# Reference change to IntelliJ IDEA CE 2024.1: what differs in phases 1 to 3

> Status after phase 4 part 2 (2026-11-07): part 2 closed items 2, 3, 5, 6, 7, 8 and 9 of the Summary and the full-ref `git merge` exception of item 10, in tests (nothing seen in a real window; soft wraps above all); items 1 and 4 were closed by part 1. Still open: `View saved changes…` on the unstash cards (part 3, with the Stash tab) and five matrix rows left out on purpose (`Git.Stage.ToggleCommitAll`, `Git.Stage.Stash.Files`, `Vcs.Log.OpenAnotherTabInEditor`, `Vcs.Diff.EnableDisableCombinedDiff`, `Vcs.Diff.ToggleSearchInChanges`; reasons in the matrix). Details: `phase-4-validation.md`, part 2.

Date: 2026-11-03. Before: IntelliJ IDEA CE 2023.2.3, tag `idea/232.10072.27`. After: IntelliJ IDEA CE 2024.1, build `IC-241.14494.240`, tag `idea/241.14494.240` (commit `c047a5caf941e5522efcf339eef7b660afa2e300`). Scope: the phase 1, 2 and 3 rows of `docs/parity-matrix.md`. Nothing is fixed here.

Method. Reading only: nothing was built, run or tested. The two tags were read from the partial copies in `.superpowers/ref-idea-232/ic/` and `.superpowers/ref-idea-241/ic/`, completed for this review with the `fetch_ic.py` scripts (Git settings, branch, push, update, merge, rebase, checkin, commands, branches popup, log UI, diff settings, merge viewer, commit mode). The message bundles were compared key by key, and every old value that changed or disappeared was searched in `src/` and `src-tauri/src/` with `git grep -F` (fragments of 10 characters or more, plus a hand-picked list of shorter ones). The action files (`plugin.xml`, `VcsActions.xml`, `vcs-log.xml`, `dvcs.xml`) were compared by structure: groups, their children in order, separators, `add-to-group` and `keyboard-shortcut`. The macOS keymaps of 232 come from the tag (`platform/platform-resources/src/keymaps/`) and those of 241 from the installed `app-client.jar`. The user's 2024.1 settings are the ones the controller read and gave in the brief; this review did not open `~/Library/Application Support/JetBrains`.

IntelliJ paths are relative to the root of `intellij-community`; `241:` and `232:` say which tag. Line numbers of rama are those of branch `phase-4` on 2026-11-03.

Impact, by the 2024.1 usage counts of `actionSummary.xml`: **high** for an action used thousands of times or for every commit; **medium** for hundreds of uses; **low** for tens, for actions not in the counts, or for a difference that rarely shows. Fix size: **S** one constant, label or function and its test; **M** one component or flow and its tests; **L** several components, the keyboard routing and the matrix.

## Summary

1. **Commit mode (high, L).** The user commits with the modal Commit Changes dialog and the Local Changes tab, in 2023.2 and in 2024.1 alike. rama built the non-modal Commit tool window with a staging alternative, which this user has never seen in IntelliJ (section 1).
2. **Soft wraps in the diff (medium to high, M).** The user has them on in 2024.1 (Diff.ShowDiff 14453, ChangesBrowser ShowDiff 6124). rama's diff does not wrap.
3. **Branches popup search (medium, S to M).** In 241 the search field says `Search for branches and actions`, the top actions are filtered too, and the gear has `Show Actions in Search Results`. The minimum width is 375 instead of 350. GitCheckoutAction (3753) goes through this popup.
4. **Two new commit checks on by default (medium, M):** files over 50 MB and file names that are not valid on Windows. They run on every commit and rarely fire.
5. **Drop Commits asks for confirmation (low, S).** The default is on in 241, and rama drops without asking, as 232 did.
6. **New dialog options (low, S each):** `--allow-unrelated-histories` in Merge, `--update-refs` in Rebase, and a `Cancel Reword` confirmation in the interactive rebase dialog.
7. **Update with rebase asks `Rebasing Published Commit` (low, S).** It also applies to Update Selected (423 uses). `Push All up to Here` now also works without a local branch (low, S).
8. **Merge and rebase toolbar widget (low, M).** While a merge, rebase, cherry-pick or revert is in progress, the main toolbar shows Resolve, Continue and Abort.
9. **About 25 copied texts changed (low, S together):** `Couldn't` became `Could not`, some titles are now in sentence case, the text filter tooltip changed, and some notifications gained action buttons.
10. **Unchanged:** the Git shortcuts, the Git main menu, the log context menu, the Local Changes popup, the diff toolbar, the diff and merge defaults and algorithms, and the git arguments of the flows that were spot-checked. The one exception is `git merge` from the popup, which now uses the full ref name.

## 1. Commit mode

### The chain, confirmed in the source

| Step | Source | Result for the user |
|---|---|---|
| `getNewCommitMode` returns `ExternalCommitMode` (staging) only if the single VCS has custom local changes. Otherwise it returns `NonModalCommitMode` if `isNonModalInSettings() && canSetNonModal()`, and `ModalCommitMode` if not | 241:`platform/vcs-impl/src/com/intellij/vcs/commit/CommitModeManager.kt:76-91` | — |
| `GitVcs.isWithCustomLocalChanges` = `isStagingAreaEnabled() && canEnableStagingArea()` | 241:`plugins/git4idea/src/git4idea/GitVcs.java:353-356` | — |
| `canEnableStagingArea() = CommitModeManager.isNonModalInSettings()` | 241:`plugins/git4idea/src/git4idea/index/GitStageManager.kt:49`; same in 232:`…/GitStageManager.kt:48` | Staging needs the non-modal setting |
| `isNonModalInSettings() = vcs.force.non.modal.commit \|\| COMMIT_FROM_LOCAL_CHANGES` | 241:`CommitModeManager.kt:149`; 232:`CommitModeManager.kt:173` | The registry key is false in both (241:`registry.properties:836`) |
| `COMMIT_FROM_LOCAL_CHANGES` defaults to false in 232 and true in 241 | 232/241:`platform/vcs-api/src/com/intellij/openapi/vcs/VcsApplicationSettings.java:31` | 2023.2: no entry, so false. 2024.1: explicit `false` |
| Modal mode does not use the Commit tool window, so Local Changes goes to the `Version Control` tool window | 241:`CommitModeManager.kt:182` (`ModalCommitMode` does not override `useCommitToolWindow`); 241:`…/changes/ui/ChangesViewContentManager.kt:51-52, 88-91`; 241:`VcsExtensions.xml:168-171` | Local Changes is a tab next to Log and Console (⌘9) |
| The Git settings page greys out `Enable staging area` while the mode is modal | 241:`plugins/git4idea/src/git4idea/config/GitVcsPanel.kt:187-188, 371`; comment `This will disable changelists support. Only for non-modal commit interface.` (GitBundle `settings.enable.staging.area.comment`) | `STAGING_AREA_ENABLED=true` is saved, greyed out and has no effect |

The modal mode is therefore not a change between 232 and 241. It was the user's mode in both versions, and phase 0 did not record it (matrix section 1 lists theme, keymap, log columns and `openInNewWindow` only). Since 241 flipped the default to non-modal, the user's explicit `false` is a deliberate choice.

### What rama built

The app starts on a `Commit` tab (⌘0) with the change tree with checkboxes, the toolbar and the message panel below it, and a separate `Log` tab (⌘9) (`src/App.svelte:59, 412-444`; `src/commit/CommitView.svelte:1316, 1451`; `src/commit/ChangesTree.svelte:317`). ⌘K moves the focus to the message (`src/lib/keymap.ts:31`; matrix row `ChangesView.ToggleCommitUi`: "The app only has the non-modal interface"). The staging view replaces the changelists when `stagingArea` is on (`src/commit/StagingView.svelte`; `src-tauri/src/settings.rs:41, 73`; `src/settings/SettingsView.svelte:169-174`). There is no `Use non-modal commit interface` setting (VcsBundle `settings.commit.without.dialog`).

### What the user would see differently on day one

| Difference | IntelliJ 2024.1 with the user's settings | rama | Impact | Size |
|---|---|---|---|---|
| Where the changes live | `Local Changes` tab of the `Version Control` tool window, together with Log and Console (⌘9) | Its own `Commit` tab (⌘0), with Log in another tab | high | L |
| ⌘0 | The Commit tool window does not exist, so ⌘0 does nothing | Shows the Commit tab (`src/lib/keymap.ts:35`) | low | S |
| Checkboxes in the tree | None in Local Changes. Inclusion is chosen in the dialog | Checkboxes per file and list in the tab (`ChangesTree.svelte:317`) | high | M |
| ⌘K | Opens the modal `Commit Changes` dialog (VcsBundle `commit.dialog.title`, 241:`…/changes/ui/CommitChangeListDialog.java:79`) with the changelist chooser, the tree with checkboxes, the message, the commit options on the side (Git: Author, Amend, Sign-off, plus the before- and after-commit checks), a collapsible `Diff` panel (`:383`, `changes.diff.separator`) and the Commit and `Commit and Push…` buttons | Moves the focus to the message panel of the tab (`keymap.ts:31`, `CommitView.svelte:1451`) | high | L |
| Commit options | Inline in the dialog | A separate `Show Commit Options` dialog (`src/commit/CommitMessage.svelte:163`, `OptionsDialog.svelte`) | medium | M |
| Partial commit by lines | Line and hunk checkboxes in the dialog's `Diff` panel | In the separate diff window, synchronized with the tab | high | L |
| Amend (⌃⌥M) and Commit and Push (⌥⌘K) | Inside the dialog | In the tab (`keymap.ts:32`; `CommitView.svelte:1199`) | medium | M |
| Fixup…, Squash Into…, Commit File | Open the `Commit Changes` dialog with the message or the file preset (241:`plugins/git4idea/src/git4idea/rebase/GitAutoSquashCommitAction.kt:36`) | Fill the Commit tab | low | M |
| Double click on a file in Local Changes | Opens the source, because `SHOW_DIFF_ON_DOUBLE_CLICK` defaults to false in the modal tab (241:`…/changes/actions/ShowOnDoubleClickToggleAction.kt:18`; `VcsApplicationSettings.java:39`). The value in the user's `vcs.xml` was not given | Opens the diff | low (⌘D: 14453) | S |
| Staging area | Saved as on, greyed out and inactive. The Staging view and `Git.Stage.*` are never shown | A checkbox that can be enabled and defaults to off. If the user's `true` were copied, rama would switch to a Staging view that IntelliJ never shows them | medium | S |
| Add to VCS ⌥⌘A (292) | With changelists, adds the unversioned files to the active changelist | Does nothing with changelists (matrix row `ChangesView.AddUnversioned`, partial). This gap already existed, and the modal mode makes it more visible | medium | M |

### What a switch would cost (not decided)

- `src/App.svelte` (tabs, ⌘0 and ⌘9, startup tab, decision "the app opens on the Commit view" in spec section 4): one `Version Control` area with Local Changes, Log and Console.
- `src/commit/CommitView.svelte` (1702 lines) split in two: a Local Changes view (tree without checkboxes, toolbar, changelists, popup menu, rollback) and a new modal Commit Changes dialog (changelist chooser, tree with checkboxes, `CommitMessage.svelte`, inline options, embedded `src/diff/DiffView.svelte` with the line checkboxes, Commit and Commit and Push…).
- `src/commit/inclusion.ts`, `selection.ts`, `request.ts`: inclusion scoped to a dialog session instead of the tab. The diff window (`src/diff/DiffWindow.svelte`, `window.ts`) would stop reporting inclusion to a view that no longer has checkboxes.
- `src/commit/WarningsGate.svelte`, the Fixup, Squash Into, Undo Commit and Commit File entry points (`src/log/editing.ts`), and Commit and Push (`src/push/model.ts`, `prepareCommitPush`).
- Settings: a `Use non-modal commit interface` setting (Settings > Version Control > Commit) that controls whether `Enable staging area` is enabled (`src-tauri/src/settings.rs`, `src/settings/SettingsView.svelte`). `StagingView.svelte` and `staging.ts` remain reachable only in non-modal mode.
- Tests: `CommitView.test.ts`, `StagingView.test.ts`, `App.test.ts`, `inclusion.test.ts`, `refresh.test.ts`, `DiffWindow.test.ts`.
- Matrix: section 2 rows 1 and 2, section 3 row `Enable staging area`, and section 4 rows `ActivateCommitToolWindow`, `ChangesView.ToggleCommitUi`, `CheckinProject`, `CheckinFiles`, `ChangesView.ShowCommitOptions`, `Vcs.ToggleAmendCommitMode`, `Vcs.Show.Local.Changes`, `Git.Commit.And.Push.Executor` and `Git.Show.Stage`.

The alternative of keeping only non-modal is also a choice: it would mean the user turns on `Use non-modal commit interface` in IntelliJ for a fair comparison.

## 2. Texts

Bundles compared (232 → 241): `GitBundle` 65 values changed, 18 removed and 87 added; `VcsBundle` 14, 10 and 28; `VcsLogBundle` 5, 2 and 10; `DiffBundle` 1, 2 and 16; `ActionsBundle` 16, 48 and 97, none of them Git; `DvcsBundle` 1, 0 and 0; `UIBundle` 2, 6 and 8; `CollaborationToolsBundle` 3, 9 and 53. Most changes are `Couldn't` → `Could not`, `However` → `However,`, `git` → `Git`, and HTML links turned into notification actions. The ones rama uses:

| Key (bundle) | 232 | 241 | rama | Impact | Size |
|---|---|---|---|---|---|
| `rebase.log.drop.action.failure.title` (Git) | `Couldn't Drop Commits` | `Could not Drop Commits` | `src/log/editing.ts:211` | low | S |
| `rebase.log.drop.success.notification.title` (Git) | `{0,choice,1#Commit\|2#Commits} Dropped` | `Dropped {0} {0,choice,1#commit\|2#commits}` | `src/log/editing.ts:212` | low | S |
| `rebase.log.drop.undo.impossible.title` (Git) | `Cannot Undo Drop` | `Cannot undo drop` | `src/log/editing.ts:213` | low | S |
| `rebase.log.drop.undo.failed.title` (Git) | `Undo Drop Failed` | `Undo drop failed` | `src/log/editing.ts:214` | low | S |
| `rebase.log.reword.action.failure.title` (Git) | `Couldn't Edit Commit Message` | `Could not Edit Commit Message` | `src/log/editing.ts:205` | low | S |
| `rebase.log.squash.action.failure.title` (Git) | `Couldn't Squash Commits` | `Could not Squash Commits` | `src/log/editing.ts:217` | low | S |
| `rebase.notification.failed.stash.text` (Git) | `Couldn't stash local uncommitted changes:` | `Could not stash local uncommitted changes:` | `src-tauri/src/rebase.rs:352` | low | S |
| `save.notification.failed.title` (Git), `notification.title.couldn.t.save.uncommitted.changes` (Vcs) | `Couldn't save uncommitted changes` | `Could not save uncommitted changes` | `src-tauri/src/update.rs:34` | low | S |
| `git.executable.validation.cant.identify.executable.message` (Git) | `Cannot identify version of git executable {0}` | `… of Git executable {0}` | `src-tauri/src/git.rs:370` | low | S |
| `git.tag.could.not.create.tag`, `branch.worker.could.not.create.tag` (Git) | `Couldn't create tag` | `Could not create tag` | `src/branches/actions.ts:631` | low | S |
| `git.rename.branch.could.not.rename.from.to` (Git) | `Couldn't rename {0} to {1}` | `Could not rename {0} to {1}` | `src/branches/actions.ts:492` | low | S |
| `checkout.operation.could.not.checkout.error[.title]` (Git) | `Couldn't checkout {0}` | `Could not checkout {0}` | `src/branches/actions.ts:344, 411` | low (GitCheckoutAction 3753, failure only) | S |
| `checkout.new.branch.operation.could.not.create.new.branch`, `create.branch.operation.could.not.create.new.branch` (Git) | `Couldn't create new branch {0}` | `Could not create new branch {0}` | `src/branches/actions.ts:440` | low | S |
| `delete.branch.operation.could.not.restore.branch.error`, `delete.tag.operation.could.not.restore.tag` (Git) | `Couldn't Restore {0}` | `Could not Restore {0}` | `src/branches/actions.ts:500, 569` | low | S |
| `delete.remote.branch.operation.couldn.t.find.remote.by.name` (Git) | `Couldn't find remote by name: {0}` | `Could not find remote by name: {0}` | `src-tauri/src/vcs_commands.rs:921` | low | S |
| `branch.ui.handler.unmerged.files.error.notification` → `….notification.text` (Git) | `You have to <a>resolve</a> all merge conflicts before {1}.<br/>After resolving conflicts you also probably would want to commit your files to the current branch.` | `You need to resolve all merge conflicts before {0}.<br/>After resolving conflicts do not forget to commit your files to the current branch before switching to another branch.`, with the action `Resolve conflicts…` (`….resolve.conflicts.action.text`) | `src/branches/merging.ts:45` (text), `:321` (action `Resolve…`) | low | S |
| `push.upto.here.not.supported.notification.title` / `.message` (Git) | `Push failed to start` / `'Push All up to Here' is supported for commit contained in a local branch only` | Removed (behaviour in section 4) | `src/push/model.ts:482-483` | low | S |
| `stash.unstash.unresolved.conflict.warning.notification.message` (Git) | `… saved to <a>stash</a>.<br/>Unstash is not complete, you have unresolved merges in your working tree<br/><a>Resolve</a> conflicts and drop the stash.` | The same without links, plus the actions `Resolve conflicts…` and `View saved changes…` | `src/branches/actions.ts:238` (rama's own wording, which matches neither) | low | S |
| `restore.notification.failed.stash.message` (Git) | `… saved to <a href='saver'>stash</a>.<br/>…` | `… saved to stash.<br/>…`, plus the action `View saved changes…` (`restore.notification.failed.show.changes.action`) | `src/update/flow.ts:47-48`: the text already matches 241; the action is missing | low | S |
| `vcs.log.commit.does.not.match.view.and.reset.link` (VcsLog) | `View and Reset Filters` | `View and reset filters` | `src/log/LogActions.svelte:76` | low | S |
| `vcs.log.commit.does.not.match.view.in.tab.link` (VcsLog) | `View in New Tab` | `View in new tab` | `src/log/LogActions.svelte:77` | low | S |
| `vcs.log.filter.text.hash.tooltip` (VcsLog) | `Text or Hash Filter` | `Filter by Commit Message or Hashes`, plus the new placeholder `Text or hash` (`vcs.log.filter.text.hash.empty.text`, 241:`platform/vcs-log/impl/src/com/intellij/vcs/log/ui/filter/VcsLogClassicFilterUi.kt:200, 254`) | `src/log/FilterBar.svelte:63, 372-373` | low | S |
| `git.branches.popup.search.field.empty.text` (Git, new) | `Search` (`editorsearch.search.hint`) | `Search for branches{0}{1}`, which in a single repository is `Search for branches and actions` | `src/branches/BranchesPopup.svelte:520-521` | medium | S |
| `git.branches.popup.tree.no.nodes` (Git, new) | `Branch '{0}' not found` | `'{0}' not found` when actions are filtered too | `src/branches/BranchesPopup.svelte` (empty text) | low | S |
| `merge.operation.merged.to` (Git, formatting only) | Branch names in bold and code | Code only | `src/branches/merging.ts` (success card) | low | S |

Texts that already follow 241: `Delete {0}` after a merge is an action button in rama (`src/branches/merging.ts:226`), as in 241 (232 had a link). `View them` is an action in rama (`src/git/flow.ts:286`), as in 241 (`local.changes.would.be.overwritten.by.merge.view.them.action`).

The texts hard-coded in Kotlin or Java instead of the bundles were not diffed systematically. The ones checked are unchanged, for example `GitRebaseTodoModelConverter.kt:24, 45`, which rama copies in `src/rebase/model.ts:355, 370` and `src-tauri/src/todo.rs:202`.

## 3. Menus and action groups

The structure of `plugins/git4idea/resources/META-INF/plugin.xml`, `platform/vcs-impl/resources/META-INF/VcsActions.xml`, `platform/vcs-log/impl/src/META-INF/vcs-log.xml` and `platform/dvcs-impl/src/META-INF/dvcs.xml` was compared group by group, in the same paths in both tags. **The groups below did not change:** `Git.MainMenu`, `Git.Experimental.Branch.Popup.Actions` and the branch submenus, `Vcs.Log.ContextMenu` and `Git.Log.ContextMenu`, `ChangesViewPopupMenu` and the changes view toolbar, `Git.Stage.*` except the line below, and the diff toolbar, which is built in code (241:`platform/diff-impl/src/com/intellij/diff/tools/simple/SimpleDiffViewer.java` and `tools/util/base/TextDiffViewerUtil.java` add no actions).

| Difference | 232 → 241 | rama | Impact | Size |
|---|---|---|---|---|
| Branches popup gear `Git.Branches.Popup.Settings` | + `Show Actions in Search Results` (`git.branches.popup.filter.by.action`, the single-repository text) and `Repositories` (multi-root only), under `Add in Search Results` (241:`plugin.xml:212`; `ui/branch/popup/GitBranchesTreePopupHeaderActions.kt:121-150`) | The gear has `Show Recent Branches` and the grouping options, without this toggle (`src/branches/BranchesPopup.svelte:445`) | medium | S |
| Main toolbar `MainToolbarVCSGroup` | + `main.toolbar.git.MergeRebase`, a widget with `GitMergeRebaseWidgetGroup`: Resolve Conflicts, Continue Rebase, Abort Rebase, Abort Merge, Abort Cherry-Pick and Abort Revert, coloured, shown while the operation is in progress (241:`plugin.xml:496-510`; `ui/toolbar/GitMergeRebaseWidget.kt`) | Notification cards with Continue and Abort, and the Git menu. There is no toolbar widget | low (Git.Merge.Abort 43) | M |
| Edit Commit Message dialog toolbar `Git.Reword.ToolbarActions` | + `Vcs.ShowMessageHistory` (241:`plugin.xml:56`) | No history button in the reword dialog. Message history belongs to phase 4 (158 uses) | low | S, after phase 4 |
| `CommitView.GearActions` | + `Git.Stage.Configure` (`Display as`: Staging Area / Changelists) and `Git.Stage.ToggleCommitAll` (`Commit All when Nothing Is Staged`) | Missing. Only shown in non-modal mode, so not for this user | low | S |
| `Git.Stage.Tree.Menu` | + `Git.Stage.Stash.Files` (`Stash Files`) after `Git.Stage.Reset` | Missing. Staging only, and it needs the stash of phase 4 | low | S |
| `LocalChangesView.ShowOnDoubleClick` | Its own two actions are replaced by references to `CommitView.ShowOnDoubleClick.EditorPreview` and `.Source` (VcsActions.xml) | Matrix out-of-scope rows `LocalChangesView.ShowOnDoubleClick.*` no longer exist | low | S (matrix) |
| `Vcs.Log.ShowOtherBranches` | Removed, together with its references in `Log.FileHistory.KeymapGroup` and `Vcs.FileHistory.Toolbar` (232:`vcs-log.xml:138, 252`) | Matrix phase 4 row, pending: it should be dropped | low | S (matrix) |
| `Log.Unsorted.KeymapGroup` | + `Vcs.Log.OpenAnotherTabInEditor` (`Open New Git Log Tab in Editor`). `Vcs.Log.OpenAnotherTab` becomes the `InToolWindow` variant | No editor tabs. Not applicable | low | — |
| `Vcs.Diff.Commit.KeymapGroup` | + `Vcs.Diff.IncludeWholeChangedLinesIntoCommit` (no shortcut) | The line selection actions are partial in the matrix anyway | low | S |
| `Vcs.Diff.EditorTabs.Group`, `Editor.Search.Filter.Results` | + `Vcs.Diff.EnableDisableCombinedDiff` and `Vcs.Diff.ToggleSearchInChanges` | Not applicable (no editor tabs, no find in the diff) | low | — |
| Main toolbar quick actions | `addQuickAction.Git.*` become `MainToolbarQuickActions.VCS` | Out of scope in the matrix | — | — |
| Branches popup classic list | `GitBranchPopup` removed. Only the tree popup remains (241:`ui/branch/GitBranchesAction.java`) | rama copies the tree popup (232 already used it in the New UI, 232:`GitBranchesTreePopup.kt:728`) | — | — |

## 4. Defaults and behaviour

Git settings defaults that did not change: `USE_CREDENTIAL_HELPER=false` and `STAGING_AREA_ENABLED=false` (241:`config/GitVcsApplicationSettings.java:25-26`), the update method, the save policy, the incoming check, the push options and `SHOW_RECENT_BRANCHES=true` (`GitVcsOptions.kt:50-51`). The other defaults are unchanged too: those of the log (`VcsLogUiPropertiesImpl.kt`, `VcsLogApplicationSettings.kt`, column visibility only moved), of the diff (`TextDiffSettingsHolder.kt`, only a listener added), of the comparison and merge algorithms (`platform/util/diff/…/comparison/*`, `platform/diff-impl/…/merge/*` except a refactor of `MergeThreesideViewer.java`), and of the registry keys `git.*` except `git.enable.stash.toolwindow`. The git arguments are identical in `commands/GitImpl.java`, the commit options (`checkin/*`), `fetch/GitFetchSupportImpl.kt`, `actions/GitPull.java`, `merge/GitPullDialog.kt`, `update/GitMergeUpdater.java`, `update/GitRebaseUpdater.java`, `cherrypick/GitCherryPicker.java` and `reset/GitResetOperation.java`.

| Difference | 232 → 241 | rama | Impact | Size |
|---|---|---|---|---|
| Drop Commits confirmation | 232: none. 241: `SHOW_DROP_COMMIT_DIALOG=true` (`config/GitVcsApplicationSettings.java:29`). An OK/Cancel warning `Drop Commit(s)` / `Do you want to drop N commit(s)?` with `Drop` and `Don't ask again` (`rebase/log/drop/GitDropLogAction.kt:28, 52-63`). Setting `Ask for confirmation to drop commits` (`rebase/log/GitLogCommitEditingConfigurable.kt:17`). The user's `git.xml` has no entry, so it is on | Drops without asking (`src/log/editing.ts:424`; phase 3 decisions, line 28) | low | S |
| Drop success notification | 232: title only. 241: the body lists up to 10 subjects in quotes, shortened to 40 characters (`GitDropLogAction.kt:35-38`) | Title only | low | S |
| Large file check before the commit | New: `WARN_ABOUT_LARGE_FILES=true`, `WARN_ABOUT_LARGE_FILES_LIMIT_MB=50` (`config/GitVcsOptions.kt:62-65`). It warns `You are about to commit a large file into the repository: …` and is skipped if `lfs.repositoryformatversion` is set (`checkin/GitCheckinHandlerFactory.kt:62, 159-190`). New settings row `Warn when committing files larger than [50] MB` (`config/GitVcsPanel.kt`) | Only the CRLF, detached HEAD and rebase checks (`src-tauri/src/commit.rs:995-996`; `src/commit/WarningsGate.svelte:80, 112`) | medium | M |
| File name check before the commit | New: `WARN_ABOUT_BAD_FILE_NAMES=true` (`GitVcsOptions.kt:66`). Off on Windows, on on macOS. It flags Windows reserved names and characters in added or renamed paths, with the dialog `File Name Is Not Allowed on Windows` (`GitCheckinHandlerFactory.kt:74, 437-500`). New settings row `Warn when committing files with names that might cause issues on other systems` | Missing | low | M |
| Protected branch patterns | An invalid pattern is skipped when matching (`config/GitSharedSettings.java`, `isMatchSafe`), and the field shows the error while typing (`GitVcsPanel.kt`, `validateProtectedBranchesPatterns`) | Reported when saving (`an_invalid_pattern_is_reported_when_saving`) | low | S |
| Merge dialog | + `--allow-unrelated-histories`, `Allow merging histories that do not share a common ancestor.`, compatible with every option (`merge/GitMergeOption.kt:17, 26`; `merge/GitMergeDialog.kt:416`) | `src/git/options.ts:6-25` | low | S |
| Rebase dialog | + `--update-refs`, `Auto-update branches pointing to rebased commits`, a plain flag with no git version check (`rebase/GitRebaseOption.kt:19, 38`; `rebase/GitRebaseDialog.kt:468`). The user's git 2.38.0 supports it | `src/git/options.ts:86-97`; `src-tauri/src/rebase.rs:722-740` | low | S |
| Interactive rebase dialog | New: cancelling while a reword is being edited asks `Cancel Reword` / `Entered commit message will be lost` with `Keep Editing` (`rebase/interactive/dialog/GitRebaseCommitsTableView.kt:115-117`) | No confirmation | low | S |
| Edit Commit Message dialog | New: a changed message is saved to the commit message history on close (`rebase/log/GitNewCommitMessageActionDialog.kt:123`) | No message history until phase 4 | low | S |
| Update with rebase | New: before rebasing, it asks `Rebasing Published Commit` if the commits to rebase are already in a protected branch, and Cancel aborts (`update/GitRebaseUpdater.java:62, 75`). This covers Update Project and Update Selected. `Pull…` runs `git pull` directly and is not affected | `src-tauri/src/update.rs:413` does not check. The check exists for the popup rebase (`src-tauri/src/rebase.rs:555`, `vcs_commands.rs:218`) | low (Update Selected 423; it only fires with published commits) | S |
| `git merge` from the branches popup | 232: `git merge <short name>`. 241: `git merge <full ref>` (`refs/heads/x`, `refs/remotes/origin/x`, `refs/tags/x`) (`branch/GitMergeOperation.java:84, 228`) | `git merge x` (`src-tauri/src/merge.rs:210`; `docs/reports/phase-3-commands.md`, row 1) | low (GitMergeBranchAction 633; same result unless a branch and a tag share a name) | S |
| `Push All up to Here…` | 232: disabled without a current branch, and an error if the commit is in no local branch. 241: always enabled. A commit outside local branches is pushed as a detached source (`actions/GitPushUpToCommitAction.kt:16-42`; the `isEnabled` override is gone) | `src/log/contextMenu.ts:322` (disabled), `src/push/model.ts:478-486` (error) | low | S |
| Branches popup search | Filters the top actions as well as the branches when `FILTER_BY_ACTION_IN_POPUP` is on, which is the default (`config/GitVcsOptions.kt:53-56`; `ui/branch/tree/GitBranchesTreeModelUtil.kt:252-258`). Placeholder and empty text as in section 2. Minimum size 375×300 instead of 350×300 (`ui/branch/popup/GitBranchesTreePopup.kt:110, 162-174`) | Only branches, placeholder `Search`, minimum width 350 (`src/branches/BranchesPopup.svelte:80, 520-521`) | medium | M |
| Incoming check over ssh (`Auto`) | A launchd `SSH_AUTH_SOCK` that is a symlink to another agent counts as an external agent (`branch/GitBranchIncomingOutgoingManager.java:110-121`) | Only the `com.apple.launchd` substring (`src-tauri/src/branch_info.rs:33, 361`) | low | S |
| Environment of every git command | + `INTELLIJ_GIT_EXECUTABLE=true` (`commands/GitHandler.java:491`; `commands/GitCommand.java:84`) | Not set (`src-tauri/src/git.rs:296-297`). Only hooks can see it, and copying an IntelliJ-named variable is debatable | low | S |
| Fetch while a fetch is running | `Git.Fetch` is disabled while a fetch runs (`actions/GitFetch.java:34`) | `src/update/flow.ts:603` | low (Git.Fetch 1076, overlap is rare) | S |
| HTTP login dialog | Empty username or password is rejected on OK, unless the credential helper is on (`commands/GitHttpLoginDialog.kt`) | `src/lib/AskpassDialog.svelte` | low | S |
| Conflicts dialog → merge window | The merge request now carries the conflict type (`platform/vcs-impl/…/merge/MultipleFileMergeDialog.kt:445`) | Not compared (see the end) | low | — |
| Notifications | Several links became action buttons: checkout `Rollback`, `Resolve conflicts…`, `View saved changes…`, `Delete x` (`branch/GitCheckoutOperation.java`, `branch/GitBranchUiHandlerImpl.java`, `branch/GitMergeOperation.java:159-161`) | Already buttons except the ones listed in section 2 | low | S |
| Stash tab | `git.enable.stash.toolwindow` is true in 241 (`plugin.xml:605`; false in 232). It is a phase 4 feature, and in modal mode the tab goes to the `Version Control` tool window | Phase 4 | — | — |

## 5. Keymap

No difference for any action in the matrix. The macOS keymaps (`$default.xml`, `Mac OS X.xml`, `Mac OS X 10.5+.xml`) differ only in `CodeFloatingToolbar.GotoNextMenu` and `GotoPrevMenu` (added) and `ServiceView.GroupByServiceGroups` (removed). No `keyboard-shortcut` element changed in `plugin.xml`, `VcsActions.xml`, `vcs-log.xml` or `dvcs.xml`. The only change in what a shortcut does for this user comes from the commit mode (section 1): ⌘0 has no Commit tool window, and ⌘K opens a dialog.

## 6. User settings

| Setting | IdeaIC2023.2 (matrix section 1) | IdeaIC2024.1 (brief) | rama | Impact | Size |
|---|---|---|---|---|---|
| Commit interface | Not recorded. No entry in `vcs.xml`, so `COMMIT_FROM_LOCAL_CHANGES=false` (modal) | `COMMIT_FROM_LOCAL_CHANGES=false`, explicit because the 241 default is true | Non-modal only | high | L (section 1) |
| Staging area | Not recorded | `STAGING_AREA_ENABLED=true`, inactive because the mode is modal | Default false. Could be enabled freely | medium | S |
| Credential helper | Not recorded | `USE_CREDENTIAL_HELPER=true` | Default true. Phase 2 decision 3 says "unlike IntelliJ", which now matches the user | — | S (decision wording) |
| Soft wraps in the diff | Not recorded | On in the Default place (`diff.xml`; `TextDiffSettingsHolder.kt:46`, `USE_SOFT_WRAPS`, default false) | No wrapping in `src/diff/` (no `lineWrapping` and no soft wrap option) | medium to high (Diff.ShowDiff 14453) | M (aligned side-by-side with wrapped lines) |
| Log columns | Root, Subject, Author, Date, Space | Root, Subject, Author, Date (no Space.CommitStatus) | Subject, Author and Date (`columns_subject_author_date_by_default`) | — | — |
| Diff in its own window | `openInNewWindow=true` | `openInNewWindow=true` | Own window | — | — |
| Patch location | Not recorded | `PATCH_STORAGE_LOCATION=$USER_HOME$/Downloads/PATCHS` | Phase 4 (Create Patch 2348, Copy as Patch 3430) | phase 4 | — |
| Drop confirmation | Did not exist | No entry, so default true | No confirmation | low | S (section 4) |
| Theme | `ExperimentalDark` | Not given. Registry `ide.experimental.ui=true` (New UI) | Dark New UI | — | — |
| Keymap | No `keymap.xml`, default macOS | Not given | Default macOS | — | — |
| Registry | Not recorded | Only `ide.experimental.ui=true` and `debugger.new.tool.window.layout=true` | Not applicable | — | — |

## What could not be checked

- A running 2024.1 with the `git-wrapper.sh` trace. All the comparisons above come from reading the code. The command spot-checks cover the files listed in section 4, not every flow of `phase-{1,2,3}-commands.md`.
- The user's project-level Git settings in 2024.1 (`GitVcsSettings` in each project's `.idea/workspace.xml`): update method, save policy, protected branches, large file warning. They were not read.
- Values the brief does not mention, assumed to be at their default: `SHOW_DIFF_ON_DOUBLE_CLICK`, `SHOW_DROP_COMMIT_DIALOG`, theme and keymap in 2024.1.
- Whether the standalone diff window shows the line inclusion checkboxes in modal mode, outside the Commit Changes dialog. The workflow code that decides it (`ChangesViewManager`, `LocalChangeListDiffRequest` users) was not read.
- The exact layout of the 241 Commit Changes dialog (option panels, button order, the position of the Diff splitter). Only `CommitChangeListDialog.java` was skimmed.
- The log UI rewrite from Java to Kotlin (`platform/vcs-log/impl/src/com/intellij/vcs/log/ui/filter`, `table`, `frame`): only the file-level diff sizes and the text filter were read. `VcsStructureChooser.java` (Paths filter, 69 changed lines) and `StructureFilterPopupComponent.java` were not reviewed.
- The effect of passing the conflict type to the merge request (`MultipleFileMergeDialog.kt:445`) on deleted and modified conflicts.
- Hard-coded texts outside the bundles, and rama literals shorter than 10 characters other than the hand-picked ones (`Rollback`, `Reset Head`, `View them`, `Delete …`, `anyway`, `However`, `Couldn't`).
- The usage count of `CheckinProject` and `Vcs.UpdateProject`, which are not among the counts in the brief. The impact of the commit mode is rated high because it affects every commit.
