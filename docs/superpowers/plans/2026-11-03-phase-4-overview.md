# Phase 4: shelf, stash, patches, history, GitHub and the rest. Overview

> **For agentic workers:** this document is not executed by itself. Each part below has its own plan (`2026-11-03-phase-4-<n>-<name>.md`), which the user approves before it starts. Every part plan includes this document's Global Constraints and Decisions.

**Goal:** full parity with the Git features the user uses in IntelliJ, so that IntelliJ can be uninstalled.

**Spec:** `docs/superpowers/specs/2026-10-31-intellij-git-client-design.md`. Scope:
- the phase 4 rows of `docs/parity-matrix.md`;
- the rows earlier phases deferred to phase 4;
- the differences that `docs/reports/phase-4-reference-change.md` found in phases 1 to 3.

**Reference:** IntelliJ IDEA CE 2024.1, build `IC-241.14494.240`, tag `idea/241.14494.240` (commit `c047a5caf941e5522efcf339eef7b660afa2e300`). The user chose it on 2026-11-03 (spec section 7).
- Sources: `.superpowers/ref-idea-241/ic/`, filled with `.superpowers/ref-idea-241/fetch_ic.py`. That script skips `.form` files; get them from `raw.githubusercontent.com` at the same commit.
- Shortcuts, colours and icons come from the jars of `/Applications/IntelliJ IDEA CE.app`, which is that same build. Read them, never write to them.
- The research notes behind this document are in `.superpowers/phase-4/` (not in the repository):
  - `research-modal-commit.md`
  - `research-stash-shelf-patches.md`
  - `research-history-compare.md`
  - `research-github-gitlab.md`

**Exit criterion:** the user uses the app on their real repositories without opening IntelliJ for anything Git-related, and declares the phase finished.

**Branch:** `phase-4`, created from `main` (`e213e8d`). All the parts go on this branch, one after another.

## What the user uses

These are the 2024.1 counts in `actionSummary.xml`, read without changing anything. They set the order of the parts and the weight of each decision.
- Diff: 14,453 and 6,124.
- Rollback: 3,831. Checkout: 3,753. Push: 3,479.
- Patches: Copy as Patch to Clipboard 3,430, Create Patch from Local Changes 2,348, Apply Patch from Clipboard 1,428, Apply Patch 354.
- Pull: 2,213. Fetch: 1,076.
- Stash: Apply in the Stash tab 800, Stash Changes 555, Unstash Changes 238.
- File history: 187. Commit message history: 158. Copy Revision Number: 134.
- Show Repository at Revision: 17.
- Shelf: about 14. Annotate: never.

Settings the user has in 2024.1, different from what phase 0 read in 2023.2:
- modal commit: `COMMIT_FROM_LOCAL_CHANGES=false`;
- staging area saved as on, but inactive because the mode is modal;
- soft wraps in the diff;
- patches saved in `~/Downloads/PATCHS`;
- one GitLab account.

The user says they work with github.com only (2026-11-03).

## Parts, in order

| Part | Plan | Content | Why in this place |
|---|---|---|---|
| 1 | `phase-4-1-modal-commit.md` | Matrix regenerated from 241. `Version Control` tool window (⌘9) with Local Changes, Log and Console. Modal `Commit Changes` dialog (⌘K), also usable with executors. `Use non-modal commit interface` setting, which also unlocks `Enable staging area`. The 241 commit checks: large files and file names. | It is how the user commits every day (decision 1). Parts 3 and 5 reuse the dialog for Shelve and Create Patch. |
| 2 | `phase-4-2-reference-alignment.md` | The S and M fixes from sections 2 to 4 of the reference report: copied texts, drop confirmation, Merge and Rebase options, `Cancel Reword`, `Rebasing Published Commit`, branches popup search, `Push All up to Here`, `git merge` with the full ref, soft wraps in the diff. | Phases 1 to 3 become 241 before the user validates them. Small and independent. |
| 3 | `phase-4-3-patches-stash-shelf.md` | Patch reader and writer. Create Patch, Copy as Patch, Apply Patch, Apply Patch from Clipboard. Stash Changes, Unstash, the Stash tab. Shelf store, import from `.idea/shelf`, Shelve, Unshelve, the Shelf tab. Patch conflicts in the merge window. `Clean working tree using: Shelve`. | The most used part after commit and diff. |
| 4 | `phase-4-4-history-compare.md` | File tree and read-only viewer. Opening a file from outside. Git submenu and `Git > Current File`. File history tab. Show Repository at Revision. Comparisons from the log, the branches popup and the changes panel. `Not Cherry-Picked Commits`, `Select in Tree…`, the merge window's `Show Details`. Annotate. Selection history. | Ordered inside the part by use: file history first, Annotate last. |
| 5 | `phase-4-5-github.md` | Keychain and accounts. HTTP client. Login with a token. Protected-branch rules from GitHub. Pull Requests tool window: list, details, status, commits and changes, checkout and Show in Git Log. Open on GitHub and Copy Link. | Needs two new dependencies (decision 15). |
| 6 | `phase-4-6-signing-submodules-worktrees.md` | `Configure GPG Key…` with the key list. `GPG Signature` column and signature in the details. Opening a linked worktree or a submodule as a repository. Submodule update after Update Project. Submodule diff titles. Closing: matrix, `NOTICE`, phase report, consumption measurement. | Small, and it closes the phase. |

Each part ends with its rows in the matrix, its entries in `NOTICE` and its section in `docs/reports/phase-4-validation.md`.

## Global Constraints

The same as phase 3 (plan `2026-11-02-phase-3-conflicts-and-rewriting.md`, Global Constraints), with these changes:
- Reference 241 instead of 232. Bundles are cited from the 241 copy. A text that differs between 232 and 241 takes the 241 value.
- Every new command that changes files, the index, the stash or the shelf calls `flush_diff_window` first and takes `RepoLocks`. The counts of `mutating_without_saving_the_diff_is_not_allowed` and `whatever_touches_the_repository_saves_the_diff_window_first` are updated with it.
- Applying a patch, unshelving, unstashing, Get and checking out a PR branch save a recoverable copy first (`snapshots::create`).
- The shelf, the accounts and the GitHub data are the app's own data (`storage::repo_dir` and `~/Library/Application Support/<app>/`). `.idea/` is only read, by the shelf import.
- Tokens:
  - go only to the macOS Keychain;
  - never go to disk, the console, an error, an argument, the environment or a URL;
  - the interface sends a token once, at login, and never receives it back.
- Text from GitHub (titles, descriptions, labels, logins) is untrusted, like text from a repository: it is shown as text, without HTML, and the CSP is not relaxed.
- Tests stay offline:
  - GitHub through a fake HTTP server on `127.0.0.1:0`;
  - PR refs through local bare remotes with `refs/pull/N/head`;
  - the Keychain through an in-memory store.

## Decisions

Each one departs from IntelliJ 2024.1 or from the approved matrix, or settles an open point. The user approves them with this document. Decisions 1, 9, 13 and 14 come from the user's answers of 2026-11-03.

1. **Modal commit mode, and the setting.** rama adds IntelliJ's modal mode: Local Changes in the `Version Control` tool window and the `Commit Changes` dialog. The `Use non-modal commit interface` setting switches to the current Commit tool window. Unlike IntelliJ 2024.1, whose default is non-modal, the default is **modal**, because that is the user's mode. Cost: `CommitView.svelte` is split in two, and the spec's startup decision ("the app opens on the Commit view") becomes "opens on Local Changes".
2. **The matrix is regenerated from 241.** `scripts/extract_actions.py` runs over the 241 files, and the Result column is kept. Rows that 241 removed (`Vcs.Log.ShowOtherBranches`, `LocalChangesView.ShowOnDoubleClick.*`) leave the table, with a note in the report. Rows that only exist in non-modal mode or in staging mode say so.
3. **No Find Action.** Four actions have no menu entry in IntelliJ, only Find Action: `Compare with Latest Repository Version`, `Compare with Tag…`, `Browse Changes…` and `Annotated Line Operations`. They are marked "no entry point without Find Action". `Compare with Specified Revision…` is invisible for Git in IntelliJ and is marked "no effect". Cost: those four actions cannot be reached.
4. **The shelf is kept in the app's own folder, in IntelliJ's format.** It lives in `storage::repo_dir(<repo>)/shelf/`, with one folder per shelf (`shelved.patch` plus binaries) and one XML file per shelf, the same as `.idea/shelf/`. So one reader serves both rama's shelves and the import. Cost: none.
5. **Repeatable, read-only shelf import.** It reads `.idea/shelf/` (and `CUSTOM_SHELF_PATH` if `workspace.xml` sets one) and skips what was already imported, using description, date and patch hash. A shelf deleted in rama is not imported again. Cost: a shelf changed in IntelliJ after its import comes back as a new one.
6. **Patch apply without the fuzzy algorithm.** rama ports exact matching with offset (`PlainSimplePatchApplier`). If a file does not apply and there is a base, it tries `merge3`, and if that fails it opens the merge window with three contents. With no base, the file is left out and listed in `Patch Applying Partly Failed`. Cost: a patch that IntelliJ would apply with fuzz fails here.
7. **Local History becomes a recoverable copy.** `Before patch` and `After patch` become `snapshots::create`, and `Abort and Rollback` and `Rollback` restore from it. Cost: one 30-day copy per apply.
8. **No `git apply` for unshelving.** IntelliJ shelves have no `index` lines, so `--3way` has no base. Cost: rama needs its own applier, decision 6.
9. **`Clean working tree using:` Shelve or Stash, Shelve by default**, as in IntelliJ. The user does not know their per-project value, so they change it in the settings if needed. Phase 2 decisions 1 and 2 and phase 3 decision 1 are overridden. Cost: the policy changes for the user on the first Update after this part.
10. **After an error or a cancel, the changes stay saved, as in IntelliJ.** The notification offers `View saved changes…`, which opens the Stash or the Shelf tab. Phase 2 decisions 4 and 5 (`Restore Changes`) are overridden, and saves recorded by earlier builds keep working. Cost: one more click than today to get the changes back.
11. **`Unstash…` with the bug fixed.** In 241 both checkboxes of the dialog are bound to `popStash` (`GitUnstashAsDialog.kt:70,76`), so `Reinstate index` never passes `--index`. rama honours each checkbox. Cost: a small departure, with no dead end.
12. **`Changes` and `Repositories` are tool windows**, on the left stripe like in IntelliJ, with closable tabs. They hold `Show Diff with Working Tree`, `Show All Affected Files` and `Show Repository at Revision`. Cost: none.
13. **Only GitHub in phase 4; GitLab later.** The user works with github.com only. The accounts, HTTP and checkout layers are written so that adding GitLab is a new provider. Cost: the merge requests of the spec, `[user]`, wait until the user asks for them. GitHub Enterprise comes for free, through the `Server:` field.
14. **Token login only, and no Create Pull Request.** `Log In via GitHub…` goes through JetBrains' OAuth service, and the app does not use JetBrains services. `Log In with Token…` stays, with `Generate…` opening the pre-filled token page. `Create Pull Request…` and the push notification link are left for later. Cost: the user creates a token once.
15. **Two new dependencies:** `security-framework` for the Keychain, and `ureq` with `native-tls` for HTTPS. Rejected alternatives:
    - `security add-generic-password -w` puts the secret in an argument;
    - `reqwest` is much larger;
    - `curl` gives no proxy control and needs its output parsed.

    Cost: a larger binary. The Keychain may ask for permission after rebuilds that are not signed.
16. **Pull requests without review.** Comments, approve, merge, the timeline and editing reviewers are left out, per the spec. The description is shown as plain text under the title: 241 moved it into the timeline, which is left out. Avatars become initials, so no remote images and no CSP change. The list is polled for changes only while the tool window is visible and the app has focus. The changes are computed locally with git, after the same two fetches IntelliJ does. Cost: visual differences and no Markdown.
17. **PR checkout with the 241 bug fixed.** When a remote with the author's name already exists with another URL, 241 fails with `Branch checkout failed`. rama uses `<login>_1`, `<login>_2`… Cost: a small departure.
18. **No Sync Fork, no clone from the account and no Share Project in phase 4.** They are not in the spec's list. Cost: they are done from the terminal or added later.
19. **Worktrees and submodules as IntelliJ 2024.1 has them: no management window.** 241 has no interface to create or remove worktrees. rama opens a linked worktree or a submodule as a normal repository, with the watcher on its real git folder and on the common one. Update Project runs `git submodule update --recursive` when there are submodules in detached HEAD, as `GitSubmoduleUpdater` does. Cost: `git worktree add` stays in the terminal. The matrix row is renamed.
20. **SSH signing through git, as in 241.** 241 only configures GPG keys (`Configure GPG Key…`). If the user's config has `gpg.format=ssh`, git signs by itself and the `GPG Signature` column shows what git reports (`%G?`). The matrix row "with GPG and SSH" is renamed. Cost: no interface to choose an SSH key.
21. **Editable local side in comparisons**, as in IntelliJ, with the save rules and the hash check of phase 1. Cost: none.
22. **`Show All Branches` does not exist in 241.** File history has a `Branch` filter in its toolbar instead. Without the index it works through plain `git log`, as in 241, except for `History Up to Here`. Of the rows that phase 2 moved here, `Vcs.Log.ShowAllAffected` is done in part 4 and `Vcs.Log.ShowOtherBranches` leaves the table (decision 2).
23. **Not Cherry-Picked Commits by patch only.** Without IntelliJ's index, 241 uses `git cherry`, and so does rama. With the index, IntelliJ also matches author, time and message. Cost: a commit with the same content but a different patch is not greyed out.

## Review Focus for the whole phase

Each part plan turns these into tests in the task that owns them.

1. **Devin writes while a patch, an unshelve or an unstash is being applied.** The recoverable copy is taken first. Nothing that changed after the copy is overwritten without the merge window or the conflict dialog. Parts 3 and 4.
2. **The commit dialog is open while files change.** The dialog keeps its inclusion for files that still exist, drops the ones that are gone, and commits only what it shows. Part 1.
3. **A shelf or patch with binaries, renames, deletions, files without a final newline, CRLF and non-UTF-8 text.** Round trip with IntelliJ's own `shelved.patch` fixtures, made in a throwaway project. Part 3.
4. **A long file history with renames and merges** (more than 1,000 commits, a rename on a side branch). The same result as 241's algorithm, and the window does not freeze. Part 4.
5. **A token never leaks:** not in an error from GitHub, not in the console, not in the fake server's recorded requests except in the `Authorization` header. Part 5.
