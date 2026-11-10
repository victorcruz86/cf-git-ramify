# Opening projects: Open… and Recent Projects

How Ramify opens a repository from its own window, compared with IntelliJ IDEA CE 2024.1 (`idea/241.14494.240`). Added on 2026-11-10, after phase 4 part 6.

## What it does

- **Open** (the toolbar button) and **File > Open…** show IntelliJ's `Open File or Project` chooser, the native macOS one, for folders only. It starts in the folder typed in the path field, or in the home folder. The chosen folder is opened like a typed one, in the same window. Typing a path and pressing Enter still opens it without the chooser.
- **File > Recent Projects**, below Open…, lists the repositories opened in Ramify, the latest first, as `RecentProjectsGroup` and `RecentProjectListActionProvider` do:
  - Every repository that opens is recorded, from any entry point: Open, a typed path, the path Ramify was started with, a clone opened in a new window.
  - The list keeps 50, the default of `ide.max.recent.projects`; past that the oldest goes.
  - A row shows the folder's name. When two repositories share a name (the window's own one included), each shows its path, from `~` when it is inside the home folder.
  - The window's own repository is not listed. With nothing to list the submenu is disabled.
  - A path that no longer exists stays in the list, as in IntelliJ, in case it is on a drive that is not connected. Choosing it shows `Reopen Project` with IntelliJ's text and `OK` / `Remove From List`.
- With a modal dialog open, File > Open… and Recent Projects do nothing, like the Git menu.

The list lives in `recent-projects.json` in Ramify's data folder (`~/Library/Application Support/dev.ramify.app`), never in a repository. It is written to a temporary file and renamed, and read again right before each write, so what another Ramify window recorded stays. The submenu is rebuilt when a repository opens and when Ramify gains the focus.

## Differences from IntelliJ

- **This Window or New Window.** IntelliJ asks where to open a project when one is already open (`Open project in: Ask`, the user's setting: `confirmOpenNewProject2 = -1`). Ramify always opens in the same window, with Open as well as with Recent Projects. Cost: to keep the current repository open, Ramify must be started again with `open -n`.
- **Import from IntelliJ.** IntelliJ's own list (`recentProjects.xml`) is not read: Ramify's list starts empty and fills as repositories are opened in Ramify.
- **Manage Projects…** The searchable window IntelliJ puts at the end of the submenu does not exist. The list is only managed by opening repositories and by `Remove From List`.
- **Groups, the Dock menu and the limit.** No project groups, no recent projects in the Dock menu, and the limit of 50 cannot be changed.
- **Projects open in other windows.** IntelliJ never drops a project that is open when trimming to the limit. Ramify does not know what other Ramify windows have open, so the 51st oldest goes even if it is open somewhere else (it comes back when that window opens it again).
- **Files.** IntelliJ's chooser also opens single files and project files (`.ipr`); Ramify's opens folders only.
- **Two windows at once.** The file has no lock: two windows recording at the very same moment can lose one of the two entries until that repository is opened again.

## What was proven with tests

- `src-tauri/src/recent.rs`: order without repeats, the limit of 50, a missing file and folder, a damaged file set aside as `recent-projects.damaged.json`, removal, the window's own repository left out, `~` paths for shared names (also against the open repository, and not for a folder that only shares the home folder's prefix).
- `src-tauri/src/commands.rs`: Open… heads the File menu with no accelerator, and a Recent Projects row carries its path and is not a Git menu action.
- `src/App.test.ts`: Open, a cancelled chooser and its starting folder, Enter, File > Open…, a recent project that opens, `Remove From List`, `OK`, and both menus doing nothing behind a modal dialog.

## What could not be checked

The native chooser, the menu bar and the submenu were not seen in a real window, and no real repository was opened: the tests simulate the chooser and the Tauri API, and the menu itself cannot be built without a running app. That is for the user to check in the app.
