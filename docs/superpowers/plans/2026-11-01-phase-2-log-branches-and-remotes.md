# Phase 2: log, branches, push and pull. Implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Work day to day without opening IntelliJ except to resolve conflicts: see the log with its real graph, filter it, manage branches and tags, and bring in and push changes.

**Architecture:** The Rust core replaces the provisional graph with a port of the one in `platform/vcs-log/graph`, with per-tab views that combine order, filters and collapsing. Network operations go through a task executor with progress, cancellation, console and credentials through askpass. The Svelte interface adds the full log panel, the branches popup, and the push, pull and Update Project dialogs.

**Tech Stack:** Tauri 2.12, Rust 1.99.1, Svelte 5, Vitest 5, Canvas 2D for the log. No new dependencies unless a task says so.

**Spec:** `docs/superpowers/specs/2026-10-31-intellij-git-client-design.md`. Scope: the phase 2 rows of `docs/parity-matrix.md`.

**Exit criterion:** the user works in their real repositories without opening IntelliJ except for conflicts, and declares the phase finished.

**Branch:** `fase-2`, created from `fase-1`. Neither `fase-0` nor `fase-1` has been merged into `main`.

**Reference read:** the tag `idea/232.10072.27`. Whoever implements reads the file cited in each task at that tag, with `gh api` or a partial clone, and copies the literal texts from there, in English as in IntelliJ.

## Global Constraints

- Platform: macOS arm64. No JVM or Chromium of its own.
- Every Git operation uses the real `git` binary, with arguments and without a shell, `--` before paths, and `LC_ALL=C` and `GIT_OPTIONAL_LOCKS=0`, as `src-tauri/src/git.rs` already does.
- Reference: IntelliJ IDEA CE 2023.2.3, build `IC-232.10072.27`, tag `idea/232.10072.27`. Do not use `master`.
- Devin edits files in the local copy and the user does the Git operations.
- One repository per window.
- Own data in `~/Library/Application Support/<app>/`, per repository. Nothing is ever written to `.idea/`.
- Before reset, force checkout or discarding changes, a recoverable copy with `snapshots::create`, kept for 30 days.
- Every command that changes files or the index first calls `flush_diff_window` and takes `RepoLocks`, as in phase 1.
- Destructive operations ask for the same confirmations as IntelliJ, no more and no fewer.
- Automatic tests use throwaway repositories and `file://` or local bare remotes. Never the real ones, nor the network, nor `.idea/`, nor `~/Library/Application Support/JetBrains`.
- A password or passphrase is never written to disk, nor to the console, nor to an error, nor to an argument.
- Validate in Rust, not only in the interface: branch and tag names with `git check-ref-format`, and no name, remote, URL or path that starts with `-` reaches git as an option.
- What comes from a repository or a remote is untrusted text: commit messages, author and branch names, git output and ssh texts are shown as text, without `{@html}`, and Tauri's CSP is not relaxed.
- Targets (guideline): about 300 MB of memory at rest, CPU close to 0 %, less than 1 second of startup, log scrolling without jumps, the New UI dark theme and the macOS keymap.
- Do not use the name or logos of JetBrains or IntelliJ.
- Commits ending with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Decisions of this plan

Each one departs from IntelliJ or from the approved matrix. The user approves them with the plan.

1. **Stash only until phase 4.** `Clean working tree using` is Stash or Shelve, and in IntelliJ the default value is Shelve. The shelf arrives in phase 4, so here everything is saved with `git stash`.
2. **The "auto, always or never" setting does not exist in 2023.2.3.** The matrix was wrong. The row becomes `Clean working tree using`, and in this phase it is shown with Stash fixed.
3. **System credentials.** According to section 4 of the design, `Use credential helper` comes enabled, unlike in IntelliJ. Without Password Safe, the login dialog has no `Remember`: if there is a helper, git saves it.
4. **Restore after an error or a cancellation.** IntelliJ leaves the changes saved in those cases. Here they are restored if the repository went back to its clean state, because there is no stash interface until phase 4.
5. **`Restore Changes` button.** If saved changes remain, because of conflicts or because restoring fails, the notification stays with that button, which does `git stash pop` of that entry. IntelliJ links to the shelf, which does not exist here.
6. **Conflicts.** The app detects the state and leaves the repository as git leaves it, with `Abort`. Resolving them belongs to phase 3, as do `Continue` and `Skip`.
7. **GitHub protected-branch rules, to phase 4.** They need the Keychain token. Here there is a list of patterns, with `master` and `main`, saved per repository.
8. **Filtering without stages.** IntelliJ asks for 5, 2,000 and then all the commits. Here everything is requested and shown when it finishes. The result is the same and takes longer to appear.
9. **Full reload of the log.** IntelliJ joins only the new commits. Here everything is reread when the references change, and only if the log is visible. Editing files does not trigger it.
10. **Rows with no effect in the app.** `Vcs.Log.CheckFirstCommitsIndexData`, `CheckSelectedCommitsIndexData`, `InvalidateCaches` and `ResumeIndexing` belong to IntelliJ's index, which the app does not have. `ShowRootsColumnAction` and `Git.Log.Branches.GroupBy.Repository` have no effect with one repository. They stay `pending` with that reason.
11. **`Vcs.Log.ShowAllAffected` and `Vcs.Log.ShowOtherBranches` move to phase 4.** They only act on file history.
12. **Row height 24.** IntelliJ calculates it from the font and the value is not in the theme. It is adjusted during validation by comparing screenshots.

## Review Focus

1. Devin writes a file while Update Project or Smart Checkout have the changes saved: restoring gives a conflict, the stash entry is still there and the notice lets you recover it. Test in Task 11.
2. A credential requested from an app opened from Finder, without a terminal: the dialog appears or it fails clearly, it never hangs, and cancelling kills git and ssh. Test in Task 2.
3. Force push with stale information or to a protected branch: `--force` is never sent unless the user asks for it, and a protected one cannot be forced. Test in Task 14.
4. `reset --hard` or `checkout --force` that overwrite an unversioned file that exists in the target: it goes into the recoverable copy. Test in Task 9.
5. Cancelling an Update halfway: a merge goes back with `git reset --merge`, a rebase with `git rebase --abort`, and the saved changes come back. Test in Task 11.

---

### Task 1: Long tasks, console and notifications

Reference: `git4idea/commands/GitImplBase.java`, `GitTextHandler.java:119-154`, `GitVcsConsoleWriter.java`, `GitHandler.java:139-158`, `GitCommand.java:36-58`.

**Files:**
- Create: `src-tauri/src/task.rs`, `src-tauri/src/console.rs`, `src/lib/tasks.ts`, `src/lib/tasks.test.ts`, `src/lib/notifications.ts`, `src/lib/notifications.test.ts`, `src/lib/Notifications.svelte`, `src/lib/TaskStatus.svelte`, `src/console/ConsoleView.svelte`, `src/console/console.test.ts`
- Modify: `src-tauri/src/git.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/commands.rs`, `src/App.svelte`, `src/lib/tabs.ts`, `src/lib/keymap.ts`, `src/lib/api.ts`

**Interfaces:**
- Produces:
  - `task::Runner`, in `AppState`. `Runner::start(kind: TaskKind, title: String, work: impl FnOnce(&TaskContext) -> Result<T, GitError>) -> TaskId`. `TaskKind` is `Read`, `Write` or `Remote(String)`. `Write` takes `RepoLocks` and serializes, like the write lock of `GitVcs`. `Remote` serializes only per remote, and does not block a commit.
  - `TaskContext::git(args: &[&str]) -> Result<GitOutput, GitError>`, with output read live, progress through `\r` on stderr and cancellation. `GitOutput { stdout, stderr }`. `TaskContext::check_cancelled() -> Result<(), GitError>`.
  - `GitError::Cancelled`. Command `task_cancel(id: TaskId)`.
  - Events `task-changed { id, title, text, fraction: Option<f32>, state: "running" | "done" | "failed" | "cancelled" }` and `console-line { time, command: bool, text }`.
  - `console::Console`: ring of 2,000 lines in memory. Command `console_lines() -> Vec<ConsoleLine>`.
  - `notify(notification: Notification)` in `src/lib/notifications.ts`, with `Notification { kind: "info" | "warning" | "error", title, body?, actions?: { label, run }[], sticky?: boolean }`.
  - `type Tab = "commit" | "log" | "console"`.

- [ ] **Step 1: Write the failing tests**

- `write_task_waits_for_the_previous_one`: two `Write` tasks on the same repository do not overlap.
- `read_task_does_not_wait`: a `Read` runs with a `Write` in progress.
- `remote_task_serializes_per_remote`: two `Remote("origin")` do not overlap, and `Remote("otro")` runs at the same time.
- `cancelling_kills_the_child`: with a fake `git` that sleeps, `task_cancel` sends SIGTERM and returns `GitError::Cancelled` in under 1 s.
- `cancelling_kills_the_grandchild`: git's child process dies too. The child is launched in its own group with `process_group(0)` and the signal goes to the group.
- `cancelling_escalates_to_sigkill`: a fake that ignores SIGTERM dies with SIGKILL after the deadline. The deadline is 60 s, like IntelliJ, and injectable in the test.
- `progress_reads_lines_with_carriage_return`: `Receiving objects:  45%\r` gives `fraction` 0.45 and does not go into the console.
- `console_shows_only_commands_that_write`: `fetch` is logged, `rev-list` is not, with the `HH:mm:ss.SSS: [<folder>] git <args>` format of `GitImplBase.java:482-493`.
- `console_truncates_to_10000_characters`.
- `console_hides_credentials_in_urls`: `https://u:clave@host/r` comes out as `https://u:***@host/r`.
- In TS, `informational_notification_closes_on_its_own`, `sticky_notification_stays`, `action_button_closes_it`, `running_task_shows_title_and_cancel`, `console_tab_with_show_console_tab`.

- [ ] **Step 2: Check that they fail**

Run: `pnpm vitest run src/lib src/console && (cd src-tauri && cargo test task:: console::)`
Expected: FAIL

- [ ] **Step 3: Implement**

`TaskContext::git` shares `command()` with `git.rs` and adds `-c core.quotepath=false -c log.showSignature=false`. It reads stdout and stderr in separate threads.

Notifications appear at the bottom right, and the informational ones close after 10 s. Show the active task in the bottom bar, with its cancel button.

While at it, fix the phase 1 finding "a log error stays visible in the Commit tab": the error becomes a notification.

- [ ] **Step 4: Check that they pass**

Run: `pnpm vitest run && (cd src-tauri && cargo test)`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: long tasks with cancellation, console and notifications"
```

### Task 2: Credentials

Reference: `git4idea/commands/GitHandlerAuthenticationManager`, `GitHttpLoginDialog`, and the SSH texts quoted in Step 3, read from the installed app of the same version.

**Files:**
- Create: `src-tauri/src/askpass.rs`, `src/lib/AskpassDialog.svelte`, `src/lib/askpass.test.ts`
- Modify: `src-tauri/src/main.rs`, `src-tauri/src/task.rs`, `src-tauri/src/settings.rs`, `src-tauri/src/commands.rs`, `src/App.svelte`, `src/lib/api.ts`, `src/settings/SettingsView.svelte`

**Interfaces:**
- Consumes: `TaskContext::git`.
- Produces:
  - `askpass::Server::start(on_prompt) -> Server`. Unix socket in a new `0700` directory per run, with a random 32-byte token.
  - `askpass::client_main() -> Option<i32>`, first line of `main`: if `RAMA_ASKPASS_SOCKET` and `RAMA_ASKPASS_TOKEN` exist, it acts as a client and exits without opening Tauri.
  - `askpass::classify(prompt: &str) -> PromptKind`, with `PromptKind` equal to `Username { url } | Password { url, user } | Passphrase { key } | SshPassword { user } | HostKey { text }`.
  - Event `askpass-prompt { id, kind }` and command `askpass_answer(id: u64, answer: Option<String>)`.
  - `Settings.use_credential_helper: bool`, `true` by default.

- [ ] **Step 1: Write the failing tests**

- `classify_recognizes_https_username`: `Username for 'https://github.com': `.
- `classify_recognizes_https_password`, `classify_recognizes_passphrase`, `classify_recognizes_ssh_password`, `classify_recognizes_unknown_host`, `classify_odd_text_asks_for_password`.
- `client_returns_the_answer`: the test binary, with a real server, writes the answer to stdout.
- `client_without_a_valid_token_gets_rejected`: a wrong token receives nothing and exits with 1.
- `socket_only_for_the_owner`: the directory has mode `0700`.
- `message_over_64_kb_is_rejected`: the server cuts the connection without reading it whole.
- `without_an_answer_git_is_cancelled`: `answer: None` makes the client exit with 1, and git fails without being left waiting.
- `git_with_askpass_without_a_terminal`: `git ls-remote` against a fake `GIT_SSH_COMMAND` that calls `$SSH_ASKPASS` receives the answer. It checks that `GIT_TERMINAL_PROMPT=0` and `SSH_ASKPASS_REQUIRE=force` are set.
- `helper_off_adds_an_empty_config`: with `use_credential_helper` false, `-c credential.helper=` is prepended to everything except `git config`.
- `helper_on_adds_nothing`.
- `the_answer_does_not_reach_the_console`: after an answer, no console line contains it.
- In TS, `login_dialog_has_username_and_password`, `unknown_host_asks_yes_or_no`, `cancel_answers_null`, `password_does_not_stay_in_the_state_on_close`.

- [ ] **Step 2: Check that they fail**

Run: `pnpm vitest run src/lib/askpass.test.ts && (cd src-tauri && cargo test askpass::)`
Expected: FAIL

- [ ] **Step 3: Implement**

`GIT_ASKPASS` and `SSH_ASKPASS` point to `std::env::current_exe()`. The dialog copies IntelliJ's texts:

- HTTPS: title `Log In to {url}`, text `Enter credentials:`, fields `Username:` and `Password:`, button `Log In`.
- Passphrase: title `SSH Key Passphrase`, text `Password for the SSH key "{key}":`.
- SSH password: title `SSH Password Login`, text `Password for user {user}:`.
- Host: title `SSH Confirmation`, with the text that ssh sends. `Yes` answers `yes`.

The field is `type="password"` with `autocomplete="off"`. Put the `Use credential helper` setting in the Git section of Settings. The answer only travels through the socket: it is not stored.

- [ ] **Step 4: Check that they pass**

Run: `pnpm vitest run && (cd src-tauri && cargo test)`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: ask for git credentials with a dialog of its own"
```

### Task 3: References and loading of the log

Reference: `git4idea/log/GitLogProvider.java:304,425-532`, `GitRefManager.kt:88-127,215`, `GitLogRecord.java:153`, `GitLogUtil.java:228`, `RefsModel.java:44`.

**Files:**
- Create: `src-tauri/src/log/refs.rs`, `src-tauri/src/log/commands.rs`
- Modify: `src-tauri/src/log/store.rs`, `src-tauri/src/log/parse.rs`, `src-tauri/src/log/mod.rs`, `src-tauri/src/commands.rs`, `src-tauri/src/lib.rs`, `src/App.svelte`, `src/lib/api.ts`

**Interfaces:**
- Produces:
  - `refs::GitRef { name: Box<str>, kind: RefKind, commit: u32 }`, with `RefKind` equal to `Head | Local | Remote | Tag`.
  - `refs::Refs { all: Vec<GitRef>, current: Option<u32>, tracked: HashMap<u32, u32> }`, with the indexes into `all`.
  - `refs::best_ref(refs_of_head: &[&GitRef]) -> Option<&GitRef>` with the order of `GitRefManager.kt:215`: `origin/master` or `origin/main`, other remotes, `master` or `main`, other local ones, tags, `HEAD`, the rest; on a tie, by name.
  - `refs::groups(refs_of_commit: &[&GitRef], ...) -> Vec<RefGroup>`, with the grouping of `GitRefManager.kt:88-127`.
  - `store::Commit { hash, parents, author: u32, committer: u32, author_time: i64, commit_time: i64, subject }`. Authors and committers are interned in `store::People`.
  - `store::load(repo) -> CommitStore`. Reads with `log HEAD --branches --remotes --tags --date-order --decorate=full --encoding=UTF-8 --`, without `--all`.
  - `store::refs_fingerprint(repo) -> Result<u64, GitError>`, from `git for-each-ref` and `HEAD`.

- [ ] **Step 1: Write the failing tests**

- `loading_does_not_include_the_stash`: after `git stash`, the stash commit does not appear. It fails today because of `--all`.
- `decorate_separates_head_and_branch`: `HEAD -> refs/heads/x` gives `Head` and `Local`.
- `decorate_removes_origin_head_and_grafted`.
- `decorate_recognizes_tag`.
- `best_ref_prefers_origin_main`, `best_ref_full_order`.
- `groups_join_local_and_tracked_remote`: `master` with `origin/master` form `origin & master`.
- `groups_detached_head_gets_exclamation_mark`.
- `interned_authors_take_one_entry`: 1,000 commits by the same author give one entry in `People`.
- `fingerprint_changes_when_creating_a_branch`, `fingerprint_does_not_change_when_editing_a_file`.
- `repository_without_commits`, `detached_head`, `shallow_clone` keep passing.

- [ ] **Step 2: Check that they fail**

Run: `(cd src-tauri && cargo test log::)`
Expected: FAIL

- [ ] **Step 3: Implement**

Move the log commands to `log/commands.rs`. Keep `LogSummary` and extend `LogRow` in Task 4.

- [ ] **Step 4: Check that they pass and that memory does not get worse**

Run: `(cd src-tauri && cargo test) && pnpm vitest run`
Expected: PASS

Measure, with `git/git` in `~/rama-medida/git`, the memory after `log_open` against phase 1. If it grows by more than 10 %, reduce it before going on.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: references and loading of the log like IntelliJ"
```

### Task 4: Real graph

Reference: `graph/impl/permanent/GraphLayoutBuilder.java`, `HeadCommitsComparator.java:41-73`, `PrintElementGeneratorImpl.kt`, `GraphElementComparatorByLayoutIndex.java`, `GraphColorGetterByHead.kt`, `PrintElementPresentationManagerImpl.kt`, `DefaultColorGenerator.kt`, `PaintParameters.java`, `SimpleGraphCellPainter.java`, `EdgesInRowGenerator`.

**Files:**
- Create: `src-tauri/src/log/layout.rs`, `src-tauri/src/log/print.rs`, `src-tauri/src/log/view.rs`, `src/log/graphColor.ts`, `src/log/graphColor.test.ts`, `src/log/draw.test.ts`
- Modify: `src-tauri/src/log/graph.rs`, `src-tauri/src/log/commands.rs`, `src/log/draw.ts`, `src/log/LogView.svelte`, `src/lib/api.ts`, `src/lib/theme.ts`

**Interfaces:**
- Produces:
  - `layout::layout_indexes(graph: &LinearGraph, heads: &[u32]) -> Vec<u32>`, starting at 1.
  - `graph::LinearGraph { rows: Vec<u32>, ups, downs }`: rows to commits, with edges `GraphEdge { up: u32, down: u32, style: EdgeStyle }`. `EdgeStyle` is `Usual | Dotted | Dashed`.
  - `print::elements_in_row(view: &View, row: u32) -> PrintRow`.
  - `PrintRow { node_x: u16, node_color: i32, width: u16, edges: Box<[ShortEdge]>, arrows: Box<[Arrow]> }`, with `ShortEdge { x: u16, other_x: u16, up: bool, color: i32, style: EdgeStyle }` and `Arrow { x: u16, up: bool, color: i32, target_row: u32 }`.
  - `view::View`, created by `View::build(store, refs, spec: &ViewSpec)`, with `ViewSpec { sort, long_edges, collapsed, filter }`. `SortType` for now only `Off`.
  - `log_rows(view_id, start, count)` returns `LogRow` with `PrintRow`.
  - `graphColor(id: number): string` in TS.

- [ ] **Step 1: Write the failing tests**

Rust:

- `first_parent_chain_shares_index`, `another_parent_starts_a_chain`, `indexes_start_at_one`.
- `heads_sorted_by_best_reference`: `origin/main` goes before a local branch.
- `column_is_the_rank_in_the_row`: with two edges that end, the column of one branch drops from 2 to 1 in the next row. The column is not a fixed lane.
- `edge_to_next_row_is_diagonal_center_to_center`.
- `color_of_the_head_chain_node_is_the_hash_of_the_name`: `master` gives `String.hashCode("master")`.
- `color_of_the_rest_is_the_index`.
- `edge_color_is_that_of_the_end_with_the_greater_index`.
- `edge_of_30_rows_is_hidden`: with Long Edges off, it is drawn only 1 row from each end, with a down arrow and an up arrow.
- `edge_of_29_rows_is_drawn_whole`.
- `with_long_edges_edges_of_1000_keep_250_per_end`.
- `arrow_leads_to_the_target_row`.
- `sweep_is_the_same_with_and_without_checkpoints`: on a random graph of 5,000 commits with a fixed seed, `elements_in_row` gives the same as a direct calculation without a cache.
- `difficult_graph_does_not_break`: octopus, multiple roots, shallow clone and empty repository.

TS:

- `palette_same_as_intellij`: `graphColor(1)` is `rgb(100,150,160)`, `graphColor(0)` black, `master` gives `(140,168,118)` and `origin/master` `(152,126,80)`.
- `measures_scale_with_the_row`: at 22 high, 15 wide, radius 4 and line 1.5.
- `selection_paints_the_brand_color_and_then_its_own`.

- [ ] **Step 2: Check that they fail**

Run: `pnpm vitest run src/log && (cd src-tauri && cargo test log::)`
Expected: FAIL

- [ ] **Step 3: Implement**

Port the cited algorithms. The constants are `LONG_EDGE=30`, `VISIBLE_PART=1` and `ARROW=∞` with Long Edges off, and `1000`, `250` and `30` on.

Save the active edges every 1,000 rows, like `EdgesInRowGenerator`, and compute each row by sweeping from the control point. Without this the memory cost is quadratic with long edges.

The color formula is `r=id*200+30, g=id*130+50, b=id*90+100` in a 32-bit integer with overflow, and each channel is `abs(n % 100)+70`, with the signed remainder.

The cell width is `max(max(pos,(pos+otherPos)/2)+1, min(6, recommendedWidth)) * nodeWidth`. The row height is 24, and all measures scale by `rowHeight/22`.

- [ ] **Step 4: Check that they pass and measure**

Run: `pnpm vitest run && (cd src-tauri && cargo test)`
Expected: PASS

With `git/git`, in release, `log_open` must stay under 2 s and `log_rows` of 50 rows under 5 ms at any point of the log. Write the measurement in `docs/reports/phase-2-graph.md`.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: log graph like IntelliJ"
```

### Task 5: Ordering and collapsing

Reference: `graph/impl/facade/bek/*`, `LinearBekController.kt`, `LinearBekGraphBuilder.java`, `CollapsedController`, `CollapsedActionManager.java:215`, `LinearFragmentGenerator.java:37`, `PermanentGraphImpl.java:60-68`.

**Files:**
- Create: `src-tauri/src/log/sort.rs`, `src-tauri/src/log/linear_bek.rs`, `src-tauri/src/log/collapse.rs`
- Modify: `src-tauri/src/log/view.rs`, `src-tauri/src/log/commands.rs`, `src/lib/api.ts`

**Interfaces:**
- Consumes: `LinearGraph`, `View`.
- Produces:
  - `SortType` equal to `Off | Standard | Linear`, `Off` by default.
  - `sort::bek_order(graph, layout, times) -> Vec<u32>`.
  - `linear_bek::collapse_merges(graph) -> LinearGraph`, with `MAX_BLOCK_SIZE=200`.
  - `collapse::collapse_all(view)`, `collapse::expand_all(view)`, `collapse::toggle(view, row)`.
  - Commands `log_view_set(view_id, patch)` and `log_graph_click(view_id, row)`.

- [ ] **Step 1: Write the failing tests**

- `off_keeps_gits_order`.
- `standard_leaves_what_was_merged_under_the_merge`: on a branch with two commits and a merge, both commits stay right below the merge.
- `standard_children_before_parents`, also on a random graph with a fixed seed.
- `linear_collapses_every_merge`.
- `linear_respects_the_maximum_block`: a block of 201 is not collapsed.
- `collapse_all_collapses_linear_fragments`: 5 linear commits between a merge and a fork end up in a dotted edge.
- `collapse_does_not_collapse_commits_with_a_reference`.
- `collapse_does_not_collapse_10_or_more`.
- `expand_all_returns_to_the_original_graph`.
- `click_collapses_and_expands_a_fragment`.
- `no_collapse_with_details_filter`.

- [ ] **Step 2: Check that they fail**

Run: `(cd src-tauri && cargo test log::)`
Expected: FAIL

- [ ] **Step 3: Implement**

Port the cited files, without simplifying them. In Linear order, the buttons say `Collapse Merges` and `Expand Merges`. In the others, `Collapse Linear Branches` and `Expand Linear Branches`.

- [ ] **Step 4: Check that they pass**

Run: `(cd src-tauri && cargo test)`
Expected: PASS

With `git/git`, changing the order must take less than 3 s. Add the measurement to `docs/reports/phase-2-graph.md`.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: log ordering and collapsing of linear branches"
```

### Task 6: Filters and search

Reference: `GitLogProvider.java:425-532`, `VcsLogFiltererImpl.kt:54-61,360-404`, `VcsLogFilters.kt:30-38`, `DottedFilterEdgesGenerator.kt`, `FilteredController`, and `highlighters/` for the highlights.

**Files:**
- Create: `src-tauri/src/log/filter.rs`, `src/log/FilterBar.svelte`, `src/log/filters.ts`, `src/log/filters.test.ts`, `src/log/FilterBar.test.ts`
- Modify: `src-tauri/src/log/view.rs`, `src-tauri/src/log/commands.rs`, `src/log/draw.ts`, `src/lib/api.ts`

**Interfaces:**
- Consumes: `View`, `Refs`, `CommitStore`.
- Produces:
  - `filter::LogFilter { branches: Vec<String>, users: Vec<String>, since: Option<i64>, until: Option<i64>, paths: Vec<String>, text: Option<TextFilter>, hashes: Vec<String>, range: Option<(String, String)> }`.
  - `TextFilter { text: String, regex: bool, match_case: bool }`.
  - `filter::git_args(filter: &LogFilter) -> Vec<String>`.
  - `filter::reachable(store, heads) -> BitVec`.
  - `filter::dotted_edges(graph, kept) -> LinearGraph`.
  - `log_view_open(spec) -> { viewId, summary }`, as a cancelable `Read` task. Opening another view cancels the previous one of that tab.
  - `log_users() -> Vec<Person>`, `log_paths_exist(paths) -> Vec<bool>`.
  - `Highlight` in `LogRow`: `bold` for My Commits, `muted` for merges and `branch` for the current branch.

- [ ] **Step 1: Write the failing tests**

Rust:

- `default_args_include_head_branches_remotes_tags`.
- `fixed_text_args`: `--grep=<text>`, `--fixed-strings`, `--regexp-ignore-case`.
- `regex_args_only_with_a_valid_metacharacter`: `foo` is not a regex, `fo+` is and `fo(` is not.
- `match_case_removes_ignore_case`.
- `author_args_escaped_and_anchored`: `--author=^Ana \(x\)$`, and several joined with `|`.
- `path_args_at_the_end`: `--full-history --simplify-merges -- <paths>`.
- `date_args`.
- `hex_text_is_tried_first_as_a_hash`: 7 to 40 hexadecimal characters, and if it does not exist it goes on as text.
- `in_memory_branch_filter_does_not_launch_git`: with only a branch filter, no new `git log` process, checked with a fake git that fails if it is called.
- `branch_filter_with_text_passes_the_branch_to_git`.
- `filtering_joins_what_remains_with_dotted_edges`.
- `filter_without_results_gives_empty_view`.
- `branch_name_with_dash_is_not_an_option`: `-x` is not interpreted as an option.
- `highlight_my_commits_only_if_there_are_more_authors`.

TS:

- `user_filter_me`, `date_last_24_hours`, `date_last_7_days`.
- `branch_popup_shows_select_head_recent_favorites_local_and_remotes`.
- `closing_a_filter_removes_it`, `regex_and_match_case_in_the_gear`.
- `typing_must_wait_for_enter`.

- [ ] **Step 2: Check that they fail**

Run: `pnpm vitest run src/log && (cd src-tauri && cargo test log::)`
Expected: FAIL

- [ ] **Step 3: Implement**

The bar is, in this order: text field with the Regex and Match Case gear, `Branch`, `User`, `Date`, `Paths`, and on the right Refresh, View Options and Go To. While the result arrives, the log keeps showing the previous view with an indicator. Git lists with `--format=%H`, and the view is built in memory from there.

The highlights come from the theme: `currentBranchBackground` is `#1c2433`, and merges are painted in gray.

- [ ] **Step 4: Check that they pass**

Run: `pnpm vitest run && (cd src-tauri && cargo test)`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: log filters and search"
```

### Task 7: Full log panel

Reference: `MainFrame.java:158-170`, `CommitDetailsPanel.kt:66-95`, `VcsLogChangesBrowser.kt:401-420`, `VcsStatusMerger.kt:24-46`, `GitDetailsCollector.kt:86`, `LabelPainter.java:34-43`, `VcsLogGraphTable.java:81,400-440`, `VcsLogApplicationSettings.kt:57,107-123`, `SelectionSnapshot.kt`, `CommitPresentationUtil.java`.

**Files:**
- Create: `src-tauri/src/log/details.rs`, `src/log/LogPanel.svelte`, `src/log/DetailsPanel.svelte`, `src/log/ChangesPanel.svelte`, `src/log/ViewOptions.svelte`, `src/log/labels.ts`, `src/log/labels.test.ts`, `src/log/LogPanel.test.ts`, `src/log/options.ts`, `src/log/selection.ts`, `src/log/selection.test.ts`
- Modify: `src/log/LogView.svelte`, `src/log/draw.ts`, `src/App.svelte`, `src/lib/keymap.ts`, `src/lib/api.ts`, `src-tauri/src/log/commands.rs`, `src-tauri/src/commands.rs`, `src-tauri/src/diff.rs`

**Interfaces:**
- Consumes: `FilterBar`, views.
- Produces:
  - `details::CommitDetails { hash, subject, body, author, author_email, author_time, committer, committer_email, commit_time, refs, parents }`, with the command `commit_details(hashes: Vec<String>)`, up to 50.
  - `branches_containing(hash) -> Vec<String>`, resolved in memory with the graph.
  - `commit_changes_of(hash, mode: "merged" | "parents") -> Vec<ChangeGroup>`.
  - `LogOptions`, per tab and persisted: `sort`, `compactRefs`, `tagNames`, `longEdges`, `preferCommitDate`, `refsOnLeft`, `columns`, `highlights`, `showDetails`, `diffPreview`, `diffPreviewAt`.
  - `snapshotSelection()` and `restoreSelection()` in `selection.ts`.

- [ ] **Step 1: Write the failing tests**

TS:

- `columns_subject_author_date_by_default`, and `hash_is_enabled_from_columns`.
- `author_with_asterisk_if_the_committer_is_someone_else`.
- `prefer_commit_timestamp_changes_the_date`.
- `compact_label_joins_groups`, `normal_label_one_per_group`, `long_name_is_shortened_to_22`.
- `show_tag_names_shows_the_name`, `references_on_the_left_moves_the_label`.
- `details_in_order_message_author_references_branches`.
- `details_with_a_different_committer_say_so`, `details_same_person_other_time`.
- `details_hash_in_the_message_is_a_link`.
- `in_n_branches_with_show_all`, `not_in_any_branch`.
- `several_commits_join_changes`.
- `merge_without_own_changes_offers_show_changes_to_parents`.
- `show_only_affected_changes_filters_by_paths`.
- `show_details_hides_the_panel`.
- `diff_preview_below_or_on_the_right`.
- `selection_survives_the_refresh`: saves by hash, and the commit returns to its same offset.
- `at_the_very_top_stays_on_top_with_new_commits`.
- `open_another_tab_starts_without_filters`.
- `opening_a_log_commit_opens_its_diff`.

Rust:

- `details_read_message_with_blank_lines`, `details_different_author_and_committer`.
- `merge_changes_are_those_against_all_parents`.
- `containing_branches_in_memory`.
- `refresh_only_if_the_fingerprint_changes`.

- [ ] **Step 2: Check that they fail**

Run: `pnpm vitest run src/log && (cd src-tauri && cargo test log::)`
Expected: FAIL

- [ ] **Step 3: Implement**

Replace the provisional section of `App.svelte` with `LogPanel`. Split the space like IntelliJ: table and right panel at 0.7, and changes and detail at 0.7, with dividers that are remembered.

Paint the labels on the canvas with these values: left and right margin 4, top 1, bottom 2, compact separation 2 and normal 12, radius 6, font 1 pt smaller. The colors in dark are HEAD `#e1c731`, local `#3cb45c`, remote `#9f79b5`, tag `#999999`.

On receiving `repo-changed`, compare `refs_fingerprint`. If it changed and the log is visible, reload. If it is not visible, mark the reload for when it is shown. Refresh (⌘R) always reloads.

- [ ] **Step 4: Check that they pass**

Run: `pnpm vitest run && (cd src-tauri && cargo test)`
Expected: PASS

- [ ] **Step 5: Check in the app**

With a throwaway repository with branches and tags, open the log, check graph, filters and detail, and save a screenshot in `.superpowers/`. If the screen is locked, note it in the task report.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: full log panel"
```

### Task 8: Log navigation and menu

Reference: `VcsLogNavigationUtil.kt:238-290`, `vcs-log.xml:181-191` of `platform/vcs-log/impl` and `plugin.xml:293-317` of git4idea.

**Files:**
- Create: `src/log/GoToRef.svelte`, `src/log/goto.ts`, `src/log/goto.test.ts`, `src/log/contextMenu.ts`, `src/log/contextMenu.test.ts`
- Modify: `src/log/LogPanel.svelte`, `src/log/LogView.svelte`, `src/lib/keymap.ts`, `src/lib/keymap.test.ts`, `src-tauri/src/log/commands.rs`, `src/lib/api.ts`

**Interfaces:**
- Consumes: `LogPanel`, `Refs`.
- Produces:
  - `resolveTarget(text, refs): { kind: "ref" | "hash" | "invalid", value }`.
  - `log_row_of(view_id, hash) -> Option<u32>`.
  - `contextMenu(selection, repoState, handlers): MenuItem[]`.
  - `ActionId` with `Vcs.Log.GoToRef`, `Vcs.Log.FocusTextFilter`, `Vcs.Log.Refresh`, `Vcs.Log.GoToParent`, `Vcs.Log.GoToChild`, `Vcs.Log.ShowTooltip`, `Vcs.CopyRevisionNumberAction`.

- [ ] **Step 1: Write the failing tests**

- `go_to_prefers_the_first_ref_that_starts_the_same`.
- `go_to_hash_of_four_or_more_hexadecimals`.
- `go_to_error_messages`: `'x' does not look like a commit hash`, `Commit or reference 'x' not found` and `… doesn't match the filters`.
- `right_arrow_goes_to_the_parent`, `left_arrow_goes_to_the_child`.
- `several_parents_open_a_numbered_popup`.
- `cmd_l_toggles_filter_and_table`.
- `copy_hash_and_subject`: several commits are copied separated by a line break.
- `menu_in_intellij_order_without_phase_4_items`.
- `checkout_in_the_menu_lists_branches_and_revision`.
- `push_all_up_to_here_only_on_the_current_branch`.
- `matrix_shortcuts`, extended with the shortcuts of this phase.

- [ ] **Step 2: Check that they fail**

Run: `pnpm vitest run src/log src/lib/keymap.test.ts`
Expected: FAIL

- [ ] **Step 3: Implement**

The menu has, in this order: `Copy Revision Number`, `Copy Commit Subject`, `Checkout ▸`, `Reset Current Branch to Here…`, `Push All up to Here…`, submenus of branches and tags, `New Branch…`, `New Tag…`, `Go to Child Commit` and `Go to Parent Commit`.

The entries of phases 3 and 4 are not shown.

The branch actions are linked to those of Task 9. Until then they call an injected function, and the test checks the call.

- [ ] **Step 4: Check that they pass**

Run: `pnpm vitest run`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: log navigation and context menu"
```

### Task 9: Branch, tag and reset operations

Reference: `GitBranchPopupActions.java:232,717`, `GitCheckoutOperation.java:118`, `GitBranchCheckoutOperation.kt:26`, `GitDeleteBranchOperation.java:90`, `GitNewBranchDialog.kt:43`, `GitRefNameValidator`, `GitNewResetDialog.kt`, `GitImpl.java:283`.

**Files:**
- Create: `src-tauri/src/branches.rs`, `src-tauri/src/preserve.rs`, `src-tauri/src/vcs_commands.rs`, `src/branches/refName.ts`, `src/branches/refName.test.ts`, `src/branches/NewBranchDialog.svelte`, `src/branches/RenameBranchDialog.svelte`, `src/branches/CheckoutProblemDialog.svelte`, `src/branches/LocalCommitsDialog.svelte`, `src/branches/DeleteRemoteDialog.svelte`, `src/branches/ResetDialog.svelte`, `src/branches/TagDialog.svelte`, `src/branches/CheckoutRefDialog.svelte`, `src/branches/actions.ts`, `src/branches/actions.test.ts`
- Modify: `src-tauri/src/lib.rs`, `src-tauri/src/snapshots.rs`, `src/log/contextMenu.ts`, `src/lib/api.ts`

**Interfaces:**
- Consumes: `task::Runner`, `snapshots::create`, `notify`.
- Produces:
  - `branches::checkout(repo, target, mode: CheckoutMode) -> Result<CheckoutOutcome, GitError>`, with `CheckoutMode` equal to `Plain | Smart | Force` and `CheckoutOutcome` equal to `Done | Overwritten { files: Vec<String> }`.
  - `branches::checkout_remote(repo, remote_branch) -> Result<RemoteCheckout, GitError>`, with `RemoteCheckout` equal to `Done | NeedsName | LocalHasCommits`.
  - `branches::create(repo, name, start, checkout, overwrite)`.
  - `branches::rename(repo, old, new)`.
  - `branches::delete(repo, name) -> Deleted { tip, upstream, forced, base }`, `branches::restore(repo, deleted)`.
  - `branches::delete_remote(repo, remote, branch, also_local)`.
  - `branches::reset(repo, target, mode: ResetMode)`, with `ResetMode` equal to `Soft | Mixed | Hard | Keep`.
  - `branches::tag(repo, name, target, message, force)`, `branches::delete_tag(repo, name)`.
  - `preserve::save(ctx, title) -> Result<Option<Saved>, GitError>`, `preserve::restore(ctx, saved) -> Result<RestoreOutcome, GitError>`, with `Saved { sha, message }` and `RestoreOutcome` equal to `Restored | Conflicts | Failed(String)`.

- [ ] **Step 1: Write the failing tests**

Rust:

- `checkout_local`: `git checkout <b> --`.
- `checkout_with_changes_it_overwrites_returns_files`, without touching anything.
- `smart_checkout_carries_the_changes`: after the switch, the file keeps what was local.
- `smart_checkout_with_conflict_leaves_the_stash`.
- `force_checkout_creates_a_copy_first`.
- `checkout_remote_creates_a_local_with_tracking`.
- `checkout_remote_with_local_without_own_commits_moves_the_branch`.
- `checkout_remote_with_own_commits_asks`.
- `new_branch_from_another_does_not_set_upstream`: uses `X^0`.
- `delete_unmerged_branch_forces_itself_and_says_so`: there is no prior question.
- `restore_returns_branch_and_tracking`.
- `delete_remote_with_ref_already_deleted_prunes`.
- `reset_hard_creates_a_copy_of_what_was_modified`.
- `reset_hard_includes_overwritten_unversioned`: an unversioned file that exists in the target goes into the copy.
- `reset_keep_with_conflict_returns_files`.
- `lightweight_and_annotated_tag`, `tag_force`, `delete_tag_and_restore`.
- `name_with_a_leading_dash_is_rejected`: it never reaches git as an option.
- `mutating_without_saving_the_diff_is_not_allowed`: every new command waits for `flush_diff_window`, checked the same as in phase 1.

TS:

- `name_is_cleaned_while_typing`: spaces become `-` and what is not valid is removed.
- `branch_dialog_errors`: `Specify name for the new branch`, `HEAD is a reserved keyword` and `Branch name {0} already exists.`.
- `overwrite_only_with_conflict`.
- `reset_remembers_the_mode_and_defaults_to_mixed`.
- `hard_asks_for_no_further_confirmation`.
- `delete_notification_offers_restore_and_view_commits`.

- [ ] **Step 2: Check that they fail**

Run: `pnpm vitest run src/branches && (cd src-tauri && cargo test branches:: preserve::)`
Expected: FAIL

- [ ] **Step 3: Implement**

`Git Checkout Problem` uses the buttons `Smart Checkout`, `Force Checkout` and `Don't Checkout`, with "IntelliJ IDEA" removed from the text.

`git stash save` carries the message `Uncommitted changes before Checkout at <date>`, and restores with `git stash pop --index`, or `pop` without `--index` if staging area is off.

Identify the entry by its sha: `git stash pop` receives `stash@{n}`, looked up in `git stash list --format=%H`. Another stash created by hand must not confuse it.

Deleting a local branch does not ask: `git branch -d`, and `git branch -D` if git answers `not fully merged`. The deletion notifies with `Restore`.

- [ ] **Step 4: Check that they pass**

Run: `pnpm vitest run && (cd src-tauri && cargo test)`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: branch, tag and reset operations"
```

### Task 10: Branches popup and panel

Reference: `GitBranchesTreePopup*`, `GitBranchesTreeModelUtil.kt:32,118,226`, `GitBranchesTreeRenderer.kt:305`, `GitBranchIncomingOutgoingManager.java:293-402`, `GitRecentCheckoutBranches.kt`, `BranchStorage.kt`, `GitBranchManager.java:24`, `ui/branch/dashboard/`.

**Files:**
- Create: `src-tauri/src/branch_info.rs`, `src/branches/BranchesPopup.svelte`, `src/branches/BranchesPopup.test.ts`, `src/branches/model.ts`, `src/branches/model.test.ts`, `src/branches/BranchesPanel.svelte`, `src/branches/BranchesPanel.test.ts`
- Modify: `src/App.svelte`, `src/log/LogPanel.svelte`, `src/lib/api.ts`, `src-tauri/src/vcs_commands.rs`, `src-tauri/src/settings.rs`, `src/settings/SettingsView.svelte`, `src/lib/keymap.ts`

**Interfaces:**
- Consumes: actions of Task 9, `Refs`.
- Produces:
  - `branch_info::list(repo) -> BranchList { local, remote, recent, current, previous }`.
  - `branch_info::incoming_outgoing(repo) -> HashMap<String, Sync>`, with `Sync { incoming: bool, outgoing: bool }`.
  - `branch_info::check_remotes(ctx, strategy)`, with `IncomingCheck` equal to `Auto | Always | Never`.
  - `favorites_get()`, `favorites_toggle(name, kind)`, saved per repository.
  - `buildTree(list, query, options): TreeNode[]` in `model.ts`.
  - `branchActions(branch, state): MenuItem[]`.

- [ ] **Step 1: Write the failing tests**

Rust:

- `recent_ones_come_from_the_reflog`: at most 5, with `git reflog --max-count 50 --grep-reflog checkout: --`.
- `outgoing_with_rev_list`, `incoming_with_rev_list`.
- `ls_remote_detects_commits_without_fetch`: with a `file://` remote that moved ahead, it marks incoming.
- `auto_skips_ssh_if_the_agent_is_not_the_system_one`.
- `never_does_not_launch_ls_remote`.
- `without_network_the_list_does_not_break`: a downed remote leaves the local data.
- `default_and_excluded_favorites`.

TS:

- `order_current_favorites_no_slash_name`.
- `opens_on_the_previous_branch`.
- `fuzzy_search_includes_origin`, `when_filtering_recent_ones_leave_local`, `without_results_shows_branch_not_found`.
- `groups_by_slash`, `local_open_remote_closed`.
- `enter_opens_the_submenu_and_does_not_checkout`.
- `space_toggles_favorite`.
- `submenu_in_order_without_phase_4_items`.
- `current_branch_only_has_new_branch_update_push_rename`.
- `icon_and_tooltip_of_incoming_and_outgoing`.
- `top_rows_update_commit_push_new_branch_checkout_tag`.
- `panel_double_click_changes_the_branch_filter`, `update_branch_filter_and_navigate_are_mutually_exclusive`.

- [ ] **Step 2: Check that they fail**

Run: `pnpm vitest run src/branches && (cd src-tauri && cargo test branch_info::)`
Expected: FAIL

- [ ] **Step 3: Implement**

The branch widget of the top bar opens the popup, like the New UI. The submenu does not include `Compare with` or `Show Diff with Working Tree` (phase 4), nor `Merge`, `Rebase` or `Checkout and Rebase` (phase 3).

The log's branches panel (`Git.ShowBranches`, `Git.Log.Hide.Branches`) reuses `buildTree`. `ls-remote` runs when the repository is opened, every 20 minutes and after a fetch from the popup, only with the app visible. That task never asks for credentials: its askpass answers with a failure without opening the dialog.

- [ ] **Step 4: Check that they pass**

Run: `pnpm vitest run && (cd src-tauri && cargo test)`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: branches popup and panel"
```

### Task 11: Fetch and Update Project

Reference: `GitUpdateProcess.java:123-260,358-417,473-526`, `GitUpdater.java:93-95,194-201`, `GitMergeUpdater.java:74-176`, `GitRebaseUpdater.java:47-123`, `GitRebaser.java:51-85`, `GitPreservingProcess.java:78-106`, `GitFetchSupportImpl.kt:172-351`, `MergeChangeCollector.java:51-162`, `GitUpdateInfoAsLog.kt`, `AbstractCommonUpdateAction.java:566-579`.

**Files:**
- Create: `src-tauri/src/fetch.rs`, `src-tauri/src/update.rs`, `src/update/UpdateDialog.svelte`, `src/update/FixTrackedDialog.svelte`, `src/update/flow.ts`, `src/update/flow.test.ts`
- Modify: `src-tauri/src/preserve.rs`, `src-tauri/src/vcs_commands.rs`, `src-tauri/src/settings.rs`, `src/lib/keymap.ts`, `src/lib/api.ts`, `src/App.svelte`

**Interfaces:**
- Consumes: `task::Runner`, `preserve`.
- Produces:
  - `fetch::fetch_all(ctx) -> FetchResult { pruned, failed }`. Per remote: `git fetch <remote> --recurse-submodules=no --progress --prune`.
  - `update::update_project(ctx, method: UpdateMethod) -> UpdateResult`, with `UpdateMethod` equal to `Merge | Rebase`.
  - `UpdateResult { outcome: Outcome, files: u32, commits: u32, range: Option<(String, String)>, saved: Option<Saved>, restore: RestoreOutcome }`.
  - `Outcome` equal to `Success | NothingToUpdate | Conflicts | NotReady(String) | NoTrackedBranch { branch } | Error(String) | Cancelled`.
  - `update::update_branch(ctx, local, remote, remote_branch)`, without touching the directory.
  - `Settings.update_method`, `Merge` by default.

- [ ] **Step 1: Write the failing tests**

- `fetch_runs_each_remote_with_prune`, `fetch_reports_deleted_branches`, `failed_fetch_keeps_stderr`.
- `without_incoming_it_does_nothing`: nothing is saved and it gives `NothingToUpdate`.
- `fast_merge`, `merge_with_own_commit`, `linear_rebase`.
- `merge_only_saves_if_the_changes_overlap`.
- `rebase_saves_any_change`.
- `rebase_with_changes_tries_ff_only_first`.
- `detached_head_gives_not_ready_with_the_intellij_text`.
- `without_a_tracked_branch_it_offers_to_choose_one`, `set_as_tracked_branch_sets_upstream`.
- `merge_in_progress_gives_not_ready`, `rebase_in_progress_gives_not_ready`.
- `conflict_leaves_merge_head_and_the_stash`.
- `conflict_in_rebase_leaves_the_rebase_state`.
- `unversioned_that_would_be_overwritten_gives_error_and_restores`.
- `cancelling_merge_does_reset_merge_and_restores`.
- `cancelling_rebase_does_abort_and_restores`.
- `devin_writes_while_it_is_saved`: with a fake git that modifies the file during the merge, restoring gives `Conflicts`, the stash entry stays and neither version is lost.
- `restoring_does_not_take_another_stash`.
- `counts_files_and_commits_of_the_range`: range from `merge-base(local, tracked)`.
- `update_of_a_non_current_branch_only_advances`: with divergence, it fails without touching anything.
- `staged_changes_return_to_the_stage`.

TS:

- `dialog_with_merge_and_rebase`, `dont_show_again_is_saved_on_accept`, `cancelling_does_not_save`.
- `shift_shows_it_even_if_hidden`.
- `result_n_files_updated_in_m_commits`, `all_files_are_up_to_date`.
- `fetch_error_gives_update_failed_and_update_canceled`.
- `saved_changes_offer_restore_changes`.
- `conflicts_offer_abort`.

- [ ] **Step 2: Check that they fail**

Run: `pnpm vitest run src/update && (cd src-tauri && cargo test update:: fetch:: preserve::)`
Expected: FAIL

- [ ] **Step 3: Implement**

The order is that of `GitUpdateProcess`: check state, tracked branch, fetch of the tracked remotes, see whether there are incoming changes, `--ff-only` if it is a rebase with changes, save, update, restore.

The commands are `git merge <remote/branch> --no-stat -v` and `git rebase <remote/branch>`. The `Rebasing Merge Commits` dialog uses its buttons `Merge`, `Rebase` and `Cancel`.

Restore also after `Error` and `Cancelled`, if the repository was not left with a merge or rebase in progress (decision 4). With conflicts do not restore, and leave the sticky notification `Local changes were not restored`, with `Restore Changes` (decision 5) and `Abort`.

Abort is `git reset --merge` or `git rebase --abort`. After aborting, do not restore on its own: the button remains available.

- [ ] **Step 4: Check that they pass**

Run: `pnpm vitest run && (cd src-tauri && cargo test)`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: fetch and Update Project"
```

### Task 12: Pull and update result

Reference: `GitPullDialog.kt:180-259`, `GitPullOption.kt`, `GitPull.java:94-161`, `GitMergeAction.java:168-262`, `GitUpdateSession.kt:73-115`, `GitUpdateInfoAsLog.kt:65-165`.

**Files:**
- Create: `src-tauri/src/pull.rs`, `src/update/PullDialog.svelte`, `src/update/PullDialog.test.ts`, `src/update/pullOptions.ts`, `src/update/pullOptions.test.ts`
- Modify: `src-tauri/src/vcs_commands.rs`, `src-tauri/src/settings.rs`, `src/log/LogPanel.svelte`, `src/App.svelte`, `src/settings/SettingsView.svelte`, `src/lib/api.ts`

**Interfaces:**
- Consumes: `update::update_project`, `LogFilter.range`, `Outcome`.
- Produces:
  - `pull::pull(ctx, remote, branch, options: Vec<PullOption>) -> UpdateResult`.
  - `PullOption` equal to `Rebase | FfOnly | NoFf | Squash | NoCommit | NoVerify`.
  - `incompatible(options): Set<PullOption>` in `pullOptions.ts`.
  - `Settings.update_paths: Vec<String>`, the filter of `Filter "Update Project" information by paths:`.
  - `Update Info` tab of the log, opened with `View Commits` with the preset range.

- [ ] **Step 1: Write the failing tests**

Rust:

- `pull_merge_uses_the_intellij_arguments`: `git pull --no-stat -v --progress <remote> <branch>`.
- `pull_without_remote_prefix`.
- `pull_rebase_goes_through_update`: with `--rebase` `git pull` is not launched.
- `pull_does_not_save_changes`.
- `pull_no_commit_leaves_a_prepared_merge`.

TS:

- `title_pull_to_branch`, `tracked_remote_and_branch_by_default`.
- `without_a_tracked_one_uses_the_one_with_the_same_name`.
- `selection_errors`: `Select the remote to pull from` and `The branch to pull from should be selected.`.
- `incompatible_options_greyed_out`.
- `options_are_remembered`.
- `reloading_branches_launches_fetch_of_the_remote`.
- `view_commits_opens_a_tab_with_the_range`.
- `path_filter_counts_matching_commits`.
- `project_partially_updated_if_it_does_not_finish`.

- [ ] **Step 2: Check that they fail**

Run: `pnpm vitest run src/update && (cd src-tauri && cargo test pull::)`
Expected: FAIL

- [ ] **Step 3: Implement**

The update ends in the notification `{N} file(s) updated in {M} commit(s)`, with `View Commits`. With a path filter it adds `{K} commits matching filters`. `Vcs.UpdateProject`, `UpdateFiles` and `Update` of the popup share the flow.

- [ ] **Step 4: Check that they pass**

Run: `pnpm vitest run && (cd src-tauri && cargo test)`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: pull and Update Info tab"
```

### Task 13: Push dialog

Reference: `GitPushOperation.java:379`, `GitPushNativeResultParser`, `GitImpl.java:505`, `dvcs/push/ui/*`, `GitPushSupport`, messages of `GitBundle.properties:721-734`.

**Files:**
- Create: `src-tauri/src/push.rs`, `src/push/PushDialog.svelte`, `src/push/PushDialog.test.ts`, `src/push/model.ts`, `src/push/model.test.ts`
- Modify: `src-tauri/src/vcs_commands.rs`, `src-tauri/src/settings.rs`, `src/lib/keymap.ts`, `src/lib/api.ts`, `src/App.svelte`, `src/log/contextMenu.ts`

**Interfaces:**
- Consumes: `task::Runner`, `FilePane` and the log's list of changes.
- Produces:
  - `push::target(repo, branch) -> PushTarget { remote, branch, is_new }`.
  - `push::outgoing(repo, source, target) -> Vec<CommitSummary>`.
  - `push::push(ctx, spec: PushSpec) -> PushResult`.
  - `PushSpec { source, remote, target, tags: Option<PushTags>, force: Option<ForceMode>, no_verify }`.
  - `PushTags` equal to `All | CurrentBranch`, `ForceMode` equal to `Lease | Plain`.
  - `PushResult { status: PushStatus, pushed: u32, source, destination }`.
  - `PushStatus` equal to `Success | NewBranch | UpToDate | Forced | RejectedNoFf | RejectedStale | RejectedOther(String) | Error(String)`.
  - `push::parse_porcelain(stdout: &str) -> Vec<PorcelainLine>`.
  - `push::has_pre_push_hook(repo) -> bool`.

- [ ] **Step 1: Write the failing tests**

Rust:

- `target_is_the_tracked_one`, `without_tracked_uses_origin_and_the_same_name`, `without_remotes_it_asks_to_define_one`.
- `new_branch_carries_set_upstream_and_full_path`.
- `push_arguments`: `git push --progress --porcelain <remote> refs/heads/x:x`.
- `tags_all_uses_tags`, `tags_current_uses_follow_tags`.
- `no_verify_only_if_unchecked_with_hook`.
- `push_up_to_commit_uses_hash`.
- `parser_success`, `parser_non_fast_forward_rejection`, `parser_fetch_first_rejection`, `parser_stale_info`, `parser_without_a_line_uses_stderr`.
- `up_to_date_branch_gives_up_to_date`.
- `real_push_to_a_bare_remote`.
- `outgoing_commits_of_a_new_branch`: `--not --remotes=<remote> --max-count=1000`.

TS:

- `title_push_commits_to_repo`.
- `row_branch_arrow_remote_and_editable_target`, `badge_new`.
- `shows_20_commits_and_the_rest_with_dots`.
- `selecting_a_commit_shows_changes_and_detail`.
- `run_git_hooks_only_with_a_hook`.
- `push_tags_remembers_the_choice`.
- `success_notifications_literal`, `everything_is_up_to_date`.
- `cmd_shift_k_opens_the_dialog`.
- `push_all_up_to_here_opens_with_that_commit`.

- [ ] **Step 2: Check that they fail**

Run: `pnpm vitest run src/push && (cd src-tauri && cargo test push::)`
Expected: FAIL

- [ ] **Step 3: Implement**

Do not add an upstream checkbox: it is automatic. `push` and `fetch` are `TaskKind::Remote(<remote>)`, because they do not touch the index.

- [ ] **Step 4: Check that they pass**

Run: `pnpm vitest run && (cd src-tauri && cargo test)`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: push dialog"
```

### Task 14: Force push, rejections and Commit and Push

Reference: `ForcePushAction.kt`, `GitSharedSettings.java:69`, `GitPushOperation.java:120-190`, `GitPushAfterCommitDialog.java:60`, `GitBundle.properties:721-734`.

**Files:**
- Create: `src/push/PushRejectedDialog.svelte`, `src/push/ForcePushDialog.svelte`, `src/push/force.ts`, `src/push/force.test.ts`, `src/push/commitAndPush.ts`, `src/push/commitAndPush.test.ts`
- Modify: `src-tauri/src/push.rs`, `src-tauri/src/settings.rs`, `src-tauri/src/storage.rs`, `src/push/PushDialog.svelte`, `src/commit/CommitMessage.svelte`, `src/commit/CommitView.svelte`, `src/commit/StagingView.svelte`, `src/commit/CommitView.test.ts`, `src/settings/SettingsView.svelte`, `src/lib/keymap.ts`

**Interfaces:**
- Consumes: `push::push`, `update::update_project`, `CommitView`.
- Produces:
  - `push::is_protected(patterns: &[String], branch: &str) -> bool`, with regex `^p$` over the name without the remote. An invalid pattern does not protect, and a warning is given when saving it.
  - `RepoSettings { protected_branches: Vec<String>, force_push_targets: Vec<String> }`, per repository. `["master", "main"]` by default.
  - `Settings.push_auto_update`, `preview_push_on_commit` (`true`), `preview_push_protected_only` (`false`).
  - `shouldPreview(settings, protectedTarget, canPush): boolean`.
  - `CommitMessage` with `Commit and Push…` active and ⌘⌥K bound.

- [ ] **Step 1: Write the failing tests**

Rust:

- `main_and_master_protected_by_default`.
- `pattern_is_an_anchored_regex`: `release.*` protects `release-1`, and `master` does not protect `master2`.
- `invalid_pattern_does_not_break`.
- `force_with_lease_carries_branch_and_sha`: `--force-with-lease=<branch>:<sha>`.
- `force_with_lease_without_tracked_carries_an_empty_sha`.
- `protected_branch_rejects_force_in_the_core`: even if the interface asked for it, `push` returns an error and does not launch git.
- `stale_lease_is_rejected_stale`: with a real remote that moved ahead, it overwrites nothing.
- `retry_after_updating_pushes`: rejection, update by merge and a second real push.
- `retries_limited_to_10`.
- `it_does_not_retry_on_force_or_on_a_non_current_branch`.

TS:

- `force_push_button_in_the_dropdown`.
- `protected_disables_with_force_push_prohibited`.
- `force_push_dialog_literal`, `dont_warn_remembers_the_target`.
- `force_push_anyway_without_confirming`.
- `rejection_merge_rebase_cancel_literal`.
- `remember_saves_method_and_autoupdate`.
- `unpushed_merges_recommend_merge_and_rebase_anyway`.
- `commit_and_push_opens_dialog_by_default`.
- `commit_and_push_without_dialog_if_only_protected`.
- `commit_and_push_with_amend_warns_of_rejection`.
- `failed_commit_does_not_push`.

- [ ] **Step 2: Check that they fail**

Run: `pnpm vitest run src/push src/commit && (cd src-tauri && cargo test push::)`
Expected: FAIL

- [ ] **Step 3: Implement**

`Force Push` uses the text `You're going to force push to {branch}. It may overwrite commits at the remote. Are you sure you want to proceed?`, with `Don't warn about this target`. With the protected branch, the button comes out disabled with `Force push to {target} is prohibited`.

Put in Settings: `Update method:`, `Clean working tree using:` with Stash fixed (decision 2), `Explicitly check for incoming commits on remotes:`, `Protected branches:`, `Show Push dialog for Commit and Push`, `Show Push dialog only when committing to protected branches`, `Filter "Update Project" information by paths:` and `Auto-update if push of the current branch was rejected`.

- [ ] **Step 4: Check that they pass**

Run: `pnpm vitest run && (cd src-tauri && cargo test)`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: force push, rejections and Commit and Push"
```

### Task 15: Remotes, clone and init

Reference: `GitConfigureRemotesDialog`, `GitDefineRemoteDialog`, `GitImpl.java:184`, `GitInit`, action `Git.OpenExcludeFile`.

**Files:**
- Create: `src-tauri/src/remotes.rs`, `src/remotes/RemotesDialog.svelte`, `src/remotes/DefineRemoteDialog.svelte`, `src/remotes/CloneDialog.svelte`, `src/remotes/remotes.test.ts`
- Modify: `src-tauri/src/vcs_commands.rs`, `src-tauri/src/lib.rs`, `src/App.svelte`, `src/lib/api.ts`, `src/push/PushDialog.svelte`

**Interfaces:**
- Consumes: `task::Runner`, `repo_open`.
- Produces:
  - `remotes::list(repo) -> Vec<Remote>`, with `Remote { name, url, push_url }`.
  - `remotes::add`, `remotes::edit`, `remotes::remove`, `remotes::test_url(ctx, url)`.
  - `remotes::clone(ctx, url, dir)` and `remotes::init(path)`.
  - `open_exclude_file()`.
  - Menu bar: `Git` with `Clone…`, `Create Git Repository…`, `Manage Remotes…` and `Open .git/info/exclude`.

- [ ] **Step 1: Write the failing tests**

Rust:

- `add_rename_seturl_remove`.
- `order_with_origin_first`.
- `url_is_tested_with_ls_remote`.
- `url_with_a_leading_dash_is_rejected`: `-oProxyCommand=…` does not reach git.
- `ext_url_is_rejected`: `ext::` and other execution transports fail, because git blocks them by default on clone. The test pins down that `protocol.allow` is not changed.
- `clone_with_recurse_submodules`: `git clone --progress --recurse-submodules <url> -- <dir>`.
- `cancelled_clone_deletes_only_what_it_created`: if the directory already existed, it is still there.
- `init_creates_a_repository`, `init_in_a_repository_warns`.
- `nonexistent_exclude_is_silent`.

TS:

- `literal_remote_errors`: `Remote name cannot be empty`, `Remote name '{0}' is already used` and `Remote URL cannot be empty`.
- `confirm_remove_remote_literal`.
- `clone_opens_the_repository_when_done`.

- [ ] **Step 2: Check that they fail**

Run: `pnpm vitest run src/remotes && (cd src-tauri && cargo test remotes::)`
Expected: FAIL

- [ ] **Step 3: Implement**

Open the cloned repository in the current window if there is none, and in a new window if there is. `Open .git/info/exclude` uses `open -t`: the app has no editor.

- [ ] **Step 4: Check that they pass**

Run: `pnpm vitest run && (cd src-tauri && cargo test)`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: remotes, clone and init"
```

### Task 16: Phase validation

**Files:**
- Create: `docs/reports/phase-2-validation.md`, `docs/reports/phase-2-commands.md`, `docs/reports/phase-2-consumption.json`
- Modify: `docs/parity-matrix.md`, `scripts/extract_actions.py`, `scripts/test_extract_actions.py`, `README.md`, `docs/superpowers/specs/2026-10-31-intellij-git-client-design.md`, `src/perf/run.ts`

**Interfaces:**
- Consumes: the complete app.
- Produces: the matrix with the result column for the phase 2 rows.

- [ ] **Step 1: Fix the matrix and the classifier**

Apply decisions 2, 7 and 11, and note the 12 decisions in section 4 of the design. Also fix the three `Vcs.Log.*` rows with a different phase noted in phase 1. Keep the Result column when regenerating. First add `test_show_all_affected_in_phase_4` and check that it fails.

- [ ] **Step 2: Compare commands**

With `scripts/git-wrapper.sh` and a throwaway repository, record what the app launches in checkout, new branch, fetch, update, pull, push and force push. Compare with the commands cited in this plan and note the differences in `docs/reports/phase-2-commands.md`.

- [ ] **Step 3: Measure again**

Update `src/perf/run.ts` to open `LogPanel`. Measure `git/git`.

Run: `python3 scripts/measure.py --app src-tauri/target/release/rama --repo ~/rama-medida/git --out docs/reports/phase-2-consumption.json --runs 5`
Expected: JSON with memory, CPU, startup and scroll, and `window_visible: true`.

If the screen is locked, leave written which values were not measured, as in phase 1.

- [ ] **Step 4: Go through the matrix**

For each phase 2 row, note `done`, `partial` or `pending`, citing the test. "Confirm by hand" indicates what only a person can check.

- [ ] **Step 5: Write the report**

Compare with phase 1 and with IntelliJ. List what could not be checked: real network, GitHub, SSH with agent, Finder and physical keyboard. Include a manual checklist for real repositories.

- [ ] **Step 6: Hand over to the user**

Build the `.app` and say how to open it. The phase ends when the user confirms it in their real repositories.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "docs: phase 2 validation"
```
