# Ramify

Lightweight Git client for macOS, built with Tauri. It copies the Git support of IntelliJ IDEA Community Edition 2024.1 (phases 1 to 3 copied 2023.2.3) and always uses the real `git` binary. It was called rama until 2026-11-10; on its first start Ramify copies rama's data folder and its webview storage (the interface settings), and on first use its GitHub token in the Keychain (`src-tauri/src/migration.rs`). The originals are never moved or deleted.

What it does, by phase:

- **Phase 1:** the Commit view with changelists, partial commit by lines, the diff window and the staging area mode.
- **Phase 2:** the log with its graph, filters, details and context menu; the branches popup and panel; branch, tag and reset operations; fetch, Update Project and Pull; the push dialog with force push and protected branches; remotes, clone and init.
- **Phase 3:** conflicts (the Conflicts dialog and the three-pane merge window); Merge and Rebase; continue, skip and abort; cherry-pick and revert; Edit Commit Message, Drop, Squash, Undo Commit, Fixup and Squash Into; interactive rebase.
- **Phase 4:** the modal commit dialog; patches, stash and shelf, with the import of IntelliJ's shelf; annotate, file history, the selection history and comparisons; GitHub accounts, protected branches and the Pull Requests tool window; commit signing, linked worktrees and submodules.
- **Several project windows:** each repository in its own window of the same process, `This Window` / `New Window`, closing, quitting and reopening projects, the menu bar following the focused window, and the project widget.

Credentials are git's own (credential helper and ssh), with a dialog of its own when git asks for them; GitHub tokens live only in the Keychain. What is missing and what differs from IntelliJ is in `docs/parity-matrix.md` and in the reports in `docs/reports/` (`phase-1-validation.md` to `phase-4-validation.md`, plus the commands and decisions of each phase).

No phase is considered finished until the user has used it on their real repositories. The manual checklists are in section 5 of each part of `docs/reports/phase-4-validation.md` and in the earlier validation reports.

## Requirements

- macOS on Apple Silicon.
- Node of the version given in `.nvmrc`. The Node that the default shell ships is too old for the tests.
- pnpm.
- Rust. `rust-toolchain.toml` pins the version by itself.

## How to run it

```bash
nvm use
pnpm install
pnpm tauri dev
```

That last command has never been run in the validation of the phases: what was tested is the app built in release.

To build the app:

```bash
pnpm tauri build --bundles app
```

The result is left in `src-tauri/target/release/bundle/macos/Ramify.app`. It carries only a local (ad hoc) signature, not Apple's distribution one. Built on this Mac it should open without more ado. If you copy it from another machine or download it, macOS will say it cannot verify it: go to System Settings > Privacy & Security and click "Open Anyway".

## Opening a repository

```bash
open -a src-tauri/target/release/bundle/macos/Ramify.app /path/to/the/repository
```

If Ramify is running, the path goes to the last focused window and through the same question as `File > Open…`; if it is not, it should start with that repository. Neither case has been seen in a real run yet. To open another repository while Ramify runs, use `File > Open…`, `File > Recent Projects` or the project widget in the toolbar: each asks `This Window` or `New Window` (the setting `Open project in` keeps the answer), and a repository already open in a window is focused instead of opened twice. `open -n …/Ramify.app --args /path/to/the/repository` also works but is a known gap: it starts a second Ramify process that knows nothing about the first (the open and recent project lists are shared files, and the last writer wins). Ramify reopens the projects that were open at the last quit, unless `Reopen projects on startup` is off in the Settings. Double-clicking Ramify in Finder also works: start it, then type the path in the bar or use File > Open….

## License and attribution

Ramify is licensed under the Apache License 2.0; the text is in `LICENSE`.

Parts of Ramify are ports of code from IntelliJ IDEA Community Edition (Apache License 2.0): phases 1 to 3 from tag `idea/232.10072.27`, phase 4 from tag `idea/241.14494.240`. Among them, `src-tauri/src/log/` is a Rust port; `src/log/` (among others, `draw.ts`, `graphColor.ts` and `labels.ts`), `src/branches/model.ts`, `src/branches/refName.ts` and `src/lib/speedSearch.ts` are TypeScript ports of the same code, and `src-tauri/src/log/testdata/` contains test data from that same repository. The attribution, the changes and the origin files are in `NOTICE`, and the license text in `LICENSE`.

## Tests

```bash
pnpm test
(cd src-tauri && cargo test)
python3 -I -m unittest discover -s scripts
```
