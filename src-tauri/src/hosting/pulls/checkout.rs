// `Checkout '…'…` and `Show '…' in Git Log` of a pull request's branch, and the checked-out icon.
// Ports GitRemoteBranchesUtil (fetchAndCheckoutRemoteBranch, fetchAndShowRemoteBranchInLog,
// findRemote, findOrCreateRemote, shouldAddHttpRemote, findNameForRemote, checkoutRemoteBranch,
// isRemoteBranchCheckedOut) and GHPRBranchesViewModel (git4idea and plugins/github at
// idea/241.14494.240, P§7). Overview decision 17: a `<login>` taken by another remote becomes the
// first free `<login>_1`, `<login>_2`…, where 241 fails. The checkout itself is
// `branches::checkout_remote`, with its recoverable copy and its questions.

use std::path::Path;

use serde::Serialize;

use crate::branch_info::BranchSync;
use crate::branches::{self, RemoteCheckout, RemoteOptions, Repo};
use crate::fetch;
use crate::git::{self, GitError};
use crate::remotes::{self, Remote};
use crate::task::TaskContext;

use super::super::mapping::matches_server;
use super::super::server::GithubServer;
use super::changes;
use super::details::{HeadRepository, PullRequestDetails};

const ORIGIN: &str = "origin";
// GHPRBranchesViewModel: `localPrefix` of a fork's branch.
const FORK_PREFIX: &str = "fork";

// What a pull request's branch action ends in. `Gone`: the head repository was deleted, and 241
// does nothing. `NoRemote`: no remote points to it and none could be added, with the URLs GitHub
// gave. `FetchFailed`: git's output, already without credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Headed<T> {
    Gone,
    NoRemote { urls: Vec<String> },
    FetchFailed { output: String },
    Ready(T),
}

impl<T> Headed<T> {
    fn map<U>(self, change: impl FnOnce(T) -> U) -> Headed<U> {
        match self {
            Headed::Gone => Headed::Gone,
            Headed::NoRemote { urls } => Headed::NoRemote { urls },
            Headed::FetchFailed { output } => Headed::FetchFailed { output },
            Headed::Ready(value) => Headed::Ready(change(value)),
        }
    }
}

// The head's remote, its fetched branch `<remote>/<head>` and the local name to offer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Prepared {
    pub remote: String,
    pub branch: String,
    pub suggested: String,
}

// The checkout's answer, with what a retry of it needs: the remote branch and the name offered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckedOut {
    pub branch: String,
    pub suggested: String,
    pub outcome: RemoteCheckout,
}

// The range of the log tab: `from..to`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Range {
    pub from: String,
    pub to: String,
}

// `findRemote`: the first remote whose first fetch URL is on the server and, without one `/` and
// one `.git` at the end, ends with the repository's `owner/name`. `endsWith` is case-sensitive and
// has no `/` boundary, as in 241.
pub fn find_remote<'a>(
    remotes: &'a [Remote],
    server: &GithubServer,
    path: &str,
) -> Option<&'a Remote> {
    remotes.iter().find(|remote| {
        let url = remote.url.strip_suffix('/').unwrap_or(&remote.url);
        let url = url.strip_suffix(".git").unwrap_or(url);
        matches_server(server, &remote.url) && url.ends_with(path)
    })
}

// `findNameForRemote` as it was meant (decision 17): `login`, else the first free `login_1`,
// `login_2`…
pub fn remote_name(existing: &[String], login: &str) -> String {
    let taken = |name: &str| existing.iter().any(|other| other == name);
    if !taken(login) {
        return login.to_string();
    }
    (1u64..)
        .map(|index| format!("{login}_{index}"))
        .find(|name| !taken(name))
        .expect("a finite list leaves a name free")
}

// `shouldAddHttpRemote` and the URL `findOrCreateRemote` takes: HTTP when `origin`'s URL, else
// the first remote's, starts with `http`, or when there are no remotes; else SSH. With HTTP
// preferred and no web URL it is SSH; with SSH preferred and no SSH URL there is none.
pub fn remote_url<'a>(remotes: &[Remote], head: &'a HeadRepository) -> Option<&'a str> {
    let preferred = remotes
        .iter()
        .find(|remote| remote.name == ORIGIN)
        .or_else(|| remotes.first());
    let http = preferred.is_none_or(|remote| remote.url.starts_with("http"));
    let present = |url: &'a str| Some(url).filter(|url| !url.is_empty());
    match present(&head.url) {
        Some(url) if http => Some(url),
        _ => present(&head.ssh_url),
    }
}

// What `git remote add` may take from GitHub's answer: `https://` (or `http://` on an `http://`
// server) with the server's host and port and no credentials, `ssh://` or `user@host:path` with
// the server's host. Nothing else, so no other transport and no other host.
fn addable_url(server: &GithubServer, url: &str) -> bool {
    if url.starts_with('-') || url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let same_host = |host: &str| host.eq_ignore_ascii_case(&server.host);
    if url.contains("://") {
        let Ok(parsed) = url::Url::parse(url) else {
            return false;
        };
        let host = parsed.host_str().is_some_and(same_host);
        return match parsed.scheme() {
            "https" | "http" => {
                let scheme = parsed.scheme() == "https" || server.scheme() == "http";
                let default = if parsed.scheme() == "http" { 80 } else { 443 };
                scheme
                    && host
                    && parsed.port_or_known_default() == Some(server.port.unwrap_or(default))
                    && parsed.username().is_empty()
                    && parsed.password().is_none()
            }
            "ssh" => host,
            _ => false,
        };
    }
    let Some((address, path)) = url.split_once(':') else {
        return false;
    };
    let Some((user, host)) = address.split_once('@') else {
        return false;
    };
    !user.is_empty() && !host.contains('/') && same_host(host) && !path.is_empty()
}

// Every configured remote's name, with a URL or not: `git remote add` refuses all of them.
fn remote_names(ctx: &TaskContext) -> Result<Vec<String>, GitError> {
    let output = ctx.git(&["remote"])?;
    Ok(output.stdout.lines().map(str::to_string).collect())
}

// `findOrCreateRemote`: the head's remote, else one added under the owner's login. `None` when
// GitHub gave no URL that can be added.
fn find_or_add_remote(
    ctx: &TaskContext,
    server: &GithubServer,
    head: &HeadRepository,
) -> Result<Option<String>, GitError> {
    let listed = remotes::list(ctx.repo())?;
    if let Some(found) = find_remote(&listed, server, &head.name_with_owner) {
        remotes::check_name(ctx, &found.name)?;
        return Ok(Some(found.name.clone()));
    }
    let Some(url) = remote_url(&listed, head).filter(|url| addable_url(server, url)) else {
        return Ok(None);
    };
    let name = remote_name(&remote_names(ctx)?, &head.owner);
    remotes::add(ctx, &name, url)?;
    Ok(Some(name))
}

// `findLocalBranchTrackingRemote`: the first local branch whose upstream is `remote_branch`, and
// whether it is the current one.
fn tracking(path: &Path, remote_branch: &str) -> Result<Option<(String, bool)>, GitError> {
    let format = "--format=%(HEAD)%00%(refname:strip=2)%00%(upstream)";
    let output = git::run(path, &["for-each-ref", format, "refs/heads"])?;
    let upstream = format!("refs/remotes/{remote_branch}");
    Ok(String::from_utf8_lossy(&output).lines().find_map(|line| {
        let mut fields = line.split('\0');
        let (head, name, tracked) = (fields.next()?, fields.next()?, fields.next()?);
        (tracked == upstream).then(|| (name.to_string(), head == "*"))
    }))
}

// P§7.2 steps 1 to 4 and the name of step 5: the head's remote (found or added), `git fetch
// <remote> <head>`, and the local name to offer: the branch that already tracks it, else
// `fork/<head>` for a fork, else `<head>`. The head branch is checked before any git sees it.
pub fn prepare(
    ctx: &TaskContext,
    sync: &BranchSync,
    server: &GithubServer,
    details: &PullRequestDetails,
) -> Result<Headed<Prepared>, GitError> {
    let Some(head) = &details.head_repository else {
        return Ok(Headed::Gone);
    };
    let head_ref = &details.head_ref_name;
    changes::check_branch_name(ctx, head_ref, "The head branch")?;
    // `git remote add` writes the configuration: under the write lock, which is let go before the
    // fetch waits for the remote's lock (the order of Update Project and of deleting a remote
    // branch: never the write lock while waiting for a remote).
    let found = ctx
        .exclusive(|| find_or_add_remote(ctx, server, head))
        .unwrap_or(Err(GitError::Cancelled))?;
    let Some(remote) = found else {
        let urls = [&head.url, &head.ssh_url]
            .into_iter()
            .filter(|url| !url.is_empty())
            .cloned()
            .collect();
        return Ok(Headed::NoRemote { urls });
    };
    match changes::fetch_one(ctx, sync, &remote, head_ref) {
        Ok(()) => {}
        Err(GitError::Cancelled) => return Err(GitError::Cancelled),
        Err(error) => {
            return Ok(Headed::FetchFailed {
                output: fetch::error_text(error),
            })
        }
    }
    let branch = format!("{remote}/{head_ref}");
    let suggested = match tracking(ctx.repo(), &branch)? {
        Some((local, _)) => local,
        None if head.is_fork => format!("{FORK_PREFIX}/{head_ref}"),
        None => head_ref.clone(),
    };
    Ok(Headed::Ready(Prepared {
        remote,
        branch,
        suggested,
    }))
}

// `Checkout '…'…`: `prepare`, then the checkout of a remote branch with the name offered, under the
// write lock. Its questions (`NeedsName`, `LocalHasCommits`) and the overwritten files go back to
// the interface. `before_changes` runs after the fetch and without the lock, as Update Project's
// does: what was written in the diff window meanwhile reaches the disk first.
pub fn checkout(
    repo: &Repo,
    sync: &BranchSync,
    server: &GithubServer,
    details: &PullRequestDetails,
    before_changes: impl FnOnce() -> Result<(), GitError>,
) -> Result<Headed<CheckedOut>, GitError> {
    let prepared = match prepare(repo.ctx, sync, server, details)? {
        Headed::Ready(prepared) => prepared,
        Headed::Gone => return Ok(Headed::Gone),
        Headed::NoRemote { urls } => return Ok(Headed::NoRemote { urls }),
        Headed::FetchFailed { output } => return Ok(Headed::FetchFailed { output }),
    };
    before_changes()?;
    let options = RemoteOptions {
        suggested: Some(&prepared.suggested),
        ..RemoteOptions::default()
    };
    let outcome = repo
        .ctx
        .exclusive(|| branches::checkout_remote(repo, &prepared.branch, options))
        .unwrap_or(Err(GitError::Cancelled))?;
    Ok(Headed::Ready(CheckedOut {
        branch: prepared.branch,
        suggested: prepared.suggested,
        outcome,
    }))
}

// `Show '…' in Git Log` (P§7.5): the head's remote is found or added and its branch fetched, and
// the range is `<remote>/<base>..<remote>/<head>`, with the base branch of the same remote and not
// fetched, as in 241.
pub fn show_in_log(
    ctx: &TaskContext,
    sync: &BranchSync,
    server: &GithubServer,
    details: &PullRequestDetails,
) -> Result<Headed<Range>, GitError> {
    let base_ref = &details.base_ref_name;
    changes::check_branch_name(ctx, base_ref, "The base branch")?;
    Ok(prepare(ctx, sync, server, details)?.map(|prepared| Range {
        from: format!("{}/{base_ref}", prepared.remote),
        to: prepared.branch,
    }))
}

// P§7.6: the head's remote exists and the current branch is the first one that tracks
// `<remote>/<head>`.
pub fn is_checked_out(
    path: &Path,
    server: &GithubServer,
    details: &PullRequestDetails,
) -> Result<bool, GitError> {
    let Some(head) = &details.head_repository else {
        return Ok(false);
    };
    let listed = remotes::list(path)?;
    let Some(remote) = find_remote(&listed, server, &head.name_with_owner) else {
        return Ok(false);
    };
    let branch = format!("{}/{}", remote.name, details.head_ref_name);
    Ok(tracking(path, &branch)?.is_some_and(|(_, current)| current))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::{mpsc, Arc};
    use std::thread;
    use std::time::Duration;

    use tempfile::TempDir;

    use super::*;
    use crate::commands::RepoLocks;
    use crate::console::Console;
    use crate::task::{Runner, TaskId, TaskKind};
    use crate::test_support::{git_command, in_task_as, TempRepo};

    // The task kinds of `github_pr_checkout` and `github_pr_show_in_log`.
    const CHECKOUT_KIND: TaskKind = TaskKind::Read;
    const SHOW_KIND: TaskKind = TaskKind::Read;

    const BASE_URL: &str = "https://github.com/o/r.git";
    const FORK_URL: &str = "https://github.com/forker/r";

    fn git_in(path: &Path, args: &[&str]) -> String {
        let output = git_command().current_dir(path).args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    fn bare() -> TempDir {
        let dir = TempDir::new().unwrap();
        git_in(dir.path(), &["init", "-q", "--bare", "-b", "main"]);
        dir
    }

    fn remote(name: &str, url: &str) -> Remote {
        Remote {
            name: name.to_string(),
            url: url.to_string(),
            push_url: None,
        }
    }

    fn head(owner: &str, is_fork: bool) -> HeadRepository {
        HeadRepository {
            owner: owner.to_string(),
            is_fork,
            name_with_owner: format!("{owner}/r"),
            url: format!("https://github.com/{owner}/r"),
            ssh_url: format!("git@github.com:{owner}/r.git"),
        }
    }

    fn details(head_repository: Option<HeadRepository>, head_ref: &str) -> PullRequestDetails {
        PullRequestDetails {
            number: 7,
            url: "https://github.com/o/r/pull/7".to_string(),
            title: String::new(),
            body: String::new(),
            state: "OPEN".to_string(),
            is_draft: false,
            base_ref_name: "main".to_string(),
            base_ref_oid: String::new(),
            head_ref_name: head_ref.to_string(),
            head_ref_oid: String::new(),
            head_repository,
            reviewers: Vec::new(),
        }
    }

    // The repository has `origin` at `https://github.com/o/r.git`, and the fork is
    // `https://github.com/forker/r`. Both URLs are rewritten to local bare repositories by
    // `url.<path>.insteadOf`, and https and ssh are refused, so nothing reaches the network even
    // if a rewrite were missing.
    struct Harness {
        repo: TempRepo,
        data: TempDir,
        upstream: TempDir,
        fork: TempDir,
        console: Arc<Console>,
        runner: Runner,
        sync: Arc<BranchSync>,
    }

    impl Harness {
        fn new() -> Self {
            let repo = TempRepo::new();
            let upstream = bare();
            let fork = bare();
            for (bare, url) in [(&upstream, BASE_URL), (&fork, FORK_URL)] {
                let key = format!("url.{}.insteadOf", bare.path().display());
                repo.git(&["config", &key, url]);
            }
            repo.git(&["config", "protocol.https.allow", "never"]);
            repo.git(&["config", "protocol.ssh.allow", "never"]);
            repo.commit("a.txt", b"1\n", "one");
            repo.git(&["remote", "add", "origin", BASE_URL]);
            repo.git(&["push", "-q", "-u", "origin", "main"]);
            let console = Arc::<Console>::default();
            let runner = Runner::with(
                Arc::<RepoLocks>::default(),
                console.clone(),
                None,
                Duration::from_secs(5),
            );
            Self {
                repo,
                data: TempDir::new().unwrap(),
                upstream,
                fork,
                console,
                runner,
                sync: Arc::default(),
            }
        }

        // `branch` of `bare` with one more commit than `main`, which adds `file`.
        fn push_branch(&self, bare: &TempDir, branch: &str, file: &str) {
            let scratch = TempRepo::new();
            let path = self.upstream.path().to_str().unwrap();
            scratch.git(&["remote", "add", "origin", path]);
            scratch.git(&["fetch", "-q", "origin"]);
            scratch.git(&["checkout", "-q", "-B", branch, "origin/main"]);
            scratch.commit(file, b"from the pull request\n", "pull request");
            let target = bare.path().to_str().unwrap();
            scratch.git(&[
                "push",
                "-q",
                target,
                &format!("{branch}:refs/heads/{branch}"),
            ]);
            if bare.path() != self.upstream.path() {
                scratch.git(&["push", "-q", target, "origin/main:refs/heads/main"]);
            }
        }

        fn prepare(&self, details: PullRequestDetails) -> Result<Headed<Prepared>, GitError> {
            let sync = self.sync.clone();
            in_task_as(&self.runner, self.repo.path(), TaskKind::Read, move |ctx| {
                prepare(ctx, &sync, &GithubServer::github_com(), &details)
            })
        }

        fn checkout(&self, details: PullRequestDetails) -> Result<Headed<CheckedOut>, GitError> {
            let sync = self.sync.clone();
            let path = self.repo.path().to_path_buf();
            let data = self.data.path().to_path_buf();
            in_task_as(&self.runner, self.repo.path(), CHECKOUT_KIND, move |ctx| {
                let repo = Repo {
                    ctx,
                    path: &path,
                    data: &data,
                };
                checkout(&repo, &sync, &GithubServer::github_com(), &details, || {
                    Ok(())
                })
            })
        }

        fn show_in_log(&self, details: PullRequestDetails) -> Result<Headed<Range>, GitError> {
            let sync = self.sync.clone();
            in_task_as(&self.runner, self.repo.path(), SHOW_KIND, move |ctx| {
                show_in_log(ctx, &sync, &GithubServer::github_com(), &details)
            })
        }

        // Another fork at `url`, rewritten to a new local bare repository.
        fn fork_at(&self, url: &str) -> TempDir {
            let fork = bare();
            let key = format!("url.{}.insteadOf", fork.path().display());
            self.repo.git(&["config", &key, url]);
            fork
        }

        // `work` as a task of `kind`, as a command starts it; its value comes back on the channel.
        fn spawn<T: Send + 'static>(
            &self,
            kind: TaskKind,
            work: impl FnOnce(&TaskContext) -> T + Send + 'static,
        ) -> (TaskId, mpsc::Receiver<T>) {
            let (send, receive) = mpsc::channel();
            let id = self.runner.start(
                "main",
                self.repo.path(),
                kind,
                "test".to_string(),
                move |ctx| {
                    let _ = send.send(work(ctx));
                    Ok(())
                },
            );
            (id, receive)
        }

        fn commands(&self) -> Vec<String> {
            let lines = self.console.lines_of("main");
            lines
                .into_iter()
                .filter(|line| line.command)
                .map(|line| line.text)
                .collect()
        }

        fn remotes(&self) -> Vec<(String, String)> {
            remotes::list(self.repo.path())
                .unwrap()
                .into_iter()
                .map(|remote| (remote.name, remote.url))
                .collect()
        }

        fn current(&self) -> String {
            self.repo
                .git(&["branch", "--show-current"])
                .trim()
                .to_string()
        }

        fn snapshots(&self) -> usize {
            crate::snapshots::list(self.data.path()).len()
        }
    }

    fn ready<T: std::fmt::Debug>(headed: Headed<T>) -> T {
        match headed {
            Headed::Ready(value) => value,
            other => panic!("not ready: {other:?}"),
        }
    }

    #[test]
    fn the_answers_reach_the_interface_tagged_by_kind() {
        assert_eq!(
            json(&Headed::Ready(Range {
                from: "forker/main".to_string(),
                to: "forker/feature".to_string(),
            })),
            serde_json::json!({ "kind": "ready", "from": "forker/main", "to": "forker/feature" })
        );
        assert_eq!(
            json(&Headed::Ready(CheckedOut {
                branch: "forker/feature".to_string(),
                suggested: "fork/feature".to_string(),
                outcome: RemoteCheckout::Done,
            })),
            serde_json::json!({
                "kind": "ready",
                "branch": "forker/feature",
                "suggested": "fork/feature",
                "outcome": { "kind": "done" }
            })
        );
        assert_eq!(
            json(&Headed::<Range>::NoRemote {
                urls: vec!["https://github.com/forker/r".to_string()]
            }),
            serde_json::json!({ "kind": "noRemote", "urls": ["https://github.com/forker/r"] })
        );
        assert_eq!(
            json(&Headed::<Range>::FetchFailed {
                output: "fatal".to_string()
            }),
            serde_json::json!({ "kind": "fetchFailed", "output": "fatal" })
        );
        assert_eq!(
            json(&Headed::<Range>::Gone),
            serde_json::json!({ "kind": "gone" })
        );
    }

    fn json(value: &impl Serialize) -> serde_json::Value {
        serde_json::to_value(value).unwrap()
    }

    #[test]
    fn checkout_finds_the_heads_remote_or_adds_one_named_after_its_owner() {
        // Review Focus 2: every shape of URL gives `findRemote`'s answer.
        let github = GithubServer::github_com();
        let found = |url: &str, path: &str| {
            let listed = [remote("r", url)];
            find_remote(&listed, &github, path).is_some()
        };
        assert!(found("git@github.com:o/r.git", "o/r"));
        assert!(found("ssh://git@github.com:22/o/r", "o/r"));
        assert!(found("https://github.com/o/r/", "o/r"));
        assert!(found("https://GitHub.com/o/r.git", "o/r"));
        // `endsWith` is case-sensitive and has no boundary, as in 241.
        assert!(!found("https://github.com/O/R.git", "o/r"));
        assert!(found("https://github.com/xo/r.git", "o/r"));
        assert!(!found("https://gitlab.com/o/r.git", "o/r"));
        assert!(!found("/srv/git/o/r.git", "o/r"));
        assert!(!found("https://github.com/o/r with space", "o/r"));
        let enterprise = GithubServer::parse("https://ghe.example.com/prefix").unwrap();
        let listed = [remote("e", "https://ghe.example.com/prefix/o/r.git")];
        assert!(find_remote(&listed, &enterprise, "o/r").is_some());
        let listed = [remote("e", "https://ghe.example.com/o/r.git")];
        assert!(find_remote(&listed, &enterprise, "o/r").is_none());
        // Two remotes on the repository: the first one; `origin` a fork of `upstream`.
        let listed = [
            remote("origin", "https://github.com/me/r.git"),
            remote("upstream", "git@github.com:o/r.git"),
            remote("other", "https://github.com/o/r"),
        ];
        assert_eq!(
            find_remote(&listed, &github, "o/r").unwrap().name,
            "upstream"
        );
        assert_eq!(
            find_remote(&listed, &github, "me/r").unwrap().name,
            "origin"
        );

        let harness = Harness::new();
        harness.push_branch(&harness.upstream, "own", "own.txt");
        harness.push_branch(&harness.fork, "feature", "feature.txt");

        // The head is the repository `origin` points to: no remote is added.
        let prepared = ready(
            harness
                .prepare(details(Some(head("o", false)), "own"))
                .unwrap(),
        );
        assert_eq!(
            prepared,
            Prepared {
                remote: "origin".to_string(),
                branch: "origin/own".to_string(),
                suggested: "own".to_string(),
            }
        );
        assert_eq!(harness.remotes().len(), 1);

        // A fork: a remote named after its owner, with the web URL, since `origin` is HTTP.
        let prepared = ready(
            harness
                .prepare(details(Some(head("forker", true)), "feature"))
                .unwrap(),
        );
        assert_eq!(prepared.remote, "forker");
        assert_eq!(prepared.branch, "forker/feature");
        assert_eq!(
            harness.remotes(),
            [
                ("origin".to_string(), BASE_URL.to_string()),
                ("forker".to_string(), FORK_URL.to_string())
            ]
        );
        assert!(harness
            .commands()
            .iter()
            .any(|command| command.ends_with(&format!("remote add forker {FORK_URL}"))));

        // A deleted fork: nothing, and no git at all.
        let before = harness.commands().len();
        assert_eq!(harness.prepare(details(None, "feature")), Ok(Headed::Gone));
        assert_eq!(harness.commands().len(), before);
    }

    #[test]
    fn a_taken_name_becomes_login_1_then_login_2() {
        let names = |list: &[&str]| list.iter().map(|name| name.to_string()).collect::<Vec<_>>();
        assert_eq!(remote_name(&names(&["origin"]), "forker"), "forker");
        assert_eq!(
            remote_name(&names(&["origin", "forker"]), "forker"),
            "forker_1"
        );
        assert_eq!(
            remote_name(&names(&["forker", "forker_1"]), "forker"),
            "forker_2"
        );
        assert_eq!(
            remote_name(&names(&["forker", "forker_2"]), "forker"),
            "forker_1"
        );

        // 241 fails here with `Branch checkout failed`: `forker` is another repository.
        let harness = Harness::new();
        harness.push_branch(&harness.fork, "feature", "feature.txt");
        let elsewhere = bare();
        let path = elsewhere.path().to_str().unwrap();
        harness.repo.git(&["remote", "add", "forker", path]);
        let prepared = ready(
            harness
                .prepare(details(Some(head("forker", true)), "feature"))
                .unwrap(),
        );
        assert_eq!(prepared.remote, "forker_1");
        assert_eq!(prepared.branch, "forker_1/feature");
        assert!(harness
            .remotes()
            .contains(&("forker_1".to_string(), FORK_URL.to_string())));
    }

    #[test]
    fn http_or_ssh_follows_origin() {
        let fork = head("forker", true);
        let url = |listed: &[Remote]| remote_url(listed, &fork).map(str::to_string);
        let http = Some(FORK_URL.to_string());
        let ssh = Some("git@github.com:forker/r.git".to_string());
        assert_eq!(url(&[]), http);
        assert_eq!(url(&[remote("origin", "https://github.com/o/r.git")]), http);
        assert_eq!(url(&[remote("origin", "git@github.com:o/r.git")]), ssh);
        // Without `origin`, the first remote decides; `origin` decides even when it is not first.
        assert_eq!(url(&[remote("a", "ssh://git@github.com/o/r")]), ssh);
        assert_eq!(url(&[remote("a", "http://github.com/o/r")]), http);
        assert_eq!(
            url(&[
                remote("a", "https://github.com/o/r"),
                remote("origin", "git@github.com:o/r.git")
            ]),
            ssh
        );
        // HTTP preferred without a web URL is SSH; SSH preferred without an SSH URL is none.
        let no_web = HeadRepository {
            url: String::new(),
            ..fork.clone()
        };
        assert_eq!(
            remote_url(&[], &no_web),
            Some("git@github.com:forker/r.git")
        );
        let no_ssh = HeadRepository {
            ssh_url: String::new(),
            ..fork.clone()
        };
        let ssh_origin = [remote("origin", "git@github.com:o/r.git")];
        assert_eq!(remote_url(&ssh_origin, &no_ssh), None);

        // What `git remote add` takes: the server's host only, and no other transport.
        let github = GithubServer::github_com();
        for good in [
            FORK_URL,
            "https://GITHUB.com/forker/r",
            "https://github.com:443/forker/r",
            "git@github.com:forker/r.git",
            "ssh://git@github.com:22/forker/r.git",
        ] {
            assert!(addable_url(&github, good), "{good}");
        }
        for bad in [
            "https://evil.example/forker/r",
            "https://github.com.evil.example/forker/r",
            "https://user:secret@github.com/forker/r",
            "https://github.com:8443/forker/r",
            "http://github.com/forker/r",
            "git@evil.example:forker/r.git",
            "ext::sh -c touch% x",
            "file:///tmp/forker/r",
            "-oProxyCommand=touch x",
            "/tmp/forker/r",
            "git@github.com:forker/r with space",
            "",
        ] {
            assert!(!addable_url(&github, bad), "{bad}");
        }
        let plain = GithubServer::parse("http://ghe.example.com").unwrap();
        assert!(addable_url(&plain, "http://ghe.example.com/forker/r"));

        // An URL that cannot be added is a remote that cannot be resolved, with GitHub's URLs.
        let harness = Harness::new();
        let evil = HeadRepository {
            url: "https://evil.example/forker/r".to_string(),
            ssh_url: "git@evil.example:forker/r.git".to_string(),
            ..fork
        };
        assert_eq!(
            harness.prepare(details(Some(evil), "feature")),
            Ok(Headed::NoRemote {
                urls: vec![
                    "https://evil.example/forker/r".to_string(),
                    "git@evil.example:forker/r.git".to_string()
                ]
            })
        );
        assert_eq!(harness.remotes().len(), 1);
    }

    #[test]
    fn checkout_fetches_the_head_branch_and_keeps_a_recoverable_copy() {
        let harness = Harness::new();
        harness.push_branch(&harness.fork, "feature", "build.txt");
        // An ignored file where the branch has one: a plain checkout overwrites it silently.
        fs::write(harness.repo.path().join(".git/info/exclude"), "build.txt\n").unwrap();
        fs::write(harness.repo.path().join("build.txt"), "mine\n").unwrap();
        assert_eq!(harness.snapshots(), 0);

        let checked = ready(
            harness
                .checkout(details(Some(head("forker", true)), "feature"))
                .unwrap(),
        );

        assert_eq!(
            checked,
            CheckedOut {
                branch: "forker/feature".to_string(),
                suggested: "fork/feature".to_string(),
                outcome: RemoteCheckout::Done,
            }
        );
        let commands = harness.commands();
        assert!(
            commands.iter().any(|command| command.ends_with(
                "fetch --recurse-submodules=no --progress --prune --end-of-options forker feature"
            )),
            "{commands:?}"
        );
        assert_eq!(harness.current(), "fork/feature");
        let upstream = harness
            .repo
            .git(&["rev-parse", "--abbrev-ref", "fork/feature@{u}"]);
        assert_eq!(upstream.trim(), "forker/feature");
        assert_eq!(harness.snapshots(), 1);
        let saved = crate::snapshots::list(harness.data.path());
        let paths: Vec<&str> = saved[0]
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect();
        assert_eq!(paths, ["build.txt"]);

        // Again: the branch that tracks it is offered, and nothing was at risk this time.
        let again = ready(
            harness
                .checkout(details(Some(head("forker", true)), "feature"))
                .unwrap(),
        );
        assert_eq!(again.suggested, "fork/feature");
        assert_eq!(again.outcome, RemoteCheckout::Done);
        assert_eq!(harness.snapshots(), 1);

        // A local branch of that name tracking another branch: the name is asked for.
        harness.repo.git(&["checkout", "-q", "main"]);
        harness
            .repo
            .git(&["branch", "-q", "--track", "own", "origin/main"]);
        harness.push_branch(&harness.upstream, "own", "own.txt");
        let asked = ready(
            harness
                .checkout(details(Some(head("o", false)), "own"))
                .unwrap(),
        );
        assert_eq!(
            asked.outcome,
            RemoteCheckout::NeedsName {
                suggested: "own".to_string()
            }
        );
        assert_eq!(harness.current(), "main");
    }

    #[test]
    fn a_head_branch_named_dash_x_never_reaches_git_as_an_option() {
        let harness = Harness::new();
        for name in [
            "-x",
            "--upload-pack=touch x",
            "+main",
            "a..b",
            "@{-1}",
            "",
            "a b",
        ] {
            let error = harness
                .prepare(details(Some(head("forker", true)), name))
                .unwrap_err();
            assert!(matches!(error, GitError::Io(_)), "{name}: {error:?}");
        }
        // An owner that git would read as an option is refused before `git remote add`.
        let dashed = HeadRepository {
            owner: "-x".to_string(),
            ..head("forker", true)
        };
        assert!(harness.prepare(details(Some(dashed), "feature")).is_err());
        assert!(harness
            .show_in_log(PullRequestDetails {
                base_ref_name: "--output=x".to_string(),
                ..details(Some(head("forker", true)), "feature")
            })
            .is_err());

        let commands = harness.commands();
        assert!(
            commands
                .iter()
                .all(|command| !command.contains(" fetch ") && !command.contains(" remote add ")),
            "{commands:?}"
        );
        assert_eq!(harness.remotes().len(), 1);
        assert!(!harness.repo.path().join("x").exists());
    }

    #[test]
    fn a_failed_fetch_says_fetch_failed_with_gits_output() {
        let harness = Harness::new();
        // The fork exists but has no `missing` branch.
        harness.push_branch(&harness.fork, "feature", "feature.txt");

        let failed = harness
            .checkout(details(Some(head("forker", true)), "missing"))
            .unwrap();

        let Headed::FetchFailed { output } = failed else {
            panic!("{failed:?}");
        };
        assert!(
            output.contains("couldn't find remote ref missing"),
            "{output}"
        );
        assert_eq!(harness.current(), "main");
        assert_eq!(harness.snapshots(), 0);
    }

    #[test]
    fn show_in_git_log_is_the_heads_remotes_base_and_head() {
        let harness = Harness::new();
        harness.push_branch(&harness.fork, "feature", "feature.txt");

        let range = ready(
            harness
                .show_in_log(details(Some(head("forker", true)), "feature"))
                .unwrap(),
        );

        // The base is the head's remote's, as in 241, and only the head is fetched.
        assert_eq!(
            range,
            Range {
                from: "forker/main".to_string(),
                to: "forker/feature".to_string(),
            }
        );
        let fetches = harness
            .commands()
            .into_iter()
            .filter(|command| command.contains(" fetch "))
            .count();
        assert_eq!(fetches, 1);
        assert_eq!(harness.current(), "main");
        assert_eq!(
            harness.show_in_log(details(None, "feature")),
            Ok(Headed::Gone)
        );
    }

    #[test]
    fn checked_out_is_the_current_branch_tracking_the_heads_remote() {
        let harness = Harness::new();
        harness.push_branch(&harness.fork, "feature", "feature.txt");
        let shown = details(Some(head("forker", true)), "feature");
        let github = GithubServer::github_com();
        let checked = || is_checked_out(harness.repo.path(), &github, &shown).unwrap();
        // No remote for the fork yet.
        assert!(!checked());

        ready(harness.checkout(shown.clone()).unwrap());
        assert!(checked());

        harness.repo.git(&["checkout", "-q", "main"]);
        assert!(!checked());
        // The first branch that tracks it is the one: another one checked out does not count.
        harness
            .repo
            .git(&["branch", "-q", "--track", "another", "forker/feature"]);
        harness.repo.git(&["checkout", "-q", "fork/feature"]);
        assert!(!checked());
        harness.repo.git(&["checkout", "-q", "another"]);
        assert!(checked());
        harness.repo.git(&["remote", "remove", "forker"]);
        assert!(!checked());
        assert!(!is_checked_out(harness.repo.path(), &github, &details(None, "feature")).unwrap());
    }

    #[test]
    fn a_checkout_and_a_remote_branch_deletion_on_the_same_remote_both_finish() {
        let harness = Harness::new();
        harness.push_branch(&harness.upstream, "own", "own.txt");
        harness.push_branch(&harness.upstream, "gone", "gone.txt");
        harness.repo.git(&["fetch", "-q", "origin"]);
        harness
            .repo
            .git(&["branch", "-q", "--track", "gone", "origin/gone"]);
        let path = harness.repo.path().to_path_buf();
        let data = harness.data.path().to_path_buf();

        // `branch_delete_remote` holds `origin` and then wants the write lock for the local
        // branch; the checkout of a pull request from `origin` starts in between.
        let (held, holding) = mpsc::channel();
        let (place, folder) = (path.clone(), data.clone());
        let (deleting, deleted) =
            harness.spawn(TaskKind::Remote("origin".to_string()), move |ctx| {
                held.send(()).unwrap();
                thread::sleep(Duration::from_millis(300));
                let repo = Repo {
                    ctx,
                    path: &place,
                    data: &folder,
                };
                branches::delete_remote(&repo, "origin", "gone", true)
            });
        holding.recv().unwrap();
        let sync = harness.sync.clone();
        let (checking, checked) = harness.spawn(CHECKOUT_KIND, move |ctx| {
            let repo = Repo {
                ctx,
                path: &path,
                data: &data,
            };
            let shown = details(Some(head("o", false)), "own");
            checkout(&repo, &sync, &GithubServer::github_com(), &shown, || Ok(()))
        });

        let deleted = deleted.recv_timeout(Duration::from_secs(20));
        let checked = checked.recv_timeout(Duration::from_secs(20));
        if deleted.is_err() || checked.is_err() {
            harness.runner.cancel("main", deleting);
            harness.runner.cancel("main", checking);
        }
        assert!(
            deleted.is_ok() && checked.is_ok(),
            "the deletion and the checkout wait for each other"
        );
        let deleted = deleted.unwrap().unwrap();
        assert_eq!(deleted.local.len(), 1, "{deleted:?}");
        assert_eq!(
            ready(checked.unwrap().unwrap()).outcome,
            RemoteCheckout::Done
        );
        assert_eq!(harness.current(), "own");
    }

    #[test]
    fn concurrent_shows_in_git_log_pick_distinct_remote_names() {
        let harness = Harness::new();
        // Four repositories of the same owner: each wants a remote named `forker`.
        let names = ["r1", "r2", "r3", "r4"];
        let forks: Vec<TempDir> = names
            .iter()
            .map(|name| {
                let fork = harness.fork_at(&format!("https://github.com/forker/{name}"));
                harness.push_branch(&fork, "feature", "feature.txt");
                fork
            })
            .collect();

        let running: Vec<_> = names
            .iter()
            .map(|name| {
                let sync = harness.sync.clone();
                let head = HeadRepository {
                    owner: "forker".to_string(),
                    is_fork: true,
                    name_with_owner: format!("forker/{name}"),
                    url: format!("https://github.com/forker/{name}"),
                    ssh_url: format!("git@github.com:forker/{name}.git"),
                };
                harness.spawn(SHOW_KIND, move |ctx| {
                    show_in_log(
                        ctx,
                        &sync,
                        &GithubServer::github_com(),
                        &details(Some(head), "feature"),
                    )
                })
            })
            .collect();
        let mut remotes: Vec<String> = running
            .into_iter()
            .map(|(_, answer)| {
                let range = ready(
                    answer
                        .recv_timeout(Duration::from_secs(20))
                        .unwrap()
                        .unwrap(),
                );
                range.to.strip_suffix("/feature").unwrap().to_string()
            })
            .collect();
        remotes.sort();

        assert_eq!(remotes, ["forker", "forker_1", "forker_2", "forker_3"]);
        let configured: Vec<String> = harness
            .remotes()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(configured.len(), 5, "{configured:?}");
        drop(forks);
    }
}
