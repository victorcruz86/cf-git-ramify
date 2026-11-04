// The git operation that is halfway done (merge, rebase, cherry-pick, revert) and how to abort it.
// The detection order and the widget text are those of `GitRepositoryReader` and
// `GitBranchUtil.getDisplayableBranchText`.
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::branches::{self, Repo};
use crate::git::{self, GitError};
use crate::rebase;
use crate::snapshots;
use crate::update::Aborted;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Operation {
    Merging,
    Rebasing,
    CherryPicking,
    Reverting,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoState {
    pub operation: Option<Operation>,
    pub branch: Option<String>,
    // The text of the branch widget.
    pub label: String,
    // The full HEAD, or empty if the repository has no commits. The dialogs return it to check that
    // nothing changed while they were open.
    pub head: String,
}

const MERGE_HEAD: &str = "MERGE_HEAD";
const REBASE_DIRS: [&str; 2] = ["rebase-apply", "rebase-merge"];
const CHERRY_PICK_HEAD: &str = "CHERRY_PICK_HEAD";
const REVERT_HEAD: &str = "REVERT_HEAD";
const HEAD_NAME: &str = "head-name";
const HEAD: &str = "HEAD";
const HEADS: &str = "refs/heads/";
// `DvcsUtil.getShortHash`.
const SHORT_HASH: usize = 8;
// `git.status.bar.widget.text.unknown`.
const UNKNOWN: &str = "<unknown>";
const REPOSITORY_CHANGED: &str = "Repository has already been changed";
const MERGE_ABORT: &str = "Merge --abort";
const CHERRY_PICK_ABORT: &str = "Cherry-pick --abort";
const REVERT_ABORT: &str = "Revert --abort";

// The git directory of this working tree: in a worktree, `MERGE_HEAD` and friends live there and
// not in the common one.
fn git_dir(repo: &Path) -> Result<PathBuf, GitError> {
    let output = git::run(repo, &["rev-parse", "--absolute-git-dir"])?;
    let text = String::from_utf8_lossy(&output);
    Ok(PathBuf::from(text.trim_end_matches('\n')))
}

// The order of `GitRepositoryReader.readRepositoryState`. With a detached HEAD, `Grafting` and
// `Reverting` also count: aborting a cherry-pick on a detached HEAD is just as necessary.
fn current_in(git_dir: &Path) -> Option<Operation> {
    let has = |name: &str| git_dir.join(name).exists();
    if has(MERGE_HEAD) {
        Some(Operation::Merging)
    } else if REBASE_DIRS.iter().any(|dir| has(dir)) {
        Some(Operation::Rebasing)
    } else if has(CHERRY_PICK_HEAD) {
        Some(Operation::CherryPicking)
    } else if has(REVERT_HEAD) {
        Some(Operation::Reverting)
    } else {
        None
    }
}

// The name of the operation in the task title (`abort.operation.progress.title`).
pub fn name(kind: Operation) -> &'static str {
    match kind {
        Operation::Merging => "Merge",
        Operation::Rebasing => "Rebase",
        Operation::CherryPicking => "Cherry-pick",
        Operation::Reverting => "Revert",
    }
}

pub fn current(repo: &Path) -> Result<Option<Operation>, GitError> {
    Ok(current_in(&git_dir(repo)?))
}

// `findCurrentBranchName`: with a detached HEAD in a rebase, the branch being rebased is in `head-name`.
fn rebased_branch(git_dir: &Path) -> Option<String> {
    REBASE_DIRS.iter().find_map(|dir| {
        let text = fs::read_to_string(git_dir.join(dir).join(HEAD_NAME)).ok()?;
        let name = text.trim().strip_prefix(HEADS)?;
        Some(name.to_string())
    })
}

fn head_of(repo: &Path) -> Result<String, GitError> {
    match git::run(repo, &["rev-parse", "-q", "--verify", HEAD]) {
        Ok(output) => Ok(String::from_utf8_lossy(&output).trim().to_string()),
        Err(GitError::Failed { code: Some(1), .. }) => Ok(String::new()),
        Err(error) => Err(error),
    }
}

// `GitBranchUtil.getDisplayableBranchText`. A cherry-pick or a revert on a detached HEAD count
// as a detached HEAD, as in IntelliJ: there is no branch to speak of.
fn label_of(operation: Option<Operation>, branch: Option<&str>, head: &str) -> String {
    let short: String = head.chars().take(SHORT_HASH).collect();
    let name = branch.map_or(short.as_str(), |branch| branch);
    match (operation, branch) {
        (Some(Operation::Merging), _) => format!("Merging {name}"),
        (Some(Operation::Rebasing), _) => format!("Rebasing {name}"),
        (Some(Operation::CherryPicking), Some(branch)) => format!("Cherry-picking in {branch}"),
        (Some(Operation::Reverting), Some(branch)) => format!("Reverting in {branch}"),
        _ if name.is_empty() => UNKNOWN.to_string(),
        _ => name.to_string(),
    }
}

pub fn state(repo: &Path) -> Result<RepoState, GitError> {
    let dir = git_dir(repo)?;
    let operation = current_in(&dir);
    let head = head_of(repo)?;
    let branch = match branches::current_name(repo)? {
        Some(branch) => Some(branch),
        None => rebased_branch(&dir),
    };
    Ok(RepoState {
        label: label_of(operation, branch.as_deref(), &head),
        operation,
        branch,
        head,
    })
}

fn aborted(kind: Operation) -> Aborted {
    match kind {
        Operation::Merging => Aborted::Merge,
        Operation::Rebasing => Aborted::Rebase,
        Operation::CherryPicking => Aborted::CherryPick,
        Operation::Reverting => Aborted::Revert,
    }
}

// `--abort` leaves the tree as it was with a reset: whatever was resolved by hand would go with it.
// Not cancellable, because it also runs when an Update was already cancelled, and a half-done abort
// is worse than the half-done operation. It says what it undid: `Nothing` if what is in progress is not `kind`.
pub fn undo(repo: &Repo, kind: Operation) -> Result<Aborted, GitError> {
    if current(repo.path)? != Some(kind) {
        return Ok(Aborted::Nothing);
    }
    let (reason, command) = match kind {
        Operation::Rebasing => {
            rebase::abort(repo)?;
            return Ok(aborted(kind));
        }
        Operation::Merging => (MERGE_ABORT, "merge"),
        Operation::CherryPicking => (CHERRY_PICK_ABORT, "cherry-pick"),
        Operation::Reverting => (REVERT_ABORT, "revert"),
    };
    branches::copy_before(repo, reason, |path| snapshots::at_risk(path, HEAD))?;
    repo.ctx.git_to_the_end(&[command, "--abort"])?;
    Ok(aborted(kind))
}

// It only acts if `kind` is what is in progress: a dialog that was left open must not undo
// something else.
pub fn abort(repo: &Repo, kind: Operation) -> Result<(), GitError> {
    undo(repo, kind).map(drop)
}

// Like `abort`, but it says what was undone, and it touches nothing if HEAD is no longer the one the asker saw.
pub fn abort_expecting(
    repo: &Repo,
    kind: Operation,
    expected_head: &str,
) -> Result<Aborted, GitError> {
    check_head(repo.path, expected_head)?;
    undo(repo, kind)
}

// The Merge… and Rebase… dialogs are only valid with the repository at rest: git lets a merge start
// in the middle of a stopped rebase, and the dialog did not see it that way.
pub(crate) fn check_idle(repo: &Path) -> Result<(), GitError> {
    if current(repo)?.is_some() {
        return Err(GitError::Io(REPOSITORY_CHANGED.to_string()));
    }
    Ok(())
}

// A dialog returns the HEAD it saw: if it is not the current one anymore, nothing is touched.
pub(crate) fn check_head(repo: &Path, expected_head: &str) -> Result<(), GitError> {
    if head_of(repo)? != expected_head {
        return Err(GitError::Io(REPOSITORY_CHANGED.to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::RepoHarness;

    // `main` and `side` in conflict, with the requested operation half done.
    fn conflicted(command: &[&str]) -> RepoHarness {
        let harness = RepoHarness::new();
        harness.diverged();
        harness.git_fails(command);
        harness
    }

    fn merging() -> RepoHarness {
        conflicted(&["merge", "side"])
    }

    fn cherry_picking() -> RepoHarness {
        conflicted(&["cherry-pick", "side"])
    }

    fn reverting() -> RepoHarness {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"base\n", "base");
        harness.repo.commit("a.txt", b"one\n", "one");
        harness.repo.commit("a.txt", b"two\n", "two");
        harness.git_fails(&["revert", "HEAD~1"]);
        harness
    }

    // `feature` rebased onto `main`, stopped at a conflict: HEAD ends up detached.
    fn rebasing() -> RepoHarness {
        let harness = RepoHarness::new();
        harness.diverged();
        harness.git(&["checkout", "-q", "side"]);
        harness.git_fails(&["rebase", "main"]);
        harness
    }

    #[test]
    fn detects_each_operation_in_its_order() {
        let plain = RepoHarness::new();
        plain.repo.commit("a.txt", b"1\n", "one");
        assert_eq!(current(plain.path()), Ok(None));
        assert_eq!(current(merging().path()), Ok(Some(Operation::Merging)));
        assert_eq!(current(rebasing().path()), Ok(Some(Operation::Rebasing)));
        assert_eq!(
            current(cherry_picking().path()),
            Ok(Some(Operation::CherryPicking))
        );
        assert_eq!(current(reverting().path()), Ok(Some(Operation::Reverting)));
        // With `rebase-apply/` too.
        let apply = RepoHarness::new();
        apply.diverged();
        apply.git(&["checkout", "-q", "side"]);
        apply.git_fails(&["rebase", "--apply", "main"]);
        assert!(apply.in_git_dir("rebase-apply"));
        assert_eq!(current(apply.path()), Ok(Some(Operation::Rebasing)));

        // `GitRepositoryReader` looks at the merge first, then the rebase, then cherry-pick and revert.
        let both = rebasing();
        both.write(".git/MERGE_HEAD", &both.head());
        assert_eq!(current(both.path()), Ok(Some(Operation::Merging)));
        let graft = cherry_picking();
        graft.write(".git/REVERT_HEAD", &graft.head());
        assert_eq!(current(graft.path()), Ok(Some(Operation::CherryPicking)));
    }

    #[test]
    fn widget_label_matches_intellij() {
        let plain = RepoHarness::new();
        plain.repo.commit("a.txt", b"1\n", "one");
        assert_eq!(state(plain.path()).unwrap().label, "main");
        assert_eq!(state(merging().path()).unwrap().label, "Merging main");
        // The branch comes from `head-name` even if HEAD is detached.
        let rebase = rebasing();
        assert_eq!(rebase.git(&["branch", "--show-current"]), "");
        let rebase_state = state(rebase.path()).unwrap();
        assert_eq!(rebase_state.label, "Rebasing side");
        assert_eq!(rebase_state.branch.as_deref(), Some("side"));
        assert_eq!(
            state(cherry_picking().path()).unwrap().label,
            "Cherry-picking in main"
        );
        assert_eq!(
            state(reverting().path()).unwrap().label,
            "Reverting in main"
        );
    }

    #[test]
    fn with_detached_head_the_widget_shows_the_short_hash() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"1\n", "one");
        harness.git(&["checkout", "-q", "--detach"]);

        let state = state(harness.path()).unwrap();

        assert_eq!(state.operation, None);
        assert_eq!(state.branch, None);
        assert_eq!(state.label, harness.head()[..8]);
        assert_eq!(state.head, harness.head());
    }

    #[test]
    fn a_repository_without_commits_has_a_branch_and_empty_head() {
        let harness = RepoHarness::new();

        let state = state(harness.path()).unwrap();

        assert_eq!(state.label, "main");
        assert_eq!(state.head, "");
    }

    #[test]
    fn the_state_travels_as_the_interface_expects() {
        let json = serde_json::to_value(state(rebasing().path()).unwrap()).unwrap();

        assert_eq!(json["operation"], "rebasing");
        assert_eq!(json["branch"], "side");
        assert_eq!(json["label"], "Rebasing side");
        assert_eq!(json["head"].as_str().map(str::len), Some(40));
        assert_eq!(
            serde_json::to_value(Operation::CherryPicking).unwrap(),
            "cherryPicking"
        );
        assert_eq!(
            serde_json::to_value(Aborted::CherryPick).unwrap(),
            "cherryPick"
        );
    }

    #[test]
    fn abort_merge_uses_merge_abort_and_saves_a_copy() {
        let harness = merging();
        let head = harness.head();

        assert_eq!(harness.run(|repo| abort(repo, Operation::Merging)), Ok(()));

        assert!(harness.ran("merge --abort"));
        assert!(!harness.in_git_dir("MERGE_HEAD"));
        assert_eq!(harness.head(), head);
        assert_eq!(harness.read("a.txt"), "main\n");
        // The conflicted file had markers that the abort discards.
        assert_eq!(harness.saved_paths(), ["a.txt"]);
    }

    #[test]
    fn abort_cherry_pick_and_revert() {
        let picking = cherry_picking();
        assert_eq!(
            picking.run(|repo| abort(repo, Operation::CherryPicking)),
            Ok(())
        );
        assert!(picking.ran("cherry-pick --abort"));
        assert!(!picking.in_git_dir("CHERRY_PICK_HEAD"));
        assert_eq!(picking.read("a.txt"), "main\n");
        assert_eq!(picking.saved_paths(), ["a.txt"]);

        let reverting = reverting();
        assert_eq!(
            reverting.run(|repo| abort(repo, Operation::Reverting)),
            Ok(())
        );
        assert!(reverting.ran("revert --abort"));
        assert!(!reverting.in_git_dir("REVERT_HEAD"));
        assert_eq!(reverting.read("a.txt"), "two\n");
        assert_eq!(reverting.saved_paths(), ["a.txt"]);
    }

    #[test]
    fn abort_without_operation_does_nothing() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"1\n", "one");
        harness.write("a.txt", "local\n");

        let head = harness.head();
        for kind in [
            Operation::Merging,
            Operation::Rebasing,
            Operation::CherryPicking,
            Operation::Reverting,
        ] {
            let seen = head.clone();
            assert_eq!(harness.run(move |repo| abort(repo, kind)), Ok(()));
            assert_eq!(
                harness.run(move |repo| abort_expecting(repo, kind, &seen)),
                Ok(Aborted::Nothing)
            );
        }

        assert!(!harness.ran("merge --abort"));
        assert!(!harness.ran("rebase --abort"));
        assert!(!harness.ran("cherry-pick --abort"));
        assert!(!harness.ran("revert --abort"));
        assert_eq!(harness.read("a.txt"), "local\n");
        assert_eq!(harness.snapshots(), 0);
    }

    #[test]
    fn abort_of_another_operation_does_not_touch_the_current_one() {
        let harness = merging();

        assert_eq!(
            harness.run(|repo| abort(repo, Operation::CherryPicking)),
            Ok(())
        );

        assert!(harness.in_git_dir("MERGE_HEAD"));
        assert_eq!(harness.snapshots(), 0);
    }

    #[test]
    fn abort_with_head_different_from_the_dialog_fails_without_touching() {
        let harness = merging();
        let seen = harness.head();
        // HEAD moved while the dialog was open.
        let side_head = harness.git(&["rev-parse", "side"]);
        harness.git(&["update-ref", "HEAD", side_head.trim()]);

        let aborted = harness.run(move |repo| abort_expecting(repo, Operation::Merging, &seen));

        assert!(
            matches!(&aborted, Err(GitError::Io(text)) if text == "Repository has already been changed"),
            "{aborted:?}"
        );
        assert!(harness.in_git_dir("MERGE_HEAD"));
        assert_eq!(harness.snapshots(), 0);
        assert!(!harness.ran("merge --abort"));
    }
}
