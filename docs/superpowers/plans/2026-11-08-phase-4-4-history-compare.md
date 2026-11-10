# Phase 4, part 4: history and comparisons. Implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** the user looks back at their files as in IntelliJ 2024.1:
- a `Project` file tree (⌘1) and a read-only viewer, also for files opened from outside the app;
- file history (187 uses a year), `Show Repository at Revision` (17), `Copy Revision Number` (134), `Compare with Local`, `Get`;
- the comparisons of the Git submenu, the log, the branch popup and the log's changes panel;
- `Not Cherry-Picked Commits`, `Select in Tree…`, the merge window's `Show Details`;
- `Annotate` and `Show History for Selection…`, last, because the user never used them.

**Architecture:**
- Rust reads every revision and runs every git command. New modules:
  - `viewer.rs`: file content at a revision or on disk, with IntelliJ's size and binary rules;
  - `tree.rs`: the file tree and `git ls-tree` for a revision;
  - `compare.rs`: the revisions and change lists that comparisons need;
  - `get_version.rs`: `Get`, with a recoverable copy first;
  - `history.rs`: file history through `git log` and the rename walk of `GitFileHistory`, without an index;
  - `annotate.rs`, `block_history.rs` and `log/cherry.rs`.
- The diff window gets a second kind of request, two revisions, besides today's working-tree request. The Local side keeps the save rules of phase 1.
- New tool windows on the left stripe: `Project`, `Changes` (one tab at a time) and `Repositories`. File history and Compare Branches are new tab kinds of the Log.
- The viewer is one separate window with a tab per file (decision 2).

**Tech Stack:** Tauri 2.12, Rust 1.99.1, Svelte 5, Vitest 5, CodeMirror 6. No new dependency.

**Spec:** `docs/superpowers/specs/2026-10-31-intellij-git-client-design.md` and the phase 4 overview `docs/superpowers/plans/2026-11-03-phase-4-overview.md`. The overview's Global Constraints and Decisions are part of this plan, especially 3, 12, 21, 22 and 23.

Research (not in the repository), which the implementer reads before each task:
- `.superpowers/phase-4/research-history-compare.md`: entry points, git commands, texts, menu orders (the first pass);
- `.superpowers/phase-4/research-part-4-241.md`: the gaps the first pass left (layouts, titles, colours, shortcuts, the user's settings), with its corrections in §0;
- `.superpowers/phase-4/research-part-4-ramify.md`: what Ramify has today, with lines.

Each task names the sections it implements as `H§n` (first pass), `G§n` (241 gaps) and `R§n` (Ramify).

**Reference:** tag `idea/241.14494.240`. Bundles: `B:` GitBundle, `V:` VcsBundle, `A:` ActionsBundle, `L:` VcsLogBundle, `D:` DvcsBundle, `DB:` DiffBundle, `IB:`/`EB:` IdeBundle/EditorBundle (installed jar). Copy literal texts from the cited line, not from this plan.

**Branch:** `phase-4`, after part 3 (`944d203`).

**Commands** (keep the machine load low; one suite at a time):
- TS: `PATH=$HOME/.cache/ramify-toolchain/node-v22.23.4-darwin-arm64/bin:$PATH pnpm exec vitest run --maxWorkers=2 <files>` and `pnpm exec tsc --noEmit -p .`
- Rust: `PATH=$HOME/.cargo/bin:$PATH CARGO_BUILD_JOBS=4 cargo test --manifest-path src-tauri/Cargo.toml --lib <filter> -- --test-threads=4`
- Python: `python3 -I -m unittest discover -s scripts`

## Global Constraints

The overview's Global Constraints, which include phase 3's. The ones this part leans on most:
- `Get` and every save from a comparison never overwrite a file whose content changed since it was read. `Get` saves a recoverable copy first (`snapshots::create`). Saves keep phase 1's hash check.
- Every new command that changes files or the index calls `flush_diff_window` first, takes `RepoLocks`, and updates the counts of `mutating_without_saving_the_diff_is_not_allowed` and `whatever_touches_the_repository_saves_the_diff_window_first`.
- Real `git` with arguments, `--` before paths, `--literal-pathspecs` on every path-taking call, no shell. Every `<rev>:<path>`, `ls-tree`, `blame` and `log -- <path>` goes through `git::exact`, so `core.precomposeunicode` cannot rename a path (part 3's declined item).
- Paths, file contents, commit messages, author names and branch names are untrusted text: shown as text, no `{@html}`. Show Current Revision's HTML popup and the annotation tooltip are rebuilt as plain elements.
- A long history, blame or selection history is a cancellable task that never blocks the window.
- Tests use throwaway repositories, never the network, `.idea/` of a real project, or `~/Library/Application Support/JetBrains`.
- Commits end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Decisions of this part

They add to the overview's decisions. The user approves them with this plan. Each has its cost if wrong.

1. **Opening a file from outside starts its own Ramify process, as opening a repository does today.** `Ramify <file>` and `Ramify <file>:<line>` find the repository with `git rev-parse --show-toplevel` from the file's folder, open it, and show the file in the viewer at that line. Not copied: reaching a Ramify that is already running (that needs `tauri-plugin-single-instance`, a new dependency) and Finder's "Open With". Cost: opening a file of a repository that is already open gives a second window on it.
2. **The viewer is one separate window with a tab per file**, because Ramify has no editor area. The user's IntelliJ opens diffs in a separate window too (`openInNewWindow=true`, G§13). The tabs wrap in rows (`SCROLL_TAB_LAYOUT_IN_EDITOR=false`). It is read-only, as the spec says. Editing stays in the diff window. Cost: files do not open beside the tree.
3. **The Project tree reads the folders from disk on expand**, as IntelliJ's file system does, and colours each entry from `git status --ignored`. It shows ignored files in their colour and hides IntelliJ's default ignore patterns (G§1). Cost: a folder with tens of thousands of entries is listed whole when it is opened.
4. **History tabs are a tab kind of the Log, and are remembered per repository**, as 241 reopens them when the log is recreated (H§1). Compare Branches is also a Log tab kind (two stacked logs), not an editor tab. Its filters live in memory only, as in 241 (G§4). Cost: none.
5. **`Changes` and `Repositories` are tool windows on the left stripe, registered on first use** (overview decision 12). `Changes` holds one tab, and a new one replaces it (G§0.3). `Repositories` has one tab per repository, and since Ramify has one repository per window, a new revision replaces it. Cost: none.
6. **`Get` writes only the files.** It does not run `git add` or `git rm`: IntelliJ asks about adding by default, and Ramify leaves a new file unversioned for the user to add. It writes to the path the file had in that revision, as IntelliJ does. Cost: one `Add` (⌥⌘A) after getting a deleted file back.
7. **Annotate and selection history run in the viewer**, which is where Ramify has a gutter and a selection. The annotation options are IntelliJ's defaults, since the user has none set (G§13): `Last Name`, colours by `Order`, `Date` and `Author` shown. Cost: no annotation inside the diff window.
8. **Not copied:**
   - `Show Diff On Hover` in the annotation `View` menu;
   - `Show Diff in a New Tab or Window`, which does what `Show Diff` does, since Ramify has no editor tabs (as part 3's Shelf tab);
   - `Choose Parent to Compare` in file history: 241 shows a merge row as a three-side diff (G§0.6), and Ramify does the same;
   - F4 inside the file history table: 241 has no defined flow there (G§2);
   - the `Git.Stage.Compare.*` and `Show Staged Version` rows: they exist only in staging mode, which is not the user's mode (H§0.1). They are marked "not in the user's mode".

   The rows that only Find Action reaches stay without an entry point (overview decision 3). Cost: none for this user.

## Review Focus

1. **Devin writes a file while `Get` runs, or while the Local side of a comparison is saved.** The copy is taken first. A file whose content changed after it was read is left as it is and reported. Tests in Tasks 4 and 5.
2. **A long history with renames and merges**: more than 1,000 commits, a rename on a side branch, a file deleted and re-added. The rows equal 241's algorithm: full history, every `A` commit as a candidate, a rename restarting from the parent with the old path, at most 1,000 per walk. The window answers while it loads, and closing the tab cancels it. Tests in Task 7.
3. **Paths that git could read another way**:
   - names with `*`, `?`, `[`, `:(glob)` or a leading `-`;
   - spaces;
   - the decomposed (NFD) form of `é`;
   - a file inside an ignored folder.

   The tree, history, comparisons, `Get`, blame and `Show Current Revision` all reach that exact file and no other. Tests in Tasks 3, 4, 7 and 11.
4. **Large and binary files** in the viewer, comparisons and Annotate:
   - a 30 MB text file shows the read-only preview with 241's text;
   - a PNG says binary;
   - Annotate is disabled on a binary file.

   Nothing freezes. Tests in Tasks 2, 4 and 11.
5. **The repository changes while a history tab, a comparison or an annotation is open, or while it loads** (checkout, rebase, a commit from the terminal):
   - history and Compare Branches tabs refresh with the log;
   - an annotation of the local file is recomputed;
   - `Get` refuses before any write when HEAD changed since its list was read (`expected_head`).

   Tests in Tasks 5, 7, 8 and 11.

---

### Task 1: Groundwork: tool windows, log tab kinds and carried fixes

Research: R§2 (tabs), R§4 (tool windows), R§8 (warnings and carried items). This task changes no visible behaviour except the carried fixes.

**Files:**
- Create:
  - `src/shell/toolWindows.ts`: the registry of tool windows (`id`, label, icon, shortcut, stripe side, order, `registeredOnFirstUse`) that `App.svelte` reads instead of `WINDOWS` and the two-key records;
  - `src/log/tabKinds.ts`: `LogTabKind = "log" | "update" | "history" | "compare"`, with the title, persistence and reuse rule of each.
- Modify:
  - `src/lib/tabs.ts`: `ToolWindowId` becomes the registry's union (`commit | git | project | changes | repositories`), with `toolWindows`, `keepSelected` and `stripeWindows` typed over it;
  - `src/App.svelte`: the stripe, `activateWindow` and the ⌘N keys come from the registry; it gets smaller;
  - `src/log/LogPanel.svelte`: `LogTab.kind` replaces the `update?` field, and `openUpdateInfo` goes through `tabKinds.ts`;
  - the Svelte warnings of R§8: `src/commit/CommitMessage.svelte:57` (`historyButton` as `$state`), `src/update/PullDialog.svelte:31`, `src/remotes/DefineRemoteDialog.svelte:19-21`, `src/remotes/InitDialog.svelte:12` (initial value read once, on purpose: `untrack`);
  - part 3's open items:
    - N1, `src/patch/ApplyPatchDialog.svelte:136-137`: a re-plan keeps the row of a binary with Overwrite ticked;
    - N2, `src/App.svelte` `loadShelf`: a failed Shelf read is retried on the next `repo-changed`, and the stale comment goes;
    - N3, `src-tauri/src/shelf/tab.rs`: `Restore` of a list from `shelf-erased/` clears its deleted mark and sets its date to now, so the weekly cleanup does not erase it again; `docs/reports/phase-4-validation.md:463` updated.
- Test: `src/lib/tabs.test.ts`, `src/App.test.ts`, `src/log/LogPanel.test.ts`, `src/patch/ApplyPatchDialog.test.ts`, `src-tauri/src/shelf/tab.rs`.

**Interfaces:**
- Produces: `registerToolWindow(id)`, `activateWindow(id)`, `TOOL_WINDOWS` in `src/shell/toolWindows.ts`; `LogTabKind`, `openLogTab(kind, key, init)` (find-or-create by `key`) in `src/log/tabKinds.ts`.

- [ ] **Step 1: Write the failing tests:**
  - `the_stripe_shows_registered_windows_in_registry_order`: `project` (⌘1) before `commit`, as the user's `window.layouts.xml` (G§13). `changes` and `repositories` appear only after `registerToolWindow`.
  - `meta_1_activates_the_project_window_and_hides_it_when_active` (G§1).
  - `update_info_tabs_keep_their_find_or_create_rule` (the existing behaviour, through `openLogTab`).
  - `a_replanned_binary_with_overwrite_stays_ticked` (N1).
  - `a_failed_shelf_read_is_retried_on_the_next_change` (N2).
  - `a_list_restored_from_erased_survives_the_next_cleanup` (N3).
  - The dev build prints none of the four Svelte warnings: check with `pnpm exec svelte-check` or the vite log. Write down which one you used.
- [ ] **Step 2: Check that they fail.**
- [ ] **Step 3: Implement.** The Project window shows an empty placeholder until Task 3.
- [ ] **Step 4: Check that they pass**, and the existing `App`, `LogPanel` and `tabs` tests stay green.
- [ ] **Step 5: Commit.** `git commit -m "refactor: a tool-window registry and log tab kinds; part 3 carried fixes"`

### Task 2: Revision content and the read-only viewer

Research: G§2, H§5 (Show File at Revision), H§7 (viewer), R§3 (`diff::read_side`), R§4 (argv). Decisions 1 and 2.

**Files:**
- Create:
  - `src-tauri/src/viewer.rs`:
    - `read(repo, target) -> ViewerContent`, which reads the disk or `git cat-file blob <rev>:<path>` through `git::exact`;
    - the limits: content load 20,000 KB; above it, a preview of the first 2.5 MB with `IB:1970`; binary detection as `DiffContentFactoryImpl` (G§2);
    - `viewer_open`, `viewer_content` and `initial_target` commands;
  - `src/viewer/ViewerWindow.svelte`: the tab strip (wrapping rows, ⌘W closes, a padlock with tooltip `read-only` `IB:2367`) and the CodeMirror read-only editor (`languageFor`, soft wraps from a new `DiffPlace` `Viewer`, default on) with a line selection;
  - `src/viewer/window.ts`: `openInViewer(target)`.
- Modify:
  - `src-tauri/src/commands.rs`: the `viewer` window, like `diff`;
  - `src-tauri/src/lib.rs`: the commands;
  - `src/main.ts`: the `viewer` page;
  - `src/App.svelte`: `start()` uses `initial_target`, so a file argument opens the viewer at its line;
  - `src/log/ChangesPanel.svelte`:
    - `Edit Source` (F4, ⌘↓) opens the local file (G§0.7);
    - `Open Repository Version` opens the after-revision; it is disabled when every selected change is a deletion or a folder;
  - `src/lib/api.ts`: the new commands.
- Test: `src-tauri/src/viewer.rs`, `src/viewer/ViewerWindow.test.ts`, `src/log/ChangesPanel.test.ts`, `src/App.test.ts`.

**Interfaces:**
- Produces:
  - `ViewerTarget { path: String, revision: Option<String>, line: Option<u32> }`;
  - `ViewerContent = Text { text, preview_of: Option<u64> } | Binary { size } | Absent`;
  - the tab title `<name>` for a local file, `<name> (<8-char hash>)` for a revision (G§2);
  - `InitialTarget { repo, file: Option<String>, line: Option<u32> }`;
  - TS: `openInViewer(target)` and `viewerSelection(): { path, revision, startLine, endLine } | null`, used by Tasks 6, 11 and 12.

- [ ] **Step 1: Write the failing tests:**
  - `a_revision_is_read_at_its_own_path_through_exact`, with an NFD name.
  - `thirty_megabytes_give_a_preview_with_the_241_text` and `a_png_is_binary`.
  - `a_missing_revision_says_could_not_load_content` (`V:98-99`).
  - `a_file_argument_opens_its_repository_and_the_viewer_at_the_line`. Also `a_folder_argument_still_opens_the_repository` (the old shape), and `a_file_outside_a_repository_says_so` (Ramify's own text, recorded in the report).
  - `tabs_reuse_the_same_path_and_revision_and_wrap`.
  - `edit_source_opens_the_local_file_and_open_repository_version_the_revision`.
- [ ] **Step 2: Check that they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Check that they pass.**
- [ ] **Step 5: Commit.** `git commit -m "feat: a read-only viewer for files and revisions, and opening a file from outside"`

### Task 3: The Project tool window

Research: G§1, H§7, R§6 (`statusColor`, `speedSearch.ts`). Decision 3.

**Files:**
- Create:
  - `src-tauri/src/tree.rs`: `list_dir(repo, dir)`, which reads the entries of one folder with `read_dir`, hides the default ignore patterns, and lists folders first in natural, case-insensitive name order (G§1). Command `tree_list`.
  - `src/tree/ProjectTree.svelte` and `src/tree/model.ts`, a lazy tree:
    - entries load on expand and colour from `repoStatus(true)`, with ignored files and the dark New UI colours of G§1;
    - speed search;
    - double click, Enter, F4 and ⌘↓ open a file in the viewer; on a folder, double click and Enter expand or collapse it;
    - ⌘− and ⌘= collapse and expand all;
    - it refreshes on `repo-changed`;
    - the context menu is `Git ▸` from Task 4's `gitFileMenu`, with an empty placeholder until then.
- Modify: `src/App.svelte` (the `project` pane), `src-tauri/src/lib.rs`, `src/lib/api.ts`.
- Test: `src-tauri/src/tree.rs`, `src/tree/model.test.ts`, `src/tree/ProjectTree.test.ts`.

**Interfaces:**
- Consumes: `openInViewer` (Task 2); `TOOL_WINDOWS` (Task 1).
- Produces: `TreeEntry { name, path, dir }`; `treeSelection(): string[]` (repository-relative paths), which Task 6's `Git > Current File` reads.

- [ ] **Step 1: Write the failing tests:**
  - `folders_first_then_natural_case_insensitive` (`a2` before `a10`, `B` beside `b`).
  - `ds_store_and_dot_git_are_hidden_but_ignored_files_are_shown_coloured`.
  - `a_name_with_glob_characters_and_a_leading_dash_lists_and_opens` (Review Focus 3).
  - `double_click_opens_a_file_and_toggles_a_folder`.
  - `a_change_on_disk_recolours_and_relists_open_folders`.
- [ ] **Step 2: Check that they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Check that they pass.**
- [ ] **Step 5: Commit.** `git commit -m "feat: the Project tool window"`

### Task 4: Two-revision diff requests and file comparisons

Research: H§4 (table), G§5 (titles), G§0.5, G§0.9, R§3. Overview decision 21.

**Files:**
- Create:
  - `src-tauri/src/compare.rs`:
    - `head_revision_of(repo, path)` (`git log -n1 HEAD -- <path>`, H§4);
    - `file_revisions(repo, path)`, which Task 7 replaces with the history walk;
    - `changes_with(repo, rev, paths)` (`git diff --name-status --diff-filter=ADCMRUXT -M <rev> -- <paths>`);
    - `changes_between(repo, a, b, paths)`;
  - `src/compare/popups.ts`: the popups `Select Branch to Compare` (`D:101`, speed search), `Select Tag to Compare` (`B:1041-1044`) and `File Revisions` (`V:114-115`).
- Modify:
  - `src-tauri/src/commands.rs`: `diff_window_open_request(request)` beside `diff_window_open(path)`;
  - `src/diff/DiffWindow.svelte`: a request union `worktree | revisions`. A `revisions` request has its own file list and titles (G§5: full hashes, `Local`, `Local (<hash>)`, `<branch>` / `<current> (Local)`, the window title rules of G§3). It skips status, staging and the selection plumbing. The Local side stays editable with `FilePane`'s save rules.
  - `src/diff/window.ts`, `src/lib/api.ts`.
- Test: `src-tauri/src/compare.rs`, `src/diff/DiffWindow.test.ts`, `src/compare/popups.test.ts`.

**Interfaces:**
- Produces:
  - `DiffRequest = { kind: "worktree", path } | { kind: "revisions", files: RevisionPair[], title }`;
  - `RevisionPair { left: SideRef, right: SideRef }`, with `SideRef { revision: string | "local" | null, path, title }`;
  - `compareSameVersion(path)`, `compareWithRevision(path, hash)`, `compareWithBranch(path, branch)`, `compareWithLocal(hash, paths)` and `compareRevisions(a, b, paths)` in `src/compare/actions.ts`;
  - for one file they open the diff window, for folders or several files Task 5's Changes window.

- [ ] **Step 1: Write the failing tests:**
  - `same_version_titles_are_full_hash_and_local_and_swap_when_the_target_is_newer` (G§0.9).
  - `compare_with_branch_titles_are_branch_and_current_local`, and with a detached HEAD the short hash.
  - `a_revisions_request_lists_only_its_files_and_never_reads_status`.
  - `the_local_side_saves_with_the_hash_check_and_refuses_a_changed_disk` (Review Focus 1).
  - `a_renamed_file_titles_old_arrow_new`.
  - `popups_list_branches_and_tags_with_speed_search`, and `a_missing_branch_says_d_102`.
  - `a_large_or_binary_revision_shows_the_241_texts` (`DB:273`, `DB:42`, `DB:46-47`; Review Focus 4).
  - `compare_reaches_a_glob_named_and_nfd_file` (Review Focus 3).
- [ ] **Step 2: Check that they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Check that they pass.**
- [ ] **Step 5: Commit.** `git commit -m "feat: two-revision diff requests and file comparisons"`

### Task 5: The Changes tool window and `Get`

Research: G§4 (`CompareWithLocalDialog`, `Changes`), G§6 (Get), H§5 (Get), R§6 (snapshots, locks). Decisions 5 and 6.

**Files:**
- Create:
  - `src-tauri/src/get_version.rs`, `revision_get(items, expected_head)`:
    - `flush_diff_window`, `RepoLocks`, `check_path`;
    - one `snapshots::create` of the local files it will touch;
    - then, per file, delete, create, move back, or overwrite, as G§6 step 4;
    - a file whose content changed since the list was read is left and reported;
    - commands added to both count tests.
  - `src/changes/ChangesToolWindow.svelte`, one tab:
    - the title, for example `Changes Between {0} and local version` (`B:1061-1063`), `Paths Affected by Commit {0}` (`L:117`) or `Difference Between {0} and {1} Versions in {2}` (`V:1115`);
    - the toolbar: `Refresh` (⌘R, only with a local side), `Show Diff` (⌘D), `Get` (only with a local side), separator, `Group By ▸`;
    - the popup: `Show Diff`, `Create Patch...`, `Get`;
    - the tree grouped by directory, no checkboxes;
    - double click and Enter run `Show Diff` in the diff window;
    - an optional commit message under the tree (Show All Affected Files).
  - `src/changes/get.ts`: the `Get Revision` confirmation (`V:57-60,172`), the progress `Loading Content` (`V:549`) and the errors (`V:176-178`).
- Modify: `src/App.svelte` (the `changes` window registered on first use), `src-tauri/src/lib.rs`, `src/lib/api.ts`.
- Test: `src-tauri/src/get_version.rs`, `src/changes/ChangesToolWindow.test.ts`, `src/changes/get.test.ts`.

**Interfaces:**
- Consumes: `compareWithLocal`, `compareRevisions` (Task 4).
- Produces:
  - `showChanges({ title, left, right, files, message? })`, which replaces the open tab (G§0.3);
  - `getRevision(items: { path, revision, oldPath? }[])`, used by Tasks 7 and 8.

- [ ] **Step 1: Write the failing tests:**
  - `get_over_a_modified_file_asks_overwrite_modified_file_and_cancel_stops`.
  - `get_takes_the_copy_first_and_leaves_a_file_devin_changed_meanwhile` (Review Focus 1).
  - `get_of_a_deleted_revision_deletes_and_of_a_renamed_one_writes_the_old_path`.
  - `get_refuses_before_writing_when_head_moved` (Review Focus 5).
  - `get_runs_no_git_add_or_rm` (decision 6).
  - `a_new_changes_tab_replaces_the_open_one`.
  - `refresh_and_get_appear_only_with_a_local_side`.
  - The command count tests.
- [ ] **Step 2: Check that they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Check that they pass.**
- [ ] **Step 5: Commit.** `git commit -m "feat: the Changes tool window and Get, with a recoverable copy"`

### Task 6: The Git submenu, `Git > Current File`, Show Current Revision and Copy Revision Number

Research: G§1 (the selection-aware table), H§7, G§6, R§4 (the native menu and its routing).

**Files:**
- Create: `src/files/gitFileMenu.ts`, the order of `Git.ContextMenu` (`X:265-275`) without `Rollback File...` (G§0.1):
  - `Commit File…`, `Add`, separator;
  - `Annotate with Git Blame`, `Compare with the Same Repository Version`, `Compare with Revision...`, `Compare with Branch…`, `Show History`, `Show History for Selection…` (viewer only), `Show Current Revision`, `Rollback...`;
  - separator, `Resolve Conflicts…`;
  - the texts and enabled state for a file, several files and a folder, from G§1's table.

  It is used by the tree, the viewer's editor menu and the main menu. `Show History` stays disabled until Task 7, and `Annotate` and `Show History for Selection…` until Tasks 11 and 12; each of those tasks enables its item.
- Modify:
  - `src-tauri/src/commands.rs`: `GIT_MENU` gets `Show Git Log` and `Current File ▸` = `Git.FileActions` (`X:100-111`), in `Git.MainMenu`'s order (`X:161-164`): `Reset HEAD…`, separator, `Show Git Log`, `Patch`, `Uncommitted Changes`, `Current File`, separator, `Manage Remotes…`. Its title follows the selection (`V:982`, `B:1324-1325`), and its items are enabled from the tree or viewer selection through `git_menu_enable`.
  - `src/App.svelte`: the runners.
  - `src/files/currentRevision.ts`: the `Show Current Revision` popup. `V:611` is rebuilt as plain text lines: name, `Current version is <hash>.`, `Modified by <author>[ (<committer>)]`, the pretty date and the message. The progress text is `V:949`, and the git call is `log -n1 --all`.
  - `Copy Revision Number`: full hashes, oldest first, joined by a space (G§6). It applies in the log, history, Changes and annotation.
- Test: `src/files/gitFileMenu.test.ts`, `src-tauri/src/commands.rs` (the menu order), `src/files/currentRevision.test.ts`.

**Interfaces:**
- Consumes: `treeSelection` (Task 3), `viewerSelection` (Task 2), Task 4's actions.
- Produces: `gitFileMenu(selection, state): MenuItem[]`.

- [ ] **Step 1: Write the failing tests:**
  - `file_several_and_folder_texts_and_enablement_as_241`.
  - `the_git_menu_follows_the_order_of_git_main_menu`, updated with `Show Git Log` and the `Current File` group.
  - `current_file_title_follows_the_selection`.
  - `show_current_revision_shows_the_last_commit_on_any_branch_as_text`.
  - `copy_revision_number_is_oldest_first_space_separated`.
- [ ] **Step 2: Check that they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Check that they pass.**
- [ ] **Step 5: Commit.** `git commit -m "feat: the Git file menu, Git > Current File and Show Current Revision"`

### Task 7: File history

Research: H§1 (all), G§3, H§0.8-0.9, R§1 (the store is always loaded, `dotted_edges`, `log_row_of`). Decision 4.

**Files:**
- Create:
  - `src-tauri/src/history.rs`:
    - `file_history(repo, path, start, cancel) -> Vec<HistoryRow { hash, path, status }>`, the algorithm of `GitFileHistory` with `fullHistory=true`:
      - `git log --name-status --full-history --simplify-merges --max-count=1000 <starts> -- <path>`;
      - every `A` commit is a candidate for `show -M --follow` (H§1 flags), and merge parents without a record go through `diff -M --diff-filter=R`;
      - each rename restarts from `<parent> -- <old path>`;
      - `visitedCommits` deduplicates;
    - `start` = the Branch filter (default `HEAD`; empty = `HEAD --branches --remotes --tags`) or a revision for `History Up to Here`, which has no Branch filter;
  - `src-tauri/src/log/history_view.rs`: a log view whose rows are those hashes on the permanent graph with `dotted_edges`, plus `history_path(view, row)`;
  - `src/log/HistoryTab.svelte`:
    - the toolbar on top: `Branch` filter, separator, `Refresh` (⌘R), `Show Diff` (⌘D), `Show All Affected Files` (⌃⌘A), separator, `View Options`;
    - the table with the columns `Author, Date, Commit` (G§3: Subject with refs, `Prefer Commit Date` off);
    - details hidden by default, and the diff preview on the right with the `VcsLogView` place and the splitter 0.706 (G§13);
    - the messages: empty `L:150`, error `L:149` with `Refresh`, commit not found `L:165-168`;
  - `src/log/historyMenu.ts`, `Vcs.FileHistory.ContextMenu` (H§1):
    - `Copy Revision Number`, `Create Patch...`;
    - separator, `Get`, `Show File at Revision`, `Compare with Local`, `Show Diff` (`Compare` with two rows), `Show All Affected Files`, `Annotate`, `Show in Git Log`;
    - separator, `Revert Commit`; separator, `New Tag…`.
- Modify:
  - `src/log/LogPanel.svelte` and `tabKinds.ts`: the `history` kind, titled `<file name>` or `<file name> (<short hash>)` and reused by path and hash; remembered per repository;
  - `src-tauri/src/log/commands.rs`: the history view commands, as a task in `TaskKind::Read`;
  - `src/compare/actions.ts`: `showHistory(path)`. On a folder or the root it opens a normal Log tab with the path filter and `HEAD`, with `Show Only Affected Changes` on (H§1);
  - `src/log/ChangesPanel.svelte`: `History Up to Here`.
- Test: `src-tauri/src/history.rs`, `src-tauri/src/log/history_view.rs`, `src/log/HistoryTab.test.ts`, `src/log/historyMenu.test.ts`.

**Interfaces:**
- Consumes: Task 4's diff requests (a merge row opens three sides, as decision 8 says), Task 5's `showChanges` and `getRevision`, Task 2's viewer, `log_row_of` (`Show in Git Log`).
- Produces: `showHistory(path, revision?)`.

- [ ] **Step 1: Write the failing tests:**
  - `history_follows_a_rename_from_the_parent_with_the_old_path`.
  - `a_rename_on_a_side_branch_and_a_delete_and_readd_give_241_rows` (Review Focus 2). Build the fixture repository in the test, and write the expected rows from 241's algorithm by hand in the test.
  - `more_than_1000_commits_stop_at_1000_per_walk_without_a_notification`.
  - `the_branch_filter_defaults_to_head_and_empty_means_all_refs`.
  - `history_up_to_here_has_no_branch_filter`.
  - `closing_the_tab_cancels_the_walk`.
  - `a_merge_row_opens_three_sides`.
  - `a_deleted_row_disables_get_and_show_file_at_revision`.
  - `show_in_git_log_selects_the_commit_in_the_main_log`.
  - `history_tabs_are_reused_and_reopened_after_the_log_reloads`.
  - `a_folder_opens_a_log_tab_with_the_path_filter`.
  - `a_path_with_glob_characters_has_its_own_history` (Review Focus 3).
  - `a_commit_from_the_terminal_refreshes_an_open_history_tab` (Review Focus 5).
  - `show_history_in_the_git_file_menu_is_enabled_now`.
- [ ] **Step 2: Check that they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Check that they pass.**
- [ ] **Step 5: Commit.** `git commit -m "feat: file history as a Log tab, without an index"`

### Task 8: The deferred log, branch and changes-panel items, and Compare Branches

Research: H§6, H§4 (the branch rows), G§4 (two-log compare), R§2, R§5.

**Files:**
- Modify:
  - `src/log/contextMenu.ts`, in `Vcs.Log.ContextMenu` order (H§6):
    - `Copy Revision Number`, `Create Patch...`, separator;
    - [Cherry-Pick], `Checkout`, `Show Repository at Revision`;
    - `Compare Versions`, visible only with two commits;
    - `Compare with Local`, enabled with one.

    The test `menu_in_intellij_order_without_phase_4_items` becomes `menu_in_intellij_order`.
  - `src/branches/model.ts`: `Compare with '<current>'` (off for the current branch) and `Show Diff with Working Tree`, after `Checkout and Rebase onto…` (H§6). The tests drop "without_phase_4_items".
  - `src/branches/BranchesPanel.svelte`: the `Compare Branches` toolbar button, enabled with exactly two selected (`B:605-609`).
  - `src/log/ChangesPanel.svelte`, `Vcs.RepositoryChangesBrowserMenu` (`VA:330-340`):
    - `Compare with Local`, `Compare Before with Local`, `Edit Source`, `Open Repository Version`;
    - `Revert Selected Changes`, `Cherry-Pick Selected Changes`, `Create Patch...`, `Get`, `History Up to Here`;
    - the toolbar `Revert Selected Changes` and `History Up to Here`.
- Create:
  - `src/log/CompareBranchesTab.svelte`, the `compare` tab kind:
    - two stacked log views, `current..branch` above and reversed below, each with the filter bar minus Branch;
    - the banner `B:1119` with bold names, empty `B:1118`, title `Compare: <branch> and <current>` (`B:1120-1121`), no swap;
  - `src/changes/` integration for `Show Diff with Working Tree`: `showChanges` titled `Changes Between {0} and Current Working Tree` (`B:1301`), `No differences`, error `B:1303`, from `git diff -R --name-status --diff-filter=ADCMRUXT -M <branch>`.
- Test: `src/log/contextMenu.test.ts`, `src/branches/model.test.ts`, `src/branches/BranchesPanel.test.ts`, `src/log/ChangesPanel.test.ts`, `src/log/CompareBranchesTab.test.ts`.

**Interfaces:**
- Consumes: Tasks 4, 5 and 7, and Task 9's `showRepositoryAtRevision`. Build the item with a no-op until Task 9 lands, and wire it there.

- [ ] **Step 1: Write the failing tests:**
  - `menu_in_intellij_order`, with one and with two commits.
  - `branch_submenu_in_241_order_and_compare_off_for_current`.
  - `compare_branches_needs_two_selected`.
  - `changes_panel_menu_in_241_order`.
  - `compare_tab_shows_both_ranges_and_the_empty_text`.
  - `compare_tab_refreshes_with_the_log_after_a_checkout` (Review Focus 5).
  - `show_diff_with_working_tree_fills_the_changes_window`.
- [ ] **Step 2: Check that they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Check that they pass.**
- [ ] **Step 5: Commit.** `git commit -m "feat: the deferred log, branch and changes-panel comparisons, and Compare Branches"`

### Task 9: Show Repository at Revision

Research: H§5, G§4 (`Repositories`). Decision 5.

**Files:**
- Modify:
  - `src-tauri/src/tree.rs`: `ls_tree(repo, rev, dir)`, which runs `git ls-tree <rev> -- <dir>/` through `git::exact` and is lazy per folder;
  - the `repositories` tool window: `src/repositories/RepositoryBrowser.svelte`:
    - the tab `<root> at <short hash>` (`B:1367`), with no toolbar;
    - folder and file icons, multi-select;
    - double click and Enter open each selected file read-only;
    - the popup `Jump to Source` (`A:553`) and `Compare with Local` (`A:1636`);
  - `src/log/contextMenu.ts`: wire `Show Repository at Revision`.
- Test: `src-tauri/src/tree.rs`, `src/repositories/RepositoryBrowser.test.ts`.

- [ ] **Step 1: Write the failing tests:**
  - `ls_tree_lists_one_level_of_a_revision`.
  - `a_second_revision_replaces_the_tab`.
  - `enter_opens_the_file_at_that_revision_read_only`.
  - `compare_with_local_opens_the_diff`.
- [ ] **Step 2: Check that they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Check that they pass.**
- [ ] **Step 5: Commit.** `git commit -m "feat: Show Repository at Revision"`

### Task 10: Not Cherry-Picked Commits, `Select in Tree…` and the merge window's `Show Details`

Research: G§7, G§8, G§9, H§5, H§6, R§1 (`muted`), R§3 (`Flow`).

**Files:**
- Create:
  - `src-tauri/src/log/cherry.rs`: `not_picked(repo, current, compared)`, from `git cherry <current|HEAD> <compared>`, where the `+` lines are the commits not picked;
  - `src/log/StructureChooser.svelte`:
    - `Select Paths to Filter by` (`L:219`);
    - folders and files, without ignored or unversioned ones;
    - the checkbox rules of `SelectionManager` (G§8);
    - the limit 100, with the red `L:220-221`;
    - OK disabled while nothing is checked;
  - `src/merge/DetailsDialog.svelte`:
    - a single commit: its changes tree with the conflicted file preselected, over the message;
    - a range: progress `B:1293`, the subjects on the left and the changes over the details on the right, and `Filter by conflicted file` (`B:1291`), on.
- Modify:
  - `src/log/ViewOptions.svelte`: the `Not Cherry-Picked Commits` toggle, after the `Highlight` section (G§7). With one branch filter it runs at once. Otherwise it opens `Select Source Branch` (`B:1050`), which replaces the branch filter.
    - The other rows get `#6F737A`.
    - A change of branch filter cancels it with the balloon `B:1053`.
    - A refresh recomputes it.
    - Errors use `B:1052`.
  - `src/log/FilterBar.svelte`: the Paths popup becomes `Select…`, `Select in Tree…`, `Recent`, with 10 recent groups.
  - `src-tauri/src/conflicts.rs`: `merge_details(path)` gives each side as a single commit or a range, and only in a merge, rebase or cherry-pick (G§9).
  - `src/merge/MergeWindow.svelte`: the `Show Details` link (`B:1292`) at the right of a side title.
- Test: `src-tauri/src/log/cherry.rs`, `src/log/ViewOptions.test.ts`, `src/log/StructureChooser.test.ts`, `src/log/FilterBar.test.ts`, `src-tauri/src/conflicts.rs`, `src/merge/MergeWindow.test.ts`.

- [ ] **Step 1: Write the failing tests:**
  - `cherry_marks_picked_commits_by_patch` (overview decision 23).
  - `toggle_without_one_branch_asks_select_source_branch`.
  - `changing_the_branch_filter_cancels_with_the_balloon`.
  - `structure_chooser_selects_folders_and_files_up_to_100`.
  - `ok_replaces_the_paths_filter_and_adds_to_recent`.
  - `show_details_appears_only_in_merge_rebase_and_cherry_pick`.
  - `a_range_side_lists_its_commits_filtered_by_the_conflicted_file`.
- [ ] **Step 2: Check that they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Check that they pass.**
- [ ] **Step 5: Commit.** `git commit -m "feat: Not Cherry-Picked Commits, Select in Tree and Show Details"`

### Task 11: Annotate

Research: H§2, G§10, R§6 (`hunks::compute`). Decision 7.

**Files:**
- Create:
  - `src-tauri/src/annotate.rs`:
    - `blame(repo, path, revision, local_text)`: `git blame --porcelain -l -t --encoding=UTF-8 -w <rev> -- <path>` through `git::exact`; a local file uses the blame of the last commit that touched it;
    - lines changed locally map to blank through `hunks::compute`;
    - the revisions are grouped by committer date and coloured by order with G§10's 21 dark colours;
    - the numbers come from the history, oldest = 1;
  - `src/viewer/AnnotationGutter.svelte` and `src/viewer/annotation.ts`:
    - the columns `Date` (pretty date), `Author` (`ShortNameType` `Last Name`), with `Revision` and `Commit number` hidden;
    - left alignment;
    - the latest commit in bold `#ced0d6`, the others `#8d9199`;
    - the tooltip as text (`V:927-930`);
    - a click selects the commit in the main Log;
    - the gutter popup of H§2: `Show Diff`, `View ▸`, `Copy Revision Number`, `Annotate Revision`, `Annotate Previous Revision`, `Close Annotations`.
- Modify:
  - `src/viewer/ViewerWindow.svelte`: the gutter context menu (`Annotate with Git Blame` / `Close Annotations`, then `Soft-Wrap`);
  - `src/files/gitFileMenu.ts`: enable `Annotate`. It is disabled on a folder or a binary file, where it says `Cannot annotate directory` (`B:18`).
- Test: `src-tauri/src/annotate.rs`, `src/viewer/annotation.test.ts`, `src/viewer/AnnotationGutter.test.ts`.

- [ ] **Step 1: Write the failing tests:**
  - `blame_porcelain_maps_lines_to_revisions`.
  - `locally_changed_lines_are_blank`.
  - `colours_by_order_newest_first_with_the_dark_palette`.
  - `short_names_follow_shortnametype` (the rules of G§10).
  - `annotate_previous_revision_opens_the_parent_at_the_translated_line`.
  - `a_change_on_disk_recomputes_the_local_annotation` (Review Focus 5).
  - `a_binary_file_cannot_be_annotated`.
- [ ] **Step 2: Check that they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Check that they pass.**
- [ ] **Step 5: Commit.** `git commit -m "feat: Annotate with Git Blame in the viewer"`

### Task 12: Show History for Selection

Research: H§3, G§11, G§0.2.

**Files:**
- Create:
  - `src-tauri/src/block_history.rs`, a port of `Block` and `DiffUtil.updateRangeOnModification` (G§11). Its tests are `SelectedBlockHistoryTest` lines 17-384, copied as table tests and listed in `NOTICE`. `selection_history(repo, path, start, end, local_text, cancel)` streams one block per revision of Task 7's history, with `git cat-file blob <rev>:<path>` through `git::exact`.
  - `src/history/SelectionHistoryWindow.svelte`, a non-modal window titled `History for Selection` (`V:626-627`):
    - the block diff on top (0.5), titled `Revision {0}` (`V:182`);
    - the table `Version | Date | Author | Commit Message`;
    - `Changes only`, on (`V:180`);
    - the status `Loading revision {0}…` with `(n/total)`;
    - `Commit Message:` (`V:898`);
    - the first row `Local Changes` (`V:899`);
    - the popup `Show Diff`, `Compare with Local`, `Show All Affected Files`, `Copy Revision Number`.
- Modify: `src/files/gitFileMenu.ts` (enable `Show History for Selection…` with a viewer selection), the `commands.rs` window plumbing.
- Test: `src-tauri/src/block_history.rs`, `src/history/SelectionHistoryWindow.test.ts`.

- [ ] **Step 1: Write the failing tests:**
  - `selected_block_history_cases_from_intellij` (all the copied cases).
  - `changes_only_keeps_revisions_whose_block_differs`.
  - `closing_the_window_cancels_the_stream`.
- [ ] **Step 2: Check that they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Check that they pass.**
- [ ] **Step 5: Commit.** `git commit -m "feat: Show History for Selection"`

### Task 13: Matrix, report and full verification

**Files:**
- Modify: `docs/parity-matrix.md`. Every row of R§7 gets its result and its test:
  - `Vcs.Log.ShowOtherBranches` leaves the table (it is the Branch filter now);
  - `Vcs.ShowMessageHistory` keeps `done`;
  - `Select in Tree…` is relabelled as the Paths popup;
  - the merge `Show Details` gets a row;
  - the staging rows say "not in the user's mode";
  - the Find Action rows say "no entry point without Find Action";
  - the counts sentence covers part 4.
- Modify: `docs/reports/phase-4-validation.md`, with a Part 4 section:
  - decisions, rulings with their costs, and tests by feature;
  - what could not be checked;
  - the manual checklist:
    - history of a real renamed file against IntelliJ's;
    - opening a file from the terminal;
    - `Get` over a modified file;
    - Compare Branches on a real branch;
    - Annotate on a real file;
    - Show Repository at Revision;
    - Not Cherry-Picked on a branch with picks.
- Modify: `NOTICE` (`GitFileHistory`, `Block`, `SelectedBlockHistoryTest`, `DeepComparator`, the annotation colours) and `docs/reports/phase-4-reference-change.md` (the status line).

- [ ] **Step 1:** Each `done` cites its test, and the test exists.
- [ ] **Step 2: Full verification, one suite at a time:** tsc, full vitest, `cargo test --lib`, clippy `--all-targets -D warnings`, python. The report writes the exact counts.
- [ ] **Step 3: Commit.** `git commit -m "docs: phase 4 part 4 results in the matrix and the report"`
