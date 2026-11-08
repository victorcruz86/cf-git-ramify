# Phase 4, part 3: patches, stash and shelf. Implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** the user saves, moves and restores uncommitted work as in IntelliJ 2024.1:
- patches: Copy as Patch to Clipboard, Create Patch from Local Changes, Apply Patch, Apply Patch from Clipboard;
- stash: Stash Changes, Unstash, the Stash tab;
- shelf: Shelve, Unshelve, the Shelf tab, and import from `.idea/shelf`;
- `Clean working tree using: Shelve` for Update, rebase, checkout and Smart Merge.

**Architecture:**
- Rust owns every format and every write. Three new modules:
  - `patch.rs` reads and writes IntelliJ's patch format;
  - `patch_apply.rs` applies a patch with a recoverable copy first;
  - `shelf.rs` keeps the shelf store in IntelliJ's layout under `storage::repo_dir(<repo>)/shelf/`.

  `stash.rs` wraps `git stash`.
- `preserve.rs` gets two savers, Stash and Shelve, behind one interface. Its five callers stay as they are.
- The interface reuses the part 1 commit dialog as the Shelve and Create Patch executor, the merge editors in a new "contents" mode for patch conflicts, and a new read-only two-text diff view, extracted from `DiffView.svelte`, for stash and shelf previews.
- `Stash` and `Shelf` are tabs of the `Git` tool window, or of the `Commit` window in non-modal mode. Each is visible only while it has entries, as in IntelliJ.

**Tech Stack:** Tauri 2.12, Rust 1.99.1, Svelte 5, Vitest 5, CodeMirror 6. One new dependency: `tauri-plugin-dialog` (decision 1).

**Spec:** `docs/superpowers/specs/2026-10-31-intellij-git-client-design.md` and the phase 4 overview `docs/superpowers/plans/2026-11-03-phase-4-overview.md`. The overview's Global Constraints and Decisions, especially 4 to 11, are part of this plan.

Research (not in the repository), which the implementer reads before each task:
- `.superpowers/phase-4/research-stash-shelf-patches.md`: user-facing behaviour, entry points, texts;
- `.superpowers/phase-4/research-part-3-patches-241.md`: exact reader, writer, applier and shelf rules, dialog layouts, shortcuts, fixtures;
- `.superpowers/phase-4/research-part-3-rama.md`: what rama has today, with lines.

Each task names the sections it implements.

**Reference:** tag `idea/241.14494.240`. Paths are relative to `intellij-community`:
- `VI/` = `platform/vcs-impl/src/com/intellij/openapi/`;
- `G/` = `plugins/git4idea/src/git4idea/`.

Bundles: `V:` = VcsBundle, `GB:` = GitBundle, `A:` = ActionsBundle. Copy literal texts from the cited line, not from this plan.

Fixtures: `.superpowers/ref-idea-241/ic/platform/platform-tests/testData/diff/` and `.../vcs-tests/testData/shelf/`. These are Apache 2.0. Copied fixtures go to `src-tauri/testdata/patch/` and `src-tauri/testdata/shelf/` and are listed in `NOTICE`.

**Branch:** `phase-4`, after part 2 (`018cddf`).

**Commands** (keep the machine load low; one suite at a time):
- TS: `PATH=$HOME/.cache/rama-toolchain/node-v22.23.4-darwin-arm64/bin:$PATH pnpm exec vitest run --maxWorkers=2 <files>` and `pnpm exec tsc --noEmit -p .`
- Rust: `PATH=$HOME/.cargo/bin:$PATH CARGO_BUILD_JOBS=4 cargo test --manifest-path src-tauri/Cargo.toml --lib <filter> -- --test-threads=4`
- Python: `python3 -I -m unittest discover -s scripts`

## Global Constraints

The overview's Global Constraints, which include phase 3's. The ones this part leans on most:
- Applying a patch, unshelving and unstashing save a recoverable copy first (`snapshots::create`).
- Every new command that changes files, the index, the stash or the shelf calls `flush_diff_window` first, takes `RepoLocks`, and updates the counts of `mutating_without_saving_the_diff_is_not_allowed` and `whatever_touches_the_repository_saves_the_diff_window_first`.
- The shelf is the app's own data (`storage::repo_dir`). `.idea/` is only read, by the import.
- Patch text, shelf descriptions and stash messages are untrusted text: shown as text, no `{@html}`. Every path in a patch is validated in Rust (`snapshots::check_path`), so nothing escapes the repository and nothing starting with `-` reaches git as an option.
- Real `git` with arguments, `--` before paths, no shell. `pbpaste`, `pbcopy` and `open -R` run as commands with arguments.
- Tests use throwaway repositories, never the network, `.idea/` of a real project, or `~/Library/Application Support/JetBrains`.
- Commits end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Decisions of this part

They add to the overview's decisions 4 to 11. The user approves them with this plan. Each has its cost if wrong.

1. **Native file choosers through `tauri-plugin-dialog`.** It is the official Tauri plugin: NSOpenPanel and NSSavePanel, added as a crate, an npm package and one capability. It is used by:
   - Apply Patch: `Select Patch File`, the path field, `Map Base Directory…`, `Select missing base`;
   - Patch File Settings: `To file:`, `Base path:`;
   - `Import Patches…`.

   Rejected alternatives:
   - typed paths only: today's Open repository field, with no browsing for 2,700 patch uses a year;
   - `osascript choose file`: an old-style dialog.

   Cost: one more dependency (the overview listed only the two for GitHub). Typed paths still work in every field.
2. **The clipboard is read with `/usr/bin/pbpaste`,** next to the existing `pbcopy` writer, with `LC_CTYPE=UTF-8`. Reveal in Finder is `open -R <path>`. Cost: none.
3. **Binary sections go through git.**
   - Writing: rama writes each binary file's `GIT binary patch` section with `git diff --binary` between two blobs made with `git hash-object -w --stdin --no-filters`.
   - Applying: it applies a binary entry with `git apply --binary` on a temporary patch that holds only that entry.
   - Shelf binaries are whole-file copies, as in IntelliJ, so the shelf needs neither.
   - Not copied from IntelliJ: zlib bytes byte for byte (the decoded bytes are equal), and its zero-byte decoding of `delta` sections, a bug git does not have.

   Cost: binary bytes in a written patch differ from IntelliJ's; both apply the same.
4. **The applier is exact, then exact at an offset, then a 3-way merge with the base, then the merge window.** This corrects the overview's decision 6: IntelliJ's `PlainSimplePatchApplier` is exact only, and the offset search belongs to the fuzzy `GenericPatchApplier`. rama's rules:
   1. each hunk exactly at its stated line;
   2. otherwise the same whole hunk at the nearest offset within ±1000 lines, with hunks kept in order and not overlapping;
   3. otherwise, if every hunk matches reversed, the file is "already applied";
   4. otherwise, with a base (the shelf's `<+>` text, or `(revision h)` for non-git patches, as in IntelliJ), `merge3::try_resolve(base, local, patched base)`, and if that fails the merge window;
   5. with no base, the file fails and is listed in `Patch Applying Partly Failed`.

   Not copied:
   - split hunks at different offsets;
   - trimmed context;
   - the "Result of Patch Apply" viewer for files without a base;
   - the context-diff format.

   Cost: a patch that IntelliJ applies with fuzz, or opens in that viewer, fails here, and the user re-applies it by hand.
5. **Paths in Apply are relative to the repository root.** `Remove/Restore Leading Directory`, `Remove/Restore All Leading Directories` and `Map Base Directory…` are copied. IntelliJ's project search by name and context (`MatchPatchPaths`) is not. Cost: a patch made in another folder layout needs the strip or map actions.
6. **One encoding: UTF-8.** rama treats text as UTF-8 everywhere. The `Encoding:` row of Patch File Settings shows `UTF-8`, disabled. Patches are written in UTF-8 with `\n`. Cost: no other charsets.
7. **Separate `Stash` and `Shelf` tabs, the 2024.1 default.** `Combine stashes and shelves in one tab` is not copied. The `Vcs.Shelf.*` rows stay "no effect". Cost: a setting the user has off.
8. **No commit checks for Shelve and Create Patch.** IntelliJ's Git checks return early when the commit is not a VCS commit (`G/checkin/GitCheckinHandlerFactory.kt:513-514`, `isVcsCommit`), so CRLF, large-file, file-name, detached HEAD and rebase warnings never show for these executors. The executor path still gets a catch with a notification, the draft saved as the list comment, and a tie to its repository. Cost: none.
9. **The patch folder starts at the repository root.** rama does not read `PATCH_STORAGE_LOCATION`. It remembers the last folder per repository and for the whole app, as IntelliJ does. Cost: the first time, the user browses to `~/Downloads/PATCHS` once.
10. **Not copied:**
    - drag and drop between Local Changes and the Shelf;
    - the `Shelf` settings page (location, base content);
    - `Getting started with Stash and Shelf` (a help link);
    - the `Help` buttons.

    Cost: a few seconds per use for drag and drop, and a page the user does not use.
11. **The shelf base (`<+>` lines) is written by rama's own reading of IntelliJ's code.** No fixture at the tag has one. The manual checklist includes importing a real IntelliJ shelf, with a tab and an emoji in it. Cost: if the escape rules are wrong, imported bases fail and those files fall back to "no base" (decision 4, step 5).

## Review Focus

1. **Devin writes a file while a patch, an unshelve or an unstash is applied.**
   - The recoverable copy is taken first.
   - The applier writes a file only if its content still equals what it read, and otherwise reports it as failed.
   - Nothing changed after the copy is overwritten without the merge window or the conflict dialog.

   Tests in Tasks 2 and 10.
2. **Round trip with binaries, renames, deletions, no final newline, CRLF and non-UTF-8 text.** Create a patch, apply it to a clean copy, and the tree is identical. A shelf written by rama is read back identical. The IntelliJ fixtures read, apply and write as their expected files. Tests in Tasks 1, 2 and 8.
3. **Apply Patch from Clipboard with text that is not a patch, or a 20 MB clipboard.** The action says `Cannot Apply Patch Now` or `No changes detected` as IntelliJ does, with no freeze. Test in Task 5.
4. **Update with `Clean working tree using: Shelve` stops in a conflict.**
   - The changes stay in the shelf.
   - `View saved changes…` opens the Shelf tab with that list selected.
   - A stash saved by an earlier build still shows `Restore Changes` and can be restored.

   Test in Task 11.
5. **The repository switches while Shelve, Create Patch or Apply runs.** The work finishes on the repository that started it, or is refused before any write. Tests in Tasks 4, 5 and 9.

---

### Task 1: Patch reader and writer

Research: `research-part-3-patches-241.md` §1 (reader), §2 (writer); `research-stash-shelf-patches.md` §3 (format). Reference: `VI/diff/impl/patch/{PatchReader,PatchFileHeaderParser,GitPatchParser,UnifiedDiffWriter,BinaryPatchContentParser}.java`, `VI/vcs/changes/patch/{PatchWriter,GitPatchWriter,BinaryPatchWriter}.java`, `TextPatchBuilder`.

**Files:**
- Create: `src-tauri/src/patch.rs`, `src-tauri/testdata/patch/` (copied from `platform-tests/testData/diff/{patch,patchReader,patchTextDetection}`).
- Modify: `src-tauri/src/lib.rs` (module), `NOTICE` (ported code and fixtures).

**Interfaces:**
- Produces:
  - `pub struct Patch { pub header: Header, pub files: Vec<FilePatch> }`, with `Header { subject: Option<String>, message: Option<String> }`;
  - `pub enum FilePatch { Text(TextFilePatch), Binary(BinaryFilePatch) }`;
  - `TextFilePatch { before: Option<String>, after: Option<String>, before_rev: Option<String>, after_rev: Option<String>, new_mode: Option<u32>, hunks: Vec<Hunk>, base: Option<Vec<u8>>, charset: Option<String> }`;
  - `Hunk { before_start, before_count, after_start, after_count, lines: Vec<Line> }`, with `Line { kind: Context|Added|Removed, text: Vec<u8>, no_newline: bool }`;
  - `BinaryFilePatch { before, after, git_section: Vec<u8> }` (the raw `GIT binary patch` block).
- Functions:
  - `pub fn read(text: &[u8]) -> Result<Patch, PatchError>`;
  - `pub fn is_patch(text: &[u8]) -> bool`;
  - `pub fn write(patch: &Patch, options: &WriteOptions) -> Vec<u8>`, with `WriteOptions { include_base: bool, reverse: bool, base_path: PathBuf }`;
  - `pub fn build(repo: &Path, entries: &[PatchSource], options: &WriteOptions) -> Result<Patch, PatchError>`.
- `PatchSource { before: Option<(String, Vec<u8>)>, after: Option<(String, Vec<u8>)>, before_rev: Option<String>, binary: bool }`. Its contents come from Task 4's helper. Hunks are built with `similar` at 3 lines of context.

- [ ] **Step 1: Write the failing tests** in `patch.rs`:
  - `writer_matches_intellij_fixtures`: each `patch/<case>/{before,after}` gives exactly `expected.patch` with the date and revision suffixes masked. It includes `modifyWithCRLF`, the `modifyNewline` cases, `addToEmptyFile` and `deleteWholeFile`.
  - `reader_parses_headers_and_subject` (`patchReader/*`).
  - `patch_text_detection_matches_intellij` (`patchTextDetection/*`, true and false).
  - `base_and_charset_lines_round_trip`: a text with a tab, `\`, a newline and an emoji survives `<+>` writing and reading.
  - `binary_section_comes_from_git_and_reads_back`: a PNG patch from `build` applies with `git apply --binary` on a copy.
  - `rename_new_deleted_and_mode_headers`.
  - `cr_never_reaches_hunk_lines`.
  - `property_create_then_read_is_identity`: 2,000 seeded random cases, as in `CreateApplyPatchSymmetryTest.kt:29-67`.
- [ ] **Step 2: Check that they fail.** Expected: FAIL.
- [ ] **Step 3: Implement**, following the order, separators and escape rules of §1 and §2 literally. Paths with spaces or non-ASCII characters use git's quoting rules (§1.5).
- [ ] **Step 4: Check that they pass**, then run clippy.
- [ ] **Step 5: Commit.** `git commit -m "feat: patch reader and writer in IntelliJ's format"`

### Task 2: Patch applier with a recoverable copy

Research: `research-part-3-patches-241.md` §3 (all), decision 4. Reference: `VI/diff/impl/patch/formove/PatchApplier.java`, `PathsVerifier.java`, `VI/vcs/changes/patch/ApplyPatchAction.java:135-233`.

**Files:**
- Create: `src-tauri/src/patch_apply.rs`, `src-tauri/testdata/patch/applyPatch/` (copied).
- Modify: `src-tauri/src/lib.rs`, `src-tauri/src/vcs_commands.rs` (commands), `NOTICE`.

**Interfaces:**
- Consumes: `patch::{read, Patch}`, `snapshots::{create, restore}`, `merge3::try_resolve`.
- Produces:
  - `pub fn plan(repo, patch, mapping: &PathMapping) -> ApplyPlan`: per file, the target path and `Status { Applicable, AlreadyApplied, MissingBase, Missing, Exists }`, with no writes;
  - `pub fn apply(ctx, repo, data, patch, mapping, selected: &[String], overwrite: &[String]) -> ApplyResult`.
- `ApplyResult { snapshot: SnapshotId, status: Status(Success|AlreadyApplied|Partial|Failure|Abort), failed: Vec<String>, conflicts: Vec<PatchConflict> }`, with `PatchConflict { path, base, local, patched, local_hash }`.
- `pub fn rollback(data, repo, snapshot)` restores the snapshot.
- The status order and its `and` follow `ApplyPatchStatus.java:28-53`.
- Commands, each with `flush_diff_window`, `RepoLocks` and its count:
  - `patch_plan(source: PatchInput)`;
  - `patch_apply(source, mapping, selected, overwrite, expected_head)`;
  - `patch_conflict_resolve(path, content, local_hash)`;
  - `patch_rollback(snapshot)`.
- `PatchInput = File(path) | Text(string)`.

- [ ] **Step 1: Write the failing tests:**
  - `applies_intellij_fixtures_with_their_status`: every `applyPatch/<case>` gives `after/` and the status `ApplyPatchTest.java` expects. The context-diff cases and those that need Generic fuzz are listed in a skip table, with the reason.
  - `a_hunk_moved_by_three_lines_applies_at_the_offset`.
  - `two_hunks_keep_their_order_at_offsets`.
  - `whole_file_already_applied`.
  - `with_a_base_try_resolve_writes_the_merge`.
  - `with_a_base_and_a_real_conflict_returns_a_conflict` (nothing written to that file).
  - `without_a_base_the_file_fails`.
  - `new_file_that_exists_is_skipped_unless_overwrite`.
  - `deleting_a_missing_file_counts_as_applied`.
  - `binary_entry_applies_through_git`.
  - `a_snapshot_is_taken_before_any_write_and_rollback_restores_it`.
  - `a_file_changed_after_reading_is_not_overwritten` (Review Focus 1: the file changes between plan and write, and the result lists it as failed).
  - `patch_conflict_resolve_refuses_when_the_local_hash_changed`.
  - The command count tests.
- [ ] **Step 2: Check that they fail.** Expected: FAIL.
- [ ] **Step 3: Implement.** Moves happen only after a successful apply (`doMoveIfNeeded`). New and deleted files are added to or removed from git silently (`git add -- p`, `git rm --cached -- p` where tracked).
- [ ] **Step 4: Check that they pass.**
- [ ] **Step 5: Commit.** `git commit -m "feat: patch applier with a recoverable copy first"`

### Task 3: Infrastructure: choosers, clipboard read, Finder, merge window with contents, two-text diff

Decisions 1 and 2. Research: `research-part-3-rama.md` §7, §9, §10.

**Files:**
- Modify:
  - `src-tauri/Cargo.toml`, `package.json` and the lockfiles (`tauri-plugin-dialog` 2.x, the version matching Tauri 2.12; exact version pinned);
  - `src-tauri/capabilities/default.json` (`dialog:allow-open`, `dialog:allow-save` only);
  - `src-tauri/src/lib.rs`;
  - `src-tauri/src/log/commands.rs` (`clipboard_read` next to `clipboard_write`);
  - `src-tauri/src/remotes.rs` or a new spot next to `open_in_text_editor` (`reveal_in_finder(path)`);
  - `src-tauri/src/commands.rs` (merge session);
  - `src/merge/MergeWindow.svelte`, `src/merge/window.ts`;
  - `src/lib/api.ts`.
- Create: `src/lib/choosers.ts` (`chooseFile`, `chooseFolder`, `chooseSaveFile`, each `Promise<string | null>`) and `src/diff/TextsDiff.svelte` (props `{ oldText, newText, path, titles?: [string, string] }`, read-only; its Soft-Wrap place is the one IntelliJ's saved-patches preview uses, read from `VI/vcs/changes/savedPatches/`, or `Default` if it sets none).
- Test: `src/lib/choosers.test.ts`, `src/merge/MergeWindow.test.ts`, `src/diff/TextsDiff.test.ts`, Rust tests in the touched files.

**Interfaces:**
- Produces:
  - `clipboardRead(): Promise<string>`, `revealInFinder(path): Promise<void>`;
  - `mergeWindowOpenContents(input: { id: string; title: string; titles: [string, string, string]; left: string; base: string; right: string; path: string }): Promise<void>`. The window's `merge-done` event then carries `{ id, result, content? }`.
  - In contents mode, Save returns the text in the event. It does not call `conflict_resolve`, and it skips `check()`.
- Every command that reads a chosen path validates it in Rust: it must be absolute, and the call is refused when it is not.

- [ ] **Step 1: Write the failing tests:**
  - the chooser wrappers return `null` on cancel;
  - `clipboard_read_returns_utf8` (Rust, through a fake `pbpaste` path injected like `copy_with`);
  - `contents_mode_saves_into_the_done_event_and_never_calls_conflict_resolve`;
  - `contents_mode_shows_the_given_titles`;
  - `TextsDiff` shows both texts with their titles.
- [ ] **Step 2: Check that they fail.**
- [ ] **Step 3: Implement.** `DiffView.svelte` keeps working through the extracted view.
- [ ] **Step 4: Check that they pass.** Then run `git diff --stat` and confirm the capability file grants only open and save.
- [ ] **Step 5: Commit.** `git commit -m "feat: file choosers, clipboard read, Reveal in Finder and a contents mode for the merge window"`

### Task 4: Copy as Patch and Create Patch

Research: `research-stash-shelf-patches.md` §7 (Copy, Create, Create Patch... from a selection), `research-part-3-patches-241.md` §6.2. Decisions 6, 8 and 9. Carried from part 1: the executor path (`.superpowers/phase-4/part-1-carried.md`, Task 6).

**Files:**
- Create:
  - `src/patch/PatchSettingsDialog.svelte` (`Patch File Settings`);
  - `src/patch/CreatedPatchDialog.svelte` (`Create Patch` / `Successfully created patch <path>` with `Reveal in Finder`, `Close`, `Don't ask again`);
  - `src/patch/flow.ts`.
- Modify:
  - `src-tauri/src/patch.rs`: `changelist_content(repo, path, selection) -> PatchSource`, which picks HEAD and `stored_form` or `partial_content` (`research-part-3-rama.md` §2);
  - `src-tauri/src/vcs_commands.rs`: `patch_create(entries, options, to: File(path) | Clipboard, expected_repo)`;
  - `src-tauri/src/settings.rs`: `patch_folder` per repository and app-wide, `show_patch_in_explorer: Option<bool>`, `patch_to_clipboard` per repository;
  - `src/commit/dialog.ts`, `CommitDialog.svelte`, `dialogStore.ts`:
    - `requestExecutor(executor, request)`;
    - the executor branch gets a catch with a notification, the draft saved as the list comment, a repository tie (`sameRepo`), and `oldPath` for renames in `ExecutorChanges`;
  - `src/commit/ChangesPanel.svelte`: popup items `Create Patch from Local Changes...` and `Copy as Patch to Clipboard` in their 241 place (`research-stash-shelf-patches.md` §1);
  - `src-tauri/src/commands.rs`:
    - the `Patch` submenu of the Git menu (`Group` takes a title);
    - `Create Patch from Local Changes...` hidden in staging mode;
  - `src/commit/StagingView.svelte`: `Create Patch...` and `Copy as Patch to Clipboard` with the HEAD→index, index→disk and HEAD→disk rule;
  - `src/log/` changes tree: `Create Patch...` and `Copy as Patch to Clipboard`.
- Test: `src/patch/flow.test.ts`, `src/patch/PatchSettingsDialog.test.ts`, `src/commit/CommitDialog.test.ts`, `src/commit/LocalChangesView.test.ts`, `src/commit/StagingView.test.ts`, Rust `vcs_commands.rs` and `settings.rs`.

**Interfaces:**
- Consumes: `patch::{build, write}` (Task 1), `chooseSaveFile`, `chooseFolder`, `revealInFinder` (Task 3).
- Produces: `src/patch/flow.ts` `patchFlow(ui, api)` with `copyAsPatch(changes)` and `createPatch(changes, message)`. Task 9 reuses the executor plumbing.

- [ ] **Step 1: Write the failing tests:**
  - `copy_as_patch_puts_the_selected_changes_on_the_clipboard_and_says_so` (`Patch copied to clipboard`).
  - `create_patch_opens_the_dialog_with_create_patch_button_then_the_settings`:
    - default file `<folder>/<message sanitised>.patch`;
    - the `Subject:` header from the message;
    - a partial changelist writes only its hunks.
  - The settings validation texts V:439-442.
  - The overwrite question V:434 and V:406.
  - `Reverse patch`.
  - `To clipboard` is remembered.
  - `the_created_dialog_offers_reveal_and_remembers_dont_ask_again`.
  - `executor_failure_is_notified_and_the_draft_is_kept`.
  - `a_repository_switch_during_create_patch_writes_for_the_first_one` (Review Focus 5).
  - Staging rule tests.
  - `create_patch_from_local_changes_is_hidden_in_staging_mode`.
  - Rust: `patch_create_writes_utf8_with_lf`.
- [ ] **Step 2: Check that they fail.**
- [ ] **Step 3: Implement.** Every text comes from V and A as cited in the research.
- [ ] **Step 4: Check that they pass.**
- [ ] **Step 5: Commit.** `git commit -m "feat: Copy as Patch and Create Patch as in IntelliJ 2024.1"`

### Task 5: Apply Patch and Apply Patch from Clipboard

Research: `research-stash-shelf-patches.md` §7 (Apply, from Clipboard), `research-part-3-patches-241.md` §3.4-3.5 and §6.1, 6.3, 6.4. Decisions 4 and 5.

**Files:**
- Create:
  - `src/patch/ApplyPatchDialog.svelte`, with modes `apply`, `clipboard` and `unshelve` (`unshelve` is used by Task 10):
    - `Patch file name:` with browse;
    - the toolbar (`Show Difference` ⌘D, `Change Directory Paths`, `Refresh`);
    - the tree with checkboxes and the statuses, the legend, and the changelist chooser;
    - the buttons per mode;
    - the `Analyze and apply patch from clipboard on the fly` checkbox;
  - `src/patch/PartlyFailedDialog.svelte`;
  - `src/patch/OverwriteDialog.svelte`.
- Modify:
  - `src/patch/flow.ts`: `applyPatch()`, `applyFromClipboard()`, conflicts through `mergeWindowOpenContents` with the titles V:350-354, and `Abort…` (V:381-386);
  - `src-tauri/src/settings.rs`: `detect_patch_on_the_fly`, default false;
  - `src/App.svelte`: on-the-fly detection when the window gains focus with the setting on;
  - the Git menu `Patch` submenu: `Apply Patch...`, `Apply Patch from Clipboard...`;
  - `src/lib/notifications.ts` uses: V:482-484 and V:360-363.
- Test: `src/patch/ApplyPatchDialog.test.ts`, `src/patch/flow.test.ts`, `src/App.test.ts`.

**Interfaces:**
- Consumes: `patch_plan`, `patch_apply`, `patch_conflict_resolve`, `patch_rollback` (Task 2), `clipboardRead`, `chooseFile`, `chooseFolder`, `TextsDiff`, `mergeWindowOpenContents` (Task 3).
- Produces: `ApplyPatchDialog` props `{ mode, plan, lists, onanswer }`, reused by Task 10.

- [ ] **Step 1: Write the failing tests:**
  - `apply_patch_asks_select_patch_file_then_shows_the_tree_with_added_modified_deleted_and_missing_base`.
  - Changelist chooser: the name and description come from `Subject:` and the message, as in V and `ApplyPatchDifferentiatedDialog.java:404-432`.
  - `strip_and_map_change_the_targets`.
  - Overwrite: `Overwrite Existing Files` for one file and for several.
  - `a_conflict_opens_the_merge_window_and_abort_asks_abort_and_rollback_skip_or_continue`.
  - `failures_show_patch_applying_partly_failed_and_rollback_restores`.
  - The notifications for ALREADY_APPLIED, PARTIAL and SUCCESS.
  - `from_clipboard_without_a_patch_says_cannot_apply_patch_now`, and a 20 MB non-patch text answers in under 1 s with `is_patch` (Review Focus 3).
  - `on_the_fly_offers_the_dialog_once_per_clipboard_text`.
  - `a_repository_switch_during_apply_is_refused_before_writing` (`expected_head`).
- [ ] **Step 2: Check that they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Check that they pass.**
- [ ] **Step 5: Commit.** `git commit -m "feat: Apply Patch and Apply Patch from Clipboard"`

### Task 6: Stash core, Stash Changes and Unstash

Research: `research-stash-shelf-patches.md` §2 (Stash Changes…, the Unstash dialog, shared commands and results). Overview decision 11 (the bug fixed).

**Files:**
- Create:
  - `src-tauri/src/stash.rs`:
    - `list` (`--pretty=format:%H:%P:%at:%gd:%s`);
    - `save(message, keep_index)` (`stash push -m` when the message starts with `-`);
    - `push_paths`;
    - `unstash(entry, pop, reinstate_index, branch)`;
    - `drop`, `clear`;
    - `changes(entry)` by parent (index and untracked groups);
    - `file_sides(entry, path)`;
  - `src/stash/StashDialog.svelte`, `src/stash/UnstashDialog.svelte`, `src/stash/UnstashAsDialog.svelte`, `src/stash/flow.ts`.
- Modify:
  - `src-tauri/src/vcs_commands.rs`: commands with flush, locks and counts. Read-only list and changes run without the write lock, as `console.rs` `READING_FORMS` already allows;
  - `src-tauri/src/commands.rs`: the Git menu `Uncommitted Changes` group, with `Stash Changes…` and `Unstash Changes…`;
  - `src/commit/StagingView.svelte`: `Stash Silently` and `Stash Files` (staging only);
  - `src/conflicts/` titles for unstash conflicts (GB:470-472);
  - notifications `Files stashed successfully` with `View Git stashes` (GB:432-434).
- Test: `src-tauri/src/stash.rs`, `src/stash/*.test.ts`.

**Interfaces:**
- Produces:
  - `StashEntry { sha, parents, time, index: u32, subject, branch: Option<String> }`;
  - `stash_list()`, `stash_save(message, keepIndex)`, `stash_unstash(sha, pop, reinstateIndex, branch, expectedHead)`, `stash_drop(sha)`, `stash_clear()`, `stash_changes(sha)`, `stash_file_sides(sha, path)`.
  - Entries are named by sha, and Rust maps a sha to `stash@{n}` at the moment of the call (refused if it is gone).
  - A recoverable copy is taken before an unstash that touches files.

- [ ] **Step 1: Write the failing tests:**
  - Rust:
    - `save_then_list_then_pop_round_trip`;
    - `a_message_starting_with_dash_goes_through_push_m`;
    - `unstash_with_reinstate_index_passes_index`, which pins decision 11 (both checkboxes honoured);
    - `unstash_as_new_branch`;
    - `drop_by_sha_when_positions_moved`;
    - `index_conflict_says_unstash_failed_due_to_an_index_conflict`;
    - `conflicts_leave_the_entry_and_return_unmerged`;
    - `untracked_would_be_overwritten_lists_the_files`;
    - `changes_group_index_and_untracked_parents`;
    - `a_snapshot_precedes_unstash`.
  - TS:
    - the `Stash` dialog texts and Keep index;
    - `Remove Stash stash@{0}?`;
    - `Remove All Stashes?`;
    - `Unstash Changes in <root>` with `As new branch:`, `Pop stash` and `Reinstate index`, and the OK texts.
- [ ] **Step 2: Check that they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Check that they pass.**
- [ ] **Step 5: Commit.** `git commit -m "feat: Stash Changes and Unstash with both options honoured"`

### Task 7: The Stash tab

Research: `research-stash-shelf-patches.md` §1 (Stash tab, Show Git Stash, Unstash Changes… toggling) and §2 (The Stash tab). Decision 7. Carried from part 1: `VersionControlWindow` takes `aria-label` as a prop.

**Files:**
- Create: `src/stash/SavedPatchesView.svelte`, the shared layout for Task 10:
  - list over changes on the left, `TextsDiff` preview on the right;
  - an `Apply` (⌘⏎) and `Pop` bar with the `?` tooltip;
  - speed search;
  - empty texts.

  Also `src/stash/StashTab.svelte`.
- Modify:
  - `src/App.svelte`: `AVAILABLE` derived from content (Stash while `stash_list` is not empty);
  - `src/lib/tabs.ts`: a vanished selected tab falls back to the first;
  - `src/vcs/VersionControlWindow.svelte`: `label` prop; the Commit window renders its strip in non-modal mode;
  - `src/stash/flow.ts`: `Show Git Stash` toggle (`VcsShowToolWindowTabAction` rules), and `Unstash Changes…` toggling the tab or opening the dialog when there are no stashes;
  - the watcher refresh on `refs/stash` changes.
- Test: `src/stash/StashTab.test.ts`, `src/lib/tabs.test.ts`, `src/App.test.ts`.

- [ ] **Step 1: Write the failing tests:**
  - `the_stash_tab_appears_with_the_first_stash_and_leaves_with_the_last`.
  - `rows_show_subject_branch_and_tooltip`.
  - `the_changes_of_a_stash_group_index_and_untracked`.
  - `double_click_shows_the_diff_titled_stash_0_file`.
  - `apply_and_pop_never_pass_index`.
  - The context menu order: `Pop`, `Apply`, `Unstash…`, `Drop`, separator, `Show Diff`, `Show Diff in a New Tab or Window`, `Compare with Local`.
  - `drop_with_delete_and_backspace_asks_first`.
  - The 4-state `show_git_stash_toggles` table.
  - `unstash_changes_opens_the_dialog_only_without_stashes`.
  - `the_window_label_follows_the_window`.
- [ ] **Step 2: Check that they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Check that they pass.**
- [ ] **Step 5: Commit.** `git commit -m "feat: the Stash tab"`

### Task 8: Shelf store and import from `.idea/shelf`

Research: `research-part-3-patches-241.md` §4 (all), `research-stash-shelf-patches.md` §3. Overview decisions 4 and 5; decision 11.

**Files:**
- Create: `src-tauri/src/shelf.rs`, `src-tauri/testdata/shelf/` (copied from `vcs-tests/testData/shelf/`).
- Modify:
  - `src-tauri/src/idea_import.rs`: read `VcsManagerConfiguration` `USE_CUSTOM_SHELF_PATH` and `CUSTOM_SHELF_PATH`;
  - `src-tauri/src/commands.rs`: retention on open and daily, next to `prune_snapshots`;
  - `src-tauri/src/lib.rs`, `NOTICE`.

**Interfaces:**
- Produces:
  - `ShelvedList { name, description, date_ms, recycled, to_delete, deleted, patch: PathBuf, binaries: Vec<ShelvedBinary> }`;
  - `pub fn list(data) -> Vec<ShelvedList>` (newest first);
  - `create(data, repo, description, patch: &Patch, binaries, system: bool) -> ShelvedList`;
  - `load_patch(data, name) -> Patch`;
  - `recycle`, `save_remaining_and_recycle_applied`, `delete`, `restore(date)`, `rename(name, description)`;
  - `cleanup(data, now)`;
  - `import(data, repo) -> ImportResult { added, skipped }`: read-only on `.idea`, de-duplicated by `(description, date, sha256 of shelved.patch)` and by an `imported.json` record, so a list deleted in rama is not imported again.
- The XML is written with `quick_xml::Writer` in the exact attribute order and spacing of the fixtures. Names follow `suggestFileName`: cut at 90 characters, collisions as `Name`, `Name1`, …

- [ ] **Step 1: Write the failing tests:**
  - `reads_every_intellij_shelf_fixture`;
  - `writes_xml_byte_identical_to_the_fixtures`;
  - `names_sanitise_cut_and_number`;
  - `binaries_are_copied_as_name_n_ext`;
  - `recycle_and_partial_recycle_follow_241` (the partial one keeps the base);
  - `delete_then_delete_erases`;
  - `restore_sets_the_date`;
  - `cleanup_erases_deleted_older_than_7_days_and_turns_system_recycled_into_deleted`;
  - `import_is_read_only_and_repeatable`: `.idea` byte-identical before and after, a second import adds nothing, and a list deleted in rama does not come back;
  - `import_reads_a_custom_shelf_path`;
  - `round_trip_with_binary_rename_delete_no_newline_crlf_non_utf8` (Review Focus 2).
- [ ] **Step 2: Check that they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Check that they pass.**
- [ ] **Step 5: Commit.** `git commit -m "feat: shelf store in IntelliJ's format and a repeatable import from .idea"`

### Task 9: Shelving

Research: `research-stash-shelf-patches.md` §4. Decision 8.

**Files:**
- Modify:
  - `src-tauri/src/vcs_commands.rs`:
    - `shelf_shelve(entries, description, rollback: bool, expected_repo)`: flush and locks, a snapshot, `shelf::create`, then roll back the shelved hunks or files with `hunks::rollback_lines` and `rollback::rollback_paths`;
    - `shelf_shelve_silently(lists)`: one shelf per changelist, named after it;
  - `src/commit/ChangesPanel.svelte`: popup item `Shelve Changes...`, and the toolbar's `Shelve Silently` (⇧⌘H) last;
  - `src/lib/keymap.ts`: `ChangesView.ShelveSilently` ⇧⌘H;
  - the Git menu `Uncommitted Changes` group: `Shelve Changes...`, `Show Shelf`;
  - `src/shelf/flow.ts` (create).
- Test: `src-tauri/src/vcs_commands.rs`, `src/shelf/flow.test.ts`, `src/commit/LocalChangesView.test.ts`.

- [ ] **Step 1: Write the failing tests:**
  - `shelve_changes_opens_the_dialog_titled_shelve_changes_and_shelves_the_included_part`, with partial lines kept on disk.
  - `only_directories_says_they_cannot_be_shelved` (V:489).
  - `an_emptied_changelist_is_deleted`.
  - `shelve_silently_makes_one_shelf_per_list_and_rolls_back`.
  - `save_to_shelf_does_not_roll_back` (keymap only).
  - `the_shelf_tab_opens_with_the_new_list_selected`.
  - `a_failed_shelve_rolls_nothing_back` (the snapshot exists).
  - `a_repository_switch_during_shelve_is_refused_before_writing` (Review Focus 5).
- [ ] **Step 2: Check that they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Check that they pass.**
- [ ] **Step 5: Commit.** `git commit -m "feat: Shelve Changes and Shelve Silently"`

### Task 10: The Shelf tab and unshelving

Research: `research-stash-shelf-patches.md` §1 (Shelf tab), §5, §6; `research-part-3-patches-241.md` §6.5.

**Files:**
- Create: `src/shelf/ShelfTab.svelte`, on the `SavedPatchesView` layout. It has:
  - a toolbar: `Unshelve Silently` ⌥⌘U, `Show Diff`, `View Options` › `Group By` and `Already Unshelved`, Expand All, Collapse All;
  - the context menu in the order of §1;
  - inline `Rename...` (F2, ⇧F6);
  - `Delete` with no question, plus the `Shelf Deletion` notification with `Undo`;
  - `Recently Deleted` and `Restore`;
  - `Import Patches…`;
  - `Import from .idea` (decision 5 of the overview).
- Modify:
  - `src/shelf/flow.ts`: `unshelve(lists, files?, removeApplied)`, which applies through Task 2 with the stored base and creates the shelf's changelist (V:536 chooser for several lists, none with changelists off);
  - `src/patch/ApplyPatchDialog.svelte`: `unshelve` mode, with the `Remove successfully applied files from the shelf` checkbox (repo setting `remove_strategy`, default false);
  - `src/App.svelte`: Shelf visible while there are shelves;
  - `src/lib/keymap.ts`: ⌥⌘U, ⇧⌘U, F2/⇧F6 in the tab.
- Test: `src/shelf/ShelfTab.test.ts`, `src/shelf/flow.test.ts`, Rust tests for the unshelve command.

- [ ] **Step 1: Write the failing tests:**
  - `the_shelf_tab_lists_newest_first_with_count_and_pretty_date`.
  - `unshelve_with_one_list_opens_unshelve_changes`.
  - `unshelve_with_several_lists_asks_the_changelist_only_with_changelists_on`.
  - `remove_applied_recycles_or_splits_as_241`.
  - `unshelve_uses_the_stored_base_for_conflicts`.
  - `devin_writing_during_unshelve_is_not_overwritten` (Review Focus 1).
  - `delete_then_undo_restores_the_list`.
  - `rename_changes_the_description_only`.
  - `already_unshelved_shows_recycled_lists_grey`.
  - `import_patches_finds_files_and_asks`.
  - `show_shelf_toggles`.
- [ ] **Step 2: Check that they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Check that they pass.**
- [ ] **Step 5: Commit.** `git commit -m "feat: the Shelf tab and unshelving"`

### Task 11: `Clean working tree using: Shelve`

Research: `research-stash-shelf-patches.md` §8, `research-part-3-rama.md` §3. Overview decisions 9 and 10.

**Files:**
- Modify:
  - `src-tauri/src/preserve.rs`: a `Saver` with `Stash` (today's) and `Shelve`:
    - `Shelve`: one system shelf per changelist named `<message> [<list>]` with `to_delete`, then `git restore --staged --worktree --source=HEAD -- <paths>` (or `reset --hard` without HEAD);
    - restore into the original list;
    - `Saved` names its kind and identity;
    - `Pending` records from earlier builds (stash sha) keep restoring;
    - the index-only snapshot stays.
  - `src-tauri/src/settings.rs`: `clean_tree: Stash | Shelve` per repository, default `Shelve`;
  - `src/settings/SettingsView.svelte`: the radio is live (GB:707, 1385-1386);
  - `update.rs`, `rebase.rs`, `merge.rs`, `branches.rs`: pass the saver, keeping their signatures where possible;
  - `src/update/flow.ts`, `src/branches/actions.ts`, `src/operation/operation.ts`:
    - the 241 texts G:362-365, 398-401, 316, 291, 297;
    - `View saved changes…`, which opens the Shelf tab with the list selected, or the Stash tab (Unstash dialog when there is no tab);
    - `Restore Changes` stays only for pending records saved by earlier builds;
  - `docs/superpowers/specs/2026-10-31-intellij-git-client-design.md`: phase 2 decisions 1, 2, 4 and 5, and phase 3 decision 1, marked overridden by phase 4 decisions 9 and 10.
- Test: `src-tauri/src/preserve.rs`, `update.rs`, `rebase.rs`, `src/update/flow.test.ts`, `src/branches/actions.test.ts`.

- [ ] **Step 1: Write the failing tests:**
  - `update_with_shelve_saves_one_shelf_per_list_and_restores_into_it`.
  - `a_stopped_update_leaves_the_shelf_and_offers_view_saved_changes` (Review Focus 4, which opens the Shelf tab with that list selected).
  - `a_pending_stash_from_an_earlier_build_still_restores`.
  - `rebase_with_shelve_says_local_changes_were_shelved_before_rebase_with_view_shelf`.
  - `smart_checkout_with_shelve`.
  - `index_only_content_is_kept_in_a_snapshot`.
  - `the_radio_saves_per_repository_and_defaults_to_shelve`.
  - `stash_policy_keeps_todays_behaviour` (the existing tests stay green).
- [ ] **Step 2: Check that they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Check that they pass.**
- [ ] **Step 5: Commit.** `git commit -m "feat: Clean working tree using Shelve, the IntelliJ default"`

### Task 12: Matrix, report and full verification

**Files:**
- Modify: `docs/parity-matrix.md`. Every row this part touched gets its result and its test:
  - the stash, shelf and patch actions;
  - `ChangesView.UnshelveSilently`;
  - `Git.Stage.Stash.Files`;
  - `Clean working tree using`.

  The rows of decisions 7 and 10 get their reason.
- Modify: `docs/reports/phase-4-validation.md`, with a Part 3 section: decisions, rulings with costs, tests by feature, what could not be checked, and the manual checklist:
  - a patch from IntelliJ applied in rama, and the other way;
  - the clipboard flows with a real clipboard;
  - the native choosers;
  - a real `.idea/shelf` import with a tab and an emoji;
  - Update with Shelve on a real conflict;
  - the merge window in contents mode;
  - unstash with Reinstate index.
- Modify: `NOTICE` (ported classes and fixtures, checked complete), `docs/reports/phase-4-reference-change.md` (status line).

- [ ] **Step 1:** Each `done` cites its test.
- [ ] **Step 2: Full verification, one suite at a time:** tsc, full vitest, `cargo test --lib`, clippy `-D warnings`, python. The report writes the exact counts.
- [ ] **Step 3: Commit.** `git commit -m "docs: phase 4 part 3 results in the matrix and the report"`
