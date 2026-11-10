// The protected branch rules loaded from GitHub after each fetch. Ports GHProtectedBranchRulesLoader
// (plugins/github, extensions/GHProtectedBranchRulesLoader.kt) and PatternUtil.convertToRegex
// (platform/util) at idea/241.14494.240. Ramify's own choice is decision 6 of the part 5 plan: the
// rules are kept per remote in `<repo data>/github-protection.json`, and a fetch of a remote
// replaces only that remote's entry.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use serde_json::{json, Value};

use crate::fetch;
use crate::remotes::{self, Remote};
use crate::secrets::SecretStore;
use crate::settings;
use crate::storage;
use crate::task::TaskContext;

use super::accounts::{self, GithubAccount};
use super::http::{Client, HostingError, PARSE_FAILED};
use super::mapping::mappings;
use super::server::GithubServer;

pub(crate) const FILE_NAME: &str = "github-protection.json";
const CORRUPT_FILE_NAME: &str = "github-protection.json.corrupt";
// GHGQLRequests.Repo.getProtectionRules, as 241 sends it (P Appendix A.5).
const QUERY: &str = include_str!("graphql/getProtectionRules.graphql");
// GithubRequestPagination.DEFAULT_PAGE_SIZE.
const PAGE_SIZE: u32 = 100;
// GithubBundle 338.
const LOADING: &str = "Loading GitHub branch protection rules";

// Each remote's rules, already converted to regexes, as 241 stores them.
pub type Rules = BTreeMap<String, Vec<String>>;

// Reading, changing and writing the file is one step: two fetches that finish at once (Fetch and an
// `Update` of a branch) must not lose one remote's entry.
static SAVING: Mutex<()> = Mutex::new(());

// `PatternUtil.convertToRegex`: split on `\` without the empty pieces, `.` escaped first, then the
// rules in their order, and the pieces joined by an escaped backslash.
pub fn convert_to_regex(mask: &str) -> String {
    const RULES: [(&str, &str); 13] = [
        ("*", ".*"),
        ("?", "."),
        ("+", "\\+"),
        ("(", "\\("),
        (")", "\\)"),
        ("[", "\\["),
        ("]", "\\]"),
        ("/", "\\/"),
        ("^", "\\^"),
        ("$", "\\$"),
        ("{", "\\{"),
        ("}", "\\}"),
        ("|", "\\|"),
    ];
    mask.split('\\')
        .filter(|piece| !piece.is_empty())
        .map(|piece| {
            let escaped = piece.replace('.', "\\.");
            RULES
                .iter()
                .fold(escaped, |text, (key, value)| text.replace(key, value))
        })
        .collect::<Vec<_>>()
        .join("\\\\")
}

fn stored(data: &Path) -> io::Result<Rules> {
    storage::load_json(data, FILE_NAME, CORRUPT_FILE_NAME)
}

// Every remote's rules, without repeats. Strict: a file that cannot be read is an error.
pub fn try_loaded(data: &Path) -> io::Result<Vec<String>> {
    let mut patterns: Vec<String> = Vec::new();
    for pattern in stored(data)?.into_values().flatten() {
        if !patterns.contains(&pattern) {
            patterns.push(pattern);
        }
    }
    Ok(patterns)
}

// The same, lenient: a file that cannot be read means no rules.
pub fn loaded(data: &Path) -> Vec<String> {
    try_loaded(data).unwrap_or_default()
}

// `GHProtectedBranchRulesLoader.doAfterSuccessfulFetch` for one fetched remote, inside the fetch's
// task and after git. Nothing it meets reaches the fetch: an error keeps the remote's entry and
// is only logged, without the token. The task gets back the progress text it showed before.
pub fn load(
    ctx: &TaskContext,
    client: &Client,
    store: &dyn SecretStore,
    base: &Path,
    data: &Path,
    repo: &Path,
    remote: &str,
) {
    let before = ctx.progress();
    let replaced = replace_entry(ctx, client, store, base, data, repo, remote);
    ctx.restore_progress(before);
    if let Err(error) = replaced {
        if error != HostingError::Cancelled {
            eprintln!("{}", failure_line(remote, &error));
        }
    }
}

fn replace_entry(
    ctx: &TaskContext,
    client: &Client,
    store: &dyn SecretStore,
    base: &Path,
    data: &Path,
    repo: &Path,
    remote: &str,
) -> Result<(), HostingError> {
    let io_failure = |error: io::Error| HostingError::Io(error.to_string());
    // `isSynchronizeBranchProtectionRules` off: 241 empties the whole list.
    if !settings::load_repo(data).load_github_protection {
        return save(data, Rules::clear).map_err(io_failure);
    }
    let remotes =
        remotes::list(repo).map_err(|error| HostingError::Io(fetch::error_text(error)))?;
    let found = rules_of(ctx, client, store, base, data, &remotes, remote)?;
    save(data, |rules| {
        // Decision 6: the entries of the remotes that are gone go too.
        rules.retain(|name, _| remotes.iter().any(|known| known.name == *name));
        match found {
            Some(patterns) if !patterns.is_empty() => {
                rules.insert(remote.to_string(), patterns);
            }
            _ => {
                rules.remove(remote);
            }
        }
    })
    .map_err(io_failure)
}

// 241's INFO line. `HostingError` never carries the token, the request's headers or its URL.
fn failure_line(remote: &str, error: &HostingError) -> String {
    format!("Could not load the GitHub branch protection rules of {remote}: {error}")
}

// The remote's rules: `None` when it does not point to a known server's repository, or when no
// account on that server has a token.
fn rules_of(
    ctx: &TaskContext,
    client: &Client,
    store: &dyn SecretStore,
    base: &Path,
    data: &Path,
    remotes: &[Remote],
    remote: &str,
) -> Result<Option<Vec<String>>, HostingError> {
    let list = accounts::list(base).map_err(|error| HostingError::Io(error.to_string()))?;
    if list.is_empty() {
        return Ok(None);
    }
    ctx.set_text(LOADING);
    let Some(mapping) = mappings(remotes, &accounts::known_servers(&list))
        .into_iter()
        .find(|mapping| mapping.remote == remote)
    else {
        return Ok(None);
    };
    let coordinates = &mapping.coordinates;
    let default = settings::load_repo(data).github_default_account;
    let Some(account) = account_for(&list, default.as_deref(), &coordinates.server) else {
        return Ok(None);
    };
    let Some(token) = accounts::token(store, account) else {
        return Ok(None);
    };
    let variables = json!({
        "repoOwner": coordinates.owner,
        "repoName": coordinates.name,
        "pageSize": PAGE_SIZE,
    });
    let check = || ctx.check_cancelled().map_err(|_| HostingError::Cancelled);
    let nodes = client.graphql_pages(
        &coordinates.server.graphql_url(),
        Some(&token),
        None,
        QUERY,
        &variables,
        &["repository", "branchProtectionRules"],
        &check,
    )?;
    let mut patterns: Vec<String> = Vec::new();
    for node in nodes {
        let pattern = node
            .get("pattern")
            .and_then(Value::as_str)
            .ok_or_else(|| HostingError::Io(PARSE_FAILED.to_string()))?;
        let regex = convert_to_regex(pattern);
        if !patterns.contains(&regex) {
            patterns.push(regex);
        }
    }
    Ok(Some(patterns))
}

// The repository's default account when it is on that server, else the first account on it.
fn account_for<'a>(
    list: &'a [GithubAccount],
    default: Option<&str>,
    server: &GithubServer,
) -> Option<&'a GithubAccount> {
    let on_server = |account: &&GithubAccount| account.server.same_ignoring_scheme(server);
    default
        .and_then(|id| list.iter().find(|account| account.id() == id))
        .filter(on_server)
        .or_else(|| list.iter().find(on_server))
}

// Reads, changes and writes the file. It is written only when something changed, so a fetch with
// nothing to store leaves no new file behind.
fn save(data: &Path, change: impl FnOnce(&mut Rules)) -> io::Result<()> {
    let _saving = SAVING.lock().unwrap_or_else(PoisonError::into_inner);
    let before = stored(data)?;
    let mut rules = before.clone();
    change(&mut rules);
    if rules == before {
        return Ok(());
    }
    fs::create_dir_all(data)?;
    let json = serde_json::to_vec_pretty(&rules).map_err(io::Error::other)?;
    storage::write_atomic(&data.join(FILE_NAME), &json)
}

// What the tests of the fetches need: an account on the fake GitHub (or on a server where nothing
// listens), a remote whose URL points to its `o/r` while git still fetches from a local bare
// repository, and the hook that the app's setup installs.
#[cfg(test)]
pub(crate) mod testing {
    use std::net::TcpListener;
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    use serde_json::{json, Value};
    use tempfile::TempDir;

    use super::*;
    use crate::branch_info::AfterFetch;
    use crate::hosting::accounts::{self, GithubAccount};
    use crate::hosting::server::GithubServer;
    use crate::secrets::{MemoryStore, Secret, SecretStore};
    use crate::test_support::{git_command, FakeGitHub, Reply};

    pub const TOKEN: &str = "ghp_PROTECTION0123456789";

    pub struct GithubRemote {
        pub fake: FakeGitHub,
        pub server: GithubServer,
        // The app's data folder, with the accounts list.
        pub base: TempDir,
        pub store: Arc<MemoryStore>,
    }

    impl GithubRemote {
        // `octocat` on the fake GitHub, with `TOKEN`.
        pub fn new() -> Self {
            let fake = FakeGitHub::new();
            let server = GithubServer::parse(&fake.url()).unwrap();
            Self::on(fake, server)
        }

        // `octocat` on a port that was free a moment ago: nothing listens there.
        pub fn unreachable() -> Self {
            let port = TcpListener::bind("127.0.0.1:0")
                .unwrap()
                .local_addr()
                .unwrap()
                .port();
            let server = GithubServer::parse(&format!("http://127.0.0.1:{port}")).unwrap();
            Self::on(FakeGitHub::new(), server)
        }

        fn on(fake: FakeGitHub, server: GithubServer) -> Self {
            let github = Self {
                fake,
                server,
                base: TempDir::new().unwrap(),
                store: Arc::new(MemoryStore::new()),
            };
            github.add_account(&github.server.clone(), "octocat", TOKEN);
            github
        }

        pub fn add_account(
            &self,
            server: &GithubServer,
            login: &str,
            token: &str,
        ) -> GithubAccount {
            let account = GithubAccount {
                server: server.clone(),
                login: login.to_string(),
            };
            let mut list = accounts::list(self.base.path()).unwrap();
            list.push(account.clone());
            accounts::save(self.base.path(), &list).unwrap();
            let secret = Secret::new(token.to_string());
            self.store
                .set(&accounts::service(server), login, &secret)
                .unwrap();
            account
        }

        // The repository `o/r` on the server.
        pub fn url(&self) -> String {
            format!("{}/o/r.git", self.server.web_url())
        }

        // `remote` of `repo` reads `url()`, and git fetches and pushes it from `bare`.
        pub fn point(&self, repo: &Path, remote: &str, bare: &Path) {
            let url = self.url();
            git(repo, &["remote", "set-url", remote, &url]);
            let key = format!("url.{}.insteadOf", bare.display());
            git(repo, &["config", &key, &url]);
        }

        // What `lib.rs` installs, with this account list and token store, and `data` as the
        // repository's data folder.
        pub fn hook(&self, data: &Path) -> AfterFetch {
            let client = Client::new(Duration::from_secs(5));
            let store = self.store.clone();
            let base = self.base.path().to_path_buf();
            let data = data.to_path_buf();
            Arc::new(move |ctx, repo, remote| {
                load(ctx, &client, store.as_ref(), &base, &data, repo, remote)
            })
        }
    }

    // One page of `getProtectionRules`; `next` is the cursor of the following one.
    pub fn page(patterns: &[&str], next: Option<&str>) -> Reply {
        let nodes: Vec<Value> = patterns
            .iter()
            .enumerate()
            .map(|(index, pattern)| json!({ "id": format!("BPR_{index}"), "pattern": pattern }))
            .collect();
        let body = json!({
            "data": {
                "repository": {
                    "branchProtectionRules": {
                        "pageInfo": {
                            "startCursor": null,
                            "hasPreviousPage": false,
                            "endCursor": next,
                            "hasNextPage": next.is_some(),
                        },
                        "nodes": nodes,
                    }
                }
            }
        });
        Reply::json("POST", "/api/graphql", 200, "OK", &body.to_string())
    }

    // The only page.
    pub fn rules(patterns: &[&str]) -> Reply {
        page(patterns, None)
    }

    pub fn seed(data: &Path, entries: &[(&str, &[&str])]) {
        let rules: Rules = entries
            .iter()
            .map(|(remote, patterns)| {
                let patterns = patterns.iter().map(|pattern| pattern.to_string()).collect();
                (remote.to_string(), patterns)
            })
            .collect();
        std::fs::write(
            data.join(FILE_NAME),
            serde_json::to_vec_pretty(&rules).unwrap(),
        )
        .unwrap();
    }

    pub fn entries(data: &Path) -> Vec<(String, Vec<String>)> {
        stored(data).unwrap().into_iter().collect()
    }

    pub fn entry(remote: &str, patterns: &[&str]) -> (String, Vec<String>) {
        let patterns = patterns.iter().map(|pattern| pattern.to_string()).collect();
        (remote.to_string(), patterns)
    }

    fn git(repo: &Path, args: &[&str]) {
        let status = git_command().current_dir(repo).args(args).status().unwrap();
        assert!(status.success(), "git {args:?}");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{mpsc, Arc, Mutex};
    use std::time::Duration;

    use serde_json::{json, Value};
    use tempfile::TempDir;

    use super::testing::{entries, entry, page, rules, seed, GithubRemote, TOKEN};
    use super::*;
    use crate::branch_info::BranchSync;
    use crate::branches::Repo;
    use crate::commands::RepoLocks;
    use crate::console::Console;
    use crate::fetch;
    use crate::hosting::http::HostingError;
    use crate::hosting::server::GithubServer;
    use crate::pull;
    use crate::settings::{self, UpdateMethod};
    use crate::task::{Runner, TaskKind};
    use crate::test_support::{git_command, in_task, Reply, TempRepo};
    use crate::update::{self, Outcome, UpdateOptions};

    const SECOND_TOKEN: &str = "ghp_SECONDPROTECTION0123";

    // A clone of a bare `origin` whose URL points to the GitHub of `github`.
    struct Harness {
        github: GithubRemote,
        repo: TempRepo,
        origin: TempDir,
        data: TempDir,
        console: Arc<Console>,
        runner: Runner,
        sync: Arc<BranchSync>,
    }

    fn bare() -> TempDir {
        let dir = TempDir::new().unwrap();
        let status = git_command()
            .args(["init", "-q", "--bare", "-b", "main"])
            .arg(dir.path())
            .status()
            .unwrap();
        assert!(status.success());
        dir
    }

    impl Harness {
        fn new() -> Self {
            Self::with(GithubRemote::new())
        }

        fn with(github: GithubRemote) -> Self {
            let origin = bare();
            let repo = TempRepo::new();
            repo.commit("a.txt", b"1\n", "one");
            repo.git(&["remote", "add", "origin", origin.path().to_str().unwrap()]);
            repo.git(&["push", "-q", "-u", "origin", "main"]);
            github.point(repo.path(), "origin", origin.path());
            let console = Arc::<Console>::default();
            let runner = Runner::with(
                Arc::<RepoLocks>::default(),
                console.clone(),
                None,
                Duration::from_secs(5),
            );
            let data = TempDir::new().unwrap();
            let sync = Arc::new(BranchSync::default());
            sync.set_after_fetch(github.hook(data.path()));
            Self {
                github,
                repo,
                origin,
                data,
                console,
                runner,
                sync,
            }
        }

        // In a task of the kind the command would start.
        fn run<T: Send + 'static>(
            &self,
            kind: TaskKind,
            work: impl FnOnce(&Repo, &BranchSync) -> T + Send + 'static,
        ) -> T {
            let (send, receive) = mpsc::channel();
            let sync = self.sync.clone();
            let path = self.repo.path().to_path_buf();
            let data = self.data.path().to_path_buf();
            self.runner
                .start(self.repo.path(), kind, "test".to_string(), move |ctx| {
                    let repo = Repo {
                        ctx,
                        path: &path,
                        data: &data,
                    };
                    send.send(work(&repo, &sync)).unwrap();
                    Ok(())
                });
            receive.recv_timeout(Duration::from_secs(60)).unwrap()
        }

        fn fetch(&self, remote: &str) -> Result<Vec<String>, crate::git::GitError> {
            let remote = remote.to_string();
            self.run(TaskKind::Read, move |repo, sync| {
                fetch::fetch_remote(repo.ctx, sync, &remote)
            })
        }

        // Someone else pushes a commit to `main`.
        fn colleague_pushes(&self) {
            let colleague = TempRepo::new();
            let url = self.origin.path().to_str().unwrap();
            colleague.git(&["remote", "add", "origin", url]);
            colleague.git(&["fetch", "-q", "origin"]);
            colleague.git(&["checkout", "-q", "-B", "main", "origin/main"]);
            colleague.commit("theirs.txt", b"t\n", "from outside");
            colleague.git(&["push", "-q", "origin", "main"]);
        }

        fn console_text(&self) -> String {
            let lines = self.console.lines();
            let texts: Vec<String> = lines.into_iter().map(|line| line.text).collect();
            texts.join("\n")
        }
    }

    fn body(request: &crate::test_support::Recorded) -> Value {
        serde_json::from_str(&request.body).unwrap()
    }

    #[test]
    fn convert_to_regex_is_241s() {
        // P§4.2's examples.
        assert_eq!(convert_to_regex("main"), "main");
        assert_eq!(convert_to_regex("release/*"), r"release\/.*");
        assert_eq!(convert_to_regex("v1.?"), r"v1\..");
        assert_eq!(convert_to_regex("feat-[0-9]+"), r"feat-\[0-9\]\+");
        // `StringUtil.split` drops the empty pieces: two backslashes are one literal backslash.
        assert_eq!(convert_to_regex(r"a\\b"), r"a\\b");
        assert_eq!(convert_to_regex(r"\a\"), "a");
        // `*` is replaced on its own, and GitHub's classes become literal brackets.
        assert_eq!(convert_to_regex("**"), ".*.*");
        assert_eq!(convert_to_regex("[0-9]"), r"\[0-9\]");
        assert_eq!(convert_to_regex("^a$|{b}()"), r"\^a\$\|\{b\}\(\)");
    }

    #[test]
    fn every_page_is_loaded_with_the_default_account_else_the_first_on_that_server() {
        let harness = Harness::new();
        let github = &harness.github;
        github.add_account(&github.server, "hubber", SECOND_TOKEN);
        let other = GithubServer::parse("ghe.example.com").unwrap();
        github.add_account(&other, "alice", "ghp_OTHERSERVER0123456789");
        let default = format!("{}/hubber", github.server.address());
        settings::save_github_default_account(harness.data.path(), Some(default)).unwrap();
        let texts = Arc::new(Mutex::new(Vec::new()));
        let seen = texts.clone();
        harness
            .runner
            .listen(move |event| seen.lock().unwrap().push(event.text.clone()));

        github.fake.script(vec![
            page(&["main", "release/*"], Some("CURSOR1")),
            page(&["v1.?", "main"], None),
        ]);
        harness.fetch("origin").unwrap();
        github.fake.finish();

        assert_eq!(
            entries(harness.data.path()),
            [entry("origin", &["main", r"release\/.*", r"v1\.."])]
        );
        let requests = github.fake.requests();
        assert_eq!(requests.len(), 2);
        for (request, cursor) in requests.iter().zip([Value::Null, json!("CURSOR1")]) {
            assert_eq!(
                request.header("Authorization"),
                Some(format!("Bearer {SECOND_TOKEN}").as_str())
            );
            let sent = body(request);
            assert_eq!(sent["query"], QUERY);
            assert_eq!(
                sent["variables"],
                json!({ "repoOwner": "o", "repoName": "r", "pageSize": 100, "cursor": cursor })
            );
        }
        assert!(texts
            .lock()
            .unwrap()
            .iter()
            .any(|text| text == "Loading GitHub branch protection rules"));

        // A default account on another server: the first account on this one.
        let elsewhere = format!("{}/alice", other.address());
        settings::save_github_default_account(harness.data.path(), Some(elsewhere)).unwrap();
        github.fake.reply(rules(&["hotfix"]));
        harness.fetch("origin").unwrap();
        github.fake.finish();
        let last = github.fake.requests().pop().unwrap();
        assert_eq!(
            last.header("Authorization"),
            Some(format!("Bearer {TOKEN}").as_str())
        );
        assert_eq!(entries(harness.data.path()), [entry("origin", &["hotfix"])]);
    }

    // The loader shows its own text while it asks GitHub, then gives the task back the text it had
    // (as `shelve_watching` does), so a fetch's or a pull's progress does not end on GitHub's.
    #[test]
    fn the_loader_puts_back_the_progress_text_the_task_showed_before() {
        let harness = Harness::new();
        let texts = Arc::new(Mutex::new(Vec::new()));
        let seen = texts.clone();
        harness
            .runner
            .listen(move |event| seen.lock().unwrap().push(event.text.clone()));
        let hook = harness.github.hook(harness.data.path());
        let path = harness.repo.path().to_path_buf();

        harness.github.fake.reply(rules(&["main"]));
        in_task(&harness.runner, harness.repo.path(), move |ctx| {
            ctx.set_text("Fetching origin");
            hook(ctx, &path, "origin");
        });
        harness.github.fake.finish();

        let texts = texts.lock().unwrap().clone();
        let loading = texts
            .iter()
            .position(|text| text == LOADING)
            .expect("the loader showed its text");
        assert_eq!(
            texts[loading + 1..].first().map(String::as_str),
            Some("Fetching origin"),
            "{texts:?}"
        );
        assert_eq!(entries(harness.data.path()), [entry("origin", &["main"])]);
    }

    #[test]
    fn the_checkbox_off_ignores_the_loaded_rules_and_the_next_fetch_clears_them() {
        let harness = Harness::new();
        let data = harness.data.path();
        seed(data, &[("origin", &[r"release\/.*"])]);
        let defaults_and = |extra: &[&str]| {
            let mut patterns = vec!["master".to_string(), "main".to_string()];
            patterns.extend(extra.iter().map(|pattern| pattern.to_string()));
            patterns
        };
        assert_eq!(
            settings::effective_protected(data),
            defaults_and(&[r"release\/.*"])
        );
        assert_eq!(
            settings::try_effective_protected(data).unwrap(),
            defaults_and(&[r"release\/.*"])
        );

        settings::save_load_github_protection(data, false).unwrap();
        assert_eq!(settings::effective_protected(data), defaults_and(&[]));
        assert_eq!(
            settings::try_effective_protected(data).unwrap(),
            defaults_and(&[])
        );

        // No request: the fake has nothing queued and would fail the test.
        harness.fetch("origin").unwrap();
        harness.github.fake.finish();
        assert!(harness.github.fake.requests().is_empty());
        assert!(entries(data).is_empty());

        settings::save_load_github_protection(data, true).unwrap();
        assert_eq!(settings::effective_protected(data), defaults_and(&[]));
    }

    #[test]
    fn pull_merge_and_update_branch_also_load_the_rules() {
        let harness = Harness::new();
        harness.repo.git(&["branch", "side"]);
        harness.colleague_pushes();

        // `Update` of a branch that is not the current one.
        harness.github.fake.reply(rules(&["hotfix/*"]));
        harness
            .run(TaskKind::Remote("origin".to_string()), |repo, sync| {
                update::update_branch(repo.ctx, sync, "side", "origin", "main")
            })
            .unwrap();
        harness.github.fake.finish();
        assert_eq!(
            entries(harness.data.path()),
            [entry("origin", &[r"hotfix\/.*"])]
        );

        // `Pull…` with merge, which runs `git pull`.
        harness.github.fake.reply(rules(&["release/*"]));
        let result = harness.run(TaskKind::Read, |repo, sync| {
            pull::pull(repo, sync, "origin", "main", &[], || Ok(()))
        });
        harness.github.fake.finish();
        assert_eq!(result.outcome, Outcome::Success);
        assert_eq!(
            entries(harness.data.path()),
            [entry("origin", &[r"release\/.*"])]
        );
    }

    #[test]
    fn an_unreachable_server_does_not_fail_update_project() {
        let harness = Harness::with(GithubRemote::unreachable());
        seed(harness.data.path(), &[("origin", &["main"])]);
        harness.colleague_pushes();

        let result = harness.run(TaskKind::Read, |repo, sync| {
            update::update_project(
                repo,
                sync,
                UpdateMethod::Merge,
                UpdateOptions::default(),
                || Ok(()),
            )
        });

        assert_eq!(result.outcome, Outcome::Success);
        assert_eq!(result.commits, 1);
        assert_eq!(entries(harness.data.path()), [entry("origin", &["main"])]);
    }

    #[test]
    fn the_token_is_not_in_the_console_after_a_failed_load() {
        let harness = Harness::new();
        seed(harness.data.path(), &[("origin", &["main"])]);
        // GitHub's answers repeat the token: neither the console nor the log line may.
        let unauthorized = || {
            let body = format!(r#"{{"message":"Bad credentials: {TOKEN}"}}"#);
            Reply::json("POST", "/api/graphql", 401, "Unauthorized", &body)
        };
        let refused = || {
            let body = format!(r#"{{"errors":[{{"message":"Token {TOKEN} has no access"}}]}}"#);
            Reply::json("POST", "/api/graphql", 200, "OK", &body)
        };
        harness
            .github
            .fake
            .script(vec![unauthorized(), refused(), unauthorized(), refused()]);

        harness.fetch("origin").unwrap();
        harness.fetch("origin").unwrap();
        let path = harness.repo.path().to_path_buf();
        let data = harness.data.path().to_path_buf();
        let base = harness.github.base.path().to_path_buf();
        let store = harness.github.store.clone();
        let failures = in_task(&harness.runner, harness.repo.path(), move |ctx| {
            let client = Client::new(Duration::from_secs(5));
            let remotes = remotes::list(&path).unwrap();
            let load = || {
                rules_of(
                    ctx,
                    &client,
                    store.as_ref(),
                    &base,
                    &data,
                    &remotes,
                    "origin",
                )
                .unwrap_err()
            };
            [load(), load()]
        });
        harness.github.fake.finish();

        assert!(
            matches!(failures[0], HostingError::Authentication(_)),
            "{failures:?}"
        );
        assert!(
            matches!(failures[1], HostingError::GraphQl(_)),
            "{failures:?}"
        );
        for failure in &failures {
            let line = failure_line("origin", failure);
            assert!(
                line.starts_with("Could not load the GitHub branch protection rules of origin: ")
            );
            assert!(!line.contains(TOKEN), "{line}");
        }
        assert!(!harness.console_text().contains(TOKEN));
        assert_eq!(entries(harness.data.path()), [entry("origin", &["main"])]);
    }
}
