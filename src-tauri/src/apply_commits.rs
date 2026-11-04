// Cherry-Pick and Revert Commit of the log: applies the commits one by one and stops at the first failure.
// Ported from `GitApplyChangesProcess`, `GitCherryPicker`, `GitRevertOperation` and from what
// `VcsCherryPickManager` does with the order; the commit texts are IntelliJ's.
use std::collections::BTreeSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::branches::{self, Repo};
use crate::git::{self, GitError};
use crate::operation::{self, Operation};
use crate::preserve;
use crate::rebase;
use crate::settings;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    CherryPick,
    Revert,
}

// Why the sequence stopped. `hash` is the commit that could not be applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "reason", rename_all = "camelCase")]
pub enum Stop {
    Conflicts { hash: String },
    LocalChanges { hash: String, files: Vec<String> },
    Untracked { hash: String, files: Vec<String> },
    Failed { hash: String, text: String },
}

// `applied` and `skipped` are in the order in which they were applied. `remaining` are the commits that were
// not tried, in the order in which they arrived, to continue with them after resolving.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyResult {
    pub applied: Vec<String>,
    pub skipped: Vec<String>,
    pub stopped: Option<Stop>,
    pub remaining: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Finished {
    Committed,
    // What was resolved left the tree as it was: there is nothing to commit.
    Empty,
}

const CHERRY_PICK_HEAD: &str = "CHERRY_PICK_HEAD";
const REVERT_HEAD: &str = "REVERT_HEAD";
const MESSAGE_ENCODING: &str = "i18n.commitEncoding=UTF-8";
// `action.description.cant.revert.merge.commit`.
const CANT_REVERT_MERGE: &str = "Reverting merge commits is not allowed";
const REPOSITORY_CHANGED: &str = "Repository has already been changed";
// `DvcsUtil.getShortHash`.
const SHORT_HASH: usize = 8;

impl Kind {
    fn command(self) -> &'static str {
        match self {
            Self::CherryPick => "cherry-pick",
            Self::Revert => "revert",
        }
    }

    // What git leaves while the operation is half-done.
    fn head_file(self) -> &'static str {
        match self {
            Self::CherryPick => CHERRY_PICK_HEAD,
            Self::Revert => REVERT_HEAD,
        }
    }

    fn operation(self) -> Operation {
        match self {
            Self::CherryPick => Operation::CherryPicking,
            Self::Revert => Operation::Reverting,
        }
    }
}

// The full hashes, without repeats and already validated. A revert does not accept merges: this is checked before
// touching anything, so that half is not left applied.
fn validated(repo: &Repo, kind: Kind, hashes: &[String]) -> Result<Vec<String>, GitError> {
    let mut full: Vec<String> = Vec::new();
    for hash in hashes {
        branches::check_argument(hash, "The commit")?;
        let hash = branches::commit_of(repo, hash)?;
        if full.contains(&hash) {
            continue;
        }
        if kind == Kind::Revert {
            let line = repo.ctx.git(&["rev-list", "--parents", "-n", "1", &hash])?;
            if line.stdout.split_whitespace().count() > 2 {
                return Err(branches::invalid(CANT_REVERT_MERGE.to_string()));
            }
        }
        full.push(hash);
    }
    Ok(full)
}

// `GitCherryPicker.shouldAddSuffix`: the commit is in some protected remote branch.
fn is_published(path: &Path, protected: &[(String, String)], hash: &str) -> Result<bool, GitError> {
    for (full, _) in protected {
        match git::run(path, &["merge-base", "--is-ancestor", hash, full]) {
            Ok(_) => return Ok(true),
            Err(GitError::Failed { code: Some(1), .. }) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(false)
}

// The protected remote branches, only if the setting is on and it is a cherry-pick.
fn protected_for(repo: &Repo, kind: Kind, suffix: bool) -> Result<Vec<(String, String)>, GitError> {
    if kind != Kind::CherryPick || !suffix {
        return Ok(Vec::new());
    }
    let patterns = settings::load_repo(repo.data).protected_branches;
    rebase::protected_remotes(repo.path, &patterns)
}

// `GitApplyChangesProcess.executeForCommit` for one commit.
enum One {
    Applied,
    Skipped,
    Stopped(Stop),
}

// `GitSimpleEventDetector(CHERRY_PICK_CONFLICT)`, and if git does not give the advice (`advice.mergeConflict`),
// what a conflict leaves: the operation half-done with unresolved paths.
fn conflicted(path: &Path, kind: Kind, idle: bool, text: &str) -> Result<bool, GitError> {
    if text
        .to_lowercase()
        .contains("after resolving the conflicts")
    {
        return Ok(true);
    }
    Ok(idle
        && operation::current(path)? == Some(kind.operation())
        && crate::commit::has_unmerged(path)?)
}

fn nothing_to_commit(kind: Kind, text: &str) -> bool {
    text.contains("nothing to commit")
        || (kind == Kind::CherryPick && text.contains("previous cherry-pick is now empty"))
}

// If the operation is still half-done over a change already applied, it is cleaned up like `--skip`: leaving it
// would block the next cherry-pick.
fn clear(repo: &Repo, kind: Kind) -> Result<(), GitError> {
    if operation::current(repo.path)? == Some(kind.operation()) {
        repo.ctx.git_to_the_end(&[kind.command(), "--skip"])?;
    }
    Ok(())
}

fn apply_one(repo: &Repo, kind: Kind, hash: &str, suffix: bool) -> Result<One, GitError> {
    let idle = operation::current(repo.path)?.is_none();
    let mut args = vec![kind.command()];
    match kind {
        Kind::CherryPick if suffix => args.push("-x"),
        Kind::CherryPick => {}
        // Git's editor does not exist here: the message is git's.
        Kind::Revert => args.push("--no-edit"),
    }
    args.push(hash);
    let (stdout, stderr) = match repo.ctx.git_loud(&args) {
        Ok(_) => return Ok(One::Applied),
        Err(GitError::Failed { stdout, stderr, .. }) => (stdout, stderr),
        Err(error) => return Err(error),
    };
    let text = format!("{stdout}\n{stderr}");
    let hash = hash.to_string();
    if conflicted(repo.path, kind, idle, &text)? {
        return Ok(One::Stopped(Stop::Conflicts { hash }));
    }
    if text.contains(preserve::UNTRACKED_FILES) {
        let files = preserve::listed_files(&text, preserve::UNTRACKED_FILES);
        return Ok(One::Stopped(Stop::Untracked { hash, files }));
    }
    if text.contains("would be overwritten by") {
        let files = branches::overwritten_files(&text);
        return Ok(One::Stopped(Stop::LocalChanges { hash, files }));
    }
    if nothing_to_commit(kind, &text) {
        if idle {
            clear(repo, kind)?;
        }
        return Ok(One::Skipped);
    }
    let text = rebase::failure_text(&stdout, &stderr);
    Ok(One::Stopped(Stop::Failed { hash, text }))
}

// `hashes` arrive in the log's order, from the most recent to the oldest. `VcsCherryPickManager` reverses
// them for the cherry-pick, and `GitRevertOperation` applies them as they are.
pub fn run(
    repo: &Repo,
    kind: Kind,
    hashes: &[String],
    expected_head: &str,
) -> Result<ApplyResult, GitError> {
    operation::check_head(repo.path, expected_head)?;
    let shown = validated(repo, kind, hashes)?;
    let order: Vec<&String> = match kind {
        Kind::CherryPick => shown.iter().rev().collect(),
        Kind::Revert => shown.iter().collect(),
    };
    let suffix = repo.ctx.cherry_pick_suffix();
    let protected = protected_for(repo, kind, suffix)?;
    let mut result = ApplyResult::default();
    for (position, hash) in order.iter().enumerate() {
        repo.ctx.check_cancelled()?;
        let add = !protected.is_empty() && is_published(repo.path, &protected, hash)?;
        match apply_one(repo, kind, hash, add)? {
            One::Applied => result.applied.push((*hash).clone()),
            One::Skipped => result.skipped.push((*hash).clone()),
            One::Stopped(stop) => {
                result.stopped = Some(stop);
                let rest = &order[position + 1..];
                result.remaining = shown
                    .iter()
                    .filter(|hash| rest.contains(hash))
                    .cloned()
                    .collect();
                break;
            }
        }
    }
    Ok(result)
}

fn head_hash(path: &Path, kind: Kind) -> Result<Option<String>, GitError> {
    let file = git::git_path(path, kind.head_file())?;
    Ok(std::fs::read_to_string(file)
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty()))
}

fn message_of(
    path: &Path,
    kind: Kind,
    hash: &str,
    suffix: bool,
    patterns: &[String],
) -> Result<String, GitError> {
    match kind {
        // `GitRevertOperation`: `Revert "subject"` and the short hash, without a final period.
        Kind::Revert => {
            let subject = git::run(path, &["log", "-1", "--format=%s", hash])?;
            let subject = String::from_utf8_lossy(&subject);
            let short: String = hash.chars().take(SHORT_HASH).collect();
            Ok(format!(
                "Revert \"{}\"\n\nThis reverts commit {short}",
                subject.trim_end_matches('\n')
            ))
        }
        // `GitCherryPicker.createCommitMessage`.
        Kind::CherryPick => {
            let full = git::run(path, &["log", "-1", "--format=%B", hash])?;
            let mut message = String::from_utf8_lossy(&full)
                .trim_end_matches('\n')
                .to_string();
            if suffix {
                let protected = rebase::protected_remotes(path, patterns)?;
                if is_published(path, &protected, hash)? {
                    message.push_str(&format!("\n\n(cherry picked from commit {hash})"));
                }
            }
            Ok(message)
        }
    }
}

// `GitApplyChangesProcess`, after `ConflictResolver.merge()` resolved: commits with the proposed
// message. With `CHERRY_PICK_HEAD` in place, git keeps the author and the date of the commit; IntelliJ
// removes it before committing because its commit can be partial, and here it is not. If what was resolved
// leaves the tree as it was, there is nothing to commit (`changes.isEmpty()`): it is skipped.
pub fn commit_after_conflicts(repo: &Repo, kind: Kind, hash: &str) -> Result<Finished, GitError> {
    branches::check_argument(hash, "The commit")?;
    let hash = branches::commit_of(repo, hash)?;
    let name = kind.command();
    let Some(current) = head_hash(repo.path, kind)? else {
        return Err(GitError::Io(format!("No {name} in progress")));
    };
    if current != hash {
        return Err(GitError::Io(REPOSITORY_CHANGED.to_string()));
    }
    if crate::commit::has_unmerged(repo.path)? {
        return Err(GitError::Io(crate::commit::UNMERGED_PATHS.to_string()));
    }
    let patterns = settings::load_repo(repo.data).protected_branches;
    let message = message_of(
        repo.path,
        kind,
        &hash,
        repo.ctx.cherry_pick_suffix(),
        &patterns,
    )?;
    match repo.ctx.git(&["diff", "--cached", "--quiet"]) {
        Ok(_) => {
            repo.ctx.git_to_the_end(&[name, "--skip"])?;
            return Ok(Finished::Empty);
        }
        Err(GitError::Failed { code: Some(1), .. }) => {}
        Err(error) => return Err(error),
    }
    repo.ctx
        .git_loud(&["-c", MESSAGE_ENCODING, "commit", "-m", &message])?;
    Ok(Finished::Committed)
}

// What the Commit view proposes while there is a half-done cherry-pick or revert: IntelliJ's message
// and not git's, which carries the comments of the conflicts.
pub fn proposed_message(
    path: &Path,
    data: &Path,
    kind: Kind,
    suffix: bool,
) -> Result<Option<String>, GitError> {
    let Some(hash) = head_hash(path, kind)? else {
        return Ok(None);
    };
    let patterns = settings::load_repo(data).protected_branches;
    message_of(path, kind, &hash, suffix, &patterns).map(Some)
}

// What the Commit view proposes depending on what is half-done: IntelliJ's message if it is a
// cherry-pick or a revert, and nothing if it is anything else (the merge carries its own, git's).
pub fn proposal_in_progress(
    path: &Path,
    data: &Path,
    suffix: bool,
) -> Result<Option<String>, GitError> {
    let kind = match operation::current(path)? {
        Some(Operation::CherryPicking) => Kind::CherryPick,
        Some(Operation::Reverting) => Kind::Revert,
        _ => return Ok(None),
    };
    proposed_message(path, data, kind, suffix)
}

// Cherry-Pick Selected Changes and Revert Selected Changes of the log's changes panel. IntelliJ
// builds a patch of its own and applies it with its `PatchApplier`; here it comes from `git diff` and is applied by
// `git apply` (decision 8 of the plan).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "reason", rename_all = "camelCase")]
pub enum PathsResult {
    Applied,
    // `--3way` left the conflict with its stages in the index, for the Conflicts dialog.
    Conflicts,
    Failed(String),
}

// The paths of a commit that are applied, against `base` (the one the panel shows). Without `base`, the old
// side does not exist: the empty tree, as in the panel (a root commit, or what a selection creates).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Selection {
    pub hash: String,
    pub base: Option<String>,
    pub paths: Vec<String>,
}

const NOTHING_TO_APPLY: &str = "There are no changes to apply";
// What the panel shows: `RevertSelectedChangesAction` and its `Apply` pair.
const REVERT_CHANGES: &str = "Revert Changes";
const APPLY_CHANGES: &str = "Apply Changes";

// `git diff` with the paths as literals and in a format that `git apply` understands whatever the
// user's configuration says (prefixes, textconv, renames).
fn patch_of(repo: &Repo, selection: &[Selection]) -> Result<Vec<u8>, GitError> {
    let mut patch = Vec::new();
    for group in selection {
        branches::check_argument(&group.hash, "The commit")?;
        let hash = branches::commit_of(repo, &group.hash)?;
        let base = match &group.base {
            Some(base) => {
                branches::check_argument(base, "The base commit")?;
                branches::commit_of(repo, base)?
            }
            None => empty_tree(repo)?,
        };
        if group
            .paths
            .iter()
            .any(|path| path.is_empty() || path.contains('\0'))
        {
            return Err(branches::invalid("The path is not valid".to_string()));
        }
        let head = [
            "diff",
            "--binary",
            "--full-index",
            "--no-renames",
            "--no-ext-diff",
            "--no-textconv",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            &base,
            &hash,
        ];
        patch.extend(git::git_with_paths(repo.path, false, &head, &group.paths)?);
    }
    Ok(patch)
}

fn empty_tree(repo: &Repo) -> Result<String, GitError> {
    let tree = git::run_with_input(repo.path, &["hash-object", "-t", "tree", "--stdin"], b"")?;
    Ok(String::from_utf8_lossy(&tree).trim().to_string())
}

fn names(output: &[u8]) -> Result<BTreeSet<String>, GitError> {
    output
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
        .map(|name| {
            String::from_utf8(name.to_vec())
                .map_err(|_| GitError::Io("There is a path that is not UTF-8".to_string()))
        })
        .collect()
}

// Those of `paths` whose stage is not HEAD's, the unresolved ones included.
fn staged_among(path: &Path, paths: &[String]) -> Result<BTreeSet<String>, GitError> {
    let head = ["diff", "--cached", "--name-only", "-z", "--no-renames"];
    names(&git::git_with_paths(path, false, &head, paths)?)
}

// Those of `paths` whose disk is not the index's: edits, deletions, mode and type.
fn unstaged_among(path: &Path, paths: &[String]) -> Result<BTreeSet<String>, GitError> {
    let head = ["diff", "--name-only", "-z", "--no-renames"];
    names(&git::git_with_paths(path, false, &head, paths)?)
}

fn unmerged(path: &Path) -> Result<BTreeSet<String>, GitError> {
    names(&git::run(
        path,
        &["diff", "--name-only", "--diff-filter=U", "-z"],
    )?)
}

fn apply(repo: &Repo, patch: &[u8], reverse: bool, three_way: bool) -> Result<(), GitError> {
    // The user's `apply.whitespace=error` would make a patch that already belongs to a commit fail.
    let mut args = vec!["apply", "--whitespace=nowarn"];
    if reverse {
        args.push("-R");
    }
    if three_way {
        args.push("--3way");
    }
    repo.ctx.git_loud_with_input(&args, patch).map(drop)
}

// `--3way` leaves everything in the stage. It is taken out, except the conflicts (which keep their stages) and what
// was already in the stage before: the user put that there.
fn unstage(repo: &Repo, paths: &[String], kept: &BTreeSet<String>) -> Result<(), GitError> {
    let conflicts = unmerged(repo.path)?;
    let list: Vec<String> = staged_among(repo.path, paths)?
        .into_iter()
        .filter(|path| !kept.contains(path) && !conflicts.contains(path))
        .collect();
    if list.is_empty() {
        return Ok(());
    }
    git::git_with_paths(repo.path, true, &["reset", "-q"], &list).map(drop)
}

fn failure_of(error: GitError) -> Result<String, GitError> {
    match error {
        GitError::Failed { stdout, stderr, .. } => Ok(rebase::failure_text(&stdout, &stderr)),
        error => Err(error),
    }
}

// `RevertCommittedStuffAbstractAction`: the patch from `base` to `hash` for those paths, reversed if
// `reverse`. First `git apply` on the working tree, which overwrites nothing: either everything goes in or nothing
// does, and it leaves no stage. If it does not go in, `--3way`; that one requires the files to have no changes
// without stage, so it does not overwrite local changes either. What was already in the stage it does rewrite, and
// it is copied beforehand.
fn apply_groups(
    repo: &Repo,
    selection: &[Selection],
    reverse: bool,
) -> Result<PathsResult, GitError> {
    let patch = patch_of(repo, selection)?;
    if patch.is_empty() {
        return Ok(PathsResult::Failed(NOTHING_TO_APPLY.to_string()));
    }
    let first = match apply(repo, &patch, reverse, false) {
        Ok(()) => return Ok(PathsResult::Applied),
        Err(error) => failure_of(error)?,
    };
    let paths: Vec<String> = selection
        .iter()
        .flat_map(|group| group.paths.iter().cloned())
        .collect();
    // `--3way` works on the index and rewrites the disk: with something unstaged in those paths (an
    // edit, a deletion, a mode or type change) it undoes it silently.
    if !unstaged_among(repo.path, &paths)?.is_empty() {
        return Ok(PathsResult::Failed(first));
    }
    let kept = staged_among(repo.path, &paths)?;
    let reason = if reverse {
        REVERT_CHANGES
    } else {
        APPLY_CHANGES
    };
    branches::copy_before(repo, reason, |path| {
        Ok(staged_among(path, &paths)?.into_iter().collect())
    })?;
    let before = unmerged(repo.path)?;
    match apply(repo, &patch, reverse, true) {
        Ok(()) => {
            unstage(repo, &paths, &kept)?;
            Ok(PathsResult::Applied)
        }
        Err(error) => {
            failure_of(error)?;
            if unmerged(repo.path)?.is_subset(&before) {
                // The first attempt says why it does not go in; the `--3way` one, that there is no base.
                return Ok(PathsResult::Failed(first));
            }
            unstage(repo, &paths, &kept)?;
            Ok(PathsResult::Conflicts)
        }
    }
}

pub fn apply_paths(
    repo: &Repo,
    hash: &str,
    paths: &[String],
    reverse: bool,
) -> Result<PathsResult, GitError> {
    // The panel shows a commit against its first parent; if it is a root, against nothing.
    branches::check_argument(hash, "The commit")?;
    let hash = branches::commit_of(repo, hash)?;
    let line = repo.ctx.git(&["rev-list", "--parents", "-n", "1", &hash])?;
    let base = line.stdout.split_whitespace().nth(1).map(str::to_string);
    let selection = [Selection {
        hash,
        base,
        paths: paths.to_vec(),
    }];
    apply_groups(repo, &selection, reverse)
}

// What the changes panel asks for: several paths, each with the commit and the base it shows, and the
// `HEAD` that was seen when choosing the destination list.
pub fn apply_selection(
    repo: &Repo,
    expected_head: &str,
    selection: &[Selection],
    reverse: bool,
) -> Result<PathsResult, GitError> {
    operation::check_head(repo.path, expected_head)?;
    apply_groups(repo, selection, reverse)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::RepoHarness;
    use tempfile::TempDir;

    fn hashes(harness: &RepoHarness, range: &str) -> Vec<String> {
        let output = harness.git(&["rev-list", range]);
        output.lines().map(str::to_string).collect()
    }

    fn subjects(harness: &RepoHarness) -> Vec<String> {
        let output = harness.git(&["log", "--format=%s", "main"]);
        output.lines().map(str::to_string).collect()
    }

    fn message_of(harness: &RepoHarness, revision: &str) -> String {
        let message = harness.git(&["log", "-1", "--format=%B", revision]);
        message.trim_end().to_string()
    }

    // The hashes of `list` at those positions.
    fn at(list: &[String], positions: &[usize]) -> Vec<String> {
        positions.iter().map(|&at| list[at].clone()).collect()
    }

    fn text(error: &GitError) -> String {
        branches::text_of(error.clone())
    }

    fn apply(harness: &RepoHarness, kind: Kind, list: &[String]) -> Result<ApplyResult, GitError> {
        let (head, list) = (harness.head(), list.to_vec());
        harness.run(move |repo| run(repo, kind, &list, &head))
    }

    fn finish(harness: &RepoHarness, kind: Kind, hash: &str) -> Result<Finished, GitError> {
        let hash = hash.to_string();
        harness.run(move |repo| commit_after_conflicts(repo, kind, &hash))
    }

    fn proposal(harness: &RepoHarness, kind: Kind, suffix: bool) -> Option<String> {
        let (path, data) = (
            harness.path().to_path_buf(),
            harness.data.path().to_path_buf(),
        );
        proposed_message(&path, &data, kind, suffix).unwrap()
    }

    // `main` at `base` and `side` with `one`, `two` and `three`, each in its own file. Returns the
    // hashes in the log's order: `three`, `two`, `one`.
    fn picking() -> (RepoHarness, Vec<String>) {
        let harness = RepoHarness::new();
        harness.repo.commit("base.txt", b"base\n", "base");
        harness.git(&["checkout", "-q", "-b", "side"]);
        for name in ["one", "two", "three"] {
            harness
                .repo
                .commit(&format!("{name}.txt"), name.as_bytes(), name);
        }
        harness.git(&["checkout", "-q", "main"]);
        let list = hashes(&harness, "main..side");
        (harness, list)
    }

    // A bare remote whose `main` already has what `side` has: its commits are published.
    fn publish(harness: &RepoHarness) -> TempDir {
        let remote = TempDir::new().unwrap();
        let url = remote.path().to_str().unwrap();
        harness.git(&["init", "-q", "--bare", url]);
        harness.git(&["remote", "add", "origin", url]);
        harness.git(&["push", "-q", "origin", "side:main"]);
        harness.git(&["fetch", "-q", "origin"]);
        remote
    }

    // The second commit of `side`, which someone else wrote, touches `a.txt` just like `main`.
    fn conflicting() -> (RepoHarness, Vec<String>) {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"base\n", "base");
        harness.git(&["checkout", "-q", "-b", "side"]);
        harness.repo.commit("one.txt", b"one\n", "one");
        harness.write("a.txt", "side\n");
        harness.git(&["add", "a.txt"]);
        harness.git(&[
            "commit",
            "-q",
            "-m",
            "clashes",
            "-m",
            "with body",
            "--author=Another Person <another@example.com>",
        ]);
        harness.repo.commit("three.txt", b"three\n", "three");
        harness.git(&["checkout", "-q", "main"]);
        harness.repo.commit("a.txt", b"main\n", "on main");
        let list = hashes(&harness, "main..side");
        (harness, list)
    }

    fn resolve(harness: &RepoHarness, text: &str) {
        harness.write("a.txt", text);
        harness.git(&["add", "a.txt"]);
    }

    #[test]
    fn cherry_pick_of_a_published_commit_carries_x() {
        let (harness, list) = picking();
        let _remote = publish(&harness);

        let result = apply(&harness, Kind::CherryPick, &list[2..]).unwrap();

        assert_eq!(result.applied, &list[2..]);
        assert_eq!(
            message_of(&harness, "HEAD"),
            format!("one\n\n(cherry picked from commit {})", list[2])
        );
    }

    #[test]
    fn unpublished_cherry_pick_carries_no_x() {
        let (harness, list) = picking();

        apply(&harness, Kind::CherryPick, &list[2..]).unwrap();

        assert_eq!(message_of(&harness, "HEAD"), "one");
    }

    #[test]
    fn setting_off_never_carries_x() {
        let (harness, list) = picking();
        let _remote = publish(&harness);
        harness.runner.set_cherry_pick_suffix(false);

        apply(&harness, Kind::CherryPick, &list[2..]).unwrap();

        assert_eq!(message_of(&harness, "HEAD"), "one");
    }

    #[test]
    fn only_protected_branches_publish() {
        let (harness, list) = picking();
        let _remote = publish(&harness);
        // With other patterns, `origin/main` is no longer protected.
        settings::save_protected_branches(harness.data.path(), vec!["release".to_string()])
            .unwrap();

        apply(&harness, Kind::CherryPick, &list[2..]).unwrap();

        assert_eq!(message_of(&harness, "HEAD"), "one");
    }

    #[test]
    fn several_commits_in_intellij_order() {
        let (harness, list) = picking();

        let result = apply(&harness, Kind::CherryPick, &list).unwrap();

        // The log gives them from the most recent to the oldest, and they are applied from the oldest to the most recent.
        assert_eq!(result.applied, at(&list, &[2, 1, 0]));
        assert_eq!(subjects(&harness), ["three", "two", "one", "base"]);
        assert_eq!(result.stopped, None);
        assert!(result.remaining.is_empty());
    }

    #[test]
    fn empty_commit_is_skipped_and_it_goes_on() {
        let (harness, list) = picking();
        // `main` already has the change of `one`, with another message.
        harness.repo.commit("one.txt", b"one", "same thing");

        let result = apply(&harness, Kind::CherryPick, &list[..3]).unwrap();

        assert_eq!(result.skipped, at(&list, &[2]));
        assert_eq!(result.applied, at(&list, &[1, 0]));
        assert_eq!(result.stopped, None);
        assert!(!harness.in_git_dir("CHERRY_PICK_HEAD"));
        assert_eq!(subjects(&harness), ["three", "two", "same thing", "base"]);
    }

    #[test]
    fn conflict_stops_and_leaves_cherry_pick_head() {
        let (harness, list) = conflicting();

        let result = apply(&harness, Kind::CherryPick, &list).unwrap();

        assert_eq!(result.applied, at(&list, &[2]));
        assert_eq!(
            result.stopped,
            Some(Stop::Conflicts {
                hash: list[1].clone()
            })
        );
        // What was not tried, to continue after resolving.
        assert_eq!(result.remaining, at(&list, &[0]));
        assert!(harness.in_git_dir("CHERRY_PICK_HEAD"));
        assert_eq!(
            operation::current(harness.path()).unwrap(),
            Some(Operation::CherryPicking)
        );
    }

    #[test]
    fn conflict_is_detected_even_if_git_gives_no_advice() {
        let (harness, list) = conflicting();
        harness.git(&["config", "advice.mergeConflict", "false"]);

        let result = apply(&harness, Kind::CherryPick, &list[1..2]).unwrap();

        assert_eq!(
            result.stopped,
            Some(Stop::Conflicts {
                hash: list[1].clone()
            })
        );
    }

    #[test]
    fn commit_after_conflicts_keeps_the_author_and_the_suffix() {
        let (harness, list) = conflicting();
        let _remote = publish(&harness);
        let picked = list[1].clone();
        let result = apply(&harness, Kind::CherryPick, std::slice::from_ref(&picked)).unwrap();
        assert!(matches!(result.stopped, Some(Stop::Conflicts { .. })));
        resolve(&harness, "resolved\n");

        let finished = finish(&harness, Kind::CherryPick, &picked).unwrap();

        assert_eq!(finished, Finished::Committed);
        assert!(!harness.in_git_dir("CHERRY_PICK_HEAD"));
        assert_eq!(
            harness.git(&["log", "-1", "--format=%an <%ae>"]).trim(),
            "Another Person <another@example.com>"
        );
        assert_eq!(harness.git(&["log", "-1", "--format=%cn"]).trim(), "Test");
        assert_eq!(
            message_of(&harness, "HEAD"),
            format!("clashes\n\nwith body\n\n(cherry picked from commit {picked})")
        );
        assert_eq!(harness.read("a.txt"), "resolved\n");
    }

    #[test]
    fn commit_after_conflicts_unpublished_carries_no_suffix() {
        let (harness, list) = conflicting();
        let picked = list[1].clone();
        apply(&harness, Kind::CherryPick, std::slice::from_ref(&picked)).unwrap();
        resolve(&harness, "resolved\n");

        finish(&harness, Kind::CherryPick, &picked).unwrap();

        assert_eq!(message_of(&harness, "HEAD"), "clashes\n\nwith body");
    }

    #[test]
    fn proposed_message_of_a_cherry_pick_is_intellijs() {
        let (harness, list) = conflicting();
        let _remote = publish(&harness);
        let picked = list[1].clone();
        assert_eq!(proposal(&harness, Kind::CherryPick, true), None);
        apply(&harness, Kind::CherryPick, std::slice::from_ref(&picked)).unwrap();

        assert_eq!(
            proposal(&harness, Kind::CherryPick, true).unwrap(),
            format!("clashes\n\nwith body\n\n(cherry picked from commit {picked})")
        );
        assert_eq!(
            proposal(&harness, Kind::CherryPick, false).unwrap(),
            "clashes\n\nwith body"
        );
        // A cherry-pick proposes nothing if what is half-done is a revert.
        assert_eq!(proposal(&harness, Kind::Revert, true), None);
    }

    #[test]
    fn commit_view_proposes_according_to_what_is_half_done() {
        let (harness, list) = conflicting();
        let (path, data) = (
            harness.path().to_path_buf(),
            harness.data.path().to_path_buf(),
        );
        let proposed = || proposal_in_progress(&path, &data, true).unwrap();
        assert_eq!(proposed(), None);

        apply(&harness, Kind::CherryPick, std::slice::from_ref(&list[1])).unwrap();
        assert_eq!(proposed().as_deref(), Some("clashes\n\nwith body"));
        harness.git(&["cherry-pick", "--abort"]);

        // A merge half-done does not belong to this module: its message is git's.
        harness.git_fails(&["merge", "side"]);
        assert_eq!(proposed(), None);
    }

    #[test]
    fn commit_after_unresolved_conflicts_commits_nothing() {
        let (harness, list) = conflicting();
        let picked = list[1].clone();
        apply(&harness, Kind::CherryPick, std::slice::from_ref(&picked)).unwrap();
        let head = harness.head();

        let error = finish(&harness, Kind::CherryPick, &picked).unwrap_err();

        assert!(text(&error).contains("unmerged files"), "{error:?}");
        assert_eq!(harness.head(), head);
        assert!(harness.in_git_dir("CHERRY_PICK_HEAD"));
    }

    #[test]
    fn commit_after_conflicts_of_another_commit_commits_nothing() {
        let (harness, list) = conflicting();
        apply(&harness, Kind::CherryPick, std::slice::from_ref(&list[1])).unwrap();
        resolve(&harness, "resolved\n");
        let head = harness.head();

        let error = finish(&harness, Kind::CherryPick, &list[0]).unwrap_err();

        assert!(text(&error).contains("already been changed"), "{error:?}");
        assert_eq!(harness.head(), head);
        assert!(harness.in_git_dir("CHERRY_PICK_HEAD"));
    }

    #[test]
    fn commit_after_conflicts_with_nothing_in_progress_fails() {
        let (harness, list) = conflicting();

        let error = finish(&harness, Kind::CherryPick, &list[1]).unwrap_err();

        assert!(
            text(&error).contains("No cherry-pick in progress"),
            "{error:?}"
        );
    }

    #[test]
    fn resolved_same_as_head_is_skipped_and_cleaned() {
        let (harness, list) = conflicting();
        let picked = list[1].clone();
        apply(&harness, Kind::CherryPick, std::slice::from_ref(&picked)).unwrap();
        resolve(&harness, "main\n");
        let head = harness.head();

        let finished = finish(&harness, Kind::CherryPick, &picked).unwrap();

        assert_eq!(finished, Finished::Empty);
        assert_eq!(harness.head(), head);
        assert!(!harness.in_git_dir("CHERRY_PICK_HEAD"));
        assert!(!harness.in_git_dir("MERGE_MSG"));
    }

    #[test]
    fn revert_of_several_goes_from_the_most_recent() {
        let (harness, list) = picking();
        harness.git(&["checkout", "-q", "side"]);

        let result = apply(&harness, Kind::Revert, &list[..2]).unwrap();

        assert_eq!(result.applied, at(&list, &[0, 1]));
        let log = harness.git(&["log", "--format=%s", "-3"]);
        assert_eq!(
            log.lines().collect::<Vec<_>>(),
            ["Revert \"two\"", "Revert \"three\"", "three"]
        );
    }

    #[test]
    fn revert_with_conflict_proposes_revert_quotes() {
        let (harness, _) = conflicting();
        harness.repo.commit("a.txt", b"other\n", "again");
        let target = harness.git(&["rev-parse", "HEAD~1"]).trim().to_string();

        let result = apply(&harness, Kind::Revert, std::slice::from_ref(&target)).unwrap();

        assert_eq!(
            result.stopped,
            Some(Stop::Conflicts {
                hash: target.clone()
            })
        );
        assert!(harness.in_git_dir("REVERT_HEAD"));
        let expected = format!("Revert \"on main\"\n\nThis reverts commit {}", &target[..8]);
        assert_eq!(proposal(&harness, Kind::Revert, true).unwrap(), expected);
        resolve(&harness, "resolved\n");

        let finished = finish(&harness, Kind::Revert, &target).unwrap();

        assert_eq!(finished, Finished::Committed);
        assert_eq!(message_of(&harness, "HEAD"), expected);
        assert!(!harness.in_git_dir("REVERT_HEAD"));
    }

    #[test]
    fn revert_of_a_merge_is_rejected() {
        let (harness, _) = picking();
        harness.repo.commit("m.txt", b"m\n", "on main");
        harness.git(&["merge", "-q", "--no-ff", "-m", "join", "side"]);
        let merge = harness.head();

        let error = apply(&harness, Kind::Revert, std::slice::from_ref(&merge)).unwrap_err();

        assert!(
            text(&error).contains("Reverting merge commits is not allowed"),
            "{error:?}"
        );
        assert_eq!(harness.head(), merge);
        assert!(!harness.in_git_dir("REVERT_HEAD"));
    }

    #[test]
    fn revert_of_a_merge_among_others_applies_none() {
        let (harness, _) = picking();
        harness.repo.commit("m.txt", b"m\n", "on main");
        harness.git(&["merge", "-q", "--no-ff", "-m", "join", "side"]);
        let list = vec![
            harness.head(),
            harness.git(&["rev-parse", "HEAD~1"]).trim().to_string(),
        ];
        let head = harness.head();

        assert!(apply(&harness, Kind::Revert, &list).is_err());

        assert_eq!(harness.head(), head);
    }

    #[test]
    fn local_changes_that_would_be_overwritten_give_local_changes() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"base\n", "base");
        harness.git(&["checkout", "-q", "-b", "side"]);
        harness.repo.commit("a.txt", b"side\n", "changes a");
        harness.git(&["checkout", "-q", "main"]);
        harness.write("a.txt", "dirty\n");
        let list = hashes(&harness, "main..side");

        let result = apply(&harness, Kind::CherryPick, &list).unwrap();

        assert_eq!(
            result.stopped,
            Some(Stop::LocalChanges {
                hash: list[0].clone(),
                files: vec!["a.txt".to_string()]
            })
        );
        assert!(result.applied.is_empty());
        assert_eq!(harness.read("a.txt"), "dirty\n");
        assert!(!harness.in_git_dir("CHERRY_PICK_HEAD"));
    }

    #[test]
    fn an_untracked_file_that_would_be_overwritten_gives_untracked() {
        let (harness, list) = picking();
        harness.write("one.txt", "mine\n");

        let result = apply(&harness, Kind::CherryPick, &list).unwrap();

        assert_eq!(
            result.stopped,
            Some(Stop::Untracked {
                hash: list[2].clone(),
                files: vec!["one.txt".to_string()]
            })
        );
        assert_eq!(result.remaining, at(&list, &[0, 1]));
        assert_eq!(harness.read("one.txt"), "mine\n");
    }

    #[test]
    fn another_git_failure_gives_failed_with_its_text() {
        let harness = RepoHarness::new();
        harness.diverged();
        // An unresolved merge: git refuses to do a cherry-pick on top of it.
        harness.git_fails(&["merge", "side"]);
        let list = hashes(&harness, "main..side");

        let result = apply(&harness, Kind::CherryPick, &list).unwrap();

        let Some(Stop::Failed { hash, text }) = result.stopped else {
            panic!("{result:?}");
        };
        assert_eq!(hash, list[0]);
        assert!(text.contains("unmerged files"), "{text}");
        // What was already half-done is not ours: it is not touched.
        assert!(harness.in_git_dir("MERGE_HEAD"));
        assert!(!harness.in_git_dir("CHERRY_PICK_HEAD"));
    }

    #[test]
    fn a_hash_that_looks_like_an_option_or_does_not_exist_never_reaches_git() {
        let (harness, _) = picking();
        let head = harness.head();

        for bad in ["--abort", "-n", "", "0123456789abcdef"] {
            let error = apply(&harness, Kind::CherryPick, &[bad.to_string()]);
            assert!(error.is_err(), "{bad}");
        }

        assert_eq!(harness.head(), head);
        assert!(!harness.ran("cherry-pick --abort"));
    }

    #[test]
    fn a_head_that_changed_touches_nothing() {
        let (harness, list) = picking();

        let error = harness
            .run({
                let list = list.clone();
                move |repo| {
                    run(
                        repo,
                        Kind::CherryPick,
                        &list,
                        "0000000000000000000000000000000000000000",
                    )
                }
            })
            .unwrap_err();

        assert!(text(&error).contains("already been changed"), "{error:?}");
        assert!(!harness.ran(&format!("cherry-pick {}", list[0])));
    }

    #[test]
    fn a_repeated_hash_is_applied_once() {
        let (harness, list) = picking();

        let result = apply(
            &harness,
            Kind::CherryPick,
            &[list[2].clone(), list[2].clone()],
        )
        .unwrap();

        assert_eq!(result.applied, at(&list, &[2]));
    }

    // Ten lines, so that a change in 2 and another in 5 end up in different stretches of the merge
    // but within the same context of the patch.
    fn numbered() -> String {
        (1..=10).map(|n| format!("l{n}\n")).collect()
    }

    fn with(from: &str, to: &str) -> String {
        numbered().replace(&format!("{from}\n"), &format!("{to}\n"))
    }

    // `main` at `g` (f.txt and g.txt) and `side` on top with `changes`: line 2 of f.txt, g.txt and a new
    // n.txt. It stays on `main` and returns the hash of `changes`.
    fn selecting() -> (RepoHarness, String) {
        let harness = RepoHarness::new();
        harness.repo.commit("f.txt", numbered().as_bytes(), "base");
        harness.repo.commit("g.txt", b"g\n", "g");
        harness.git(&["checkout", "-q", "-b", "side"]);
        harness.write("f.txt", &with("l2", "two"));
        harness.write("g.txt", "g side\n");
        harness.write("n.txt", "new\n");
        harness.git(&["add", "-A"]);
        harness.git(&["commit", "-q", "-m", "changes"]);
        let hash = harness.head();
        harness.git(&["checkout", "-q", "main"]);
        (harness, hash)
    }

    fn strings(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|path| path.to_string()).collect()
    }

    fn select(
        harness: &RepoHarness,
        hash: &str,
        paths: &[&str],
        reverse: bool,
    ) -> Result<PathsResult, GitError> {
        let (hash, paths) = (hash.to_string(), strings(paths));
        harness.run(move |repo| apply_paths(repo, &hash, &paths, reverse))
    }

    fn staged_names(harness: &RepoHarness) -> String {
        harness.git(&["diff", "--cached", "--name-only"])
    }

    #[test]
    fn applying_paths_leaves_local_changes_unstaged() {
        let (harness, hash) = selecting();
        let head = harness.head();

        let result = select(&harness, &hash, &["f.txt", "n.txt"], false).unwrap();

        assert_eq!(result, PathsResult::Applied);
        assert_eq!(harness.read("f.txt"), with("l2", "two"));
        assert_eq!(harness.read("n.txt"), "new\n");
        // What was not chosen is not touched.
        assert_eq!(harness.read("g.txt"), "g\n");
        assert_eq!(staged_names(&harness), "");
        assert_eq!(harness.head(), head);
        assert_eq!(
            harness.git(&["status", "--porcelain"]),
            " M f.txt\n?? n.txt\n"
        );
        assert!(harness.ran("apply --whitespace=nowarn"));
        assert!(!harness.ran("apply --whitespace=nowarn --3way"));
    }

    #[test]
    fn reverting_paths_uses_minus_r() {
        let (harness, hash) = selecting();
        harness.git(&["checkout", "-q", "side"]);
        let head = harness.head();

        let result = select(&harness, &hash, &["f.txt", "n.txt"], true).unwrap();

        assert_eq!(result, PathsResult::Applied);
        assert_eq!(harness.read("f.txt"), numbered());
        assert!(!harness.path().join("n.txt").exists());
        // What was not chosen stays as the commit left it.
        assert_eq!(harness.read("g.txt"), "g side\n");
        assert_eq!(staged_names(&harness), "");
        assert_eq!(harness.head(), head);
        assert!(harness.ran("apply --whitespace=nowarn -R"));
    }

    #[test]
    fn if_it_does_not_apply_cleanly_it_retries_with_3way_and_leaves_a_conflict() {
        let (harness, hash) = selecting();
        harness
            .repo
            .commit("f.txt", with("l2", "main").as_bytes(), "on main");

        let result = select(&harness, &hash, &["f.txt", "g.txt"], false).unwrap();

        assert_eq!(result, PathsResult::Conflicts);
        assert!(harness.ran("apply --whitespace=nowarn"));
        assert!(harness.ran("apply --whitespace=nowarn --3way"));
        // The conflict is left with its three stages and the marks in the file.
        let stages = harness.git(&["ls-files", "-u", "f.txt"]);
        assert_eq!(stages.lines().count(), 3, "{stages}");
        assert!(harness.read("f.txt").contains("<<<<<<<"));
        // What did go in is taken out of the stage; the conflict is not.
        assert_eq!(harness.read("g.txt"), "g side\n");
        assert_eq!(
            harness.git(&["status", "--porcelain"]),
            "UU f.txt\n M g.txt\n"
        );
    }

    #[test]
    fn with_a_clean_3way_the_changes_are_left_unstaged() {
        let (harness, hash) = selecting();
        harness
            .repo
            .commit("f.txt", with("l5", "five").as_bytes(), "on main");

        let result = select(&harness, &hash, &["f.txt", "n.txt"], false).unwrap();

        assert_eq!(result, PathsResult::Applied);
        assert!(harness.ran("apply --whitespace=nowarn --3way"));
        let merged = with("l2", "two").replace("l5\n", "five\n");
        assert_eq!(harness.read("f.txt"), merged);
        assert_eq!(staged_names(&harness), "");
        assert_eq!(
            harness.git(&["status", "--porcelain"]),
            " M f.txt\n?? n.txt\n"
        );
        assert_eq!(harness.snapshots(), 0);
    }

    #[test]
    fn what_was_already_staged_stays_staged_and_is_copied() {
        let (harness, hash) = selecting();
        harness
            .repo
            .commit("f.txt", with("l5", "five").as_bytes(), "on main");
        harness.write("f.txt", &with("l5", "five").replace("l9\n", "nine\n"));
        harness.git(&["add", "f.txt"]);

        let result = select(&harness, &hash, &["f.txt"], false).unwrap();

        assert_eq!(result, PathsResult::Applied);
        assert_eq!(staged_names(&harness), "f.txt\n");
        let text = harness.read("f.txt");
        assert!(text.contains("two") && text.contains("five") && text.contains("nine"));
        // The recoverable copy of what `--3way` rewrites.
        assert_eq!(harness.saved_paths(), ["f.txt"]);
    }

    #[test]
    fn an_unstaged_local_change_is_not_overwritten() {
        let (harness, hash) = selecting();
        harness.write("f.txt", &with("l2", "local"));

        let result = select(&harness, &hash, &["f.txt"], false).unwrap();

        let PathsResult::Failed(text) = result else {
            panic!("it should have failed: {result:?}");
        };
        assert!(text.contains("f.txt"), "{text}");
        assert_eq!(harness.read("f.txt"), with("l2", "local"));
        assert_eq!(staged_names(&harness), "");
        assert_eq!(harness.snapshots(), 0);
    }

    #[test]
    fn chosen_paths_are_literals_not_options() {
        let (harness, hash) = selecting();
        harness.git(&["checkout", "-q", "side"]);
        harness.repo.commit("-r.txt", b"dash\n", "dash");
        let dashed = harness.head();
        harness.git(&["checkout", "-q", "main"]);

        let result = select(&harness, &dashed, &["-r.txt"], false).unwrap();
        assert_eq!(result, PathsResult::Applied);
        assert_eq!(harness.read("-r.txt"), "dash\n");

        // A pattern matches nothing: without a patch there is nothing to apply.
        let result = select(&harness, &hash, &["*.txt"], false).unwrap();
        assert!(matches!(result, PathsResult::Failed(_)), "{result:?}");
        assert_eq!(harness.read("f.txt"), numbered());
    }

    #[test]
    fn a_hash_that_is_not_a_commit_touches_nothing() {
        let (harness, _) = selecting();

        for hash in [
            "-x",
            "",
            "0000000000000000000000000000000000000000",
            "does-not-exist",
        ] {
            let error = select(&harness, hash, &["f.txt"], false).unwrap_err();
            assert!(!text(&error).is_empty());
        }

        assert_eq!(harness.git(&["status", "--porcelain"]), "");
    }

    #[test]
    fn a_rename_is_applied_with_both_paths() {
        let (harness, _) = selecting();
        harness.git(&["checkout", "-q", "-b", "moves"]);
        harness.git(&["mv", "g.txt", "h.txt"]);
        harness.git(&["commit", "-q", "-m", "moves"]);
        let moved = harness.head();
        harness.git(&["checkout", "-q", "main"]);

        let result = select(&harness, &moved, &["h.txt", "g.txt"], false).unwrap();

        assert_eq!(result, PathsResult::Applied);
        assert_eq!(harness.read("h.txt"), "g\n");
        assert!(!harness.path().join("g.txt").exists());
    }

    #[test]
    fn a_binary_file_goes_in_byte_by_byte() {
        let (harness, _) = selecting();
        harness.git(&["checkout", "-q", "side"]);
        let bytes: Vec<u8> = (0..=255u8).chain([0, 255, 0]).collect();
        harness.repo.commit("b.bin", &bytes, "binary");
        let hash = harness.head();
        harness.git(&["checkout", "-q", "main"]);

        let result = select(&harness, &hash, &["b.bin"], false).unwrap();

        assert_eq!(result, PathsResult::Applied);
        assert_eq!(std::fs::read(harness.path().join("b.bin")).unwrap(), bytes);
    }

    #[test]
    fn reverting_a_root_commit_deletes_what_it_created() {
        let harness = RepoHarness::new();
        let root = harness.repo.commit("r.txt", b"root\n", "root");
        harness.repo.commit("s.txt", b"s\n", "another");

        let result = select(&harness, &root, &["r.txt"], true).unwrap();

        assert_eq!(result, PathsResult::Applied);
        assert!(!harness.path().join("r.txt").exists());
        assert!(harness.path().join("s.txt").exists());
    }

    #[test]
    fn each_file_is_compared_with_its_base() {
        // Two commits: the first touches f.txt and the second g.txt; one file of each is chosen.
        let harness = RepoHarness::new();
        harness.repo.commit("f.txt", numbered().as_bytes(), "base");
        harness.repo.commit("g.txt", b"g\n", "g");
        harness.git(&["checkout", "-q", "-b", "side"]);
        let first = harness
            .repo
            .commit("f.txt", with("l2", "two").as_bytes(), "one");
        let second = harness.repo.commit("g.txt", b"g side\n", "two");
        harness.git(&["checkout", "-q", "main"]);
        let head = harness.head();
        let selection = vec![
            Selection {
                hash: first.clone(),
                base: Some(
                    harness
                        .git(&["rev-parse", &format!("{first}^")])
                        .trim()
                        .to_string(),
                ),
                paths: strings(&["f.txt"]),
            },
            Selection {
                hash: second,
                base: Some(first),
                paths: strings(&["g.txt"]),
            },
        ];

        let result = harness
            .run(move |repo| apply_selection(repo, &head, &selection, false))
            .unwrap();

        assert_eq!(result, PathsResult::Applied);
        assert_eq!(harness.read("f.txt"), with("l2", "two"));
        assert_eq!(harness.read("g.txt"), "g side\n");
    }

    #[test]
    fn a_head_that_changed_in_apply_paths_touches_nothing() {
        let (harness, hash) = selecting();
        let selection = vec![Selection {
            hash,
            base: None,
            paths: strings(&["f.txt"]),
        }];

        let error = harness
            .run(move |repo| {
                let head = "0000000000000000000000000000000000000000";
                apply_selection(repo, head, &selection, false)
            })
            .unwrap_err();

        assert!(text(&error).contains("already been changed"), "{error:?}");
        assert_eq!(harness.git(&["status", "--porcelain"]), "");
        assert!(!harness.ran("apply --whitespace=nowarn"));
    }

    // Plain `apply` fails (line 5 changed in `main`) and `--3way` would be tried; what is unstaged
    // in the file must stop it beforehand.
    fn refuses_local(change: impl FnOnce(&RepoHarness), expected: impl FnOnce(&RepoHarness)) {
        let (harness, hash) = selecting();
        harness
            .repo
            .commit("f.txt", with("l5", "five").as_bytes(), "on main");
        change(&harness);
        let status = harness.git(&["status", "--porcelain"]);

        let result = select(&harness, &hash, &["f.txt"], false).unwrap();

        let PathsResult::Failed(text) = result else {
            panic!("it should have failed: {result:?}");
        };
        assert!(text.contains("f.txt"), "{text}");
        assert!(!harness.ran("apply --whitespace=nowarn --3way"));
        assert_eq!(harness.git(&["status", "--porcelain"]), status);
        assert_eq!(harness.snapshots(), 0);
        expected(&harness);
    }

    #[test]
    fn an_unstaged_local_deletion_is_not_undone() {
        refuses_local(
            |harness| std::fs::remove_file(harness.path().join("f.txt")).unwrap(),
            |harness| assert!(!harness.path().join("f.txt").exists()),
        );
    }

    #[test]
    fn an_unstaged_local_edit_does_not_go_to_3way() {
        refuses_local(
            |harness| harness.write("f.txt", &with("l5", "five").replace("l9\n", "local\n")),
            |harness| assert!(harness.read("f.txt").contains("local")),
        );
    }

    #[test]
    fn an_unstaged_local_mode_change_does_not_go_to_3way() {
        use std::os::unix::fs::PermissionsExt;
        refuses_local(
            |harness| {
                let path = harness.path().join("f.txt");
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
            },
            |harness| {
                let mode = std::fs::metadata(harness.path().join("f.txt")).unwrap();
                assert_eq!(mode.permissions().mode() & 0o111, 0o111);
            },
        );
    }

    #[test]
    fn an_unstaged_local_type_change_does_not_go_to_3way() {
        refuses_local(
            |harness| {
                std::fs::remove_file(harness.path().join("f.txt")).unwrap();
                std::os::unix::fs::symlink("g.txt", harness.path().join("f.txt")).unwrap();
            },
            |harness| {
                let meta = std::fs::symlink_metadata(harness.path().join("f.txt")).unwrap();
                assert!(meta.file_type().is_symlink());
            },
        );
    }

    #[test]
    fn without_a_base_the_old_side_is_the_empty_tree() {
        // A root that creates r.txt and a later commit that edits it: the panel, with both chosen,
        // shows r.txt as added, with the root's base (null) and the later one's hash.
        let harness = RepoHarness::new();
        harness.repo.commit("r.txt", b"one\n", "root");
        let later = harness.repo.commit("r.txt", b"two\n", "edits");
        let head = harness.head();
        let selection = vec![Selection {
            hash: later,
            base: None,
            paths: strings(&["r.txt"]),
        }];

        let result = harness
            .run(move |repo| apply_selection(repo, &head, &selection, true))
            .unwrap();

        assert_eq!(result, PathsResult::Applied);
        assert!(!harness.path().join("r.txt").exists());
    }
}
