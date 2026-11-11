# Several project windows in one Ramify process: design

Date: 2026-11-10. Reference: IntelliJ IDEA CE 2024.1, tag `idea/241.14494.240`.

## 1. Intent

The user works with several repositories at once and wants Ramify to behave like IntelliJ:

- Opening a project while one is open asks whether to open it in **This Window** or a **New Window**.
- Quitting Ramify and starting it again reopens every project that was open.
- One Ramify in the Dock, ⌘Q quits all of it, ⌘` cycles through the project windows.

The user chose this model, one process with several windows (option B), over one process per window (option A, today's `open -n`), knowing it is the larger change. The reason is the Dock, ⌘Q and ⌘`: with option A each window is its own app.

## 2. What IntelliJ does (and Ramify copies)

From `ProjectManagerImpl.kt`, `ProjectUtil.kt`, `GeneralSettings.kt`, `RecentProjectsManagerBase.kt` and the bundles at the tag:

1. **Opening a project that is already open** focuses its window and opens nothing (`findAndFocusExistingProjectForPath`, compared with `Files.isSameFile`).
2. **Opening a project while another is open** asks, when `Open project in` is `Ask`:
   - Title `Open Project`.
   - Message `Where would you like to open the project '<name>'?`, where `<name>` is the folder's name (`prompt.open.project.with.name.in.new.frame`).
   - Buttons `This Window` (default), `New Window`, `Cancel`.
   - Checkbox `Don't ask again` (`ProjectNewWindowDoNotAskOption`): it stores the chosen button as the setting.
   - The window it replaces is the last focused one.
   - With no project open, it opens without asking.
3. **Settings** (`ProjectConceptBundle`):
   - `Open project in`: `New window` / `Current window` / `Ask`. The default is `Ask`, which is also the user's IntelliJ setting (`confirmOpenNewProject2 = -1`).
   - `Reopen projects on startup`, on by default.
4. **Reopening.**
   - A project counts as open from the moment it opens until its window is closed by the user.
   - Quitting does not close projects, so quitting keeps them open for the next start. With `Reopen projects on startup`, the next start opens all of them.
5. **This Window** closes the current project with its usual checks (`closeAndDisposeKeepingFrame`, `checkCanClose`) and opens the new one in the same frame.

Not copied:
- **Attach**: IntelliJ IDEA does not offer it (`ProjectAttachProcessor.canAttachToProject()` is false).
- **Window tabs**: the macOS window tabs (`Move Tab to New Window`).
- **The Welcome screen.**
- **Frame bounds per project.** These are left out of this design; see section 8.

## 3. Goals, non-goals, anti-goals

**Goals:**
- Several project windows in one process, each with its own repository and everything that hangs from it: its own diff, merge, viewer and selection history windows, tasks, console, log, watcher, askpass and editor prompts.
- The open flow of section 2 (points 1, 2 and 5), the two settings, and reopening on startup.
- Clone, Create Git Repository and Recent Projects go through the same open flow.
- IntelliJ's project widget in the toolbar, with its popup of open and recent projects (5.10).

**Non-goals:**
- The non-copied items listed in section 2.
- A single window that shows two repositories.

**Anti-goals.** Each of these is a failure even if the rest works:
- **Cross-window leaks.** An action, prompt, task, console line, notification, menu state or `repo-changed` of one window reaches or affects another window's repository.
- **Lost work on close or quit.** Closing or quitting loses typed text that the single-window app keeps today. This covers the diff window's unsaved text, a half-done merge window and the commit message draft. It breaks the rule "no perder trabajo".
- **One repository in two windows.** The same repository open in two windows; point 1 of section 2 prevents it.
- **Broken one-window use.** Any feature that works today with one window stops working with one window.
- **Weaker rules.** The rules of `CLAUDE.md` and the project's standing rules get weaker:
  - Every command that changes files or the index still calls `flush_diff_window`, now for its own window, and takes `RepoLocks`.
  - Long flows still check that the repository did not change.

## 4. Constraints

- **Stack:** Tauri 2, Rust and Svelte 5, as today. The native menu bar is app-wide on macOS, so there is only one.
- **Load rules:** one building or testing agent at a time, `nice -n 15`, `CARGO_BUILD_JOBS=3`, `--test-threads=3`, `vitest --maxWorkers=2`, and the integration tests in every verification.
- **Tests:** throwaway repositories only, never the user's real ones.
- **Language:** everything in the repository is in English.
- **Source-pinning tests:** some tests read source text and pin exact lines or counts. Among them are the `flush_diff_window(&app, &state, …)` line, the first line of `repo_open`, the "137 `pub async fn`" count in `vcs_commands.rs`, `current_repo(&state)? != repo` in the shelve commands, `owns_watcher(MAIN_WINDOW)` and `action_window`. They are rewritten to pin the new shapes. They are not deleted: what they protect, such as a flush before every write, must still be checked.

## 5. Decided design

### 5.1 Window identity

**Labels:**

| Window | First project window | Project window number n (n ≥ 2) |
|---|---|---|
| Main | `main` | `main-<n>` |
| Diff | `diff` | `diff-<n>` |
| Merge | `merge` | `merge-<n>` |
| Viewer | `viewer` | `viewer-<n>` |
| Selection history | `selection-history` | `selection-history-<n>` |

- A window's **owner** is the main window with the same suffix. One Rust function maps any label to its owner and is unit-tested.
- The first window keeps today's labels, so one-window use, the smoke runner and many tests stay as they are.
- Numbers are not reused while the process lives.

**Capabilities** list the labels with globs (`main*`, `diff*`, `merge*`, `viewer*`, `selection-history*`), keeping each window's current permissions.

**Frontend:**
- `ownWindow()` returns the real label (`getCurrentWebviewWindow().label`), not the `?window=` parameter.
- `?window=` still selects which part to load. A new main window loads `index.html` with no parameter, as today.

### 5.2 Backend state: app-wide and per window

`AppState` keeps what is truly app-wide:
- `operations` (`RepoLocks`, keyed by repository path), `console` (the storage), `tasks` (the runner), the askpass and editor servers, `activation`, `hosting`, `signatures` (re-keyed, see below) and the settings.

Everything that today means "the" repository or "the" auxiliary window moves into a per-window session:
- `sessions: Mutex<HashMap<String /* main label */, Arc<WindowSession>>>`.
- `WindowSession` holds: `repo`, `log`, `watcher`, the branch-sync `seen`, `diff_request`, `diff_flush`, `merge_session`, `merge_leave` and `merge_ready`.
- It also holds what statics hold today for the single auxiliary windows: the viewer's `PENDING` and the selection history's `PENDING`/`RUNNING`.

**How commands find their window.** Every Tauri command that uses the repository or an auxiliary window takes the calling `WebviewWindow`, which Tauri injects. The frontend's `invoke` calls do not change.
- `current_repo(&state)` becomes `current_repo(&state, &window)`, which resolves the owner and reads its session.
- The same applies to `repo_data_dir`, `staging_context`, the `vcs_commands` helpers (`start`, `read`, `read_locked`, `read_as`, `start_on_shelf`, `still_open`, `is_open`), hosting's `opened` and `connection_of`, the log's `with_log`/`with_store`, `flush_diff_window` and `leave_merge_window`.
- About 190 call sites change mechanically. The existing `expected_repo` and `repo` guards stay.

**Statics that become per repository or per window:**
- `SHELF_CLEANER` keeps every open repository's shelf on its daily cleaning, not only the last one opened.
- `SHELF_SEEN` is evaluated for the repository of the window that changed.
- `signatures.pending` is keyed by window and requester.

Global file locks may stay global: `CHANGELISTS_LOCK`, the shelf `STORE`, `PENDING_LOCK`, accounts, protection and the log's recent filters. They only serialise.

### 5.3 Events

- Everything that belongs to one window's repository goes **to that window and its auxiliary windows only**:
  - `repo-changed`, with the watcher, `repo_changed` after tasks and the shelf cleaner;
  - task events, console lines, askpass and editor prompts;
  - `branch-sync-changed`, `merge-done`, `leave-refused`, `git-action`, `open-*`;
  - the diff and merge requests, the viewer and selection history updates.
- `settings-changed` stays a broadcast. `app-activated`/`app-deactivated` stay app-level, but go to every main window.
- In the frontend, every listener of a window-scoped event listens with its own window as target.
  - Today most use `listen` with no target, which in Tauri 2 also receives events sent to other windows.
  - One helper in `src/lib/api.ts` does this. It is the default for every listener, and the explicit app-wide exceptions are listed in the code.
- The frontend's own `emit` calls are scoped to the owner's windows the same way: `notifyRepoChanged` and the diff and selection events (`selection-changed`, `selection-state`, `selection-lost`, `diff-ready`, `diff-drawn`, `diff-comparison`).
- Task 1 of the plan must prove, with a small test or spike, how a targeted listener behaves with a broadcast `emit` in Tauri 2's real implementation, before building on it.

### 5.4 Tasks, console, askpass, editor

**Tasks:**
- A task carries its owner window from the command that started it.
- `TaskEvent` gains the owner, and events go to the owner's windows.
- `task_cancel` only cancels a task of the calling window.
- `finish_writes` on quit still waits for every window's writes.

**Console:** each line carries its owner. `console_lines` and `console-line` are per window, so each window's console shows only its repository's commands, like IntelliJ's per-project console. The 2,000-line limit applies per window.

**Askpass:**
- It gets a per-task token, the way the editor already has one.
- The server maps the token to the owner window. The prompt is revealed and emitted there, and the answer is accepted only from that window.
- A git process with no task, such as a blocking read, uses the token of the window whose command started it.

**Editor:** `on_request` receives the owner, through the per-task registration that already exists, and routes the prompt to it.

### 5.5 Auxiliary windows

- Each main window has its own diff, merge, viewer and selection history windows, labelled as in 5.1 and created on demand as today.
- `repo_open` and closing a window destroy and close only the auxiliary windows of that window.
- `close_main`'s current omission of `selection-history` is fixed.

### 5.6 Opening

One backend entry point, `project_open(target, how)`, used by all of these:
- the Open button and File > Open… (after the chooser);
- a typed path;
- Recent Projects;
- Clone and Create Git Repository (`openRepository` in `src/remotes/model.ts`, which today starts `open -n`);
- startup and reopening.

The steps, in order:

1. Resolve the repository root, as `repo_open` does.
2. If a window already has that root, compared as canonical paths, focus that window and stop.
3. If the calling window has no repository, open in it.
4. Otherwise apply `Open project in`:
   - `Current window` opens here.
   - `New window` opens in a new main window.
   - `Ask` shows the dialog of section 2 point 2, rendered by the calling window.
     - `Don't ask again` writes the chosen button to the setting.
     - `Cancel` does nothing.
5. **This Window** runs today's `repo_open` path in the calling window. That path already flushes the diff window and leaves the merge window first, and refuses if they refuse.
6. **New Window** creates the next `main-<n>`. The new window asks the backend for its initial target on start, the same way `initial_target` works today but per window. Then it opens it.

Command-line paths and the `open -n` fallback: `ramify <path>` started by the user while Ramify runs goes through the same steps through macOS's open-file event, if Tauri exposes it, else via the single-instance check. **Builder decides** after checking Tauri 2's options. Whatever is chosen must not start a second process that opens the same repository.

### 5.7 Settings

`Settings` gains two values, shown in the Settings view in a `System` group:
- `openProjectIn`: `"ask" | "new" | "current"`, default `"ask"`. Label `Open project in` with `New window`, `Current window`, `Ask`.
- `reopenProjects`: `boolean`, default `true`. Label `Reopen projects on startup`.

### 5.8 Closing, quitting, reopening

**Open projects file.** `open-projects.json` lives in the data folder. It is a **snapshot of this process's open windows**:
- It holds the repository of every main window that has one, in the order the windows were opened, and which of them was focused last.
- It is rewritten atomically whenever that set or the last focused window changes: an open, This Window replacing a repository, a window close, a focus change between main windows.
- Quitting does not write it, so it still holds what was open at quit. It is right even after a crash, like IntelliJ's `opened` flag.
- *(Builder check, my call.)* A start with a path argument opens that path, so the next snapshot holds only it: the previous list is not kept for a later start. With `reopenProjects` off, the snapshot is still written, only not used at start.

**Closing a main window** (red button or ⌘W):
1. Run the leave flow for that window only: flush its diff window and leave its merge window. A refusal keeps everything open.
2. *(Builder check, my call.)* Save that window's commit message draft to the active changelist's comment, the way the `Commit Changes` dialog already keeps its draft, so it comes back when that repository is opened again. Quit does the same for every window, which today loses the commit tool window's draft.
3. *(Builder check, my call.)* Wait for that window's tasks, as quit does for all of them:
   - Its reads are cancelled.
   - Its writes are waited for, up to the same 60 seconds, while that window starts no new write.
   - Other windows are not affected. This needs a per-owner version of `Runner::finish_writes`; today's global `closing` flag would stop every window.
   - A prompt (askpass, editor) that comes during the wait is still shown, because the window is destroyed only after the wait.
4. Destroy its auxiliary windows.
5. Drop its session, which stops the watcher.
6. Write the open list without it.
7. Destroy the window.

Closing the **last** main window keeps today's behaviour: everything is hidden, the repository stays open and in the open list, and clicking the Dock shows it again.

**Quit** (⌘Q, the Dock's Quit, logout):
- The leave flow runs for every window in turn, as `quit` does today for one: flush, leave the merge window, wait for writes.
- A refusal in any window stops the quit.
- The open list is not changed.
- The Dock's Quit and logout go through `terminate:`, which cannot be stopped (`replace_quit`'s comment). They keep today's behaviour, and the open list is already correct because it is written continuously.

**Starting** with no path argument and `reopenProjects` on:
- The repositories in the open list are opened. The first one goes in the initial window, the others in new main windows, and the last focused one ends focused.
- A path that no longer exists is skipped and removed from the open list, with a notification in the initial window.
- With a path argument, only that path opens, as today. The list is kept for the next plain start, as IntelliJ does with a project given on the command line.

### 5.9 The menu bar

- The Git menu's enabled state, the `Current File` submenu, the GitHub group and Recent Projects reflect the **focused main window**, or the owner of a focused auxiliary window.
- When a main window gains focus, it re-sends its menu state. `git_menu_enable` calls from a window that is not focused are ignored.
- Menu actions (`git-action`, Open…, Recent Projects, Settings) go to the focused main window, or to the viewer, as `action_window` does today for one viewer.
- With no Ramify window focused they go to the last focused main window.
- Recent Projects leaves out the focused window's repository and marks nothing else. Choosing a project goes through 5.6.
- macOS lists the windows in the `Window` menu by itself.

### 5.10 The project widget

Added at the user's request after the first review (2026-11-10). This is IntelliJ's `ProjectToolbarWidgetAction` (`openapi/wm/impl/headertoolbar/ProjectToolbarWidgetAction.kt`).

**Placement:** it goes first in the main toolbar, before the branch widget. The path field and the Open button stay after them, as today.

**The button:**
- It shows the project's icon and its name, the folder's name.
- Its tooltip is the path from `~` (`getLocationRelativeToUserHome`).
- Without a repository it is not shown.

**The project icon:** IntelliJ's coloured square with the project's initials (`RecentProjectIconHelper`). The initials and the colour are ported from that file at the tag, so the same project gets the same letters and colour as in IntelliJ.

**The popup**, a list popup like the branches popup:
- First the widget's actions. In IntelliJ IDEA these are `New Project…` and `Open…` (the group `ProjectWidget.Actions`). Ramify shows `Open…` only, since it creates no projects. The builder checks at the tag whether IntelliJ IDEA 2024.1 also adds `Get from Version Control…`. If it does, Ramify shows it and maps it to Clone.
- Then a section `Open Projects` (`project.widget.open.projects`): the repositories open in Ramify's windows. Choosing one focuses its window.
- Then a section `Recent Projects` (`project.widget.recent.projects`): the recent list without the open ones, at most 100 rows (`MAX_RECENT_COUNT`). Choosing one goes through 5.6, so it can ask This Window / New Window.
- Each project row shows the icon (20 px), the name, and below it the path from `~` in the small grey font.

### 5.11 Background loops

- The remote check (`check_remotes`, `BranchSync`) checks every open repository whose window is visible, not only "the" current one. It sends `branch-sync-changed` to that repository's window.
- `app_visible` becomes "this window is visible and not minimised".
- The protection loader is already per repository.

## 6. Proof expected

- **Rust:**
  - unit tests for label-to-owner mapping, session lookup, the open list (snapshot order, close removes, quit keeps, missing paths skipped, last focused), focus-instead-of-reopen by canonical path, and askpass and editor routing by token;
  - the per-window wait on close: a write of the closing window is waited for, a read is cancelled, and another window's tasks run on;
  - tests that two sessions with two throwaway repositories keep their repository, tasks, console lines and `repo-changed` apart.
- **Frontend (jsdom):**
  - the open dialog (texts, buttons, `Don't ask again` writing the setting, `Cancel`);
  - each `Open project in` value; no dialog without a repository;
  - the targeted listeners ignoring another window's events;
  - the two new settings;
  - the commit draft being saved to the changelist comment on close and quit;
  - the project widget: its name, tooltip and icon; the popup's sections; focusing an open project; opening a recent one through the open flow; the initials and colour against cases taken from IntelliJ's own code or tests at the tag.
- **Whole suite:** green, with the source-pinning tests rewritten to the new shapes.
- **Report:** what could not be seen without a real window goes in the validation report's manual checklist. That covers ⌘`, the Dock, real window creation and focus, the menu following focus, and reopening after a real quit.

## 7. Left to the builder

- **How the work splits into plan tasks**, as long as every task leaves one-window use working and green.
- **Internal names and module layout** (for example a `windows.rs` for labels, sessions and the open list).
- **The Tauri mechanism** for a path handed to a running Ramify (5.6).
- **The Settings view layout** of the two new values.
- **Wording that IntelliJ has no text for**, such as the notification for skipped missing paths. It must be in English and short.

## 8. Out of scope, recorded as differences

- **Window frames:** no frame bounds per project. New windows open at the default size, cascaded by macOS.
- **No Welcome screen**, **no Attach** and **no window tabs**.
- **Importing IntelliJ's recent or open projects:** still not done.
- **One process at a time:** a second Ramify process started by hand with `open -n` keeps working but does not know about the first. The open list and recent list are shared files, so the last writer wins for the open list. This is recorded as a known gap.
