// `Git.Fetch`, with the steps of `GitFetchSupportImpl`. It does not touch the tree or the index: a
// commit does not have to wait for the network.
use serde::Serialize;

use crate::branch_info::BranchSync;
use crate::branches;
use crate::console;
use crate::git::GitError;
use crate::task::TaskContext;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FetchFailure {
    pub remote: String,
    pub error: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct FetchResult {
    pub pruned: Vec<String>,
    pub failed: Vec<FetchFailure>,
}

// `x [deleted] (none) -> origin/branch`, and in recent versions with `-` instead of `x`.
fn pruned_ref(line: &str) -> Option<&str> {
    let (flag, rest) = line.trim_start().split_at_checked(1)?;
    if flag != "x" && flag != "-" {
        return None;
    }
    let rest = rest.trim_start().strip_prefix("[deleted]")?;
    let name = rest.rsplit_once("->")?.1.trim();
    (!name.is_empty()).then_some(name)
}

// What git said, without the "git fetch … failed" in front: it is what IntelliJ shows.
pub fn error_text(error: GitError) -> String {
    match error {
        GitError::Failed { stderr, .. } if !stderr.trim().is_empty() => {
            console::hide_credentials(stderr.trim())
        }
        error => branches::text_of(error),
    }
}

// The deleted remote branches.
pub fn fetch_remote(
    ctx: &TaskContext,
    sync: &BranchSync,
    remote: &str,
) -> Result<Vec<String>, GitError> {
    branches::check_argument(remote, "The remote")?;
    let args = [
        "fetch",
        remote,
        "--recurse-submodules=no",
        "--progress",
        "--prune",
    ];
    let output = ctx
        .on_remote(remote, || ctx.git_loud(&args))
        .ok_or(GitError::Cancelled)??;
    sync.mark_authenticated(ctx.repo(), remote);
    sync.fetched(ctx, ctx.repo(), remote);
    let pruned = output.stderr.lines().filter_map(pruned_ref);
    Ok(pruned.map(str::to_string).collect())
}

// A remote that fails does not stop the others, like `fetchAllRemotes`.
pub fn fetch_all(ctx: &TaskContext, sync: &BranchSync) -> Result<FetchResult, GitError> {
    let mut result = FetchResult::default();
    for remote in ctx.git(&["remote"])?.stdout.lines() {
        match fetch_remote(ctx, sync, remote) {
            Ok(pruned) => result.pruned.extend(pruned),
            Err(GitError::Cancelled) => return Err(GitError::Cancelled),
            Err(error) => result.failed.push(FetchFailure {
                remote: remote.to_string(),
                error: error_text(error),
            }),
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::RepoLocks;
    use crate::console::Console;
    use crate::hosting::protection::testing::{entries, entry, rules, seed, GithubRemote};
    use crate::task::{Runner, TaskKind};
    use crate::test_support::{Reply, TempRepo};
    use std::path::Path;
    use std::sync::{mpsc, Arc};
    use std::time::Duration;
    use tempfile::TempDir;

    const WAIT: Duration = Duration::from_secs(60);

    struct Harness {
        repo: TempRepo,
        origin: TempDir,
        console: Arc<Console>,
        runner: Runner,
        sync: Arc<BranchSync>,
    }

    fn bare() -> TempDir {
        let dir = TempDir::new().unwrap();
        let status = crate::test_support::git_command()
            .args(["init", "-q", "--bare", "-b", "main"])
            .arg(dir.path())
            .status()
            .unwrap();
        assert!(status.success());
        dir
    }

    impl Harness {
        fn new() -> Self {
            let repo = TempRepo::new();
            let origin = bare();
            repo.commit("a.txt", b"1\n", "one");
            repo.git(&["remote", "add", "origin", origin.path().to_str().unwrap()]);
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
                origin,
                console,
                runner,
                sync: Arc::default(),
            }
        }

        fn fetch(&self) -> Result<FetchResult, GitError> {
            let (send, receive) = mpsc::channel();
            let sync = self.sync.clone();
            self.runner.start(
                "main",
                self.repo.path(),
                TaskKind::Read,
                "Fetching…".to_string(),
                move |ctx| {
                    send.send(fetch_all(ctx, &sync)).unwrap();
                    Ok(())
                },
            );
            receive.recv_timeout(WAIT).unwrap()
        }

        fn fetch_one(&self, remote: &str) -> Result<Vec<String>, GitError> {
            let (send, receive) = mpsc::channel();
            let sync = self.sync.clone();
            let remote = remote.to_string();
            self.runner.start(
                "main",
                self.repo.path(),
                TaskKind::Read,
                "Fetching…".to_string(),
                move |ctx| {
                    send.send(fetch_remote(ctx, &sync, &remote)).unwrap();
                    Ok(())
                },
            );
            receive.recv_timeout(WAIT).unwrap()
        }

        // The rules of the remotes that point to `github` are loaded into `data` after each fetch.
        fn loading_rules(&self, github: &GithubRemote, data: &Path) {
            self.sync.set_after_fetch(github.hook(data));
        }

        fn commands(&self) -> Vec<String> {
            let lines = self.console.lines_of("main");
            let commands = lines.into_iter().filter(|line| line.command);
            commands.map(|line| line.text).collect()
        }
    }

    fn git_in(path: &Path, args: &[&str]) {
        assert!(crate::test_support::git_command()
            .current_dir(path)
            .args(args)
            .status()
            .unwrap()
            .success());
    }

    #[test]
    fn fetch_runs_each_remote_with_prune() {
        let harness = Harness::new();
        let second = bare();
        let url = second.path().to_str().unwrap();
        harness.repo.git(&["remote", "add", "copy", url]);

        let result = harness.fetch();

        assert_eq!(result, Ok(FetchResult::default()));
        let commands = harness.commands();
        for remote in ["origin", "copy"] {
            let expected = format!("fetch {remote} --recurse-submodules=no --progress --prune");
            assert!(
                commands.iter().any(|line| line.ends_with(&expected)),
                "{commands:?}"
            );
        }
    }

    #[test]
    fn fetch_reports_deleted_branches() {
        let harness = Harness::new();
        harness.repo.git(&["push", "-q", "origin", "main:old"]);
        harness.repo.git(&["fetch", "-q", "origin"]);
        git_in(harness.origin.path(), &["branch", "-q", "-D", "old"]);

        let result = harness.fetch().unwrap();

        assert_eq!(result.pruned, ["origin/old"]);
        assert_eq!(result.failed, []);
        assert_eq!(
            harness.repo.git(&["branch", "-r", "--list", "origin/old"]),
            ""
        );
    }

    #[test]
    fn failed_fetch_keeps_stderr() {
        let harness = Harness::new();
        let dir = TempDir::new().unwrap();
        let gone = dir.path().join("does-not-exist");
        let url = gone.to_str().unwrap();
        harness.repo.git(&["remote", "add", "broken", url]);

        let result = harness.fetch().unwrap();

        assert_eq!(result.failed.len(), 1);
        assert_eq!(result.failed[0].remote, "broken");
        assert!(
            result.failed[0]
                .error
                .contains("does not appear to be a git repository"),
            "{:?}",
            result.failed
        );
    }

    #[test]
    fn a_failing_remote_does_not_stop_the_others_and_only_the_one_that_answers_counts() {
        let harness = Harness::new();
        let dir = TempDir::new().unwrap();
        let gone = dir.path().join("does-not-exist");
        harness
            .repo
            .git(&["remote", "add", "aaa", gone.to_str().unwrap()]);

        harness.fetch().unwrap();

        let repo = harness.repo.path();
        assert!(harness.sync.is_authenticated(repo, "origin"));
        assert!(!harness.sync.is_authenticated(repo, "aaa"));
    }

    #[test]
    fn a_remote_with_a_dash_does_not_reach_git_as_an_option() {
        let harness = Harness::new();
        let config = harness.repo.path().join(".git/config");
        let mut text = std::fs::read_to_string(&config).unwrap();
        text.push_str("[remote \"--upload-pack=touch\"]\n\turl = /does/not/exist\n");
        std::fs::write(&config, text).unwrap();

        let result = harness.fetch().unwrap();

        assert_eq!(result.failed.len(), 1);
        assert!(!harness
            .commands()
            .iter()
            .any(|line| line.contains("--upload-pack")));
    }

    // Decision 6: each remote has its own entry, and 241 would have emptied the whole list.
    #[test]
    fn fetching_a_non_github_remote_clears_only_its_entry() {
        let harness = Harness::new();
        let github = GithubRemote::new();
        let data = TempDir::new().unwrap();
        harness.loading_rules(&github, data.path());
        let url = harness.origin.path().to_str().unwrap();
        harness.repo.git(&["remote", "add", "hub", url]);
        github.point(harness.repo.path(), "hub", harness.origin.path());
        seed(
            data.path(),
            &[
                ("origin", &["stale"]),
                ("hub", &[r"release\/.*"]),
                ("gone", &["old"]),
            ],
        );

        // `origin` is a local path: no request, its entry goes, and so does the removed remote's.
        harness.fetch_one("origin").unwrap();
        github.fake.finish();
        assert!(github.fake.requests().is_empty());
        assert_eq!(entries(data.path()), [entry("hub", &[r"release\/.*"])]);

        github.fake.reply(rules(&["main"]));
        harness.fetch_one("hub").unwrap();
        github.fake.finish();
        assert_eq!(entries(data.path()), [entry("hub", &["main"])]);
    }

    #[test]
    fn a_loader_error_keeps_the_entry_and_the_fetch_still_succeeds() {
        let harness = Harness::new();
        let github = GithubRemote::new();
        let data = TempDir::new().unwrap();
        harness.loading_rules(&github, data.path());
        github.point(harness.repo.path(), "origin", harness.origin.path());
        seed(data.path(), &[("origin", &["main"])]);
        harness.repo.git(&["push", "-q", "origin", "main:old"]);
        harness.repo.git(&["fetch", "-q", "origin"]);
        git_in(harness.origin.path(), &["branch", "-q", "-D", "old"]);

        github.fake.script(vec![
            Reply::json(
                "POST",
                "/api/graphql",
                502,
                "Bad Gateway",
                r#"{"message":"Server Error"}"#,
            ),
            Reply::json(
                "POST",
                "/api/graphql",
                200,
                "OK",
                r#"{"errors":[{"message":"Could not resolve to a Repository"}]}"#,
            ),
        ]);
        let first = harness.fetch();
        let second = harness.fetch();
        github.fake.finish();

        assert_eq!(
            first,
            Ok(FetchResult {
                pruned: vec!["origin/old".to_string()],
                failed: Vec::new(),
            })
        );
        assert_eq!(second, Ok(FetchResult::default()));
        assert_eq!(entries(data.path()), [entry("origin", &["main"])]);
        assert!(harness.sync.is_authenticated(harness.repo.path(), "origin"));
    }
}
