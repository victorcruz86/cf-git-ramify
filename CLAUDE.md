# Ramify

Ramify is a Git client for macOS (Tauri 2, Rust, Svelte 5) that copies the Git support of IntelliJ IDEA CE 2024.1. It was called rama until 2026-11-10; `src-tauri/src/migration.rs` copies rama's data folder and Keychain items.

## Name

- `Ramify` in prose and in everything the user sees: titles, menus, dialogs, messages, documentation.
- `ramify` in identifiers: the package, crate and binary, file and folder names, temp and socket prefixes, environment variables (`RAMIFY_…`), localStorage and JSON keys, the bundle identifier `dev.ramify.app`.
- The old name `rama` appears only where the migration needs it, in the history that records it, and in commit messages already in git.
- Do not use `IDEA`, `IntelliJ` or `JetBrains` in the product's name: they are JetBrains trademarks.

## Language

Everything written into this repository is in English, with no exception:

- Code: identifiers, file names, comments, doc comments, log and error messages.
- User-facing texts of the app, including the ones IntelliJ has no text for.
- Data: JSON and configuration keys and values, stored file names, environment variable names.
- Tests: names, descriptions, fixtures and test data (file names, repository names, messages).
- Commit messages, branch names, pull request titles and descriptions.
- Documentation: README, plans, specs, reports, decision records, NOTICE.
- Folders and files the project creates outside the repository (for example `~/ramify-measure`).

Conversation with the user stays in Spanish; only what lands in the repository, or what the project writes to disk, is in English.
