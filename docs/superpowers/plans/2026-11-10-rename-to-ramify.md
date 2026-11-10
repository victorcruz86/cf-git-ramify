# Rename to Ramify Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** the project, the app and every text of the repository are called Ramify instead of rama, and the user keeps their data, their GitHub token and their settings across the change.

**Architecture:** a mechanical rename of names, identifiers and texts; one new module, `migration.rs`, that copies the old data folder on the first start and copies each Keychain item the first time it is read; the documentation and `CLAUDE.md` follow.

**Tech Stack:** Rust (Tauri 2), Svelte 5 and TypeScript, Python scripts.

**Spec:** `docs/superpowers/specs/2026-10-31-intellij-git-client-design.md`. The user chose the name and the scope on 2026-11-10: "Ramify", "everything, with migration".

## Global Constraints

- Everything written into the repository is in English (`CLAUDE.md`).
- In prose and in what the user sees, `Ramify`; in identifiers, file names, the binary, prefixes and keys, `ramify`.
- New bundle identifier: `dev.ramify.app`. The old one, `dev.rama.app`, only appears in `migration.rs` and its tests, and in the history that records it.
- No work is lost: the old data folder and the old Keychain items are copied, never moved or deleted.
- Tokens only in the Keychain: the migration reads one item and writes another, with nothing on disk, console, error, argument, env or URL.
- Load rules: one build or test run at a time, `nice -n 15`, `CARGO_BUILD_JOBS=3`, `--test-threads=3`, `vitest --maxWorkers=2`.
- Work on a new branch `rename-ramify` from `main`. Merge and push only when the user asks.

## Decisions

1. **Name and casing.** `Ramify` in texts, titles and menus (`Quit Ramify`, the window title, the About name); `ramify` in the package, crate (`ramify`, `ramify_lib`), binary, temp and socket prefixes (`ramify-askpass-`, `ramify-commit-`, `.ramify-restore`…), menu ids, the `--ramify-editor` flag, the User-Agent and the localStorage keys (`ramify.annotation.options`…). Cost: none; nothing outside the app reads these names (git gets the editor and askpass through env on each call, nothing is written to git config).
2. **The data folder is copied once.** On start, before anything reads app data: if `~/Library/Application Support/dev.ramify.app` does not exist and `dev.rama.app` does, copy it whole to `dev.ramify.app.migrating`, then rename that to `dev.ramify.app`. The old folder stays. A failed copy leaves no `dev.ramify.app`, removes the half copy, leaves a `.migration-failed` mark, logs the error and starts with empty data; on the next start the folder that session created is set aside as `dev.ramify.app.failed-<n>` (never deleted) and the copy is tried again (final review, I1). Cost: the data is on disk twice until the user deletes the old folder; an old rama build opened afterwards writes to the old folder only.
3. **Keychain items are copied on first read.** The service becomes `ramify GitHub <server>`. When the new item is missing, the old `rama GitHub <server>` item is read and written under the new service; the old item stays. Cost: the token is in two Keychain items until the user deletes the old one in Keychain Access; macOS may ask once whether Ramify may read the item rama created.
4. **The WebView's storage is copied too** (changed by the final review, I2). WebKit keeps localStorage per bundle identifier, so a new identifier would reset every interface setting, Sign-off per repository among them. `main()` copies `~/Library/WebKit/dev.rama.app` whole to `dev.ramify.app` before Tauri creates the webview, with the same staged copy as the data folder, and the interface copies its three `rama.` keys to `ramify.` once, never over a key Ramify saved. Cost: WebKit's format is not read, so whether the copied storage is picked up is only known on the user's first real start; if it is not, every interface setting goes back to its default once.
5. **The leftovers of old builds are still cleaned.** Where rama cleans its own temp files by prefix, both prefixes (`rama-` and `ramify-`) are accepted. Cost: none.
6. **All docs say Ramify, history included.** Plans, reports, the parity matrix, `NOTICE` and `README.md` are rewritten with the new name, except where they quote the old identifier, the old Keychain service or a commit message. Commit messages already in git keep "rama". `README.md` is also brought up to date: it still describes phase 3 and IntelliJ 2023.2.3.
7. **The measuring folder `~/rama-medida` becomes `~/ramify-measure`** (moved with `mv`; it is a throwaway clone made by this project), and the docs that name it follow. The two Spanish keys of `docs/reports/phase-3-consumption.json` (`medido`, `motivo`) become `measured` and `reason`, and the fixture name `-archivo.txt` in the phase 1 plan becomes `-file.txt` if no test uses it.
8. **`CLAUDE.md`.** The title becomes `# Ramify`, it names the product and the casing rule, and the English rule is extended to JSON and configuration keys, fixture and test data names, and the names of folders the project creates outside the repository.
9. **A clean build.** The crate name changes every artifact, so `cargo clean` first (it frees about 10 GB of rama's build cache) and one rebuild at low priority.
10. **Not in this plan without the user's word:** renaming the GitHub repository `victorcruz86/cf-git-ide` to `ramify` (GitHub redirects the old URL; `origin` is updated), and renaming the local folder `cf-git-ide` (it moves the Claude project memory, which would be copied first). Each is asked for at the end.

## Review Focus

1. A first start with only `dev.rama.app`: the data is in `dev.ramify.app` and the old folder is untouched.
2. A first start with both folders: nothing is copied over the new one.
3. A copy that fails halfway: no `dev.ramify.app` is left and the next start retries.
4. A token saved by rama: Ramify reads it, the new item exists afterwards and the old one is still there; a missing old item is "no token", not an error.
5. A leftover `rama-` temp file in `.git` from an old build is still cleaned.

---

### Task 1: The mechanical rename

**Files:** `package.json`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, `src-tauri/src/**/*.rs`, `src/**/*.{ts,svelte}`, `scripts/*`, and their tests.

- [ ] **Step 1:** Create the branch `rename-ramify` from `main`.
- [ ] **Step 2:** Rename per decision 1: `productName` and the window title `Ramify`, identifier `dev.ramify.app`, package and crate names, every runtime string listed by `git grep -n -w rama -- src src-tauri/src scripts`, and the tests that assert them. Keep `rama` only where decisions 2, 3 and 5 need the old name.
- [ ] **Step 3:** Accept both prefixes where temp files are cleaned (decision 5), with a test that a `rama-` leftover is cleaned.
- [ ] **Step 4:** `cargo clean`, then `cargo check`, `pnpm tsc`, and the unit tests of what changed.
- [ ] **Step 5:** Commit `refactor: rename rama to Ramify`.

### Task 2: Migration of the data folder and the Keychain items

**Files:** Create `src-tauri/src/migration.rs`. Modify `src-tauri/src/lib.rs` (setup) and `src-tauri/src/hosting/accounts.rs` / `src-tauri/src/secrets.rs` (the read path).

**Interfaces:**
- `pub fn migrate_data_folder(support: &Path) -> Result<Migration, io::Error>` where `support` is `~/Library/Application Support` and `Migration` is `Copied`, `NotNeeded`.
- `pub fn read_token(store: &dyn SecretStore, server: &str, account: &str) -> Result<Option<Secret>, ApiError>`: new service first, then the old one, copying it when found.

- [ ] **Step 1:** Write the failing tests: `only_the_old_folder_is_copied_whole_and_kept`, `with_both_folders_nothing_is_copied`, `a_failed_copy_leaves_no_new_folder_and_retries_next_time` (Review Focus 1 to 3), and with `MemoryStore` `a_token_saved_by_rama_is_read_and_copied_and_the_old_item_stays`, `without_any_item_there_is_no_token` (Review Focus 4).
- [ ] **Step 2:** Run them and see them fail.
- [ ] **Step 3:** Implement; call `migrate_data_folder` in `setup` before anything reads app data, and log the result without paths of the user's repositories.
- [ ] **Step 4:** Run them and see them pass.
- [ ] **Step 5:** Commit `feat: Ramify copies rama's data and Keychain items on first use`.

### Task 3: Documentation and CLAUDE.md

**Files:** `CLAUDE.md`, `README.md`, `NOTICE`, `docs/**/*.md`, `docs/reports/phase-3-consumption.json`.

- [ ] **Step 1:** Replace the product name per decision 6, keeping the old identifier, the old Keychain service and quoted commit messages.
- [ ] **Step 2:** Update `README.md` to phase 4 and IntelliJ 2024.1.
- [ ] **Step 3:** Decision 7: `mv ~/rama-medida ~/ramify-measure`, the docs that name it, the two JSON keys and the fixture name.
- [ ] **Step 4:** Decision 8 in `CLAUDE.md`.
- [ ] **Step 5:** `git grep -n -i -w rama` lists only what decisions 2, 3, 5 and 6 keep.
- [ ] **Step 6:** Commit `docs: the project is called Ramify`.

### Task 4: Verification

- [ ] **Step 1:** The full suite at low priority, one after another: `pnpm tsc`, vitest, the Rust library tests, the integration tests, clippy and the Python script tests.
- [ ] **Step 2:** A release build (`nice -n 15 pnpm tauri build --bundles app`) and a check that the bundle is `Ramify.app` with `CFBundleIdentifier` `dev.ramify.app`. The first start against the user's real data folder is left to the user, as with every phase.
- [ ] **Step 3:** One final review of the whole branch on the most capable model, read only, then the fixes it asks for.
- [ ] **Step 4:** Tell the user what was not checked (the migration on their real folder and Keychain item) and ask about decision 10.
