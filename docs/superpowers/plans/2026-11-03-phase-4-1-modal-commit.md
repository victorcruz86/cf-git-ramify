# Phase 4, part 1: modal commit mode. Implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** the user commits as they do in IntelliJ 2024.1, in modal mode with changelists:
- the `Git` tool window (⌘9) with the Local Changes, Log and Console tabs;
- ⌘K opens the `Commit Changes` dialog;
- the `Use non-modal commit interface` setting goes back to the current Commit tool window.

The same dialog, with an executor, serves `Shelve Changes` and `Create Patch` in part 3.

**Architecture:**
- `CommitView.svelte` is split in three:
  - a `ChangesPanel` that both modes share: tree, toolbar, menus, changelists, rollback and notices;
  - a `LocalChangesView` without checkboxes, for modal mode;
  - the current non-modal view, which keeps its checkboxes and the message panel.
- The dialog is an HTML modal in the main window, built from:
  - a pure TS model, `dialog.ts`, for the initial inclusion, the initial list, the message policy and the per-executor texts;
  - existing components: `ChangesTree`, `CommitMessage` in `editorOnly` mode, `FilePane` embedded, and `WarningsGate`.
- The diff window learns who owns the line inclusion: nobody, the non-modal view, or the open dialog.

**Tech Stack:** Tauri 2.12, Rust 1.99.1, Svelte 5, Vitest 5, CodeMirror 6. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-10-31-intellij-git-client-design.md`, and the phase 4 overview `docs/superpowers/plans/2026-11-03-phase-4-overview.md`: its Global Constraints and Decisions are part of this plan. Starting point: `docs/reports/phase-4-reference-change.md` section 1. Research: `.superpowers/phase-4/research-modal-commit.md`.

**Reference:** tag `idea/241.14494.240`. Paths are relative to `intellij-community`:
- `VI/` = `platform/vcs-impl/src/com/intellij/`;
- `VA/` = `platform/vcs-api/src/com/intellij/openapi/vcs/`;
- `G/` = `plugins/git4idea/src/git4idea/`.

Bundles: `V:` = VcsBundle, `GB:` = GitBundle, `A:` = ActionsBundle. Whoever implements reads the cited file and copies the literal texts from there.

**Branch:** `phase-4`.

**Commands** (the user asked to keep the machine load low):
- TS: `PATH=$HOME/.cache/rama-toolchain/node-v22.23.4-darwin-arm64/bin:$PATH pnpm exec vitest run --maxWorkers=2 <files>` and `pnpm exec tsc --noEmit -p .`
- Rust: `PATH=$HOME/.cargo/bin:$PATH CARGO_BUILD_JOBS=4 cargo test --manifest-path src-tauri/Cargo.toml --lib <filter> -- --test-threads=4`
- Python: `python3 -I -m unittest discover -s scripts`

## Decisions of this part

They add to those of the overview, which the user approves with this plan.

1. **The default is modal**, as the user has it. `Settings.non_modal_commit` defaults to `false`; IntelliJ 2024.1 defaults to `true`. Cost: a user who did not choose this would find the dialog first.
2. **One `Git` stripe button in modal mode.** rama has no editor area, so the `Git` tool window fills the content area. Log and Console stop being separate stripe buttons and become tabs: Local Changes, then Log, then Console, by IntelliJ's weights. Shelf and Stash come in part 3. In non-modal mode the stripe shows `Commit` (⌘0) and `Git` (Log and Console). Pressing ⌘9 again with the window active does nothing; IntelliJ would hide the window, and rama has nothing to show behind it. Cost: the layout changes for whoever used the current stripe.
3. **A double click in Local Changes opens the diff until part 4.** IntelliJ opens the source by default (`SHOW_DIFF_ON_DOUBLE_CLICK=false`), but rama has no viewer until part 4. Part 4 adds the `Show on Double-Click` gear item and the source viewer. Cost: until then a double click shows the diff, not the file.
4. **The `Preview Diff` splitter is included**, visible by default (`LOCAL_CHANGES_DETAILS_PREVIEW_SHOWN=true`), as a read-only embedded `FilePane` without line checkboxes. Cost: an embedded diff that loads when the selection changes; nothing loads while the preview is hidden.
5. **No `DiffCommitMessageEditor`.** In IntelliJ, the diff window opened from the dialog shows a second editor for the message. rama edits the message only in the dialog. Cost: little; IntelliJ hides that editor easily.
6. **Dialog size.** The first opening is 900×700, centred. After that the size, both splitters and the open or closed `Diff` panel are remembered in `localStorage` under IntelliJ's keys (`CommitChangelistDialog2`, `CommitChangeListDialog.SPLITTER_PROPORTION_2`, `CommitChangeListDialog.DETAILS_SPLITTER_PROPORTION_2`, `CommitChangeListDialog.DETAILS_SHOW_OPTION_`). Cost: the first opening may differ from IntelliJ's preferred size.
7. **The message history keeps 25 entries**, as IntelliJ does (`VA/VcsConfiguration.java:146`). It kept 20 since phase 1 (`src-tauri/src/messages.rs:7`). Cost: none.
8. **No code checks and no `Commit Message Inspections` table.** rama has no code model, and `validate.ts` keeps its fixed rules. The `Commit Checks` group shows only `Run Git hooks`, and only when the repository has hooks. Cost: none for a Git-only use.
9. **The setting switches on Save.** IntelliJ moves the tabs live when the setting is applied. rama remounts the views on Save. The draft survives because it lives in the changelist comment. Cost: the views blink once.

## Review Focus

1. **Devin changes files while the dialog is open.** The tree refreshes:
   - a file that is gone leaves the inclusion;
   - a new file comes in unchecked;
   - a line selection that is no longer valid is reported, as in the non-modal view;
   - Commit sends only what the dialog shows at that moment.

   Test in Task 6.
2. **Cancel and reopen.** The draft is kept as the changelist comment, the inclusion is not kept, and amend comes back unchecked. Changing the list in the chooser saves the draft to the old list. Tests in Tasks 5 and 6.
3. **Switching the mode with work in progress.** A draft in the non-modal view, or line selections in the diff window, are not lost when the user switches to modal and back. The diff window stops showing line checkboxes when nobody owns them. Tests in Tasks 3 and 7.
4. **⌘K, Commit File…, Fixup and Squash from each place.** Each arrives at the dialog with IntelliJ's inclusion and list rules, and with the prefilled text. Tests in Tasks 5 and 8.
5. **The 241 checks on a commit that has a 60 MB file or a file called `aux.txt`.** Each warning shows over the dialog. Cancel keeps the dialog open, and `Commit Anyway` continues. Test in Task 9.

---

### Task 1: The matrix regenerated from 241

Reference: decision 2 of the overview. The 241 sources needed by `scripts/extract_actions.py`:
- `plugins/git4idea/resources/META-INF/plugin.xml`, `platform/vcs-impl/resources/META-INF/VcsActions.xml`, `platform/vcs-log/impl/src/META-INF/vcs-log.xml`, `platform/dvcs-impl/src/META-INF/dvcs.xml`;
- the bundles that the matrix header cites;
- the macOS keymaps from the installed `app-client.jar` (`keymaps/$default.xml`, `Mac OS X.xml`, `Mac OS X 10.5+.xml`).

Fetch the sources with `.superpowers/ref-idea-241/fetch_ic.py`.

**Files:**
- Modify: `scripts/extract_actions.py` (rules), `scripts/test_extract_actions.py`, `docs/parity-matrix.md`

**Interfaces:**
- Produces: `docs/parity-matrix.md` with the header naming 241, section 1 with the user's 2024.1 settings, and section 4 regenerated with the Result column kept.

- [ ] **Step 1: Write the failing tests** in `scripts/test_extract_actions.py`:
  - `test_results_are_kept_by_action_id`: a regenerated table keeps the Result text of every action id that exists in both versions.
  - `test_removed_actions_are_listed_apart`: an id that 241 no longer has (`Vcs.Log.ShowOtherBranches`) leaves the table and comes back in a "Removed in 241" list.
  - `test_stage_actions_are_marked_non_modal`: `Git.Stage.*` rows say `only in non-modal mode with the staging area`.
- [ ] **Step 2: Check that they fail.** Run `python3 -I -m unittest discover -s scripts`. Expected: FAIL.
- [ ] **Step 3: Implement.** Add a `--previous` argument that reads the current matrix and merges the Result column by action id. Print the removed ids. Then regenerate the table:
  - Header: build `IC-241.14494.240`.
  - Section 1:
    - commit interface modal (`COMMIT_FROM_LOCAL_CHANGES=false`);
    - staging saved but inactive;
    - soft wraps in the diff;
    - log columns Root, Subject, Author, Date;
    - `openInNewWindow=true`;
    - `PATCH_STORAGE_LOCATION=$USER_HOME$/Downloads/PATCHS`.

    The source is `IdeaIC2024.1/options/`, read on 2026-11-03.
  - Section 3: a new row `1 | Use non-modal commit interface (Version Control > Commit) | pending`, which Task 2 fills.
- [ ] **Step 4: Check that they pass.** Run the Python command. Expected: PASS. Then `git diff --stat docs/parity-matrix.md`: only rows of actions added or removed in 241, and the header and section 1, change in the table.
- [ ] **Step 5: Commit.** `git commit -m "docs: regenerate the parity matrix from IntelliJ 2024.1"`

### Task 2: Settings of the commit mode

Reference:
- `VI/openapi/vcs/configurable/CommitDialogConfigurable.kt:42-51,101` (page `Commit`, V:284; `Use non-modal commit interface`, V:791; comment `Applies to projects under Git and Mercurial`, V:795; `Clear initial commit message`, V:141);
- `G/config/GitVcsPanel.kt:186-188,362-372` (`Enable staging area` greyed out, with the comment GB:717);
- `G/GitVcs.java:353-356`.

**Files:**
- Modify: `src-tauri/src/settings.rs`, `src/lib/api.ts`, `src/settings/SettingsView.svelte`, `src/settings/SettingsView.test.ts`

**Interfaces:**
- Produces:
  - `Settings.non_modal_commit: bool` (default `false`, decision 1) and `Settings.clear_initial_commit_message: bool` (default `false`), serialised as `nonModalCommit` and `clearInitialCommitMessage`.
  - In TS:
    - `Settings.nonModalCommit`, `Settings.clearInitialCommitMessage`;
    - `effectiveStaging(settings: Settings): boolean`, equal to `settings.nonModalCommit && settings.stagingArea`, in `src/lib/api.ts`;
    - `commitMode(settings: Settings): "modal" | "non-modal"`, in the same file.

- [ ] **Step 1: Write the failing tests.**
  - Rust (`settings.rs`): `non_modal_commit_and_clear_message_default_to_false`; `a_settings_file_without_the_new_fields_reads_modal`.
  - TS (`SettingsView.test.ts`):
    - `the Commit section shows Use non-modal commit interface with its comment and saves it`;
    - `Enable staging area is greyed out while the commit interface is modal`, which also checks that it is enabled again as soon as the checkbox above is ticked, without saving;
    - `staging is effective only in non-modal mode`, which tests `effectiveStaging`.
- [ ] **Step 2: Check that they fail.** Run the Rust filter `settings::` and `SettingsView.test.ts`. Expected: FAIL.
- [ ] **Step 3: Implement.** Add a `Commit` section before `Git` in `SettingsView.svelte`, with the two checkboxes. `Enable staging area` stays saved, and its value is not touched when it is greyed out. Every place that reads `settings.stagingArea` to choose a view reads `effectiveStaging(settings)`: `grep -rn stagingArea src`.
- [ ] **Step 4: Check that they pass.** Run the whole TS suite and `cargo test --lib settings::`. Expected: PASS.
- [ ] **Step 5: Commit.** `git commit -m "feat: commit mode settings, modal by default"`

### Task 3: The `Git` tool window and its tabs

Reference:
- `platform/vcs-impl/resources/META-INF/VcsExtensions.xml:168-193`;
- `VI/openapi/vcs/changes/ui/vcsToolWindowFactories.kt:30-35,77-81` (stripe title `Git`, the Commit window only in non-modal mode);
- `VI/openapi/vcs/changes/ui/ChangesViewContentManager.kt:33-91,229-258,315-321` (weights: Local Changes 10, Shelf 40, Stash 41, Log 50, Console 60; first content selected; contents moved between windows);
- `VI/openapi/vcs/changes/ChangesViewManager.java:1088` (`Local Changes` V:879 in modal mode, `Commit` V:1112 in non-modal mode);
- keymap `ActivateVersionControlToolWindow` ⌘9, `ActivateCommitToolWindow` ⌘0.

**Files:**
- Create: `src/vcs/VersionControlWindow.svelte`
- Modify: `src/lib/tabs.ts`, `src/lib/tabs.test.ts`, `src/App.svelte`, `src/App.test.ts`, `docs/superpowers/specs/2026-10-31-intellij-git-client-design.md` (section 4, "Startup")

**Interfaces:**
- Consumes: `commitMode(settings)` from Task 2.
- Produces:
  - `type ContentId = "local-changes" | "shelf" | "stash" | "log" | "console"`.
  - `CONTENT_WEIGHT: Record<ContentId, number>` with IntelliJ's weights.
  - `orderContents(ids: ContentId[]): ContentId[]`.
  - `toolWindows(mode, available: ContentId[]): { commit: ContentId[] | null; git: ContentId[] }`: in modal mode `commit` is `null`; in non-modal mode `local-changes`, `shelf` and `stash` go to `commit`.
  - `VersionControlWindow` props `{ contents: ContentId[]; selected: ContentId; onselect }`, rendering the tab strip with IntelliJ's texts. Tabs are not closable.
  - `requestTab(tab)` keeps working, with `ContentId` plus `"commit"` instead of `Tab`.

- [ ] **Step 1: Write the failing tests.**
  - `tabs.test.ts`: `tabs are ordered by IntelliJ's weights`, `a Stash tab goes after Shelf and before Log`, `in modal mode Local Changes is in the Git window and there is no Commit window`, `in non-modal mode Local Changes, Shelf and Stash go to the Commit window`.
  - `App.test.ts`:
    - `in modal mode the app opens on Local Changes without loading the log`, which replaces `opens_on_commit_without_loading_the_log`;
    - `cmd 0 does nothing in modal mode`;
    - `cmd 9 shows the Git window with Local Changes, Log and Console in that order`;
    - `switching the setting moves Local Changes between the Commit and Git windows`;
    - `the commit view stays alive when switching tabs`, kept for the non-modal mode.
- [ ] **Step 2: Check that they fail.** Run `tabs.test.ts` and `App.test.ts`. Expected: FAIL.
- [ ] **Step 3: Implement.** `App.svelte` builds the stripe from `toolWindows`. In modal mode the stripe shows a single `Git` button with the `ToolWindowChanges` icon; in non-modal mode it shows `Commit` and `Git`. ⌘9 selects the `Git` window and its first content. ⌘0 only acts in non-modal mode. The spec's "Startup" paragraph becomes: "the app opens on Local Changes (modal) or on the Commit tool window (non-modal), and the log loads only when its tab is opened".
- [ ] **Step 4: Check that they pass.** Run the whole TS suite. Expected: PASS.
- [ ] **Step 5: Commit.** `git commit -m "feat: Git tool window with Local Changes, Log and Console"`

### Task 4: Split `CommitView` and create Local Changes

Reference:
- `VI/openapi/vcs/changes/ChangesViewManager.java:354-369,570-606,640-676,740-806` (tree without checkboxes, vertical toolbar, `Preview Diff` splitter, double click);
- `platform/vcs-impl/resources/META-INF/VcsActions.xml:146-205` (`ChangesViewToolbar` and `ChangesViewPopupMenu`, in their order);
- `VI/openapi/vcs/actions/commit/CommonCheckinFilesAction.kt:30,47-62` (`Commit File…`, `Commit Files…`, `Commit Directory…`, `Commit Directories…`; V:107-108);
- `VA/VcsConfiguration.java:63` (`LOCAL_CHANGES_DETAILS_PREVIEW_SHOWN=true`).

**Files:**
- Create: `src/commit/ChangesPanel.svelte`, `src/commit/LocalChangesView.svelte`, `src/commit/LocalChangesView.test.ts`
- Modify: `src/commit/CommitView.svelte`, `src/commit/CommitView.test.ts` (tests of the shared part move to `LocalChangesView.test.ts` or stay, without duplicating), `src/App.svelte`

**Interfaces:**
- Consumes: `withoutCheck` (`src/commit/tree.ts:66`), `FilePane` with `embedded` (`src/diff/FilePane.svelte:53-67`).
- Produces:
  - `ChangesPanel` props:
    - `{ checkable: boolean; toolbar: "horizontal" | "vertical"; onopen(path); onselection(paths, lists) }`;
    - snippets for the extra toolbar items: `Commit…` in modal mode;
    - the methods that `CommitView` has today for the tree: `refresh()`, `focusTree()`, `selectedChanges()`.
  - `LocalChangesView` props `{ repo: string; oncheckin(request: CheckinRequest) }`, with `CheckinRequest = { kind: "project" | "files"; paths: string[]; lists: string[] }`. Task 5 resolves the inclusion; this view only says what is selected.
  - `CommitView` keeps its props and its behaviour in non-modal mode.

- [ ] **Step 1: Write the failing tests** in `LocalChangesView.test.ts`:
  - `Local Changes shows the changelists without checkboxes`;
  - `the toolbar follows the order of ChangesViewToolbar`: Refresh, Commit…, Rollback…, Show Diff, Changelists, View Options, Expand All, Collapse All, Preview Diff;
  - `the popup follows the order of ChangesViewPopupMenu`, with `Commit File…` first and its text by selection (`Commit Files…`, `Commit Directory…`);
  - `Preview Diff shows the selected file's diff without line checkboxes and is shown by default`;
  - `double click opens the diff` (decision 3);
  - `Commit… with nothing selected asks for the whole list and Commit File… for the selected files`.
- [ ] **Step 2: Check that they fail.** Run `LocalChangesView.test.ts`. Expected: FAIL.
- [ ] **Step 3: Implement.** Move into `ChangesPanel` everything in section 6(a) of the research note: tree state, conflicts, toolbar, menu, changelist dialogs, rollback and snapshots. `CommitView` uses `ChangesPanel` with `checkable` and keeps the inclusion, the message and the commit. `LocalChangesView` uses `ChangesPanel` without checkboxes, with the vertical toolbar and the splitter. `Shelve Silently` and the shelf and patch menu items are left out until part 3, with one comment in the menu definition that says so.
- [ ] **Step 4: Check that they pass.** Run the whole TS suite. `CommitView.test.ts` must pass unchanged, except the tests moved to the new file. Expected: PASS.
- [ ] **Step 5: Commit.** `git commit -m "refactor: share the changes panel and add the Local Changes view"`

### Task 5: The dialog model

Reference:
- `VI/openapi/vcs/actions/commit/CheckinActionUtil.kt:50-80,115-150` (inclusion and initial list for ⌘K and Commit File);
- `VI/vcs/commit/SingleChangeListCommitMessagePolicy.kt:16-74` and `AbstractCommitMessagePolicy.kt:19-27` (message on open, on changing the list, on cancel and after commit);
- `G/checkin/GitCheckinEnvironment.java:115-140` (`MERGE_MSG`, `SQUASH_MSG`);
- `VI/openapi/vcs/changes/ui/CommitChangeListDialog.java:177-193,268,567-598` (executor title, buttons disabled with nothing included);
- `VA/VcsConfiguration.java:146,162-176` (25 messages).

**Files:**
- Create: `src/commit/dialog.ts`, `src/commit/dialog.test.ts`
- Modify: `src-tauri/src/changelists.rs`, `src-tauri/src/messages.rs`, `src-tauri/src/commands.rs`, `src-tauri/src/lib.rs`, `src/lib/api.ts`

**Interfaces:**
- Consumes: `CheckinRequest` from Task 4.
- Produces:
  - Rust:
    - `changelists::set_comment(repo, id: &str, comment: &str) -> Result<(), GitError>` and the command `changelist_set_comment(id, comment)`. It does not write the index or the work tree, so it needs neither `flush_diff_window` nor `RepoLocks`.
    - `messages::MAX_MESSAGE_HISTORY = 25` and `messages::last_commit_message(repo) -> Option<String>`, the previous commit's message, from the history file.
  - TS, in `dialog.ts`:
    - `initialList(request: CheckinRequest, lists: ChangeList[], active: string): string`;
    - `initialInclusion(request: CheckinRequest, list: ChangeList, unversioned: string[]): Set<string>`;
    - `initialMessage(input: { prefilled?: string; list: ChangeList; mergeMessage?: string; lastMessage?: string; clearInitial: boolean }): string`;
    - `onListChanged(draft: string, from: ChangeList, to: ChangeList): { save: { id: string; comment: string }; message: string }`;
    - `onClosed(input: { committed: boolean; draft: string; list: ChangeList; fullyIncluded: boolean }): { id: string; comment: string } | null`;
    - `type Executor = "commit" | "shelve" | "patch"` and `dialogTexts(executor): { title: string; primary: string; dropdown: string[]; options: boolean; unversioned: boolean }`. The three texts are `Commit Changes` / `Commit`, `Shelve Changes` / `Shelve Changes`, and `Create Patch` / `Create Patch…`.
    - `canCommit(included: Set<string>): boolean`.

- [ ] **Step 1: Write the failing tests.**
  - Rust: `set_comment_keeps_name_and_id`, `history_keeps_twenty_five_messages`, `the_last_message_is_kept_apart_from_the_history`.
  - TS (`dialog.test.ts`):
    - `with nothing selected the whole initial list is included`;
    - `with files selected only those are included`;
    - `the initial list is the selected list, else the list of the first selected change, else the active list`;
    - `Commit File picks the active list if it holds one of the files, else the only affected list`;
    - `the message is the list comment, then a non-default name, then MERGE_MSG, then the last commit message`;
    - `Clear initial commit message leaves only the prefilled text`;
    - `changing the list saves the draft as the old list's comment`;
    - `cancel keeps the message as the list comment`;
    - `a full commit of the default list clears its comment`;
    - `the shelve and patch variants have their own title and one button and no options`;
    - `nothing included disables the buttons even with amend`.
- [ ] **Step 2: Check that they fail.** Run `dialog.test.ts` and the Rust filters `changelists::` and `messages::`. Expected: FAIL.
- [ ] **Step 3: Implement.** These are pure functions with no Svelte, plus the Rust command registered in `lib.rs` and `api.ts`.
- [ ] **Step 4: Check that they pass.** Expected: PASS.
- [ ] **Step 5: Commit.** `git commit -m "feat: commit dialog model with IntelliJ's inclusion and message rules"`

### Task 6: The `Commit Changes` dialog

Reference:
- `VI/openapi/vcs/changes/ui/CommitChangeListDialog.java:85-93,257-438,482-544,607-616,719-741,803-877`. It covers the layout, the splitters and their keys, the `Diff` panel (V:1040) with line checkboxes, the buttons, and the size.
- `MultipleLocalChangeListsBrowser.java:97-191,236,252-280,379-419`. It covers `Changelist:` (V:281), the toolbar, the popup and the tree without list nodes.
- `DefaultCommitChangeListDialog.kt:27-46` and `CommitLegendPanel.kt:36-68` (V:292-294). They cover the branch and the legend, which counts included files only and uses a thousands separator.
- `VI/openapi/vcs/ui/CommitMessage.java:121-130,174-177,213,245`. It covers the `Commit Message` separator (V:41) and the history button ⌃M.
- `VI/vcs/commit/CommitOptionsPanel.kt:48-142` and `G/checkin/GitCommitOptions.kt:125-239`:
  - `Git`: `Author:` GB:47, `Amend commit` V:1107-1108, `Sign-off commit` GB:1297-1298;
  - `Commit Checks`: V:43, `Run Git hooks` GB:665-666 only if there are hooks, `G/checkin/GitSkipHooksCommitHandlerFactory.kt:50-73`.
- `AmendCommitHandlerImpl.kt:46-97`. Amend loads the last message only if the text is untouched, and the button text does not change.
- `SingleChangeListCommitWorkflowHandler.kt:59-121`. The empty message shows `Comment Is Empty` / `Please specify comment` (V:36-37). If the checks pass, the dialog closes and then the commit runs.
- `G/checkin/GitCommitAndPushExecutor.kt:27`, DvcsBundle `action.commit.and.push.text`. It covers `Commit and Push…`.

**Files:**
- Create: `src/commit/CommitDialog.svelte`, `src/commit/CommitOptionsPanel.svelte`, `src/commit/ChangelistChooser.svelte`, `src/commit/CommitDialog.test.ts`
- Modify: `src/commit/request.ts` (`legend` with a thousands separator), `src/commit/request.test.ts`, `src/commit/tree.ts` (a single-list tree without list nodes, from `fileNodes` and `group`), `src/commit/tree.test.ts`, `src-tauri/src/commit.rs` + `commands.rs` (`repo_has_commit_hooks() -> bool`: `pre-commit` or `commit-msg` in `git rev-parse --git-path hooks`), `src/App.svelte` (mounting the dialog), `src/lib/api.ts`

**Interfaces:**
- Consumes: everything in Task 5; `WarningsGate.confirm`; `buildCommit`, `listSelections` and `dropLostHunks` from `request.ts`; `prepareCommitPush` (`src/push/model.ts`).
- Produces:
  - `CommitDialog` props `{ request: CheckinRequest; executor: Executor; prefilled?: string; rebaseFrom?: string; onexecute?(changes: ExecutorChanges): Promise<void>; onclose() }`.
  - `ExecutorChanges = { list: string; message: string; paths: string[]; partial: PartialFile[] }`, the same shape the commit request already uses. Part 3 calls it for Shelve and Patch.
  - `openCommitDialog(input)` exported from `App.svelte` through a store in `src/commit/dialogStore.ts`, so that `LocalChangesView`, the log and the branches popup can open it.

- [ ] **Step 1: Write the failing tests** in `CommitDialog.test.ts`:
  - `opens titled Commit Changes with the focus in the message`;
  - `the changelist chooser lists every list with the active one in bold and is disabled with one list`;
  - `the tree has no changelist nodes, has checkboxes per file and shows Unversioned Files`;
  - `the legend counts included files only, with a thousands separator`;
  - `Run Git hooks is shown only when the repository has hooks`;
  - `Amend commit loads the last message only if the text was not edited and the button stays Commit`;
  - `an empty message shows Comment Is Empty and keeps the dialog open`;
  - `a cancelled warning keeps the dialog open`;
  - `passing the checks closes the dialog and then commits`;
  - `Commit and Push… commits then opens the push`;
  - `the Diff panel shows line checkboxes and toggling them changes what is committed`;
  - `the Diff panel, the size and the splitters are remembered`;
  - `a file that disappears while the dialog is open leaves the inclusion and a new one comes in unchecked` (Review Focus 1).
- [ ] **Step 2: Check that they fail.** Run `CommitDialog.test.ts`. Expected: FAIL.
- [ ] **Step 3: Implement.**
  - Layout from top to bottom: `Changelist:`; the toolbar; the tree; the branch and the legend; the splitter; `Commit Message` with ⌃M; the options on the right, 150 to 400 px wide; the collapsible `Diff` panel.
  - Keys: ⌘⏎ Commit, ⌥⌘K Commit and Push, ⌃⌥M amend, ⌃M history, Esc Cancel.
  - The commit runs after the dialog closes. Progress goes to `TaskStatus`, and an error goes to a notification. Selections that are no longer valid are reported as they are today.
  - `OptionsDialog.svelte` stays only in the non-modal view.
- [ ] **Step 4: Check that they pass.** Run the whole TS suite and `cargo test --lib commit::`. Expected: PASS.
- [ ] **Step 5: Commit.** `git commit -m "feat: Commit Changes dialog"`

### Task 7: Who owns the line checkboxes in the diff window

Reference:
- `CommitChangeListDialog.java:808` (`ALLOW_EXCLUDE_FROM_COMMIT` in the dialog);
- `ChangesViewManager.java:731-738` (the diff of Local Changes has no line checkboxes in modal mode).

**Files:**
- Modify: `src/App.svelte` (`publishSelection`, `selectionLost`, `onSelectionChanged`), `src/diff/DiffWindow.svelte`, `src/diff/window.ts`, `src/diff/DiffWindow.test.ts`, `src/lib/api.ts`

**Interfaces:**
- Consumes: `CommitDialog` from Task 6, `CommitView`.
- Produces: the selection state the window receives carries `owner: "none" | "commit-view" | "dialog"`. With `none` the window shows no line checkboxes. Line changes go back only to the owner.

- [ ] **Step 1: Write the failing tests** in `DiffWindow.test.ts`:
  - `without a commit session the diff shows no line checkboxes`;
  - `line checkboxes in the window change the inclusion of the open dialog`;
  - `closing the dialog clears the window's checkboxes`;
  - `in non-modal mode the commit view still owns the line checkboxes` (Review Focus 3).
- [ ] **Step 2: Check that they fail.** Expected: FAIL.
- [ ] **Step 3: Implement.** `App.svelte` decides the owner: the dialog while it is open; otherwise the non-modal view if the mode is non-modal; otherwise nobody.
- [ ] **Step 4: Check that they pass.** Run the whole TS suite. Expected: PASS.
- [ ] **Step 5: Commit.** `git commit -m "feat: the diff window follows the owner of the commit inclusion"`

### Task 8: Entry points through the dialog

Reference:
- `G/rebase/GitAutoSquashCommitAction.kt:20-53` (Fixup and Squash with the active list, `Nothing to Commit` / `No changes detected` V:275-276, `Commit and Rebase…` GB:44);
- `CheckinActionUtil.kt:94-111` (the executors open the modal dialog);
- `plugin.xml:95-98` (⌥⌘K).

**Files:**
- Modify: `src/log/editing.ts`, `src/log/editing.test.ts`, `src/App.svelte`, `src/branches/BranchDialogs.svelte` and `src/branches/actions.ts` (the `Commit…` entries that switch to the Commit view today), `src/App.test.ts`

**Interfaces:**
- Consumes: `openCommitDialog` from Task 6, `commitMode` from Task 2.
- Produces: one routing function, `requestCommit(input: { request: CheckinRequest; prefilled?: string; rebaseFrom?: string })` in `src/commit/dialogStore.ts`. In modal mode it opens the dialog. In non-modal mode it does what `commitView.checkin` and `commitView.propose` do today.

- [ ] **Step 1: Write the failing tests.**
  - `editing.test.ts`: `Fixup opens the commit dialog with fixup! subject and Commit and Rebase…`; `without changes in the active list Fixup says Nothing to Commit`.
  - `App.test.ts`: `cmd K opens the dialog from Local Changes and from the Log in modal mode`; `in non-modal mode cmd K still focuses the Commit tool window message`; `cmd alt K in modal mode opens the dialog`.
- [ ] **Step 2: Check that they fail.** Expected: FAIL.
- [ ] **Step 3: Implement.** Every `commitView.checkin()` / `propose()` call goes through `requestCommit`. ⌥⌘K outside the dialog opens the dialog, because in IntelliJ that action only works with a dialog open.
- [ ] **Step 4: Check that they pass.** Run the whole TS suite. Expected: PASS.
- [ ] **Step 5: Commit.** `git commit -m "feat: commit entry points open the dialog in modal mode"`

### Task 9: The 241 commit checks: large files and file names

Reference:
- `G/config/GitVcsOptions.kt:62-66` (`WARN_ABOUT_LARGE_FILES=true`, limit 50 MB, `WARN_ABOUT_BAD_FILE_NAMES=true`);
- `G/checkin/GitCheckinHandlerFactory.kt:62,74,159-190,437-500`:
  - the texts of `You are about to commit a large file…` and `File Name Is Not Allowed on Windows`;
  - the buttons;
  - the reserved Windows names and characters;
  - `lfs.repositoryformatversion` turning off the size check;
- `G/config/GitVcsPanel.kt` (the rows `Warn when committing files larger than [50] MB` and `Warn when committing files with names that might cause issues on other systems`).

**Files:**
- Modify: `src-tauri/src/commit.rs` (`CommitWarning::LargeFiles(Vec<(String, u64)>)` and `CommitWarning::BadFileNames(Vec<String>)`, in IntelliJ's handler order: user name, CRLF, large file, detached HEAD and rebase, file name), `src-tauri/src/settings.rs` (`warn_large_files: bool` true, `large_files_limit_mb: u32` 50, `warn_bad_file_names: bool` true), `src/commit/WarningsGate.svelte`, `src/settings/SettingsView.svelte`, the matching tests

**Interfaces:**
- Consumes: the commit request of `commit.rs`.
- Produces: both warnings in `commit_warnings`, used by both the dialog and the non-modal view.

- [ ] **Step 1: Write the failing tests.**
  - Rust:
    - `a_file_over_the_limit_warns_with_its_size`;
    - `lfs_turns_off_the_size_check`;
    - `an_added_file_named_aux_warns`;
    - `a_renamed_file_with_a_colon_warns`;
    - `warnings_come_in_intellijs_order`.
  - TS:
    - `the large file warning shows over the dialog and Cancel keeps it open` (Review Focus 5);
    - `the two new rows are on the Git settings page and are saved`.
- [ ] **Step 2: Check that they fail.** Expected: FAIL.
- [ ] **Step 3: Implement.** Measure the size of the included files on disk. For a partial file, measure what goes into the index. Check the names only for added and renamed paths, as IntelliJ does.
- [ ] **Step 4: Check that they pass.** Run the whole TS suite and `cargo test --lib commit::`. Expected: PASS.
- [ ] **Step 5: Commit.** `git commit -m "feat: large file and file name checks before the commit"`

### Task 10: Matrix, report and full verification

**Files:**
- Modify: `docs/parity-matrix.md`. These rows get their result:
  - section 2: the Commit window rows;
  - section 3: `Enable staging area` and `Use non-modal commit interface`;
  - section 4: `ActivateCommitToolWindow`, `ActivateVersionControlToolWindow`, `ChangesView.ToggleCommitUi`, `CheckinProject`, `CheckinFiles`, `ChangesView.ShowCommitOptions`, `Vcs.ToggleAmendCommitMode`, `Vcs.Show.Local.Changes`, `Git.Commit.And.Push.Executor`, `Git.Show.Stage`.
- Create: `docs/reports/phase-4-validation.md`, with a section for part 1. It lists what was proven with tests and the manual checklist: the dialog in a real window, ⌘K with a real keyboard, the Diff panel with line checkboxes, and switching the mode.

- [ ] **Step 1:** Each `done` cites its test, as in the previous phases.
- [ ] **Step 2: Full verification, one suite at a time.**
  - `pnpm exec tsc --noEmit -p .`
  - `pnpm exec vitest run --maxWorkers=2`
  - `cargo test --lib -- --test-threads=4`
  - `cargo clippy --all-targets -- -D warnings`
  - `python3 -I -m unittest discover -s scripts`

  Expected: all of them pass. The report writes the exact counts.
- [ ] **Step 3: Commit.** `git commit -m "docs: phase 4 part 1 results in the matrix and the report"`
