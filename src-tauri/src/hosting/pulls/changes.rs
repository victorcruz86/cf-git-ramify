// A pull request's changes, computed with git after the two fetches 241 does
// (GHPRChangesDataProviderImpl, GHPRChangesServiceImpl, GitFetchSupportImpl and
// GitBranchComparisonResultImpl, plugins/github and git4idea at idea/241.14494.240, P§6.7-6.8).
// Decision 9: the tree waits for both fetches. Nothing here writes to the work tree or the index:
// the fetches change remote refs and FETCH_HEAD only, and the rest only reads.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::Serialize;

use crate::branch_info::BranchSync;
use crate::branches;
use crate::compare;
use crate::diff::{self, ChangedFile};
use crate::git::{self, GitError};
use crate::history;
use crate::task::TaskContext;

use super::details::{PullRequestCommit, PullRequestDetails};

// GitFetchSupportImpl: `git fetch <remote> <refspec> --recurse-submodules=no --progress --prune`.
// The options go first here, so that `--end-of-options` can keep the remote and the refspec from
// being read as options.
const FETCH_OPTIONS: [&str; 4] = ["fetch", "--recurse-submodules=no", "--progress", "--prune"];

// A changed file with the two revisions its diff shows: the merge base and the head for the whole
// pull request, P§6.8's `before` and the commit for one commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeWithSides {
    pub path: String,
    pub old_path: Option<String>,
    pub status: String,
    pub before: String,
    pub after: String,
}

// The changes of the whole pull request, or of the commit at `selection`, with that commit's
// message body from git.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestChanges {
    pub changes: Vec<ChangeWithSides>,
    pub body: Option<String>,
}

fn with_sides(files: Vec<ChangedFile>, before: &str, after: &str) -> Vec<ChangeWithSides> {
    files
        .into_iter()
        .map(|file| ChangeWithSides {
            path: file.path,
            old_path: file.old_path,
            status: file.status,
            before: before.to_string(),
            after: after.to_string(),
        })
        .collect()
}

// `git check-ref-format --branch` normalizes as well as checks, and expands `@{-1}`: a name it
// changes is not the branch GitHub named. A leading `+` would make the refspec a forced fetch of
// another branch.
pub(crate) fn check_branch_name(ctx: &TaskContext, name: &str, what: &str) -> Result<(), GitError> {
    let invalid = || branches::invalid(format!("Branch name {name} is not valid"));
    branches::check_argument(name, what)?;
    if name.starts_with('+') {
        return Err(invalid());
    }
    match ctx.git(&["check-ref-format", "--branch", name]) {
        Ok(output) if output.stdout.trim_end_matches('\n') == name => Ok(()),
        Ok(_) | Err(GitError::Failed { .. }) => Err(invalid()),
        Err(error) => Err(error),
    }
}

// One fetch of `remote`, under its lock, then what every successful fetch is followed by.
pub(crate) fn fetch_one(
    ctx: &TaskContext,
    sync: &BranchSync,
    remote: &str,
    refspec: &str,
) -> Result<(), GitError> {
    let mut args = FETCH_OPTIONS.to_vec();
    args.extend(["--end-of-options", remote, refspec]);
    ctx.on_remote(remote, || ctx.git_loud(&args))
        .ok_or(GitError::Cancelled)??;
    sync.mark_authenticated(ctx.repo(), remote);
    sync.fetched(ctx, ctx.repo(), remote);
    Ok(())
}

// GHPRChangesDataProviderImpl's two fetches: the base branch, then `refs/pull/<n>/head:`, whose
// empty destination brings the objects and FETCH_HEAD and no local ref. `remote` is the remote of
// the connection, not the head's.
pub fn fetch(
    ctx: &TaskContext,
    sync: &BranchSync,
    remote: &str,
    base_ref: &str,
    number: u64,
) -> Result<(), GitError> {
    branches::check_argument(remote, "The remote")?;
    check_branch_name(ctx, base_ref, "The base branch")?;
    fetch_one(ctx, sync, remote, base_ref)?;
    fetch_one(ctx, sync, remote, &format!("refs/pull/{number}/head:"))
}

// What the diff of one commit lists: the commit against its first parent, as GitHub's file list of
// a commit is; a commit without parents against nothing.
fn commit_files(repo: &Path, commit: &PullRequestCommit) -> Result<Vec<ChangedFile>, GitError> {
    match commit.parents.first() {
        Some(parent) => compare::changes_between(repo, parent, &commit.oid, &[]),
        None => {
            let args = git::exact(&[
                "diff-tree",
                "-r",
                "-z",
                "-M",
                "--name-status",
                "--root",
                "--no-commit-id",
                &commit.oid,
            ]);
            Ok(diff::parse_name_status(&git::run(repo, &args)?))
        }
    }
}

// `merge base → head` for the whole pull request (`selection` none). For the commit at `selection`
// of `commits` (in P§6.5's order), GitBranchComparisonResultImpl's `before`: with a linear history
// (no commit has two parents inside the pull request) the commit before it in the list, the merge
// base for the first; else its first parent inside the pull request, or the merge base. Both
// objects must be here, so this runs after `fetch`.
pub fn changes(
    repo: &Path,
    base_oid: &str,
    head_oid: &str,
    commits: &[PullRequestCommit],
    selection: Option<usize>,
) -> Result<Vec<ChangeWithSides>, GitError> {
    diff::check_hash(base_oid)?;
    diff::check_hash(head_oid)?;
    for commit in commits {
        diff::check_hash(&commit.oid)?;
        commit
            .parents
            .iter()
            .try_for_each(|parent| diff::check_hash(parent))?;
    }
    let merge_base = diff::merge_base(repo, base_oid, head_oid)?.ok_or_else(|| {
        GitError::Io(format!(
            "Commits {base_oid} and {head_oid} have no common ancestor"
        ))
    })?;
    diff::check_hash(&merge_base)?;
    let Some(at) = selection else {
        let files = compare::changes_between(repo, &merge_base, head_oid, &[])?;
        return Ok(with_sides(files, &merge_base, head_oid));
    };
    let commit = commits
        .get(at)
        .ok_or_else(|| GitError::Io(format!("The pull request has no commit {at}")))?;
    let inside: HashSet<&str> = commits.iter().map(|commit| commit.oid.as_str()).collect();
    let linear = commits.iter().all(|commit| {
        commit
            .parents
            .iter()
            .filter(|parent| inside.contains(parent.as_str()))
            .count()
            <= 1
    });
    let before = if linear {
        at.checked_sub(1).map_or(merge_base.as_str(), |previous| {
            commits[previous].oid.as_str()
        })
    } else {
        commit
            .parents
            .iter()
            .map(String::as_str)
            .find(|parent| inside.contains(parent))
            .unwrap_or(&merge_base)
    };
    Ok(with_sides(commit_files(repo, commit)?, before, &commit.oid))
}

// The commit's message body, as git has it: the API sends it only as HTML.
pub fn commit_body(repo: &Path, oid: &str) -> Result<String, GitError> {
    diff::check_hash(oid)?;
    let mut args = history::NO_SIGNATURES.to_vec();
    args.extend(["log", "-1", "--format=%b", "--end-of-options", oid]);
    let output = git::run(repo, &args)?;
    Ok(String::from_utf8_lossy(&output).trim_end().to_string())
}

// What the open pull requests already have here, per repository and number, until the app quits:
// the base and head the two fetches brought, and the commits as the commits dropdown got them.
// A commit chosen in the dropdown needs neither the network nor the fetches again; the whole pull
// request, as `Refresh` and `Retry` ask it, always fetches.
#[derive(Default)]
struct Entry {
    fetched: Option<(String, String)>,
    commits: Option<Arc<Vec<PullRequestCommit>>>,
    // The details last loaded, with the URL of the connection they were asked through: the
    // checkout and the checked-out icon read them.
    details: Option<(String, Arc<PullRequestDetails>)>,
}

#[derive(Default)]
pub struct PullCache {
    entries: Mutex<HashMap<(PathBuf, u64), Entry>>,
}

impl PullCache {
    fn lock(&self) -> MutexGuard<'_, HashMap<(PathBuf, u64), Entry>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn was_fetched(&self, repo: &Path, number: u64, base: &str, head: &str) -> bool {
        self.lock()
            .get(&(repo.to_path_buf(), number))
            .and_then(|entry| entry.fetched.as_ref())
            .is_some_and(|(fetched_base, fetched_head)| {
                fetched_base == base && fetched_head == head
            })
    }

    // A fetch succeeded (`Some`), or it starts or failed (`None`).
    pub fn set_fetched(&self, repo: &Path, number: u64, fetched: Option<(String, String)>) {
        self.lock()
            .entry((repo.to_path_buf(), number))
            .or_default()
            .fetched = fetched;
    }

    // The commits last loaded, if their last one is `head`.
    pub fn commits(
        &self,
        repo: &Path,
        number: u64,
        head: &str,
    ) -> Option<Arc<Vec<PullRequestCommit>>> {
        self.lock()
            .get(&(repo.to_path_buf(), number))
            .and_then(|entry| entry.commits.clone())
            .filter(|commits| commits.last().is_some_and(|last| last.oid == head))
    }

    pub fn set_commits(&self, repo: &Path, number: u64, commits: Arc<Vec<PullRequestCommit>>) {
        self.lock()
            .entry((repo.to_path_buf(), number))
            .or_default()
            .commits = Some(commits);
    }

    // The details last loaded through the connection with that URL.
    pub fn details(&self, repo: &Path, number: u64, url: &str) -> Option<Arc<PullRequestDetails>> {
        self.lock()
            .get(&(repo.to_path_buf(), number))
            .and_then(|entry| entry.details.as_ref())
            .filter(|(asked, _)| asked == url)
            .map(|(_, details)| details.clone())
    }

    pub fn set_details(
        &self,
        repo: &Path,
        number: u64,
        url: &str,
        details: Arc<PullRequestDetails>,
    ) {
        self.lock()
            .entry((repo.to_path_buf(), number))
            .or_default()
            .details = Some((url.to_string(), details));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use tempfile::TempDir;

    use super::*;
    use crate::commands::RepoLocks;
    use crate::console::Console;
    use crate::task::Runner;
    use crate::test_support::{git_command, TempRepo};

    struct Harness {
        repo: TempRepo,
        origin: TempDir,
        console: Arc<Console>,
        runner: Runner,
        sync: Arc<BranchSync>,
    }

    fn git_in(path: &Path, args: &[&str]) -> String {
        let output = git_command().current_dir(path).args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    impl Harness {
        // `origin` has `main` and `refs/pull/7/head`; the clone has only its own `main`.
        fn new() -> Self {
            let repo = TempRepo::new();
            let origin = TempDir::new().unwrap();
            git_in(origin.path(), &["init", "-q", "--bare", "-b", "main"]);
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

        // The pull request's commits are written in a scratch clone, pushed only as
        // `refs/pull/7/head`, and the base branch moves on there too; this repository knows
        // neither. The commits, oldest first, and the bases.
        fn pull_request(&self, build: impl FnOnce(&TempRepo)) -> (String, String) {
            let scratch = TempRepo::new();
            scratch.git(&[
                "remote",
                "add",
                "origin",
                self.origin.path().to_str().unwrap(),
            ]);
            scratch.git(&["fetch", "-q", "origin"]);
            scratch.git(&["checkout", "-q", "-B", "main", "origin/main"]);
            let fork = scratch.git(&["rev-parse", "HEAD"]).trim().to_string();
            scratch.git(&["checkout", "-q", "-b", "feature"]);
            build(&scratch);
            scratch.git(&["push", "-q", "origin", "feature:refs/pull/7/head"]);
            scratch.git(&["checkout", "-q", "main"]);
            scratch.commit("base.txt", b"later\n", "base moved on");
            scratch.git(&["push", "-q", "origin", "main"]);
            let base = scratch.git(&["rev-parse", "main"]).trim().to_string();
            (fork, base)
        }

        fn head(&self) -> String {
            git_in(self.origin.path(), &["rev-parse", "refs/pull/7/head"])
        }

        fn fetch(&self, base_ref: &str) -> Result<(), GitError> {
            let sync = self.sync.clone();
            let base_ref = base_ref.to_string();
            crate::test_support::in_task(&self.runner, self.repo.path(), move |ctx| {
                fetch(ctx, &sync, "origin", &base_ref, 7)
            })
        }

        fn commands(&self) -> Vec<String> {
            let lines = self.console.lines();
            lines
                .into_iter()
                .filter(|line| line.command)
                .map(|line| line.text)
                .collect()
        }

        fn has(&self, oid: &str) -> bool {
            git_command()
                .current_dir(self.repo.path())
                .args(["cat-file", "-e", &format!("{oid}^{{commit}}")])
                .status()
                .unwrap()
                .success()
        }
    }

    fn commit_of(repo: &Path, oid: &str) -> PullRequestCommit {
        let parents = git_in(repo, &["rev-parse", &format!("{oid}^@")]);
        PullRequestCommit {
            oid: oid.to_string(),
            abbreviated_oid: oid[..7].to_string(),
            headline: String::new(),
            author: String::new(),
            committed_date: String::new(),
            parents: parents.split_whitespace().map(str::to_string).collect(),
            url: String::new(),
        }
    }

    fn shown(changes: &[ChangeWithSides]) -> Vec<(&str, &str, &str, &str)> {
        changes
            .iter()
            .map(|change| {
                (
                    change.status.as_str(),
                    change.path.as_str(),
                    change.before.as_str(),
                    change.after.as_str(),
                )
            })
            .collect()
    }

    #[test]
    fn changes_wait_for_both_fetches_and_use_the_merge_base() {
        let harness = Harness::new();
        let (fork, base) = harness.pull_request(|scratch| {
            scratch.commit("feature.txt", b"f\n", "feature");
        });
        let head = harness.head();
        assert!(!harness.has(&head) && !harness.has(&base));
        // Before the fetches the objects are not here.
        assert!(changes(harness.repo.path(), &base, &head, &[], None).is_err());

        harness.fetch("main").unwrap();

        assert!(harness.has(&head) && harness.has(&base));
        let refs = harness.repo.git(&["for-each-ref", "--format=%(refname)"]);
        assert!(!refs.contains("pull"), "{refs}");
        let commands = harness.commands();
        assert_eq!(commands.len(), 2, "{commands:?}");
        assert!(
            commands[0].ends_with(
                "fetch --recurse-submodules=no --progress --prune --end-of-options origin main"
            ),
            "{commands:?}"
        );
        assert!(
            commands[1].ends_with(
                "fetch --recurse-submodules=no --progress --prune --end-of-options origin \
                 refs/pull/7/head:"
            ),
            "{commands:?}"
        );
        let found = changes(harness.repo.path(), &base, &head, &[], None).unwrap();
        // `base.txt` came to the base branch after the fork point: it is not the pull request's.
        assert_eq!(
            shown(&found),
            [("A", "feature.txt", fork.as_str(), head.as_str())]
        );
    }

    #[test]
    fn each_fetch_is_followed_by_the_after_fetch_hook() {
        let harness = Harness::new();
        harness.pull_request(|scratch| {
            scratch.commit("feature.txt", b"f\n", "feature");
        });
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = seen.clone();
        harness.sync.set_after_fetch(Arc::new(
            move |_ctx: &TaskContext, _repo: &Path, remote: &str| {
                record.lock().unwrap().push(remote.to_string());
            },
        ));

        harness.fetch("main").unwrap();

        assert_eq!(*seen.lock().unwrap(), ["origin", "origin"]);
    }

    #[test]
    fn a_base_ref_that_git_would_not_take_as_a_branch_is_refused() {
        let harness = Harness::new();
        for name in [
            "-x",
            "--upload-pack=touch x",
            "@{-1}",
            "a..b",
            "",
            "a b",
            "+main",
        ] {
            let error = harness.fetch(name).unwrap_err();
            assert!(matches!(error, GitError::Io(_)), "{name}: {error:?}");
        }
        assert!(harness
            .commands()
            .iter()
            .all(|command| !command.contains(" fetch ")));
    }

    #[test]
    fn a_failed_fetch_fails_and_the_next_one_fetches_again() {
        let harness = Harness::new();
        // No `refs/pull/7/head` yet: the second fetch fails, after the first one.
        let error = harness.fetch("main").unwrap_err();
        assert!(matches!(error, GitError::Failed { .. }), "{error:?}");
        assert_eq!(harness.commands().len(), 2);

        harness.pull_request(|scratch| {
            scratch.commit("feature.txt", b"f\n", "feature");
        });
        harness.fetch("main").unwrap();
        assert_eq!(harness.commands().len(), 4);
        assert!(harness.has(&harness.head()));
    }

    #[test]
    fn per_commit_changes_follow_241s_before_rule() {
        // Linear: each commit against the one before it, the first against the merge base.
        let harness = Harness::new();
        let (fork, base) = harness.pull_request(|scratch| {
            scratch.commit("one.txt", b"1\n", "one");
            scratch.commit("two.txt", b"2\n", "two");
            scratch.commit("one.txt", b"1 again\n", "three");
        });
        harness.fetch("main").unwrap();
        let repo = harness.repo.path();
        let head = harness.head();
        let oids: Vec<String> =
            git_in(repo, &["rev-list", "--reverse", &format!("{base}..{head}")])
                .lines()
                .map(str::to_string)
                .collect();
        let commits: Vec<PullRequestCommit> = oids.iter().map(|oid| commit_of(repo, oid)).collect();

        let first = changes(repo, &base, &head, &commits, Some(0)).unwrap();
        let third = changes(repo, &base, &head, &commits, Some(2)).unwrap();

        assert_eq!(
            shown(&first),
            [("A", "one.txt", fork.as_str(), oids[0].as_str())]
        );
        assert_eq!(
            shown(&third),
            [("M", "one.txt", oids[1].as_str(), oids[2].as_str())]
        );
        assert!(changes(repo, &base, &head, &commits, Some(3)).is_err());

        // A merge inside the pull request: the merge against its first parent inside it, and a
        // commit of the merged line against its own parent there.
        let harness = Harness::new();
        let (fork, base) = harness.pull_request(|scratch| {
            scratch.commit("left.txt", b"l\n", "left");
            scratch.git(&["checkout", "-q", "-b", "side", "HEAD~1"]);
            scratch.commit("right.txt", b"r\n", "right");
            scratch.commit("right.txt", b"r2\n", "right again");
            scratch.git(&["checkout", "-q", "feature"]);
            scratch.git(&["merge", "-q", "--no-edit", "--no-ff", "side"]);
        });
        harness.fetch("main").unwrap();
        let repo = harness.repo.path();
        let head = harness.head();
        let left = git_in(repo, &["rev-parse", &format!("{head}^1")]);
        let right = git_in(repo, &["rev-parse", &format!("{head}^2")]);
        let right_first = git_in(repo, &["rev-parse", &format!("{right}^1")]);
        // P§6.5's order: the first parent's line, then the merged line, then the merge.
        let commits = vec![
            commit_of(repo, &left),
            commit_of(repo, &right_first),
            commit_of(repo, &right),
            commit_of(repo, &head),
        ];

        let merge = changes(repo, &base, &head, &commits, Some(3)).unwrap();
        let merged_first = changes(repo, &base, &head, &commits, Some(1)).unwrap();
        let merged_second = changes(repo, &base, &head, &commits, Some(2)).unwrap();

        // The merge brought `right.txt` to the first parent's line.
        assert_eq!(
            shown(&merge),
            [("A", "right.txt", left.as_str(), head.as_str())]
        );
        // Its parent is the fork point, outside: the merge base. Not the commit before it.
        assert_eq!(
            shown(&merged_first),
            [("A", "right.txt", fork.as_str(), right_first.as_str())]
        );
        assert_eq!(
            shown(&merged_second),
            [("M", "right.txt", right_first.as_str(), right.as_str())]
        );
    }

    #[test]
    fn a_merge_of_the_base_branch_still_counts_as_linear() {
        let harness = Harness::new();
        let (_, base) = harness.pull_request(|scratch| {
            scratch.commit("one.txt", b"1\n", "one");
            scratch.git(&["checkout", "-q", "main"]);
            scratch.commit("early.txt", b"e\n", "base before the merge");
            scratch.git(&["checkout", "-q", "feature"]);
            scratch.git(&["merge", "-q", "--no-edit", "--no-ff", "main"]);
            scratch.commit("two.txt", b"2\n", "two");
        });
        harness.fetch("main").unwrap();
        let repo = harness.repo.path();
        let head = harness.head();
        let merge = git_in(repo, &["rev-parse", &format!("{head}^1")]);
        let one = git_in(repo, &["rev-parse", &format!("{merge}^1")]);
        let commits = vec![
            commit_of(repo, &one),
            commit_of(repo, &merge),
            commit_of(repo, &head),
        ];

        let found = changes(repo, &base, &head, &commits, Some(1)).unwrap();

        assert_eq!(
            shown(&found),
            [("A", "early.txt", one.as_str(), merge.as_str())]
        );
    }

    #[test]
    fn a_hash_that_is_not_one_reaches_no_git() {
        let harness = Harness::new();
        let repo = harness.repo.path();
        let hash = "1".repeat(40);
        for bad in ["--output=x", "HEAD", "main", &"g".repeat(40)] {
            assert!(changes(repo, bad, &hash, &[], None).is_err(), "{bad}");
            assert!(changes(repo, &hash, bad, &[], None).is_err(), "{bad}");
            assert!(commit_body(repo, bad).is_err(), "{bad}");
        }
        let mut commit = commit_of(repo, harness.repo.git(&["rev-parse", "HEAD"]).trim());
        commit.parents = vec!["--all".to_string()];
        assert!(changes(repo, &hash, &hash, &[commit], Some(0)).is_err());
    }

    #[test]
    fn the_commit_body_is_read_from_git() {
        let repo = TempRepo::new();
        repo.commit(
            "a.txt",
            b"1\n",
            "Subject line\n\nThe body,\nwith <b>two</b> lines.\n",
        );
        let oid = repo.git(&["rev-parse", "HEAD"]).trim().to_string();

        assert_eq!(
            commit_body(repo.path(), &oid).unwrap(),
            "The body,\nwith <b>two</b> lines."
        );
    }

    #[test]
    fn the_cache_keeps_the_fetched_heads_and_the_commits_of_the_same_head() {
        let cache = PullCache::default();
        let repo = Path::new("/repo");
        assert!(!cache.was_fetched(repo, 7, "b", "h"));
        cache.set_fetched(repo, 7, Some(("b".to_string(), "h".to_string())));
        assert!(cache.was_fetched(repo, 7, "b", "h"));
        assert!(!cache.was_fetched(repo, 7, "b", "other"));
        assert!(!cache.was_fetched(repo, 8, "b", "h"));
        assert!(!cache.was_fetched(Path::new("/other"), 7, "b", "h"));
        cache.set_fetched(repo, 7, None);
        assert!(!cache.was_fetched(repo, 7, "b", "h"));

        let commit = |oid: &str| PullRequestCommit {
            oid: oid.to_string(),
            abbreviated_oid: String::new(),
            headline: String::new(),
            author: String::new(),
            committed_date: String::new(),
            parents: vec![],
            url: String::new(),
        };
        cache.set_commits(repo, 7, Arc::new(vec![commit("a"), commit("h")]));
        assert_eq!(cache.commits(repo, 7, "h").unwrap().len(), 2);
        assert!(cache.commits(repo, 7, "a").is_none());
        assert!(cache.commits(repo, 8, "h").is_none());
    }

    #[test]
    fn nothing_here_touches_the_work_tree_or_the_index() {
        let harness = Harness::new();
        harness.repo.commit("a.txt", b"local\n", "local");
        std::fs::write(harness.repo.path().join("a.txt"), "edited\n").unwrap();
        std::fs::write(harness.repo.path().join("new.txt"), "untracked\n").unwrap();
        harness.repo.git(&["add", "new.txt"]);
        let before = harness.repo.git(&["status", "--porcelain=v2"]);
        let index = std::fs::read(harness.repo.path().join(".git/index")).unwrap();
        let (_, base) = harness.pull_request(|scratch| {
            scratch.commit("a.txt", b"theirs\n", "theirs");
        });

        harness.fetch("main").unwrap();
        let head = harness.head();
        changes(harness.repo.path(), &base, &head, &[], None).unwrap();

        assert_eq!(
            std::fs::read(harness.repo.path().join(".git/index")).unwrap(),
            index
        );
        assert_eq!(harness.repo.git(&["status", "--porcelain=v2"]), before);
        assert_eq!(
            std::fs::read_to_string(harness.repo.path().join("a.txt")).unwrap(),
            "edited\n"
        );
    }
}
