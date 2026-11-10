# Phase 1: startup, commit, changelists and diff. Implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make all the day-to-day commits without opening IntelliJ: see changes live, organize them into changelists, choose files, hunks or lines, review the diff and confirm.

**Architecture:** A Rust core with modules by responsibility, which read the state with `git status`, persist their own data outside the repository and reproduce the commit flow of `GitCheckinEnvironment`. The Svelte interface opens on the Commit view, with the log as a lazily loaded tab, and the diff in its own window.

**Tech Stack:** Tauri 2.12, Rust 1.99.1, Svelte 5, Vitest 5, CodeMirror 6 with `@codemirror/merge`, `notify` 8, `similar` 3, `quick-xml` 0.42, `sha2` 0.11.

**Spec:** `docs/superpowers/specs/2026-10-31-intellij-git-client-design.md`. Scope: phase 1 rows of `docs/parity-matrix.md`.

**Exit criterion:** the user makes their real commits without opening IntelliJ and declares the phase finished.

**Branch:** `fase-1`, created from `fase-0`. `fase-0` is still not merged into `main`.

## Global Constraints

- Platform: macOS arm64. No JVM and no bundled Chromium.
- Every Git operation uses the real `git` binary.
- Reference: IntelliJ IDEA CE 2023.2.3, build `IC-232.10072.27`, tag `idea/232.10072.27`. Do not use `master`.
- Devin edits files in the local copy and the user performs the Git operations.
- One repository per window.
- Own data in `~/Library/Application Support/<app>/`, per repository. Nothing is ever written to `.idea/`.
- Before rollback, reset or deleting changes, a recoverable copy of the affected files, kept for 30 days.
- Destructive operations ask for the same confirmations as IntelliJ.
- Automated tests use throwaway repositories, never real ones.
- Goals (a guide): about 300 MB of memory at rest, CPU close to 0 %, less than 1 second startup, dark theme of the New UI and macOS keymap.
- Do not use the name or logos of JetBrains or IntelliJ.

## Review Focus

1. Devin changes a file with lines already selected for commit: the valid selections are kept and the others are reported. Test in Task 8.
2. A commit fails halfway (hook, GPG): what was staged before returns to its state and nothing is lost. Test in Task 7.
3. Files without a final newline, with CRLF or not UTF-8 in a partial commit: they keep their bytes. Test in Task 8.
4. `index.lock` busy during an operation: it is retried and, if it persists, there is a clear notice. Test in Task 3.
5. Damaged `changelists.json`: the app opens, keeps a copy of the file and creates new lists. Test in Task 4.

---

### Task 1: Startup and app structure

**Files:**
- Create: `src/lib/tabs.ts`, `src/lib/tabs.test.ts`, `docs/reports/phase-1-startup.md`
- Modify: `src/App.svelte`, `src/App.test.ts`, `src-tauri/src/lib.rs`, `src-tauri/src/commands.rs`, `src-tauri/tauri.conf.json`

**Interfaces:**
- Produces:
  - `type Tab = "commit" | "log"`
  - `nextTab(current: Tab, shortcut: "commit" | "log"): Tab`
  - `AppState.repo` split into `RepoSession { path }` on open, and `OpenRepo` loaded only with `log_open`
  - Command `repo_open(repo: String) -> Result<RepoInfo, ApiError>`, with `RepoInfo { path, head: Option<String>, branch: Option<String> }`

- [ ] **Step 1: Create the branch**

Run: `git switch -c fase-1 fase-0`

- [ ] **Step 2: Write the failing tests**

- `opens_on_commit_without_loading_the_log`: with a mocked `repo_open`, `App` does not call `log_open`.
- `log_tab_loads_once`: when the Log tab is opened twice, `log_open` is called once.
- In Rust, `repo_open_without_commits`: `head` and `branch` reflect a repository without commits, and `branch` is `main`.
- In Rust, `repo_open_detached_head`: `branch` is `None`.
- In Rust, `repo_open_without_repository`: `kind == "not_a_repository"`.

- [ ] **Step 3: Check that they fail**

Run: `pnpm vitest run src/App.test.ts && (cd src-tauri && cargo test commands::)`
Expected: FAIL

- [ ] **Step 4: Implement tabs and lazy loading**

`repo_open` uses `git symbolic-ref -q --short HEAD` and `git rev-parse -q --verify HEAD`. Replace `env::args()` with `env::args_os()` with lossy conversion, so that a non-UTF-8 argument does not close the app.

- [ ] **Step 5: Check that they pass**

Run: `pnpm vitest run && (cd src-tauri && cargo test)`
Expected: PASS

- [ ] **Step 6: Locate the native startup**

Build the `.app` with `pnpm tauri build --bundles app`. Measure 5 times with `open -n` up to the first-paint mark, and compare with launching the binary directly. For the 1.1 s stretch between `run()` and `setup`, use `sample <pid> 2` during startup and identify the most expensive functions.

Write the result in `docs/reports/phase-1-startup.md`, with the measured cause.

- [ ] **Step 7: Apply whatever reduces startup**

Create the window hidden (`"visible": false`) and show it after the first paint, so that it is not seen blank. With `RunEvent::ExitRequested` and `WindowEvent::CloseRequested`, closing the window hides it and the app keeps running in the background, so that reopening is immediate. `⌘Q` does close it completely.

Also apply whatever came out of the previous step. If 1 second is not reached, leave the real value in the report without changing the goal.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat: open on the Commit view and shorten startup"
```

### Task 2: Repository state

**Files:**
- Create: `src-tauri/src/status.rs`
- Modify: `src-tauri/src/lib.rs`, `src-tauri/src/commands.rs`, `src/lib/api.ts`

**Interfaces:**
- Consumes: `git::run`.
- Produces:
  - `enum FileState { Modified, Added, Deleted, Renamed, Copied, Unmerged, Untracked, Ignored }`
  - `struct FileChange { path: String, old_path: Option<String>, state: FileState, staged: bool, unstaged: bool }`
  - `status::read(repo: &Path, include_ignored: bool) -> Result<Vec<FileChange>, GitError>`
  - Command `repo_status(include_ignored: bool) -> Result<Vec<FileChange>, ApiError>`

- [ ] **Step 1: Write the failing tests**

In `status.rs`:
- `modified_unstaged`: `state == Modified`, `unstaged` true, `staged` false.
- `modified_staged_and_unstaged`: both true.
- `unversioned_file`: `Untracked`.
- `staged_rename`: `Renamed`, with `old_path`.
- `conflicto`: after a merge with a conflict, `Unmerged`.
- `deleted_after_staging_again`: the `AD` state gives `Added`, with `staged` and `unstaged`.
- `path_with_spaces_and_accents`: the path arrives unescaped.
- `ignored_only_when_requested`: appears only with `include_ignored`.
- `unversioned_folder_lists_its_files`: the files come out one by one, not as a folder.

- [ ] **Step 2: Check that they fail**

Run: `(cd src-tauri && cargo test status::)`
Expected: FAIL

- [ ] **Step 3: Implement `status::read`**

Use `git status --porcelain=v1 -z --untracked-files=all --ignore-submodules=dirty`, with `--ignored` if requested. With `-z`, renames carry the previous path in the next field.

- [ ] **Step 4: Check that they pass**

Run: `(cd src-tauri && cargo test status::)`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: reading the repository state"
```

### Task 3: Live refresh and index locking

**Files:**
- Create: `src-tauri/src/watch.rs`
- Modify: `src-tauri/Cargo.toml`, `src-tauri/src/git.rs`, `src-tauri/src/commands.rs`, `src/lib/api.ts`

**Interfaces:**
- Consumes: `RepoSession`.
- Produces:
  - `watch::relevant(path: &Path, repo: &Path) -> bool`
  - `watch::Debouncer::new(delay: Duration)`, `.push(now: Instant)`, `.ready(now: Instant) -> bool`
  - `watch::start(repo: PathBuf, on_change: impl Fn() + Send + 'static) -> Result<Watcher, GitError>`
  - Tauri event `repo-changed`, without data
  - `git::run_mutating(repo: &Path, args: &[&str]) -> Result<Vec<u8>, GitError>`
  - `GitError::IndexLocked`

- [ ] **Step 1: Write the failing tests**

In `watch.rs`:
- `ignores_objects_and_locks`: `.git/objects/ab/cd` and `.git/index.lock` are not relevant.
- `counts_git_changes`: `.git/index`, `.git/HEAD` and `.git/refs/heads/main` are.
- `counts_working_files`: `src/a.txt` is.
- `groups_consecutive_changes`: three `push` calls within 100 ms give a single `ready` after 200 ms of silence.
- `detects_a_real_write`: with `TempRepo`, writing a file calls `on_change` in under 2 seconds.

In `git.rs`:
- `run_mutating_waits_for_the_lock`: with `.git/index.lock` created and deleted by another thread after 300 ms, `git add` finishes well.
- `run_mutating_reports_if_still_locked`: with the lock permanent, it returns `IndexLocked`.

- [ ] **Step 2: Check that they fail**

Run: `(cd src-tauri && cargo test watch:: git::)`
Expected: FAIL

- [ ] **Step 3: Implement the watcher and the retries**

`notify` with FSEvents, recursive, and a thread that applies a 200 ms `Debouncer` and emits `repo-changed`. `run_mutating` retries 5 times every 200 ms if stderr contains `index.lock`. The watcher is switched when another repository is opened and stopped on close.

- [ ] **Step 4: Check that they pass**

Run: `(cd src-tauri && cargo test watch:: git::)`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: live refresh and waiting for the index lock"
```

### Task 4: Changelists

**Files:**
- Create: `src-tauri/src/changelists.rs`, `src-tauri/src/storage.rs`
- Modify: `src-tauri/Cargo.toml`, `src-tauri/src/commands.rs`, `src/lib/api.ts`

**Interfaces:**
- Consumes: `FileChange`.
- Produces:
  - `storage::repo_dir(base: &Path, repo: &Path) -> PathBuf`, with the SHA-256 of the canonical path
  - `storage::write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()>`
  - `struct ChangeList { id: String, name: String, comment: String }`
  - `struct Changelists { lists: Vec<ChangeList>, active: String, assignments: BTreeMap<String, String> }`
  - `Changelists::load(dir: &Path) -> Changelists`, `.save(dir)`, `.create(name)`, `.rename(id, name)`, `.remove(id)`, `.set_active(id)`, `.move_paths(paths, id)`, `.reconcile(paths: &[String])`, `.list_for(path) -> &str`
  - `DEFAULT_LIST_NAME = "Changes"`
  - Commands `changelists_get`, `changelists_create`, `changelists_rename`, `changelists_remove`, `changelists_set_active`, `changelists_move`

- [ ] **Step 1: Write the failing tests**

- `initial_list`: without a file, there is an active `Changes` list.
- `new_file_goes_to_the_active_one`: `reconcile` assigns the file to the active list.
- `file_that_disappears_is_forgotten`: `reconcile` removes its assignment.
- `removing_a_list_moves_to_the_active_one`: its files go to the active one.
- `removing_the_active_list`: the active one becomes the first remaining.
- `the_last_one_is_not_removed`: `remove` does not leave zero lists.
- `saves_and_recovers`: `save` and `load` return the same.
- `damaged_file`: with invalid JSON, `load` returns the initial list and keeps `changelists.json.corrupt`.
- `repo_dir_is_stable`: the same path always gives the same folder.

- [ ] **Step 2: Check that they fail**

Run: `(cd src-tauri && cargo test changelists:: storage::)`
Expected: FAIL

- [ ] **Step 3: Implement**

`write_atomic` writes to a temporary file in the same folder and renames. The base directory is Tauri's `app_data_dir()`.

- [ ] **Step 4: Check that they pass**

Run: `(cd src-tauri && cargo test changelists:: storage::)`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: own changelists outside the repository"
```

### Task 5: Import IntelliJ changelists

**Files:**
- Create: `src-tauri/src/idea_import.rs`, `src-tauri/testdata/workspace.xml`
- Modify: `src-tauri/Cargo.toml`, `src-tauri/src/commands.rs`, `src/lib/api.ts`

**Interfaces:**
- Consumes: `Changelists`.
- Produces:
  - `idea_import::read_workspace(xml: &str) -> Result<ImportedLists, String>`
  - `ImportedLists { lists: Vec<ImportedList> }`, with `ImportedList { name, comment, active: bool, paths: Vec<String> }`
  - `Changelists::merge_import(imported: ImportedLists)`
  - Command `changelists_import() -> Result<Changelists, ApiError>`

- [ ] **Step 1: Write the failing tests**

With a sample `workspace.xml`, with `<component name="ChangeListManager">`:
- `reads_lists_and_paths`: `$PROJECT_DIR$/src/a.txt` becomes `src/a.txt`.
- `marks_the_default_list`: `default="true"` gives `active`.
- `ignores_paths_outside_the_project`.
- `without_component`: returns empty lists.
- `import_twice`: `merge_import` does not duplicate lists with the same name.
- `does_not_write_to_idea`: after importing, the content of `.idea/` does not change.

- [ ] **Step 2: Check that they fail**

Run: `(cd src-tauri && cargo test idea_import::)`
Expected: FAIL

- [ ] **Step 3: Implement with `quick-xml`**

Open the file for reading only. Changes use `afterPath`, or `beforePath` if it is a deletion.

- [ ] **Step 4: Check that they pass**

Run: `(cd src-tauri && cargo test idea_import::)`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: import IntelliJ changelists in read-only mode"
```

### Task 6: Recoverable copy and rollback

**Files:**
- Create: `src-tauri/src/snapshots.rs`, `src-tauri/src/rollback.rs`
- Modify: `src-tauri/src/commands.rs`, `src/lib/api.ts`

**Interfaces:**
- Consumes: `storage::repo_dir`, `git::run_mutating`, `FileChange`.
- Produces:
  - `SNAPSHOT_DAYS: u64 = 30`
  - `snapshots::create(dir: &Path, repo: &Path, paths: &[String], reason: &str) -> io::Result<SnapshotId>`
  - `snapshots::list(dir) -> Vec<Snapshot>`, `snapshots::restore(dir, repo, id)`, `snapshots::prune(dir, now: SystemTime)`
  - `rollback::rollback(repo: &Path, dir: &Path, changes: &[FileChange], delete_added: bool) -> Result<SnapshotId, RollbackError>`
  - Commands `rollback_files`, `snapshots_list`, `snapshots_restore`

- [ ] **Step 1: Write the failing tests**

- `rollback_modified`: the file returns to `HEAD` and the copy keeps the previous content.
- `rollback_keeps_foreign_stage`: another staged file does not change.
- `rollback_new_file_without_deleting`: with `delete_added = false`, it is no longer staged and stays on disk.
- `rollback_new_file_deleting`: with `delete_added = true`, it disappears and stays in the copy.
- `rollback_deleted`: the file comes back.
- `rollback_renamed`: the previous name comes back.
- `restore_snapshot`: after the rollback, `restore` recovers the content.
- `if_the_snapshot_fails_it_touches_nothing`: with the copies folder without permission, it returns an error and the file stays the same.
- `deletes_old_snapshots`: `prune` removes those older than 30 days and leaves the recent ones.
- `binary_file`: the copy keeps the bytes.

- [ ] **Step 2: Check that they fail**

Run: `(cd src-tauri && cargo test snapshots:: rollback::)`
Expected: FAIL

- [ ] **Step 3: Implement**

Each copy is a folder with `manifest.json` and the files with their relative path. Follow IntelliJ's order: `git reset HEAD -- <paths>` and `git checkout HEAD -- <paths>`, always using `--`. `prune` runs when the repository is opened.

- [ ] **Step 4: Check that they pass**

Run: `(cd src-tauri && cargo test snapshots:: rollback::)`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: rollback with a recoverable copy"
```

### Task 7: Commit engine

**Files:**
- Create: `src-tauri/src/commit.rs`
- Modify: `src-tauri/src/commands.rs`, `src/lib/api.ts`

**Reference:** `plugins/git4idea/src/git4idea/checkin/GitCheckinEnvironment.java` (`commitRepository`, `commitUsingIndex`) and `GitRepositoryCommitter.kt`.

**Interfaces:**
- Consumes: `git::run_mutating`, `FileChange`.
- Produces:
  - `struct PartialFile { path: String, content: Vec<u8> }`
  - `struct CommitOptions { amend: bool, sign_off: bool, skip_hooks: bool, author: Option<String>, cleanup: bool }`
  - `struct CommitRequest { added: Vec<String>, removed: Vec<String>, partial: Vec<PartialFile>, message: String, options: CommitOptions }`
  - `enum CommitError { Unmerged, NothingToCommit, Gpg(String), Hook(String), Git(GitError) }`
  - `commit::commit(repo: &Path, request: &CommitRequest) -> Result<String, CommitError>`, which returns the hash
  - Command `commit_changes(request) -> Result<String, ApiError>`

- [ ] **Step 1: Write the failing tests**

- `commit_of_a_file`: another modified file stays uncommitted.
- `keeps_excluded_stage`: a staged file outside the selection stays staged after the commit.
- `restores_stage_if_the_hook_fails`: with a failing `pre-commit`, it returns `Hook` and the stage is left as it was.
- `error_with_conflicts`: with unresolved files, it returns `Unmerged`.
- `deleted_excluded_with_ad_state`: an `AD` file outside the selection does not break the commit.
- `partial_commit`: commits `PartialFile` and the disk keeps the full content.
- `partial_keeps_executable_permission`: mode `100755` is kept.
- `amend`: does not create a new commit and changes the message.
- `sign_off`: the message includes `Signed-off-by:`.
- `autor`: `--author` is applied.
- `no_changes`: returns `NothingToCommit`.
- `message_with_quotes_and_accents`: kept as is, and starting with `-` breaks nothing.
- `path_that_looks_like_an_option`: `-file.txt` is committed.

- [ ] **Step 2: Check that they fail**

Run: `(cd src-tauri && cargo test commit::)`
Expected: FAIL

- [ ] **Step 3: Implement the flow**

Repeat the order of `commitUsingIndex`:

1. Abort with `Unmerged` if `git diff --name-only --diff-filter=U` is not empty.
2. For each `PartialFile`: `git hash-object -w --stdin --path <path>` and `git update-index --add --cacheinfo <mode>,<hash>,<path>`, with the mode from `git ls-files -s`.
3. With `git diff --cached --name-status -z`, locate what is staged outside the selection. Save its blob and mode.
4. Remove those paths from the stage with `git reset HEAD -- <paths>`.
5. `git add --ignore-errors -- <added>` and `git rm --cached --ignore-unmatch -- <removed>`, without the partial paths.
6. Write the message to a temporary file and run `git commit -F <file> [--amend] [--signoff] [--no-verify] [--cleanup=strip] [--author=<a>] --`.
7. Always, also on failure, restore the excluded paths with `update-index --cacheinfo`.

Classify the failure by stderr: `gpg failed to sign` gives `Gpg`, `nothing to commit` gives `NothingToCommit`, and a hook failure gives `Hook` with its output.

- [ ] **Step 4: Check that they pass**

Run: `(cd src-tauri && cargo test commit::)`
Expected: PASS

- [ ] **Step 5: Compare with IntelliJ**

Create `scripts/git-wrapper.sh`, which logs the arguments and delegates to `git`. Compare the commands of a partial commit with excluded stage in IntelliJ 2023.2.3 and in `commit.rs`, in a throwaway repository. Note the differences in `docs/reports/phase-1-commands.md`, and fix those that change the result.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: commit engine with IntelliJ's flow"
```

### Task 8: Hunks and line selection

**Files:**
- Create: `src-tauri/src/hunks.rs`
- Modify: `src-tauri/Cargo.toml`, `src-tauri/src/commands.rs`, `src/lib/api.ts`

**Interfaces:**
- Consumes: `PartialFile`.
- Produces:
  - `struct Hunk { id: String, old_start: u32, old_lines: u32, new_start: u32, new_lines: u32 }`, with `id` computed from the content
  - `struct LineSelection { hunk: String, lines: Option<Vec<u32>> }`, where `None` is the whole hunk
  - `hunks::compute(base: &[u8], current: &[u8]) -> Vec<Hunk>`
  - `hunks::partial_content(base: &[u8], current: &[u8], selection: &[LineSelection]) -> Vec<u8>`
  - `hunks::revert_content(base: &[u8], current: &[u8], selection: &[LineSelection]) -> Vec<u8>`, for `Vcs.RollbackChangedLines`
  - `hunks::revalidate(selection: &[LineSelection], hunks: &[Hunk]) -> Revalidated`, with `Revalidated { kept: Vec<LineSelection>, lost: Vec<String> }`
  - `Changelists::move_hunks(path: &str, hunks: &[String], id: &str)` and `Changelists::hunks_in(path: &str, id: &str) -> Vec<String>`, with unassigned hunks in the file's list
  - Commands `file_hunks(path: String) -> Result<Vec<Hunk>, ApiError>`, `lines_rollback(path, selection) -> Result<SnapshotId, ApiError>` and `changelists_move_hunks(path, hunks, id)`

- [ ] **Step 1: Write the failing tests**

- `one_hunk_chosen_of_two`: the result carries only that change.
- `none_chosen`: the result is `base`, byte for byte.
- `all_chosen`: the result is `current`, byte for byte.
- `single_line_inside_a_hunk`: of three added lines, only the chosen one goes in.
- `no_final_newline`: keeps the absence of a final `\n`.
- `crlf`: keeps `\r\n` on all lines.
- `not_utf8`: keeps `\xff` bytes.
- `selections_survive_another_change`: if another hunk changes, the chosen one stays in `kept`.
- `lost_selection_is_reported`: if the chosen hunk changes, its id appears in `lost`.
- `rollback_of_a_hunk`: `revert_content` undoes only that change.
- `line_rollback_creates_snapshot`: `lines_rollback` leaves a recoverable copy before writing.
- `hunk_in_another_list`: after `move_hunks`, `hunks_in` returns each hunk in its list.
- `moved_hunk_that_disappears`: if the hunk changes, its assignment is forgotten and it goes back to the file's list.

- [ ] **Step 2: Check that they fail**

Run: `(cd src-tauri && cargo test hunks::)`
Expected: FAIL

- [ ] **Step 3: Implement**

Work with lines as bytes that include their terminator, and use `similar` with the Myers algorithm. The `id` is the SHA-256 of the hunk's old and new lines, so that it does not depend on position. That allows keeping the selection when Devin edits another part.

- [ ] **Step 4: Check that they pass**

Run: `(cd src-tauri && cargo test hunks::)`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: hunks and partial content by lines"
```

### Task 9: Commit view

**Files:**
- Create: `src/commit/CommitView.svelte`, `src/commit/ChangesTree.svelte`, `src/commit/CommitMessage.svelte`, `src/commit/tree.ts`, `src/commit/tree.test.ts`, `src/commit/validate.ts`, `src/commit/validate.test.ts`, `src/lib/keymap.ts`, `src/lib/keymap.test.ts`, `src-tauri/src/messages.rs`
- Modify: `src/App.svelte`, `src/lib/api.ts`, `src-tauri/src/commands.rs`

**Interfaces:**
- Consumes: `repo_status`, `repo-changed`, commands `changelists_*`, `commit_changes`, `rollback_files`.
- Produces:
  - `buildTree(changes: FileChange[], lists: Changelists, groupByDirectory: boolean): TreeNode[]`
  - `checkState(node: TreeNode, included: Set<string>): "on" | "off" | "mixed"`
  - `validateMessage(message: string): MessageIssue[]`, with the codes `empty`, `subject-too-long`, `second-line-not-empty`, `body-too-long`
  - `matchShortcut(event: KeyboardEvent): ActionId | null`
  - `MAX_MESSAGE_HISTORY = 20`
  - `messages::push(dir, message)`, `messages::list(dir) -> Vec<String>`

- [ ] **Step 1: Write the failing tests**

In `tree.test.ts`:
- `groups_by_changelist`: each list is a root node, with its number of files.
- `unversioned_in_its_own_section`: the `Untracked` ones are under `Unversioned Files`.
- `groups_by_directory`: `src/a/b.txt` creates `src/a` nodes, with a single entry if there is no branching.
- `mixed_checkbox`: a list with one included file and another not gives `mixed`.
- `stable_order`: a refresh does not reorder.

In `validate.test.ts`:
- `long_subject`: 73 characters give `subject-too-long`; 72 do not.
- `second_line`: a second line with text gives `second-line-not-empty`.
- `vacio`: gives `empty`.

In `keymap.test.ts`: `⌘K` gives `CheckinProject`, `⌘0` gives `ActivateCommitToolWindow`, `⌃⌥Z` gives `ChangesView.Revert`, `⌘⇧M` gives `ChangesView.Move`, `⌃M` gives `Vcs.ShowMessageHistory`, `⌃⌥M` gives `Vcs.ToggleAmendCommitMode`, `F2` gives `ChangesView.Rename`, `⌃Espacio` gives `ChangesView.SetDefault`, and `⌘D` gives `Diff.ShowDiff`.

In Rust, `history_without_duplicates` and `limited_history`: 21 messages leave 20.

- [ ] **Step 2: Check that they fail**

Run: `pnpm vitest run src/commit src/lib && (cd src-tauri && cargo test messages::)`
Expected: FAIL

- [ ] **Step 3: Implement the view**

Copy the layout of the Commit window of the dark New UI: tree on top, message at the bottom, buttons `Commit` and `Commit and Push…`. The latter stays disabled until phase 2. The checkboxes, the right click and the shortcuts follow the matrix.

The action texts are in English, the same as in IntelliJ, so as not to change habits. Copy the status colors of `ExperimentalDark`. Use IntelliJ's confirmations for rollback and for removing a list.

Each `repo-changed` event reloads `repo_status` and applies `Changelists::reconcile`, without losing the checkboxes of the files that are still there.

- [ ] **Step 4: Check that they pass**

Run: `pnpm vitest run && (cd src-tauri && cargo test)`
Expected: PASS

- [ ] **Step 5: Check in the app**

With a throwaway repository, and editing files from the terminal, check with window screenshots that the changes appear in under 1 second, and that creating a list, moving a file, committing it and doing a rollback work.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: Commit view with changelists"
```

### Task 10: Diff window

**Files:**
- Create: `src/diff/window.ts`, `src/diff/DiffWindow.svelte`, `src/diff/gutter.ts`, `src/diff/gutter.test.ts`, `src/diff/language.ts`, `src/diff/language.test.ts`, `src-tauri/src/worktree_diff.rs`
- Modify: `src/diff/DiffView.svelte`, `src/diff/sides.ts`, `src/App.svelte`, `src-tauri/src/diff.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/commands.rs`, `package.json`

**Interfaces:**
- Consumes: `Hunk`, `LineSelection`, `file_hunks`, `Side`.
- Produces:
  - `worktree_diff::sides(repo: &Path, path: &str) -> Result<FileSides, GitError>`, from `HEAD` to disk
  - Command `worktree_sides(path: String) -> Result<FileSides, ApiError>`
  - Command `file_write(path: String, content: String, expected_hash: String) -> Result<String, ApiError>`, which fails if the file changed
  - Command `diff_window_open(path: String)`, with a single, reused `diff` window
  - `languageFor(path: string): Extension`
  - `toggleLine(selection: LineSelection[], hunkId: string, line: number): LineSelection[]`
  - Event `selection-changed { path, selection }`

- [ ] **Step 1: Write the failing tests**

In `gutter.test.ts`:
- `check_a_line_of_a_whole_hunk`: goes from `None` to the list without that line.
- `check_the_last`: goes back to `None`.
- `hunk_without_lines`: disappears from the selection.

In `language.test.ts`: `.rs`, `.ts`, `.py`, `.go`, `.json`, `.c` and a file without an extension return an extension without failing.

In Rust:
- `sides_of_modified_file`, `sides_of_unversioned_file`, `sides_of_deleted_file`.
- `write_with_correct_hash`.
- `write_with_old_hash_fails`: if Devin changed the file, it is not overwritten.
- `paths_outside_the_repository_are_rejected`: `../x`, an absolute path and a symbolic link that leaves the repository give an error in `file_write` and `worktree_sides`.
- `gitlink_is_not_absent`: a submodule is not shown as a deleted file.
- `damaged_object_gives_error`: a `cat-file -s` that fails for another reason returns an error and not `Absent`.

- [ ] **Step 2: Check that they fail**

Run: `pnpm vitest run src/diff && (cd src-tauri && cargo test worktree_diff:: diff::)`
Expected: FAIL

- [ ] **Step 3: Implement**

Create a single Tauri window, like the user's `VcsEditorTab.openInNewWindow = true`. Include side-by-side and unified view, combined diff of all files (`Vcs.Diff.ShowCombinedDiff`), alignment of changes, per-word highlighting, collapsing of unchanged areas, a whitespace option and synchronized scroll. The local side is editable and saves with `⌘S` through `file_write`.

Validate paths with a common function: resolve the canonical path and require that it be inside the repository's.

The gutter's context menu includes `Rollback Lines` (`⌃⌥Z`), `Move Lines to Another Changelist…` (`⌘⇧M`) and `Show Diff for Lines`. A commit per changelist includes only the hunks of that list, through `partial_content`.

The gutter checkboxes modify the selection and share it with the Commit view through `selection-changed`. `F7` and `⇧F7` jump between changes, `⌃⇧→` and `⌃⇧←` between files.

For files without changes in a commit, show `No changes compared to the first parent`. Fix in `read_side` so that only the message `does not exist in` or `Not a valid object name` is considered `Absent`.

- [ ] **Step 4: Check that they pass**

Run: `pnpm vitest run && (cd src-tauri && cargo test)`
Expected: PASS

- [ ] **Step 5: Check in the app**

In a throwaway repository, choose two lines of a hunk in the gutter, commit them and verify with `git show` that only those went in. Edit the file from the terminal with the diff open and check that the lost selection is reported.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: diff window with line selection"
```

### Task 11: Staging area mode and settings

**Files:**
- Create: `src/commit/StagingView.svelte`, `src/commit/staging.ts`, `src/commit/staging.test.ts`, `src/settings/SettingsView.svelte`, `src-tauri/src/settings.rs`, `src-tauri/src/staging.rs`
- Modify: `src/App.svelte`, `src-tauri/src/commit.rs`, `src-tauri/src/commands.rs`, `src/lib/api.ts`

**Interfaces:**
- Consumes: `git::run_mutating`, `storage::write_atomic`, `commit::commit`.
- Produces:
  - `struct Settings { git_path: Option<String>, staging_area: bool, warn_crlf: bool, warn_detached: bool, sign_commits: bool, signing_key: Option<String> }`
  - `settings::load(dir) -> Settings`, `settings::save(dir, &Settings)`
  - `staging::stage(repo, paths)`, `staging::unstage(repo, paths)`, `staging::stage_hunks(repo, path, selection)`, `staging::unstage_hunks(repo, path, selection)`
  - `CommitWarning { Crlf(Vec<String>), DetachedHead, Rebasing }`
  - `commit::warnings(repo, request, settings) -> Vec<CommitWarning>`
  - `groupStaging(changes: FileChange[]): { staged, unstaged, untracked, conflicts }`

- [ ] **Step 1: Write the failing tests**

In Rust:
- `default_values`: `staging_area` false, warnings true, signing false.
- `configured_git_path`: `git::run` uses `git_path`.
- `stage_of_a_hunk`: the index contains only that hunk and the disk stays the same.
- `unstage_of_a_hunk`: removes it from the index.
- `crlf_notice`, `detached_head_notice`, `warning_during_rebase`.
- `signing_enabled`: passes `-S` and, if GPG fails, returns `Gpg`.

In `staging.test.ts`, `file_in_both_groups`: a file with `staged` and `unstaged` appears in both.

- [ ] **Step 2: Check that they fail**

Run: `pnpm vitest run src/commit/staging && (cd src-tauri && cargo test settings:: staging:: commit::)`
Expected: FAIL

- [ ] **Step 3: Implement**

With `staging_area` active, the Commit view shows `Staged` and `Unstaged`, without changelists, as in IntelliJ. The commit uses the index as it is. The settings follow section 3 of the matrix, only those of phase 1. The `Enable staging area` option shows the notice that it disables changelists.

- [ ] **Step 4: Check that they pass**

Run: `pnpm vitest run && (cd src-tauri && cargo test)`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: staging area mode and Git settings"
```

### Task 12: Phase validation

**Files:**
- Create: `docs/reports/phase-1-validation.md`
- Modify: `docs/parity-matrix.md`, `scripts/measure.py`

**Interfaces:**
- Consumes: the complete app.
- Produces: the matrix with the result column for the phase 1 rows.

- [ ] **Step 1: Fix the measurement script**

Save `stages` in each run, retry if the report JSON is half written and list on stderr the WebKit processes counted. Add the tests `test_incomplete_report_is_retried` and `test_keeps_stages` first, and check them failing.

- [ ] **Step 2: Measure again**

Run: `python3 scripts/measure.py --app src-tauri/target/release/ramify --repo ~/ramify-measure/git --out docs/reports/phase-1-consumption.json --runs 5`
Expected: JSON with memory, CPU and startup.

The measurement mode now opens the Commit view, with a modified file and its diff. Leave the window visible during the measurement.

- [ ] **Step 3: Go through the matrix**

For each phase 1 row of `docs/parity-matrix.md`, note `done`, `partial` or `pending`. A `partial` explains what is missing.

- [ ] **Step 4: Write the report**

Compare with phase 0 and with IntelliJ. Include what could not be checked in throwaway repositories.

- [ ] **Step 5: Hand over to the user**

Build the `.app` and say how to open it. The user makes their real commits with the app. The phase ends when the user confirms it.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "docs: phase 1 validation"
```
