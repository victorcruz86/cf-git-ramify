// The interface's GitHub commands: the accounts of Settings > Version Control > GitHub, the
// `Generate…` page, and opening a URL in the browser. A token arrives once, in `github_login`,
// and no command gives one back (Global Constraints of the part 5 plan).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::branches::Repo;
use crate::commands::{
    base_dir, blocking_api, current_repo, data_dir_of, failed, flush_diff_window, ApiError,
    AppState, SAVE_ACTION,
};
use crate::git::GitError;
use crate::hosting::accounts::{self, GithubAccount, LoginError, LoginTarget};
use crate::hosting::http::{Client, HostingError};
use crate::hosting::links::{self, LinkTarget, Reference};
use crate::hosting::protection;
use crate::hosting::pulls::changes::{self, PullCache, PullRequestChanges};
use crate::hosting::pulls::checkout::{self, CheckedOut, Headed, Range};
use crate::hosting::pulls::connections::Connections;
use crate::hosting::pulls::details::{
    self, PullRequestCommit, PullRequestDetails, PullRequestStatus,
};
use crate::hosting::pulls::list::{self, ChooserKind, Page, SavedSearch, SearchValue};
use crate::hosting::pulls::{self, Connection, Known, Session, UserCache};
use crate::hosting::server::GithubServer;
use crate::secrets::{Secret, SecretStore};
use crate::settings;
use crate::storage;
use crate::task::{TaskId, TaskKind};

// Absolute path: what the app launches does not depend on the PATH it inherited.
const OPEN: &str = "/usr/bin/open";
// GithubBundle 28.
const LOGGING_IN: &str = "Logging in\u{2026}";
// GithubBundle 167: connect has no progress text of its own in 241, only the busy selector.
const VIEW_PULL_REQUESTS: &str = "View Pull Requests";
// 241 loads the list under a progress stripe, with no text of its own.
const LOADING_PULL_REQUESTS: &str = "Loading pull requests";
const NO_CONNECTION: &str = "The Pull Requests window is not connected to a repository";
// 241 loads a pull request's details, commits and changes under `LoadingLabel`, with no text of its
// own; the status line is `GB:234`.
const LOADING_DETAILS: &str = "Loading pull request details";
const LOADING_STATUS: &str = "Loading pull request status";
const LOADING_COMMITS: &str = "Loading pull request commits";
const LOADING_CHANGES: &str = "Loading pull request changes";
const HEAD_MOVED: &str = "The pull request has changed since it was loaded";
// `CTB:67` and `CTB:68`, the progress of `Checkout '…'…` and `Show '…' in Git Log`.
const CHECKING_OUT: &str = "Checkout review request branch";
const SHOWING_IN_LOG: &str = "Show review request branch in Git Log";

// The client every GitHub request goes through, with the connect timeout of the settings, and
// where the tokens are: the Keychain in the app, memory in the tests. `users` is the logins of
// `GET /user`, which any change to the accounts forgets. `connections` are the Pull Requests
// window's open connections, one per repository, with their tokens in memory: the list's requests
// go through them. `pulls` is what the open pull requests already fetched and loaded.
pub struct HostingState {
    client: RwLock<Arc<Client>>,
    pub store: Arc<dyn SecretStore>,
    pub users: UserCache,
    pub connections: Connections,
    pub pulls: PullCache,
}

impl HostingState {
    pub fn new(store: Arc<dyn SecretStore>) -> Self {
        Self {
            client: RwLock::new(Arc::new(client_for(settings::GITHUB_CONNECTION_TIMEOUT_MS))),
            store,
            users: UserCache::default(),
            connections: Connections::default(),
            pulls: PullCache::default(),
        }
    }

    // A login, a re-login or a removal: the logins of `GET /user` and the kept tokens go.
    pub fn accounts_changed(&self) {
        self.users.clear();
        self.connections.drop_tokens();
    }

    pub fn client(&self) -> Arc<Client> {
        self.client
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    // `Connection timeout:` takes effect on the next request; one already running keeps its own.
    pub fn set_connect_timeout(&self, milliseconds: u64) {
        *self.client.write().unwrap_or_else(PoisonError::into_inner) =
            Arc::new(client_for(milliseconds));
    }
}

impl Default for HostingState {
    fn default() -> Self {
        // The tests never reach the user's Keychain, not even through `AppState::default`. A
        // `cfg!`, not a `#[cfg]`: the diff-window guard reads this file up to its first one.
        let store: Arc<dyn SecretStore> = if cfg!(test) {
            Arc::new(crate::secrets::MemoryStore::new())
        } else {
            Arc::new(crate::secrets::KeychainStore)
        };
        Self::new(store)
    }
}

fn client_for(milliseconds: u64) -> Client {
    Client::new(Duration::from_millis(milliseconds))
}

// From the app's setup: after every successful fetch, the fetched remote's protected branch rules
// are loaded from GitHub (`GHProtectedBranchRulesLoader`, a `gitFetchHandler`), with the client and
// tokens of the moment.
pub fn start_protection_loader(app: &AppHandle) {
    let base = match base_dir(app) {
        Ok(base) => base,
        Err(error) => {
            eprintln!(
                "GitHub branch protection rules will not be loaded: {}",
                error.message
            );
            return;
        }
    };
    let state = app.state::<AppState>();
    let hosting = state.hosting.clone();
    state
        .branch_sync
        .set_after_fetch(Arc::new(move |ctx, repo, remote| {
            let data = storage::repo_dir(&base, repo);
            let client = hosting.client();
            protection::load(
                ctx,
                &client,
                hosting.store.as_ref(),
                &base,
                &data,
                repo,
                remote,
            );
        }));
}

// A row of the accounts list. `server` is 241's `toString` (the scheme only when it was typed),
// and `isDefault` is about the open repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    pub id: String,
    pub server: String,
    pub login: String,
    pub name: Option<String>,
    pub error: Option<String>,
    pub needs_login: bool,
    pub is_default: bool,
}

fn view(
    account: &GithubAccount,
    details: accounts::AccountDetails,
    default: Option<&str>,
) -> AccountView {
    let id = account.id();
    AccountView {
        is_default: default == Some(id.as_str()),
        id,
        server: account.server.display(),
        login: account.login.clone(),
        name: details.name,
        error: details.error,
        needs_login: details.needs_login,
    }
}

// Every account at once, with no request: its server, login and whether it is the default, and
// `needs_login` when there is no token. The details come one row at a time (`row_with_details`).
fn rows(
    store: &dyn SecretStore,
    base: &Path,
    default: Option<&str>,
) -> Result<Vec<AccountView>, String> {
    let list = accounts::list(base)
        .map_err(|error| format!("Could not read the GitHub accounts: {error}"))?;
    Ok(rows_of(store, &list, default))
}

// The same rows, of a list already read.
fn rows_of(
    store: &dyn SecretStore,
    list: &[GithubAccount],
    default: Option<&str>,
) -> Vec<AccountView> {
    list.iter()
        .map(|account| {
            let details = accounts::AccountDetails {
                name: None,
                error: None,
                needs_login: accounts::token(store, account).is_none(),
            };
            view(account, details, default)
        })
        .collect()
}

// The rows of a list with no token read, so `needs_login` is false.
fn token_free_rows(list: &[GithubAccount], default: Option<&str>) -> Vec<AccountView> {
    let details = || accounts::AccountDetails {
        name: None,
        error: None,
        needs_login: false,
    };
    list.iter()
        .map(|account| view(account, details(), default))
        .collect()
}

// The rows of the Pull Requests selector: no token is read, and the connect says when one is
// missing. Decision 2: no default.
pub(crate) fn listed_rows(list: &[GithubAccount]) -> Vec<AccountView> {
    token_free_rows(list, None)
}

// What `github_accounts` answers: with `tokens`, the rows of Settings, which say which account
// has no token; without, no Keychain item is read (an unsigned build prompts for each one), for
// the default-account check that every repository open makes.
fn account_rows(
    store: &dyn SecretStore,
    base: &Path,
    default: Option<&str>,
    tokens: bool,
) -> Result<Vec<AccountView>, String> {
    if tokens {
        return rows(store, base, default);
    }
    let list = accounts::list(base)
        .map_err(|error| format!("Could not read the GitHub accounts: {error}"))?;
    Ok(token_free_rows(&list, default))
}

// One account with its details: the request to GitHub that `GHAccountsDetailsProvider` makes.
fn row_with_details(
    client: &Client,
    store: &dyn SecretStore,
    base: &Path,
    default: Option<&str>,
    id: &str,
) -> Result<AccountView, String> {
    let list = accounts::list(base)
        .map_err(|error| format!("Could not read the GitHub accounts: {error}"))?;
    let account = list
        .iter()
        .find(|account| account.id() == id)
        .ok_or_else(|| format!("The account {id} is no longer in the list"))?;
    Ok(view(
        account,
        accounts::details(client, store, account),
        default,
    ))
}

// The open repository's default account. A default whose account is gone is no default.
fn default_of(repo_dir: Option<&Path>) -> Option<String> {
    repo_dir.and_then(|dir| settings::load_repo(dir).github_default_account)
}

// The login, and the open repository's default following the edited account when its login
// changes. The new account has no details yet: the list asks for them.
#[allow(clippy::too_many_arguments)]
// When the default cannot follow, the login has still succeeded: the failure is logged and the
// row is shown without the default.
fn log_in(
    client: &Client,
    store: &dyn SecretStore,
    base: &Path,
    repo_dir: Option<&Path>,
    target: &LoginTarget,
    token: Secret,
    check: &dyn Fn() -> Result<(), HostingError>,
) -> Result<AccountView, LoginError> {
    let account = accounts::login(client, store, base, target, token, check)?;
    let mut default = default_of(repo_dir);
    if let (Some(dir), LoginTarget::Edit(edited)) = (repo_dir, target) {
        if default.as_deref() == Some(edited.as_str()) && *edited != account.id() {
            match settings::save_github_default_account(dir, Some(account.id())) {
                Ok(()) => default = Some(account.id()),
                Err(error) => eprintln!("Could not save the default GitHub account: {error}"),
            }
        }
    }
    let details = accounts::AccountDetails {
        name: None,
        error: None,
        needs_login: false,
    };
    Ok(view(&account, details, default.as_deref()))
}

// `−`, and the open repository's default goes with it (PersistentDefaultAccountHolder).
fn remove_account(
    store: &dyn SecretStore,
    base: &Path,
    repo_dir: Option<&Path>,
    id: &str,
) -> Result<(), String> {
    accounts::remove(store, base, id)
        .map_err(|error| format!("Could not remove the GitHub account: {error}"))?;
    if let Some(dir) = repo_dir {
        if default_of(Some(dir)).as_deref() == Some(id) {
            settings::save_github_default_account(dir, None)
                .map_err(|error| format!("Could not save the default account: {error}"))?;
        }
    }
    Ok(())
}

// `Set as Default`, for the open repository, of an account in the list.
fn set_default(base: &Path, repo_dir: &Path, id: &str) -> Result<(), String> {
    let list = accounts::list(base)
        .map_err(|error| format!("Could not read the GitHub accounts: {error}"))?;
    if !list.iter().any(|account| account.id() == id) {
        return Err(format!("The account {id} is no longer in the list"));
    }
    settings::save_github_default_account(repo_dir, Some(id.to_string()))
        .map_err(|error| format!("Could not save the default account: {error}"))
}

// The token as typed, without the spaces or the line break a paste brings along. A blank one is
// the token field's error, as in `GHTokenCredentialsUi`.
fn clean_token(token: &str) -> Result<&str, ApiError> {
    let token = token.trim();
    if token.is_empty() {
        return Err(field_error(
            "login_token",
            "Token cannot be empty".to_string(),
        ));
    }
    Ok(token)
}

// An error of the login: `kind` says which field of the dialog it belongs to.
fn field_error(kind: &'static str, message: String) -> ApiError {
    ApiError {
        kind,
        message,
        details: None,
    }
}

// What a task failure carries to the dialog: the server field's errors are told from the token's.
fn login_error(error: LoginError) -> GitError {
    match error {
        LoginError::Cancelled => GitError::Cancelled,
        LoginError::InvalidServer => GitError::LoginField {
            server: true,
            message: error.to_string(),
        },
        other => GitError::LoginField {
            server: false,
            message: other.to_string(),
        },
    }
}

// What a login is about. With `editing`, the account's own server is used and the server text,
// which the dialog shows fixed, is not read.
fn login_target(server: &str, editing: Option<String>) -> Result<LoginTarget, ApiError> {
    match editing {
        Some(id) => Ok(LoginTarget::Edit(id)),
        None => GithubServer::parse(server)
            .map(LoginTarget::Add)
            .map_err(|error| field_error("login_server", error.to_string())),
    }
}

// The browser opens only an `https://` or `http://` URL with no user or password in it, as the
// value of `open -u`, so that it is never taken for an option or a file.
pub fn open_url_command(url: &str) -> Result<Command, String> {
    let refused = || format!("Only an https:// or http:// address can be opened: {url}");
    if url.starts_with('-') || !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(refused());
    }
    let parsed = url::Url::parse(url).map_err(|_| refused())?;
    if !matches!(parsed.scheme(), "https" | "http")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(refused());
    }
    let mut command = Command::new(OPEN);
    command.arg("-u").arg(parsed.as_str());
    Ok(command)
}

fn open_repo_dir(
    app: &AppHandle,
    state: &State<'_, AppState>,
) -> Result<Option<PathBuf>, ApiError> {
    match current_repo(state) {
        Ok(repo) => data_dir_of(app, &repo).map(Some),
        Err(_) => Ok(None),
    }
}

// `tokens: false` reads no token (see `account_rows`); left out, it reads them.
#[tauri::command]
pub async fn github_accounts(
    tokens: Option<bool>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<AccountView>, ApiError> {
    let base = base_dir(&app)?;
    let repo_dir = open_repo_dir(&app, &state)?;
    let store = state.hosting.store.clone();
    blocking_api(move || {
        let default = default_of(repo_dir.as_deref());
        account_rows(
            store.as_ref(),
            &base,
            default.as_deref(),
            tokens.unwrap_or(true),
        )
        .map_err(failed)
    })
    .await
}

// One row's details: a request to GitHub, so the list asks for each row on its own and a slow
// server keeps only its own row waiting.
#[tauri::command]
pub async fn github_account_details(
    id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<AccountView, ApiError> {
    let base = base_dir(&app)?;
    let repo_dir = open_repo_dir(&app, &state)?;
    let client = state.hosting.client();
    let store = state.hosting.store.clone();
    blocking_api(move || {
        let default = default_of(repo_dir.as_deref());
        row_with_details(&client, store.as_ref(), &base, default.as_deref(), &id).map_err(failed)
    })
    .await
}

// A task, so that the dialog can cancel it; its value is the new account's `AccountView`. The
// token becomes a `Secret` before anything else.
#[tauri::command]
pub async fn github_login(
    server: String,
    token: String,
    editing: Option<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    // The server is checked before the token, as in `GithubLoginPanel`.
    let target = login_target(&server, editing)?;
    let token = Secret::new(clean_token(&token)?.to_string());
    let base = base_dir(&app)?;
    let repo_dir = open_repo_dir(&app, &state)?;
    let client = state.hosting.client();
    let hosting = state.hosting.clone();
    let place = base.clone();
    Ok(state
        .tasks
        .start(&place, TaskKind::Read, LOGGING_IN.to_string(), move |ctx| {
            let check = || ctx.check_cancelled().map_err(|_| HostingError::Cancelled);
            let logged = log_in(
                &client,
                hosting.store.as_ref(),
                &base,
                repo_dir.as_deref(),
                &target,
                token,
                &check,
            )
            .map_err(login_error);
            hosting.accounts_changed();
            logged
        }))
}

#[tauri::command]
pub async fn github_remove(
    id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), ApiError> {
    let base = base_dir(&app)?;
    let repo_dir = open_repo_dir(&app, &state)?;
    let hosting = state.hosting.clone();
    blocking_api(move || {
        let removed = remove_account(hosting.store.as_ref(), &base, repo_dir.as_deref(), &id);
        hosting.accounts_changed();
        removed.map_err(failed)
    })
    .await
}

#[tauri::command]
pub async fn github_set_default(
    id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), ApiError> {
    let base = base_dir(&app)?;
    let repo_dir = data_dir_of(&app, &current_repo(&state)?)?;
    blocking_api(move || set_default(&base, &repo_dir, &id).map_err(failed)).await
}

// `Generate…`: the page that makes a token for the typed server.
#[tauri::command]
pub async fn github_generate_url(server: String) -> Result<String, ApiError> {
    let server = GithubServer::parse(&server)
        .map_err(|error| field_error("login_server", error.to_string()))?;
    if !server.url_crate_reads_the_same_host() {
        return Err(field_error(
            "login_server",
            LoginError::InvalidServer.to_string(),
        ));
    }
    Ok(accounts::generate_url(&server))
}

fn known_servers(base: &Path) -> Result<Vec<GithubServer>, String> {
    let list = accounts::list(base)
        .map_err(|error| format!("Could not read the GitHub accounts: {error}"))?;
    Ok(accounts::known_servers(&list))
}

// `Open on GitHub` and `Copy Link`: the links of a commit or a file, one per GitHub repository of
// the open repository's remotes, empty when the place gets none. It reads local data only and
// makes no request to GitHub.
#[tauri::command]
pub async fn github_links(
    target: LinkTarget,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<Reference>, ApiError> {
    let base = base_dir(&app)?;
    let repo = current_repo(&state)?;
    blocking_api(move || {
        let servers = known_servers(&base).map_err(failed)?;
        Ok(links::references(&repo, &servers, &target)?)
    })
    .await
}

// The Pull Requests window: the GitHub repositories of the remotes, the accounts and the saved
// choice. It reads git and local data only: no request to GitHub, and no token.
#[tauri::command]
pub async fn github_known(app: AppHandle, state: State<'_, AppState>) -> Result<Known, ApiError> {
    let base = base_dir(&app)?;
    let repo = current_repo(&state)?;
    let data = data_dir_of(&app, &repo)?;
    blocking_api(move || Ok(pulls::known(&repo, &base, &data)?)).await
}

// `View Pull Requests`: a task whose value is the `Connection`. `remote` is the remote's name and
// `account` the account's id; a success saves them as the choice to reconnect with.
#[tauri::command]
pub async fn github_connect(
    remote: String,
    account: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    let base = base_dir(&app)?;
    let repo = current_repo(&state)?;
    let data = data_dir_of(&app, &repo)?;
    let client = state.hosting.client();
    let hosting = state.hosting.clone();
    let place = repo.clone();
    let ticket = state.hosting.connections.start();
    Ok(state.tasks.start(
        &place,
        TaskKind::Read,
        VIEW_PULL_REQUESTS.to_string(),
        move |ctx| -> Result<Connection, GitError> {
            let check = || ctx.check_cancelled().map_err(|_| HostingError::Cancelled);
            let opened = pulls::open(
                &check,
                &client,
                hosting.store.as_ref(),
                &hosting.users,
                &repo,
                &base,
                &data,
                &remote,
                &account,
            )
            .map_err(connect_error)?;
            // A connect that a later one, or a forget, overtook opens nothing here; the interface
            // drops its answer too.
            let connection = opened.connection.clone();
            hosting
                .connections
                .opened(&repo, ticket, opened.connection, opened.token);
            Ok(connection)
        },
    ))
}

// `Change Repository or Account…`: the saved choice is forgotten.
#[tauri::command]
pub async fn github_forget_connection(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), ApiError> {
    let repo = current_repo(&state)?;
    let data = data_dir_of(&app, &repo)?;
    state.hosting.connections.forget(&repo);
    blocking_api(move || {
        pulls::forget(&data).map_err(|error| {
            failed(format!(
                "Could not forget the pull requests connection: {error}"
            ))
        })
    })
    .await
}

// What the selector shows under `Could not connect to repository … with account …`.
fn connect_error(error: HostingError) -> GitError {
    match error {
        HostingError::Cancelled => GitError::Cancelled,
        HostingError::MissingToken => GitError::MissingToken,
        other => GitError::Io(other.to_string()),
    }
}

// What the list's requests start from: the app's data folder and the open repository's
// connection.
fn opened(app: &AppHandle, state: &State<'_, AppState>) -> Result<(PathBuf, PathBuf), ApiError> {
    let base = base_dir(app)?;
    let repo = current_repo(state)?;
    if state.hosting.connections.connection_of(&repo).is_none() {
        return Err(failed(NO_CONNECTION.to_string()));
    }
    Ok((base, repo))
}

// The session of `repo`'s connection: the token kept since the connect, or read once from the
// store and kept when there is none.
fn list_session(hosting: &HostingState, base: &Path, repo: &Path) -> Result<Session, HostingError> {
    let current = hosting
        .connections
        .current(repo)
        .ok_or_else(|| HostingError::Io(NO_CONNECTION.to_string()))?;
    let read = current.token.is_none();
    let session = pulls::session(
        hosting.store.as_ref(),
        base,
        &current.connection,
        current.token.clone(),
    )?;
    if read {
        hosting
            .connections
            .keep_token(repo, &current, session.token.clone());
    }
    Ok(session)
}

// GitHub refused the token: it is read again next time.
fn list_failure(hosting: &HostingState, repo: &Path, error: HostingError) -> HostingError {
    if error.is_authentication() {
        hosting.connections.drop_token(repo);
    }
    error
}

// What the list shows under `Can't load pull requests`: `Re-Login` for an authentication error,
// else `getLoadingErrorText`.
fn list_error(error: HostingError) -> GitError {
    match error {
        HostingError::Cancelled => GitError::Cancelled,
        HostingError::MissingToken => GitError::MissingToken,
        error if error.is_authentication() => GitError::Authentication(error.to_string()),
        other => GitError::Io(list::loading_error_text(&other)),
    }
}

// One page of the list: a task whose value is the `Page`. `cursor` is the previous page's
// `endCursor`, none for the first.
#[tauri::command]
pub async fn github_pr_page(
    value: SearchValue,
    cursor: Option<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    let (base, repo) = opened(&app, &state)?;
    let client = state.hosting.client();
    let hosting = state.hosting.clone();
    let place = repo.clone();
    Ok(state.tasks.start(
        &place,
        TaskKind::Read,
        LOADING_PULL_REQUESTS.to_string(),
        move |ctx| -> Result<Page, GitError> {
            let check = || ctx.check_cancelled().map_err(|_| HostingError::Cancelled);
            list_session(&hosting, &base, &repo)
                .and_then(|session| {
                    list::page(&check, &client, &session, &value, cursor.as_deref())
                })
                .map_err(|error| list_error(list_failure(&hosting, &repo, error)))
        },
    ))
}

// The update check: the list's ETag, or none. Its failures are the interface's to ignore. Not a
// task, by the controller's ruling: one request every 30 s, bounded by the connect and read
// timeouts, which cancelling would not shorten, and a task every 30 s would crowd the status bar.
#[tauri::command]
pub async fn github_pr_etag(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<String>, ApiError> {
    let (base, repo) = opened(&app, &state)?;
    let client = state.hosting.client();
    let hosting = state.hosting.clone();
    blocking_api(move || {
        Ok(list_session(&hosting, &base, &repo)
            .and_then(|session| list::etag(&client, &session))
            .map_err(|error| list_error(list_failure(&hosting, &repo, error)))?)
    })
    .await
}

// What a pull request's tab shows under `Can't load details` or `Can't load changes`: `Re-Login`
// for an authentication error, else the error's own text (`localizedMessage`).
fn details_error(error: HostingError) -> GitError {
    match error {
        HostingError::Cancelled => GitError::Cancelled,
        HostingError::MissingToken => GitError::MissingToken,
        error if error.is_authentication() => GitError::Authentication(error.to_string()),
        other => GitError::Io(other.to_string()),
    }
}

// A request of a pull request's tab, in its connection's session: an authentication error drops
// the kept token.
fn in_session<T>(
    hosting: &HostingState,
    base: &Path,
    repo: &Path,
    request: impl FnOnce(&Session) -> Result<T, HostingError>,
) -> Result<T, GitError> {
    list_session(hosting, base, repo)
        .and_then(|session| request(&session))
        .map_err(|error| details_error(list_failure(hosting, repo, error)))
}

// `findPullRequest`: a task whose value is the `PullRequestDetails`.
#[tauri::command]
pub async fn github_pr_details(
    number: u64,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    let (base, repo) = opened(&app, &state)?;
    let connection = connection_of(&state, &repo)?;
    let client = state.hosting.client();
    let hosting = state.hosting.clone();
    let place = repo.clone();
    Ok(state.tasks.start(
        &place,
        TaskKind::Read,
        LOADING_DETAILS.to_string(),
        move |ctx| -> Result<PullRequestDetails, GitError> {
            let check = || ctx.check_cancelled().map_err(|_| HostingError::Cancelled);
            let loaded =
                load_details(&hosting, &client, &check, &base, &repo, &connection, number)?;
            Ok(loaded.as_ref().clone())
        },
    ))
}

fn connection_of(state: &State<'_, AppState>, repo: &Path) -> Result<Connection, ApiError> {
    state
        .hosting
        .connections
        .connection_of(repo)
        .ok_or_else(|| failed(NO_CONNECTION.to_string()))
}

// `findPullRequest`, kept for the branch actions and the checked-out icon of the same
// connection.
fn load_details(
    hosting: &HostingState,
    client: &Client,
    check: crate::hosting::http::Check,
    base: &Path,
    repo: &Path,
    connection: &Connection,
    number: u64,
) -> Result<Arc<PullRequestDetails>, GitError> {
    let loaded = Arc::new(in_session(hosting, base, repo, |session| {
        details::details(check, client, session, number)
    })?);
    hosting
        .pulls
        .set_details(repo, number, &connection.url, loaded.clone());
    Ok(loaded)
}

// The details the tab shows, or asked again when they are not kept.
fn shown_details(
    hosting: &HostingState,
    client: &Client,
    check: crate::hosting::http::Check,
    base: &Path,
    repo: &Path,
    connection: &Connection,
    number: u64,
) -> Result<Arc<PullRequestDetails>, GitError> {
    match hosting.pulls.details(repo, number, &connection.url) {
        Some(kept) => Ok(kept),
        None => load_details(hosting, client, check, base, repo, connection, number),
    }
}

// `Checkout '…'…` of the branch label (P§7.2). Like Update Project, the task holds the write lock
// only to add the remote and to check out, never while it talks to GitHub or waits for the
// remote's lock to fetch. Its value is a `Headed<CheckedOut>`; the questions of the checkout go
// back to the interface, which answers them through `branch_checkout_remote` with the same
// suggested name.
#[tauri::command]
pub async fn github_pr_checkout(
    number: u64,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let (base, repo) = opened(&app, &state)?;
    let connection = connection_of(&state, &repo)?;
    let data = data_dir_of(&app, &repo)?;
    let client = state.hosting.client();
    let hosting = state.hosting.clone();
    let handle = app.clone();
    let place = repo.clone();
    Ok(state.tasks.start(
        &place,
        TaskKind::Read,
        CHECKING_OUT.to_string(),
        move |ctx| -> Result<Headed<CheckedOut>, GitError> {
            let check = || ctx.check_cancelled().map_err(|_| HostingError::Cancelled);
            let shown =
                shown_details(&hosting, &client, &check, &base, &repo, &connection, number)?;
            let state = handle.state::<AppState>();
            let flush = || {
                tauri::async_runtime::block_on(flush_diff_window(&handle, &state, SAVE_ACTION))
                    .map_err(|error| GitError::Io(error.message))
            };
            let target = Repo {
                ctx,
                path: &repo,
                data: &data,
            };
            let server = &connection.coordinates.server;
            let result = checkout::checkout(&target, &state.branch_sync, server, &shown, flush);
            crate::commands::repo_changed(&handle);
            result.map_err(git_failure)
        },
    ))
}

// `Show '…' in Git Log` (P§7.5): the head's remote is found or added, under the write lock as
// `remote_add` writes the configuration, and the head fetched without it. Its value is a
// `Headed<Range>`, which the interface opens as a log tab.
#[tauri::command]
pub async fn github_pr_show_in_log(
    number: u64,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let (base, repo) = opened(&app, &state)?;
    let connection = connection_of(&state, &repo)?;
    let client = state.hosting.client();
    let hosting = state.hosting.clone();
    let handle = app.clone();
    let place = repo.clone();
    Ok(state.tasks.start(
        &place,
        TaskKind::Read,
        SHOWING_IN_LOG.to_string(),
        move |ctx| -> Result<Headed<Range>, GitError> {
            let check = || ctx.check_cancelled().map_err(|_| HostingError::Cancelled);
            let shown =
                shown_details(&hosting, &client, &check, &base, &repo, &connection, number)?;
            let sync = &handle.state::<AppState>().branch_sync;
            let server = &connection.coordinates.server;
            let result = checkout::show_in_log(ctx, sync, server, &shown);
            // A remote may have been added, and the fetch moved a remote branch.
            crate::commands::repo_changed(&handle);
            result.map_err(git_failure)
        },
    ))
}

// The checked-out icon (P§7.6), from the details the tab loaded: without them, or without a
// connection, it is not checked out. No request to GitHub: it is asked on every `repo-changed`.
#[tauri::command]
pub async fn github_pr_checked_out(
    number: u64,
    state: State<'_, AppState>,
) -> Result<bool, ApiError> {
    let repo = current_repo(&state)?;
    let Some(connection) = state.hosting.connections.connection_of(&repo) else {
        return Ok(false);
    };
    let Some(shown) = state.hosting.pulls.details(&repo, number, &connection.url) else {
        return Ok(false);
    };
    blocking_api(move || {
        Ok(checkout::is_checked_out(
            &repo,
            &connection.coordinates.server,
            &shown,
        )?)
    })
    .await
}

// The status lines' data: a task whose value is the `PullRequestStatus`. 241 swallows its errors;
// the interface does too.
#[tauri::command]
pub async fn github_pr_status(
    number: u64,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    let (base, repo) = opened(&app, &state)?;
    let client = state.hosting.client();
    let hosting = state.hosting.clone();
    let place = repo.clone();
    Ok(state.tasks.start(
        &place,
        TaskKind::Read,
        LOADING_STATUS.to_string(),
        move |ctx| -> Result<PullRequestStatus, GitError> {
            let check = || ctx.check_cancelled().map_err(|_| HostingError::Cancelled);
            in_session(&hosting, &base, &repo, |session| {
                details::status(&check, &client, session, number)
            })
        },
    ))
}

fn load_commits(
    hosting: &HostingState,
    client: &Client,
    check: crate::hosting::http::Check,
    base: &Path,
    repo: &Path,
    number: u64,
) -> Result<Arc<Vec<PullRequestCommit>>, GitError> {
    let commits = Arc::new(in_session(hosting, base, repo, |session| {
        details::commits(check, client, session, number)
    })?);
    hosting.pulls.set_commits(repo, number, commits.clone());
    Ok(commits)
}

// `pullRequestCommits`: a task whose value is the commits in P§6.5's order. They are kept for the
// changes of one commit.
#[tauri::command]
pub async fn github_pr_commits(
    number: u64,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    let (base, repo) = opened(&app, &state)?;
    let client = state.hosting.client();
    let hosting = state.hosting.clone();
    let place = repo.clone();
    Ok(state.tasks.start(
        &place,
        TaskKind::Read,
        LOADING_COMMITS.to_string(),
        move |ctx| -> Result<Vec<PullRequestCommit>, GitError> {
            let check = || ctx.check_cancelled().map_err(|_| HostingError::Cancelled);
            let commits = load_commits(&hosting, &client, &check, &base, &repo, number)?;
            Ok(commits.as_ref().clone())
        },
    ))
}

fn git_failure(error: GitError) -> GitError {
    match error {
        error @ GitError::Failed { .. } => GitError::Io(crate::fetch::error_text(error)),
        error => error,
    }
}

// The changes tree: a task whose value is the `PullRequestChanges`. The whole pull request
// (`selection` none: the first load, `Refresh Pull Request` and `Retry`) always fetches the base
// branch and the head first; a commit reuses the fetches of the same base and head, and the
// commits the dropdown loaded. `base_ref` is the base branch's name in `findPullRequest`; the
// hashes are checked before git sees them.
#[tauri::command]
pub async fn github_pr_changes(
    number: u64,
    base_ref: String,
    base_oid: String,
    head_oid: String,
    selection: Option<usize>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    let (base, repo) = opened(&app, &state)?;
    crate::diff::check_hash(&base_oid)?;
    crate::diff::check_hash(&head_oid)?;
    let remote = state
        .hosting
        .connections
        .connection_of(&repo)
        .map(|connection| connection.remote)
        .ok_or_else(|| failed(NO_CONNECTION.to_string()))?;
    let client = state.hosting.client();
    let hosting = state.hosting.clone();
    let handle = app.clone();
    let place = repo.clone();
    Ok(state.tasks.start(
        &place,
        TaskKind::Read,
        LOADING_CHANGES.to_string(),
        move |ctx| -> Result<PullRequestChanges, GitError> {
            let fetched = selection.is_some()
                && hosting
                    .pulls
                    .was_fetched(&repo, number, &base_oid, &head_oid);
            if !fetched {
                hosting.pulls.set_fetched(&repo, number, None);
                let sync = &handle.state::<AppState>().branch_sync;
                let result = changes::fetch(ctx, sync, &remote, &base_ref, number);
                // The fetches moved remote branches.
                crate::commands::repo_changed(&handle);
                result.map_err(git_failure)?;
                hosting.pulls.set_fetched(
                    &repo,
                    number,
                    Some((base_oid.clone(), head_oid.clone())),
                );
            }
            let commits = match selection {
                None => Arc::new(Vec::new()),
                Some(_) => match hosting.pulls.commits(&repo, number, &head_oid) {
                    Some(commits) => commits,
                    None => {
                        let check = || ctx.check_cancelled().map_err(|_| HostingError::Cancelled);
                        let commits =
                            load_commits(&hosting, &client, &check, &base, &repo, number)?;
                        if commits.last().is_none_or(|last| last.oid != head_oid) {
                            return Err(GitError::Io(HEAD_MOVED.to_string()));
                        }
                        commits
                    }
                },
            };
            let found = changes::changes(&repo, &base_oid, &head_oid, &commits, selection)
                .map_err(git_failure)?;
            let body = match selection {
                Some(at) => {
                    Some(changes::commit_body(&repo, &commits[at].oid).map_err(git_failure)?)
                }
                None => None,
            };
            Ok(PullRequestChanges {
                changes: found,
                body,
            })
        },
    ))
}

// The values of the `Author`, `Assignee` or `Label` chooser: a task, cancelled when the dropdown
// closes, whose value is the list. A failure shows its message in the chooser.
#[tauri::command]
pub async fn github_pr_chooser(
    kind: ChooserKind,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    let (base, repo) = opened(&app, &state)?;
    let client = state.hosting.client();
    let hosting = state.hosting.clone();
    let place = repo.clone();
    Ok(state.tasks.start(
        &place,
        TaskKind::Read,
        chooser_title(kind).to_string(),
        move |ctx| -> Result<Vec<String>, GitError> {
            let check = || ctx.check_cancelled().map_err(|_| HostingError::Cancelled);
            list_session(&hosting, &base, &repo)
                .and_then(|session| list::choosers(&check, &client, &session, kind))
                .map_err(|error| match list_failure(&hosting, &repo, error) {
                    HostingError::Cancelled => GitError::Cancelled,
                    error => GitError::Io(error.to_string()),
                })
        },
    ))
}

// rama's own progress texts: 241 loads the choosers inside their popups.
fn chooser_title(kind: ChooserKind) -> &'static str {
    match kind {
        ChooserKind::Collaborators => "Loading collaborators",
        ChooserKind::Assignees => "Loading assignees",
        ChooserKind::Labels => "Loading labels",
    }
}

// The list's last search and its history, for the open repository (decision 10).
#[tauri::command]
pub async fn github_pr_search_saved(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<SavedSearch, ApiError> {
    let data = data_dir_of(&app, &current_repo(&state)?)?;
    blocking_api(move || Ok(list::saved_search(&data))).await
}

// The search changed: it becomes the last one, and with `remember` it goes into the history.
#[tauri::command]
pub async fn github_pr_search_save(
    value: SearchValue,
    remember: bool,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<SavedSearch, ApiError> {
    let data = data_dir_of(&app, &current_repo(&state)?)?;
    blocking_api(move || {
        list::save_search(&data, &value, remember)
            .map_err(|error| failed(format!("Could not save the pull requests search: {error}")))
    })
    .await
}

#[tauri::command]
pub async fn open_url(url: String) -> Result<(), ApiError> {
    blocking_api(move || {
        let status = open_url_command(&url)
            .map_err(failed)?
            .status()
            .map_err(|error| failed(format!("Could not open the browser: {error}")))?;
        if status.success() {
            Ok(())
        } else {
            Err(failed(format!("open exited with {status}")))
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;
    use crate::secrets::MemoryStore;
    use crate::test_support::{FakeGitHub, Reply};

    const TOKEN: &str = "ghp_COMMANDS0123456789";

    fn client() -> Client {
        Client::new(Duration::from_secs(5))
    }

    fn no_check() -> Result<(), HostingError> {
        Ok(())
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

    fn add(
        fake: &FakeGitHub,
        store: &MemoryStore,
        base: &Path,
        repo_dir: Option<&Path>,
        login: &str,
        editing: Option<&str>,
    ) -> AccountView {
        fake.reply(user(login));
        let target = match editing {
            Some(id) => LoginTarget::Edit(id.to_string()),
            None => LoginTarget::Add(GithubServer::parse(&fake.url()).unwrap()),
        };
        log_in(
            &client(),
            store,
            base,
            repo_dir,
            &target,
            Secret::new(TOKEN.to_string()),
            &no_check,
        )
        .unwrap()
    }

    #[test]
    fn removing_an_account_deletes_its_keychain_item_and_clears_the_default() {
        let fake = FakeGitHub::new();
        let store = MemoryStore::new();
        let base = TempDir::new().unwrap();
        let repo_dir = TempDir::new().unwrap();
        let octocat = add(
            &fake,
            &store,
            base.path(),
            Some(repo_dir.path()),
            "octocat",
            None,
        );
        let hubot = add(
            &fake,
            &store,
            base.path(),
            Some(repo_dir.path()),
            "hubot",
            None,
        );
        let service = format!("rama GitHub {}", fake.url().trim_start_matches("http://"));
        set_default(base.path(), repo_dir.path(), &octocat.id).unwrap();

        // Removing another account keeps the default.
        remove_account(&store, base.path(), Some(repo_dir.path()), &hubot.id).unwrap();
        assert!(store.get(&service, "hubot").unwrap().is_none());
        assert_eq!(default_of(Some(repo_dir.path())), Some(octocat.id.clone()));

        remove_account(&store, base.path(), Some(repo_dir.path()), &octocat.id).unwrap();
        assert!(store.get(&service, "octocat").unwrap().is_none());
        assert!(accounts::list(base.path()).unwrap().is_empty());
        assert_eq!(default_of(Some(repo_dir.path())), None);
        // What the rest of the repository's settings said is kept.
        assert_eq!(
            settings::load_repo(repo_dir.path()).protected_branches,
            ["master", "main"]
        );
        fake.finish();
    }

    #[test]
    fn the_default_is_a_listed_account_and_follows_a_relogin() {
        let fake = FakeGitHub::new();
        let store = MemoryStore::new();
        let base = TempDir::new().unwrap();
        let repo_dir = TempDir::new().unwrap();
        let octocat = add(
            &fake,
            &store,
            base.path(),
            Some(repo_dir.path()),
            "octocat",
            None,
        );
        assert!(!octocat.is_default);
        assert_eq!(octocat.name, None);

        assert_eq!(
            set_default(base.path(), repo_dir.path(), "github.com/nobody").unwrap_err(),
            "The account github.com/nobody is no longer in the list"
        );
        assert_eq!(default_of(Some(repo_dir.path())), None);
        set_default(base.path(), repo_dir.path(), &octocat.id).unwrap();

        let renamed = add(
            &fake,
            &store,
            base.path(),
            Some(repo_dir.path()),
            "monalisa",
            Some(&octocat.id),
        );
        assert!(renamed.is_default);
        assert_eq!(default_of(Some(repo_dir.path())), Some(renamed.id.clone()));

        fake.reply(user("monalisa"));
        let listed = row_with_details(
            &client(),
            &store,
            base.path(),
            Some(&renamed.id),
            &renamed.id,
        )
        .unwrap();
        assert_eq!(
            listed,
            AccountView {
                id: renamed.id.clone(),
                server: fake.url(),
                login: "monalisa".to_string(),
                name: Some("The monalisa".to_string()),
                error: None,
                needs_login: false,
                is_default: true,
            }
        );
        fake.finish();
    }

    #[test]
    fn a_default_that_cannot_follow_a_relogin_does_not_fail_it() {
        use std::os::unix::fs::PermissionsExt;
        let fake = FakeGitHub::new();
        let store = MemoryStore::new();
        let base = TempDir::new().unwrap();
        let repo_dir = TempDir::new().unwrap();
        let octocat = add(
            &fake,
            &store,
            base.path(),
            Some(repo_dir.path()),
            "octocat",
            None,
        );
        set_default(base.path(), repo_dir.path(), &octocat.id).unwrap();

        let mode = |mode| fs::set_permissions(repo_dir.path(), fs::Permissions::from_mode(mode));
        mode(0o555).unwrap();
        let renamed = add(
            &fake,
            &store,
            base.path(),
            Some(repo_dir.path()),
            "monalisa",
            Some(&octocat.id),
        );
        mode(0o755).unwrap();
        assert_eq!(renamed.login, "monalisa");
        assert!(!renamed.is_default);
        assert_eq!(accounts::list(base.path()).unwrap().len(), 1);
        assert_eq!(default_of(Some(repo_dir.path())), Some(octocat.id));
        fake.finish();
    }

    #[test]
    fn a_relogin_does_not_read_the_fixed_server_text() {
        assert_eq!(
            login_target("", Some("github.com/octocat".to_string())).unwrap(),
            LoginTarget::Edit("github.com/octocat".to_string())
        );
        assert_eq!(
            login_target(" ghe.example.com ", None).unwrap(),
            LoginTarget::Add(GithubServer::parse("ghe.example.com").unwrap())
        );
        assert_eq!(
            login_target("", None).unwrap_err().message,
            "Server cannot be empty"
        );
    }

    #[test]
    fn no_command_returns_a_token() {
        // No return type and no serialized type of these commands can carry a `Secret`.
        let source = include_str!("commands.rs");
        let code = source.split("#[cfg(test)]\nmod tests").next().unwrap();
        let returns: Vec<&str> = code
            .lines()
            .filter(|line| line.contains("-> Result<"))
            .collect();
        assert!(returns.len() >= 6, "{returns:?}");
        for line in returns {
            assert!(!line.contains("Secret"), "{line}");
        }
        let mut serialized = 0;
        for block in code.split("#[derive(").skip(1) {
            let (derives, rest) = block.split_once(')').unwrap();
            if derives.contains("Serialize") {
                serialized += 1;
                let body = rest.split("\n}\n").next().unwrap();
                assert!(!body.contains("Secret"), "{body}");
            }
        }
        assert_eq!(serialized, 1);

        // And what the commands send and give back has it only in `Authorization: Bearer`.
        let fake = FakeGitHub::new();
        let store = MemoryStore::new();
        let base = TempDir::new().unwrap();
        let added = add(&fake, &store, base.path(), None, "octocat", None);
        fake.reply(Reply::json(
            "GET",
            "/api/v3/user",
            401,
            "Unauthorized",
            &format!(r#"{{"message":"Bad credentials {TOKEN}"}}"#),
        ));
        let listed = row_with_details(&client(), &store, base.path(), None, &added.id).unwrap();
        assert_eq!(
            listed.error.as_deref(),
            Some("Request response: Bad credentials ***.")
        );
        let given_back = serde_json::to_string(&(added, listed)).unwrap();
        assert!(!given_back.contains(TOKEN), "{given_back}");
        let file = fs::read_to_string(base.path().join("github-accounts.json")).unwrap();
        assert!(!file.contains(TOKEN), "{file}");

        let requests = fake.requests();
        assert_eq!(requests.len(), 2);
        for request in &requests {
            assert!(!request.target.contains(TOKEN));
            assert!(!request.body.contains(TOKEN));
            for (name, value) in &request.headers {
                if name.eq_ignore_ascii_case("authorization") {
                    assert_eq!(value, &format!("Bearer {TOKEN}"));
                } else {
                    assert!(!value.contains(TOKEN), "{name}: {value}");
                }
            }
        }
        fake.finish();
    }

    #[test]
    fn the_list_makes_no_request_and_marks_the_account_without_a_token() {
        let fake = FakeGitHub::new();
        let store = MemoryStore::new();
        let base = TempDir::new().unwrap();
        let octocat = add(&fake, &store, base.path(), None, "octocat", None);
        let hubot = add(&fake, &store, base.path(), None, "hubot", None);
        let service = format!("rama GitHub {}", fake.url().trim_start_matches("http://"));
        store.delete(&service, "hubot").unwrap();

        let listed = rows(&store, base.path(), Some(&octocat.id)).unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(
            (
                listed[0].login.as_str(),
                listed[0].is_default,
                listed[0].needs_login
            ),
            ("octocat", true, false)
        );
        assert_eq!(listed[0].server, fake.url());
        assert_eq!(listed[0].name, None);
        assert_eq!(listed[0].error, None);
        assert_eq!(
            (
                listed[1].id.as_str(),
                listed[1].is_default,
                listed[1].needs_login
            ),
            (hubot.id.as_str(), false, true)
        );
        // The two logins asked once each; the list asked nothing.
        assert_eq!(fake.requests().len(), 2);
        fake.finish();
    }

    // The default-account check of a repository open needs the ids only: the rows without tokens
    // read no Keychain item (an unsigned build would prompt on every open), and keep the default.
    #[test]
    fn the_rows_without_tokens_read_no_keychain_item() {
        let fake = FakeGitHub::new();
        let memory = MemoryStore::new();
        let base = TempDir::new().unwrap();
        let octocat = add(&fake, &memory, base.path(), None, "octocat", None);
        let hubot = add(&fake, &memory, base.path(), None, "hubot", None);
        let store = CountingStore::default();

        let listed = account_rows(&store, base.path(), Some(&hubot.id), false).unwrap();

        assert_eq!(store.reads(), 0);
        let rows: Vec<(&str, bool, bool)> = listed
            .iter()
            .map(|row| (row.id.as_str(), row.is_default, row.needs_login))
            .collect();
        assert_eq!(
            rows,
            [
                (octocat.id.as_str(), false, false),
                (hubot.id.as_str(), true, false)
            ]
        );
        // With the tokens, as Settings asks: one read per account.
        account_rows(&store, base.path(), None, true).unwrap();
        assert_eq!(store.reads(), 2);
        fake.finish();
    }

    #[test]
    fn a_row_loads_its_details_on_its_own() {
        let fake = FakeGitHub::new();
        let store = MemoryStore::new();
        let base = TempDir::new().unwrap();
        let octocat = add(&fake, &store, base.path(), None, "octocat", None);

        fake.reply(Reply::json(
            "GET",
            "/api/v3/user",
            401,
            "Unauthorized",
            r#"{"message":"Bad credentials"}"#,
        ));
        let failing = row_with_details(&client(), &store, base.path(), None, &octocat.id).unwrap();
        assert_eq!(
            failing.error.as_deref(),
            Some("Request response: Bad credentials.")
        );
        assert!(failing.needs_login);

        assert_eq!(
            row_with_details(&client(), &store, base.path(), None, "github.com/nobody")
                .unwrap_err(),
            "The account github.com/nobody is no longer in the list"
        );
        fake.finish();
    }

    #[test]
    fn the_token_is_trimmed_before_it_goes_out() {
        let fake = FakeGitHub::new();
        let store = MemoryStore::new();
        let base = TempDir::new().unwrap();
        fake.reply(user("octocat"));
        log_in(
            &client(),
            &store,
            base.path(),
            None,
            &LoginTarget::Add(GithubServer::parse(&fake.url()).unwrap()),
            Secret::new(clean_token(&format!("  {TOKEN}\r\n")).unwrap().to_string()),
            &no_check,
        )
        .unwrap();
        let requests = fake.requests();
        let authorization = requests[0]
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
            .unwrap();
        assert_eq!(authorization.1, format!("Bearer {TOKEN}"));
        fake.finish();

        let blank = clean_token(" \n").err().unwrap();
        assert_eq!(
            (blank.kind, blank.message.as_str()),
            ("login_token", "Token cannot be empty")
        );
    }

    #[test]
    fn a_login_error_names_the_field_it_belongs_to() {
        let kind = |error: LoginError| ApiError::from(login_error(error)).kind;
        assert_eq!(kind(LoginError::InvalidServer), "login_server");
        assert_eq!(
            kind(LoginError::Credentials("x".to_string())),
            "login_token"
        );
        assert_eq!(
            kind(LoginError::AlreadyAdded("x".to_string())),
            "login_token"
        );
        assert_eq!(kind(LoginError::Unreachable), "login_token");
        assert_eq!(kind(LoginError::Cancelled), "cancelled");
        // What is wrong before the task starts says it too.
        assert_eq!(login_target("", None).unwrap_err().kind, "login_server");
        assert_eq!(
            login_target("ghe.example.com:port", None).unwrap_err().kind,
            "login_server"
        );
    }

    #[test]
    fn a_connect_failure_keeps_its_kind() {
        let kind = |error: HostingError| ApiError::from(connect_error(error));
        assert_eq!(kind(HostingError::Cancelled).kind, "cancelled");
        let missing = kind(HostingError::MissingToken);
        assert_eq!(
            (missing.kind, missing.message.as_str()),
            ("missing_token", "Missing access token")
        );
        let other = kind(HostingError::Unreachable);
        assert_eq!(
            (other.kind, other.message.as_str()),
            ("failed", "Server is unreachable")
        );
    }

    // The Keychain of the tests, counting its reads, or refusing them (a denied prompt).
    #[derive(Default)]
    struct CountingStore {
        inner: MemoryStore,
        reads: std::sync::atomic::AtomicUsize,
        denied: std::sync::atomic::AtomicBool,
    }

    impl CountingStore {
        fn reads(&self) -> usize {
            self.reads.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl SecretStore for CountingStore {
        fn get(
            &self,
            service: &str,
            account: &str,
        ) -> Result<Option<Secret>, crate::secrets::SecretError> {
            self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self.denied.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(crate::secrets::SecretError(
                    "User interaction is not allowed.".to_string(),
                ));
            }
            self.inner.get(service, account)
        }

        fn set(
            &self,
            service: &str,
            account: &str,
            secret: &Secret,
        ) -> Result<(), crate::secrets::SecretError> {
            self.inner.set(service, account, secret)
        }

        fn delete(&self, service: &str, account: &str) -> Result<(), crate::secrets::SecretError> {
            self.inner.delete(service, account)
        }
    }

    fn empty_page() -> Reply {
        Reply::json(
            "POST",
            "/api/graphql",
            200,
            "OK",
            r#"{"data":{"search":{"pageInfo":{"endCursor":null,"hasNextPage":false},"nodes":[]}}}"#,
        )
    }

    fn etag_reply() -> Reply {
        Reply::new(
            "GET",
            "/api/v3/repos/o/r/pulls?page=1&per_page=1",
            200,
            "OK",
            &[("ETag", "W/\"a\""), ("Content-Type", "application/json")],
            "[]",
        )
    }

    // A repository whose `origin` is `o/r` on the fake, an account with a token, and a connect.
    struct Connected {
        fake: FakeGitHub,
        repo: crate::test_support::TempRepo,
        base: TempDir,
        store: Arc<CountingStore>,
        hosting: HostingState,
        account: GithubAccount,
    }

    fn connected() -> Connected {
        let fake = FakeGitHub::new();
        let repo = crate::test_support::TempRepo::new();
        repo.git(&[
            "remote",
            "add",
            "origin",
            &format!("{}/o/r.git", fake.url()),
        ]);
        let base = TempDir::new().unwrap();
        let data = TempDir::new().unwrap();
        let store = Arc::new(CountingStore::default());
        let account = GithubAccount {
            server: GithubServer::parse(&fake.url()).unwrap(),
            login: "octocat".to_string(),
        };
        accounts::save(base.path(), std::slice::from_ref(&account)).unwrap();
        store
            .set(
                &accounts::service(&account.server),
                "octocat",
                &Secret::new(TOKEN.to_string()),
            )
            .unwrap();
        let hosting = HostingState::new(store.clone());
        fake.script(vec![
            user("octocat"),
            Reply::json(
                "POST",
                "/api/graphql",
                200,
                "OK",
                r#"{"data":{"repository":{"id":"R_1","owner":{"login":"o"},"nameWithOwner":"o/r","viewerPermission":"WRITE","mergeCommitAllowed":true,"squashMergeAllowed":true,"rebaseMergeAllowed":true,"defaultBranchRef":{"name":"main"},"isFork":false,"url":"https://github.com/o/r","sshUrl":"git@github.com:o/r.git"}}}"#,
            ),
        ]);
        let ticket = hosting.connections.start();
        let opened = pulls::open(
            &no_check,
            &client(),
            store.as_ref(),
            &hosting.users,
            repo.path(),
            base.path(),
            data.path(),
            "origin",
            &account.id(),
        )
        .unwrap();
        assert!(hosting
            .connections
            .opened(repo.path(), ticket, opened.connection, opened.token));
        Connected {
            fake,
            repo,
            base,
            store,
            hosting,
            account,
        }
    }

    impl Connected {
        fn session(&self) -> Result<Session, HostingError> {
            list_session(&self.hosting, self.base.path(), self.repo.path())
        }
    }

    #[test]
    fn the_token_is_read_once_for_many_pages_and_polls() {
        let place = connected();
        // The connect read it.
        assert_eq!(place.store.reads(), 1);
        for _ in 0..3 {
            place.fake.script(vec![empty_page(), etag_reply()]);
            let session = place.session().unwrap();
            list::page(
                &no_check,
                &client(),
                &session,
                &SearchValue::default(),
                None,
            )
            .unwrap();
            let session = place.session().unwrap();
            assert!(list::etag(&client(), &session).unwrap().is_some());
        }
        assert_eq!(place.store.reads(), 1);
        place.fake.finish();
        // Every request carried the token.
        assert!(place.fake.requests().iter().skip(2).all(|request| {
            request.header("authorization") == Some(format!("Bearer {TOKEN}").as_str())
        }));
    }

    #[test]
    fn the_kept_token_goes_after_a_relogin_or_a_refused_request() {
        let place = connected();
        // A re-login stores a new token: the list reads it, once.
        let renewed = "ghp_RENEWED0123456789";
        place
            .store
            .set(
                &accounts::service(&place.account.server),
                "octocat",
                &Secret::new(renewed.to_string()),
            )
            .unwrap();
        place.hosting.accounts_changed();
        assert_eq!(place.session().unwrap().token.expose(), renewed);
        assert_eq!(place.session().unwrap().token.expose(), renewed);
        assert_eq!(place.store.reads(), 2);

        // GitHub refuses it: the next request reads the store again.
        let refused = list_failure(
            &place.hosting,
            place.repo.path(),
            HostingError::Authentication("Bad credentials".to_string()),
        );
        assert_eq!(ApiError::from(list_error(refused)).kind, "authentication");
        place.session().unwrap();
        assert_eq!(place.store.reads(), 3);
        // Another failure keeps it.
        list_failure(&place.hosting, place.repo.path(), HostingError::Unreachable);
        place.session().unwrap();
        assert_eq!(place.store.reads(), 3);
    }

    #[test]
    fn a_pull_request_tab_request_refused_by_github_drops_the_kept_token() {
        let place = connected();
        place.fake.reply(Reply::json(
            "POST",
            "/api/graphql",
            401,
            "Unauthorized",
            r#"{"message":"Bad credentials"}"#,
        ));
        let check = || Ok(());

        let error = in_session(
            &place.hosting,
            place.base.path(),
            place.repo.path(),
            |session| details::details(&check, &client(), session, 7),
        )
        .unwrap_err();

        // `Can't load details` offers `Re-Login`, and the next request reads the store again.
        assert_eq!(ApiError::from(error).kind, "authentication");
        let reads = place.store.reads();
        place.session().unwrap();
        assert_eq!(place.store.reads(), reads + 1);
        place.fake.finish();
    }

    #[test]
    fn a_denied_keychain_read_is_not_reported_as_a_missing_token() {
        let place = connected();
        place.hosting.accounts_changed();
        place
            .store
            .denied
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let error = match place.session() {
            Ok(_) => panic!("a session opened"),
            Err(error) => error,
        };
        let shown = ApiError::from(list_error(error));
        assert_eq!(shown.kind, "failed");
        assert_eq!(
            shown.message,
            "Could not read the access token from the Keychain: User interaction is not allowed."
        );
        // Nothing was kept: once the Keychain answers, the token is read.
        place
            .store
            .denied
            .store(false, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(place.session().unwrap().token.expose(), TOKEN);
    }

    #[test]
    fn the_list_asks_only_through_its_repositorys_connection() {
        let place = connected();
        let other = TempDir::new().unwrap();
        match list_session(&place.hosting, place.base.path(), other.path()) {
            Err(error) => assert_eq!(error.to_string(), NO_CONNECTION),
            Ok(_) => panic!("another repository has a session"),
        }
        place.hosting.connections.forget(place.repo.path());
        assert!(place.session().is_err());
    }

    #[test]
    fn a_list_failure_keeps_whether_it_is_an_authentication_error() {
        let kind = |error: HostingError| ApiError::from(list_error(error)).kind;
        assert_eq!(
            kind(HostingError::Authentication("Bad credentials".to_string())),
            "authentication"
        );
        assert_eq!(kind(HostingError::Scopes), "authentication");
        assert_eq!(kind(HostingError::MissingToken), "missing_token");
        assert_eq!(kind(HostingError::Cancelled), "cancelled");
        let failure = ApiError::from(list_error(HostingError::Unreachable));
        assert_eq!(failure.kind, "failed");
        assert_eq!(failure.message, "Server is unreachable");
        let auth = ApiError::from(list_error(HostingError::Authentication(
            "Bad credentials".to_string(),
        )));
        assert_eq!(auth.message, "Request response: Bad credentials.");
    }

    #[test]
    fn open_url_refuses_a_dash_and_other_schemes() {
        let command = open_url_command("https://github.com/o/r/pull/1").unwrap();
        assert_eq!(command.get_program(), OPEN);
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["-u", "https://github.com/o/r/pull/1"]
        );
        let plain = open_url_command("http://ghe.example.com/o/r").unwrap();
        assert_eq!(
            plain.get_args().collect::<Vec<_>>(),
            ["-u", "http://ghe.example.com/o/r"]
        );

        for refused in [
            "-a",
            "--args https://github.com",
            "-https://github.com",
            "javascript:alert(1)",
            "file:///etc/passwd",
            "ftp://github.com",
            "x-github-client://openRepo",
            "github.com/o/r",
            "/Applications/Calculator.app",
            "https://",
            "",
            " https://github.com",
            "https://user:secret@github.com/o/r",
            "https://user@github.com/o/r",
            "https://:secret@github.com/o/r",
            "http://ghe.example.com@evil.test/",
        ] {
            assert!(open_url_command(refused).is_err(), "{refused}");
        }
    }
}
