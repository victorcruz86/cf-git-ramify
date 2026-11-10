// GitHub accounts: the list (server and login, in the app's data folder) and their tokens (in the
// secret store, never on disk). Ports GHAccountManager, GHAccountsDetailsProvider,
// GHTokenCredentialsUi.acquireLogin and handleError, and GHSecurityUtil.buildNewTokenUrl
// (plugins/github at idea/241.14494.240). rama's own choices are decisions 1 to 3 of the part 5
// plan: a change is saved at once, an account is `<server>/<login>`, and the list lives in
// `github-accounts.json`.

use std::fmt;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::hosting::http::{Client, HostingError, PARSE_FAILED};
use crate::hosting::server::GithubServer;
use crate::secrets::{Secret, SecretError, SecretStore};
use crate::storage;

const FILE_NAME: &str = "github-accounts.json";
const CORRUPT_FILE_NAME: &str = "github-accounts.json.corrupt";

// The texts of 241 (GithubBundle 32, 35, 38, 39 and 331, CollaborationToolsBundle 8).
const ALREADY_ADDED: &str = "Account '{0}' already added";
const INVALID_SERVER: &str = "Server URL is not valid";
const INCORRECT_CREDENTIALS: &str = "Incorrect credentials.\n";
const INVALID_DATA: &str = "Invalid authentication data.\n";
const MISSING_TOKEN: &str = "Missing access token";
// `CurrentUser`'s operation name, which GithubApiRequestExecutor puts before a parse failure.
const PROFILE_OPERATION: &str = "Can't get profile information";
// GHSecurityUtil.MASTER_SCOPES, and rama's name where 241 has `fullProductName` (decision 12),
// already encoded as URLUtil.encodeURIComponent does: space `%20`, `,` `%2C` and `:` `%3A`.
const NEW_TOKEN_QUERY: &str = "description=rama%20GitHub%20integration%20plugin\
     &scopes=repo%2Cgist%2Cread%3Aorg%2Cworkflow%2Cread%3Auser%2Cuser%3Aemail";

// Reading, changing and writing the list is one step: two logins at once must not lose one.
static LIST: Mutex<()> = Mutex::new(());

// An account: who (`login`) on which server. Its token is in the secret store, under
// `service(server)` and the login.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubAccount {
    pub server: GithubServer,
    pub login: String,
}

impl GithubAccount {
    // 241's `toString`, with the server always without its scheme (decision 2): `github.com/octocat`.
    pub fn id(&self) -> String {
        format!("{}/{}", self.server.address(), self.login)
    }
}

// The Keychain item's service. Without the scheme, as the id: `http://` and `https://` of one host
// are one server (P§1.3).
pub(crate) fn service(server: &GithubServer) -> String {
    format!("rama GitHub {}", server.address())
}

// What a login can end with. The texts are those of GHTokenCredentialsUi.handleError, except
// `Saving`, which 241 does not show (it swallows the exceptions of Apply).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginError {
    InvalidServer,
    AlreadyAdded(String),
    Unreachable,
    Credentials(String),
    Invalid(String),
    UnknownAccount(String),
    Saving(String),
    Cancelled,
}

impl fmt::Display for LoginError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoginError::InvalidServer => formatter.write_str(INVALID_SERVER),
            LoginError::AlreadyAdded(login) => {
                formatter.write_str(&ALREADY_ADDED.replace("{0}", login))
            }
            LoginError::Unreachable => formatter.write_str(&HostingError::Unreachable.to_string()),
            LoginError::Credentials(message) => {
                write!(formatter, "{INCORRECT_CREDENTIALS}{message}")
            }
            LoginError::Invalid(message) => write!(formatter, "{INVALID_DATA}{message}"),
            LoginError::UnknownAccount(id) => {
                write!(formatter, "The account {id} is no longer in the list")
            }
            LoginError::Saving(message) => {
                write!(formatter, "Could not save the account: {message}")
            }
            LoginError::Cancelled => formatter.write_str(&HostingError::Cancelled.to_string()),
        }
    }
}

// What the accounts list shows of an account besides its login (GHAccountsDetailsProvider): the
// full name, or the error in red and whether it offers `Log in`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountDetails {
    pub name: Option<String>,
    pub error: Option<String>,
    pub needs_login: bool,
}

// A damaged file is set aside and the list starts empty, as every other file of the app.
pub fn list(base: &Path) -> io::Result<Vec<GithubAccount>> {
    storage::load_json(base, FILE_NAME, CORRUPT_FILE_NAME)
}

pub fn save(base: &Path, accounts: &[GithubAccount]) -> io::Result<()> {
    fs::create_dir_all(base)?;
    let json = serde_json::to_vec_pretty(accounts).map_err(io::Error::other)?;
    storage::write_atomic(&base.join(FILE_NAME), &json)
}

// The servers whose repositories a remote can point to: github.com, which is always known, and the
// server of each account (plan decision 4).
pub fn known_servers(accounts: &[GithubAccount]) -> Vec<GithubServer> {
    let mut servers = vec![GithubServer::github_com()];
    for account in accounts {
        if !servers.contains(&account.server) {
            servers.push(account.server.clone());
        }
    }
    servers
}

// The token of an account, if the store has one. A store that fails counts as no token, as in
// GHAccountsDetailsProvider.
pub fn token(store: &dyn SecretStore, account: &GithubAccount) -> Option<Secret> {
    read_token(store, account).ok().flatten()
}

// The token of an account: `None` when the store has none, and the store's own error when it
// could not be read (a Keychain prompt that was denied, for example), which is not a missing
// token.
pub fn read_token(
    store: &dyn SecretStore,
    account: &GithubAccount,
) -> Result<Option<Secret>, SecretError> {
    store.get(&service(&account.server), &account.login)
}

// What a place that needs the token shows when the store failed.
pub fn keychain_failure(error: &SecretError) -> String {
    format!("Could not read the access token from the Keychain: {error}")
}

// Who logs in: a new account on the typed server (`Log In with Token…`, `Log In to GitHub
// Enterprise…`), or the account of a row's `Log in`, by its id, whose server is fixed
// (`isServerEditable = false`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginTarget {
    Add(GithubServer),
    Edit(String),
}

// Asks `GET <api>/user` with the token and takes its `login`, then applies 241's rule that no
// other account has that login on that server, ignoring the scheme. The token is stored only
// then. Editing renames the account to the token's login, and its old token stays until the new
// one is checked and the list is saved. `check` is the task's cancellation, asked before the
// request and before anything is stored.
pub fn login(
    client: &Client,
    store: &dyn SecretStore,
    base: &Path,
    target: &LoginTarget,
    token: Secret,
    check: &dyn Fn() -> Result<(), HostingError>,
) -> Result<GithubAccount, LoginError> {
    let saving = |error: io::Error| LoginError::Saving(error.to_string());
    let cancelled = |_| LoginError::Cancelled;
    let (server, editing) = match target {
        LoginTarget::Add(server) => (server.clone(), None),
        LoginTarget::Edit(id) => (
            listed(&list(base).map_err(saving)?, id)?.server.clone(),
            Some(id.as_str()),
        ),
    };
    if !server.url_crate_reads_the_same_host() {
        return Err(LoginError::InvalidServer);
    }
    check().map_err(cancelled)?;
    let user = current_user(client, &server, &token).map_err(login_failure)?;
    let login = user
        .get("login")
        .and_then(Value::as_str)
        .ok_or_else(|| login_failure(HostingError::Io(PARSE_FAILED.to_string())))?
        .to_string();
    check().map_err(cancelled)?;

    let _list = LIST.lock().unwrap_or_else(PoisonError::into_inner);
    let mut accounts = list(base).map_err(saving)?;
    let edited = match editing {
        Some(id) => Some(listed(&accounts, id)?.clone()),
        None => None,
    };
    let taken = accounts.iter().any(|account| {
        Some(account) != edited.as_ref()
            && account.login == login
            && account.server.same_ignoring_scheme(&server)
    });
    if taken {
        return Err(LoginError::AlreadyAdded(login));
    }
    let account = GithubAccount {
        server: server.clone(),
        login: login.clone(),
    };
    let store_token = || {
        store
            .set(&service(&server), &login, &token)
            .map_err(|error| LoginError::Saving(error.to_string()))
    };
    if edited.as_ref() == Some(&account) {
        // The same login again: the list does not change, and the new token replaces the old one
        // in the same item only now that GitHub accepted it.
        store_token()?;
        return Ok(account);
    }
    match edited
        .as_ref()
        .and_then(|edited| accounts.iter().position(|account| account == edited))
    {
        Some(position) => accounts[position] = account.clone(),
        None => accounts.push(account.clone()),
    }
    // A new item: the old login's item stays until the saved list no longer names it.
    store_token()?;
    if let Err(error) = save(base, &accounts) {
        // A token whose account is not in the list would stay in the Keychain for nobody.
        let _ = store.delete(&service(&server), &login);
        return Err(saving(error));
    }
    if let Some(old) = edited {
        let _ = store.delete(&service(&old.server), &old.login);
    }
    Ok(account)
}

fn listed<'a>(accounts: &'a [GithubAccount], id: &str) -> Result<&'a GithubAccount, LoginError> {
    accounts
        .iter()
        .find(|account| account.id() == id)
        .ok_or_else(|| LoginError::UnknownAccount(id.to_string()))
}

// GHTokenCredentialsUi.handleError.
fn login_failure(error: HostingError) -> LoginError {
    match error {
        HostingError::Cancelled => LoginError::Cancelled,
        HostingError::Unreachable => LoginError::Unreachable,
        error if error.is_authentication() => LoginError::Credentials(error.to_string()),
        error => LoginError::Invalid(presentable(&error)),
    }
}

// The exception's message: a GithubConfusingException other than a status error (here, an
// answer that is not the JSON expected) carries `CurrentUser`'s operation in front.
pub(crate) fn presentable(error: &HostingError) -> String {
    match error {
        HostingError::Io(message) if message == PARSE_FAILED => {
            format!("{PROFILE_OPERATION}\n\n{message}")
        }
        error => error.to_string(),
    }
}

// `−`: the account leaves the list and then its token the store, so a list that cannot be saved
// keeps both. No confirmation, as in 241.
pub fn remove(store: &dyn SecretStore, base: &Path, id: &str) -> io::Result<()> {
    let _list = LIST.lock().unwrap_or_else(PoisonError::into_inner);
    let mut accounts = list(base)?;
    let Some(position) = accounts.iter().position(|account| account.id() == id) else {
        return Ok(());
    };
    let account = accounts.remove(position);
    save(base, &accounts)?;
    store
        .delete(&service(&account.server), &account.login)
        .map_err(|error| io::Error::other(error.to_string()))
}

// GHAccountsDetailsProvider.loadDetails: `GET <api>/user` for the full name; without a token,
// `Missing access token`; on failure the exception's message, and `Log in` when GitHub refused
// the token.
pub fn details(
    client: &Client,
    store: &dyn SecretStore,
    account: &GithubAccount,
) -> AccountDetails {
    if !account.server.url_crate_reads_the_same_host() {
        return AccountDetails {
            name: None,
            error: Some(INVALID_SERVER.to_string()),
            needs_login: false,
        };
    }
    let Some(token) = token(store, account) else {
        return AccountDetails {
            name: None,
            error: Some(MISSING_TOKEN.to_string()),
            needs_login: true,
        };
    };
    match current_user(client, &account.server, &token) {
        Ok(user) => AccountDetails {
            name: user.get("name").and_then(Value::as_str).map(str::to_string),
            error: None,
            needs_login: false,
        },
        Err(error) => AccountDetails {
            name: None,
            error: Some(presentable(&error)),
            needs_login: error.is_authentication(),
        },
    }
}

// GHSecurityUtil.buildNewTokenUrl: on the web server (`github.com`, not `api.github.com`).
pub fn generate_url(server: &GithubServer) -> String {
    format!("{}/settings/tokens/new?{NEW_TOKEN_QUERY}", server.web_url())
}

// `GET <api>/user`, without an `Accept` (`Get.json` passes none).
pub(crate) fn current_user(
    client: &Client,
    server: &GithubServer,
    token: &Secret,
) -> Result<Value, HostingError> {
    client
        .get(&format!("{}/user", server.api_url()), Some(token), None)?
        .json()
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::time::Duration;

    use tempfile::TempDir;

    use super::*;
    use crate::secrets::{MemoryStore, SecretError};
    use crate::test_support::{FakeGitHub, Reply};

    const TOKEN: &str = "ghp_FIRST0123456789";
    const OTHER_TOKEN: &str = "ghp_SECOND0123456789";

    fn client() -> Client {
        Client::new(Duration::from_secs(5))
    }

    fn no_check() -> Result<(), HostingError> {
        Ok(())
    }

    fn server_of(fake: &FakeGitHub) -> GithubServer {
        GithubServer::parse(&fake.url()).unwrap()
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

    fn stored(store: &MemoryStore, server: &GithubServer, login: &str) -> Option<String> {
        store
            .get(&service(server), login)
            .unwrap()
            .map(|secret| secret.expose().to_string())
    }

    // `editing` names the account of a row's `Log in`, whose own server is used; else `server`.
    fn log_in(
        client: &Client,
        store: &dyn SecretStore,
        base: &Path,
        server: &GithubServer,
        token: &str,
        editing: Option<&str>,
    ) -> Result<GithubAccount, LoginError> {
        let target = match editing {
            Some(id) => LoginTarget::Edit(id.to_string()),
            None => LoginTarget::Add(server.clone()),
        };
        login(
            client,
            store,
            base,
            &target,
            Secret::new(token.to_string()),
            &no_check,
        )
    }

    #[test]
    fn login_checks_user_and_stores_the_token_only_on_success() {
        let fake = FakeGitHub::new();
        let server = server_of(&fake);
        let store = MemoryStore::new();
        let base = TempDir::new().unwrap();

        // A 401: GithubAuthenticationException, `Incorrect credentials.` and its message.
        fake.reply(Reply::json(
            "GET",
            "/api/v3/user",
            401,
            "Unauthorized",
            r#"{"message":"Bad credentials"}"#,
        ));
        let refused = log_in(&client(), &store, base.path(), &server, TOKEN, None).unwrap_err();
        assert_eq!(
            refused.to_string(),
            "Incorrect credentials.\nRequest response: Bad credentials."
        );
        assert_eq!(stored(&store, &server, "octocat"), None);
        assert!(list(base.path()).unwrap().is_empty());
        assert!(!base.path().join(FILE_NAME).exists());

        // A name that leads nowhere: UnknownHostException, `Server is unreachable`.
        let nowhere = Client::resolving(Duration::from_secs(5), |_: &str| {
            Err::<Vec<SocketAddr>, _>(io::Error::other("no such host"))
        });
        let ghe = GithubServer::parse("ghe.example.com").unwrap();
        let unreachable = log_in(&nowhere, &store, base.path(), &ghe, TOKEN, None).unwrap_err();
        assert_eq!(unreachable, LoginError::Unreachable);
        assert_eq!(unreachable.to_string(), "Server is unreachable");

        // Anything else: `Invalid authentication data.` and its message, a parse failure with
        // `CurrentUser`'s operation in front.
        fake.reply(Reply::json(
            "GET",
            "/api/v3/user",
            404,
            "Not Found",
            r#"{"message":"Not Found"}"#,
        ));
        let missing = log_in(&client(), &store, base.path(), &server, TOKEN, None).unwrap_err();
        assert_eq!(
            missing.to_string(),
            "Invalid authentication data.\n404 Not Found - Not Found"
        );
        fake.reply(Reply::json("GET", "/api/v3/user", 200, "OK", "<html>"));
        let garbled = log_in(&client(), &store, base.path(), &server, TOKEN, None).unwrap_err();
        assert_eq!(
            garbled.to_string(),
            "Invalid authentication data.\nCan't get profile information\n\nCan't parse GitHub response"
        );
        assert!(store.get(&service(&ghe), "octocat").unwrap().is_none());
        assert!(list(base.path()).unwrap().is_empty());
        assert!(!base.path().join(FILE_NAME).exists());

        // A task cancelled while GitHub answered stores nothing, even when GitHub said yes.
        fake.reply(user("octocat"));
        let checks = std::cell::Cell::new(0);
        let cancelled_after_the_request = || {
            checks.set(checks.get() + 1);
            match checks.get() {
                1 => Ok(()),
                _ => Err(HostingError::Cancelled),
            }
        };
        let cancelled = login(
            &client(),
            &store,
            base.path(),
            &LoginTarget::Add(server.clone()),
            Secret::new(TOKEN.to_string()),
            &cancelled_after_the_request,
        );
        assert!(matches!(cancelled, Err(LoginError::Cancelled)));
        assert_eq!(stored(&store, &server, "octocat"), None);
        assert!(list(base.path()).unwrap().is_empty());

        // Success: the account is listed and its token stored, nowhere else.
        fake.reply(user("octocat"));
        let account = log_in(&client(), &store, base.path(), &server, TOKEN, None).unwrap();
        assert_eq!(account.login, "octocat");
        assert_eq!(account.server, server);
        assert_eq!(account.id(), format!("{}/octocat", server.address()));
        assert_eq!(list(base.path()).unwrap(), std::slice::from_ref(&account));
        assert_eq!(stored(&store, &server, "octocat").as_deref(), Some(TOKEN));
        assert_eq!(
            token(&store, &account)
                .map(|secret| secret.expose().to_string())
                .as_deref(),
            Some(TOKEN)
        );
        let file = fs::read_to_string(base.path().join(FILE_NAME)).unwrap();
        assert!(!file.contains(TOKEN));

        let requests = fake.requests();
        assert!(requests.iter().all(|request| request.method == "GET"
            && request.target == "/api/v3/user"
            && request
                .header("accept")
                .is_none_or(|accept| accept != "application/vnd.github.v3+json")
            && request.header("authorization") == Some(format!("Bearer {TOKEN}").as_str())));
        fake.finish();
    }

    #[test]
    fn a_second_account_with_the_same_server_and_login_is_refused() {
        let fake = FakeGitHub::new();
        let store = MemoryStore::new();
        let base = TempDir::new().unwrap();
        // The fake speaks only `http://`; the account already there was typed with `https://`.
        let http = server_of(&fake);
        let secure = GithubServer {
            use_http: Some(false),
            ..http.clone()
        };
        save(
            base.path(),
            &[GithubAccount {
                server: secure.clone(),
                login: "octocat".to_string(),
            }],
        )
        .unwrap();
        store
            .set(
                &service(&secure),
                "octocat",
                &Secret::new(TOKEN.to_string()),
            )
            .unwrap();

        fake.reply(user("octocat"));
        let refused = log_in(&client(), &store, base.path(), &http, OTHER_TOKEN, None).unwrap_err();
        assert_eq!(refused, LoginError::AlreadyAdded("octocat".to_string()));
        assert_eq!(refused.to_string(), "Account 'octocat' already added");
        assert_eq!(list(base.path()).unwrap().len(), 1);
        assert_eq!(stored(&store, &secure, "octocat").as_deref(), Some(TOKEN));

        // Another login on that server is another account, and so is the login in another case.
        fake.reply(user("Octocat"));
        log_in(&client(), &store, base.path(), &http, OTHER_TOKEN, None).unwrap();
        assert_eq!(
            list(base.path())
                .unwrap()
                .iter()
                .map(GithubAccount::id)
                .collect::<Vec<_>>(),
            [
                format!("{}/octocat", http.address()),
                format!("{}/Octocat", http.address())
            ]
        );
        fake.finish();
    }

    #[test]
    fn relogin_renames_the_edited_account_to_the_tokens_login() {
        let fake = FakeGitHub::new();
        let server = server_of(&fake);
        let store = MemoryStore::new();
        let base = TempDir::new().unwrap();
        fake.script(vec![
            user("octocat"),
            user("hubot"),
            user("monalisa"),
            user("hubot"),
        ]);
        let octocat = log_in(&client(), &store, base.path(), &server, TOKEN, None).unwrap();
        let hubot = log_in(&client(), &store, base.path(), &server, OTHER_TOKEN, None).unwrap();

        // The token of `monalisa` turns `octocat` into `monalisa`, in its place.
        let renamed = log_in(
            &client(),
            &store,
            base.path(),
            &server,
            "ghp_THIRD",
            Some(&octocat.id()),
        )
        .unwrap();
        assert_eq!(renamed.login, "monalisa");
        assert_eq!(renamed.server, server);
        assert_eq!(list(base.path()).unwrap(), [renamed.clone(), hubot.clone()]);
        assert_eq!(
            stored(&store, &server, "monalisa").as_deref(),
            Some("ghp_THIRD")
        );
        assert_eq!(stored(&store, &server, "octocat"), None);
        assert_eq!(
            stored(&store, &server, "hubot").as_deref(),
            Some(OTHER_TOKEN)
        );

        // The edited account does not count against itself, the others do.
        let taken = log_in(
            &client(),
            &store,
            base.path(),
            &server,
            "ghp_FOURTH",
            Some(&renamed.id()),
        )
        .unwrap_err();
        assert_eq!(taken.to_string(), "Account 'hubot' already added");
        assert_eq!(list(base.path()).unwrap(), [renamed, hubot]);

        // An account that is gone is not created again.
        let gone = log_in(
            &client(),
            &store,
            base.path(),
            &server,
            TOKEN,
            Some("ghe.example.com/nobody"),
        )
        .unwrap_err();
        assert_eq!(
            gone,
            LoginError::UnknownAccount("ghe.example.com/nobody".to_string())
        );
        fake.finish();
    }

    #[test]
    fn remove_deletes_the_account_and_its_token_only() {
        let store = MemoryStore::new();
        let base = TempDir::new().unwrap();
        let server = GithubServer::github_com();
        let accounts = ["octocat", "hubot"].map(|login| GithubAccount {
            server: server.clone(),
            login: login.to_string(),
        });
        save(base.path(), &accounts).unwrap();
        for account in &accounts {
            store
                .set(
                    &service(&server),
                    &account.login,
                    &Secret::new(TOKEN.to_string()),
                )
                .unwrap();
        }

        remove(&store, base.path(), "github.com/octocat").unwrap();
        assert_eq!(list(base.path()).unwrap(), &accounts[1..]);
        assert_eq!(stored(&store, &server, "octocat"), None);
        assert_eq!(stored(&store, &server, "hubot").as_deref(), Some(TOKEN));
        // Removing what is not there changes nothing.
        remove(&store, base.path(), "github.com/octocat").unwrap();
        assert_eq!(list(base.path()).unwrap(), &accounts[1..]);
    }

    // A store whose writes and deletes fail on demand, as a Keychain that refuses can.
    #[derive(Default)]
    struct RefusingStore {
        inner: MemoryStore,
        refuse: std::sync::atomic::AtomicBool,
    }

    impl RefusingStore {
        fn refuse(&self, refuse: bool) {
            self.refuse
                .store(refuse, std::sync::atomic::Ordering::SeqCst);
        }

        fn refused(&self) -> Result<(), SecretError> {
            match self.refuse.load(std::sync::atomic::Ordering::SeqCst) {
                true => Err(SecretError("refused".to_string())),
                false => Ok(()),
            }
        }
    }

    impl SecretStore for RefusingStore {
        fn get(&self, service: &str, account: &str) -> Result<Option<Secret>, SecretError> {
            self.inner.get(service, account)
        }

        fn set(&self, service: &str, account: &str, secret: &Secret) -> Result<(), SecretError> {
            self.refused()?;
            self.inner.set(service, account, secret)
        }

        fn delete(&self, service: &str, account: &str) -> Result<(), SecretError> {
            self.refused()?;
            self.inner.delete(service, account)
        }
    }

    // The folder of the list cannot take a new file, so `save` fails and `list` still reads.
    fn read_only(dir: &Path, on: bool) {
        use std::os::unix::fs::PermissionsExt;
        let mode = if on { 0o555 } else { 0o755 };
        fs::set_permissions(dir, fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn a_failed_relogin_or_removal_keeps_the_old_token() {
        let fake = FakeGitHub::new();
        let server = server_of(&fake);
        let store = RefusingStore::default();
        let base = TempDir::new().unwrap();
        let read = |login: &str| {
            store
                .get(&service(&server), login)
                .unwrap()
                .map(|secret| secret.expose().to_string())
        };
        fake.reply(user("octocat"));
        let octocat = log_in(&client(), &store, base.path(), &server, TOKEN, None).unwrap();

        // A store that refuses keeps the old token, and the list as it was.
        store.refuse(true);
        fake.reply(user("octocat"));
        let refused = log_in(
            &client(),
            &store,
            base.path(),
            &server,
            OTHER_TOKEN,
            Some(&octocat.id()),
        );
        assert!(matches!(refused, Err(LoginError::Saving(_))), "{refused:?}");
        assert_eq!(read("octocat").as_deref(), Some(TOKEN));
        fake.reply(user("hubot"));
        let refused = log_in(
            &client(),
            &store,
            base.path(),
            &server,
            OTHER_TOKEN,
            Some(&octocat.id()),
        );
        assert!(matches!(refused, Err(LoginError::Saving(_))), "{refused:?}");
        assert_eq!(read("octocat").as_deref(), Some(TOKEN));
        assert_eq!(read("hubot"), None);
        assert_eq!(list(base.path()).unwrap(), std::slice::from_ref(&octocat));
        store.refuse(false);

        // A list that cannot be saved: a rename keeps the old login and its token, and the same
        // login again does not need the list.
        read_only(base.path(), true);
        fake.reply(user("hubot"));
        let unsaved = log_in(
            &client(),
            &store,
            base.path(),
            &server,
            OTHER_TOKEN,
            Some(&octocat.id()),
        );
        assert!(matches!(unsaved, Err(LoginError::Saving(_))), "{unsaved:?}");
        assert_eq!(read("octocat").as_deref(), Some(TOKEN));
        assert_eq!(read("hubot"), None);
        fake.reply(user("octocat"));
        let again = log_in(
            &client(),
            &store,
            base.path(),
            &server,
            OTHER_TOKEN,
            Some(&octocat.id()),
        );
        assert_eq!(again.unwrap(), octocat);
        assert_eq!(read("octocat").as_deref(), Some(OTHER_TOKEN));

        // `−` with a list that cannot be saved keeps the account and its token.
        assert!(remove(&store, base.path(), &octocat.id()).is_err());
        read_only(base.path(), false);
        assert_eq!(list(base.path()).unwrap(), std::slice::from_ref(&octocat));
        assert_eq!(read("octocat").as_deref(), Some(OTHER_TOKEN));

        // A Keychain that refuses the delete leaves the account out of the list all the same.
        store.refuse(true);
        assert!(remove(&store, base.path(), &octocat.id()).is_err());
        store.refuse(false);
        assert!(list(base.path()).unwrap().is_empty());
        fake.finish();
    }

    #[test]
    fn a_missing_token_gives_missing_access_token_and_needs_login() {
        let fake = FakeGitHub::new();
        let server = server_of(&fake);
        let store = MemoryStore::new();
        let account = GithubAccount {
            server: server.clone(),
            login: "octocat".to_string(),
        };

        // No request at all: the fake fails the test if it gets one.
        assert_eq!(
            details(&client(), &store, &account),
            AccountDetails {
                name: None,
                error: Some("Missing access token".to_string()),
                needs_login: true,
            }
        );

        store
            .set(
                &service(&server),
                "octocat",
                &Secret::new(TOKEN.to_string()),
            )
            .unwrap();
        fake.script(vec![
            user("octocat"),
            Reply::json(
                "GET",
                "/api/v3/user",
                200,
                "OK",
                r#"{"login":"octocat","name":null}"#,
            ),
            Reply::json(
                "GET",
                "/api/v3/user",
                401,
                "Unauthorized",
                r#"{"message":"Bad credentials"}"#,
            ),
            Reply::json("GET", "/api/v3/user", 500, "Internal Server Error", ""),
        ]);
        assert_eq!(
            details(&client(), &store, &account),
            AccountDetails {
                name: Some("The octocat".to_string()),
                error: None,
                needs_login: false,
            }
        );
        assert_eq!(
            details(&client(), &store, &account),
            AccountDetails {
                name: None,
                error: None,
                needs_login: false,
            }
        );
        assert_eq!(
            details(&client(), &store, &account),
            AccountDetails {
                name: None,
                error: Some("Request response: Bad credentials.".to_string()),
                needs_login: true,
            }
        );
        assert_eq!(
            details(&client(), &store, &account),
            AccountDetails {
                name: None,
                error: Some("500 Internal Server Error - null".to_string()),
                needs_login: false,
            }
        );
        fake.finish();
    }

    #[test]
    fn the_generate_url_is_241s_with_rama_s_name() {
        assert_eq!(
            generate_url(&GithubServer::github_com()),
            "https://github.com/settings/tokens/new?description=rama%20GitHub%20integration%20plugin&scopes=repo%2Cgist%2Cread%3Aorg%2Cworkflow%2Cread%3Auser%2Cuser%3Aemail"
        );
        assert_eq!(
            generate_url(&GithubServer::parse("http://ghe.example.com:8080/prefix/").unwrap()),
            "http://ghe.example.com:8080/prefix/settings/tokens/new?description=rama%20GitHub%20integration%20plugin&scopes=repo%2Cgist%2Cread%3Aorg%2Cworkflow%2Cread%3Auser%2Cuser%3Aemail"
        );
    }

    // `\` ends the authority for the URL crate, which ureq connects with, and `@` starts it again
    // after the user info: the server field would show one host while the token goes to another.
    #[test]
    fn a_server_the_url_crate_reads_as_another_host_is_refused_before_any_request() {
        let fake = FakeGitHub::new();
        let address = fake.url().trim_start_matches("http://").to_string();
        let port: u16 = address.rsplit(':').next().unwrap().parse().unwrap();
        // Every name leads to the fake, so a request that should not happen is seen there.
        let everywhere = Client::resolving(Duration::from_secs(5), move |_: &str| {
            Ok(vec![SocketAddr::from(([127, 0, 0, 1], port))])
        });
        let store = MemoryStore::new();
        let base = TempDir::new().unwrap();

        // `http://` first: before the check, the token reached the fake in the clear.
        for text in [
            r"http://ghe.example\@evil.test".to_string(),
            format!("http://ghe.example.com@{address}"),
            r"ghe.example\@evil.test".to_string(),
        ] {
            let server = GithubServer::parse(&text).unwrap();
            let refused =
                log_in(&everywhere, &store, base.path(), &server, TOKEN, None).unwrap_err();
            assert_eq!(refused, LoginError::InvalidServer, "{text}");
            assert_eq!(refused.to_string(), "Server URL is not valid");

            // An account with such a server, from a file edited by hand, sends nothing either.
            store
                .set(
                    &service(&server),
                    "octocat",
                    &Secret::new(TOKEN.to_string()),
                )
                .unwrap();
            let account = GithubAccount {
                server,
                login: "octocat".to_string(),
            };
            assert_eq!(
                details(&everywhere, &store, &account),
                AccountDetails {
                    name: None,
                    error: Some("Server URL is not valid".to_string()),
                    needs_login: false,
                }
            );
        }
        assert!(fake.requests().is_empty(), "{:?}", fake.requests());
        assert!(list(base.path()).unwrap().is_empty());
    }
}
