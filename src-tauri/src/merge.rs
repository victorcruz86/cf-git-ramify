// `Git > Merge…`: `git merge` with the options of IntelliJ's dialog and what follows its output.
// Ported from `GitMerge`, `GitMergeAction` and `GitMergeDialog`; the counts are those of Update.
use serde::{Deserialize, Serialize};

use crate::branches::{self, Repo};
use crate::commit;
use crate::git::GitError;
use crate::operation;
use crate::preserve::{self, RestoreOutcome, Saved};
use crate::pull;
use crate::rebase;
use crate::task::{GitOutput, TaskContext};
use crate::update::{self, Before, Outcome, Tracked, UpdateResult};

// `GitMergeOption`, in its order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MergeOption {
    NoFf,
    FfOnly,
    Squash,
    Message,
    NoCommit,
    NoVerify,
    AllowUnrelatedHistories,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "reason", rename_all = "camelCase")]
pub enum MergeOutcome {
    Merged {
        files: u32,
        commits: u32,
        range: Option<(String, String)>,
        matching: Option<u32>,
    },
    UpToDate,
    Conflicts,
    LocalChanges {
        files: Vec<String>,
    },
    Untracked {
        files: Vec<String>,
    },
    Unmerged,
    Failed(String),
}

impl MergeOption {
    fn flag(self) -> &'static str {
        match self {
            Self::NoFf => "--no-ff",
            Self::FfOnly => "--ff-only",
            Self::Squash => "--squash",
            Self::Message => "-m",
            Self::NoCommit => "--no-commit",
            Self::NoVerify => "--no-verify",
            Self::AllowUnrelatedHistories => "--allow-unrelated-histories",
        }
    }
}

// `GitSimpleEventDetector`: the git texts that IntelliJ looks for in the output.
const MERGE_CONFLICT: &str = "Automatic merge failed; fix conflicts and then commit the result";
const UNMERGED_FILES: &str = "is not possible because you have unmerged files";
// `rebase.log.commit.editing.action.prohibit.state.rebasing` with `merge`.
const MERGE_DURING_REBASE: &str = "Cannot merge during rebase";
const UP_TO_DATE: [&str; 2] = ["Already up-to-date", "Already up to date"];
const UNMERGED_BRANCHES: [&str; 4] = ["branch", "--format=%(refname)", "--no-color", "--all"];
const HEADS: &str = "refs/heads/";
const REMOTES: &str = "refs/remotes/";
const TAGS: &str = "refs/tags/";
const HEAD: &str = "HEAD";

// `GitMerge.getHandlerProvider`: the options in the chosen order, and `-m` with its text where the
// option is, only if the text is not empty.
fn arguments(options: &[MergeOption], message: Option<&str>) -> Vec<String> {
    let message = message.map(str::trim).filter(|text| !text.is_empty());
    let mut args = vec!["merge".to_string()];
    let mut seen: Vec<MergeOption> = Vec::new();
    for option in options {
        if seen.contains(option) {
            continue;
        }
        seen.push(*option);
        match (option, message) {
            (MergeOption::Message, Some(text)) => {
                args.extend([option.flag().to_string(), text.to_string()]);
            }
            (MergeOption::Message, None) => {}
            _ => args.push(option.flag().to_string()),
        }
    }
    args
}

// `GitMergeAction.handleResult`, without the conflicts dialog or the file tree: what follows a
// conflict is done by the caller. With `--no-commit` and `--squash` there are no received commits, but there are
// files that are already staged; they are counted as in Pull.
pub fn merge(
    repo: &Repo,
    branch: &str,
    options: &[MergeOption],
    message: Option<&str>,
    expected_head: &str,
) -> Result<MergeOutcome, GitError> {
    operation::check_head(repo.path, expected_head)?;
    operation::check_idle(repo.path)?;
    run_merge(repo, branch, options, message)
}

// What `merge` does without checking whether another operation is halfway: the popup lets git say so, or
// `Unmerged` if conflicts remain, as `GitMergeOperation`.
fn run_merge(
    repo: &Repo,
    branch: &str,
    options: &[MergeOption],
    message: Option<&str>,
) -> Result<MergeOutcome, GitError> {
    branches::check_argument(branch, "The branch")?;
    branches::commit_of(repo, branch)?;
    let ctx = repo.ctx;
    if preserve::has_unmerged(ctx)? {
        return Ok(MergeOutcome::Unmerged);
    }
    // Git merges in the middle of a stopped rebase and moves the detached HEAD: the rebase would be left broken.
    if commit::rebase_in_progress(repo.path) {
        return Ok(MergeOutcome::Failed(MERGE_DURING_REBASE.to_string()));
    }
    let tracked = Tracked {
        branch: HEAD.to_string(),
        remote: None,
        target: branch.to_string(),
    };
    let before = Before {
        base: update::merge_base(ctx, &tracked),
        head: ctx.git(&["rev-parse", HEAD])?.stdout.trim().to_string(),
    };
    let mut args = arguments(options, message);
    args.push(branch.to_string());
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match ctx.git_loud(&args) {
        Ok(output) => Ok(merged(ctx, &output, &tracked, before, options)),
        Err(GitError::Failed { stdout, stderr, .. }) => Ok(failure(&stdout, &stderr)),
        Err(error) => Err(error),
    }
}

fn merged(
    ctx: &TaskContext,
    output: &GitOutput,
    tracked: &Tracked,
    before: Before,
    options: &[MergeOption],
) -> MergeOutcome {
    let text = format!("{}\n{}", output.stdout, output.stderr);
    if UP_TO_DATE.iter().any(|message| text.contains(message)) {
        return MergeOutcome::UpToDate;
    }
    let counted = update::with_counts(ctx, Outcome::Success.into(), tracked, before);
    let unfinished = [MergeOption::NoCommit, MergeOption::Squash];
    let files = if counted.commits == 0 && options.iter().any(|option| unfinished.contains(option))
    {
        pull::incoming_files(ctx, &tracked.target)
    } else {
        counted.files
    };
    MergeOutcome::Merged {
        files,
        commits: counted.commits,
        range: counted.range,
        matching: None,
    }
}

// The conflict takes precedence over everything else, as in IntelliJ: the merge is left halfway and it is what
// has to be resolved.
fn failure(stdout: &str, stderr: &str) -> MergeOutcome {
    let text = format!("{stdout}\n{stderr}");
    if text.contains(MERGE_CONFLICT) {
        MergeOutcome::Conflicts
    } else if text.contains(UNMERGED_FILES) {
        MergeOutcome::Unmerged
    } else if text.contains(branches::LOCAL_CHANGES)
        || text.contains("' would be overwritten by merge")
    {
        MergeOutcome::LocalChanges {
            files: branches::overwritten_files(&text),
        }
    } else if text.contains(preserve::UNTRACKED_FILES) {
        MergeOutcome::Untracked {
            files: preserve::listed_files(&text, preserve::UNTRACKED_FILES),
        }
    } else {
        MergeOutcome::Failed(rebase::failure_text(stdout, stderr))
    }
}

// The merge of the branches popup, with what was saved if it was a Smart Merge. `restore` is `None` if
// restoring was not attempted: with conflicts, it is restored after resolving them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchMerge {
    pub outcome: MergeOutcome,
    pub saved: Option<Saved>,
    pub restore: Option<RestoreOutcome>,
}

const MERGE_TITLE: &str = "Merge";

// The reference a popup item stands for. Anything but `local | remote | tag` is rejected before the name is
// looked at; the name goes through the same checks as every other reference of the UI.
fn full_ref(repo: &Repo, name: &str, kind: &str) -> Result<String, GitError> {
    let prefix = match kind {
        "local" => HEADS,
        "remote" => REMOTES,
        "tag" => TAGS,
        other => {
            return Err(branches::invalid(format!(
                "The kind of reference '{other}' is not valid"
            )))
        }
    };
    branches::check_argument(name, "The branch")?;
    let full = format!("{prefix}{name}");
    if !branches::ref_is_valid(repo.ctx, &full)? {
        return Err(branches::invalid(format!("The branch '{name}' is not valid")));
    }
    Ok(full)
}

// `GitMergeBranchAction` and `GitMergeOperation`: `git merge <full ref>` without options. With `smart`, the
// Smart Merge that is offered when local changes would be overwritten.
//
// `GitMergeOperation` merges `branch.getFullName()`: the popup merges the item that was clicked, so a tag
// and a branch with the same name are not mixed up. `kind` is `local`, `remote` or `tag`; `name` is the short
// name (a remote branch includes its remote).
pub fn merge_into(
    repo: &Repo,
    name: &str,
    kind: &str,
    smart: bool,
    expected_head: &str,
) -> Result<BranchMerge, GitError> {
    let full = full_ref(repo, name, kind)?;
    if smart {
        return smart_merge(repo, &full, expected_head);
    }
    operation::check_head(repo.path, expected_head)?;
    Ok(BranchMerge {
        outcome: run_merge(repo, &full, &[], None)?,
        saved: None,
        restore: None,
    })
}

// `GitMergeOperation.doSmartMerge`, with `GitPreservingProcess`: saves, merges and restores. With the merge
// halfway nothing is restored (`execute(myConflictedRepositories::isEmpty)`): whoever resolves brings it
// back afterwards. Once saved, no `?` comes out.
pub fn smart_merge(
    repo: &Repo,
    branch: &str,
    expected_head: &str,
) -> Result<BranchMerge, GitError> {
    operation::check_head(repo.path, expected_head)?;
    branches::check_argument(branch, "The branch")?;
    branches::commit_of(repo, branch)?;
    let unsaved = |outcome| BranchMerge {
        outcome,
        saved: None,
        restore: None,
    };
    if preserve::has_unmerged(repo.ctx)? {
        return Ok(unsaved(MergeOutcome::Unmerged));
    }
    // `stash save` would take `MERGE_HEAD` without saying anything: with something halfway nothing is saved, and git
    // says why it does not merge.
    if update::stopped_midway(repo.path) {
        return run_merge(repo, branch, &[], None).map(unsaved);
    }
    let saved = preserve::save_with_index_copy(repo.ctx, repo.path, repo.data, MERGE_TITLE)?;
    let result = run_merge(repo, branch, &[], None);
    let Some(stash) = saved else {
        return result.map(unsaved);
    };
    if update::stopped_midway(repo.path) {
        let outcome = result.unwrap_or_else(|error| MergeOutcome::Failed(branches::text_of(error)));
        return Ok(BranchMerge {
            outcome,
            saved: Some(stash),
            restore: None,
        });
    }
    let restore = preserve::restore_tracked(repo.ctx, repo.data, &stash);
    let back = matches!(restore, RestoreOutcome::Restored | RestoreOutcome::Gone);
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(error) if back => return Err(error),
        Err(error) => MergeOutcome::Failed(branches::text_of(error)),
    };
    Ok(BranchMerge {
        outcome,
        saved: Some(stash),
        restore: Some(restore),
    })
}

// What a merge with conflicts ended up bringing, once resolved and committed: like `merge`, from the
// HEAD that existed before. The old base comes from that HEAD and from the branch, which have not moved.
pub fn summary(
    ctx: &TaskContext,
    before_head: &str,
    branch: &str,
) -> Result<MergeOutcome, GitError> {
    for (what, revision) in [("The HEAD", before_head), ("The branch", branch)] {
        branches::check_argument(revision, what)?;
        let spec = format!("{revision}^{{commit}}");
        ctx.git(&["rev-parse", "-q", "--verify", &spec])
            .map_err(|_| branches::invalid(format!("Could not find \"{revision}\"")))?;
    }
    let was = Tracked {
        branch: before_head.to_string(),
        remote: None,
        target: branch.to_string(),
    };
    let now = Tracked {
        branch: HEAD.to_string(),
        remote: None,
        target: branch.to_string(),
    };
    let before = Before {
        base: update::merge_base(ctx, &was),
        head: before_head.to_string(),
    };
    let counted = update::with_counts(ctx, Outcome::Success.into(), &now, before);
    // A committed squash receives no commits, but it does change files.
    let files = update::updated_files(ctx, before_head);
    Ok(MergeOutcome::Merged {
        files,
        commits: counted.commits,
        range: counted.range,
        matching: None,
    })
}

// `GitMerge`: with a path filter, how many of the received commits touch them.
pub fn with_matching(ctx: &TaskContext, outcome: MergeOutcome, paths: &[String]) -> MergeOutcome {
    let MergeOutcome::Merged {
        files,
        commits,
        range,
        ..
    } = outcome
    else {
        return outcome;
    };
    let carried = UpdateResult {
        files,
        commits,
        range: range.clone(),
        ..Outcome::Success.into()
    };
    let matching = update::with_matching(ctx, carried, paths).matching;
    MergeOutcome::Merged {
        files,
        commits,
        range,
        matching,
    }
}

// `GitMergeDialog.loadUnmergedBranchesForRoot`: what is not yet in HEAD, with the remote ones as
// `origin/x`. `origin/HEAD` and the like are not branches that can be chosen.
pub fn unmerged_branches(ctx: &TaskContext) -> Result<Vec<String>, GitError> {
    let mut args = UNMERGED_BRANCHES.to_vec();
    args.push("--no-merged");
    let output = ctx.git_loud(&args)?;
    Ok(output
        .stdout
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            match (line.strip_prefix(HEADS), line.strip_prefix(REMOTES)) {
                (Some(local), _) => Some(local),
                (None, Some(remote)) => Some(remote).filter(|name| !name.ends_with("/HEAD")),
                _ => None,
            }
        })
        .map(str::to_string)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::RepoHarness;
    use std::process::Command;
    use tempfile::TempDir;

    // `side` adds `b.txt` and `c.txt` on top of `base`, and `main` stays at `base`: it fast-forwards.
    fn ahead() -> RepoHarness {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"base\n", "base");
        harness.git(&["checkout", "-q", "-b", "side"]);
        harness.repo.commit("b.txt", b"b\n", "one");
        harness.repo.commit("c.txt", b"c\n", "two");
        harness.git(&["checkout", "-q", "main"]);
        harness
    }

    // With a commit of its own on `main`, merging `side` creates a merge commit.
    fn diverging() -> RepoHarness {
        let harness = ahead();
        harness.repo.commit("m.txt", b"m\n", "main");
        harness
    }

    fn merge_side(
        harness: &RepoHarness,
        options: &[MergeOption],
        message: Option<&str>,
    ) -> Result<MergeOutcome, GitError> {
        let (head, options) = (harness.head(), options.to_vec());
        let message = message.map(str::to_string);
        harness.run(move |repo| merge(repo, "side", &options, message.as_deref(), &head))
    }

    fn merged(files: u32, commits: u32) -> impl Fn(&Result<MergeOutcome, GitError>) -> bool {
        move |outcome| {
            matches!(outcome, Ok(MergeOutcome::Merged { files: f, commits: c, .. })
                if *f == files && *c == commits)
        }
    }

    #[test]
    fn merge_with_each_option_launches_the_intellij_command() {
        use MergeOption::*;
        // The order of the options is the dialog's, and `-m` carries its text right behind it.
        let cases: [(bool, &[MergeOption], Option<&str>, &str); 9] = [
            (true, &[NoFf], None, "merge --no-ff side"),
            (true, &[FfOnly], None, "merge --ff-only side"),
            (false, &[Squash], None, "merge --squash side"),
            (false, &[Message], Some("hello"), "merge -m hello side"),
            (false, &[NoCommit], None, "merge --no-commit side"),
            (false, &[NoVerify], None, "merge --no-verify side"),
            (
                false,
                &[AllowUnrelatedHistories],
                None,
                "merge --allow-unrelated-histories side",
            ),
            (
                false,
                &[NoVerify, NoFf, Message],
                Some("  joined  "),
                "merge --no-verify --no-ff -m joined side",
            ),
            (false, &[], None, "merge side"),
        ];
        for (fast, options, message, expected) in cases {
            let harness = if fast { ahead() } else { diverging() };

            let outcome = merge_side(&harness, options, message);

            assert!(
                matches!(outcome, Ok(MergeOutcome::Merged { .. })),
                "{expected}: {outcome:?}"
            );
            assert!(harness.ran(expected), "{expected}");
        }
    }

    #[test]
    fn merge_with_allow_unrelated_histories_joins_histories_without_a_common_ancestor() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"base\n", "base");
        harness.git(&["checkout", "-q", "--orphan", "side"]);
        harness.git(&["rm", "-rf", "-q", "."]);
        harness.repo.commit("o.txt", b"o\n", "orphan");
        harness.git(&["checkout", "-q", "main"]);

        let refused = merge_side(&harness, &[], None);
        assert!(
            !matches!(refused, Ok(MergeOutcome::Merged { .. })),
            "{refused:?}"
        );

        let joined = merge_side(&harness, &[MergeOption::AllowUnrelatedHistories], None);
        assert!(
            matches!(joined, Ok(MergeOutcome::Merged { .. })),
            "{joined:?}"
        );
        assert!(harness.ran("merge --allow-unrelated-histories side"));
    }

    #[test]
    fn merge_without_text_has_no_m_and_text_without_the_option_neither() {
        use MergeOption::*;
        let blank = diverging();
        assert!(merge_side(&blank, &[Message], Some("   ")).is_ok());
        assert!(blank.ran("merge side"));

        let loose = diverging();
        assert!(merge_side(&loose, &[NoVerify], Some("loose")).is_ok());
        assert!(loose.ran("merge --no-verify side"));
    }

    #[test]
    fn merge_with_message_uses_it_in_the_commit() {
        let harness = diverging();

        let outcome = merge_side(&harness, &[MergeOption::Message], Some("my message"));

        assert!(outcome.is_ok(), "{outcome:?}");
        assert_eq!(harness.git(&["log", "-1", "--format=%s"]), "my message\n");
    }

    #[test]
    fn fast_and_committing_merges_count_files_and_commits() {
        let fast = ahead();
        let tip = fast.git(&["rev-parse", "side"]).trim().to_string();
        let outcome = merge_side(&fast, &[], None);
        assert!(merged(2, 2)(&outcome), "{outcome:?}");
        assert_eq!(fast.head(), tip);
        let base = fast.git(&["rev-parse", "HEAD~2"]).trim().to_string();
        // The range for `View commits`: from the previous base to the received tip.
        assert!(
            matches!(&outcome, Ok(MergeOutcome::Merged { range: Some(r), .. })
            if *r == (base.clone(), tip.clone()))
        );

        let committed = diverging();
        let outcome = merge_side(&committed, &[], None);
        assert!(merged(2, 2)(&outcome), "{outcome:?}");
        assert_eq!(
            committed
                .git(&["rev-list", "--parents", "-1", "HEAD"])
                .split(' ')
                .count(),
            3
        );
    }

    #[test]
    fn merge_without_commit_or_squash_counts_the_files_it_brings() {
        for option in [MergeOption::NoCommit, MergeOption::Squash] {
            let harness = diverging();
            let before = harness.head();

            let outcome = merge_side(&harness, &[option], None);

            // It receives no commits, but leaves the files of `side` staged.
            assert!(merged(2, 0)(&outcome), "{option:?}: {outcome:?}");
            assert_eq!(harness.head(), before);
        }
    }

    #[test]
    fn merging_what_is_already_merged_gives_up_to_date() {
        let harness = ahead();
        assert!(merge_side(&harness, &[], None).is_ok());

        let outcome = merge_side(&harness, &[], None);

        assert_eq!(outcome, Ok(MergeOutcome::UpToDate));
    }

    #[test]
    fn merge_with_conflict_gives_conflicts_and_leaves_merge_head() {
        let harness = RepoHarness::new();
        harness.diverged();

        let outcome = merge_side(&harness, &[], None);

        assert_eq!(outcome, Ok(MergeOutcome::Conflicts));
        assert!(harness.in_git_dir("MERGE_HEAD"));
    }

    #[test]
    fn merge_squash_with_conflict_gives_conflicts() {
        let harness = RepoHarness::new();
        harness.diverged();

        let outcome = merge_side(&harness, &[MergeOption::Squash], None);

        assert_eq!(outcome, Ok(MergeOutcome::Conflicts));
        assert!(!harness.in_git_dir("MERGE_HEAD"));
    }

    #[test]
    fn merge_with_changes_that_would_be_overwritten_gives_local_changes() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"base\n", "base");
        harness.git(&["checkout", "-q", "-b", "side"]);
        harness.repo.commit("a.txt", b"side\n", "side");
        harness.git(&["checkout", "-q", "main"]);
        harness.write("a.txt", "mine\n");

        let outcome = merge_side(&harness, &[], None);

        assert_eq!(
            outcome,
            Ok(MergeOutcome::LocalChanges {
                files: vec!["a.txt".to_string()]
            })
        );
        assert_eq!(harness.read("a.txt"), "mine\n");
    }

    #[test]
    fn merge_with_untracked_gives_untracked() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"base\n", "base");
        harness.git(&["checkout", "-q", "-b", "side"]);
        harness.repo.commit("new.txt", b"from side\n", "side");
        harness.git(&["checkout", "-q", "main"]);
        harness.write("new.txt", "mine\n");

        let outcome = merge_side(&harness, &[], None);

        assert_eq!(
            outcome,
            Ok(MergeOutcome::Untracked {
                files: vec!["new.txt".to_string()]
            })
        );
        assert_eq!(harness.read("new.txt"), "mine\n");
    }

    #[test]
    fn merge_with_unresolved_files_gives_unmerged_and_changes_nothing() {
        let harness = RepoHarness::new();
        harness.diverged();
        harness.git_fails(&["merge", "side"]);
        harness.git(&["merge", "--abort"]);
        harness.git_fails(&["merge", "side"]);
        // Without `MERGE_HEAD` but with unresolved paths, as after a `stash pop` with conflicts.
        std::fs::remove_file(harness.path().join(".git/MERGE_HEAD")).unwrap();
        let head = harness.head();

        let outcome = merge_side(&harness, &[], None);

        assert_eq!(outcome, Ok(MergeOutcome::Unmerged));
        assert_eq!(harness.head(), head);
    }

    #[test]
    fn merge_of_branch_with_leading_dash_is_rejected() {
        let harness = ahead();
        let head = harness.head();

        let results = harness.run(move |repo| {
            ["-x", "--no-ff", "", "does-not-exist"]
                .map(|branch| merge(repo, branch, &[], None, &head).is_err())
        });

        assert_eq!(results, [true; 4]);
        assert!(!harness.ran("merge -x"));
        assert!(!harness.ran("merge --no-ff"));
        assert!(!harness.ran("merge does-not-exist"));
    }

    #[test]
    fn merge_and_rebase_check_head() {
        let harness = ahead();
        let before = harness.head();
        let stale = "0".repeat(40);

        let (merged, rebased) = harness.run(move |repo| {
            let params = crate::rebase::RebaseParams {
                upstream: Some("side".to_string()),
                ..Default::default()
            };
            (
                merge(repo, "side", &[], None, &stale),
                crate::rebase::rebase(repo, &params, &stale),
            )
        });

        let changed = GitError::Io("Repository has already been changed".into());
        assert_eq!(merged, Err(changed.clone()));
        assert_eq!(rebased, Err(changed));
        assert_eq!(harness.head(), before);
        assert!(!harness.ran("merge side"));
        assert!(!harness.ran("rebase side"));
    }

    // Git lets you do a merge in the middle of a stopped rebase: the dialog did not see it that way.
    #[test]
    fn merge_and_rebase_with_another_operation_halfway_touch_nothing() {
        let harness = RepoHarness::new();
        harness.diverged();
        harness.git(&["checkout", "-q", "side"]);
        harness.git_fails(&["rebase", "main"]);
        let head = harness.head();
        let other = harness.head();

        let (merged, rebased) = harness.run(move |repo| {
            let params = crate::rebase::RebaseParams {
                upstream: Some("main".to_string()),
                ..Default::default()
            };
            (
                merge(repo, "main", &[], None, &head),
                crate::rebase::rebase(repo, &params, &other),
            )
        });

        let changed = GitError::Io("Repository has already been changed".into());
        assert_eq!(merged, Err(changed.clone()));
        assert_eq!(rebased, Err(changed));
        assert!(!harness.ran("merge main"));
        assert!(!harness.ran("rebase main"));
    }

    fn resolve_and_commit(harness: &RepoHarness) {
        harness.write("a.txt", "resolved\n");
        harness.git(&["add", "a.txt"]);
        harness.run(crate::commit::merge_commit).unwrap();
    }

    #[test]
    fn the_summary_of_a_resolved_conflicted_merge_counts_files_and_commits() {
        let harness = RepoHarness::new();
        harness.diverged();
        let before = harness.head();
        assert_eq!(merge_side(&harness, &[], None), Ok(MergeOutcome::Conflicts));
        resolve_and_commit(&harness);
        let tip = harness.git(&["rev-parse", "side"]).trim().to_string();
        let base = harness.git(&["rev-parse", "main~1^"]).trim().to_string();

        let outcome = harness.run(move |repo| summary(repo.ctx, &before, "side"));

        assert!(
            matches!(&outcome, Ok(MergeOutcome::Merged { files: 1, commits: 1, range: Some(r), .. })
                if *r == (base.clone(), tip.clone())),
            "{outcome:?}"
        );
    }

    #[test]
    fn the_summary_of_a_conflicted_squash_counts_files_without_commits() {
        let harness = RepoHarness::new();
        harness.diverged();
        let before = harness.head();
        let options = [MergeOption::Squash];
        assert_eq!(
            merge_side(&harness, &options, None),
            Ok(MergeOutcome::Conflicts)
        );
        resolve_and_commit(&harness);

        let outcome = harness.run(move |repo| summary(repo.ctx, &before, "side"));

        assert!(merged(1, 0)(&outcome), "{outcome:?}");
    }

    #[test]
    fn the_summary_of_a_missing_or_dash_prefixed_branch_is_rejected() {
        let harness = ahead();
        let head = harness.head();

        let results = harness.run(move |repo| {
            ["-x", "does-not-exist"].map(|branch| summary(repo.ctx, &head, branch).is_err())
        });

        assert_eq!(results, [true; 2]);
    }

    // `a.txt` has three lines: `side` changes the first and the user the third. `main` adds `m.txt`.
    fn overwritten() -> RepoHarness {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"1\n2\n3\n", "base");
        harness.repo.commit("b.txt", b"base\n", "b");
        harness.git(&["checkout", "-q", "-b", "side"]);
        harness.repo.commit("a.txt", b"one\n2\n3\n", "side");
        harness.git(&["checkout", "-q", "main"]);
        harness.repo.commit("m.txt", b"m\n", "main");
        harness.write("a.txt", "1\n2\nthree\n");
        harness
    }

    fn merge_into_side(harness: &RepoHarness, smart: bool) -> Result<BranchMerge, GitError> {
        let head = harness.head();
        harness.run(move |repo| merge_into(repo, "side", "local", smart, &head))
    }

    fn stash_count(harness: &RepoHarness) -> usize {
        harness
            .git(&["stash", "list", "--format=%H"])
            .lines()
            .count()
    }

    #[test]
    fn merge_into_is_merge_without_arguments() {
        let harness = diverging();

        let merged = merge_into_side(&harness, false).unwrap();

        assert!(
            matches!(merged.outcome, MergeOutcome::Merged { .. }),
            "{merged:?}"
        );
        assert_eq!(merged.saved, None);
        assert!(harness.ran(" merge refs/heads/side"));
        assert_eq!(
            harness
                .git(&["rev-list", "--parents", "-1", "HEAD"])
                .split(' ')
                .count(),
            3
        );
    }

    // A branch `x` one commit ahead of `base`, and a tag `x` on another commit.
    fn branch_and_tag_named_x() -> (RepoHarness, String, String) {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"base\n", "base");
        harness.git(&["checkout", "-q", "-b", "x"]);
        harness.repo.commit("branch.txt", b"b\n", "on the branch");
        let branch = harness.head();
        harness.git(&["checkout", "-q", "main"]);
        harness.git(&["checkout", "-q", "-b", "elsewhere"]);
        harness.repo.commit("tag.txt", b"t\n", "on the tag");
        let tag = harness.head();
        harness.git(&["tag", "x"]);
        harness.git(&["checkout", "-q", "main"]);
        (harness, branch, tag)
    }

    fn merge_x(harness: &RepoHarness, kind: &'static str) -> Result<BranchMerge, GitError> {
        let head = harness.head();
        harness.run(move |repo| merge_into(repo, "x", kind, false, &head))
    }

    #[test]
    fn popup_merge_of_a_tag_with_a_branchs_name_merges_the_tag() {
        let (harness, branch, tag) = branch_and_tag_named_x();

        let merged = merge_x(&harness, "tag").unwrap();

        assert!(
            matches!(merged.outcome, MergeOutcome::Merged { .. }),
            "{merged:?}"
        );
        assert!(harness.ran(" merge refs/tags/x"));
        assert_eq!(harness.head(), tag);
        assert_ne!(harness.head(), branch);
    }

    #[test]
    fn popup_merge_of_a_branch_with_a_tags_name_merges_the_branch() {
        let (harness, branch, tag) = branch_and_tag_named_x();

        merge_x(&harness, "local").unwrap();

        assert!(harness.ran(" merge refs/heads/x"));
        assert_eq!(harness.head(), branch);
        assert_ne!(harness.head(), tag);
    }

    #[test]
    fn popup_merge_of_a_remote_branch_uses_refs_remotes() {
        let harness = diverging();
        let side = harness.git(&["rev-parse", "side"]).trim().to_string();
        harness.git(&["update-ref", "refs/remotes/origin/side", &side]);
        harness.git(&["branch", "-q", "-D", "side"]);
        let head = harness.head();

        let merged = harness
            .run(move |repo| merge_into(repo, "origin/side", "remote", false, &head))
            .unwrap();

        assert!(
            matches!(merged.outcome, MergeOutcome::Merged { .. }),
            "{merged:?}"
        );
        assert!(harness.ran(" merge refs/remotes/origin/side"));
    }

    #[test]
    fn an_unknown_ref_kind_is_rejected() {
        let harness = diverging();
        let before = harness.head();

        for kind in ["", "head", "refs/heads/", "--abort", "Local"] {
            let head = harness.head();
            let rejected = harness.run(move |repo| merge_into(repo, "side", kind, false, &head));
            assert!(matches!(rejected, Err(GitError::Io(_))), "{kind}: {rejected:?}");
        }
        let head = harness.head();
        let smart = harness.run(move |repo| merge_into(repo, "side", "bogus", true, &head));
        assert!(matches!(smart, Err(GitError::Io(_))), "{smart:?}");
        assert_eq!(harness.head(), before);
        assert!(!harness.ran(" merge refs/heads/side"));
    }

    #[test]
    fn merge_into_with_changes_that_would_be_overwritten_touches_nothing() {
        let harness = overwritten();

        let merged = merge_into_side(&harness, false).unwrap();

        assert_eq!(
            merged.outcome,
            MergeOutcome::LocalChanges {
                files: vec!["a.txt".to_string()]
            }
        );
        assert_eq!(harness.read("a.txt"), "1\n2\nthree\n");
        assert_eq!(stash_count(&harness), 0);
    }

    #[test]
    fn smart_merge_saves_merges_and_restores() {
        let harness = overwritten();

        let merged = merge_into_side(&harness, true).unwrap();

        assert!(
            matches!(merged.outcome, MergeOutcome::Merged { .. }),
            "{merged:?}"
        );
        let saved = merged.saved.unwrap();
        assert!(
            saved
                .message
                .starts_with("Uncommitted changes before Merge at "),
            "{}",
            saved.message
        );
        assert_eq!(merged.restore, Some(RestoreOutcome::Restored));
        assert_eq!(harness.read("a.txt"), "one\n2\nthree\n");
        assert_eq!(stash_count(&harness), 0);
    }

    #[test]
    fn smart_merge_with_conflict_restores_after_resolving() {
        let harness = overwritten();
        harness.git(&["stash", "-q"]);
        harness.git(&["checkout", "-q", "side"]);
        harness.repo.commit("b.txt", b"side\n", "clashes");
        harness.git(&["checkout", "-q", "main"]);
        harness.repo.commit("b.txt", b"main\n", "clashes on main");
        harness.git(&["stash", "pop", "-q"]);

        let merged = merge_into_side(&harness, true).unwrap();

        assert_eq!(merged.outcome, MergeOutcome::Conflicts);
        assert_eq!(merged.restore, None);
        assert_eq!(stash_count(&harness), 1);
        let saved = merged.saved.unwrap();
        harness.write("b.txt", "resolved\n");
        harness.git(&["add", "b.txt"]);
        harness.run(crate::commit::merge_commit).unwrap();

        let restore = harness.run(move |repo| update::restore_requested(repo, &saved));

        assert_eq!(restore, RestoreOutcome::Restored);
        assert_eq!(harness.read("a.txt"), "one\n2\nthree\n");
        assert_eq!(stash_count(&harness), 0);
    }

    // A Smart Merge with conflicts, resolved and with the commit rejected by a hook: `MERGE_HEAD`
    // remains and no unresolved paths are left. Returns what was saved.
    fn merge_resolved_but_rejected(harness: &RepoHarness) -> Saved {
        harness.git(&["stash", "-q"]);
        harness.git(&["checkout", "-q", "side"]);
        harness.repo.commit("b.txt", b"side\n", "clashes");
        harness.git(&["checkout", "-q", "main"]);
        harness.repo.commit("b.txt", b"main\n", "clashes on main");
        harness.git(&["stash", "pop", "-q"]);
        let merged = merge_into_side(harness, true).unwrap();
        assert_eq!(merged.outcome, MergeOutcome::Conflicts);
        harness.write("b.txt", "resolved\n");
        harness.git(&["add", "b.txt"]);
        let hook = harness.path().join(".git/hooks/pre-commit");
        std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
        std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        assert!(harness.run(crate::commit::merge_commit).is_err());
        std::fs::remove_file(&hook).unwrap();
        assert!(harness.in_git_dir("MERGE_HEAD"));
        assert_eq!(harness.git(&["ls-files", "--unmerged"]), "");
        merged.saved.unwrap()
    }

    // What was saved is not mixed with the result of the unconfirmed merge: the entry remains, with
    // `Restore Changes`, by both routes.
    fn refused_while_merging(harness: &RepoHarness, saved: &Saved) {
        let refused = RestoreOutcome::Failed("Finish or abort first".to_string());
        let direct = {
            let saved = saved.clone();
            harness.run(move |repo| preserve::restore(repo.ctx, &saved))
        };
        assert_eq!(direct, Ok(refused.clone()));
        let requested = {
            let saved = saved.clone();
            harness.run(move |repo| update::restore_requested(repo, &saved))
        };
        assert_eq!(requested, refused);
        assert_eq!(harness.git(&["rev-parse", "stash@{0}"]).trim(), saved.sha);
        assert_eq!(harness.read("a.txt"), "one\n2\n3\n");
        let pending = preserve::pending(harness.path(), harness.data.path()).unwrap();
        assert_eq!(pending.len(), 1);
    }

    #[test]
    fn with_the_merge_commit_rejected_nothing_is_restored_until_it_is_confirmed() {
        let harness = overwritten();
        let saved = merge_resolved_but_rejected(&harness);

        refused_while_merging(&harness, &saved);

        harness.run(crate::commit::merge_commit).unwrap();
        let restore = harness.run(move |repo| update::restore_requested(repo, &saved));
        assert_eq!(restore, RestoreOutcome::Restored);
        assert_eq!(harness.read("a.txt"), "one\n2\nthree\n");
        assert_eq!(stash_count(&harness), 0);
    }

    #[test]
    fn with_the_merge_commit_rejected_changes_are_restored_after_aborting_it() {
        let harness = overwritten();
        let saved = merge_resolved_but_rejected(&harness);

        refused_while_merging(&harness, &saved);

        harness.git(&["merge", "--abort"]);
        let restore = harness.run(move |repo| update::restore_requested(repo, &saved));
        assert_eq!(restore, RestoreOutcome::Restored);
        assert_eq!(harness.read("a.txt"), "1\n2\nthree\n");
        assert_eq!(stash_count(&harness), 0);
    }

    #[test]
    fn with_a_rebase_stopped_at_edit_nothing_is_restored() {
        let harness = rebasing("edit");
        let before = harness.read("a.txt");
        harness.write("a.txt", "local\n");
        let saved = harness
            .run(|repo| preserve::save(repo.ctx, "test"))
            .unwrap()
            .unwrap();

        let direct = {
            let saved = saved.clone();
            harness.run(move |repo| preserve::restore(repo.ctx, &saved))
        };

        assert_eq!(
            direct,
            Ok(RestoreOutcome::Failed("Finish or abort first".to_string()))
        );
        assert_eq!(harness.git(&["rev-parse", "stash@{0}"]).trim(), saved.sha);
        assert_eq!(harness.read("a.txt"), before);
    }

    // `side` clashes with `main` and the merge is left halfway; `other` branches off `side`.
    fn unfinished() -> RepoHarness {
        let harness = RepoHarness::new();
        harness.diverged();
        harness.git(&["branch", "other", "side"]);
        harness.git_fails(&["merge", "side"]);
        harness
    }

    fn merge_into_other(harness: &RepoHarness, smart: bool) -> Result<BranchMerge, GitError> {
        let head = harness.head();
        harness.run(move |repo| merge_into(repo, "other", "local", smart, &head))
    }

    // `fatalUnmergedFilesError`: `Cannot merge because of unmerged files`, not `Repository has already
    // been changed`.
    #[test]
    fn merge_into_with_a_halfway_merge_with_conflicts_gives_unmerged() {
        let harness = unfinished();

        for smart in [false, true] {
            let merged = merge_into_other(&harness, smart).unwrap();

            assert_eq!(merged.outcome, MergeOutcome::Unmerged, "smart: {smart}");
            assert_eq!(merged.saved, None);
        }
        assert!(!harness.ran("merge other"));
        assert_eq!(stash_count(&harness), 0);
        assert!(harness.in_git_dir("MERGE_HEAD"));
    }

    // Resolved but not committed: git says so, and Smart Merge saves nothing (`stash` would take
    // `MERGE_HEAD`).
    #[test]
    fn merge_into_with_a_resolved_unconfirmed_merge_gives_gits_text() {
        let harness = unfinished();
        harness.write("a.txt", "resolved\n");
        harness.git(&["add", "a.txt"]);
        harness.write("b.txt", "local\n");

        for smart in [false, true] {
            let merged = merge_into_other(&harness, smart).unwrap();

            assert!(
                matches!(&merged.outcome, MergeOutcome::Failed(text) if text.contains("MERGE_HEAD exists")),
                "smart: {smart}: {merged:?}"
            );
            assert_eq!(merged.saved, None);
        }
        assert_eq!(stash_count(&harness), 0);
        assert!(harness.in_git_dir("MERGE_HEAD"));
        assert_eq!(harness.read("b.txt"), "local\n");
    }

    // `side` has two commits (`clashes` touches `a.txt`, `follows` adds `b.txt`) and is rebased onto `main`,
    // stopped at an `edit` or with the conflict resolved but not continued; `other` branches off `main` with
    // `o.txt`. Git lets you do `merge other` in the middle of the stopped rebase.
    fn rebasing(stop: &str) -> RepoHarness {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"base\n", "base");
        harness.git(&["checkout", "-q", "-b", "side"]);
        harness.repo.commit("a.txt", b"side\n", "clashes");
        harness.repo.commit("b.txt", b"b\n", "follows");
        harness.git(&["checkout", "-q", "main"]);
        harness.repo.commit("a.txt", b"main\n", "main");
        harness.git(&["checkout", "-q", "-b", "other"]);
        harness.repo.commit("o.txt", b"o\n", "other");
        harness.git(&["checkout", "-q", "side"]);
        if stop == "edit" {
            // Stopped at `edit` on the first commit of `side`, with a clean index.
            let output = std::process::Command::new("git")
                .current_dir(harness.path())
                .args(["rebase", "-i", "-X", "theirs", "main"])
                .env("GIT_SEQUENCE_EDITOR", "sed -i.bak '1s/^pick/edit/'")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        } else {
            harness.git_fails(&["rebase", "main"]);
            harness.write("a.txt", "resolved\n");
            harness.git(&["add", "a.txt"]);
        }
        assert!(harness.in_git_dir("rebase-merge"));
        harness
    }

    fn untouched_by_merge(harness: &RepoHarness) {
        let head = harness.head();
        let status = harness.git(&["status", "--porcelain"]);
        for smart in [false, true] {
            let merged = merge_into_other(harness, smart).unwrap();

            assert_eq!(
                merged.outcome,
                MergeOutcome::Failed("Cannot merge during rebase".to_string()),
                "smart: {smart}"
            );
            assert_eq!(merged.saved, None);
        }
        assert_eq!(harness.head(), head);
        assert_eq!(harness.git(&["status", "--porcelain"]), status);
        assert!(harness.in_git_dir("rebase-merge"));
        assert!(!harness.path().join("o.txt").exists());
        assert_eq!(stash_count(harness), 0);
        assert!(!harness.ran("merge other"));
    }

    #[test]
    fn merge_into_during_a_rebase_stopped_at_edit_touches_nothing() {
        let harness = rebasing("edit");
        assert_eq!(harness.git(&["ls-files", "--unmerged"]), "");

        untouched_by_merge(&harness);
    }

    #[test]
    fn merge_into_during_a_resolved_rebase_not_continued_touches_nothing() {
        let harness = rebasing("resolved");
        assert_eq!(harness.git(&["ls-files", "--unmerged"]), "");

        untouched_by_merge(&harness);
    }

    // With the rebase stopped at conflicts, IntelliJ's text for unresolved paths takes precedence.
    #[test]
    fn merge_into_during_a_rebase_with_conflicts_gives_unmerged() {
        let harness = RepoHarness::new();
        harness.diverged();
        harness.git(&["branch", "other", "main"]);
        harness.git(&["checkout", "-q", "side"]);
        harness.git_fails(&["rebase", "main"]);

        for smart in [false, true] {
            let merged = merge_into_other(&harness, smart).unwrap();
            assert_eq!(merged.outcome, MergeOutcome::Unmerged, "smart: {smart}");
        }
        assert!(harness.in_git_dir("rebase-merge"));
        assert_eq!(stash_count(&harness), 0);
    }

    // What a Smart Merge saved and then stayed with unresolved conflicts: restoring it applies
    // nothing, the entry remains and the notice offers `Restore Changes`.
    #[test]
    fn restoring_with_the_merge_halfway_applies_nothing_and_keeps_the_entry() {
        let harness = overwritten();
        harness.git(&["stash", "-q"]);
        harness.git(&["checkout", "-q", "side"]);
        harness.repo.commit("b.txt", b"side\n", "clashes");
        harness.git(&["checkout", "-q", "main"]);
        harness.repo.commit("b.txt", b"main\n", "clashes on main");
        harness.git(&["stash", "pop", "-q"]);
        let merged = merge_into_side(&harness, true).unwrap();
        assert_eq!(merged.outcome, MergeOutcome::Conflicts);
        let saved = merged.saved.unwrap();

        let requested = {
            let saved = saved.clone();
            harness.run(move |repo| update::restore_requested(repo, &saved))
        };
        assert_eq!(
            requested,
            RestoreOutcome::Failed("Finish or abort first".to_string())
        );
        assert_eq!(harness.git(&["rev-parse", "stash@{0}"]).trim(), saved.sha);
        assert_eq!(
            preserve::pending(harness.path(), harness.data.path())
                .unwrap()
                .len(),
            1
        );

        // And even if it reached git: `pop` refuses and the entry stays.
        let direct = {
            let saved = saved.clone();
            harness.run(move |repo| preserve::restore(repo.ctx, &saved))
        };
        assert_eq!(
            direct,
            Ok(RestoreOutcome::Failed("Finish or abort first".to_string()))
        );
        assert_eq!(harness.git(&["rev-parse", "stash@{0}"]).trim(), saved.sha);
        assert_eq!(harness.read("a.txt"), "one\n2\n3\n");
    }

    #[test]
    fn smart_merge_checks_head_and_does_not_save_with_something_halfway() {
        let harness = overwritten();
        let stale = "0".repeat(40);

        let stale_result = harness.run(move |repo| smart_merge(repo, "side", &stale));

        assert_eq!(
            stale_result,
            Err(GitError::Io("Repository has already been changed".into()))
        );
        assert_eq!(stash_count(&harness), 0);
        assert_eq!(harness.read("a.txt"), "1\n2\nthree\n");
    }

    // `origin` has `main` and `remote_branch`; locally, `side` and `done` (this one already merged into `main`).
    #[test]
    fn no_merged_lists_local_and_remote_branches() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"base\n", "base");
        harness.git(&["branch", "done"]);
        harness.git(&["checkout", "-q", "-b", "side"]);
        harness.repo.commit("b.txt", b"b\n", "side");
        harness.git(&["checkout", "-q", "main"]);
        let origin = TempDir::new().unwrap();
        let init = Command::new("git")
            .args(["init", "-q", "--bare"])
            .arg(origin.path())
            .status()
            .unwrap();
        assert!(init.success());
        let url = origin.path().to_str().unwrap();
        harness.git(&["remote", "add", "origin", url]);
        harness.git(&["push", "-q", "origin", "main", "side:remote_branch"]);
        harness.git(&["fetch", "-q", "origin"]);
        // An alias like `origin/HEAD` is not a branch that can be chosen.
        harness.git(&[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/remote_branch",
        ]);

        let names = harness.run(|repo| unmerged_branches(repo.ctx));

        assert_eq!(
            names,
            Ok(vec!["side".to_string(), "origin/remote_branch".to_string()])
        );
        assert!(harness.ran("branch --format=%(refname) --no-color --all --no-merged"));
    }

    // The interface reads these shapes: `kind` and, if it carries data, `reason`.
    #[test]
    fn the_result_travels_as_the_interface_expects() {
        let json = |outcome: MergeOutcome| serde_json::to_value(outcome).unwrap();

        assert_eq!(
            json(MergeOutcome::Merged {
                files: 2,
                commits: 1,
                range: Some(("a".into(), "b".into())),
                matching: None
            }),
            serde_json::json!({ "kind": "merged", "reason": {
                "files": 2, "commits": 1, "range": ["a", "b"], "matching": null
            } })
        );
        assert_eq!(
            json(MergeOutcome::UpToDate),
            serde_json::json!({ "kind": "upToDate" })
        );
        assert_eq!(
            json(MergeOutcome::LocalChanges {
                files: vec!["a".into()]
            }),
            serde_json::json!({ "kind": "localChanges", "reason": { "files": ["a"] } })
        );
        assert_eq!(
            json(MergeOutcome::Failed("bad".into())),
            serde_json::json!({ "kind": "failed", "reason": "bad" })
        );
        let options: Vec<MergeOption> =
            serde_json::from_str(r#"["noFf","ffOnly","noVerify","allowUnrelatedHistories"]"#)
                .unwrap();
        assert_eq!(
            options,
            [
                MergeOption::NoFf,
                MergeOption::FfOnly,
                MergeOption::NoVerify,
                MergeOption::AllowUnrelatedHistories
            ]
        );
    }
}
