# Several project windows in one process: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One Ramify process with several project windows, each with its own repository, like IntelliJ. This includes the This Window / New Window question, reopening on startup, a menu bar that follows the focused window, and the project widget.

**Architecture:** Each main window owns a `WindowSession` in `AppState`. Commands get the calling `WebviewWindow` injected by Tauri, so the frontend's `invoke` calls do not change.
- Every per-repository piece moves from the app to the session: repository, log, watcher, auxiliary windows, tasks, console, askpass and editor prompts.
- Events go only to the owner's windows.
- The first window keeps today's labels, so one-window use never stops working between tasks.

**Tech Stack:** Tauri 2.12.5, Rust, Svelte 5, TypeScript, vitest + jsdom.

**Spec:** `docs/superpowers/specs/2026-11-10-multi-window-design.md`. Read it before any task. Section numbers below (§) refer to it.

## Global Constraints

- **Language:** everything written to the repository is in English.
- **Names:** `Ramify` in prose and UI, `ramify` in identifiers.
- **No lost work:** a recoverable copy before reset, force checkout, abort or deleting changes.
- **Before writing:** every command that changes files or the index calls `flush_diff_window` (now for its own window) and takes `RepoLocks` first.
- **Git:** real git with arguments, no shell. Nothing from the UI reaches git as an option; validate in Rust.
- **Secrets:** a password or token never goes to disk, console, error, argument, env or URL. Repository texts are shown as text.
- **Long flows** check that the repository did not change while they waited.
- **Tests** use throwaway repositories and local remotes. They never write to `.idea/` or `~/Library/Application Support/JetBrains`.
- **Same confirmations as IntelliJ.** Any difference is written down with its cost.
- **Do not change system defaults.**
- **Load rules:** one building or testing agent at a time; `nice -n 15`, `CARGO_BUILD_JOBS=3`, `cargo test -- --test-threads=3`, `vitest --maxWorkers=2`. Run the integration tests in every verification.
- **Toolchain:** `PATH=$HOME/.cargo/bin:$HOME/.cache/ramify-toolchain/node-v22.23.4-darwin-arm64/bin:$PATH`.
- **Every task ends green and with one-window use unchanged**, unless the task's own goal is to change it.
- **Source-pinning tests** (`include_str!` of sources: the flush line, the first line of `repo_open`, the `pub async fn` count in `vcs_commands.rs`, the shelve guards, `owns_watcher`, `action_window`) are rewritten to the new shapes in the task that changes the shape. They are never deleted.
- **Formatting:** `rustfmt --edition 2021` only on the files a task touches. The crate has older unformatted files; do not reformat them.

**The verification block.** "Run the verification block" means, in order, from the repo root:

```bash
export PATH=$HOME/.cargo/bin:$HOME/.cache/ramify-toolchain/node-v22.23.4-darwin-arm64/bin:$PATH CARGO_BUILD_JOBS=3
nice -n 15 pnpm exec tsc --noEmit -p .
nice -n 15 pnpm vitest run --maxWorkers=2
(cd src-tauri && nice -n 15 cargo clippy --all-targets -- -D warnings)
(cd src-tauri && nice -n 15 cargo test -- --test-threads=3)
python3 -I -m unittest discover -s scripts
```

Expected: tsc exit 0; every vitest file passes; clippy prints no warning; every `test result` line says `0 failed`; python prints `OK`.

## How Tauri routes events (checked in tauri-2.12.5, `manager/mod.rs` and `event/listener.rs`)

- `app.emit(e)` reaches every listener, whatever its target.
- `emit_to(label, e)` reaches listeners whose target is that label, **and** every listener registered with target `Any`.
- `listen(e, h)` in `@tauri-apps/api/event` registers with `Any`. `listen(e, h, { target: label })` only hears events sent to that label, plus broadcasts.

Hence: per-window events use `emit_to` per label, and every per-window frontend listener passes `{ target: ownLabel }`. Task 1 pins this with a test of the frontend helper; the Rust side is pinned by these notes and by the label tests.

## Review Focus

1. **Two windows, a write in window A while window B's diff window holds unsaved text.** Only A's diff window is flushed; B's text stays. Test in Task 3.
2. **Closing window A while its push waits for a password.** The prompt shows in A, the close waits for the write, and window B is not touched. Test in Task 9.
3. **Opening a repository by a subfolder, a trailing slash or a symlink of a repository already open in another window.** That window is focused and nothing opens twice. Test in Task 8.
4. **Quitting while window B (not focused) has a half-done merge window.** The question appears in B's merge window and a refusal stops the quit for every window. Test in Task 9.
5. **A Git menu action while an auxiliary window of B is focused.** The action goes to B's main window or B's viewer, never to A. Test in Task 10.

---

### Task 1: Window labels, owners and per-window listening

**Files:**
- Create: `src-tauri/src/windows.rs`; `src/lib/windows.ts`; `src/lib/windows.test.ts`
- Modify: `src-tauri/src/lib.rs` (module), `src-tauri/capabilities/default.json`, `choosers.json`, `selection-history.json`, `src/lib/api.ts` (`ownWindow`)

**Interfaces:**
- Produces (Rust, `windows.rs`):
  - `pub enum WindowKind { Main, Diff, Merge, Viewer, SelectionHistory }`
  - `pub fn owner_of(label: &str) -> Option<String>`
  - `pub fn kind_of(label: &str) -> Option<WindowKind>`
  - `pub fn label_for(kind: WindowKind, owner: &str) -> String`
  - `pub fn owner_group(owner: &str) -> [String; 5]`: the five labels of one project window.
  - `pub fn next_main_label(taken: &HashSet<String>, counter: &AtomicU32) -> String`: `main` first, then `main-2`, `main-3`…; never reuses a number in the process.
- Produces (TS, `windows.ts`):
  - `ownLabel(): string`: from `getCurrentWebviewWindow().label`.
  - `ownerOf(label)`, `ownerGroup(owner)`, mirroring Rust.
  - `listenOwn<T>(event, handler)`: `listen` with `{ target: ownLabel() }`.
  - `emitToOwner(event, payload?)`: `emitTo` each label of `ownerGroup(ownerOf(ownLabel()))`.

- [ ] **Step 1: Write the Rust label tests** in `windows.rs`:
  - `owner_of("main") == Some("main")`, `owner_of("diff") == Some("main")`, `owner_of("selection-history") == Some("main")`;
  - `owner_of("merge-3") == Some("main-3")`, `owner_of("viewer-12") == Some("main-12")`;
  - `owner_of("main-0")`, `owner_of("diff-")`, `owner_of("other")`, `owner_of("selection-history-x")` are all `None`;
  - `label_for(Diff, "main-2") == "diff-2"`, and `label_for(Main, "main") == "main"`;
  - `next_main_label` gives `main`, then `main-2`, `main-3`; after `main-2` is removed from `taken` the next is `main-4`;
  - a label never maps to an owner whose suffix differs.
- [ ] **Step 2: Run** `cd src-tauri && cargo test --lib windows::`. Expected: does not compile (functions missing).
- [ ] **Step 3: Implement** `windows.rs`. Parse `<kind>` or `<kind>-<n>` with n ≥ 2 written without leading zeros.
- [ ] **Step 4: Run** the same command. Expected: PASS.
- [ ] **Step 5: Write `src/lib/windows.test.ts`.** Mock `@tauri-apps/api/webviewWindow` (`getCurrentWebviewWindow` → `{ label: "diff-2" }`) and `@tauri-apps/api/event`. Assert:
  - `ownerOf` gives the same answers as the Rust test;
  - `listenOwn("x", h)` calls `listen("x", h, { target: "diff-2" })`;
  - `emitToOwner("y", 1)` calls `emitTo` for `main-2`, `diff-2`, `merge-2`, `viewer-2`, `selection-history-2`.
- [ ] **Step 6: Implement `windows.ts`.** Make `ownWindow()` in `api.ts` return `ownLabel()`, and rename its call sites to `ownLabel`. Today's `?window=` keeps selecting what `main.ts` loads.
- [ ] **Step 7: Capabilities.** Change each window list to globs: `"main*"`, `"diff*"`, `"merge*"`, `"viewer*"`, `"selection-history*"`. Keep each file's permissions as they are; `choosers.json` keeps only `main*`.
- [ ] **Step 8: Run the verification block.** Expected: all green, no behavior change.
- [ ] **Step 9: Commit** `feat: window labels and owners for several project windows`.

### Task 2: Per-window sessions in the backend (commands.rs and the small modules)

**Files:**
- Modify: `src-tauri/src/commands.rs`, `log/commands.rs`, `compare.rs`, `viewer.rs`, `annotate.rs`, `block_history.rs`, `tree.rs`, `get_version.rs`

**Interfaces:**
- Consumes: `windows::owner_of`.
- Produces (commands.rs):
  - `pub struct WindowSession`, with fields:
    - `repo: Mutex<Option<RepoSession>>`, `log: SharedLog`, `watcher: Mutex<Option<Watcher>>`;
    - `diff_request: Mutex<Option<DiffRequest>>`, `diff_flush: Mutex<VecDeque<Sender<bool>>>`;
    - `merge_session: Mutex<Option<MergeSession>>`, `merge_leave: Mutex<VecDeque<Sender<LeaveReply>>>`, `merge_ready: Mutex<bool>`.
  - `AppState.sessions: Mutex<HashMap<String, Arc<WindowSession>>>`. The fields above leave `AppState`.
  - `pub(crate) fn session_of(state: &AppState, label: &str) -> Result<Arc<WindowSession>, ApiError>`: resolves the owner and creates the session on first use; an unknown label is an error.
  - `pub(crate) fn current_repo(state: &State<AppState>, window: &WebviewWindow) -> Result<PathBuf, ApiError>`.
  - `repo_data_dir(app, state, window)` and `staging_context(app, state, window)` take the window the same way.
  - `flush_diff_window(app, state, window, action)` and `leave_merge_window(app, state, window, action)` act on the owner's diff and merge windows.
  - Temporary shim: `pub(crate) fn main_repo(state) -> Result<PathBuf, ApiError>` reads the `main` session for the files Task 3 migrates. Mark it `// Removed in Task 3`.

- [ ] **Step 1: Write the failing tests** in `commands.rs` tests:
  - `two_sessions_keep_their_repository`: two `TempRepo`s, sessions `main` and `main-2`, set each repo. `current_repo` for a `main-2`-owned label (use `diff-2` through `session_of`) gives the second repository; for `main`, the first.
  - `a_label_without_owner_has_no_session`: `session_of(&state, "other")` is `Err`.
- [ ] **Step 2: Run** `cargo test --lib two_sessions_keep_their_repository a_label_without_owner_has_no_session`. Expected: does not compile.
- [ ] **Step 3: Implement** `WindowSession` and `session_of`, and move the fields.
- [ ] **Step 4: Add `window: WebviewWindow` to every Tauri command** in the files of this task that reads the repository, the log or an auxiliary window. Pass it to the helpers.
  - `repo_open` writes `session_of(window).repo` instead of `state.repo`.
  - `resume_watching`, `refresh_recent_menu`, `owns_watcher` and `RunEvent::Reopen` use the `main` session for now (Task 9 generalises).
- [ ] **Step 5: Rewrite the source-pinning tests** that read these files to the new line shapes, keeping what they assert: a flush before every write and the first line of `repo_open`.
- [ ] **Step 6: Run the verification block.** Expected: green.
- [ ] **Step 7: Commit** `refactor: per-window sessions for the repository, log, watcher, diff and merge`.

### Task 3: Per-window sessions in vcs_commands and hosting

**Files:**
- Modify: `src-tauri/src/vcs_commands.rs`, `hosting/commands.rs`, `log/signatures.rs`, `commands.rs` (remove the shim)

**Interfaces:**
- Consumes: Task 2's `current_repo(state, window)`, `repo_data_dir(…, window)`, `flush_diff_window(…, window, …)`.
- Produces:
  - `start`, `read`, `read_locked`, `read_as` and `start_on_shelf` in `vcs_commands.rs` take `window: &WebviewWindow`.
  - `still_open(app, window, repo)` and `is_open(state, window, repo)`.
  - hosting's `opened(app, state, window)` and `connection_of(…, window)`.
  - `SignatureLoads.pending` is keyed by `(String /* owner */, u32)`.
  - `main_repo` is deleted.

- [ ] **Step 1: Write the failing test** `a_write_flushes_only_its_own_diff_window` (Review Focus 1), in `commands.rs` tests. With sessions `main` and `main-2`, `flush_diff_window` for `main-2` must:
  - enqueue its sender only in the `main-2` session;
  - target label `diff-2`;
  - return at once when `diff-2` does not exist;
  - leave `main`'s `diff_flush` queue untouched.
- [ ] **Step 2: Run it.** Expected: FAIL. With the shim, the flush still reads `main`.
- [ ] **Step 3: Migrate** every command and helper of these files to the injected window, and delete `main_repo`. Rewrite the pinning tests:
  - the `pub async fn` count stays 137;
  - the shelve guards now read `current_repo(&state, &window)? != repo`;
  - the flush line has the window argument.
- [ ] **Step 4: Run the verification block.** Expected: green.
- [ ] **Step 5: Commit** `refactor: vcs and hosting commands act on their own window's repository`.

### Task 4: Events go to the owner's windows

**Files:**
- Modify: `src-tauri/src/commands.rs`, `vcs_commands.rs`, `annotate.rs`, `block_history.rs`, `watch.rs` (notifier); `src/lib/api.ts` and every listener the inventory lists (App.svelte; ChangesPanel, ProjectTree, ConflictsDialog, LogPanel, BranchesPopup, BranchesPanel, pulls/branch.ts, GpgKeyRow, AskpassDialog, MessageEditorDialog, ConsoleView, conflicts/flow.ts, patch/flow.ts, DiffWindow, MergeWindow, ViewerWindow, SelectionHistoryWindow, tasks.ts)
- Test: `src-tauri/src/commands.rs` tests, `src/App.test.ts`, `src/lib/windows.test.ts`

**Interfaces:**
- Consumes: `windows::owner_group`, `listenOwn`, `emitToOwner`.
- Produces: `pub fn emit_to_owner<S: Serialize + Clone>(app: &AppHandle, owner: &str, event: &str, payload: S)`, which sends to each existing label of `owner_group(owner)`.
- The notifier and `repo_changed` take the owner. `RepoChange` stays as it is.
- `repo-changed`, `branch-sync-changed`, `merge-done`, `leave-refused`, `git-action`, `git-file-action`, `annotation-show-in-log`, `selection-history-show-affected`, `open-*` and the diff and merge requests use `emit_to_owner`.
- `settings-changed` stays `app.emit`. `app-activated`/`app-deactivated` go to every main label.

- [ ] **Step 1: Write the failing frontend test** in `App.test.ts`: `events of another window are not heard`. Mock `getCurrentWebviewWindow` → `main`. Assert that:
  - the `listen` calls for `repo-changed`, `open-recent`, `open-project-chooser`, `open-settings`, `leave-refused`, `app-activated`, `git-action` all carry `{ target: "main" }`;
  - `settings-changed` has no target.
- [ ] **Step 2: Run it.** Expected: FAIL.
- [ ] **Step 3: Replace** every per-window `listen` with `listenOwn`, and every frontend `emit` of `notifyRepoChanged`, `selection-*` and `diff-*` with `emitToOwner`. The explicit app-wide exceptions are `settings-changed` and the `app-*` pair; list them in a comment in `windows.ts`.
- [ ] **Step 4: Backend.** Replace each `emit_to(MAIN_WINDOW, …)`, the `app.emit("repo-changed")` and the targeted aux emits with `emit_to_owner` and the owner of the calling window, task or watcher. `action_window` returns the focused owner's main window or its viewer (Task 10 adds the focus part; here it uses `main`).
- [ ] **Step 5: Rust test** `emit_targets_are_the_owner_group`: `owner_group("main-2")` gives the five labels; `emit_to_owner` with no existing window does not fail.
- [ ] **Step 6: Run the verification block.** Expected: green.
- [ ] **Step 7: Commit** `feat: per-window events`.

### Task 5: Tasks, console, askpass and editor belong to a window

**Files:**
- Modify: `src-tauri/src/task.rs`, `console.rs`, `askpass.rs`, `editor.rs`, `commands.rs` (`forward_task_events`, `start_askpass`, `start_editor`, `task_cancel`, `console_lines`), `vcs_commands.rs` (pass the owner to `start`)
- Test: `task.rs`, `console.rs`, `askpass.rs`, `editor.rs` tests

**Interfaces:**
- Produces:
  - `Runner::start(&self, owner: &str, repo, kind, title, work)`; `TaskEvent.owner: String`.
  - `Runner::cancel(&self, owner: &str, id) -> bool`: false for another owner's task.
  - `Runner::finish_writes_of(&self, owner: &str, limit: Duration) -> bool`: cancels that owner's reads, waits for its writes and refuses new writes **for that owner only**. `finish_writes` keeps its meaning for quit.
  - `ConsoleLine.owner: String`; `Console::command(&self, owner, repo, args)`, `Console::output(&self, owner, text)`, `Console::lines_of(&self, owner) -> Vec<ConsoleLine>`. The 2,000 limit applies per owner.
  - askpass: `Server::register(&self, owner: String) -> (ClientEnv, AskpassRegistration)`, a per-task token dropped with the registration. `on_prompt(owner, id, kind)`. `askpass_answer` checks that the answering window's owner matches.
  - editor: `on_request(owner, id, prompt)`, with the owner taken from the per-task registration.

- [ ] **Step 1: Write the failing tests:**
  - `task.rs` `a_task_event_carries_its_owner`.
  - `task.rs` `cancel_refuses_another_windows_task`.
  - `task.rs` `finish_writes_of_waits_for_its_writes_and_leaves_others_running`: a write of `main-2` is waited for, a read of `main-2` is cancelled, a write of `main` starts and finishes normally during the wait, and after it `main-2` cannot start a write while `main` can.
  - `console.rs` `lines_are_kept_per_window` (also the per-owner 2,000 limit).
  - `askpass.rs` `a_prompt_reaches_the_owner_of_its_token` (two registrations, two owners).
  - `askpass.rs` `an_unknown_token_is_refused`.
  - `editor.rs` `the_request_names_its_owner`.
- [ ] **Step 2: Run them.** Expected: do not compile.
- [ ] **Step 3: Implement.** A git process outside a task (a blocking read that may ask for credentials) registers with the owner of the command that started it.
- [ ] **Step 4: Frontend.** `tasks.ts` and `ConsoleView` already listen per window after Task 4. `console_lines` returns `lines_of(owner)`.
- [ ] **Step 5: Run the verification block.** Expected: green.
- [ ] **Step 6: Commit** `feat: tasks, console and git prompts belong to their window`.

### Task 6: Auxiliary windows per project window

**Files:**
- Modify: `src-tauri/src/commands.rs` (diff and merge builders, `repo_open`, `close_main`), `viewer.rs` (`PENDING` → session), `block_history.rs` (`PENDING`/`RUNNING` → session), `src/main.ts`

**Interfaces:**
- Consumes: `windows::label_for`, `session_of`.
- Produces:
  - The diff, merge, viewer and selection history windows are built with `label_for(kind, owner)` and the same `?window=` URL as today.
  - `WindowSession` gains `viewer_pending: Mutex<Vec<ViewerTarget>>`, `selection_pending` and `selection_running` (moved from the statics).
  - `repo_open` and `close_main` destroy and close only the owner's auxiliary windows.
  - `close_main` also hides `selection-history`.

- [ ] **Step 1: Write the failing test** `opening_a_repository_closes_only_its_own_auxiliary_windows`. Unit-test the helper that lists which labels `repo_open` destroys for owner `main-2`: exactly `diff-2`, `merge-2`, `viewer-2`, `selection-history-2`. Plus `close_main_hides_the_selection_history_too` on the label list it hides.
- [ ] **Step 2: Run them.** Expected: FAIL.
- [ ] **Step 3: Implement**, removing the two statics.
- [ ] **Step 4: Run the verification block.** Expected: green.
- [ ] **Step 5: Commit** `feat: diff, merge, viewer and selection history windows per project window`.

### Task 7: The two settings

**Files:**
- Modify: `src-tauri/src/settings.rs`, `src/lib/api.ts` (`Settings`), `src/settings/SettingsView.svelte`, the `SETTINGS` fixture in `src/App.test.ts`
- Test: `settings.rs` tests, `src/settings/SettingsView.test.ts`

**Interfaces:**
- Produces:
  - `Settings.open_project_in: OpenProjectIn`, an enum serialized as `"ask" | "new" | "current"`, default `Ask`. TS: `openProjectIn`.
  - `Settings.reopen_projects: bool`, default `true`. TS: `reopenProjects`.
  - An older `settings.json` without them reads with the defaults.

- [ ] **Step 1: Write the failing tests:**
  - `settings.rs` `an_old_settings_file_gets_the_window_defaults`.
  - `SettingsView.test.ts` `the System group sets where projects open and whether they reopen`. The labels are exactly `Open project in`, `New window`, `Current window`, `Ask`, `Reopen projects on startup`, and saving sends the new values.
- [ ] **Step 2: Run them.** Expected: FAIL.
- [ ] **Step 3: Implement**, with a `System` group in the Settings view.
- [ ] **Step 4: Run the verification block.** Expected: green.
- [ ] **Step 5: Commit** `feat: Open project in and Reopen projects on startup settings`.

### Task 8: The open flow and new windows

**Files:**
- Create: `src/project/OpenProjectDialog.svelte`, `src/project/open.ts`, `src/project/open.test.ts`
- Modify: `src-tauri/src/commands.rs` (new commands), `src-tauri/src/lib.rs` (`RunEvent::Opened`), `src/App.svelte` (Open, typed path, chooser, Recent), `src/remotes/model.ts` (`openRepository`), `src/lib/api.ts`

**Interfaces:**
- Consumes: Task 7's settings, `next_main_label`, `session_of`.
- Produces (Rust):
  - `#[tauri::command] project_resolve(target: String) -> Result<String, ApiError>`: the canonical repository root, as `repo_info` gives it.
  - `#[tauri::command] project_focus_open(root: String, window) -> Result<bool, ApiError>`: if another window's session holds that root (compared with `fs::canonicalize`), shows and focuses that main window and returns true.
  - `#[tauri::command] project_new_window(root: String) -> Result<String, ApiError>`: builds the next main window, stores `root` as that label's initial target and returns the label.
  - `initial_target` becomes per label: a stored target for the calling label wins over argv, which only the first `main` reads.
- Produces (TS, `open.ts`): `openProject(target: string, deps): Promise<void>`. In order:
  1. resolve;
  2. `project_focus_open`, and stop if true;
  3. no repository here → open here;
  4. else by `openProjectIn`: `current` → here; `new` → `project_new_window`; `ask` → `OpenProjectDialog`.
- Dialog: title `Open Project`; message `Where would you like to open the project '<folder name>'?`; buttons `This Window` (default), `New Window`, `Cancel`; checkbox `Don't ask again`, which saves `openProjectIn` as `current` or `new`.

- [ ] **Step 1: Write the failing tests** in `open.test.ts` (dependencies injected):
  - `an already open repository focuses its window and opens nothing` (Review Focus 3: the resolver is given a subfolder, a trailing slash and a symlink, and all resolve to the open root).
  - `without a repository it opens here without asking`.
  - each of `current`, `new` and `ask` does what the setting says.
  - `the dialog texts and buttons`.
  - `Don't ask again saves the choice`.
  - `Cancel does nothing`.
- [ ] **Step 2: Rust tests:**
  - `focus_open_matches_canonical_roots` (symlink and trailing slash, with `TempRepo`);
  - `a_new_window_gets_its_own_initial_target` (two labels, two targets, argv only for `main`).
- [ ] **Step 3: Run them.** Expected: FAIL.
- [ ] **Step 4: Implement.** Route Open (after the chooser), Enter on a typed path, Recent Projects, Clone and Create Git Repository through `openProject`. `openRepository` no longer calls `open_in_new_window`; keep the Rust command only if the integration tests still use it, else remove it and its test.
- [ ] **Step 5: `RunEvent::Opened { urls }`** (a path handed to the running app, for example `open -a Ramify <path>`) goes through `openProject` in the last focused main window. Record in the ledger whether a plain `open -n` still starts a second process (expected yes; a known gap per §8).
- [ ] **Step 6: Run the verification block.** Expected: green.
- [ ] **Step 7: Commit** `feat: This Window or New Window, and focusing an open project`.

### Task 9: Closing, quitting and reopening

**Files:**
- Create: `src-tauri/src/open_projects.rs`
- Modify: `src-tauri/src/commands.rs` (`close_main` → `close_project_window`, `quit`, `leave`), `src-tauri/src/lib.rs` (`CloseRequested`, `Reopen`, startup), `src/App.svelte` (commit draft on request, reopen at start), `src/commit/*` (draft to changelist comment)

**Interfaces:**
- Produces (`open_projects.rs`):
  - `pub struct OpenProjects { pub projects: Vec<String>, pub last_focused: Option<String> }`
  - `pub fn load(dir) -> io::Result<OpenProjects>`, with a damaged file set aside like `recent.rs`.
  - `pub fn save(dir, &OpenProjects) -> io::Result<()>`, atomic.
  - `pub fn snapshot(sessions_in_open_order: &[(String, String)], focused: Option<&str>) -> OpenProjects`.
- Produces (commands.rs):
  - `pub fn close_project_window(app: AppHandle, label: &str)`, which follows §5.8 steps 1–7.
  - A non-last window is destroyed. The last one keeps today's hide behaviour.
  - The frontend event `commit-draft-save`, with an answer through `commit_draft_saved`, saves the window's commit message draft into the active changelist's comment (the same store the `Commit Changes` dialog uses) before the window goes.
  - `quit` runs the leave flow for every main window in turn. A refusal anywhere stops the quit.
- Startup: with no argv path and `reopenProjects`, the first `main` opens the first project. Every other project gets `project_new_window`, and the last focused one is focused at the end. Missing paths are skipped, removed, and reported in one notification: `Some projects could not be reopened because their folders no longer exist: <names>`.

- [ ] **Step 1: Write the failing tests:**
  - `open_projects.rs`: `the_snapshot_keeps_the_opening_order_and_the_last_focused`, `a_damaged_file_is_set_aside`, `saving_is_atomic`.
  - commands: `closing_a_window_waits_for_its_writes_and_shows_its_prompts` (Review Focus 2: with a fake askpass prompt during the wait, the prompt targets the closing window's label and the session is dropped only after `finish_writes_of` returns).
  - commands: `quit_stops_when_any_window_refuses` (Review Focus 4: a merge window of `main-2` answers refuse; the quit returns without `exit` and `main` is untouched).
  - `App.test.ts`: `the commit draft is saved to the changelist comment when the window closes` and `at start the open projects reopen, the missing ones are reported`.
- [ ] **Step 2: Run them.** Expected: FAIL.
- [ ] **Step 3: Implement.** Write the snapshot on every open, This Window replacement, window close and focus change between main windows, never on quit. A start with an argv path does not read the list.
- [ ] **Step 4: Run the verification block.** Expected: green.
- [ ] **Step 5: Commit** `feat: closing a project window, quitting all, and reopening projects on startup`.

### Task 10: The menu bar follows the focused window

**Files:**
- Modify: `src-tauri/src/commands.rs` (`git_menu_enable`, `git_menu_current_file`, `action_window`, `open_recent`, `open_project_chooser`, `open_settings`, `refresh_recent_menu`, `focus_changed`), `src/App.svelte` (`syncMenu` on focus gained)

**Interfaces:**
- Produces:
  - `AppState.last_focused_main: Mutex<String>`, updated when a main window or one of its auxiliary windows gains focus.
  - `pub fn menu_owner(state) -> String`: that value.
- `git_menu_enable` and `git_menu_current_file` from a window whose owner is not `menu_owner` do nothing.
- Menu events go to `menu_owner`'s main window, or its viewer per `action_window`.
- `refresh_recent_menu` leaves out `menu_owner`'s repository and runs on every focus change between owners.
- A main window that gains focus calls `syncMenu`.

- [ ] **Step 1: Write the failing tests:**
  - `a_menu_action_goes_to_the_focused_owner` (Review Focus 5: focus on `viewer-2` with a FILE_MENU id → `viewer-2`; another id → `main-2`; never `main`).
  - `menu_state_from_an_unfocused_window_is_ignored`.
  - `recent_projects_leave_out_the_focused_windows_repository` (pure part on `menu_entries`, as in `recent.rs`).
  - `App.test.ts` `gaining focus syncs the Git menu`.
- [ ] **Step 2: Run them.** Expected: FAIL.
- [ ] **Step 3: Implement**, and rewrite the `action_window` pinning tests.
- [ ] **Step 4: Run the verification block.** Expected: green.
- [ ] **Step 5: Commit** `feat: the menu bar follows the focused project window`.

### Task 11: Background loops per open repository

**Files:**
- Modify: `src-tauri/src/vcs_commands.rs` (`check_remotes`, `app_visible`), `branch_info.rs` (`seen` per repository), `commands.rs` (`SHELF_CLEANER`, `SHELF_SEEN`)

**Interfaces:**
- Produces:
  - `BranchSync.seen: Mutex<HashMap<PathBuf, ToFetch>>`.
  - `check_remotes` iterates every session whose main window is visible and not minimised, and emits `branch-sync-changed` to that owner.
  - The shelf cleaner keeps a set of data folders, added on open and removed on close.

- [ ] **Step 1: Write the failing tests:**
  - `branch_info.rs` `what_was_seen_is_kept_per_repository`.
  - `commands.rs` `the_shelf_cleaner_keeps_every_open_repository` (the set after opening A, opening B and closing A).
- [ ] **Step 2: Run them.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run the verification block.** Expected: green.
- [ ] **Step 5: Commit** `feat: remote checks and shelf cleaning for every open repository`.

### Task 12: The project widget

**Files:**
- Create: `src/project/projectIcon.ts` (port), `src/project/projectIcon.test.ts`, `src/project/ProjectWidget.svelte`, `src/project/ProjectWidget.test.ts`
- Modify: `src-tauri/src/commands.rs` (`projects_list` command), `src/App.svelte` (toolbar), `NOTICE` (ported file)

**Interfaces:**
- Consumes: `recent::menu_entries`, sessions, `openProject`.
- Produces:
  - `#[tauri::command] projects_list(window) -> ProjectsList { open: Vec<ProjectRow>, recent: Vec<ProjectRow> }`, where `ProjectRow { path, name, home_path }`.
    - `open` lists the other windows' repositories.
    - `recent` lists the recent list minus every open one, at most 100.
  - `projectInitials(name: string): string` and `projectColorIndex(path: string): number`, ported from `RecentProjectIconHelper.kt` at `idea/241.14494.240`, with the colour palette from the same file.

- [ ] **Step 1: Fetch `RecentProjectIconHelper.kt`** at the tag (`gh api` raw), read how it makes initials and picks colours, and write `projectIcon.test.ts` with the cases its code determines. Include at least: a one-word name, a hyphenated name, a camelCase name and a name with digits. Add the expected colour index for two fixed paths, computed by the same algorithm by hand in the test comment.
- [ ] **Step 2: Run it.** Expected: FAIL.
- [ ] **Step 3: Port it**, and add the file to `NOTICE` like the other ports.
- [ ] **Step 4: Check at the tag** whether IntelliJ IDEA 2024.1 adds `Get from Version Control…` to `ProjectWidget.Actions`. Search the tag's tree for `add-to-group group-id="ProjectWidget.Actions"`. Ledger the answer: present → show it and map it to Clone; absent → `Open…` only.
- [ ] **Step 5: Write `ProjectWidget.test.ts`:**
  - the button shows icon and name, tooltip `~/…`, and is hidden without a repository;
  - the popup shows `Open…`, then `Open Projects` and its rows, then `Recent Projects` and its rows, each with name and `~/…` path;
  - choosing an open project calls `project_focus_open`;
  - choosing a recent one calls `openProject`.
- [ ] **Step 6: Run it.** Expected: FAIL.
- [ ] **Step 7: Implement.** Put the widget first in the toolbar; the path field and Open stay.
- [ ] **Step 8: Run the verification block.** Expected: green.
- [ ] **Step 9: Commit** `feat: the project widget with open and recent projects`.

### Task 13: Documentation and the release build

**Files:**
- Modify: `docs/reports/opening-projects.md`, `docs/parity-matrix.md` (section 2 rows for project windows and the widget), `README.md`, `docs/reports/phase-4-validation.md` (a `Multi-window` part with sections 1–7 as the other parts)

- [ ] **Step 1: Update `opening-projects.md`.** Remove the differences this plan closes (This Window / New Window, reopening). Add §8's remaining gaps and the ledger's rulings.
- [ ] **Step 2: Write the validation part.** Section 5 is the manual checklist:
  - two windows with two throwaway repositories;
  - ⌘`;
  - the Dock;
  - This Window / New Window / Don't ask again;
  - closing with a running push;
  - quitting with a half-done merge in the unfocused window;
  - reopening after quit;
  - the widget popup;
  - the menu following focus.
- [ ] **Step 3: Run the verification block** and build: `nice -n 15 pnpm tauri build --bundles app`. Expected: green and `Finished 1 bundle`.
- [ ] **Step 4: Commit** `docs: several project windows`.
