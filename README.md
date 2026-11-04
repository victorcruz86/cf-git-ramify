# rama

Lightweight Git client for macOS, built with Tauri. It copies the Git features of IntelliJ IDEA 2023.2.3 and always uses the real `git` binary.

So far it has the Commit view with changelists, partial commit by lines, the diff window and the staging area mode (phase 1), and, in phase 2, the log with its graph, filters, details and context menu, the branches popup and panel, branch, tag and reset operations, fetch, Update Project and Pull, the push dialog with force push and protected branches, remotes, clone and init, and, in phase 3, conflicts (the Conflicts dialog and the three-pane merge window), Merge and Rebase with their dialogs and the branches popup actions, continue, skip and abort, cherry-pick and revert, Edit Commit Message, Drop, Squash, Undo Commit, Fixup and Squash Into, interactive rebase and the three-pane comparison of HEAD, staged and local. Credentials: git's own (credential helper and ssh), with a dialog of its own when git asks for them. What is missing (stash, shelf, history and comparisons in phase 4) and what differs from IntelliJ is in `docs/parity-matrix.md` and in the reports in `docs/reports/`: `phase-1-validation.md`, `phase-2-validation.md`, `phase-2-commands.md`, `phase-2-decisions.md`, `phase-3-validation.md`, `phase-3-commands.md` and `phase-3-decisions.md`.

Phases 2 and 3 are not considered finished: the user finishes them when using them on their real repositories. The manual checklists are in `docs/reports/phase-2-validation.md` and `docs/reports/phase-3-validation.md`.

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

That last command has never been run in the validation of phases 1, 2 and 3: what was tested is the app built in release.

To build the app:

```bash
pnpm tauri build --bundles app
```

The result is left in `src-tauri/target/release/bundle/macos/rama.app`. It carries only a local (ad hoc) signature, not Apple's distribution one. Built on this Mac it should open without more ado. If you copy it from another machine or download it, macOS will say it cannot verify it: go to System Settings > Privacy & Security and click "Open Anyway".

## Opening a repository

```bash
open -n src-tauri/target/release/bundle/macos/rama.app --args /path/to/the/repository
```

`-n` opens a new window even if the app is already running, and it is what `Git > Clone…` uses to open the cloned repository in another window. It can also be opened with a double click from Finder, typing the path in the bar.

## License and attribution

`src-tauri/src/log/` is a Rust port of code from IntelliJ IDEA Community Edition (tag `idea/232.10072.27`, Apache License 2.0); `src/log/` (among others, `draw.ts`, `graphColor.ts` and `labels.ts`), `src/branches/model.ts`, `src/branches/refName.ts` and `src/lib/speedSearch.ts` are TypeScript ports of the same code, and `src-tauri/src/log/testdata/` contains test data from that same repository. The attribution, the changes and the origin files are in `NOTICE`, and the license text in `LICENSE-APACHE`.

## Tests

```bash
pnpm test
(cd src-tauri && cargo test)
(cd scripts && python3 -m unittest test_extract_actions test_measure)
```
