# Phase 4, part 6: signing, worktrees and submodules, and the phase close. Implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** the last pieces of IntelliJ 2024.1's Git support, then the close of phase 4:
- commit signing as 241 has it:
  - `Configure GPG Key…` with the key list;
  - the `GPG Signature` log column;
  - the signature in the commit details;
- a linked worktree or a submodule opened as a repository, with rama noticing every change made to it from outside;
- `git submodule update --recursive` after Update Project;
- submodule diffs with 241's contents and titles;
- the close of the phase: the matrix, `NOTICE`, the reference-change report and the phase summary. The consumption measurement is a separate step that runs only when the user says so.

**Architecture:**
- **Worktrees and submodules.** The watcher learns the repository's real git folder and its common folder from git (`rev-parse --absolute-git-dir` and `--git-common-dir`) instead of assuming `<root>/.git`. That closes part 1's parked P7.
- **Signing.** It moves to 241's model: `commit.gpgSign` and `user.signingkey` in the repository's git config, which git applies by itself. rama's app-wide `Sign commits` setting and its `-S` go away. A new `src-tauri/src/gpg.rs` finds `gpg`, lists the secret keys, and reads and writes the two config values.
- **Signatures.** They are read lazily, only for the visible rows and the selected commits, with 241's `git log --no-walk=unsorted --format=%H%n%G?%n%GS%n%GF --stdin`. They never go into the all-commits log format.
- **Submodules.**
  - A new `src-tauri/src/submodules.rs` reads `git submodule status --recursive` and decides the update.
  - The diff code shows a gitlink as its commit hash, with 241's titles.

**Tech Stack:** Tauri 2.12, Rust 1.99.1, Svelte 5, Vitest 5. No new dependency.

**Spec:** `docs/superpowers/specs/2026-10-31-intellij-git-client-design.md` and the phase 4 overview `docs/superpowers/plans/2026-11-03-phase-4-overview.md`. The overview's Global Constraints and its decisions 19 and 20 are part of this plan.

**Research** (not in the repository). The implementer reads it before each task:
- `.superpowers/phase-4/research-part-6-241.md` (P§n): the 241 behaviour, texts and commands, and git 2.54 checks;
- `.superpowers/phase-4/research-part-6-rama.md` (M§n): rama today, with lines and the observations at its end.

**Reference:**
- Tag `idea/241.14494.240`.
- Bundles: `GitB:` GitBundle, `DiffB:` DiffBundle, `VLB:` VcsLogBundle.
- Copy literal texts from the cited line, not from this plan.

**Branch:** `phase-4`, after part 5 (`996b302`).

**Commands.** These rules are binding, because the MacBook froze under parallel builds:
- one heavy command at a time, never in the background, never in a second copy of the project;
- clear old `*.rcgu.o` and incremental caches under `src-tauri/target/debug` between tasks when the disk is under 50 GiB free.

The commands:
- TS: `PATH=$HOME/.cache/rama-toolchain/node-v22.23.4-darwin-arm64/bin:$PATH nice -n 15 pnpm exec vitest run --maxWorkers=2 <files>` and `nice -n 15 pnpm exec tsc --noEmit -p .`
- Rust: `PATH=$HOME/.cargo/bin:$PATH CARGO_BUILD_JOBS=3 nice -n 15 cargo test --manifest-path src-tauri/Cargo.toml --lib <filter> -- --test-threads=3`
- Integration: the same with `--no-fail-fast --test '*'`, in every final verification.
- Python: `python3 -I -m unittest discover -s scripts`

## Global Constraints

The overview's Global Constraints, including phase 3's. The ones this part leans on most:
- **Changing the repository.** Every command that changes files, the index or `.git/config` calls `flush_diff_window` first and takes `RepoLocks`. The guard tests and `scripts/smoke_ipc.py` follow the new commands.
- **Git.** Real `git` with arguments and no shell. Hashes go through `diff::check_hash`, paths through `--literal-pathspecs` and `--`, and every `git log` and `git show` gets `-c log.showSignature=false`.
- **gpg.** It runs only by an absolute path that rama resolved itself, with no shell. A passphrase never passes through rama: gpg's own pinentry asks for it.
- **No lost work.** A submodule update never overwrites a submodule's local changes. git refuses, and rama shows git's error.
- **Untrusted text.** Key user IDs, signer names, submodule paths and hashes are shown as text, with no `{@html}`.
- **Tests stay offline.**
  - Worktrees come from `git worktree add` in throwaway repositories.
  - Submodules come from local bare repositories, with `-c protocol.file.allow=always` used only in tests.
  - Signatures use `TempRepo::sign_commits` (SSH, no gpg needed) and a fake gpg script for the key list.
  - Nothing touches the user's real gpg keyring or git config.
- **Writing.** Everything written into the repository is in English. Each commit ends with the `Co-Authored-By` line of the model that wrote it.

## Decisions of this part

They add to the overview's decisions. The user approves them with this plan, and each has its cost if wrong.

1. **Signing moves to 241's model.**
   - `Configure GPG Key…` writes `commit.gpgSign` and `user.signingkey` to the repository's local git config, as 241 does (P§1.5), and git signs by itself.
   - rama's app-wide `Sign commits` checkbox and `Signing key` field (`settings.rs:60-61`, `SettingsView.svelte:303-322`) and the `-S<key>` that `commit.rs:432` adds are removed.
   - The user's IntelliJ settings and global git config show no signing (P§1.8).
   - Cost: anyone who relied on rama's app-wide toggle has to set it per repository.
2. **Where `Configure GPG Key…` goes.** It goes in the settings, in the existing commit section, where 241 has it (`Settings > Version Control > Git > Commit`, P§1.1), with 241's status label. The overview said "the commit dialog or panel gear", but that place does not exist in 241.
   - rama has one repository per window, so only the single-root dialog is built (P§1.2). Its title is `Configure GPG Key`, without the trailing space that 241's empty `mention` leaves.
   - Cost: no multi-root table.
3. **Finding gpg.** gpg is found once, as git is found:
   - `gpg.program` from the repository's git config, when set;
   - else `gpg` on `PATH`;
   - else `/opt/homebrew/bin/gpg`, `/usr/local/bin/gpg` and `/usr/local/MacGPG2/bin/gpg`.

   Signing also needs git's own gpg call to find gpg when rama is opened from Finder, whose PATH is `/usr/bin:/bin:/usr/sbin:/sbin` (M§6). So every git rama starts gets those three folders appended to its `PATH`, when they are not already in it. 241 inherits the login shell's environment instead.

   Cost: a gpg installed elsewhere needs `gpg.program`.
4. **SSH signing keeps working, and 241's trap is closed.**
   - With `gpg.format=ssh`, the settings row and dialog show the configured key as it is, as 241 does.
   - Unlike 241, the dialog refuses to write a GPG key ID while `gpg.format` is `ssh`, because that breaks every commit (P§1.7). The refusal text is rama's own: `Commits are signed with SSH (gpg.format=ssh). Change gpg.format to use a GPG key.`
   - Unchecking still writes `commit.gpgSign false`.
   - Cost: one rama text.
5. **No `See GPG setup guide` link.** It opens a JetBrains help topic (P§1.2, P§1.6), and the app uses no JetBrains services.
   - A commit that fails with `gpg failed to sign the data` keeps rama's existing `Could not sign the commit: …` error, with git's output.
   - Cost: no guide link.
6. **The signature icons are rama's own drawings**, as for every icon so far: a green check badge for verified, and an amber badge for the other signed states. 241's texts and states are kept.
   - Cost: a visual difference.
7. **The submodule update fits one repository per window.**
   - 241 runs `GitSubmoduleUpdater` only when the submodule and its parent are both registered roots (P§4.2), which never happens in rama.
   - rama decides from the open repository itself. After an Update Project that ends `Success` or `NothingToUpdate`, it runs `git submodule update --recursive` once in the repository when `git submodule status --recursive` shows an initialized submodule (status ` ` or `+`) whose HEAD is detached.
   - It runs inside the same task and the same lock, after the repository's own changes are restored, and its output goes to the Console.
   - rama does not save the submodules' local changes first, as 241 does. git refuses to overwrite them, so nothing is lost, and rama shows the error as a notification: title `Submodule update failed` (rama's own), with git's output in the body. 241 only logs it and shows a misleading `Local changes were not restored`.
   - Cost: a submodule with local changes is not updated until they are committed or stashed in it.
8. **Every gitlink is shown as a submodule.** 241 does so only for registered roots (P§4.3); rama does it for every mode-160000 entry.
   - **Contents:** the commit hash as text, as 241 does, instead of today's `Subproject commit <hash>`.
   - **The local side:** the submodule's checked-out HEAD, as 241 does, instead of today's index commit (M§5).
   - **Titles:** `{0} (Submodule)` (`GitB:1382`), with the side titles of P§4.4.
   - Cost: none.
9. **The watcher watches the git folder and the common folder recursively**, as 241 does (P§3.3), when they are outside the work tree. The `objects/` and `*.lock` filters stay. Cost: none expected; the consumption measurement checks it.
10. **The consumption measurement waits for the user.**
    - It needs a release build (`pnpm tauri build`, with LTO) and five runs of `scripts/measure.py` on an idle machine, which is the heaviest load of the phase.
    - It runs as the last step, only when the user says so, and with the phase 3 gate: input idle for at least 180 s, a 1-minute load under 15, and the screen not locked.
    - Until then the report says it was not measured.
    - Cost: the phase summary waits for that number.

## Review Focus

1. **Worktree and submodule layouts:**
   - a linked worktree whose `gitdir:` is relative or absolute, or contains spaces;
   - a submodule inside a linked worktree's superproject;
   - a worktree whose main repository was moved, which leaves a broken `gitdir`.

   rama opens the first three and sees index, branch and stash changes made from a terminal. The broken one fails with git's error and no panic. Tests in Task 1.
2. **gpg in every state:**
   - not installed, failing, or listing no key;
   - several keys, including disabled keys and keys without signing capability;
   - a user ID with `\x3a` escapes or control characters;
   - `gpg.format=ssh`.

   The list, the texts and the writes follow P§1.2-1.7 and decision 4, and nothing hangs. Tests in Tasks 2 and 3.
3. **Signatures on a large history.**
   - The log of `git/git`, about 86,000 commits, scrolls as before.
   - Signatures load only while the column is shown, for the visible rows ±15, debounced, with the previous request cancelled.
   - The details load the signature for at most the selected commits.
   - An unknown `%G?` letter (for example `P`) shows nothing and logs nothing loud.

   Tests in Task 4.
4. **A submodule update that cannot run cleanly:**
   - the submodule has local changes;
   - it is not initialized;
   - it is nested;
   - its URL needs credentials;
   - its commit is missing from its remote.

   No local change is lost. git's error reaches the notification. askpass handles credentials, and the Update result is otherwise unchanged. Tests in Task 5.
5. **A gitlink whose submodule is not checked out, or whose commit is not in the parent revision.** The diff shows an empty side, with no crash and no misleading content, and with 241's titles. Tests in Task 6.

---

### Task 1: Linked worktrees and submodules, watched as repositories

Research: P§3, P§4.1, M§1. Decisions 9 and overview 19. This closes P7 (`docs/reports/phase-4-validation.md:459`).

**Files:**
- Modify:
  - `src-tauri/src/watch.rs`:
    - `WatchedPaths { root, git_dir, common_dir }`;
    - `pub fn start(paths: WatchedPaths, on_change) -> Result<Watcher, GitError>`, which adds recursive watches on `git_dir` and `common_dir` when they are outside `root`;
    - `relevant(path, &paths)` and `touches_stash(path, &paths)` match against the two folders, not against a literal `.git`. Stash changes are `refs/stash`, `logs/refs/stash` and `packed-refs` in the common folder.
  - `src-tauri/src/git.rs`: `pub fn git_dirs(repo: &Path) -> Result<(PathBuf, PathBuf), GitError>`, which returns `rev-parse --absolute-git-dir` and `--git-common-dir` resolved against `repo`. It does not use `--path-format=absolute`, because the fallback gits may be older.
  - `src-tauri/src/commands.rs`: `repo_open` and `resume_watching` build `WatchedPaths` from `git_dirs`.
  - `src-tauri/src/test_support.rs`:
    - `TempRepo::worktree(name) -> TempRepo`, from `git worktree add`;
    - `TempRepo::with_submodule(path) -> TempRepo`, a local bare repository added with `-c protocol.file.allow=always`;
    - `TempRepo::in_git_dir` follows `git rev-parse --git-dir`.
- Test: `src-tauri/src/watch.rs`, `src-tauri/src/git.rs`, `src-tauri/src/branches.rs`.

**Interfaces:**
- Produces: `git::git_dirs`, `WatchedPaths`, and the two test helpers. Tasks 5 and 6 use the helpers.

- [ ] **Step 1: Write the failing tests:**
  - `a_linked_worktree_sees_index_branch_and_stash_changes_from_outside`. Use a real watcher on a worktree: `git add`, `git commit`, `git checkout -b` and `git stash` from a "terminal" (`git -C`) each produce a change, and the stash one sets `stash`.
  - `a_submodule_sees_its_own_index_and_branch_changes`.
  - `a_submodule_inside_a_worktree_is_watched_on_its_own_git_dir`.
  - `objects_and_locks_in_the_common_dir_are_ignored`.
  - `a_worktree_with_a_broken_gitdir_fails_to_open_with_gits_error` (Review Focus 1).
  - `a_normal_clone_still_registers_a_single_watch`, as before.
  - `checking_out_a_branch_used_by_another_worktree_shows_could_not_checkout_with_gits_error` (`GitB:1156`, P§3.4). This pins rama's existing checkout error against 241's.
- [ ] **Step 2: Check that they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Check that they pass**, along with every existing `watch.rs` test.
- [ ] **Step 5: Commit.** `git commit -m "fix: linked worktrees and submodules are watched on their real git folders"`

### Task 2: gpg, the signing config, and commit signing through git

Research: P§1.4-1.7, M§2, M§6. Decisions 1, 3, 4 and 5.

**Files:**
- Create: `src-tauri/src/gpg.rs`:
  - `pub fn program(repo: &Path) -> Option<PathBuf>`, decision 3's lookup;
  - `pub fn secret_keys(program: &Path) -> Result<Vec<GpgKey { id, user_id: Option<String> }>, GitError>`, which runs `<program> --list-secret-keys --with-colons --fixed-list-mode --batch --no-tty` with no stdin;
    - it parses as P§1.4: `sec` lines without `D` and with `s` or `S`, and the first `uid` after each, unescaped;
    - like 241, it ignores the exit code but keeps stdout;
    - a program that cannot start is an error with its message;
  - `pub fn config(repo: &Path) -> Result<SigningConfig { sign: bool, key: Option<String>, ssh: bool }, GitError>`, which reads `commit.gpgSign`, `user.signingkey` and `gpg.format` with `git config --null --get` (exit 1 means unset), using git's boolean rules;
  - `pub fn write(repo: &Path, key: Option<&str>) -> Result<(), GitError>`, as P§1.5:
    - with a key, `git config commit.gpgSign true` and then `git config user.signingkey <id>`;
    - without, `git config commit.gpgSign false` only;
    - local scope, with exit 1 ignored;
    - with `ssh` and a key, it refuses with decision 4's text and writes nothing.
- Modify:
  - `src-tauri/src/git.rs`: `git::command` appends decision 3's folders to the child's `PATH` when they are missing.
  - `src-tauri/src/commit.rs`: remove `sign`, `signing_key` and the `-S` (`commit.rs:432-433`). `CommitError::Gpg` stays for `gpg failed to sign the data`.
  - `src-tauri/src/settings.rs`, `src-tauri/src/commands.rs` (`with_signature`) and `src/lib/api.ts`: remove `sign_commits` and `signing_key`. An old settings file that has them still loads, and they are dropped on save.
  - Commands, in a new `src-tauri/src/gpg_commands.rs` or in `vcs_commands.rs`:
    - `gpg_config()` and `gpg_keys()`: read-only, in `READS`, and `NOT_RUN` in smoke because they need gpg;
    - `gpg_config_set(key: Option<String>)`: `flush_diff_window` first, a `TaskKind::Write` task, and listed in the guards.
- Test: `src-tauri/src/gpg.rs`, `src-tauri/src/git.rs`, `src-tauri/src/commit.rs`, `src-tauri/src/settings.rs`.

**Interfaces:**
- Produces:
  - `GpgKey`, `SigningConfig`;
  - the TS functions `gpgConfig()`, `gpgKeys()`, `gpgConfigSet(key | null)`;
  - the `PATH` rule. Task 3 uses all of these.

- [ ] **Step 1: Write the failing tests:**
  - `the_key_list_keeps_signing_keys_and_their_first_uid`. Use a fake gpg script that prints colon output with:
    - a signing key;
    - a disabled key;
    - an encrypt-only key;
    - two `uid`s;
    - a `\x3a` escape.
  - `a_failing_gpg_gives_an_empty_list_and_a_missing_one_an_error` (Review Focus 2).
  - `gpg_program_from_the_config_wins_and_the_fallbacks_are_tried_in_order`.
  - `writing_a_key_sets_gpgsign_and_signingkey_locally_and_unchecking_writes_only_false`. From a linked worktree, it lands in the main config, as P§1.5 says.
  - `with_gpg_format_ssh_writing_a_gpg_key_is_refused_and_nothing_changes` (decision 4).
  - `git_children_get_the_gpg_folders_on_their_path`.
  - `a_commit_is_signed_by_git_from_the_config_without_dash_s`: with `sign_commits` (SSH) the commit is signed, and the argv has no `-S`.
  - `the_gpg_failure_still_reads_could_not_sign_the_commit` (the existing `gpg_error` test, kept).
  - `an_old_settings_file_with_sign_commits_still_loads`.
- [ ] **Step 2: Check that they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Check that they pass.**
- [ ] **Step 5: Commit.** `git commit -m "feat: gpg keys and the signing config are read and written as 241 does, and git signs by itself"`

### Task 3: `Configure GPG Key…` in the settings

Research: P§1.1-1.3. Decisions 2 and 4.

**Files:**
- Create:
  - `src/settings/GpgKeyRow.svelte`: the button `Configure GPG Key…` (`GitB:727`), enabled with an open repository, and the status label of P§1.1:
    - `Commits signing with GPG key is not configured` (`GitB:723`);
    - `Commits are signed with GPG key: {0}`, with `<uid> (<id>)` or `<id>` (`GitB:724`);
    - empty while loading;
    - an error label when the config cannot be read.

    It refreshes when the settings open and on `repo-changed`.
  - `src/settings/GpgKeyDialog.svelte`, P§1.2:
    - the title `Configure GPG Key`;
    - the checkbox `Sign commits with GPG key:` (`GitB:732`), with focus;
    - the combo: the key ID, then the user ID in grey;
    - the loading spinner;
    - `Cannot find suitable private key` (`GitB:737`);
    - `Synced with gitconfig` (`GitB:729`) bottom left;
    - `OK` and `Cancel`.

    It follows the enable rules of P§1.2. Errors show in the dialog, and it stays open. A configured key that is not in the list is still shown and selected.
- Modify: `src/settings/SettingsView.svelte`. Remove the `Sign commits` and `Signing key` fields, and add `GpgKeyRow` as the last row of the existing commit section. This is not part of `Save`: the dialog's OK writes at once, through `gpgConfigSet`, like 241's dialog.
- Test: `src/settings/GpgKeyRow.test.ts`, `src/settings/GpgKeyDialog.test.ts`, `src/settings/SettingsView.test.ts`.

**Interfaces:**
- Consumes: Task 2.

- [ ] **Step 1: Write the failing tests:**
  - `the_row_shows_241s_status_for_no_signing_a_known_key_and_an_unknown_key`.
  - `the_dialog_lists_keys_with_their_user_id_and_selects_the_configured_one`.
  - `an_empty_key_list_says_cannot_find_suitable_private_key`.
  - `ok_writes_the_selected_key_and_unchecked_writes_no_key`.
  - `with_ssh_the_dialog_shows_the_key_and_ok_with_a_gpg_key_shows_ramas_refusal` (decision 4).
  - `a_user_id_with_markup_is_text`.
  - `the_old_sign_commits_fields_are_gone`.
- [ ] **Step 2: Check that they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Check that they pass.**
- [ ] **Step 5: Commit.** `git commit -m "feat: Configure GPG Key… in the settings"`

### Task 4: The `GPG Signature` column and the signature in the details

Research: P§2, M§2 (log, columns, details). Decision 6.

**Files:**
- Create: `src-tauri/src/log/signatures.rs`:
  - `pub fn signatures(repo: &Path, hashes: &[String]) -> Result<Vec<Signature { hash, status: SignatureStatus, signer, fingerprint }>, GitError>`;
    - it runs `git -c log.showSignature=false log --no-walk=unsorted --format=%H%n%G?%n%GS%n%GF --stdin --`, with the hashes validated and written to stdin, and parses groups of 4 lines;
    - `%G?` maps as P§2.2. `N` and unknown letters give no entry, and an unknown letter only logs quietly;
  - an app-wide cache of the last 1000 entries, expiring 60 s after last access, as P§2.2;
  - the command `log_signatures(hashes)`: a cancellable read task, where a new request cancels the previous one.
- Modify:
  - `src/log/options.ts`: `LogColumn` gains `"signature"`, hidden by default.
  - `src/log/ViewOptions.svelte` and `src/log/HistoryTab.svelte`: the `Columns` menu item `GPG Signature` (`GitB:1084`), after the existing columns.
  - `src/log/draw.ts` and `src/log/LogView.svelte`: a fixed-width icon cell. While the column is shown, it requests the visible rows ±15, debounced 300 ms, and re-requests on scroll and on model changes. The tooltip uses the texts of P§2.3, and an unsigned or not-yet-loaded row has the empty icon and the tooltip `No GPG signature` (`GitB:1086`).
  - `src/log/DetailsPanel.svelte`: for the selected commits (at most `MAX_COMMITS`), after the committer line:
    - the icon and the status text;
    - for a verified signature, `Signature fingerprint:` and `Signed by:` with their values (`GitB:1099-1100`), in the context-help colour;
    - nothing for `N`.
  - `src/lib/icons/`: `signatureVerified.svg` and `signatureSigned.svg`, rama's own drawings.
- Test: `src-tauri/src/log/signatures.rs`, `src/log/LogView.test.ts`, `src/log/DetailsPanel.test.ts`, `src/log/ViewOptions.test.ts`, `src/log/HistoryTab.test.ts`.

**Interfaces:**
- Produces: `SignatureStatus`; the TS `logSignatures(hashes)`.

- [ ] **Step 1: Write the failing tests:**
  - `ssh_signed_commits_read_as_241_with_and_without_allowed_signers`. Use `sign_commits`: without the allowed-signers file it is `N`; with it, `G` with the signer and fingerprint; with an unknown signer, `U` (P§1.7).
  - `unsigned_and_unknown_letters_give_no_entry`.
  - `the_column_is_hidden_by_default_and_toggles_in_both_columns_menus`.
  - `signatures_load_only_while_shown_for_the_visible_rows_plus_fifteen_and_a_new_request_cancels_the_old` (Review Focus 3).
  - `tooltips_and_details_follow_241s_texts_per_state`.
  - `the_all_commits_log_format_has_no_signature_placeholder`: a guard on `LOG_FORMAT`.
- [ ] **Step 2: Check that they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Check that they pass.**
- [ ] **Step 5: Commit.** `git commit -m "feat: the GPG Signature column and the signature in the commit details"`

### Task 5: The submodule update after Update Project

Research: P§4.2, M§4. Decision 7.

**Files:**
- Create: `src-tauri/src/submodules.rs`:
  - `pub fn detached(ctx, repo) -> Result<Vec<String>, GitError>`. It parses `git submodule status --recursive` and returns the initialized ones (` ` or `+`) whose HEAD is detached, checked with `git -C <path> symbolic-ref -q HEAD`, where exit 1 means detached.
  - `pub fn update(ctx, repo) -> Result<(), GitError>`. It runs `git submodule update --recursive` through `ctx.git_loud`, so it reaches the Console and askpass applies.
- Modify:
  - `src-tauri/src/update.rs`: after `finish` on `Success` and `NothingToUpdate`, still under the write lock, if `detached` is not empty, run `update`. A failure adds `submodule_error: Option<String>` to `UpdateResult`; the outcome stays as it was.
  - `src/update/flow.ts`: a `submodule_error` gives the error notification `Submodule update failed`, with git's output in the body.
  - `src/lib/api.ts`.
- Test: `src-tauri/src/submodules.rs`, `src-tauri/src/update.rs`, `src/update/flow.test.ts`.

**Interfaces:**
- Consumes: Task 1's `TempRepo::with_submodule`.

- [ ] **Step 1: Write the failing tests:**
  - `update_project_updates_a_detached_submodule_to_the_new_recorded_commit`. Use a bare superproject remote whose new commit moves the gitlink. After Update the submodule is at the new commit, and the argv is `submodule update --recursive`.
  - `a_submodule_on_a_branch_or_not_initialized_is_left_alone`.
  - `nothing_to_update_still_updates_a_detached_submodule`.
  - `a_submodule_with_local_changes_keeps_them_and_the_error_reaches_the_notification` (Review Focus 4, no lost work).
  - `a_nested_submodule_is_updated_by_the_recursive_call`.
  - `a_failed_submodule_update_does_not_change_the_update_outcome`.
- [ ] **Step 2: Check that they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Check that they pass.**
- [ ] **Step 5: Commit.** `git commit -m "feat: Update Project updates detached submodules afterwards"`

### Task 6: Submodule diffs

Research: P§4.3-4.4, M§5. Decision 8.

**Files:**
- Modify:
  - `src-tauri/src/diff.rs`: `gitlink_side(hash)` gives the 40-hex hash as text. A missing gitlink gives an empty side.
  - `src-tauri/src/worktree_diff.rs`: the local side of a gitlink is the submodule's checked-out HEAD, from `git -C <path> rev-parse HEAD`, validated. A submodule that is not checked out gives an empty side.
  - `src-tauri/src/compare.rs`.
  - `src/diff/` and `src/compare/actions.ts`: a change whose path is a gitlink gets:
    - the window title `{0} (Submodule)` (`GitB:1382`), where `{0}` is the `<name> (<parent folder>)` title rama already builds;
    - the side titles of P§4.4: in Local Changes, the parent's full HEAD hash and `Your version` (`DiffB:91`); in the log, the parent commit's full hash and the commit's full hash.
- Test: `src-tauri/src/diff.rs`, `src-tauri/src/worktree_diff.rs`, `src-tauri/src/compare.rs`, `src/diff/*.test.ts`, `src/compare/actions.test.ts`.

**Interfaces:**
- Consumes: Task 1's `TempRepo::with_submodule`.

- [ ] **Step 1: Write the failing tests:**
  - `a_real_submodule_in_local_changes_shows_its_checked_out_head_on_the_local_side`.
  - `a_submodule_in_a_commit_shows_parent_and_commit_hashes`.
  - `the_diff_title_ends_with_submodule_and_the_sides_are_full_hashes`.
  - `a_submodule_that_is_not_checked_out_gives_an_empty_side` (Review Focus 5).
  - The existing `gitlink_is_not_absent` tests are updated to the new content, not removed.
- [ ] **Step 2: Check that they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Check that they pass.**
- [ ] **Step 5: Commit.** `git commit -m "feat: submodule diffs show commit hashes with 241's titles"`

### Task 7: Part 6 report and the phase close

**Files:**
- Modify:
  - `docs/parity-matrix.md`:
    - section 2 line 71 is renamed per decision 19, and line 72 per decision 20; each gets its result;
    - section 3 line 112 (GPG) gets its result;
    - new rows for `Configure GPG Key…`, the `GPG Signature` column and the submodule diff;
    - the part 6 counts paragraph, and the dates sentence at line 9.

    Every `done` row cites a test that exists; check that with grep.
  - `docs/reports/phase-4-validation.md`:
    - a Part 6 section in the shape of the others (sections 1-7 and a Conclusion), with the ledger's rulings and their costs, the differences, what could not be checked and the manual checklist;
    - P7 marked closed;
    - a `## Phase 4 summary` with every part's result, the open items across parts, and a consumption section. Until Task 8 runs, that section says it was not measured and why (decision 10).
  - `docs/reports/phase-4-reference-change.md`: the status line says "after part 6", and names what part 6 closed.
  - `NOTICE`:
    - the stale header tag (line 11) becomes 241;
    - blocks for `config/gpg/*`, the signature column, `GitSubmoduleUpdater`, the submodule diff, and the worktree/submodule repository files.

**The manual checklist includes:**
- a real GPG key: list, select, a signed commit from rama opened from Finder, and the column and details on it;
- a real SSH-signed repository with an allowed-signers file;
- a linked worktree and a submodule opened in rama, with terminal changes showing up;
- Update Project on a repository with a submodule in detached HEAD, including one with local changes;
- a submodule diff from Local Changes and from the log;
- scrolling the `git/git` log with the `GPG Signature` column shown.

- [ ] **Step 1:** Each `done` cites its test, and the test exists.
- [ ] **Step 2: Full verification, one suite at a time:** tsc, full vitest, `cargo test --lib`, `cargo test --no-fail-fast --test '*'`, clippy `--all-targets -D warnings`, python. The report writes the exact counts.
- [ ] **Step 3: Commit.** `git commit -m "docs: phase 4 part 6 results, and the phase 4 close"`

### Task 8: Consumption measurement (only with the user's go-ahead)

Decision 10. It is not dispatched with the others: the controller asks the user first.

- [ ] **Step 1:** Build the release app once: `nice -n 15 pnpm tauri build --bundles app`.
- [ ] **Step 2:** Prepare `~/rama-medida/git` with the phase 3 recipe (`docs/reports/phase-3-validation.md`, "To repeat the measurement").
- [ ] **Step 3:** Wait for the gate: input idle for at least 180 s (`ioreg -c IOHIDSystem`, `HIDIdleTime`), a 1-minute load under 15, and the screen not locked. Give up after 30 minutes and record why.
- [ ] **Step 4:** Run `python3 scripts/measure.py --app <binary> --repo ~/rama-medida/git --out docs/reports/phase-4-consumption.json --runs 5` twice: once with the `GPG Signature` column hidden and once with it shown.
- [ ] **Step 5:** Write the table into the phase summary, with phase 0's headers (Goal / Target / App / IntelliJ / Met), and commit with `docs: phase 4 consumption`.
