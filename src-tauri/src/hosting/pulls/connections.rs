// The Pull Requests window's open connections, one per repository, with the token of each one's
// account kept in memory. A token is read from the Keychain once, by the connect, and again only
// after it was dropped: by a change to the accounts (a login, a re-login or a removal) or by an
// authentication error. It never goes to disk and is never serialized.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::secrets::Secret;

use super::Connection;

// What a connect is given when it starts: its place in the order of connects, and which tokens
// were valid then.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ticket {
    seq: u64,
    tokens: u64,
}

// What a request of the list starts from. `tokens` says which tokens were valid when it was read,
// for `keep_token`.
pub struct Current {
    pub connection: Connection,
    pub token: Option<Arc<Secret>>,
    pub tokens: u64,
}

struct Open {
    seq: u64,
    connection: Connection,
    token: Option<Arc<Secret>>,
}

// `floor`: a connect that started before the last `forget` of the repository opens nothing.
#[derive(Default)]
struct Slot {
    floor: u64,
    open: Option<Open>,
}

#[derive(Default)]
struct Inner {
    started: u64,
    // Bumped whenever tokens are dropped: a token read before that is not kept.
    tokens: u64,
    repos: HashMap<PathBuf, Slot>,
}

#[derive(Default)]
pub struct Connections {
    inner: Mutex<Inner>,
}

impl Connections {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    // A connect begins.
    pub fn start(&self) -> Ticket {
        let mut inner = self.lock();
        inner.started += 1;
        Ticket {
            seq: inner.started,
            tokens: inner.tokens,
        }
    }

    // A connect for `repo` succeeded. It opens only if no later connect of that repository opened
    // first and the repository was not forgotten since it started; its token is kept only if no
    // token was dropped since. Whether it opened.
    pub fn opened(
        &self,
        repo: &Path,
        ticket: Ticket,
        connection: Connection,
        token: Secret,
    ) -> bool {
        let mut inner = self.lock();
        let tokens = inner.tokens;
        let slot = inner.repos.entry(repo.to_path_buf()).or_default();
        let later = slot.open.as_ref().is_some_and(|open| open.seq > ticket.seq);
        if ticket.seq <= slot.floor || later {
            return false;
        }
        slot.open = Some(Open {
            seq: ticket.seq,
            connection,
            token: (ticket.tokens == tokens).then(|| Arc::new(token)),
        });
        true
    }

    // `Change Repository or Account…`: the repository's connection closes, and a connect still
    // running for it will not open.
    pub fn forget(&self, repo: &Path) {
        let mut inner = self.lock();
        let started = inner.started;
        let slot = inner.repos.entry(repo.to_path_buf()).or_default();
        slot.floor = started;
        slot.open = None;
    }

    pub fn current(&self, repo: &Path) -> Option<Current> {
        let inner = self.lock();
        let open = inner.repos.get(repo)?.open.as_ref()?;
        Some(Current {
            connection: open.connection.clone(),
            token: open.token.clone(),
            tokens: inner.tokens,
        })
    }

    pub fn connection_of(&self, repo: &Path) -> Option<Connection> {
        self.current(repo).map(|current| current.connection)
    }

    // A token read from the store for `repo`'s connection is kept, unless tokens were dropped
    // since `current` gave `tokens`, or the connection changed.
    pub fn keep_token(&self, repo: &Path, current: &Current, token: Arc<Secret>) {
        let mut inner = self.lock();
        if inner.tokens != current.tokens {
            return;
        }
        if let Some(open) = inner
            .repos
            .get_mut(repo)
            .and_then(|slot| slot.open.as_mut())
        {
            if open.token.is_none() && open.connection == current.connection {
                open.token = Some(token);
            }
        }
    }

    // The accounts changed: every kept token goes.
    pub fn drop_tokens(&self) {
        let mut inner = self.lock();
        inner.tokens += 1;
        for open in inner
            .repos
            .values_mut()
            .filter_map(|slot| slot.open.as_mut())
        {
            open.token = None;
        }
    }

    // GitHub refused `repo`'s token: the next request reads it again.
    pub fn drop_token(&self, repo: &Path) {
        let mut inner = self.lock();
        inner.tokens += 1;
        if let Some(open) = inner
            .repos
            .get_mut(repo)
            .and_then(|slot| slot.open.as_mut())
        {
            open.token = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hosting::mapping::RepoCoordinates;
    use crate::hosting::pulls::RepoInfo;
    use crate::hosting::server::GithubServer;

    fn connection(account: &str) -> Connection {
        Connection {
            remote: "origin".to_string(),
            url: "https://github.com/o/r.git".to_string(),
            account: account.to_string(),
            coordinates: RepoCoordinates {
                server: GithubServer::github_com(),
                owner: "o".to_string(),
                name: "r".to_string(),
            },
            viewer_login: "octocat".to_string(),
            repository: RepoInfo {
                id: "R_1".to_string(),
                owner: "o".to_string(),
                name_with_owner: "o/r".to_string(),
                viewer_permission: None,
                merge_commit_allowed: true,
                squash_merge_allowed: true,
                rebase_merge_allowed: true,
                default_branch: None,
                is_fork: false,
                url: "https://github.com/o/r".to_string(),
                ssh_url: "git@github.com:o/r.git".to_string(),
            },
            title: "r".to_string(),
        }
    }

    fn token(text: &str) -> Secret {
        Secret::new(text.to_string())
    }

    fn account_of(connections: &Connections, repo: &Path) -> Option<String> {
        connections.connection_of(repo).map(|open| open.account)
    }

    #[test]
    fn a_late_connect_never_replaces_a_later_one() {
        let connections = Connections::default();
        let x = PathBuf::from("/repos/x");
        let y = PathBuf::from("/repos/y");

        // A connect starts for X, Y opens and connects, and X's finishes last.
        let for_x = connections.start();
        let for_y = connections.start();
        assert!(connections.opened(&y, for_y, connection("github.com/y"), token("t")));
        assert!(connections.opened(&x, for_x, connection("github.com/x"), token("t")));
        assert_eq!(
            account_of(&connections, &y).as_deref(),
            Some("github.com/y")
        );
        assert_eq!(
            account_of(&connections, &x).as_deref(),
            Some("github.com/x")
        );

        // Two connects of one repository that finish out of order: the later one stays.
        let older = connections.start();
        let newer = connections.start();
        assert!(connections.opened(&y, newer, connection("github.com/new"), token("t")));
        assert!(!connections.opened(&y, older, connection("github.com/old"), token("t")));
        assert_eq!(
            account_of(&connections, &y).as_deref(),
            Some("github.com/new")
        );

        // A connect started before a forget opens nothing; one started after does.
        let before = connections.start();
        connections.forget(&y);
        assert_eq!(account_of(&connections, &y), None);
        assert!(!connections.opened(&y, before, connection("github.com/late"), token("t")));
        assert_eq!(account_of(&connections, &y), None);
        // Forgetting Y leaves X's.
        assert!(account_of(&connections, &x).is_some());
        let after = connections.start();
        assert!(connections.opened(&y, after, connection("github.com/again"), token("t")));
        assert_eq!(
            account_of(&connections, &y).as_deref(),
            Some("github.com/again")
        );
    }

    #[test]
    fn the_token_is_kept_until_the_accounts_change_or_github_refuses_it() {
        let connections = Connections::default();
        let repo = PathBuf::from("/repos/r");
        let ticket = connections.start();
        connections.opened(&repo, ticket, connection("github.com/a"), token("first"));
        let kept = connections.current(&repo).unwrap().token.unwrap();
        assert_eq!(kept.expose(), "first");

        // A login, a re-login or a removal.
        connections.drop_tokens();
        let current = connections.current(&repo).unwrap();
        assert!(current.token.is_none());
        connections.keep_token(&repo, &current, Arc::new(token("second")));
        assert_eq!(
            connections.current(&repo).unwrap().token.unwrap().expose(),
            "second"
        );

        // GitHub refused it.
        connections.drop_token(&repo);
        let current = connections.current(&repo).unwrap();
        assert!(current.token.is_none());
        // A token read before a drop is not kept.
        connections.drop_tokens();
        connections.keep_token(&repo, &current, Arc::new(token("stale")));
        assert!(connections.current(&repo).unwrap().token.is_none());

        // A connect that read its token before the accounts changed keeps the connection only.
        let ticket = connections.start();
        connections.drop_tokens();
        assert!(connections.opened(&repo, ticket, connection("github.com/b"), token("old")));
        assert!(connections.current(&repo).unwrap().token.is_none());
    }
}
