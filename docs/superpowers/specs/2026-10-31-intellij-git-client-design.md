# Desktop Git client in the style of IntelliJ: design

Date: 2026-10-31. Status: approved. Phase 0 finished on 2026-10-31.

Legend: **[user]** the user said or chose it. **[proposed and accepted]** Claude proposed it and the user said "Yes, go ahead". **[Claude's decision]** added while writing this document and not yet reviewed.

## 1. Intent

The user programs with Devin and keeps IntelliJ IDEA CE open only to use Git. They use all of its features and no other client seems as good to them. The problem is what a Java IDE open all day consumes. **[user]**

They want a separate app that is IntelliJ's Git and nothing else, implemented "exactly the same". **[user]**

Success is being able to close IntelliJ for good without missing anything in Git.

## 2. Goals

1. Full parity with the Git features of IntelliJ IDEA CE, the version the user uses (section 5). **[user]** Phases 1 to 3 copy 2023.2.3; from phase 4 on, the reference is 2024.1 (section 7).
2. Look, shortcuts and behaviour identical to that version, so no habit has to change. **[proposed and accepted]**
3. Consumption far below IntelliJ's, comfortable to leave open all day. **[user]**

Measurable goals **[Claude's decision]**. They are a guide, not a yes-or-no limit: what decides is that the app is clearly lighter than IntelliJ with the same repository.

Measurement conditions: Apple Silicon Mac, one open window, a public repository of about 100,000 commits (for example `git/git` or similar), log loaded and a diff open. The physical memory (`footprint`) of all the app's processes is added up, including those of the WebView.

- About 300 MB of memory at rest.
- CPU close to 0 % at rest, with no file changes.
- Less than 1 second of cold start until it is usable.
- Log scrolling without visible jumps.

## 3. What this project is not

**Non-goals:**

- Features that need the IDE to understand the code: inspections before the commit, reformatting, go to definition from a diff. **[proposed and accepted]** Syntax highlighting is included.
- Other version control systems (Mercurial, Subversion, Perforce). **[Claude's decision]**
- General code editor, integrated terminal or running the project. **[Claude's decision]**
- IntelliJ's Local History, because it is not part of Git. Its safety-net role is covered by the prior copy of section 4. **[Claude's decision]**
- Features added by third-party plugins, such as GitToolBox. **[Claude's decision]**
- Several repositories in one window. **[user]**
- Reviewing pull requests inside the app: inline comments, approving or merging. **[user]**
- Windows and Linux at this stage. **[proposed and accepted]**
- Public distribution: it is for personal use. **[proposed and accepted]**

**It would be a failure even if it works:**

- That it feels like "another Git client" and forces relearning workflows. **[Claude's decision]**
- That it weighs so much that it ends up being one more process to watch. **[Claude's decision]**
- That it loses work or leaves a repository in a bad state. **[Claude's decision]**

## 4. How it is built

**Decided [proposed and accepted]:**

- **Tauri**: Rust core and web interface on the system WebView. No JVM or bundled Chromium.
- **Real `git` binary** for all operations, just like IntelliJ. Git is not reimplemented.
- **macOS first.** The user's machine is arm64.
- **Built in phases** (section 6).

**Decided by the user:**

- **One repository per window.** A project is exactly one repository, and several windows can be open at once.
- **Devin edits files in the local copy and the user performs all the Git operations.** The app does not need to coordinate with Devin's commits or rebases, but it does with files that change while working.

**Claude's decision:**

- **Live refresh:** watch the working directory and `.git` with FSEvents and update without intervention. If a file changes while lines are selected for commit, the selections that are still valid are kept and the ones that are not are reported.
- **Own data outside the repository:** changelists, log index and preferences live in `~/Library/Application Support/<app>/`, per repository. That way the repository is not dirtied and `.idea/` is not interfered with.
- **Read-only, repeatable import:** IntelliJ and the app will coexist for months, so importing changelists from `workspace.xml` and shelves from `.idea/shelf/` can be repeated by hand. `.idea/` is never written to.
- **Copy before destructive operations:** before rollback, reset or discarding changes, a recoverable copy of the affected files is saved and kept for 30 days. IntelliJ does not do exactly this, but it replaces the recovery that Local History gives, and it avoids losing Devin's work that is not yet committed.
- **File tree and read-only viewer:** annotate, file and selection history, and compare with branch need an entry point. The project tree and a viewer with syntax highlighting are included, without editing.
- **Opening from outside:** a terminal command opens a repository or a file in the app, to get there from Devin without navigating.
- **Credentials:** Git's stay in the system (credential helper and SSH). The GitHub and GitLab tokens go in the macOS Keychain.

**Up to whoever builds it:** UI framework, diff editor component, log index engine, app name and internal structure. Constraint: meet the consumption goals of section 2.

**Decided after phase 0:**

- **Startup:** the app opens on the Commit view, and the log is loaded only when its tab is opened. Measured reason: loading the log costs 1.3 s in a repository of 86,000 commits. **[Claude's decision]**
- **Diff in its own window**, because it is the user's current setting in IntelliJ. **[Claude's decision]**
- **The recoverable copy goes into phase 1**, with the rollback. **[user, when approving the matrix]**

**Decided in phase 2** (plan `2026-11-01-phase-2-log-branches-and-remotes.md`, Decisions section; the user approved them with the plan) **[Claude's decision]**:

1. **Only stash until phase 4.** `Clean working tree using` is Stash or Shelve, and in IntelliJ the default value is Shelve. The shelf arrives in phase 4, so everything is saved with `git stash`.
2. **The "auto, always or never" setting does not exist in 2023.2.3.** The matrix was wrong: the row becomes `Clean working tree using`, with Stash fixed in this phase.
3. **System credentials.** `Use credential helper` comes enabled, unlike in IntelliJ. Without Password Safe, the login dialog has no `Remember`: if there is a helper, git saves it.
4. **Restoring after an error or a cancellation.** IntelliJ leaves the changes saved in those cases. Here they are restored if the repository went back to its clean state, because there is no stash interface until phase 4.
5. **`Restore Changes` button.** If saved changes remain, the notification stays with that button, which does `git stash pop` of that entry. IntelliJ links to the shelf, which does not exist here.
6. **Conflicts.** The app detects the state and leaves the repository as git leaves it, with `Abort`. Resolving them, `Continue` and `Skip` belong to phase 3.
7. **GitHub protected-branch rules, to phase 4.** They need the Keychain token. Here there is a list of patterns, with `master` and `main`, saved per repository.
8. **Filtering without stages.** IntelliJ asks for 5, 2,000 and then all the commits. Here everything is requested and shown when it finishes.
9. **Full reload of the log.** IntelliJ joins only the new commits. Here everything is reread when the references change, and only if the log is visible.
10. **Rows with no effect in the app.** `Vcs.Log.CheckFirstCommitsIndexData`, `CheckSelectedCommitsIndexData`, `InvalidateCaches` and `ResumeIndexing` belong to IntelliJ's index. `ShowRootsColumnAction` and `Git.Log.Branches.GroupBy.Repository` have no effect with one repository per window.
11. **`Vcs.Log.ShowAllAffected` and `Vcs.Log.ShowOtherBranches` move to phase 4.** They only act on the file history. `Select in Tree…` and the comparisons of the log menu (`Compare with…`, `Show Diff with Working Tree`) also go to phase 4. `Pull into…` belongs to phase 3.
12. **Row height 24.** IntelliJ computes it from the font and the value is not in the theme. It remains to be compared with IntelliJ screenshots (see `docs/reports/phase-2-validation.md`).

**Decided in phase 3** (plan `2026-11-02-phase-3-conflicts-and-rewriting.md`, Decisions section; the user approved them with the plan) **[Claude's decision]**:

1. **Only stash until phase 4**, same as in phase 2. Rebase, Smart Merge and editing from the log save with `git stash`, with IntelliJ's messages (`Uncommitted changes before rebase`, `Uncommitted changes before Merge at <fecha>`). IntelliJ's default policy is Shelve.
2. **`GIT_SEQUENCE_EDITOR` in addition to `GIT_EDITOR`.** IntelliJ only sets `GIT_EDITOR`, and a user's `sequence.editor` wins over it. Here both are set.
3. **A single merge window.** IntelliJ opens one per file from the Commit view and a modal one from the Conflicts dialog. Here there is only one, and asking for another file while it is open brings it to the front without changing file. Two files cannot be resolved at the same time.
4. **The diff from base to each side uses Myers (`similar`)**, the one `hunks.rs` already uses. IntelliJ uses its "fair" diff with line optimizations. The rest is ported as is: the merging of ranges, the type of each block and the automatic resolution. In ambiguous cases a block may start or end one line earlier or later than in IntelliJ.
5. **Recoverable copy before overwriting a file in conflict.** Accept Yours, Accept Theirs and Apply in the merge window copy the working file first. IntelliJ relies on Local History, and here Devin may have written in it.
6. **`Show Details` and `Filter by conflicted file`, to phase 4.** The links that list the commits involved in a conflict need the file history.
7. **No `Conflicts` tool window and no "Resolving merge conflicts is in progress" editor bar.** The first is off by default in 2023.2.3 (registry key `git.merge.conflicts.toolwindow`), and the second belongs to the IDE's editor, which does not exist here.
8. **Cherry-Pick Selected Changes and Revert Selected Changes with `git apply`.** IntelliJ builds a patch of its own and applies it with its `PatchApplier`. Here `git diff` of the commit is used for those paths, with `-R` when reverting, and `git apply`; if it does not go in cleanly, it is repeated with `--3way`, which leaves the conflict with its stages for the Conflicts dialog.
9. **The matrix, corrected.** The `git4idea.actions.branch.*` rows without text were left as "Out of scope" by the classifier, but they are the branches popup actions: Merge, Rebase, Checkout with Rebase and Pull into… (`WithMerge` and `WithRebase`) move to phase 3; Checkout, New Branch, Delete, Rename, Push and Update Selected, to phase 2, which already covers them; Compare with… and Show Diff with Working Tree, to phase 4.
10. **The `using log` mode of the interactive rebase is the only one that opens the dialog before launching git.** It is the default mode in 2023.2.3. If the range has a merge, a root commit or a `fixup!` or `squash!` subject, it switches to native mode, like IntelliJ. Lines that are not pick, edit, drop, squash, reword or fixup are shown in the free text editor, also like IntelliJ.

## 5. Feature inventory

**Commit**
- Changes, Unversioned and Ignored, with grouping by directory.
- Changelists: create, rename, move changes, active list.
- Alternative mode with Git's staging area, as in IntelliJ's settings.
- Partial commit by hunks and by lines, with selection in the diff margin.
- Amend, commit and push, author, sign-off, message history and message validation.
- Rollback.

**Diff**
- Side-by-side and unified view, at line and word level.
- Whitespace options, collapsing of unchanged zones and synchronized scroll.
- Editing of the local side from the diff.
- Navigation between changes and between files.

**Log**
- Commit graph.
- Filters by branch, user, date and path. Text search with regex and case matching.
- Commit details and tree of changed files.
- Actions on commits: cherry-pick, revert, reset, checkout, create branch, create tag, undo commit, edit message, squash, fixup, drop, interactive rebase from here, push up to here, compare with local and create patch.

**Branches and remotes**
- Branch popup with favourites and incoming and outgoing indicators.
- Checkout, create, rename, delete, compare, merge, rebase and update.
- Tags, remote management, fetch, Update Project (merge or rebase).
- Push dialog with force-with-lease and protected branches.
- Clone, init and add to `.gitignore`.

**Conflicts**
- Resolution dialog.
- Three-pane merge with per-block acceptance and automatic application of what does not conflict.
- Abort and continue of merge, rebase and cherry-pick.

**Interactive rebase**
- Dialog with pick, reword, edit, squash, fixup, drop and reordering.

**Stash and Shelf**
- Stash: create, apply, pop, delete.
- Shelf: save, partial unshelve, delete.
- Create and apply patches.

**History**
- Annotate (blame) with navigation by revision.
- File and selection history.
- Compare a file with a branch or revision and compare branches.

**More**
- Worktrees and submodules.
- Commit signing (GPG and SSH).
- GitHub and GitLab pull requests and merge requests: list, detail, status, changes and fetching the branch. **[user]**
- Git console with the commands that were run.

This inventory is a starting point **[Claude's decision]**. Phase 0 turns it into the definitive parity matrix from the source code and from the app itself. The user approves that matrix, and any cut, before phase 1 starts. It includes the Git settings that appear in `Settings > Version Control > Git`, not all the version control ones.

## 6. Phases [proposed and accepted]

Only phase 0 was added afterwards **[Claude's decision]**.

| Phase | Content | Exit criterion |
|---|---|---|
| 0 | Parity matrix, app skeleton, consumption test with a large repository | Consumption goals reachable, shown with a test log and diff |
| 1 | Commit, changelists, line staging, diff | All everyday commits without opening IntelliJ |
| 2 | Log, graph, filters, branches, push, pull | Without opening IntelliJ except for conflicts |
| 3 | Three-pane merge, interactive rebase, cherry-pick | Without opening IntelliJ for any usual operation |
| 4 | Shelf, stash, annotate, history, worktrees, GitHub and GitLab | Full parity and IntelliJ can be uninstalled |

Phase 0 goes first because the biggest risk is technical: a log and a diff in a WebView must be smooth and consume little. If the goals are not reached, the technology is decided again with the user before going on.

Each phase is declared finished by the user, by using it on their real repositories **[Claude's decision]**. The import of shelves from section 4 arrives with phase 4, when the app can already use them. IntelliJ stays installed until the end of that phase.

## 7. Fidelity and reference

The reference for phases 1 to 3 is IntelliJ IDEA CE 2023.2.3 **[user]**, build `IC-232.10072.27`. Its code is the tag `idea/232.10072.27` of `JetBrains/intellij-community`. `master` must not be used: it has a different structure and behaviour.

**Changed on 2026-11-03 [user]:** from phase 4 on, the reference is IntelliJ IDEA CE 2024.1, build `IC-241.14494.240`, tag `idea/241.14494.240` (commit `c047a5caf941e5522efcf339eef7b660afa2e300`). It is the version installed in `/Applications` and the one the user uses every day: its settings folder, `IdeaIC2024.1`, changes daily, while `IdeaIC2023.2` has not changed since 2024-03-14. The differences with phases 1 to 3 are reviewed apart, in `docs/reports/phase-4-reference-change.md`.

Paths checked in that tag on 2026-10-31:

| Path | Content |
|---|---|
| `plugins/git4idea/src/git4idea/` | Git logic and plugin interface |
| `platform/vcs-log/` | Log and graph |
| `platform/vcs-impl/src/` | Changelists, shelf and annotations |
| `platform/diff-impl/src/` | Diff and merge |
| `plugins/github/`, `plugins/gitlab/` | Pull requests and merge requests |

The app copies the New UI with dark theme, according to the user's current settings, and the default macOS keymap. Phase 0 confirms this with the user's installation.

Comparison method **[Claude's decision]**:

- Record the commands that IntelliJ 2023.2.3 launches with a `git` wrapper and compare them with those the app launches for the same action.
- Automatic tests use throwaway repositories, never the real ones. Use on real repositories is done by the user when validating each phase.
- For the look, compare screenshots with IntelliJ in the same window and theme.
- Destructive operations (reset hard, force push, deleting branches) ask for the same confirmations as IntelliJ.

## 8. License

The logic and the interface can be taken as a reference. The tag `idea/232.10072.27` is under Apache 2.0, according to its `LICENSE.txt`, so porting code requires keeping the attribution. The app will not use the name or logos of JetBrains or IntelliJ. **[Claude's decision]**

## 9. Environment data

- macOS arm64, with rustup, nvm and pnpm installed. The default Rust (1.68.2) is too old for Tauri 2, and the project pins its own version of Rust and Node.
- IntelliJ IDEA CE 2024.1 (`IC-241.14494.240`) in `/Applications`, installed on 2024-04-24. The settings of 2023.1, 2023.2 and 2023.3 are still in `~/Library/Application Support/JetBrains/`, but they are not in use. A 2022.3 is also installed with Toolbox, which is not the reference.
- Git 2.38.0 (Homebrew). It is the minimum version to support unless a feature requires another.
