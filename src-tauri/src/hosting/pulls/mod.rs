// The Pull Requests tool window's connection: which GitHub repositories the remotes point to, which
// accounts can open them, and the connect itself. Ports GHPRToolWindowViewModel (the saved choice),
// GHPRDataContextRepository.getContextAsync (connect), GithubAccountInformationProvider (the
// 30-minute cache of `GET /user`) and GHUIUtil.getRepositoryDisplayName (plugins/github at
// idea/241.14494.240). rama's own choices are decisions 2, 4 and 5 of the part 5 plan: the default
// account plays no part, there is no server discovery, and connect skips `findUser("ghost")`.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::git::GitError;
use crate::remotes;
use crate::secrets::{Secret, SecretStore};
use crate::storage;

use super::accounts::{self, GithubAccount};
use super::commands::{listed_rows, AccountView};
use super::http::{Check, Client, HostingError, PARSE_FAILED};
use super::mapping::{mappings, GithubMapping, RepoCoordinates};

pub mod changes;
pub mod checkout;
pub mod connections;
pub mod details;
pub mod list;
pub mod reviewers;

const CHOICE_FILE: &str = "github-pr-connection.json";
const CORRUPT_CHOICE_FILE: &str = "github-pr-connection.json.corrupt";
// GHGQLRequests.Repo.find, as 241 sends it (P Appendix A.6).
const QUERY: &str = include_str!("../graphql/findRepository.graphql");
// GithubAccountInformationProvider: `expireAfterWrite(30 minutes)`.
pub const USER_LIFETIME: Duration = Duration::from_secs(30 * 60);
// GithubBundle 35.
const INVALID_SERVER: &str = "Server URL is not valid";

// A remote that points to a GitHub repository, as the repository combo shows it: `display` is
// `owner/repo`, with the server in front when the repositories are on several servers, and the
// remote's name goes after it in grey. `server` is the server as an account writes it, and
// `repository` is 241's `GHRepositoryCoordinates.toString`, which the connect error names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MappingView {
    pub remote: String,
    pub url: String,
    pub display: String,
    pub server: String,
    pub repository: String,
}

// What the last successful connect chose: the remote's URL and the account's id
// (`GithubPullRequestsProjectUISettings.selectedUrlAndAccount`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Choice {
    pub url: String,
    pub account: String,
}

// Everything the window and its selector start from, with no request to GitHub and no token read:
// the mappings in the combo's order (by `display`), every account by login, and the saved choice.
// Whether an account has a token is learned when connecting (`HostingError::MissingToken`): on an
// unsigned build every Keychain read can ask the user, and this is read on every `repo-changed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Known {
    pub mappings: Vec<MappingView>,
    pub accounts: Vec<AccountView>,
    pub saved: Option<Choice>,
}

// The fragment `repository` of `findRepository`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoInfo {
    pub id: String,
    pub owner: String,
    pub name_with_owner: String,
    pub viewer_permission: Option<String>,
    pub merge_commit_allowed: bool,
    pub squash_merge_allowed: bool,
    pub rebase_merge_allowed: bool,
    pub default_branch: Option<String>,
    pub is_fork: bool,
    pub url: String,
    pub ssh_url: String,
}

// An open connection. `coordinates` are the account's server and the repository path GitHub
// gave, which may differ from the remote's after a rename or a move; every later request uses
// them. `title` is the list tab's name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub remote: String,
    pub url: String,
    pub account: String,
    pub coordinates: RepoCoordinates,
    pub viewer_login: String,
    pub repository: RepoInfo,
    pub title: String,
}

// The logins `GET /user` gave, per account id, for 30 minutes. Any change to the accounts empties
// it, as `accountsState` does in 241.
#[derive(Default)]
pub struct UserCache {
    entries: Mutex<HashMap<String, (Instant, String)>>,
}

impl UserCache {
    pub fn login(
        &self,
        id: &str,
        now: Instant,
        load: impl FnOnce() -> Result<String, HostingError>,
    ) -> Result<String, HostingError> {
        let fresh = self
            .lock()
            .get(id)
            .filter(|(at, _)| now.saturating_duration_since(*at) < USER_LIFETIME)
            .map(|(_, login)| login.clone());
        if let Some(login) = fresh {
            return Ok(login);
        }
        let login = load()?;
        self.lock().insert(id.to_string(), (now, login.clone()));
        Ok(login)
    }

    pub fn clear(&self) {
        self.lock().clear();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, (Instant, String)>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

// `GHUIUtil.getRepositoryDisplayName`: the server (without its scheme) when the repositories are
// on several servers, the owner when they have several owners or `always_show_owner`, then the
// name. The servers compare with their scheme and the owners with their case, as in 241.
pub fn display_name(
    all: &[&RepoCoordinates],
    repository: &RepoCoordinates,
    always_show_owner: bool,
) -> String {
    let differ = |same: &dyn Fn(&RepoCoordinates, &RepoCoordinates) -> bool| {
        all.len() > 1 && all.iter().any(|other| !same(all[0], other))
    };
    let show_server = differ(&|first, other| first.server == other.server);
    let show_owner =
        show_server || always_show_owner || differ(&|first, other| first.owner == other.owner);
    let mut name = String::new();
    if show_server {
        name.push_str(&repository.server.address());
        name.push('/');
    }
    if show_owner {
        name.push_str(&repository.owner);
        name.push('/');
    }
    name.push_str(&repository.name);
    name
}

// `GHRepositoryCoordinates.toString`.
fn coordinates_text(coordinates: &RepoCoordinates) -> String {
    format!(
        "{}/{}/{}",
        coordinates.server.display(),
        coordinates.owner,
        coordinates.name
    )
}

fn accounts_failure(error: io::Error) -> String {
    format!("Could not read the GitHub accounts: {error}")
}

// GHHostedRepositoriesManager.knownRepositoriesState and the accounts, read from disk and git
// only. The window is available while `mappings` is not empty.
pub fn known(repo: &Path, base: &Path, data: &Path) -> Result<Known, GitError> {
    let list = accounts::list(base).map_err(|error| GitError::Io(accounts_failure(error)))?;
    let found = mappings(&remotes::list(repo)?, &accounts::known_servers(&list));
    let all: Vec<&RepoCoordinates> = found.iter().map(|mapping| &mapping.coordinates).collect();
    let mut views: Vec<MappingView> = found
        .iter()
        .map(|mapping| MappingView {
            remote: mapping.remote.clone(),
            url: mapping.url.clone(),
            display: display_name(&all, &mapping.coordinates, true),
            server: mapping.coordinates.server.display(),
            repository: coordinates_text(&mapping.coordinates),
        })
        .collect();
    views.sort_by(|left, right| left.display.cmp(&right.display));
    // Decision 2: the default account is not this window's business.
    let mut rows = listed_rows(&list);
    rows.sort_by(|left, right| left.login.cmp(&right.login));
    Ok(Known {
        mappings: views,
        accounts: rows,
        saved: saved(data),
    })
}

// The saved choice; a file that cannot be read is no choice.
pub fn saved(data: &Path) -> Option<Choice> {
    storage::load_json(data, CHOICE_FILE, CORRUPT_CHOICE_FILE).unwrap_or_default()
}

pub fn save_choice(data: &Path, choice: &Choice) -> io::Result<()> {
    fs::create_dir_all(data)?;
    let json = serde_json::to_vec_pretty(choice).map_err(io::Error::other)?;
    storage::write_atomic(&data.join(CHOICE_FILE), &json)
}

// `Change Repository or Account…`: `selectedUrlAndAccount = null`.
pub fn forget(data: &Path) -> io::Result<()> {
    match fs::remove_file(data.join(CHOICE_FILE)) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

// The `repository` fragment as GitHub sends it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawRepository {
    id: String,
    owner: RawOwner,
    name_with_owner: String,
    viewer_permission: Option<String>,
    merge_commit_allowed: bool,
    squash_merge_allowed: bool,
    rebase_merge_allowed: bool,
    default_branch_ref: Option<RawRef>,
    is_fork: bool,
    url: String,
    ssh_url: String,
}

#[derive(Deserialize)]
struct RawOwner {
    login: String,
}

#[derive(Deserialize)]
struct RawRef {
    name: String,
}

fn parse_failed() -> HostingError {
    HostingError::Io(PARSE_FAILED.to_string())
}

// GithubApiRequestExecutor puts `CurrentUser`'s operation before a body it cannot read.
fn profile_failure(error: HostingError) -> HostingError {
    match error {
        HostingError::Io(_) => HostingError::Io(accounts::presentable(&error)),
        error => error,
    }
}

// The account's token from the store: `Missing access token` when there is none, and the store's
// own error when it could not be read.
fn stored_token(store: &dyn SecretStore, account: &GithubAccount) -> Result<Secret, HostingError> {
    accounts::read_token(store, account)
        .map_err(|error| HostingError::Io(accounts::keychain_failure(&error)))?
        .ok_or(HostingError::MissingToken)
}

// GHPRDataContextRepository.getContextAsync, with the account's token: `GET /user` (cached) and
// `findRepository` on the path of the remote. `known` are every mapping of the repository, for
// the list tab's name. `check` is the task's cancellation, asked before each request. The token
// read here is given back with the connection, so that the list's requests need no other read.
pub fn connect(
    check: Check,
    client: &Client,
    store: &dyn SecretStore,
    users: &UserCache,
    mapping: &GithubMapping,
    account: &GithubAccount,
    known: &[GithubMapping],
) -> Result<Opened, HostingError> {
    let server = &account.server;
    if !server.url_crate_reads_the_same_host() {
        return Err(HostingError::Io(INVALID_SERVER.to_string()));
    }
    let token = stored_token(store, account)?;
    check()?;
    let viewer_login = users.login(&account.id(), Instant::now(), || {
        let user = accounts::current_user(client, server, &token).map_err(profile_failure)?;
        user.get("login")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| profile_failure(parse_failed()))
    })?;
    check()?;
    let parsed = &mapping.coordinates;
    let variables = json!({ "repoOwner": parsed.owner, "repoName": parsed.name });
    let found = client
        .graphql_at(
            &server.graphql_url(),
            Some(&token),
            None,
            QUERY,
            &variables,
            &["repository"],
        )?
        .ok_or_else(|| {
            HostingError::Io(format!(
                "Repository {}/{} does not exist at {} or you don't have access.",
                parsed.owner,
                parsed.name,
                server.display()
            ))
        })?;
    let raw: RawRepository = serde_json::from_value(found).map_err(|_| parse_failed())?;
    // `GHRepository.path`: the repository may have been renamed or moved.
    let (owner, name) = raw
        .name_with_owner
        .split_once('/')
        .map(|(owner, rest)| (owner, rest.split('/').next().unwrap_or(rest)))
        .ok_or_else(parse_failed)?;
    let coordinates = RepoCoordinates {
        server: server.clone(),
        owner: owner.to_string(),
        name: name.to_string(),
    };
    let all: Vec<&RepoCoordinates> = known.iter().map(|mapping| &mapping.coordinates).collect();
    let connection = Connection {
        remote: mapping.remote.clone(),
        url: mapping.url.clone(),
        account: account.id(),
        title: display_name(&all, &coordinates, false),
        coordinates,
        viewer_login,
        repository: RepoInfo {
            id: raw.id,
            owner: raw.owner.login,
            name_with_owner: raw.name_with_owner,
            viewer_permission: raw.viewer_permission,
            merge_commit_allowed: raw.merge_commit_allowed,
            squash_merge_allowed: raw.squash_merge_allowed,
            rebase_merge_allowed: raw.rebase_merge_allowed,
            default_branch: raw.default_branch_ref.map(|branch| branch.name),
            is_fork: raw.is_fork,
            url: raw.url,
            ssh_url: raw.ssh_url,
        },
    };
    Ok(Opened { connection, token })
}

// A connect that succeeded, with the token it read. The token stays in Rust's memory: it is not
// serialized and never reaches the interface.
pub struct Opened {
    pub connection: Connection,
    pub token: Secret,
}

// What every request inside an open connection needs: the repository path GitHub gave, and the
// token of the connection's account.
pub struct Session {
    pub coordinates: RepoCoordinates,
    pub token: Arc<Secret>,
}

fn no_longer_listed(account: &str) -> HostingError {
    HostingError::Io(format!("The account {account} is no longer in the list"))
}

// The connection's account must still be listed, on the connection's server, and have a token.
// `cached` is the token kept in memory since the connect: the store is read only without one.
pub fn session(
    store: &dyn SecretStore,
    base: &Path,
    connection: &Connection,
    cached: Option<Arc<Secret>>,
) -> Result<Session, HostingError> {
    let list = accounts::list(base).map_err(|error| HostingError::Io(accounts_failure(error)))?;
    let account = list
        .iter()
        .find(|candidate| candidate.id() == connection.account)
        .filter(|candidate| {
            candidate
                .server
                .same_ignoring_scheme(&connection.coordinates.server)
        })
        .ok_or_else(|| no_longer_listed(&connection.account))?;
    if !account.server.url_crate_reads_the_same_host() {
        return Err(HostingError::Io(INVALID_SERVER.to_string()));
    }
    let token = match cached {
        Some(token) => token,
        None => Arc::new(stored_token(store, account)?),
    };
    Ok(Session {
        coordinates: RepoCoordinates {
            server: account.server.clone(),
            ..connection.coordinates.clone()
        },
        token,
    })
}

// `View Pull Requests`: the remote and the account the selector chose, read again from git and
// disk, then the connect, then the saved choice. A choice that cannot be saved is only logged:
// the connection is open.
#[allow(clippy::too_many_arguments)]
pub fn open(
    check: Check,
    client: &Client,
    store: &dyn SecretStore,
    users: &UserCache,
    repo: &Path,
    base: &Path,
    data: &Path,
    remote: &str,
    account: &str,
) -> Result<Opened, HostingError> {
    let list = accounts::list(base).map_err(|error| HostingError::Io(accounts_failure(error)))?;
    let remotes =
        remotes::list(repo).map_err(|error| HostingError::Io(crate::fetch::error_text(error)))?;
    let known = mappings(&remotes, &accounts::known_servers(&list));
    let mapping = known
        .iter()
        .find(|mapping| mapping.remote == remote)
        .ok_or_else(|| {
            HostingError::Io(format!(
                "The remote {remote} does not point to a GitHub repository"
            ))
        })?;
    let chosen = list
        .iter()
        .find(|candidate| candidate.id() == account)
        .ok_or_else(|| no_longer_listed(account))?;
    if !chosen
        .server
        .same_ignoring_scheme(&mapping.coordinates.server)
    {
        return Err(HostingError::Io(format!(
            "The account {account} is not on the server of {}",
            coordinates_text(&mapping.coordinates)
        )));
    }
    let opened = connect(check, client, store, users, mapping, chosen, &known)?;
    let choice = Choice {
        url: mapping.url.clone(),
        account: account.to_string(),
    };
    if let Err(error) = save_choice(data, &choice) {
        eprintln!("Could not save the pull requests connection: {error}");
    }
    Ok(opened)
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tempfile::TempDir;

    use super::*;
    use crate::hosting::server::GithubServer;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::secrets::{MemoryStore, Secret, SecretError};
    use crate::test_support::{FakeGitHub, Reply, TempRepo};

    const TOKEN: &str = "ghp_PULLS0123456789";

    fn client() -> Client {
        Client::new(Duration::from_secs(5))
    }

    fn no_check() -> Result<(), HostingError> {
        Ok(())
    }

    // The token store of the tests, counting its reads: each one is a Keychain prompt on an
    // unsigned build.
    #[derive(Default)]
    struct CountingStore {
        inner: MemoryStore,
        reads: AtomicUsize,
    }

    impl SecretStore for CountingStore {
        fn get(&self, service: &str, account: &str) -> Result<Option<Secret>, SecretError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.inner.get(service, account)
        }

        fn set(&self, service: &str, account: &str, secret: &Secret) -> Result<(), SecretError> {
            self.inner.set(service, account, secret)
        }

        fn delete(&self, service: &str, account: &str) -> Result<(), SecretError> {
            self.inner.delete(service, account)
        }
    }

    // A repository, the app's data folder with the accounts, the repository's data folder and the
    // token store.
    struct Place {
        repo: TempRepo,
        base: TempDir,
        data: TempDir,
        store: CountingStore,
    }

    impl Place {
        fn new(remotes: &[(&str, &str)]) -> Self {
            let repo = TempRepo::new();
            for (name, url) in remotes {
                repo.git(&["remote", "add", name, url]);
            }
            Self {
                repo,
                base: TempDir::new().unwrap(),
                data: TempDir::new().unwrap(),
                store: CountingStore::default(),
            }
        }

        fn account(&self, server: &str, login: &str, token: Option<&str>) -> GithubAccount {
            let account = GithubAccount {
                server: GithubServer::parse(server).unwrap(),
                login: login.to_string(),
            };
            let mut list = accounts::list(self.base.path()).unwrap();
            list.push(account.clone());
            accounts::save(self.base.path(), &list).unwrap();
            if let Some(token) = token {
                self.store
                    .set(
                        &accounts::service(&account.server),
                        login,
                        &Secret::new(token.to_string()),
                    )
                    .unwrap();
            }
            account
        }

        fn known(&self) -> Known {
            known(self.repo.path(), self.base.path(), self.data.path()).unwrap()
        }

        fn reads(&self) -> usize {
            self.store.reads.load(Ordering::SeqCst)
        }

        fn open(
            &self,
            users: &UserCache,
            remote: &str,
            account: &str,
        ) -> Result<Connection, HostingError> {
            self.open_checking(&no_check, users, remote, account)
        }

        fn open_checking(
            &self,
            check: Check,
            users: &UserCache,
            remote: &str,
            account: &str,
        ) -> Result<Connection, HostingError> {
            open(
                check,
                &client(),
                &self.store,
                users,
                self.repo.path(),
                self.base.path(),
                self.data.path(),
                remote,
                account,
            )
            .map(|opened| opened.connection)
        }
    }

    fn user(login: &str) -> Reply {
        Reply::json(
            "GET",
            "/api/v3/user",
            200,
            "OK",
            &format!(r#"{{"login":"{login}","name":"The {login}"}}"#),
        )
    }

    fn repository(name_with_owner: &str) -> Reply {
        let owner = name_with_owner.split('/').next().unwrap();
        let body = json!({
            "data": {
                "repository": {
                    "id": "R_1",
                    "owner": { "__typename": "Organization", "id": "O_1", "login": owner },
                    "nameWithOwner": name_with_owner,
                    "viewerPermission": "WRITE",
                    "mergeCommitAllowed": true,
                    "squashMergeAllowed": false,
                    "rebaseMergeAllowed": true,
                    "defaultBranchRef": { "name": "main" },
                    "isFork": false,
                    "url": format!("https://github.com/{name_with_owner}"),
                    "sshUrl": format!("git@github.com:{name_with_owner}.git"),
                }
            }
        });
        Reply::json("POST", "/api/graphql", 200, "OK", &body.to_string())
    }

    fn mapping(display: &str, remote: &str) -> (String, String) {
        (display.to_string(), remote.to_string())
    }

    fn shown(known: &Known) -> Vec<(String, String)> {
        known
            .mappings
            .iter()
            .map(|view| mapping(&view.display, &view.remote))
            .collect()
    }

    #[test]
    fn the_window_shows_only_with_a_github_remote() {
        // github.com is always known: no account is needed.
        let dotcom = Place::new(&[("origin", "git@github.com:o/r.git")]);
        let known = dotcom.known();
        assert_eq!(shown(&known), [mapping("o/r", "origin")]);
        assert_eq!(
            known.mappings[0],
            MappingView {
                remote: "origin".to_string(),
                url: "git@github.com:o/r.git".to_string(),
                display: "o/r".to_string(),
                server: "github.com".to_string(),
                repository: "github.com/o/r".to_string(),
            }
        );
        assert!(known.accounts.is_empty());
        assert_eq!(known.saved, None);

        // Decision 4: a GitHub Enterprise remote needs an account on that server.
        let enterprise = Place::new(&[("origin", "https://ghe.example.com/o/r.git")]);
        assert!(enterprise.known().mappings.is_empty());
        enterprise.account("ghe.example.com", "hubot", None);
        let known = enterprise.known();
        assert_eq!(shown(&known), [mapping("o/r", "origin")]);
        assert_eq!(known.accounts.len(), 1);
        // The tokens are not read here: only a connect reads one.
        assert!(!known.accounts[0].needs_login);
        assert_eq!(enterprise.reads(), 0);

        // A remote that is not GitHub, or a local path, gives nothing.
        let other = Place::new(&[
            ("origin", "https://gitlab.com/o/r.git"),
            ("local", "/srv/r.git"),
        ]);
        assert!(other.known().mappings.is_empty());
    }

    #[test]
    fn the_repository_combo_is_sorted_and_names_the_server_when_there_are_several() {
        let place = Place::new(&[
            ("origin", "https://github.com/me/r.git"),
            ("upstream", "https://github.com/acme/r.git"),
            ("work", "https://ghe.example.com/acme/tool.git"),
        ]);
        place.account("ghe.example.com", "zed", Some(TOKEN));
        place.account("github.com", "bob", Some(TOKEN));
        place.account("github.com", "alice", None);
        let known = place.known();
        assert_eq!(
            shown(&known),
            [
                mapping("ghe.example.com/acme/tool", "work"),
                mapping("github.com/acme/r", "upstream"),
                mapping("github.com/me/r", "origin"),
            ]
        );
        let logins: Vec<&str> = known
            .accounts
            .iter()
            .map(|row| row.login.as_str())
            .collect();
        assert_eq!(logins, ["alice", "bob", "zed"]);
        // Decision 2: no account is the default here.
        assert!(known.accounts.iter().all(|row| !row.is_default));
    }

    #[test]
    fn the_list_tab_is_named_as_241_names_the_repository() {
        let coordinates = |server: &str, owner: &str, name: &str| RepoCoordinates {
            server: GithubServer::parse(server).unwrap(),
            owner: owner.to_string(),
            name: name.to_string(),
        };
        let mine = coordinates("github.com", "me", "r");
        let parent = coordinates("github.com", "acme", "r");
        let work = coordinates("ghe.example.com", "acme", "tool");

        assert_eq!(display_name(&[&mine], &mine, false), "r");
        assert_eq!(display_name(&[&mine], &mine, true), "me/r");
        assert_eq!(display_name(&[&mine, &parent], &mine, false), "me/r");
        assert_eq!(
            display_name(&[&mine, &work], &work, false),
            "ghe.example.com/acme/tool"
        );
        // The same owner on one server: the name alone.
        let other = coordinates("github.com", "me", "other");
        assert_eq!(display_name(&[&mine, &other], &other, false), "other");
    }

    #[test]
    fn connect_uses_the_api_repository_path_after_a_rename() {
        let fake = FakeGitHub::new();
        let url = format!("{}/old-owner/old-name.git", fake.url());
        let place = Place::new(&[("origin", &url)]);
        let account = place.account(&fake.url(), "octocat", Some(TOKEN));
        fake.script(vec![user("octocat"), repository("new-owner/new-name")]);

        let users = UserCache::default();
        let connection = place.open(&users, "origin", &account.id()).unwrap();
        assert_eq!(connection.coordinates.owner, "new-owner");
        assert_eq!(connection.coordinates.name, "new-name");
        assert_eq!(connection.coordinates.server, account.server);
        assert_eq!(connection.title, "new-name");
        assert_eq!(connection.viewer_login, "octocat");
        assert_eq!(connection.remote, "origin");
        assert_eq!(connection.url, url);
        assert_eq!(connection.account, account.id());
        assert_eq!(
            connection.repository,
            RepoInfo {
                id: "R_1".to_string(),
                owner: "new-owner".to_string(),
                name_with_owner: "new-owner/new-name".to_string(),
                viewer_permission: Some("WRITE".to_string()),
                merge_commit_allowed: true,
                squash_merge_allowed: false,
                rebase_merge_allowed: true,
                default_branch: Some("main".to_string()),
                is_fork: false,
                url: "https://github.com/new-owner/new-name".to_string(),
                ssh_url: "git@github.com:new-owner/new-name.git".to_string(),
            }
        );

        // `findRepository` asks with the remote's path, as 241 sends it, with the token.
        let requests = fake.requests();
        assert_eq!(requests.len(), 2);
        let body: Value = serde_json::from_str(&requests[1].body).unwrap();
        assert_eq!(body["query"], QUERY);
        assert_eq!(
            body["variables"],
            json!({ "repoOwner": "old-owner", "repoName": "old-name" })
        );
        assert_eq!(
            requests[1].header("authorization"),
            Some(format!("Bearer {TOKEN}").as_str())
        );
        // The choice is saved, by the remote's URL.
        assert_eq!(
            place.known().saved,
            Some(Choice {
                url,
                account: account.id(),
            })
        );
        fake.finish();
    }

    #[test]
    fn a_missing_repository_says_241s_text() {
        let fake = FakeGitHub::new();
        let place = Place::new(&[("origin", &format!("{}/o/r.git", fake.url()))]);
        let account = place.account(&fake.url(), "octocat", Some(TOKEN));
        fake.script(vec![
            user("octocat"),
            Reply::json(
                "POST",
                "/api/graphql",
                200,
                "OK",
                r#"{"data":{"repository":null}}"#,
            ),
        ]);
        let error = place
            .open(&UserCache::default(), "origin", &account.id())
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            format!(
                "Repository o/r does not exist at {} or you don't have access.",
                fake.url()
            )
        );
        // Nothing is saved.
        assert_eq!(place.known().saved, None);
        fake.finish();
    }

    #[test]
    fn a_missing_token_says_could_not_connect_and_missing_access_token() {
        let fake = FakeGitHub::new();
        let place = Place::new(&[("origin", &format!("{}/o/r.git", fake.url()))]);
        let account = place.account(&fake.url(), "octocat", None);
        place.known();
        assert_eq!(place.reads(), 0);
        let error = place
            .open(&UserCache::default(), "origin", &account.id())
            .unwrap_err();
        assert_eq!(error, HostingError::MissingToken);
        assert_eq!(error.to_string(), "Missing access token");
        // The connect read the one token it needed, and asked GitHub nothing.
        assert_eq!(place.reads(), 1);
        assert!(fake.requests().is_empty());
        fake.finish();
    }

    #[test]
    fn a_cancelled_connect_asks_github_nothing_more() {
        let fake = FakeGitHub::new();
        let place = Place::new(&[("origin", &format!("{}/o/r.git", fake.url()))]);
        let account = place.account(&fake.url(), "octocat", Some(TOKEN));
        let cancelled = || Err(HostingError::Cancelled);
        let error = place
            .open_checking(&cancelled, &UserCache::default(), "origin", &account.id())
            .unwrap_err();
        assert_eq!(error, HostingError::Cancelled);
        assert!(fake.requests().is_empty());

        // Cancelled after `GET /user`: no `findRepository`, and nothing saved.
        fake.reply(user("octocat"));
        let asked = std::cell::Cell::new(0);
        let second = || {
            asked.set(asked.get() + 1);
            if asked.get() > 1 {
                Err(HostingError::Cancelled)
            } else {
                Ok(())
            }
        };
        let error = place
            .open_checking(&second, &UserCache::default(), "origin", &account.id())
            .unwrap_err();
        assert_eq!(error, HostingError::Cancelled);
        assert_eq!(fake.requests().len(), 1);
        assert_eq!(place.known().saved, None);
        fake.finish();
    }

    #[test]
    fn the_user_is_asked_once_in_30_minutes() {
        let fake = FakeGitHub::new();
        let place = Place::new(&[("origin", &format!("{}/o/r.git", fake.url()))]);
        let account = place.account(&fake.url(), "octocat", Some(TOKEN));
        fake.script(vec![user("octocat"), repository("o/r"), repository("o/r")]);
        let users = UserCache::default();
        place.open(&users, "origin", &account.id()).unwrap();
        place.open(&users, "origin", &account.id()).unwrap();
        fake.finish();

        let start = Instant::now();
        let asked = std::cell::Cell::new(0);
        let load = || {
            asked.set(asked.get() + 1);
            Ok(format!("login{}", asked.get()))
        };
        assert_eq!(users.login("a", start, load).unwrap(), "login1");
        let later = start + USER_LIFETIME - Duration::from_secs(1);
        assert_eq!(users.login("a", later, load).unwrap(), "login1");
        assert_eq!(
            users.login("a", start + USER_LIFETIME, load).unwrap(),
            "login2"
        );
        users.clear();
        assert_eq!(
            users.login("a", start + USER_LIFETIME, load).unwrap(),
            "login3"
        );
        // A failure is not kept.
        let failing = || Err(HostingError::Unreachable);
        assert_eq!(
            users.login("b", start, failing),
            Err(HostingError::Unreachable)
        );
        assert_eq!(users.login("b", start, load).unwrap(), "login4");
    }

    #[test]
    fn a_failed_connect_names_what_github_said() {
        let fake = FakeGitHub::new();
        let place = Place::new(&[("origin", &format!("{}/o/r.git", fake.url()))]);
        let account = place.account(&fake.url(), "octocat", Some(TOKEN));
        fake.reply(Reply::json(
            "GET",
            "/api/v3/user",
            401,
            "Unauthorized",
            r#"{"message":"Bad credentials"}"#,
        ));
        let error = place
            .open(&UserCache::default(), "origin", &account.id())
            .unwrap_err();
        assert!(error.is_authentication());
        assert_eq!(error.to_string(), "Request response: Bad credentials.");
        fake.finish();

        // The remote or the account the interface chose may be gone.
        assert_eq!(
            place
                .open(&UserCache::default(), "upstream", &account.id())
                .unwrap_err()
                .to_string(),
            "The remote upstream does not point to a GitHub repository"
        );
        assert_eq!(
            place
                .open(&UserCache::default(), "origin", "github.com/nobody")
                .unwrap_err()
                .to_string(),
            "The account github.com/nobody is no longer in the list"
        );
        let other = place.account("github.com", "hubot", Some(TOKEN));
        assert_eq!(
            place
                .open(&UserCache::default(), "origin", &other.id())
                .unwrap_err()
                .to_string(),
            format!(
                "The account github.com/hubot is not on the server of {}/o/r",
                fake.url()
            )
        );
    }

    #[test]
    fn a_session_needs_the_listed_account_and_its_token() {
        let fake = FakeGitHub::new();
        let place = Place::new(&[("origin", &format!("{}/o/r.git", fake.url()))]);
        let account = place.account(&fake.url(), "octocat", Some(TOKEN));
        fake.script(vec![user("octocat"), repository("new-owner/new-name")]);
        let connection = place
            .open(&UserCache::default(), "origin", &account.id())
            .unwrap();
        fake.finish();

        let reads = place.reads();
        let session = session(&place.store, place.base.path(), &connection, None).unwrap();
        assert_eq!(session.token.expose(), TOKEN);
        assert_eq!(place.reads(), reads + 1);
        // With the token kept since the connect, the store is not read.
        let kept = session.token.clone();
        let again = super::session(&place.store, place.base.path(), &connection, Some(kept)).unwrap();
        assert_eq!(again.token.expose(), TOKEN);
        assert_eq!(place.reads(), reads + 1);
        assert_eq!(session.coordinates.owner, "new-owner");
        assert_eq!(session.coordinates.name, "new-name");
        assert_eq!(session.coordinates.server, account.server);

        // The token is gone.
        place
            .store
            .delete(&accounts::service(&account.server), "octocat")
            .unwrap();
        assert_eq!(
            session_error(&place, &connection),
            HostingError::MissingToken
        );
        // The account is gone.
        accounts::save(place.base.path(), &[]).unwrap();
        assert_eq!(
            session_error(&place, &connection).to_string(),
            format!("The account {} is no longer in the list", account.id())
        );
    }

    // A Keychain that refuses to be read (a denied prompt).
    struct DeniedStore;

    impl SecretStore for DeniedStore {
        fn get(&self, _: &str, _: &str) -> Result<Option<Secret>, SecretError> {
            Err(SecretError("User interaction is not allowed.".to_string()))
        }

        fn set(&self, _: &str, _: &str, _: &Secret) -> Result<(), SecretError> {
            Ok(())
        }

        fn delete(&self, _: &str, _: &str) -> Result<(), SecretError> {
            Ok(())
        }
    }

    #[test]
    fn a_denied_keychain_read_is_not_a_missing_token() {
        let fake = FakeGitHub::new();
        let place = Place::new(&[("origin", &format!("{}/o/r.git", fake.url()))]);
        let account = place.account(&fake.url(), "octocat", Some(TOKEN));
        let denied = "Could not read the access token from the Keychain: User interaction is not allowed.";
        let error = open(
            &no_check,
            &client(),
            &DeniedStore,
            &UserCache::default(),
            place.repo.path(),
            place.base.path(),
            place.data.path(),
            "origin",
            &account.id(),
        )
        .err()
        .unwrap();
        assert_eq!(error, HostingError::Io(denied.to_string()));
        assert!(fake.requests().is_empty());

        fake.script(vec![user("octocat"), repository("o/r")]);
        let connection = place
            .open(&UserCache::default(), "origin", &account.id())
            .unwrap();
        fake.finish();
        match session(&DeniedStore, place.base.path(), &connection, None) {
            Err(error) => assert_eq!(error, HostingError::Io(denied.to_string())),
            Ok(_) => panic!("a session opened"),
        }
    }

    fn session_error(place: &Place, connection: &Connection) -> HostingError {
        match session(&place.store, place.base.path(), connection, None) {
            Ok(_) => panic!("a session opened"),
            Err(error) => error,
        }
    }

    #[test]
    fn change_repository_or_account_forgets_the_choice() {
        let place = Place::new(&[("origin", "https://github.com/o/r.git")]);
        let choice = Choice {
            url: "https://github.com/o/r.git".to_string(),
            account: "github.com/octocat".to_string(),
        };
        save_choice(place.data.path(), &choice).unwrap();
        assert_eq!(place.known().saved, Some(choice));
        forget(place.data.path()).unwrap();
        assert_eq!(place.known().saved, None);
        // Forgetting twice is not an error.
        forget(place.data.path()).unwrap();

        // A damaged file is no choice.
        fs::write(place.data.path().join(CHOICE_FILE), "{").unwrap();
        assert_eq!(place.known().saved, None);
    }
}
