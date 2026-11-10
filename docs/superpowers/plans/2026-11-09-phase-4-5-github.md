# Phase 4, part 5: GitHub. Implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** the user works with their github.com repositories as in IntelliJ 2024.1, without OAuth and without review:
- GitHub accounts, with the tokens in the macOS Keychain, and `Log In with Token…` / `Log In to GitHub Enterprise…`;
- protected-branch rules loaded from GitHub after every fetch;
- the `Pull Requests` tool window:
  - a list with search, filters and quick filters;
  - a details tab per PR with its description, status, commits and changes;
  - `Checkout` and `Show in Git Log` of the PR branch;
- `Open on GitHub` and `Copy Link`.

**Architecture:**
- Rust makes every HTTP request and holds every token. The webview sends a token once, at login, and never receives one.
- New modules under `src-tauri/src/hosting/`:
  - `server.rs`: GitHub server paths and API URLs;
  - `mapping.rs`: remote URL → GitHub repository;
  - `http.rs`: the client, errors, paging and GraphQL;
  - `accounts.rs`, `protection.rs`, `links.rs`;
  - `pulls/` (`list.rs`, `details.rs`, `changes.rs`, `checkout.rs`);
  - `commands.rs`;
  - the `.graphql` texts.
- `src-tauri/src/secrets.rs` holds the Keychain store and an in-memory store for tests.
- PR changes are computed locally with git after the same two fetches 241 does (overview decision 16). Checkout reuses `branches::checkout_remote`, which already takes the recoverable copy.
- Frontend:
  - `src/hosting/`: accounts, the login dialog and link menus;
  - `src/pulls/`: the tool window, the selector, the list and the details.
  - `Pull Requests` is a first-use tool window on the left stripe, like `Changes`. It is the first tool window with closable tabs.

**Tech Stack:** Tauri 2.12, Rust 1.99.1, Svelte 5, Vitest 5. Two new dependencies (overview decision 15):
- `ureq` 2.12.1, with `default-features = false, features = ["native-tls"]`. Its source is already in the cargo cache.
- `security-framework`, at the version that `native-tls` resolves to, so only one copy is built. Plus `native-tls` itself.

`security-framework`, `security-framework-sys` and `native-tls` are not in the cargo cache: **Task 1's first build needs network once.** The user approves it with this plan.

**Spec:** `docs/superpowers/specs/2026-10-31-intellij-git-client-design.md` and the phase 4 overview `docs/superpowers/plans/2026-11-03-phase-4-overview.md`. The overview's Global Constraints and its decisions 13 to 18 are part of this plan.

**Research** (not in the repository). The implementer reads it before each task:
- `.superpowers/phase-4/research-github-gitlab.md` (E§n): the first pass, from the phase design;
- `.superpowers/phase-4/research-part-5-241.md` (P§n): the 241 details, with corrections to E in P§10 and the GraphQL texts in its Appendix A;
- `.superpowers/phase-4/research-part-5-rama.md` (M§n): rama today, with lines, and the hazards in M§0.

**Reference:**
- Tag `idea/241.14494.240`.
- Bundles: `GB:` GithubBundle, `CTB:` CollaborationToolsBundle, `GitB:` GitBundle.
- Copy literal texts from the cited line, not from this plan.

**Branch:** `phase-4`, after part 4 (`f2a3c7b`, also `main`).

**Commands.** These are binding: the MacBook froze under parallel builds. Run one command at a time, at low priority, never in the background, and never in a second copy of the project.
- TS: `PATH=$HOME/.cache/rama-toolchain/node-v22.23.4-darwin-arm64/bin:$PATH nice -n 15 pnpm exec vitest run --maxWorkers=2 <files>` and `nice -n 15 pnpm exec tsc --noEmit -p .`
- Rust: `PATH=$HOME/.cargo/bin:$PATH CARGO_BUILD_JOBS=3 nice -n 15 cargo test --manifest-path src-tauri/Cargo.toml --lib <filter> -- --test-threads=3`
- Integration: the same with `--no-fail-fast --test '*'`, in every final verification.
- Python: `python3 -I -m unittest discover -s scripts`

## Global Constraints

The overview's Global Constraints, including phase 3's. The ones this part leans on most:
- **Tokens:**
  - only in the Keychain (`secrets.rs`): service `rama GitHub <server>`, account `<login>`;
  - never on disk, in the console, an error, an argument, the environment, a URL or a log line;
  - the webview sends one once, in `github_login`, and no command returns one;
  - a redirect to another host or scheme drops the `Authorization` header.
- **Text from GitHub is untrusted:** titles, bodies, labels, logins, branch names and check names are shown as text. No `{@html}`, no `messageHeadlineHTML` or `messageBodyHTML`, no change to the CSP, no remote images. Label colours are accepted only as six hex digits.
- **URLs opened in the browser:** only `https://` (or the account's `http://` server) URLs of a known GitHub server or of GitHub's own data (PR, check and commit URLs that the API returned for that server), through `/usr/bin/open` with the URL as one argument that never starts with `-`.
- **Long requests:** every network call is a cancellable task with the connect timeout from the settings (default 5 s) and a 60 s read timeout. A failure never blocks the window and never fails a fetch.
- **New tool windows and commands:**
  - Every new command that changes files or the index calls `flush_diff_window` first and takes `RepoLocks`.
  - The counts of `mutating_without_saving_the_diff_is_not_allowed` and `whatever_touches_the_repository_saves_the_diff_window_first` follow the new commands.
  - Every new command is exercised in `src/perf/smoke.ts` or listed in `NOT_RUN` of `scripts/smoke_ipc.py`; network commands are always `NOT_RUN`.
- **Tests stay offline:**
  - GitHub through the fake server on `127.0.0.1:0` (Task 1);
  - PR refs through local bare remotes with `git update-ref refs/pull/N/head`;
  - the Keychain through the in-memory store.
- **Everything written into the repository is in English.** Commits end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Decisions of this part

They add to the overview's decisions, and the user approves them with this plan. Each has its cost if wrong.

1. **Account changes apply at once, not on Save.** 241 keeps them pending until Apply. rama's settings dialog saves `Settings`, and a token must never ride in `Settings`, so `+`, `−`, `Set as Default` and `Log in` act immediately. Cost: Cancel does not undo an added or removed account.
2. **Where accounts live:**
   - Identity is (server without its scheme, login), which is 241's uniqueness rule (P§1.3). It is written `<server>/<login>`, for example `github.com/octocat`.
   - The list (server and login only) is in `<app data>/github-accounts.json`.
   - The default account is per repository: a new `RepoSettings.github_default_account: Option<String>`.
   - Cost: none.
3. **Re-login goes straight to the token dialog.** On github.com, 241 first asks OAuth or token (P§1.2). Cost: none, since there is no OAuth (overview decision 14).
4. **No server discovery.** The known servers are github.com plus the servers of the accounts. 241 also probes unknown remote hosts, and its probe is broken behind a path prefix (P§3.1). Cost: a GitHub Enterprise remote shows no `Pull Requests` window and no `Open on GitHub` until an account on that server exists.
5. **Connect skips `findUser("ghost")`.** A deleted author shows `ghost`, the login that request returns. Cost: none.
6. **Protected rules are stored per remote.** A successful fetch of remote R replaces R's entry: its rules, or none when R is not GitHub or has no token. The effective list is the user's patterns plus the union of the entries. Entries of remotes that no longer exist are dropped.
   - 241 replaces the whole list with what the remotes of the last fetch returned, so fetching only a non-GitHub remote empties it (P§4.1).
   - As in 241, the loader runs inside the fetch's task, after git, with the progress text `Loading GitHub branch protection rules`. A loader error keeps R's previous entry and is never shown.
   - Cost: a remote's rules stay in force until that remote is fetched again, which forbids more, not less. A fetch can wait for the loader's requests (connect timeout 5 s).
7. **No "unseen" dot and no tool-window badge**, because 241 clears them only from the timeline, which is left out (P§5.5). **No access lines** (`Repository access required…`, `Merging is disabled…`), because they are about merge rights and rama has no merge or close actions (P§6.3). Cost: visual only.
8. **Where `Open on GitHub` and `Copy Link` go** (P§8.5):
   - **Same place as 241:**
     - the Log menu, last;
     - the file history menu, last, plus its toolbar button;
     - the selection history popup, last;
     - the annotation gutter, after a separator.
   - **The Project tree and the viewer** have no `Open In` submenu in rama, so they get a new `Open In ▸ GitHub`, last.
   - **`Copy Link to GitHub Repository`** goes in a new `Copy / Paste Special` submenu of the viewer's editor menu, with the line range. rama has no `Copy Path/Reference…` popup, so the Project tree gets no Copy Link.
   - **Several GitHub remotes:** the group becomes a submenu named by URL, with `_` shown as a space, as in 241.
   - Cost: two small submenus that 241 fills with other items; no Copy Link from the tree.
9. **The PR changes tree waits for both fetches.** 241 draws the tree from REST and only the diff waits (P§6.7). Cost: with a failed fetch rama shows `Can't load changes` where 241 shows the tree and fails the diff.
10. **The list's last search and search history are kept per repository** in `github-pr-search.json`. 241 keeps them per project in `workspace.xml`. Cost: none.
11. **Settings:**
    - `Connection timeout:` (seconds, `0..60`, default 5) is an app setting, and the read timeout stays at 60 s (P§1.1).
    - `Clone git repositories using ssh` (no clone in rama) and `Automatically mark opened files as viewed` (review) are not copied.
    - Cost: none.
12. **HTTP details:**
    - the `User-Agent` is `rama`;
    - no gzip;
    - redirects are followed by hand, up to 10, and the `Authorization` header is kept only on the same scheme, host and port;
    - the `Generate…` page's description is `rama GitHub integration plugin` (P§2.4).
    - Cost: none.
13. **The PR diff opens in rama's diff window**, titled `Diff for Pull Request #<n>` (`GB:246`), as every rama diff does. 241 opens an editor tab. Cost: none.

## Review Focus

1. **A token leaks.**
   - Where it must not appear: an error from GitHub, the console, a notification, a recorded request of the fake server outside the `Authorization` header, a redirect to another host, or a panic message.
   - Tests in Tasks 1, 2 and 5. A grep test asserts that no command's return type carries a token.
2. **Remote URLs of every shape:**
   - `git@github.com:o/r.git`, `ssh://git@github.com:22/o/r`, `https://github.com/o/r/`, `https://github.com/O/R.git`;
   - a GHE server with a path prefix;
   - a local path, an underscore host, a URL with a space;
   - two remotes on the same repository, and `origin` as a fork with `upstream` as its parent.

   The mapping, the links and checkout's `findRemote` give 241's result for each. Tests in Tasks 1, 4 and 9.
3. **The network fails:**
   - offline, a timeout, 401 `Bad credentials`, 403 rate limit, 404;
   - a GraphQL `NOT_FOUND`, `INSUFFICIENT_SCOPES` and several errors;
   - a GHE server older than 3.0;
   - an HTML body with `200`.

   Every place shows 241's text for it (P§9.3), nothing hangs past the timeouts, and a fetch never fails because of the protected-rules loader. Tests in Tasks 1, 5, 7 and 8.
4. **Untrusted text from GitHub:**
   - a title with `<img onerror>`, a body of 1 MB, control characters and right-to-left marks;
   - a label colour `red;background:url(x)`;
   - a check URL `javascript:alert(1)`;
   - a branch name with a leading `-`.

   Everything is shown as text, the colour is dropped, the URL is refused, and git receives the branch only after `--end-of-options` or as a validated ref. Tests in Tasks 7, 8 and 9.
5. **The repository changes while a PR tab is open or loads:**
   - a checkout or commit from the terminal;
   - the head remote removed;
   - the PR force-pushed, so `headRefOid` moved;
   - another repository opened in the window.

   What must happen:
   - the checked-out icon follows `repo-changed`;
   - `Refresh Pull Request` reloads the changes for the new head;
   - an answer for an old head or an old repository is dropped;
   - a checkout waits for `RepoLocks` and keeps its recoverable copy.

   Tests in Tasks 8 and 9.

---

### Task 1: HTTP client, GitHub servers and remote mapping

Research: P§3.1-3.2, P§9, E§5-6, M§0, M§8, M§9. Decisions 4 and 12.

**Files:**
- Modify:
  - `src-tauri/Cargo.toml`: add `ureq`, with the features above, and `native-tls`. Update `Cargo.lock`.
  - `src-tauri/src/lib.rs`: add `mod hosting`.
  - `src/log/LogPanel.svelte:981`: `Cargando…` → `Loading…`. This is a language-rule fix (M§0.6).
  - `NOTICE`: the ported classes.
- Create:
  - `src-tauri/src/hosting/mod.rs`.
  - `src-tauri/src/hosting/server.rs`, a port of `GithubServerPath` (P§2.3, P§9.1):
    - `GithubServer { use_http: Option<bool>, host: String, port: Option<u16>, suffix: Option<String> }`;
    - `parse(text) -> Result<GithubServer, ServerError>`, with 241's regex and trimming;
    - `api_url()`, `graphql_url()`, `web_url()`;
    - `display()` (`github.com`, or the text as typed);
    - `same_ignoring_scheme(&other)`;
    - `is_github_com()`.
  - `src-tauri/src/hosting/mapping.rs`, a port of `GitHostingUrlUtil.getUriFromRemoteUrl`/`match` and `GithubUrlUtil.getUserAndRepositoryFromRemoteUrl` (P§3.1-3.2):
    - `RepoCoordinates { server, owner, name }`, with case-insensitive equality;
    - `GithubMapping { coordinates, remote: String, url: String }`;
    - `mappings(remotes: &[Remote], servers: &[GithubServer]) -> Vec<GithubMapping>`: fetch URLs only, the first server that matches, several remotes on one repository collapsing to the first;
    - `web_base(&mapping) -> String`.
  - `src-tauri/src/hosting/http.rs`:
    - `Client::new(connect_timeout: Duration)`;
    - `get(url, token: Option<&Secret>, accept: Option<&str>) -> Result<Response, HostingError>`;
    - `graphql(url, token, accept, query, variables) -> Result<Value, HostingError>`;
    - `pages_rest(url…)`, which follows `Link: rel="next"`;
    - `etag(url, token) -> Result<Option<String>, HostingError>`;
    - redirects by hand (decision 12);
    - the GHE version check from `x-github-enterprise-version` (P§9.2).
  - `HostingError` and its 241 texts:
    - `Authentication(String)` = `Request response: <presentable>.`;
    - `RateLimit(String)`;
    - `Status { code, reason, message }` = `<code> <reason> - <presentable>`;
    - `GraphQl(String)`, which is `[m1, m2]` for several errors;
    - `Scopes` = `Access token has not been granted the required scopes.`;
    - `Unreachable`;
    - `Io(String)`.

    The presentable message follows `GithubErrorMessage` (P§9.2). `Display` never includes request headers.
  - `src-tauri/src/secrets.rs`:
    - `pub struct Secret(String)`, with a `Debug` that prints `Secret(***)`, no `Display`, no `Serialize`, and zeroed on drop;
    - `trait SecretStore { get, set, delete }`, keyed by `(service, account)`;
    - `MemoryStore`.

    The Keychain store comes in Task 2.
  - `src-tauri/src/test_support.rs`: `FakeGitHub`:
    - a `TcpListener` on `127.0.0.1:0` with a thread that serves a script of `(method, path+query, status, reason, headers, body)` in order;
    - it records each request: method, path, headers and body;
    - `url()` and `requests()`;
    - an unexpected request fails the test.
- Test: `src-tauri/src/hosting/{server,mapping,http}.rs`, `src-tauri/src/secrets.rs`.

**Interfaces:**
- Produces: `GithubServer`, `RepoCoordinates`, `GithubMapping`, `mappings`, `web_base`, `Client`, `HostingError`, `Secret`, `SecretStore`, `MemoryStore`, `FakeGitHub`. Every later task uses them.

- [ ] **Step 1: Write the failing tests:**
  - `server_paths_follow_241`. The six forms of P§2.3, with their API, GraphQL and web URLs, and the three validation errors `GB:34`, `GB:35`, `GB:31`. The token check belongs to Task 2.
  - `remote_urls_map_as_241` covers Review Focus 2:
    - each shape gives its coordinates, or no mapping;
    - two remotes on the same repository give one mapping;
    - a fork and its parent give two;
    - `/git` matches `/gitlab/…`, as 241 does.
  - `errors_read_like_241`. A 401 `{"message":"Bad credentials"}` gives `Request response: Bad credentials.`. Also:
    - the 403 rate limit;
    - `404 Not Found - Not Found`;
    - a GraphQL `NOT_FOUND` with `data` null;
    - two GraphQL errors;
    - `INSUFFICIENT_SCOPES`;
    - a `text/html` 200;
    - `x-github-enterprise-version: 2.22`, with the exact text of P§9.2.
  - `a_redirect_to_another_host_drops_the_token` checks the recorded requests of two fake servers. Also `a_redirect_loop_stops_at_ten`.
  - `the_token_never_shows_in_an_error_or_debug`. Format each `HostingError` and `Secret` with `{}` and `{:?}`, with the token in the response body and in the URL query, and check the token is absent.
  - `rest_pages_follow_link_next` and `graphql_pages_follow_end_cursor`.
  - `a_slow_server_times_out_within_the_connect_and_read_limits`. Use a short limit in the test.
- [ ] **Step 2: Check that they fail.**
- [ ] **Step 3: Implement.** The first `cargo` run needs network once to download `native-tls` and `security-framework`. If it fails offline, stop and report; do not vendor or swap crates.
- [ ] **Step 4: Check that they pass.**
- [ ] **Step 5: Commit.** `git commit -m "feat: an HTTP client for GitHub, its server paths and the remote mapping"`

### Task 2: Keychain and accounts

Research: P§1, P§2, E§2, E§6, M§4. Decisions 1, 2, 3 and 12.

**Files:**
- Modify:
  - `src-tauri/Cargo.toml`: add `security-framework`.
  - `src-tauri/src/secrets.rs`: `KeychainStore`, built on `set_generic_password`, `get_generic_password` and `delete_generic_password`.
  - `src-tauri/src/settings.rs`:
    - `RepoSettings.github_default_account: Option<String>`;
    - `Settings.github_connection_timeout_ms: u64` (default `5000`, clamped to `0..=60000`, as P§1.1's `connectTimeout`).
  - `src-tauri/src/lib.rs`, `src/lib/api.ts`, `scripts/smoke_ipc.py` (`NOT_RUN`).
- Create:
  - `src-tauri/src/hosting/accounts.rs`:
    - `GithubAccount { server: GithubServer, login: String }`, with `id() = "<server>/<login>"`;
    - `list(base)` and `save(base, accounts)`, using `storage::load_json` and `storage::write_atomic`;
    - `login(client, store, base, server, token: Secret, editing: Option<id>) -> Result<GithubAccount, LoginError>`: `GET <api>/user`, then the uniqueness rule of P§1.3; the token is written to the Keychain only on success;
    - `remove(store, base, id)`, which deletes the Keychain item;
    - `token(store, &account) -> Option<Secret>`;
    - `details(client, store, &account) -> AccountDetails { name: Option<String>, error: Option<String>, needs_login: bool }`, with 241's texts (P§1.2);
    - `generate_url(server) -> String` (P§2.4).
  - `src-tauri/src/hosting/commands.rs`:
    - `github_accounts() -> Vec<AccountView>`, where `AccountView { id, server, login, name, error, needsLogin, isDefault }` and the default comes from the open repository;
    - `github_login(server: String, token: String, editing: Option<String>) -> Result<AccountView, ApiError>`, a task that wraps `token` in a `Secret` at once;
    - `github_remove(id)` and `github_set_default(id)`;
    - `github_generate_url(server)`;
    - `open_url(url)`, as `reveal_in_finder` with `/usr/bin/open`. It refuses anything that is not `https://`/`http://`, or that starts with `-`.
- Test: `src-tauri/src/hosting/accounts.rs`, `src-tauri/src/secrets.rs`, `src-tauri/src/hosting/commands.rs`.

**Interfaces:**
- Consumes: Task 1.
- Produces:
  - `accounts::list`, `accounts::token`, `GithubAccount`, `AccountView`;
  - `HostingState { client, store: Arc<dyn SecretStore> }` in `AppState`, with the Keychain in the app and `MemoryStore` in tests;
  - the TS functions `githubAccounts`, `githubLogin`, `githubRemove`, `githubSetDefault`, `githubGenerateUrl`, `openUrl`.

- [ ] **Step 1: Write the failing tests:**
  - `login_checks_user_and_stores_the_token_only_on_success`, against the fake server. Also a 401, which gives `Incorrect credentials.\n…`, and an unreachable server, which gives `Server is unreachable` (`GB:38`, `GB:331`). The store is unchanged after both failures.
  - `a_second_account_with_the_same_server_and_login_is_refused`: `Account 'octocat' already added` (`GB:32`), with `http://` vs `https://` on the same host counting as the same server.
  - `relogin_renames_the_edited_account_to_the_tokens_login`.
  - `removing_an_account_deletes_its_keychain_item_and_clears_the_default`.
  - `a_missing_token_gives_missing_access_token_and_needs_login` (`CTB:8`).
  - `the_generate_url_is_241s_with_rama_s_name`: the exact string of P§2.4.
  - `no_command_returns_a_token`. Grep `hosting/commands.rs` for `Secret` in any `-> Result<` or `Serialize` type. Also check the recorded requests: the token appears only in `Authorization: Bearer`.
  - `open_url_refuses_a_dash_and_other_schemes`.
  - Manual, recorded for Task 10: add a real account and quit and reopen rama; the account is still there. Check the token in Keychain Access.
- [ ] **Step 2: Check that they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Check that they pass.**
- [ ] **Step 5: Commit.** `git commit -m "feat: GitHub accounts with their tokens in the Keychain"`

### Task 3: GitHub settings and the login dialog

Research: P§1.1-1.4, P§2.1-2.5, M§4. Decisions 1, 3 and 11.

**Files:**
- Create:
  - `src/hosting/LoginDialog.svelte`, with the variants of P§2.1:
    - title and OK text;
    - `Server:` editable or not;
    - `Token:` as a password field, with the scopes comment (`CTB:174` with 241's list) and `Generate…`, which opens `githubGenerateUrl` with `openUrl`;
    - validation in 241's order;
    - while busy, the fields are disabled;
    - errors under the field, and OK stays enabled.

    The token lives only in the field and in the one `githubLogin` call; it is cleared after it.
  - `src/hosting/AccountsList.svelte`:
    - the toolbar `+` popup (`Log In with Token…`, separator, `Log In to GitHub Enterprise…`), `−` and `Set as Default`;
    - the rows of P§1.2: full name or login, bold when default, the server below, the error in red and the `Log in` link;
    - the empty list `No accounts added` with `Add account…` (⌘N);
    - the details load per row.
  - `src/hosting/accounts.ts`: the store, plus `loginFor(server, editing?)`, which opens the dialog. Tasks 6 and 7 reuse it.
- Modify:
  - `src/settings/SettingsView.svelte`: a `GitHub` section with the accounts list and `Connection timeout:` `[n]` `seconds` (0-60), saved with `Settings`. The section is placed after the existing `Git` sections.
  - The notification `Missing default GitHub account`, with `Configure…` (`GB:21`, `GitB:672`). It shows when the stored default no longer exists, and `Configure…` opens the settings.
- Test: `src/hosting/LoginDialog.test.ts`, `src/hosting/AccountsList.test.ts`, `src/settings/SettingsView.test.ts`.

**Interfaces:**
- Consumes: Task 2's TS functions.
- Produces: `loginFor(server: string | null, editing?: string): Promise<AccountView | null>`, and the `githubAccountsStore`.

- [ ] **Step 1: Write the failing tests:**
  - `the_token_dialog_variants_have_241s_titles_and_buttons`. These are the four rows of P§2.1, with the server editable only for Enterprise and for a request without a server.
  - `validation_stops_at_the_first_failure_in_241s_order`.
  - `the_token_is_sent_once_and_cleared`. `githubLogin` is called once, and the field is empty after success and after a failure.
  - `generate_opens_the_prefilled_page`.
  - `the_accounts_list_shows_default_in_bold_and_log_in_on_auth_errors`.
  - `set_as_default_is_off_for_the_default_and_minus_needs_a_row`.
  - `connection_timeout_saves_milliseconds_and_shows_seconds`.
- [ ] **Step 2: Check that they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Check that they pass.**
- [ ] **Step 5: Commit.** `git commit -m "feat: the GitHub settings section and the token login dialog"`

### Task 4: Open on GitHub and Copy Link

Research: P§8, M§5 (menus), M§8 (browser). Decisions 8 and 4.

**Files:**
- Create:
  - `src-tauri/src/hosting/links.rs`:
    - `commit_url(base, hash)`;
    - `file_url(base, revision, path, lines: Option<(u32, u32)>)`, with `encodePath` and the `#L<a>[-L<b>]` fragment (P§8.1, 0-based lines in, 1-based out);
    - `last_revision_of(repo, path)` = `git log -n1 HEAD -- <path>`, through `git::exact` with `-c log.showSignature=false` (P§8.2). It fails with `Cannot open in browser` / `Cannot retrieve latest revision for file` (`GitB:1560,1562`);
    - `references(repo, target) -> Vec<Reference { name, url }>`: one per GitHub mapping, with `name` = the web base with `_` → space. It is empty for an untracked, added or ignored file and for the repository root (P§8.3).
  - Commands in `hosting/commands.rs`: `github_links(target: LinkTarget) -> Vec<Reference>`, with `LinkTarget = Commit { hash } | File { path, lines }`.
  - `src/hosting/links.ts`: `hostingItems(target, kind: "open" | "copy", label)`. It gives no item for no reference, one item for one, and a submenu for several. `open` runs `openUrl` and `copy` runs `clipboardWrite`.
- Modify, adding the item at 241's place (P§8.5):
  - `src/log/contextMenu.ts`: `Open on GitHub` last, for the first selected commit.
  - `src/log/historyMenu.ts` and `src/log/HistoryTab.svelte`: `Open on GitHub` last in the menu, and the toolbar button last, with the tooltip `Open on GitHub`. It opens the commit URL of the row (P§8.2). Remove the "left out" comment at `historyMenu.ts:38`.
  - `src/history/SelectionHistoryWindow.svelte`: `Open on GitHub` last, the commit URL. Remove the comment at `:259-260`.
  - `src/viewer/annotation.ts`: a separator and `Open on GitHub` after `Annotate Previous Revision`, the commit URL of the line.
  - `src/tree/ProjectTree.svelte`: `Open In ▸ GitHub`, the file URL at `last_revision_of`, no lines.
  - `src/viewer/ViewerWindow.svelte`:
    - `Open In ▸ GitHub` and `Copy / Paste Special ▸ Copy Link to GitHub Repository`, both with the selection's lines, using 241's end-line rule including its column-0 quirk (P§8.2);
    - for a revision tab, the file URL at that revision.
- Test: `src-tauri/src/hosting/links.rs`, `src/hosting/links.test.ts`, the existing menu tests (`contextMenu.test.ts`, `historyMenu.test.ts`, `SelectionHistoryWindow.test.ts`, `annotation.test.ts`, `ProjectTree.test.ts`, `ViewerWindow.test.ts`).

**Interfaces:**
- Consumes: Task 1's `mappings`, `web_base`; Task 2's `openUrl`.
- Produces: `hostingItems`, which Task 7 does not need (PR URLs come from the API).

- [ ] **Step 1: Write the failing tests:**
  - `urls_follow_241`:
    - a commit;
    - a file with a space and `é`;
    - lines 0-0 → `#L1`, and 4-9 → `#L5-L10`;
    - a directory.
  - `a_file_url_uses_the_last_commit_that_touched_it_not_head`.
  - `no_reference_for_untracked_added_ignored_or_root` and `two_github_remotes_give_a_submenu_named_by_url`.
  - `each_menu_has_open_on_github_at_241s_place`, one assertion per menu file.
  - `the_viewer_copies_the_link_with_its_lines`, including the column-0 quirk.
  - `no_github_remote_hides_the_items`.
- [ ] **Step 2: Check that they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Check that they pass.**
- [ ] **Step 5: Commit.** `git commit -m "feat: Open on GitHub and Copy Link"`

### Task 5: Protected branches from GitHub

Research: P§4, M§1, M§2, E§4. Decision 6.

**Files:**
- Modify:
  - `src-tauri/src/settings.rs`:
    - `RepoSettings.load_github_protection: bool`, default `true` (`GitSharedSettings.java:36`);
    - `pub fn effective_protected(dir) -> Vec<String>` and `pub fn try_effective_protected(dir) -> io::Result<Vec<String>>` = the user's patterns ∪ (when on) the loaded ones;
    - `RepoSettings` sent to the interface gains a read-only `github_protected: Vec<String>` that is never saved (M§1).
  - Every caller of M§1's table reads the effective list:
    - `push.rs` `check_force` (strict) and `info`;
    - `branches.rs` `delete_remote` (strict);
    - `rebase.rs` `published`;
    - `apply_commits.rs` `protected_for`;
    - `rewrite.rs` `protected_branch`;
    - `vcs_commands.rs` `push_info`;
    - TS `src/branches/state.ts`.
  - `src-tauri/src/branch_info.rs`: `BranchSync` gets `after_fetch: Option<Arc<dyn Fn(&TaskContext, &Path, &str) + Send + Sync>>`, set in `lib.rs` setup, plus a `fetched(ctx, repo, remote)` that calls it. It is called at the three sinks of M§2: `fetch.rs:61`, `update.rs:696` and `pull.rs:255`, and by Tasks 8 and 9's fetches.
  - `src/settings/SettingsView.svelte`, under `Protected branches:`:
    - the checkbox `Load branch protection rules from GitHub` (`GitB:701`), with its comment (`GitB:702`);
    - the read-only grey suffix of P§4.4: `;`-joined, refreshed on focus loss, and validated as in P§4.4.
- Create:
  - `src-tauri/src/hosting/protection.rs`:
    - `convert_to_regex(mask)`, exactly P§4.2;
    - `load(ctx, client, store, base, data, repo, remote)`:
      - the mapping of `remote`, if any;
      - the account: the repository's default when it is on that server, else the first account on that server;
      - all pages of `getProtectionRules` (Appendix A.5);
      - the result replaces that remote's entry in `<data>/github-protection.json`, with removed remotes dropped (decision 6);
      - errors are swallowed and keep the entry.
  - `src-tauri/src/hosting/graphql/getProtectionRules.graphql`, from P Appendix A.5.
- Test: `src-tauri/src/hosting/protection.rs`, `src-tauri/src/settings.rs`, `src-tauri/src/push.rs`, `src-tauri/src/fetch.rs`, `src/settings/SettingsView.test.ts`.

**Interfaces:**
- Consumes: Tasks 1 and 2.
- Produces: `settings::effective_protected`, `settings::try_effective_protected`, `BranchSync::fetched`. Tasks 8 and 9 call `fetched` after their fetches.

- [ ] **Step 1: Write the failing tests:**
  - `convert_to_regex_is_241s`: the four examples of P§4.2, plus `a\\b`, `**` and `[0-9]`.
  - `a_fetch_loads_the_remotes_rules_and_force_push_to_them_is_prohibited`. Use the fake server and a bare `origin` whose URL maps to the fake server's account. The test walks `fetch_remote`, then `check_force`, and expects `Force push to release/1 is prohibited`.
  - `fetching_a_non_github_remote_clears_only_its_entry`, the departure in decision 6.
  - `a_loader_error_keeps_the_entry_and_the_fetch_still_succeeds`. Also `an_unreachable_server_does_not_fail_update_project`.
  - `the_checkbox_off_ignores_the_loaded_rules_and_the_next_fetch_clears_them`.
  - `the_user_list_is_saved_without_the_github_rules` (M§1).
  - `pull_merge_and_update_branch_also_load_the_rules` (the two sinks outside `fetch.rs`).
  - `the_token_is_not_in_the_console_after_a_failed_load` (Review Focus 1).
- [ ] **Step 2: Check that they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Check that they pass.** This includes every existing test of the callers.
- [ ] **Step 5: Commit.** `git commit -m "feat: protected branch rules loaded from GitHub after every fetch"`

### Task 6: The Pull Requests tool window and its selector

Research: P§3.3-3.6, M§3, M§10. Decisions 2, 4 and 5.

**Files:**
- Create:
  - `src-tauri/src/hosting/pulls/mod.rs`:
    - `known(repo, base) -> Known { mappings: Vec<MappingView>, accounts: Vec<AccountView> }`;
    - `connect(ctx, client, store, mapping, account) -> Connection { coordinates (API path), viewer_login, repository: RepoInfo }`, with `GET /user` (cached 30 minutes per account) and `findRepository` (Appendix A.6). A missing repository gives 241's unlocalized text (P§3.6);
    - the saved choice `(remote url, account id)` in `<data>/github-pr-connection.json`.
  - `src-tauri/src/hosting/graphql/findRepository.graphql`.
  - Commands: `github_known()`, and `github_connect(remote, account)`, which is a task.
  - `src/pulls/PullRequestsWindow.svelte`:
    - **Tabs.** A fixed list tab named as P§5.1, plus closable `#<n>` tabs whose tooltip is `<repo>: <title>`. The tab strip reuses `LogTabs.svelte` with props for its `aria-label`s and without its `+`.
    - **The gear menu.** `Change Repository or Account…` (`GB:168`), shown as in P§3.3.
  - `src/pulls/Selector.svelte`, with P§3.5:
    - the repository combo, `owner/repo` plus the remote name in grey;
    - the account selector, filtered to the server;
    - `View Pull Requests` (`GB:167`);
    - `Log In with Token…` or `Log In to GitHub Enterprise…` when there is no account or no token;
    - the error area with `Log in again…`;
    - the footer note `Change repository or account later under the gear icon`;
    - the automatic pair of P§3.4, and the saved choice.
  - `src/pulls/store.ts`: the connection and the tab list (open, close, reuse by number).
- Modify:
  - `src/shell/toolWindows.ts`: the `pulls` entry, `Pull Requests`, on the left, registered on first use.
  - `src/lib/tabs.ts`, `src/App.svelte`, `src/lib/api.ts`, `src-tauri/src/lib.rs`.
  - A new `src/lib/icons/pullRequests.svg`, a plain drawing of rama's own.
  - The window registers when `github_known()` has a mapping, and is checked again on `open()` and on `repo-changed`.
- Test: `src-tauri/src/hosting/pulls/mod.rs`, `src/pulls/Selector.test.ts`, `src/pulls/PullRequestsWindow.test.ts`, `src/lib/tabs.test.ts`.

**Interfaces:**
- Consumes: Tasks 1-3.
- Produces:
  - `Connection`, `MappingView { remote, url, display }`;
  - TS `pullsStore`, `openPullRequestTab(number)`, `currentConnection()`.

  Tasks 7-9 work inside a connection.

- [ ] **Step 1: Write the failing tests:**
  - `the_window_shows_only_with_a_github_remote`: github.com with no account shows it; GHE without an account does not (decision 4).
  - `one_mapping_and_one_account_connect_by_themselves`, `a_saved_choice_reconnects`, and `otherwise_the_first_repo_and_the_first_account_by_name_are_preselected`.
  - `a_missing_token_says_could_not_connect_and_missing_access_token`.
  - `a_failed_connect_names_repo_and_account_and_offers_log_in_again`.
  - `connect_uses_the_api_repository_path_after_a_rename`.
  - `change_repository_or_account_forgets_the_choice`.
  - `pr_tabs_open_once_per_number_and_close`.
- [ ] **Step 2: Check that they fail.**
- [ ] **Step 3: Implement.** Until Task 7, the list tab shows an empty placeholder, and until Task 8 a PR tab does too.
- [ ] **Step 4: Check that they pass.**
- [ ] **Step 5: Commit.** `git commit -m "feat: the Pull Requests tool window and its repository and account selector"`

### Task 7: The pull request list

Research: P§5, P§9.3, Appendix A.1. Decisions 7 and 10.

**Files:**
- Create:
  - `src-tauri/src/hosting/pulls/list.rs`:
    - `SearchValue { text, state, assignee, review, author, label }`;
    - `query_string(&coordinates, &SearchValue)`, exactly P§5.4;
    - `page(ctx, conn, value, cursor) -> Page { items: Vec<PullRequestShort>, end_cursor, has_next }` through `issueSearch` (Appendix A.1, page size 50, its `Accept`);
    - `PullRequestShort { number, title, url, state, is_draft, mergeable, author: Option<String>, created_at, labels: Vec<Label { name, color: Option<String> }>, assignees, reviewers: Vec<(name, ReviewState)>, unresolved }`. `color` is kept only when it is six hex digits;
    - `etag(conn)`, the request of P§5.8;
    - `choosers(conn, kind)` for collaborators, assignees and labels (REST pages, P§5.2);
    - the persisted `last` and `history` (up to 10) in `<data>/github-pr-search.json`.
  - `src-tauri/src/hosting/pulls/reviewers.rs`: the reviewer map of P§6.4, shared with Task 8.
  - `src-tauri/src/hosting/graphql/issueSearch.graphql`.
  - Commands: `github_pr_page(value, cursor)` (a task), `github_pr_etag()`, `github_pr_chooser(kind)`, `github_pr_search_saved()` and `github_pr_search_save(value)`.
  - `src/pulls/List.svelte`:
    - the search field, applied on Enter, with its history popup;
    - the filter row in P§5.2's order;
    - the quick-filter popup with `Clear {0} Filters`;
    - the outdated bar `The list is outdated.` with `Refresh`;
    - the rows of P§5.5 without the dot (decision 7). Avatars are initials, the label icon has its tooltip list, and the comment counter is always shown;
    - line 2 is `#<n> · created <date>, by <login>`, with `formatPrettyDate`;
    - Enter or a double click opens the PR tab;
    - paging at 70% scroll;
    - the empty and error states of P§5.6 with `Re-Login` / `Retry`;
    - the context menu of P§5.9.
  - `src/pulls/search.ts`: the quick filters, the filter count and the short text.
  - `src/pulls/poll.ts`: the ETag check every 30 s, only while the window is active and the app has focus (overview decision 16). The first answer only sets the baseline, and a reload resets it.
- Test: `src-tauri/src/hosting/pulls/{list,reviewers}.rs`, `src/pulls/List.test.ts`, `src/pulls/search.test.ts`, `src/pulls/poll.test.ts`.

**Interfaces:**
- Consumes: Task 6's connection, Task 3's `loginFor`.
- Produces: `PullRequestShort`, `reviewers::map`, and `formatPrettyDate` in `src/lib/dates.ts`, if none exists yet. Task 8 uses it.

- [ ] **Step 1: Write the failing tests:**
  - `the_search_string_is_241s`: every filter, free text first, `label:"…"` quoted, the API path.
  - `the_default_search_is_assigned_to_you_unless_one_was_saved`, and `clear_filters_sets_the_empty_search`.
  - `the_filter_dropdowns_are_in_241s_order_with_their_values`.
  - `rows_show_state_tags_conflict_icon_and_comment_count_even_at_zero`.
  - `a_title_with_html_is_text_and_a_bad_label_colour_is_dropped` (Review Focus 4).
  - `pages_load_at_seventy_percent_and_stop_after_an_error`.
  - `the_list_is_outdated_only_after_a_changed_etag_and_polls_only_while_shown_and_focused`.
  - `errors_show_cant_load_with_re_login_for_auth_and_retry_otherwise`.
  - `empty_says_no_pull_requests_in_repo_or_matching_filters`.
  - `the_reviewer_map_follows_241`: last review wins, a request overrides, the author is excluded, and bots become `ghost`.
- [ ] **Step 2: Check that they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Check that they pass.**
- [ ] **Step 5: Commit.** `git commit -m "feat: the pull request list with search, filters and quick filters"`

### Task 8: Pull request details, status, commits and changes

Research: P§6, Appendix A.2-A.4, M§6. Decisions 7, 9 and 13.

**Files:**
- Create:
  - `src-tauri/src/hosting/pulls/details.rs`:
    - `details(ctx, conn, number)` through `findPullRequest`: title, body, url, state, draft, base and head names and oids, head repository (owner, `isFork`, `nameWithOwner`, url, sshUrl), and the reviewers;
    - `status(ctx, conn, number)` through `findPullRequestMergeability`, built as `GHPRMergeabilityStateBuilder` (P§6.3): jobs and their states, conflicts, the required approvals, the restriction;
    - `commits(ctx, conn, number)` through `pullRequestCommits`, all pages, in P§6.5's order.
  - `src-tauri/src/hosting/pulls/changes.rs`:
    - `fetch(ctx, sync, repo, remote, base_ref, number)`: `git fetch <remote> <base_ref> --recurse-submodules=no --progress --prune`, then `git fetch <remote> refs/pull/<n>/head: …`. It runs inside `ctx.on_remote(remote, …)`, and calls `BranchSync::fetched` after each (Task 5). `base_ref` goes through `git check-ref-format --branch`;
    - `changes(repo, base_oid, head_oid, commits, selection: Option<usize>) -> Vec<ChangeWithSides>`: `merge-base`, whole PR = merge base → head, and per commit with P§6.8's `before` rule. Hashes go through `diff::check_hash`, and git calls through `git::exact`.
    - Add a `pub fn merge_base(repo, a, b) -> Result<Option<String>, GitError>`, from `conflicts.rs`'s private one (M§6).
  - `src-tauri/src/hosting/graphql/{findPullRequest,findPullRequestMergeability,pullRequestCommits}.graphql`.
  - Commands: `github_pr_details(number)`, `github_pr_status(number)`, `github_pr_commits(number)` and `github_pr_changes(number, base_oid, head_oid, selection)`. They are tasks, and the last one fetches first.
  - `src/pulls/Details.svelte`, with P§6.2's layout without the timeline or the actions:
    - **Title row:** the title, then `#<n>` as a link with the tooltip `Open on GitHub`, and the state tag.
    - **Description:** the body as plain text in a `pre-wrap` block, or `No description provided` in grey italic (`GB:264`).
    - **Commits and branch row:** the commits dropdown (`Changes` / `Changes from`, `{0} commits`, `All commits ({0})`, `Search commits`, next and previous), then the branch link (Task 9 fills its popup).
    - **Commit info:** the selected commit with `Show details` / `Hide details`. The body is read from git after the fetch, because the API has only HTML.
    - **Dates:** the commit rows and the commit info use rama's existing pretty date-time (`src/log/draw.ts:234-243`). Check its one-hour bound against 241's `HOUR + MINUTE` (P§6.5, `DateFormatUtil.java:167`) and fix it if the 241 source says so, with a test.
    - **Changes:** the tree, reusing `ChangesTree`, `fileNodes` and `pairOf` from `changes/ChangesToolWindow.svelte`. Extract them into a shared module so the `Changes` window keeps working.
      - Double click and `Show Diff` open `diffWindowOpenRequest({ kind: "revisions", title: "Diff for Pull Request #<n>" })`.
      - Its menu follows P§6.8 without `Mark as Viewed`.
    - **Status:** the lines of P§6.3 in order, without the access lines (decision 7):
      - CI with `Details…`, which opens `Open in Browser` with the jobs;
      - conflicts, required reviews and the restriction;
      - `Missing reviewer`, and the reviewer lines.
    - **Context menu:** `Refresh Pull Request`, `Open Pull Request in Browser` and `Copy Pull Request URL`.
    - **States:** loading and failure as in P§6.2, P§6.9 and P§9.3. `Can't load details` and `Can't load changes` come with `Retry` / `Re-Login`, and `Pull request does not contain any changes`.
- Test: `src-tauri/src/hosting/pulls/{details,changes}.rs`, `src/pulls/Details.test.ts`, `src/changes/ChangesToolWindow.test.ts` (it still passes after the extraction).

**Interfaces:**
- Consumes: Tasks 5-7; `compare::changes_between`, `diff.rs`, `diffWindowOpenRequest`.
- Produces: `PullRequestDetails { … head_repository: Option<HeadRepository { owner, is_fork, name_with_owner, url, ssh_url }> }`. Task 9 reads it.

- [ ] **Step 1: Write the failing tests:**
  - `changes_wait_for_both_fetches_and_use_the_merge_base`, with a bare remote holding `refs/pull/7/head`, a base branch that moved on after the fork point, and no local ref for the PR afterwards.
  - `per_commit_changes_follow_241s_before_rule`, with a linear PR and with a merge inside the PR.
  - `a_failed_fetch_shows_cant_load_changes_and_retry_fetches_again`.
  - `the_ci_line_follows_241`, one case per first-match rule of P§6.3, including `All required checks have passed` with nothing required and a failure.
  - `required_approvals_show_only_when_blocked_and_not_failing`.
  - `the_description_is_plain_text_and_a_check_url_must_be_https` (Review Focus 4).
  - `the_diff_title_is_diff_for_pull_request_n`.
  - `an_answer_for_an_old_head_or_another_repository_is_dropped` (Review Focus 5).
  - `refresh_reloads_details_status_and_changes_for_the_new_head`.
- [ ] **Step 2: Check that they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Check that they pass.**
- [ ] **Step 5: Commit.** `git commit -m "feat: pull request details with status, commits and local changes"`

### Task 9: Checkout and Show in Git Log

Research: P§6.6, P§7, M§5 (log tabs), M§7. Overview decision 17.

**Files:**
- Create:
  - `src-tauri/src/hosting/pulls/checkout.rs`:
    - `find_remote(remotes, server, path)`, P§7.2 step 1: the first fetch URL, case-sensitive `endsWith`;
    - `remote_name(existing, login)`: `<login>`, else the first free `<login>_1`, `<login>_2`… (decision 17);
    - `remote_url(remotes, head)`: HTTP when `origin`'s or the first remote's URL starts with `http`, or there are no remotes, else SSH (P§7.4);
    - `prepare(ctx, sync, repo, details) -> Prepared { remote, branch: "<remote>/<head>", suggested }`:
      - find or add the remote with `remotes::add`;
      - `git fetch <remote> <headRefName> --recurse-submodules=no --progress --prune` inside `ctx.on_remote`, then `BranchSync::fetched`;
      - the suggested name: the tracking branch, else `fork/<head>` for a fork, else `<head>`.
  - Commands:
    - `github_pr_checkout(number)`, which calls `flush_diff_window` first, is a `TaskKind::Write` task, and adds the command to `whatever_touches_the_repository_saves_the_diff_window_first`. It runs `prepare` and then `branches::checkout_remote(repo, branch, RemoteOptions { name: Some(suggested), … })`, so the recoverable copy and the existing dialogs apply. The `NeedsName` and `LocalHasCommits` answers reach the interface as today.
    - `github_pr_show_in_log(number) -> Range { from, to }`.
    - `github_pr_checked_out(number) -> bool` (P§7.6).
  - `src/pulls/branch.ts`: the branch popup (`Checkout '{0}'…`, `Show '{0}' in Git Log`, ad text `{0} ← {1}`) and the flows. They reuse `branches/actions.ts`'s `checkoutRemote` answers (name dialog, local commits) with the suggested name.
- Modify:
  - `src/pulls/Details.svelte`: the branch label (`owner:branch` rule, P§6.6) with the checked-out icon, recomputed on `repo-changed`.
  - `src/log/tabKinds.ts`: a `"range"` kind for `Show in Git Log`, `persisted: false`, `reused: true`, titled with the range text `<remote>/<base>..<remote>/<head>`. It opens with `ranges: [[from, to]]`.
- Notifications:
  - no remote: `Branch checkout failed` / `Cannot resolve git remote for merge request source repository` plus the URLs (`CTB:73-74`);
  - a fetch failure: `Fetch Failed` plus git's output (`GitB:1387`);
  - a deleted fork: nothing, as in 241.
- Test: `src-tauri/src/hosting/pulls/checkout.rs`, `src/pulls/branch.test.ts`, `src/log/tabKinds.test.ts`.

**Interfaces:**
- Consumes: Task 8's `PullRequestDetails`; `remotes::list`, `remotes::add`, `branches::checkout_remote`, `openLogTab`.
- Produces: nothing new for later tasks.

- [ ] **Step 1: Write the failing tests:**
  - `checkout_finds_the_heads_remote_or_adds_one_named_after_its_owner`.
  - `a_taken_name_becomes_login_1_then_login_2` (decision 17). 241 fails here.
  - `http_or_ssh_follows_origin`.
  - `checkout_fetches_the_head_branch_and_keeps_a_recoverable_copy`. The test checks `RepoHarness::snapshots()` grows when a local edit is at risk, and that a fork suggests `fork/<head>`.
  - `a_head_branch_named_dash_x_never_reaches_git_as_an_option` (Review Focus 4).
  - `show_in_git_log_opens_a_range_tab_from_the_heads_remote`.
  - `the_checked_out_icon_follows_repo_changed` (Review Focus 5).
  - `a_failed_fetch_says_fetch_failed_with_gits_output`.
- [ ] **Step 2: Check that they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Check that they pass.**
- [ ] **Step 5: Commit.** `git commit -m "feat: checkout of a pull request branch and Show in Git Log"`

### Task 10: Matrix, report and full verification

**Files:**
- Modify:
  - `docs/parity-matrix.md`:
    - the `Pull requests…` row and the protected-branches row get their results and tests;
    - new rows for every `Github.*`/`GitHub.*` action in scope, with their result;
    - the out-of-scope ones say why: OAuth, Create Pull Request, review, Gist, Share, Sync Fork, clone, Review Mode, Git HTTP auth;
    - the counts sentence covers part 5.
  - `docs/reports/phase-4-validation.md`, a Part 5 section in the shape of the others:
    - decisions with their costs and the rulings from the ledger;
    - tests by feature;
    - what could not be checked;
    - the manual checklist:
      - a real github.com account with a real token, and the Keychain item after a restart;
      - the list, filters and paging on a real repository with PRs;
      - a PR from a fork: details, status and changes;
      - checkout of that PR and Show in Git Log;
      - a force push to a protected branch after Fetch;
      - Open on GitHub from each menu;
      - an unsigned dev build asking for Keychain access.
  - `NOTICE`: the ported classes and the GraphQL texts.
- [ ] **Step 1:** Each `done` row cites its test, and the test exists in `src/`, `src-tauri/src` or `src-tauri/tests`. Check with grep.
- [ ] **Step 2: Full verification, one suite at a time:** tsc, full vitest, `cargo test --lib`, `cargo test --no-fail-fast --test '*'`, clippy `--all-targets -D warnings`, python. The report writes the exact counts.
- [ ] **Step 3: Commit.** `git commit -m "docs: phase 4 part 5 results in the matrix and the report"`
