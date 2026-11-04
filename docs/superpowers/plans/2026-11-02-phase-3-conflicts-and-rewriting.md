# Phase 3: conflicts, merge, rebase and history rewriting. Implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Not to open IntelliJ for any everyday operation. That includes resolving conflicts in a three-pane merge, merging and rebasing from the menu and from the branches popup, continuing, skipping or aborting them, the interactive rebase, cherry-pick, revert and editing history from the log.

**Architecture:** The Rust core adds five pieces:
- a git editor of its own: the same binary acts as `GIT_EDITOR` and `GIT_SEQUENCE_EDITOR`, as it already does as askpass;
- a detector of the ongoing state;
- the conflicts module, which reads the `:1:`, `:2:` and `:3:` stages and marks files as resolved;
- a port of IntelliJ's line merge algorithm;
- a rebase engine with the model of the `git-rebase-todo` file.

The interface adds a three-pane merge window, separate from the diff one, with CodeMirror. It also adds the Conflicts, Merge…, Rebase… and Rebasing Commits dialogs, the branches popup and log actions, and the HEAD, Staged and Local comparison.

**Tech Stack:** Tauri 2.12, Rust 1.99.1, Svelte 5, Vitest 5 and CodeMirror 6 (already installed). No new dependencies unless a task says so.

**Spec:** `docs/superpowers/specs/2026-10-31-intellij-git-client-design.md`. Scope: the phase 3 rows of `docs/parity-matrix.md`, plus the ones that decision 9 brings into this phase.

**Exit criterion:** the user works in their real repositories without opening IntelliJ for any everyday operation, and declares the phase finished.

**Branch:** `fase-3`, created from `validacion-1-2` (`f952410`). `validacion-1-2` has no merge into `main`, and phase 3 touches `update.rs`, `pull.rs` and `branches.rs`, which carry fixes from the validation. When the user merges `validacion-1-2` into `main`, `fase-3` sits on top of it.

**Reference read:** the tag `idea/232.10072.27`, commit `f6fb1a451957d6cf92c5890c6c9fa05de6cc7eec`.
- In this session there is a copy of the phase folders in `/private/tmp/claude-501/-Users-victorcruz-PROJECTS-Experiments-cf-git-ide/4c8a86c1-0807-4f8a-814a-1e57cc57d855/scratchpad/ic/`, downloaded through the API with `fetch_ic.py`, which is in the same folder.
- If it is missing, it is downloaded the same way: `git clone` does not reach github.com from the sandbox, but `gh api` and `raw.githubusercontent.com` do.
- Whoever implements reads the file cited in each task and copies the literal texts from there, in English as in IntelliJ. The paths in the Reference column hang from `plugins/git4idea/src/git4idea/`, unless they start with `platform/`. `B:` is `plugins/git4idea/resources/messages/GitBundle.properties`.
- What is not in the source code, such as the keymap shortcuts or the default values of `TextDiffSettingsHolder`, is read from the jar of `/Applications/IntelliJ IDEA CE.app`, read-only, with its `javap` (`Contents/jbr/Contents/Home/bin/javap`).

## Global Constraints

- Platform: macOS arm64. No JVM or Chromium of its own.
- Every Git operation uses the real `git` binary, with arguments and without a shell, `--` before paths, and `LC_ALL=C` and `GIT_OPTIONAL_LOCKS=0`, as `git.rs` and `task.rs` already do.
- Reference: IntelliJ IDEA CE 2023.2.3, build `IC-232.10072.27`, tag `idea/232.10072.27`. Do not use `master`.
- Devin edits files in the local copy while the user works, and the user performs the Git operations.
- One repository per window. IntelliJ's dialogs for several roots (choosing a root, `Abort and Rollback`, `Could not Merge` with rollback) do not exist here.
- Own data in `~/Library/Application Support/<app>/`, per repository. Nothing is ever written in `.idea/` or `~/Library/Application Support/JetBrains`.
- Recoverable copy before reset, force checkout, abort or discarding changes, and also before overwriting a file in conflict: `snapshots::create` or `branches::copy_before`, kept for 30 days.
- Every command that changes files or the index first calls `flush_diff_window` and takes `RepoLocks`. When adding commands, update the count of `mutating_without_saving_the_diff_is_not_allowed` (`vcs_commands.rs:920`) and the list of `whatever_touches_the_repository_saves_the_diff_window_first` (`commands.rs:1908`).
- Long flows check that the repository did not change while they waited: every command that comes from a dialog receives the `HEAD` and the state that the dialog saw, and fails without touching anything if they no longer match. It is the text `Repository has already been changed` or the one IntelliJ uses at that point.
- Destructive operations ask for the same confirmations as IntelliJ, neither more nor fewer.
- Automatic tests use throwaway repositories and local bare remotes. Never the real ones, nor the network, nor `.idea/`, nor `~/Library/Application Support/JetBrains`.
- A password or passphrase never goes to disk, to the console, to an error or to an argument.
- Validation is done in Rust, not only in the interface:
  - branch and tag names with `git check-ref-format`;
  - no branch, revision, hash or path that starts with `-` reaches git as an option;
  - a hash comes from `rev-parse --verify <x>^{commit}`.
- What comes from a repository is untrusted text: messages, authors, branch names, file contents and git output. It is shown as text, without `{@html}`, and Tauri's CSP is not relaxed.
- The git editor only reads and writes files inside the git directory of the repository of the task that launched it.
- Consumption targets (guide, not limit): about 300 MB of memory at rest, CPU close to 0 %, less than 1 s startup, scroll without jumps, New UI dark theme and macOS keymap.
- Do not use the name or logos of JetBrains or IntelliJ. Where IntelliJ names itself ("IntelliJ IDEA can stash…"), the text omits it, as `CheckoutProblemDialog.svelte` already does.
- Porting code keeps the Apache 2.0 attribution: every ported file goes into `NOTICE`, in its table, with the origin classes.
- Commits ending with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Decisions of this plan

Each one departs from IntelliJ or from the approved matrix. The user approves them with the plan.

1. **Only stash until phase 4**, same as in phase 2. Rebase, Smart Merge and editing from the log save with `git stash`, with IntelliJ's messages (`Uncommitted changes before rebase`, `Uncommitted changes before Merge at <fecha>`). IntelliJ's default policy is Shelve.
2. **`GIT_SEQUENCE_EDITOR` in addition to `GIT_EDITOR`.** IntelliJ only sets `GIT_EDITOR`, and a user's `sequence.editor` wins over it. Here both are set. Cost: none visible.
3. **A single merge window.** IntelliJ opens one per file from the Commit view and a modal one from the Conflicts dialog. Here there is only one, and asking for another file while it is open brings it to the front without changing file. Cost: two files cannot be resolved at the same time.
4. **The diff from base to each side uses Myers (`similar`)**, the one `hunks.rs` already uses. IntelliJ uses its "fair" diff with line optimizations. The rest is ported as is: the merging of ranges, the type of each block and the automatic resolution. Cost: in ambiguous cases a block may start or end one line earlier or later than in IntelliJ. The ported tests of `LineComparisonMergeUtilTest` say how many cases differ; if any differs, it is noted.
5. **Recoverable copy before overwriting a file in conflict.** Accept Yours, Accept Theirs and Apply in the merge window copy the working file first. IntelliJ relies on Local History, and here Devin may have written in it.
6. **`Show Details` and `Filter by conflicted file`, to phase 4.** The links that list the commits involved in a conflict need the file history. Cost: one link less in the merge window.
7. **No `Conflicts` tool window and no "Resolving merge conflicts is in progress" editor bar.** The first is off by default in 2023.2.3 (registry key `git.merge.conflicts.toolwindow`), and the second belongs to the IDE's editor, which does not exist here.
8. **Cherry-Pick Selected Changes and Revert Selected Changes with `git apply`.** IntelliJ builds a patch of its own and applies it with its `PatchApplier`, in the changelist that the user chooses. Here `git diff` of the commit is used for those paths, with `-R` when reverting, and `git apply`. If it does not go in cleanly, it is repeated with `--3way`, which leaves the conflict with its stages for the Conflicts dialog. Cost: if a file goes in halfway, git leaves it all or nothing, whereas IntelliJ would offer its patch merge.
9. **The matrix, corrected.** The `git4idea.actions.branch.*` rows without text were left as "Out of scope" by the classifier, but they are the branches popup actions:
   - `GitMergeBranchAction`, `GitRebaseBranchAction`, `GitCheckoutWithRebaseAction` and `GitPullBranchAction$WithMerge` and `$WithRebase` move to phase 3;
   - `GitCheckoutAction`, `GitCheckoutAsNewBranch`, `GitCheckoutFromInputAction`, `GitNewBranchAction`, `GitDeleteBranchAction`, `GitRenameBranchAction`, `GitPushBranchAction` and `GitUpdateSelectedBranchAction` move to phase 2, which already covers them;
   - `GitCompareWithBranchAction` and `GitShowDiffWithBranchAction` move to phase 4.
10. **The `using log` mode of the interactive rebase is the only one that opens the dialog before launching git.** It is the default mode in 2023.2.3. If the range has a merge, a root commit or a `fixup!` or `squash!` subject, it switches to native mode, like IntelliJ. Lines that are not pick, edit, drop, squash, reword or fixup are shown in the free text editor, also like IntelliJ.

## Review Focus

1. **Devin writes in the file while it is open in the merge window.** Apply does not overwrite what is new: the hash of the working file is no longer the one from the load, the window shows `Conflict is not valid and no longer can be resolved.` with `Abort Resolve`, and writes nothing. The same if Accept Theirs arrives from the Commit view with the window open. Tests in Tasks 3 and 6.
2. **Yours and Theirs are inverted during a rebase.** `Accept Yours` leaves the version of the user's branch, which in a rebase is `--theirs`, and the columns and titles follow that same logic. Tests in Tasks 3 and 7.
3. **The git editor with nobody to answer.** If the window is closed or the dialog is cancelled with an interactive rebase waiting, git ends without a half-done rebase and without touching anything, and the task is not left hanging. If the app is closed with the rebase stopped at `edit`, on reopening Continue and Abort are seen. Tests in Tasks 1, 2 and 14.
4. **A merge, rebase, cherry-pick or revert left half-done by another tool or by the app itself on closing.** On opening the repository it is detected, the widget says so (`Rebasing main`) and the popup offers the continue actions. Abort saves the copy first. Test in Task 2.
5. **Rewriting history with uncommitted changes, for example while Devin writes.** Drop, Squash, Reword or a rebase save the changes and give them back. If giving them back produces a conflict, the stash entry is still there and there is `Restore Changes`. Tests in Tasks 9 and 12.

---

### Task 1: Own git editor

Reference:
- `rebase/GitHandlerRebaseEditorManager.java:48-65` (variables and POSIX quotes);
- `rebase/GitRebaseEditorService.kt:17-31`;
- `rebase/GitInteractiveRebaseEditorHandler.java:66-154`;
- `commands/GitImplBase.java:239-279` (removing comments);
- `commands/GitImpl.java:61,712,729,748-779` (`-c core.commentChar=\u0001`).

Model in rama: `askpass.rs` (`client_main`, `Server`, `ClientEnv`) and `commands::start_askpass`.

**Files:**
- Create: `src-tauri/src/editor.rs`, `src/editor/MessageEditorDialog.svelte`, `src/editor/editor.ts`, `src/editor/editor.test.ts`
- Modify: `src-tauri/src/main.rs`, `src-tauri/src/task.rs`, `src-tauri/src/commands.rs`, `src-tauri/src/lib.rs`, `src/App.svelte`, `src/lib/api.ts`

**Interfaces:**
- Produces:
  - `editor::EditKind` equal to `Todo | Message`. It is decided by the file name: `git-rebase-todo` is `Todo`; the rest (`COMMIT_EDITMSG`, `MERGE_MSG`, `SQUASH_MSG`…) is `Message`.
  - `trait editor::EditorHandler: Send + Sync { fn edit(&self, kind: EditKind, text: &str) -> Option<String>; }`. `None` makes the editor exit with code 1.
  - `editor::Trivial`, which returns the text as is, and `editor::Ask`, which asks the main window.
  - `editor::Server::start(on_request: impl Fn(u64, EditKind, String) -> bool + Send + Sync + 'static) -> io::Result<Server>`, `Server::register(handler: Arc<dyn EditorHandler>, git_dir: PathBuf) -> Registration`, `Server::answer(id: u64, text: Option<String>)`.
  - `Registration` carries its token and applies `GIT_EDITOR`, `GIT_SEQUENCE_EDITOR`, `RAMA_EDITOR_SOCKET` and `RAMA_EDITOR_TOKEN` to a `Command`. When dropped, it unregisters.
  - `editor::client_main() -> Option<i32>`, tried in `main.rs` before `askpass::client_main`.
  - `TaskContext::git_editing(args: &[&str], handler: Arc<dyn EditorHandler>) -> Result<GitOutput, GitError>`.
  - `editor::REBASE_CONFIG: [&str; 2] = ["-c", "core.commentChar=\u{1}"]`.
  - `editor::strip_comments(text: &str) -> String`.
  - Event `editor-request { id, kind: "todo" | "message", text }` and command `editor_answer(id: u64, text: Option<String>)`.
  - In TS: `MessageEditorDialog` with title `Git Commit Message`, OK `Continue Rebasing` and the label `Git Root: {0}` (`B:225-229`).

- [ ] **Step 1: Write the failing tests**

Rust, with a real git and a throwaway repository:
- `interactive_rebase_uses_the_handlers_todo`: the handler changes `pick` to `drop` in the second commit and the log confirms it.
- `cancelling_handler_leaves_the_repository_unchanged`: `None` in the todo. Git fails, no `rebase-merge/` is left, and HEAD and the tree are those from before.
- `users_sequence_editor_does_not_win`: with `sequence.editor=false` in the local config, the todo still reaches the handler.
- `reword_message_arrives_as_message`.
- `nothing_is_written_outside_the_git_directory`: a client that asks for `/tmp/x` receives code 1 and the file does not change.
- `foreign_token_is_rejected`.
- `without_a_window_to_answer_it_fails_immediately`: `on_request` returns `false` and git ends in less than 1 s.
- `pull_rebase_interactive_does_not_wait_for_a_terminal`: with `pull.rebase=interactive`, `git pull` receives the todo through the handler. It fixes the finding "pull.rebase=interactive without a rebase editor" of phase 2, Task 12.
- `stripping_comments_uses_the_rebase_character`: lines that start with `\u{1}` are removed; those that start with `#` stay.

TS:
- `a_message_request_opens_the_dialog_with_the_text`.
- `continue_rebasing_answers_the_edited_text`.
- `cancel_answers_null`.

- [ ] **Step 2: Check that they fail**

Run: `pnpm vitest run src/editor && (cd src-tauri && cargo test editor::)`
Expected: FAIL

- [ ] **Step 3: Implement**

`GIT_EDITOR` and `GIT_SEQUENCE_EDITOR` point to `current_exe()`, between POSIX quotes, because git launches them with `sh`.

The client reads `argv[1]` and sends the token and the path over the socket. The server:
1. checks with `canonicalize` that the path is inside the registration's `git_dir`;
2. reads the file and calls `handler.edit`;
3. writes the answer in the file and replies 0 or 1.

`Ask` emits `editor-request` to the main window with `reveal`, like `start_askpass`. If there is no window, it answers `None`. The text of a message is shown without the comment lines.

- [ ] **Step 4: Check that they pass**

Run: `pnpm vitest run && (cd src-tauri && cargo test)`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: own git editor for rebase and messages"
```

### Task 2: Ongoing operation, Continue, Skip and Abort

Reference:
- `repo/GitRepositoryReader.java:164-222` (detection order and `head-name`);
- `branch/GitBranchUtil.java:163-198` (widget text, `B:790-793`);
- `plugin.xml:136-208` (groups `Git.MainMenu.MergeActions`, `Git.MainMenu.RebaseActions` and `Git.Ongoing.Rebase.Actions`);
- `actions/GitRepositoryStateActionGroup.kt:16-23`, `actions/GitOperationActionBase.kt:22-41`;
- `actions/GitAbortOperationAction.kt:55-94` (`B:5-14`);
- `rebase/GitAbortRebaseProcess.java:85-249` (`B:155-168`);
- `actions/GitAbstractRebaseAction.java:33-46`, `GitRebaseContinue.java`, `GitRebaseSkip.java`;
- `rebase/GitRebaseProcess.java:281-305,602-617` (`B:290-291,312-314`);
- `rebase/GitRebaseProblemDetector.java:34-48`.

**Files:**
- Create: `src-tauri/src/operation.rs`, `src-tauri/src/rebase.rs`, `src/operation/operation.ts`, `src/operation/operation.test.ts`
- Modify: `src-tauri/src/branches.rs` (`rebase_onto` and `rebase_abort` move to `rebase.rs`), `src-tauri/src/update.rs`, `src-tauri/src/commands.rs` (`RepoInfo`, `GIT_MENU`), `src-tauri/src/vcs_commands.rs`, `src-tauri/src/lib.rs`, `src/App.svelte`, `src/branches/model.ts`, `src/branches/BranchesPopup.svelte`, `src/branches/actions.ts`, `src/update/flow.ts`, `src/lib/api.ts`

**Interfaces:**
- Consumes: `editor::Ask`, `editor::Trivial`, `TaskContext::git_editing` and `editor::REBASE_CONFIG` from Task 1.
- Produces:
  - `operation::Operation` equal to `Merging | Rebasing | CherryPicking | Reverting`.
  - `operation::current(repo: &Path) -> Result<Option<Operation>, GitError>`. It follows the order of `GitRepositoryReader`: `MERGE_HEAD`, `rebase-apply/` or `rebase-merge/`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`.
  - `operation::RepoState { operation: Option<Operation>, branch: Option<String>, label: String, head: String }`. `label` is the widget text: `Merging main`, `Rebasing main` (with the branch read from `rebase-*/head-name`), `Cherry-picking in main`, `Reverting in main`, or the short hash with a detached HEAD.
  - Command `repo_state() -> RepoState`. `RepoInfo` carries `state: RepoState` instead of `unfinished`, and `repo-changed` makes the interface reread it.
  - `rebase::Outcome` equal to `Done | Conflicts | Editing | Untracked { files: Vec<String> } | Failed(String)`.
  - `rebase::run(repo: &Repo, args: &[&str], handler: Arc<dyn EditorHandler>) -> Result<Outcome, GitError>`. It prepends `REBASE_CONFIG` and reads the output as `GitRebaseProblemDetector` does. If git answers "No changes - did you forget to use 'git add'?", it repeats with `--skip` without asking.
  - `rebase::continue_rebase(repo) -> Result<Outcome, GitError>` and `rebase::skip(repo) -> Result<Outcome, GitError>`, with `editor::Ask`.
  - `rebase::abort(repo) -> Result<(), GitError>`: copy against `rebase-merge/orig-head` or `rebase-apply/orig-head`, and then `rebase --abort`. It fails with `Cannot abort rebase` / `No rebase in progress` if there is no rebase.
  - `operation::abort(repo, kind: Operation) -> Result<(), GitError>`: `merge --abort`, `cherry-pick --abort` or `revert --abort`, with a copy against HEAD, and the rebase goes through `rebase::abort`. It only acts if `current` matches `kind`.
  - Commands `operation_continue() -> rebase::Outcome`, `operation_skip() -> rebase::Outcome` and `operation_abort(kind: Operation, expected_head: String)`.
  - In TS:
    - `ongoingActions(state: RepoState) -> MenuItem[]`: Abort Rebase, Abort Merge, Abort Cherry-Pick, Abort Revert, Continue Rebase and Skip Commit (`B:539-545`), only those of the current state.
    - `abortOperation(kind)` with IntelliJ's confirmation: `Abort Merge` / `Abort merge?` / `Abort`, or `Abort Rebase` / `Abort rebase in {0}?`.
    - `continueRebase(onconflicts: () => Promise<void>)` and `skipCommit(onconflicts)`. With a `Conflicts` result they call `onconflicts`. Task 7 connects it with `resolveConflicts`; until then, the app passes a function that notifies `Rebase stopped due to conflicts`.
    - `notifyRebaseStopped(outcome)` with the texts `Rebase stopped due to conflicts` (with `Resolve…`, `Continue` and `Abort`), `Rebase stopped for editing` (with `Continue` and `Abort`) and `Rebase failed` or `Continue rebase failed` (with `Retry` and `Abort`) (`B:279-308`).

- [ ] **Step 1: Write the failing tests**

Rust:
- `detects_each_operation_in_its_order`: merge with conflict, stopped rebase, cherry-pick with conflict, revert with conflict. With `MERGE_HEAD` and `rebase-merge/` at the same time the merge wins.
- `widget_label_matches_intellij`: `Merging main`, `Rebasing feature` (from `head-name` even if HEAD is detached), `Cherry-picking in main`, `Reverting in main`.
- `abort_merge_uses_merge_abort_and_saves_a_copy`.
- `abort_cherry_pick_and_revert`.
- `abort_rebase_copies_against_the_rebases_orig_head`: fixes the finding of phase 2, Task 9. With `ORIG_HEAD` moved by another command after the rebase stopped, the copy is made against `rebase-merge/orig-head`.
- `abort_without_operation_does_nothing`: fixes the finding of phase 2, Task 12, that `update::abort` does `reset --merge` without `MERGE_HEAD`. The success notification does not appear.
- `abort_with_head_different_from_the_dialog_fails_without_touching`.
- `continue_with_conflicts_gives_conflicts`, `continue_after_resolving_finishes`, `continue_that_asks_for_a_message_asks_it`, `skip_skips_the_commit`.
- `no_changes_skips_by_itself`: a commit that ends up empty when rebasing gets `--skip` and ends with `Done`.
- `rebase_with_untracked_files_that_would_be_overwritten_gives_untracked`.
- `merge_no_commit_stays_as_a_merge_in_progress`: fixes the finding of phase 2, Task 12 (`--no-commit` without Abort or warning), because the state sees it.

TS:
- `widget_shows_rebasing_main`.
- `popup_puts_the_ongoing_actions_on_top`: with a rebase, Abort Rebase, Continue Rebase and Skip Commit go before the fixed rows, and without an operation none appears.
- `git_menu_shows_the_merge_group_only_while_merging`.
- `abort_asks_for_the_intellij_confirmation`, `cancelling_the_confirmation_does_not_abort`.
- `continue_with_conflicts_calls_onconflicts`.
- `stopped_rebase_notifies_with_resolve_continue_and_abort`.
- `opening_a_half_done_repository_shows_the_state`.
- `cannot_complete_update_comes_back_with_abort_even_without_an_entry`: fixes the finding of phase 2, Task 11.

- [ ] **Step 2: Check that they fail**

Run: `pnpm vitest run src/operation src/branches src/update && (cd src-tauri && cargo test operation:: rebase:: branches:: update::)`
Expected: FAIL

- [ ] **Step 3: Implement**

`GIT_MENU` follows the order of `Git.MainMenu`. Merge… and Rebase… come in Tasks 7 and 8, but the abort and continue groups come now, and they are only enabled with their state. `git_menu_action` sends them as `git-action`.

`update::abort` and the `Abort` of the Update and Pull notifications go through `operation::abort` with whatever operation there is. Cancelling an Update halfway still uses `reset --merge`, like `GitMergeUpdater`, because that is what IntelliJ does there.

Move `RebaseOutcome` from `branches.rs` to `rebase::Outcome`, and adapt `LocalCommitsDialog` and `rebaseOnto` of `actions.ts`.

- [ ] **Step 4: Check that they pass**

Run: `pnpm vitest run && (cd src-tauri && cargo test)`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: ongoing state with continue, skip and abort"
```

### Task 3: Conflicts in the core

Reference:
- `merge/GitMergeProvider.java:99-334`;
- `merge/GitMergeUtil.java:82-92,113-141,169-223,333-395`;
- `merge/GitMergeHandler.java:55-186`;
- `util/GitFileUtils.java:257-266`;
- `merge/GitDefaultMergeDialogCustomizer.kt:98-280,343-346`;
- `conflicts/GitConflictsUtil.kt:85-101`;
- `status/GitStagingAreaHolder.java:79-89`;
- texts: `B:1149-1158,1283-1285` and `merge.tool.column.*`.

**Files:**
- Create: `src-tauri/src/conflicts.rs`
- Modify: `src-tauri/src/vcs_commands.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/test_support.rs`, `src-tauri/src/commands.rs` (`lines_rollback`), `src-tauri/src/commit.rs` (`conflicted`), `src/lib/api.ts`

**Interfaces:**
- Consumes: `operation::current` from Task 2.
- Produces:
  - `conflicts::SideStatus` equal to `Modified | Deleted`.
  - `conflicts::Kind` equal to `BothDeleted | BothAdded | BothModified | DeletedByYou | DeletedByThem | AddedByYou | AddedByThem`, with the texts of `conflicts.type.*`.
  - `conflicts::Conflict { path, yours: SideStatus, theirs: SideStatus, kind: Kind }`, already inverted in a rebase.
  - `conflicts::Session { conflicts: Vec<Conflict>, reversed: bool, yours_column: String, theirs_column: String, description: String }`:
    - `yours_column` is `Yours` or `Yours (main)`;
    - `theirs_column` is `Theirs (feature)`, with the branch of `MERGE_HEAD`, the `onto` branch of the rebase or `cherry-pick`;
    - `description` is `Merging branch feature into branch main`, `Rebasing branch feature onto branch main` or the cherry-pick text.
  - `conflicts::session(ctx: &TaskContext) -> Result<Session, GitError>`.
  - `conflicts::Version` equal to `Text(String) | Binary | Missing`.
  - `conflicts::Sides { path, base: Version, yours: Version, theirs: Version, titles: [String; 3], hash: String, reversed: bool }`. `hash` is the SHA-256 of the working file when read.
  - `conflicts::sides(ctx, path: &str) -> Result<Sides, GitError>`, with `git cat-file -p :1:<path>`, `:2:` and `:3:`, and the failures "not at stage" and similar as `Missing`.
  - `conflicts::Side` equal to `Yours | Theirs`.
  - `conflicts::accept(repo: &Repo, paths: &[String], side: Side) -> Result<(), GitError>`. It makes the copy and then `checkout --ours|--theirs --`, with the side inverted in a rebase, and `add --ignore-errors -A -f --` or `rm --`.
  - `conflicts::resolve(repo: &Repo, path: &str, content: &[u8], expected_hash: &str) -> Result<(), ConflictError>`. It makes the copy, writes and `add -f --`. `ConflictError::Outdated` if the file's hash is no longer `expected_hash`.
  - Commands `conflicts_session() -> Session`, `conflict_sides(path) -> Sides`, `conflicts_accept(paths, side)` and `conflict_resolve(path, content, expected_hash)`.
  - `TempRepo::conflict(file) -> ()` in `test_support.rs`: leaves a merge with a conflict in `file` without failing on git's exit code. It fixes the finding of phase 1, Task 2.

- [ ] **Step 1: Write the failing tests**

- `lists_the_seven_kinds`: one conflict of each type in the same merge, with `ls-files --unmerged` and the XY of `status`.
- `sides_reads_the_three_stages`.
- `deleted_by_them_gives_missing_in_theirs`.
- `binary_gives_binary`: a PNG in conflict.
- `missing_base_like_intellij`: in a `both added` conflict the base follows `GitMergeUtil.java:169-194`. Read it before writing the assertion.
- `in_rebase_sides_and_columns_are_swapped`: `yours` is the branch being rebased, and `Accept Yours` leaves its content.
- `accept_theirs_of_a_deletion_does_rm`.
- `accept_saves_a_copy_of_the_file_with_markers`.
- `resolve_writes_and_marks_resolved`.
- `resolve_with_old_hash_does_not_write`: Devin changed the file after it was read; it gives `Outdated` and the file stays the same.
- `path_with_leading_dash_is_not_an_option`: a file `-x` in conflict is accepted fine.
- `rolling_back_lines_rejects_a_conflicted_file`: fixes the finding of phase 1, Task 8, with the `has_unmerged` guard in `lines_rollback`.
- `conflict_in_a_child_folder_is_seen_with_a_folder_pathspec`: fixes the finding of phase 1, Task 11 (`conflicted` with pathspec `d/f`).

- [ ] **Step 2: Check that they fail**

Run: `cd src-tauri && cargo test conflicts:: commit:: commands::`
Expected: FAIL

- [ ] **Step 3: Implement**

`session` and `sides` are reads, of type `TaskKind::Read`. `accept` and `resolve` are writes: `flush_diff_window`, `RepoLocks` and `TaskKind::Write`.

The panel titles follow `GitDefaultMergeDialogCustomizer`:
- `Changes from main` / `Result` / `Changes from feature` in a merge;
- `Rebasing abc1234 from feature` / `Result` / `Already rebased commits and commits from main` in a rebase;
- `Local Changes` / `Result` / `Changes from cherry-pick abc1234` in a cherry-pick.

- [ ] **Step 4: Check that they pass**

Run: `cd src-tauri && cargo test`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: read, accept and resolve conflicts"
```

### Task 4: Three-way line merge

Reference:
- `platform/util/diff/src/com/intellij/diff/comparison/ComparisonMergeUtil.java` (`FairMergeBuilder`, `IgnoringChangeBuilder`);
- `platform/util/diff/src/com/intellij/diff/util/MergeRangeUtil.java` (`getMergeType`, `getLineMergeType`);
- `platform/util/diff/src/com/intellij/diff/comparison/MergeResolveUtil.kt` (`tryResolve`, `tryGreedyResolve`, and which one `DiffConfig.USE_GREEDY_MERGE_MAGIC_RESOLVE` chooses);
- `platform/util/diff/src/com/intellij/diff/comparison/ByLineRt.java`;
- tests: `platform/diff-impl/tests/testSrc/com/intellij/diff/comparison/LineComparisonMergeUtilTest.kt`, `ComparisonMergeUtilTestBase.kt` and `MergeResolveUtilTest.kt`.

**Files:**
- Create: `src-tauri/src/merge3.rs`
- Modify: `src-tauri/src/conflicts.rs` (`Sides` carries the blocks), `src-tauri/src/vcs_commands.rs`, `src-tauri/src/lib.rs`, `NOTICE`, `src/lib/api.ts`

**Interfaces:**
- Produces:
  - `merge3::Whitespace` equal to `Default | Trim | Ignore`.
  - `merge3::BlockKind` equal to `Inserted | Deleted | Modified | Conflict`.
  - `merge3::Block { left: (u32, u32), base: (u32, u32), right: (u32, u32), kind: BlockKind, left_changed: bool, right_changed: bool, auto: Option<String> }`:
    - the ranges are of lines, half-open, starting at 0;
    - `auto` is the text of the automatic resolution when the block is a conflict that can be resolved.
  - `merge3::blocks(left: &str, base: &str, right: &str, policy: Whitespace) -> Vec<Block>`.
  - `merge3::try_resolve(left: &str, base: &str, right: &str) -> Option<String>`.
  - `conflicts::Sides.blocks: Vec<Block>` with `Whitespace::Default`.
  - Command `merge_blocks(left, base, right, policy) -> Vec<Block>`, to change policy without rereading.

- [ ] **Step 1: Port IntelliJ's tests**

Turn each case of `LineComparisonMergeUtilTest` and `MergeResolveUtilTest` into a Rust test of the same name in `snake_case`, with the texts and the expected results as they are. Add:
- `block_without_conflict_on_each_side`;
- `same_change_on_both_sides_is_not_a_conflict`;
- `deletion_on_one_side_and_change_on_the_other_is_a_conflict_without_auto`;
- `ignoring_whitespace_merges_blocks`.

- [ ] **Step 2: Check that they fail**

Run: `cd src-tauri && cargo test merge3::`
Expected: FAIL

- [ ] **Step 3: Implement**

1. Take two line diffs with `similar` (`Algorithm::Myers`): from base to left and from base to right. For `Trim` and `Ignore`, compare lines normalized like `ComparisonUtil.isEqualTexts`.
2. Cross the unchanged ranges like `FairMergeBuilder.add`, and what is left between two equal ranges is a block.
3. With a policy other than `Default`, separate the ignored blocks like `IgnoringChangeBuilder`.
4. Compute the type of each block with `getMergeType`, and `auto` with the resolution that `DiffConfig` chooses.

The ported tests that fail only because of decision 4 (Myers versus "fair") stay, marked with `#[ignore = "decision 4: …"]` and the reason, and are counted in the task's report. Any other failure is fixed.

Add `merge3.rs` to the `NOTICE` table with `ComparisonMergeUtil`, `MergeRangeUtil`, `MergeResolveUtil` and the ported tests.

- [ ] **Step 4: Check that they pass**

Run: `cd src-tauri && cargo test`
Expected: PASS, with the `#[ignore]` justified.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: three-way line merge ported from IntelliJ"
```

### Task 5: Three-pane merge editor

Reference:
- `platform/diff-impl/src/com/intellij/diff/merge/MergeThreesideViewer.java:137,177-224,277-316,358-447,592,635-650,739-882,917-920,1347-1374`;
- `TextMergeChange.java:47,104-110,183-251`;
- `MergeModelBase.java:322-331`;
- texts of `platform/diff-api/resources/messages/DiffBundle.properties` (`action.presentation.*`, `merge.dialog.*`, `merge.differences.status.text`, `merge.all.changes.processed.message.text`) and those of the actions `Diff.ApplyNonConflicts*`, `Diff.MagicResolveConflicts`, `Diff.ApplyLeftSide` and similar in `platform/platform-resources-en/src/messages/ActionsBundle.properties`.

**Files:**
- Create: `src/merge/editors.ts`, `src/merge/editors.test.ts`, `src/merge/model.ts`, `src/merge/model.test.ts`, `src/merge/gutter.ts`
- Modify: `src/diff/theme.ts` (colors of the three panes)

**Interfaces:**
- Consumes: `merge3::Block` (as `MergeBlock` in `api.ts`) from Task 4.
- Produces:
  - `model.ts`:
    - `BlockState { block: MergeBlock; output: { from: number; to: number }; left: boolean; right: boolean; edited: boolean }`. `left` and `right` say whether that side was already applied or ignored.
    - `initialBlocks(blocks: MergeBlock[]): BlockState[]`.
    - `counts(states: BlockState[]): { changes: number; conflicts: number }`.
    - `statusText(counts)`: `3 changes. 1 conflict.` and the rest of the forms of `merge.differences.status.text`.
  - `editors.ts`:
    - `class MergeEditors`, with constructor `(parent: HTMLElement, data: { left: string; base: string; right: string; blocks: MergeBlock[]; titles: [string, string, string] }, handlers: { onchange: () => void; onallprocessed: () => void })`.
    - Methods:
      - `apply(index: number, side: "left" | "right", whole: boolean)`;
      - `ignore(index: number, side: "left" | "right", whole: boolean)`;
      - `resolve(index: number)`;
      - `applyNonConflicting(which: "left" | "all" | "right")`;
      - `resolveAutomatically()`;
      - `navigate(direction: 1 | -1)`;
      - `result(): string`;
      - `modified(): boolean`;
      - `counts()`;
      - `setWhitespace(policy: Whitespace, blocks: MergeBlock[])`;
      - `destroy()`.
  - Central pane: the text of the base, without markers, like `setInitialOutputContent`, and without being able to undo that load.

- [ ] **Step 1: Write the failing tests**

Choose the cases of `platform/diff-impl/tests/testSrc/com/intellij/diff/merge/MergeTest.kt` that do not depend on IntelliJ's editor and turn them into `MergeEditors` tests in jsdom. Also:
- `center_starts_with_the_base`;
- `apply_left_copies_its_lines`;
- `apply_both_sides_of_a_conflict_leaves_left_and_then_right`;
- `apply_with_modifier_resolves_the_whole_block`;
- `ignore_one_side_of_a_conflict_leaves_the_other_pending`;
- `editing_inside_a_block_marks_it_edited_and_resolve_no_longer_applies`;
- `ranges_follow_the_text_when_edited_before`;
- `apply_non_conflicting_left_all_right`: All takes the side that has the change;
- `resolve_automatically_only_touches_conflicts_with_auto_not_edited`;
- `counter_and_status_text`;
- `everything_processed_notifies_once`;
- `navigating_jumps_between_unresolved_blocks`;
- `changing_whitespace_recalculates_the_blocks`;
- `margin_shows_arrow_accept_cross_ignore_and_wand_resolve_with_their_tooltip`: `Ctrl+click to resolve conflict` in the conflicts.

- [ ] **Step 2: Check that they fail**

Run: `pnpm vitest run src/merge`
Expected: FAIL

- [ ] **Step 3: Implement**

Use three `EditorView`:
- the side ones, read-only;
- the central one, editable, with a `StateField` of `BlockState` whose ranges follow the text with `ChangeSet.mapPos`.

Each action is a transaction with its name (`Accept change`, `Ignore change`, `Resolve conflict`, `Apply non conflicted changes`, `Resolve simple conflicted changes`). That way they can be undone one at a time.

The scroll of the three is synchronized, and the two dividers are drawn like `drawDivider` of `src/diff/editor.ts`. Generalize to several panes whatever is needed from those two functions; do not copy the code.

Highlight the words inside each block with `presentableDiff`, as the diff already does.

- [ ] **Step 4: Check that they pass**

Run: `pnpm vitest run`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: three-pane merge editor"
```

### Task 6: Merge window

Reference:
- `platform/diff-impl/src/com/intellij/diff/merge/MergeRequestProcessor.java:197-318,469-529`;
- `MergeUtil.java:68-73,115-164`;
- `TextMergeViewer.java:40-94`;
- `BinaryMergeTool.java:104-108`;
- `platform/diff-impl/src/com/intellij/diff/requests/TextMergeRequestImpl.java` (`applyResult`);
- `platform/vcs-impl/src/com/intellij/openapi/vcs/merge/MultipleFileMergeDialog.kt:386-475`;
- the default value of `isAutoApplyNonConflictedChanges`, from the jar (see "Reference read").

**Files:**
- Create: `src/merge/MergeWindow.svelte`, `src/merge/MergeWindow.test.ts`, `src/merge/window.ts`
- Modify: `src/main.ts`, `src-tauri/src/commands.rs` (`merge` window), `src-tauri/src/lib.rs` (`on_window_event`), `src-tauri/src/settings.rs`, `src/lib/api.ts`

**Interfaces:**
- Consumes: `MergeEditors` from Task 5; `conflict_sides`, `conflict_resolve`, `conflicts_accept` and `merge_blocks` from Tasks 3 and 4.
- Produces:
  - `MERGE_WINDOW = "merge"` and `MERGE_WINDOW_URL = "index.html?window=merge"`.
  - Command `merge_window_open(path: String)`. It opens the window, or brings it to the front without changing file (decision 3).
  - Event `merge-done { path, result: "resolved" | "yours" | "theirs" | "cancelled" }` to the main window.
  - `Settings.merge_auto_apply: bool` and `Settings.merge_whitespace: Whitespace`, with IntelliJ's default values.

- [ ] **Step 1: Write the failing tests**

- `title_merge_revisions_for_path` (`Merge Revisions for {0}`).
- `buttons_accept_left_accept_right_cancel_apply`: on macOS, Cancel goes before Apply, and Apply is the default button, also with ⌘⏎.
- `apply_without_pending_resolves_and_closes`.
- `apply_with_pending_asks_apply_changes_and_mark_resolved`: the text and the buttons of `MergeUtil`.
- `accept_left_with_changes_asks_to_discard`: `There are unsaved changes in the result file. Discard changes and accept left anyway?`, with `Discard Changes and Accept Left` and `Continue Merge`.
- `cancel_without_changes_closes_without_asking` and `cancel_with_changes_asks_cancel_merge`.
- `closing_the_window_is_cancelling`.
- `file_changed_outside_shows_conflict_is_not_valid_and_does_not_write`: `Outdated` or `repo-changed` arrives with another hash; the red bar appears with `Abort Resolve` and Apply is left disabled.
- `binary_only_offers_accept_left_right_and_cancel`.
- `everything_processed_shows_save_changes_and_finish_merging`.
- `apply_the_non_conflicting_on_opening_if_the_setting_is_on`.
- `changing_whitespace_with_changes_asks_update_highlighting_settings`.
- `asking_for_another_file_brings_the_window_forward_without_changing_it`.

- [ ] **Step 2: Check that they fail**

Run: `pnpm vitest run src/merge`
Expected: FAIL

- [ ] **Step 3: Implement**

There are two bars:
- top: the whitespace group, `Apply non-conflicting changes:` with `Left`, `All` and `Right`, `Resolve Automatically`, the navigation and the status text;
- bottom: Accept Left and Accept Right on the left, Cancel and Apply on the right.

Accept Left and Accept Right call `conflicts_accept` with the corresponding side, taking `reversed` into account. Apply calls `conflict_resolve` with the `hash` of the load.

The window is created like the diff one (`diff_window_open`, `commands.rs:1108`), and `on_window_event` treats its closing as Cancel. When another repository is opened, the window is destroyed like the diff one (`commands.rs:899`).

- [ ] **Step 4: Check that they pass**

Run: `pnpm vitest run && (cd src-tauri && cargo test)`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: merge window"
```

### Task 7: Conflicts dialog and conflicts in the Commit view

Reference:
- `platform/vcs-impl/src/com/intellij/openapi/vcs/merge/MultipleFileMergeDialog.kt:80-374`;
- `platform/vcs-api/src/com/intellij/openapi/vcs/merge/MergeDialogCustomizer.kt:44-74`;
- `merge/GitConflictResolver.java:90-95,188-249`;
- `merge/GitMergeCommittingConflictResolver.java`;
- `merge/GitMerger.java:52-73`;
- `merge/GitConflictActions.kt:24-87`;
- `index/actions/GitStageConflictActions.kt:19-66`;
- `actions/GitResolveConflictsAction.java`;
- `checkin/GitCheckinEnvironment.java:115-140`;
- `plugin.xml:148,265,348-350,362-368,384-387`;
- texts: `multiple.file.merge.*`, `changes.nodetitle.merge.conflicts*` (VcsBundle), `merge.unresolved.conflicts.remaining.notification.*` and `conflicts.*` (`B:`).

**Files:**
- Create: `src/conflicts/ConflictsDialog.svelte`, `src/conflicts/flow.ts`, `src/conflicts/flow.test.ts`, `src/conflicts/ConflictsDialog.test.ts`
- Modify: `src/commit/tree.ts`, `src/commit/CommitView.svelte`, `src/commit/staging.ts`, `src/commit/StagingView.svelte`, `src/App.svelte`, `src-tauri/src/commit.rs` (merge message), `src-tauri/src/commands.rs` (`GIT_MENU`), `src/lib/api.ts`

**Interfaces:**
- Consumes: `conflicts_session`, `conflicts_accept` and `merge_window_open`, with its `merge-done`, from Tasks 3 and 6; `continueRebase` from Task 2.
- Produces:
  - `resolveConflicts(options: { onresolved?: () => Promise<void> }): Promise<"resolved" | "pending">`. It opens the dialog, and when closing it with pending conflicts it notifies `There are pending unresolved conflicts.` with `Resolve…`. When reopening from that notification, the title is `Pending Unresolved conflicts`.
  - `onresolved`, which each flow passes:
    - rebase: `operation_continue`;
    - merge started by the app: `merge_commit()`;
    - cherry-pick and revert: the commit of Task 10.
  - Command `merge_commit()`: `git commit -F .git/MERGE_MSG --`, or `-m "Merge branch '<rama>' of <ruta> with conflicts."` if there is no `MERGE_MSG`, like `GitMerger.mergeCommit`.
  - Command `merge_message() -> Option<String>`: reads `MERGE_MSG` or `SQUASH_MSG` for the Commit view, like `getDefaultMessageFor`.
  - `Merge Conflicts` node with the `Resolve` link in the changelists tree (new `NodeKind` `conflicts`).
  - Actions `Merge…`, `Accept Theirs` and `Accept Yours` at the start of the context menus of Changes and of Staging, only for files in conflict.
  - `Resolve Conflicts…` in the Git menu, after Rebase.

- [ ] **Step 1: Write the failing tests**

- `conflicts_dialog_columns_name_yours_theirs_with_branches`.
- `modified_and_deleted_states`.
- `merge_is_the_default_button_and_double_click_launches_it`.
- `merge_disabled_if_both_deleted`.
- `group_files_by_directory_is_remembered`.
- `accept_yours_resolves_and_removes_the_row`.
- `resolving_everything_closes_by_itself_and_calls_onresolved`.
- `closing_with_pending_notifies_and_resolve_reopens_with_another_title`.
- `in_rebase_the_columns_are_inverted`.
- `merge_conflicts_node_with_resolve_in_changelists`.
- `context_menu_of_a_conflict_starts_with_merge_accept_theirs_accept_yours`.
- `staging_unmerged_offers_the_same_actions`.
- `commit_view_proposes_the_merge_message`.
- `resolve_conflicts_only_shows_with_conflicts`.
- `everything_resolved_from_the_rebase_notification_asks_continue_rebase`: `All conflicts have been resolved. Do you want to continue rebase?`.
- `continue_rebase_with_conflicts_opens_conflicts`: the `onconflicts` of Task 2 is already `resolveConflicts`.
- Rust: `merge_commit_uses_merge_msg`, `merge_commit_without_merge_msg_uses_intellijs_text`, `merge_message_reads_squash_msg_if_there_is_no_merge`.

- [ ] **Step 2: Check that they fail**

Run: `pnpm vitest run src/conflicts src/commit && (cd src-tauri && cargo test commit::)`
Expected: FAIL

- [ ] **Step 3: Implement**

The dialog lives in the main window, with `lib/Dialog.svelte`. Its title is `Conflicts`, its only bottom button is `Close`, and the description of `Session` goes at the top.

On receiving `merge-done`, the dialog rereads `conflicts_session`. The three points that open the dialog must be the same flow: the menu, the notification and the node's link.

- [ ] **Step 4: Check that they pass**

Run: `pnpm vitest run && (cd src-tauri && cargo test)`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: conflicts dialog and actions in the Commit view"
```

### Task 8: Merge… and Rebase… dialogs

Reference:
- `actions/GitMerge.java:69-110`;
- `merge/GitMergeDialog.kt:74-87,168-178,218-260,307-313,382-446`;
- `merge/GitMergeOption.kt`;
- `config/GitMergeSettings.java`;
- `actions/GitMergeAction.java:101-237`;
- `actions/GitRebase.java:36-66`;
- `rebase/GitRebaseDialog.kt:158-254,474-513,627-652`;
- `rebase/GitRebaseOption.kt`;
- `branch/GitRebaseParams.kt:57-92`;
- `config/GitVersionSpecialty.java:273-277`;
- texts: `B:106-129,187-199,1393-1401`.

**Files:**
- Create: `src-tauri/src/merge.rs`, `src/git/MergeDialog.svelte`, `src/git/RebaseDialog.svelte`, `src/git/options.ts`, `src/git/options.test.ts`, `src/git/dialogs.test.ts`
- Modify: `src-tauri/src/settings.rs` (per repository), `src-tauri/src/rebase.rs`, `src-tauri/src/vcs_commands.rs`, `src-tauri/src/commands.rs` (`GIT_MENU`), `src-tauri/src/lib.rs`, `src/App.svelte`, `src/lib/api.ts`

**Interfaces:**
- Consumes: `rebase::run` from Task 2 and `resolveConflicts` from Task 7.
- Produces:
  - `merge::MergeOption` equal to `NoFf | FfOnly | Squash | Message | NoCommit | NoVerify`.
  - `merge::merge(repo: &Repo, branch: &str, options: &[MergeOption], message: Option<&str>, expected_head: &str) -> Result<MergeOutcome, GitError>`, with `git merge <options in order> [-m <msg>] <branch>`.
  - `merge::MergeOutcome` equal to `Merged { files: u32, commits: u32 } | UpToDate | Conflicts | LocalChanges { files } | Untracked { files } | Unmerged | Failed(String)`.
  - `merge::unmerged_branches(ctx) -> Vec<String>`: `branch --format=%(refname) --no-color --all --no-merged`.
  - `RebaseParams { upstream: Option<String>, onto: Option<String>, branch: Option<String>, interactive: bool, rebase_merges: bool, keep_empty: bool, root: bool }`.
  - `rebase::rebase(repo, params: &RebaseParams, expected_head: &str) -> Result<Outcome, GitError>`. It builds `rebase <flags> [--onto X] <upstream> [--root] [<branch>]`; with `interactive`, the handler is the one from Task 14 (`Ask` until then).
  - `RepoSettings.merge: { branch: Option<String>, options: Vec<MergeOption> }` and `RepoSettings.rebase: { options: Vec<String>, new_base: Option<String> }`.
  - In TS:
    - `mergeOptionsCompatible(selected): Set<MergeOption>` with the incompatibilities of `GitMergeOption.kt:18-34`;
    - `rebaseOptionsCompatible(selected, gitVersion)`;
    - `MergeDialog` (title `Merge into main`, OK `Merge`);
    - `RebaseDialog` (title and OK `Rebase`).

- [ ] **Step 1: Write the failing tests**

Rust:
- `merge_with_each_option_launches_the_intellij_command`: with the command log of `Harness`.
- `fast_and_committing_merges_count_files_and_commits`.
- `merge_with_conflict_gives_conflicts_and_leaves_merge_head`.
- `merge_with_changes_that_would_be_overwritten_gives_local_changes`.
- `merge_with_untracked_gives_untracked`.
- `merge_of_branch_with_leading_dash_is_rejected`.
- `rebase_onto_and_root_build_the_command_in_order`.
- `rebase_with_conflict_gives_conflicts`.
- `merge_and_rebase_check_head`.
- `no_merged_lists_local_and_remote_branches`.

TS:
- `incompatible_options_are_disabled`.
- `no_verify_only_with_git_2_24`.
- `message_visible_only_with_m`.
- `empty_branch_gives_the_merge_branch_should_be_selected`.
- `already_merged_branch_gives_already_merged`.
- `remembers_branch_and_options_not_the_message`.
- `rebase_onto_moves_the_value_between_fields`.
- `root_disables_upstream`.
- `rebase_remembers_flags_and_new_base_if_it_resolves`.
- `merge_hidden_during_a_merge`.
- `conflict_opens_conflicts_and_on_resolving_does_merge_commit_except_no_commit`.
- `result_n_files_updated_in_m_commits`.

- [ ] **Step 2: Check that they fail**

Run: `pnpm vitest run src/git && (cd src-tauri && cargo test merge:: rebase::)`
Expected: FAIL

- [ ] **Step 3: Implement**

`Merge…` and `Rebase…` go into `GIT_MENU` in the order of `Git.MainMenu`: Merge, the merge group, Rebase and the rebase submenu.

Errors use the literal texts: `Git Merge Failed`, `Your local changes will be overwritten by merge.\nCommit, stash, or revert them to proceed.` with `View them` (which opens the list of files), and `Untracked Files Prevent Merge`. This flow does not offer Smart merge, like IntelliJ.

- [ ] **Step 4: Check that they pass**

Run: `pnpm vitest run && (cd src-tauri && cargo test)`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: Merge and Rebase dialogs"
```

### Task 9: Merge, Rebase, Checkout and Rebase and Pull into from the branches popup

Reference:
- `ui/branch/GitBranchPopupActions.java:798-873,950-978,1055-1105`;
- `actions/branch/*.kt`;
- `branch/GitMergeOperation.java:87,143-322`;
- `branch/GitBranchWorker.java:150-163`;
- `ui/branch/GitCheckoutAndRebaseRemoteBranchWorkflow.kt:13-56`;
- `branch/GitSmartOperationDialog.java:79-99`;
- `rebase/GitRebaseProcess.java:118-454,490-550`;
- `rebase/GitRebaseUtils.java:126-169`;
- `update/GitUpdateExecutionProcess.kt`;
- texts: `B:169-173,279-311,368-375,899-942,1169-1190,1216-1218`.

**Files:**
- Create: `src/branches/merging.ts`, `src/branches/merging.test.ts`, `src/branches/PublishedCommitsDialog.svelte`
- Modify: `src-tauri/src/merge.rs`, `src-tauri/src/rebase.rs`, `src-tauri/src/preserve.rs`, `src-tauri/src/pull.rs`, `src-tauri/src/vcs_commands.rs`, `src-tauri/src/lib.rs`, `src/branches/model.ts`, `src/branches/actions.ts`, `src/branches/BranchDialogs.svelte`, `src/branches/CheckoutProblemDialog.svelte`, `src/lib/api.ts`

**Interfaces:**
- Consumes: `merge::merge`, `rebase::rebase` and `rebase::Outcome` from Tasks 2 and 8, `resolveConflicts` from Task 7, and `update::update_project` and `pull::pull` from phase 2.
- Produces:
  - `merge::smart_merge(repo, branch, expected_head) -> Result<MergeOutcome, GitError>`: saves with `preserve` (`Uncommitted changes before Merge at <fecha>`), does the merge and restores. With conflicts, it restores after they are resolved.
  - `rebase::Process`, the common flow of `GitRebaseProcess`:
    1. checks the state with the texts of `isRebaseAllowed`;
    2. saves the changes (`Uncommitted changes before rebase`), and again if git sees the tree as dirty;
    3. launches the rebase;
    4. on finishing successfully, restores.
    - With conflicts or stopped at `edit`, the changes stay saved and the warning says so: `Local changes were stashed before rebase.`
  - `rebase::published(ctx, upstream, branch) -> Option<String>`: the remote protected branch that already contains some commit of `upstream..branch`, with the patterns of phase 2.
  - Commands `branch_merge(branch, smart: bool, expected_head)`, `branch_rebase(onto, branch: Option<String>, expected_head)` (Rebase and Checkout and Rebase) and `branch_pull_into(remote_branch, method: UpdateMethod)`.
  - In `branchActions`:
    - `Merge 'x' into 'y'` on the branches that are not the current one;
    - `Rebase 'y' onto 'x'`, disabled with a detached HEAD (`Rebase is not possible in the detached HEAD state`);
    - `Checkout and Rebase onto 'y'`;
    - for remote branches, `Pull into 'y' Using Merge` and `Pull into 'y' Using Rebase`.

- [ ] **Step 1: Write the failing tests**

Rust:
- `merge_into_is_merge_without_arguments`.
- `smart_merge_saves_merges_and_restores`.
- `smart_merge_with_conflict_restores_after_resolving`.
- `rebase_saves_any_change_and_restores`.
- `rebase_retries_once_if_git_sees_a_dirty_tree`.
- `devin_writes_during_the_rebase`: a fake git modifies a file while the rebase lasts. Restoring gives a conflict, the entry stays and nothing is lost.
- `checkout_and_rebase_local_is_rebase_head_x`.
- `checkout_and_rebase_remote_creates_the_local_one_first`.
- `published_sees_a_commit_in_origin_main`.
- `rebase_not_allowed_with_a_merge_in_progress_gives_intellijs_text`.
- `pull_into_does_fetch_and_merge_of_the_remote`.
- `pull_into_rebase_goes_through_update_with_rebase`.

TS:
- `merge_into_does_not_appear_on_the_current`.
- `names_in_single_quotes`.
- `changes_that_would_be_overwritten_offer_smart_merge_and_dont_merge`.
- `successful_merge_notifies_merged_x_to_y_with_delete_x`: without `Delete x` if it is remote or protected.
- `already_up_to_date`.
- `merge_with_conflicts_opens_conflicts_and_if_any_remain_warns_merged_with_conflicts`.
- `unmerged_files_gives_cannot_merge_because_of_unmerged_files`.
- `published_rebase_asks_rebasing_published_commit_with_cancel_by_default`.
- `successful_rebase_notifies_rebased_x_on_y`.
- `checkout_and_rebase_notifies_checked_out_and_rebased`.
- `rebase_with_conflicts_follows_task_2`.
- `pull_into_only_on_remotes`.

- [ ] **Step 2: Check that they fail**

Run: `pnpm vitest run src/branches && (cd src-tauri && cargo test merge:: rebase:: pull::)`
Expected: FAIL

- [ ] **Step 3: Implement**

`CheckoutProblemDialog` serves for Smart Merge: the Force button is hidden, and the texts are `Git Merge Problem`, `Smart Merge` and `Don't Merge`.

`rebaseOnto` of phase 2 (`LocalCommitsDialog`) goes through `rebase::Process`, so that it has the same warning and the same saves.

The `Delete x` link uses the branch deletion of phase 2.

- [ ] **Step 4: Check that they pass**

Run: `pnpm vitest run && (cd src-tauri && cargo test)`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: merge, rebase and pull into from the branches popup"
```

### Task 10: Cherry-pick and Revert from the log

Reference:
- `cherrypick/GitCherryPicker.java:61-163`;
- `GitApplyChangesProcess.kt:77-406`;
- `GitProtectedBranches.kt:15-41`;
- `config/GitVcsSettings.java:258-263`, `config/GitVcsOptions.kt:80-81`, `config/GitVcsPanel.kt:64`;
- `revert/GitRevertAction.kt`, `revert/GitRevertOperation.kt:29-50`;
- `platform/dvcs-impl/src/com/intellij/dvcs/cherrypick/VcsCherryPickAction.java` and `VcsCherryPickManager.java` (order of the commits and enabling);
- `commands/GitSimpleEventDetector.java:36-37`;
- texts: `B:22-38,621-622,660,1378-1389` and `platform/dvcs-api/resources/messages/DvcsBundle.properties:71-77`.

**Files:**
- Create: `src-tauri/src/apply_commits.rs`, `src/log/applying.ts`, `src/log/applying.test.ts`
- Modify: `src/log/contextMenu.ts`, `src/log/LogActions.svelte`, `src-tauri/src/settings.rs`, `src/settings/` (the Git screen), `src-tauri/src/vcs_commands.rs`, `src-tauri/src/lib.rs`, `src/lib/api.ts`

**Interfaces:**
- Consumes: `resolveConflicts` from Task 7, and `operation_abort` and `ongoingActions` from Task 2.
- Produces:
  - `apply_commits::Kind` equal to `CherryPick | Revert`.
  - `apply_commits::run(repo, kind, hashes: &[String], expected_head) -> Result<ApplyResult, GitError>`. It applies one commit at a time, in the order of `VcsCherryPickManager` for cherry-pick and from the most recent to the oldest for revert. It stops at the first failure.
  - `ApplyResult { applied: Vec<String>, skipped: Vec<String>, stopped: Option<Stop> }`, with `Stop` equal to `Conflicts { hash } | LocalChanges { files } | Untracked { files } | Failed(String)`.
  - `apply_commits::commit_after_conflicts(repo, kind, hash) -> Result<(), GitError>`: removes `CHERRY_PICK_HEAD` and commits with the proposed message (the one from `GitCherryPicker.java:128-134` or the one from `GitRevertOperation.kt:45-50`). The author is kept in cherry-pick.
  - `Settings.cherry_pick_suffix: bool`, `true` by default, with the text `Add the 'cherry-picked from <hash>' suffix when picking commits pushed to protected branches`.
  - Log entries: `Cherry-Pick` and `Revert Commit` / `Revert Commits`, with their disabled text `Reverting merge commits is not allowed`.

- [ ] **Step 1: Write the failing tests**

Rust:
- `cherry_pick_of_a_published_commit_carries_x`.
- `unpublished_cherry_pick_carries_no_x`.
- `setting_off_never_carries_x`.
- `several_commits_in_intellij_order`.
- `empty_commit_is_skipped_and_it_goes_on`.
- `conflict_stops_and_leaves_cherry_pick_head`.
- `commit_after_conflicts_keeps_the_author_and_the_suffix`.
- `revert_of_several_goes_from_the_most_recent`.
- `revert_with_conflict_proposes_revert_quotes`.
- `revert_of_a_merge_is_rejected`.
- `local_changes_that_would_be_overwritten_give_local_changes`.

TS:
- `cherry_pick_successful_notification_with_one_line_per_commit`.
- `applied_2_commits_from_3_with_skipped`.
- `nothing_to_cherry_pick`.
- `with_conflicts_opens_conflicts_and_on_resolving_makes_the_commit`.
- `pending_conflicts_warn_performed_with_conflicts_with_resolve_and_abort_cherry_pick`.
- `however_succeeded_for_the_following_commits`.
- `revert_disabled_on_merges_with_its_text`.
- `cherry_pick_suffix_setting_on_the_git_screen`.

- [ ] **Step 2: Check that they fail**

Run: `pnpm vitest run src/log && (cd src-tauri && cargo test apply_commits::)`
Expected: FAIL

- [ ] **Step 3: Implement**

`git cherry-pick [-x] <hash>` and `git revert <hash>`, one commit per command. With the staging area active, after resolving no commit is made: the Staging view is left with the proposed message and the sequence stops, like `GitApplyChangesProcess.kt:251-261`.

- [ ] **Step 4: Check that they pass**

Run: `pnpm vitest run && (cd src-tauri && cargo test)`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: cherry-pick and revert from the log"
```

### Task 11: Cherry-Pick Selected Changes and Revert Selected Changes

Reference: `platform/vcs-impl/src/com/intellij/openapi/vcs/changes/actions/RevertSelectedChangesAction.java` and `RevertCommittedStuffAbstractAction.java`, `platform/vcs-impl/resources/META-INF/VcsActions.xml:273-335`, and the texts `changes.progress.title.choice.revert.apply.changes`, `changes.dialog.message.failed.to.revert.apply.changes`, `changes.revert.apply.change.list.name` and `revert.changes.changelist.chooser.title` of VcsBundle.

**Files:**
- Modify: `src-tauri/src/apply_commits.rs`, `src/log/ChangesPanel.svelte` (per-file context menu), `src/log/applying.ts`, `src/log/applying.test.ts`, `src-tauri/src/vcs_commands.rs`, `src-tauri/src/lib.rs`, `src/lib/api.ts`

**Interfaces:**
- Consumes: the changelist selector of phase 1 and `resolveConflicts` from Task 7.
- Produces:
  - `apply_commits::apply_paths(repo, hash: &str, paths: &[String], reverse: bool) -> Result<PathsResult, GitError>`, with `PathsResult` equal to `Applied | Conflicts | Failed(String)`.
  - A context menu in `ChangesPanel` with `Revert Selected Changes` and `Cherry-Pick Selected Changes`.

- [ ] **Step 1: Write the failing tests**

- `applying_paths_leaves_local_changes_unstaged`.
- `reverting_paths_uses_minus_r`.
- `if_it_does_not_apply_cleanly_it_retries_with_3way_and_leaves_a_conflict`.
- `the_suggested_changelist_is_revert_colon_name`.
- `choosing_a_list_moves_the_changes_to_it`.
- `failure_shows_failed_to_revert_changes`.

- [ ] **Step 2: Check that they fail**

Run: `pnpm vitest run src/log && (cd src-tauri && cargo test apply_commits::)`
Expected: FAIL

- [ ] **Step 3: Implement**

According to decision 8, the patch comes from `git diff --binary <parent> <hash> -- <paths>` and is applied with `git apply [-R]`. If that fails, with `git apply [-R] --3way`. Afterwards, the changes that `--3way` left in the stage are taken out of the stage, and remain as local changes in the chosen changelist.

- [ ] **Step 4: Check that they pass**

Run: `pnpm vitest run && (cd src-tauri && cargo test)`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: apply and revert selected changes of a commit"
```

### Task 12: Rewriting from the log: Edit Commit Message, Drop and Squash Commits

Reference:
- `rebase/log/GitCommitEditingActionBase.kt:161-271`;
- `rebase/GitSingleCommitEditingAction.kt:18-38`;
- `rebase/log/GitCommitEditingOperation.kt:17-35`;
- `rebase/log/GitCommitEditingEditorHandler.kt`;
- `rebase/log/GitCommitEditingNotifications.kt` and `GitCommitEditingOperationResult.kt:45-55`;
- `rebase/log/GitNewCommitMessageActionDialog.kt:47-65`;
- `rebase/GitRewordAction.kt:23-76` and `rebase/GitRewordOperation.kt:35-102`;
- `rebase/log/squash/GitSquashLogAction.kt` and `GitSquashOperation.kt:22-29`;
- `rebase/log/drop/GitDropLogAction.kt` and `GitDropOperation.kt:22-24`;
- `rebase/interactive/GitRebaseTodoModel.kt:71-87,304` and `GitRebaseTodoModelConverter.kt`;
- `rebase/GitInteractiveRebaseFile.java:44-78`;
- `rebase/GitRebaseEntry.kt:15-32`;
- `rebase/interactive/GitRewordedCommitMessageProvider.kt`;
- texts: `B:232-277,614-615,647`.

**Files:**
- Create: `src-tauri/src/todo.rs`, `src-tauri/src/rewrite.rs`, `src/log/editing.ts`, `src/log/editing.test.ts`, `src/log/CommitMessageDialog.svelte`
- Modify: `src/log/contextMenu.ts`, `src/log/LogActions.svelte`, `src/lib/keymap.ts` (F2 and ⇧F6 in the log), `src-tauri/src/vcs_commands.rs`, `src-tauri/src/lib.rs`, `NOTICE`, `src/lib/api.ts`

**Interfaces:**
- Consumes: `editor::EditorHandler`, `REBASE_CONFIG` and `TaskContext::git_editing` from Task 1; `rebase::Process` from Task 9.
- Produces:
  - `todo::Action` equal to `Pick | Edit | Drop | Squash | Reword | Fixup | Other(String)`.
  - `todo::Entry { action: Action, hash: String, subject: String }`.
  - `todo::parse(text: &str) -> Result<Vec<Entry>, TodoError>`, with `TodoError::Noop` for the `noop` case.
  - `todo::write(entries: &[Entry]) -> String`.
  - `todo::Model`, with `drop(hashes)`, `reword(hash, message)`, `unite(root, children)`, `reorder` and `to_entries()`. The children of a squash are written `fixup`, like `GitRebaseTodoModel.kt:304`.
  - `rewrite::Rewriter`, which is an `EditorHandler`: it writes the model's todo and answers the reword messages with the saved texts, checking that the text starts with the original message.
  - `rewrite::reword(repo, hash, message, expected_head) -> Result<Rewritten, GitError>`. With HEAD, it uses `commit --amend -F <file> --only --no-verify`; otherwise, or if that fails, a rebase with `reword`.
  - `rewrite::drop(repo, hashes, expected_head)` and `rewrite::squash(repo, hashes, message, expected_head)`, with `rebase --interactive --no-autosquash <parent of the oldest>`.
  - `Rewritten { old_head: String, new_head: String, outcome: rebase::Outcome }`.
  - `rewrite::undo(repo, rewritten: &Rewritten) -> Result<(), GitError>`: fails with `Repository has already been changed` if HEAD is not `new_head`, or with the protected branch text, and otherwise does `reset --keep <old_head>`.
  - `rewrite::check(ctx, hashes, action) -> Result<(), String>`: the checks of `GitCommitEditingActionBase`, with their literal texts.
  - In TS:
    - entries `Edit Commit Message…` (F2, ⇧F6), `Drop Commit` or `Drop Commits` and `Squash Commits…`, with the reason for being disabled in the description;
    - `CommitMessageDialog`, with the title `Edit Commit Message` or `Squashed Commit Message` and the label `Edit message for commit {0} by {1}` or `Edit message for squashed commit`;
    - the success notification with `Undo`.

- [ ] **Step 1: Write the failing tests**

Rust:
- `parse_and_write_round_trip`: comments with `\u{1}`, `noop`, and unknown actions as `Other`.
- `reword_of_head_uses_amend`.
- `reword_of_an_earlier_one_uses_rebase_and_keeps_the_others`.
- `reword_that_does_not_start_with_the_original_fails`.
- `drop_of_two_commits`.
- `non_contiguous_squash_reorders_like_intellij`.
- `undo_goes_back_with_reset_keep`.
- `undo_after_another_change_gives_repository_has_already_been_changed`.
- `commit_outside_the_branch_gives_the_commit_is_not_in_the_current_branch`.
- `merge_in_the_range_gives_commit_has_n_parents`.
- `commit_on_protected_branch_is_rejected_with_its_text`.
- `during_a_merge_gives_cannot_reword_during_merge`, and Reword of HEAD during a rebase is allowed.
- `local_changes_are_saved_and_restored`.
- `conflict_when_rewriting_follows_task_2`.

TS:
- `f2_in_the_log_opens_edit_commit_message_and_in_commit_is_still_rename`.
- `reword_dialog_with_full_message`.
- `squash_joins_the_messages_with_two_blank_lines`.
- `drop_without_confirmation`.
- `success_commit_message_changed_with_undo`.
- `undo_expires_if_the_repository_changes`.
- `disabled_entries_say_why`.

- [ ] **Step 2: Check that they fail**

Run: `pnpm vitest run src/log && (cd src-tauri && cargo test todo:: rewrite::)`
Expected: FAIL

- [ ] **Step 3: Implement**

Each operation is `rebase::Process` with the `Rewriter` as handler. The messages are kept in the `Rewriter`, not on disk.

Add `todo.rs` and `rewrite.rs` to `NOTICE`, with `GitRebaseTodoModel`, `GitInteractiveRebaseFile`, `GitRebaseEntry` and the operations of `rebase/log/`.

- [ ] **Step 4: Check that they pass**

Run: `pnpm vitest run && (cd src-tauri && cargo test)`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: edit message, drop and squash from the log"
```

### Task 13: Undo Commit, Fixup… and Squash Into…

Reference:
- `reset/GitUncommitAction.java:37-116`;
- `reset/GitResetOperation.java:72-98`;
- `rebase/GitAutoSquashCommitAction.kt`, `GitCommitFixupBySubjectAction.kt` and `GitCommitSquashBySubjectAction.kt`;
- `rebase/GitRebaseCheckinHandlerFactory.kt:28-36`;
- texts: `B:44,565-568,618-619,986-993`.

**Files:**
- Modify: `src-tauri/src/rewrite.rs`, `src/log/editing.ts`, `src/log/editing.test.ts`, `src/log/contextMenu.ts`, `src/commit/CommitView.svelte` (proposed message and the `Commit and Rebase…` executor), `src-tauri/src/vcs_commands.rs`, `src-tauri/src/lib.rs`, `src/lib/api.ts`

**Interfaces:**
- Consumes: `rewrite::check` from Task 12, the changelist selector of phase 1 and `rebase::Process`.
- Produces:
  - `rewrite::uncommit(repo, hash, expected_head) -> Result<Vec<String>, GitError>`: `reset --soft <parent>`, and returns the paths of the commit.
  - `rewrite::autosquash(repo, base: &str) -> Result<rebase::Outcome, GitError>`: `rebase --interactive <hash>~` with the `Ask` handler, which without `--autosquash` respects the user's `rebase.autoSquash`.
  - In TS:
    - the entry `Undo Commit…`, disabled outside HEAD with `The selected commit is not the last in the current branch`;
    - `Fixup…` and `Squash Into…`, which open the Commit view with `fixup! <subject>` or `squash! <subject>` and add the `Commit and Rebase…` executor.

- [ ] **Step 1: Write the failing tests**

- `undo_commit_keeps_the_changes_and_moves_the_head`.
- `undo_commit_asks_select_target_changelist_with_the_subject`.
- `cancelling_the_chooser_does_not_undo`.
- `undo_commit_on_a_protected_branch_is_rejected`.
- `undo_commit_successful_and_failed`.
- `fixup_opens_the_commit_view_with_fixup_and_the_subject`.
- `commit_and_rebase_commits_and_then_rebases`.
- `without_changes_in_the_default_list_says_nothing_to_commit`.

- [ ] **Step 2: Check that they fail**

Run: `pnpm vitest run src/log src/commit && (cd src-tauri && cargo test rewrite::)`
Expected: FAIL

- [ ] **Step 3: Implement**

Undo Commit does not forbid any ongoing state, like IntelliJ. `Commit and Rebase…` only appears if the repository has no ongoing operation, or has a detached HEAD.

- [ ] **Step 4: Check that they pass**

Run: `pnpm vitest run && (cd src-tauri && cargo test)`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: undo commit, fixup and squash into"
```

### Task 14: Interactive rebase

Reference:
- `rebase/GitInteractiveRebaseAction.kt:18-23`;
- `rebase/interactive/GitInteractiveRebaseUsingLog.kt:25-144`;
- `rebase/interactive/dialog/` in full: `GitInteractiveRebaseDialog.kt`, `GitRebaseCommitsTableView.kt`, `GitRebaseCommitsTableModel.kt`, `ChangeEntryStateActions.kt`, `view/CommitMessageCellEditor.kt` and `GitRebaseCommandsDialog.kt`;
- `rebase/GitInteractiveRebaseEditorHandler.java:66-174`;
- `rebase/GitInteractiveRebaseFile.java`;
- `rebase/GitRebaseProcess.java:239-244,421-428`;
- texts: `B:195,203-229,331,563-564`.

**Files:**
- Create: `src/rebase/InteractiveRebaseDialog.svelte`, `src/rebase/model.ts`, `src/rebase/model.test.ts`, `src/rebase/InteractiveRebaseDialog.test.ts`, `src/rebase/GitCommandsDialog.svelte`
- Modify: `src-tauri/src/rewrite.rs`, `src-tauri/src/rebase.rs`, `src/log/contextMenu.ts`, `src/log/editing.ts`, `src/git/RebaseDialog.svelte`, `src-tauri/src/vcs_commands.rs`, `src-tauri/src/lib.rs`, `NOTICE`, `src/lib/api.ts`

**Interfaces:**
- Consumes: `todo::Entry`, `todo::Model` and `rewrite::Rewriter` from Task 12; `rebase::Process`; the commit details panel of the phase 2 log.
- Produces:
  - `rewrite::interactive_entries(ctx, hash) -> Result<Option<Vec<todo::Entry>>, GitError>`. It reads the log from HEAD to the commit; `None` if there is a merge, a root commit, a hash that does not resolve or a `fixup!` or `squash!` subject (decision 10).
  - `rewrite::interactive(repo, base: &str, plan: Option<Vec<todo::Entry>>, expected_head) -> Result<rebase::Outcome, GitError>`:
    - with `plan`, the handler compares git's todo with the one that was generated, and if it differs it fails with `Cannot perform the rebase because the IDE generated an invalid 'git-rebase-todo' file…`, with the IDE's name omitted;
    - without `plan`, it asks the window for the todo with `editor-request`.
  - In `model.ts`, without DOM: `RebaseModel` with `pick`, `edit`, `reword(index, message)`, `squash(indices)`, `fixup(indices)`, `drop`, `moveUp`, `moveDown`, `undo`, `redo` (10 states), `reset` and `entries()`, and the enabling of each action like `ChangeEntryStateActions`.
  - `InteractiveRebaseDialog`, with title `Rebasing Commits`, OK `Start Rebasing` and size 1000×550. On the left the table and on the right the details and the changes of the commit.
  - The log entry `Interactively Rebase from Here…`. The option `Edit commits before rebasing` of the Rebase… dialog uses the same dialog.

- [ ] **Step 1: Write the failing tests**

TS, on the model:
- `squash_unites_with_the_previous_one_that_is_kept`.
- `squash_on_the_first_row_disabled`.
- `squash_marks_the_root_as_reword_with_the_united_messages`.
- `reword_equal_to_the_original_goes_back_to_pick`.
- `initial_drop_moves_to_the_end`.
- `undo_keeps_ten_states`.
- `shortcuts_alt_p_e_d_r_s_f`.

TS, on the dialog:
- `start_rebasing_disabled_while_a_message_is_edited`.
- `editing_inline_with_cmd_enter_applies`.
- `cancelling_with_changes_asks_discard_changes_and_cancel_rebase`.
- `reset_only_after_a_change`.
- `view_git_commands_shows_the_todo`.
- `details_of_the_selected_commit`.

Rust:
- `log_entries_from_head_to_the_commit`.
- `merge_in_the_range_gives_none_and_uses_native_mode`.
- `mismatching_plan_fails_without_rebasing`.
- `edit_stops_and_notifies_rebase_stopped_for_editing`.
- `reword_without_a_saved_message_asks_git_commit_message`.
- `cancelling_the_dialog_does_not_launch_git`.
- `noop_asks_whether_to_continue`.

- [ ] **Step 2: Check that they fail**

Run: `pnpm vitest run src/rebase && (cd src-tauri && cargo test rewrite:: rebase::)`
Expected: FAIL

- [ ] **Step 3: Implement**

The table has the oldest at the top and an icon column:
- single node, or double for the root of a joined group;
- pause for `edit`;
- nothing for `drop` and `fixup`.

The subject is struck through if it is `drop` and blue if it is `reword`. The children of a `fixup` are indented.

On accepting, `rewrite::interactive` is launched with `--interactive <full parent>`, without `--no-autosquash`, like `startInteractiveRebase`.

- [ ] **Step 4: Check that they pass**

Run: `pnpm vitest run && (cd src-tauri && cargo test)`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: interactive rebase"
```

### Task 15: Compare HEAD, Staged and Local Versions

Reference:
- `index/actions/GitStageCompareThreeVersionsAction.kt`;
- `index/actions/GitStageDiffAction.kt:48-66`;
- `index/GitStageDiffUtil.kt:58-67,163-176,232-245`;
- `index/GitIndexUtil.java:236-268`;
- texts: `B:814-815,839-842,850-851,869-871` and DiffBundle 42.

**Files:**
- Create: `src/diff/ThreeVersionsPane.svelte`, `src/diff/ThreeVersionsPane.test.ts`
- Modify: `src-tauri/src/worktree_diff.rs`, `src/diff/DiffWindow.svelte`, `src/merge/editors.ts` (what is common to three panes), `src/commit/StagingView.svelte`, `src/App.svelte`, `src-tauri/src/vcs_commands.rs`, `src-tauri/src/lib.rs`, `src/lib/api.ts`

**Interfaces:**
- Consumes: what is common to three panes from Task 5 and the diff window of phase 1.
- Produces:
  - `worktree_diff::three_sides(repo, path) -> Result<ThreeSides, HunkError>`, with `ThreeSides { head: String, staged: String, local: String, staged_hash: String, local_hash: String }`.
  - `worktree_diff::write_index(repo, path, content, expected_hash) -> Result<String, HunkError>`: `git hash-object -w --stdin --path <path>` and `git update-index --cacheinfo <mode>,<sha>,<path>`, with the mode from `ls-files -s`.
  - Commands `staging_three_sides(path)` and `index_write(path, content, expected_hash)`.
  - The comparison `Compare HEAD, Staged and Local Versions` in the Staging view menu, only if the three versions exist.
  - The arrows per range: Local→Staged (`Stage`), Staged→Local (`Revert`) and HEAD→Staged (`Unstage`).

- [ ] **Step 1: Write the failing tests**

- `three_panes_head_staged_local`.
- `stage_of_a_range_writes_the_index`.
- `unstage_of_a_range_returns_head_to_the_index`.
- `revert_of_a_range_changes_the_file_with_a_snapshot`.
- `editing_staged_and_switching_files_saves_it`.
- `index_changed_outside_is_not_overwritten`.
- `only_with_the_three_versions`.
- `title_with_rename_orig_arrow_new`.

- [ ] **Step 2: Check that they fail**

Run: `pnpm vitest run src/diff && (cd src-tauri && cargo test worktree_diff::)`
Expected: FAIL

- [ ] **Step 3: Implement**

The Staged and Local panes are editable. Saving goes into the `transition` and the `flush` of the diff window, so that `flush_diff_window` covers it.

In the matrix, change the rows `Git.Stage.Compare.Three.Versions` and `Git.Stage.ThreeSideDiff`: they stop being `pending`.

- [ ] **Step 4: Check that they pass**

Run: `pnpm vitest run && (cd src-tauri && cargo test)`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: three-pane comparison of HEAD, staged and local"
```

### Task 16: Phase validation

**Files:**
- Create: `docs/reports/phase-3-validation.md`, `docs/reports/phase-3-commands.md`, `docs/reports/phase-3-decisions.md`, `docs/reports/phase-3-consumption.json`
- Modify: `docs/parity-matrix.md`, `scripts/extract_actions.py`, `scripts/test_extract_actions.py`, `docs/superpowers/specs/2026-10-31-intellij-git-client-design.md`, `NOTICE`, `src/perf/run.ts`

**Interfaces:**
- Consumes: the complete app.
- Produces: the matrix with the result column for the phase 3 rows.

- [ ] **Step 1: Fix the matrix and the classifier**

Apply decision 9. First add `test_branches_popup_actions_in_their_phase`, check that it fails, and then change `classify`. Keep the Result column when regenerating. Note the 10 decisions in section 4 of the design.

- [ ] **Step 2: Compare commands**

With `scripts/git-wrapper.sh` and a throwaway repository, record what the app launches in:
- merge with and without conflict;
- Smart Merge;
- rebase, interactive rebase, continue, skip and abort;
- cherry-pick, revert and undo commit;
- reword, drop and squash;
- Accept Yours, Accept Theirs and Apply.

Compare with the commands cited in this plan, and note the differences in `docs/reports/phase-3-commands.md`.

- [ ] **Step 3: Measure again**

With the method of `docs/reports/phase-2-validation.md` ("Measurement with the visible window"): Mac untouched for 3 minutes and load below 15. Add to `src/perf/run.ts` the opening of the merge window with a conflict of `builtin/commit.c` in `~/rama-medida/git`, and measure its idle CPU and its memory.

Run: `python3 scripts/measure.py --app src-tauri/target/release/bundle/macos/rama.app/Contents/MacOS/rama --repo ~/rama-medida/git --out docs/reports/phase-3-consumption.json --runs 5`
Expected: JSON with `window_visible: true`. If the screen is locked, note what was not measured.

- [ ] **Step 4: Go through the matrix**

For each phase 3 row, note `done`, `partial` or `pending`, citing the test. "Confirm by hand" marks what only a person can check.

- [ ] **Step 5: Write the report**

Compare with phase 2 and with IntelliJ. List what could not be checked, and include a manual checklist for real repositories: real conflicts, interactive rebase of a branch of one's own, cherry-pick between branches, and physical keyboard in the merge window.

In `phase-3-decisions.md`, record the execution decisions and the minor findings left unattended. Also note which deferred findings of phases 1 and 2 were fixed in this phase:
- phase 1: Tasks 2, 8 and 11;
- phase 2: Tasks 9, 11 and 12.

- [ ] **Step 6: Hand over to the user**

Build the `.app` and say how to open it. The phase ends when the user confirms it on their real repositories.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "docs: phase 3 validation"
```
