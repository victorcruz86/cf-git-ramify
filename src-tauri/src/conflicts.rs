// The conflicts of a merge, a rebase or a cherry-pick: which ones there are, the three versions of each
// one, accepting a side and saving a result. Ported from `GitMergeProvider`, `GitMergeUtil`,
// `GitMergeHandler`, `GitDefaultMergeDialogCustomizer` and `GitRebaseMergeDialogCustomizer`.
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::branches::{self, invalid, Repo};
use crate::git::{self, GitError};
use crate::merge3::{self, Block, Whitespace};
use crate::operation::{self, Operation};
use crate::snapshots;
use crate::storage;
use crate::task::TaskContext;
use crate::worktree_diff;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SideStatus {
    Modified,
    Deleted,
}

// The texts of `conflicts.type.*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Kind {
    #[serde(rename = "both deleted")]
    BothDeleted,
    #[serde(rename = "both added")]
    BothAdded,
    #[serde(rename = "both modified")]
    BothModified,
    #[serde(rename = "deleted by you")]
    DeletedByYou,
    #[serde(rename = "deleted by them")]
    DeletedByThem,
    #[serde(rename = "added by you")]
    AddedByYou,
    #[serde(rename = "added by them")]
    AddedByThem,
}

// Already in the user's terms: in a rebase `yours` is the branch being rebased.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Conflict {
    pub path: String,
    pub yours: SideStatus,
    pub theirs: SideStatus,
    pub kind: Kind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub conflicts: Vec<Conflict>,
    // A rebase: `yours` is git's stage 3.
    pub reversed: bool,
    pub yours_column: String,
    pub theirs_column: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "text", rename_all = "camelCase")]
pub enum Version {
    Text(String),
    Binary,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sides {
    pub path: String,
    pub base: Version,
    pub yours: Version,
    pub theirs: Version,
    // Left, center and right of the merge window.
    pub titles: [String; 3],
    // SHA-256 of the working file when read; empty if it does not exist.
    pub hash: String,
    pub reversed: bool,
    // The `merge3` blocks with `Whitespace::Default`; a missing version counts as
    // empty, and with a binary there are no blocks.
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Side {
    Yours,
    Theirs,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictError {
    // The file changed on disk after it was read.
    Outdated,
    Git(GitError),
}

impl From<GitError> for ConflictError {
    fn from(error: GitError) -> Self {
        Self::Git(error)
    }
}

impl From<ConflictError> for GitError {
    fn from(error: ConflictError) -> Self {
        match error {
            ConflictError::Outdated => GitError::Outdated,
            ConflictError::Git(error) => error,
        }
    }
}

// Which stages each unresolved file has: the base, `--ours` and `--theirs`.
const BASE: u8 = 1;
const OURS: u8 = 2;
const THEIRS: u8 = 4;
const CHERRY_PICK: &str = "cherry-pick";
const RESULT: &str = "Result";
// `DvcsUtil.getShortHash`.
const SHORT_HASH: usize = 8;
const ACCEPT_YOURS: &str = "Accept Yours";
const ACCEPT_THEIRS: &str = "Accept Theirs";
const RESOLVE: &str = "Resolve Conflict";
// Outside a git operation that knows them, `MergeDialogCustomizer` uses its own.
const DEFAULT_TITLES: [&str; 3] = ["Your version", RESULT, "Changes from server"];
// What `GitBundle` says when git does not let us see more.
const NOT_IN_CONFLICT: &str = "is not in conflict";

// What each side did with the file, like the two letters of `git status`'s XY.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    Added,
    Modified,
    Deleted,
}

// From the XY that git gives to each mix of stages: `DD` only the base, `AU` only `ours`, `UD` the base and
// `ours`, `UA` only `theirs`, `DU` the base and `theirs`, `AA` both and `UU` all three. The second
// letter of `AU` and `UA` is `U`: for `GitStagingAreaHolder`, modified.
fn changes(mask: u8) -> (Change, Change) {
    use Change::{Added, Deleted, Modified};
    match mask {
        BASE => (Deleted, Deleted),
        OURS => (Added, Modified),
        m if m == BASE | OURS => (Modified, Deleted),
        THEIRS => (Modified, Added),
        m if m == BASE | THEIRS => (Deleted, Modified),
        m if m == OURS | THEIRS => (Added, Added),
        _ => (Modified, Modified),
    }
}

// `GitConflictsUtil.getConflictType`, in the order of its checks.
fn kind_of(yours: Change, theirs: Change) -> Kind {
    use Change::{Added, Deleted, Modified};
    match (yours, theirs) {
        (Deleted, Deleted) => Kind::BothDeleted,
        (Added, Added) => Kind::BothAdded,
        (Modified, Modified) => Kind::BothModified,
        (Deleted, _) => Kind::DeletedByYou,
        (_, Deleted) => Kind::DeletedByThem,
        (Added, _) => Kind::AddedByYou,
        (_, Added) => Kind::AddedByThem,
    }
}

// `MyMergeSession`: a column says "Deleted" when its stage is absent, and "Modified" otherwise.
fn status_of(mask: u8, stage: u8) -> SideStatus {
    if mask & stage == 0 {
        SideStatus::Deleted
    } else {
        SideStatus::Modified
    }
}

fn conflict_of(path: String, mask: u8, reversed: bool) -> Conflict {
    let (ours, theirs) = changes(mask);
    let (yours_change, theirs_change, yours_stage, theirs_stage) = if reversed {
        (theirs, ours, THEIRS, OURS)
    } else {
        (ours, theirs, OURS, THEIRS)
    };
    Conflict {
        path,
        yours: status_of(mask, yours_stage),
        theirs: status_of(mask, theirs_stage),
        kind: kind_of(yours_change, theirs_change),
    }
}

fn bad_path(error: std::io::Error) -> GitError {
    invalid(error.to_string())
}

fn not_utf8() -> GitError {
    invalid("The index contains a path that is not UTF-8".to_string())
}

// The stages of each unresolved file (`ls-files --unmerged`).
fn stages(repo: &Path) -> Result<BTreeMap<String, u8>, GitError> {
    let output = git::run(repo, &git::exact(&["ls-files", "--unmerged", "-z"]))?;
    let mut found = BTreeMap::new();
    for record in output.split(|byte| *byte == 0).filter(|r| !r.is_empty()) {
        let Some(tab) = record.iter().position(|byte| *byte == b'\t') else {
            continue;
        };
        let header = String::from_utf8_lossy(&record[..tab]);
        let stage: u32 = header.rsplit(' ').next().unwrap_or("").parse().unwrap_or(0);
        let path = String::from_utf8(record[tab + 1..].to_vec()).map_err(|_| not_utf8())?;
        let bit = match stage {
            1 => BASE,
            2 => OURS,
            3 => THEIRS,
            _ => continue,
        };
        *found.entry(path).or_insert(0) |= bit;
    }
    Ok(found)
}

// The merge window checks it with every notice from the watcher: someone may have marked it resolved elsewhere.
pub fn is_unmerged(repo: &Path, path: &str) -> Result<bool, GitError> {
    Ok(stages(repo)?.contains_key(path))
}

fn short(hash: &str) -> String {
    hash.chars().take(SHORT_HASH).collect()
}

// The commit that `name` points to, or nothing if it does not exist.
fn commit_of(repo: &Path, name: &str) -> Result<Option<String>, GitError> {
    let target = format!("{name}^{{commit}}");
    match git::run(repo, &["rev-parse", "-q", "--verify", &target]) {
        Ok(output) => Ok(Some(String::from_utf8_lossy(&output).trim().to_string())),
        Err(GitError::Failed { code: Some(1), .. }) => Ok(None),
        Err(error) => Err(error),
    }
}

// `GitHistoryUtils.getMergeBase`: without it, the titles are the usual ones.
fn has_merge_base(repo: &Path, one: &str, other: &str) -> Result<bool, GitError> {
    match git::run(repo, &["merge-base", one, other]) {
        Ok(_) => Ok(true),
        Err(GitError::Failed { code: Some(1), .. }) => Ok(false),
        Err(error) => Err(error),
    }
}

// `resolveBranchName`: the local branch that points there if there is only one; without local branches, the remote one.
fn branch_at(repo: &Path, hash: &str) -> Result<Option<String>, GitError> {
    for prefix in [LOCAL, REMOTE] {
        let args = [
            "for-each-ref",
            "--points-at",
            hash,
            "--format=%(refname)",
            prefix,
        ];
        let output = git::run(repo, &args)?;
        let text = String::from_utf8_lossy(&output);
        let names: Vec<&str> = text
            .lines()
            .filter_map(|line| line.strip_prefix(prefix))
            .filter(|name| !name.ends_with("/HEAD"))
            .collect();
        if !names.is_empty() {
            return Ok(match names[..] {
                [only] => Some(only.to_string()),
                _ => None,
            });
        }
    }
    Ok(None)
}

const LOCAL: &str = "refs/heads/";
const REMOTE: &str = "refs/remotes/";

// The other side of the operation: a commit and, if there is one, the branch that points to it.
struct Other {
    hash: String,
    branch: Option<String>,
}

impl Other {
    fn presentable(&self) -> String {
        self.branch.clone().unwrap_or_else(|| short(&self.hash))
    }
}

struct Picked {
    author: String,
    subject: String,
}

// Everything that the column texts, the description and the titles need to know.
struct Flow {
    operation: Option<Operation>,
    // The user's branch or, without it, its short commit.
    current: Option<String>,
    head: String,
    other: Option<Other>,
    // `REBASE_HEAD`: the commit being applied.
    rebase_head: Option<String>,
    picked: Option<Picked>,
    // `REVERT_HEAD`: the commit being undone, with its author and subject.
    reverted: Option<(String, Picked)>,
}

fn flow(repo: &Path) -> Result<Flow, GitError> {
    let state = operation::state(repo)?;
    let current = state
        .branch
        .clone()
        .or_else(|| Some(short(&state.head)).filter(|head| !head.is_empty()));
    let found = |hash: String| -> Result<Other, GitError> {
        let branch = branch_at(repo, &hash)?;
        Ok(Other { hash, branch })
    };
    let mut flow = Flow {
        operation: state.operation,
        current,
        head: state.head,
        other: None,
        rebase_head: None,
        picked: None,
        reverted: None,
    };
    match flow.operation {
        Some(Operation::Merging) => {
            flow.other = commit_of(repo, "MERGE_HEAD")?.map(found).transpose()?;
        }
        Some(Operation::Rebasing) => {
            flow.other = onto_of(repo)?.map(found).transpose()?;
            flow.rebase_head = commit_of(repo, "REBASE_HEAD")?;
        }
        Some(Operation::CherryPicking) => {
            if let Some(hash) = commit_of(repo, "CHERRY_PICK_HEAD")? {
                flow.picked = picked_of(repo, &hash)?;
                flow.other = Some(Other { hash, branch: None });
            }
        }
        Some(Operation::Reverting) => {
            if let Some(hash) = commit_of(repo, "REVERT_HEAD")? {
                flow.reverted = picked_of(repo, &hash)?.map(|picked| (hash, picked));
            }
        }
        None => {}
    }
    Ok(flow)
}

// `GitRebaseUtils.getOntoHash`.
fn onto_of(repo: &Path) -> Result<Option<String>, GitError> {
    for dir in ["rebase-merge", "rebase-apply"] {
        let path = git::git_path(repo, &format!("{dir}/onto"))?;
        if let Ok(text) = fs::read_to_string(path) {
            let hash = text.trim();
            if !hash.is_empty() && !hash.starts_with('-') {
                return commit_of(repo, hash);
            }
        }
    }
    Ok(None)
}

fn picked_of(repo: &Path, hash: &str) -> Result<Option<Picked>, GitError> {
    let args = ["show", "-s", "--format=%an%x00%s", hash];
    let output = git::run(repo, &args)?;
    let text = String::from_utf8_lossy(&output);
    Ok(text
        .trim_end_matches('\n')
        .split_once('\0')
        .map(|(author, subject)| Picked {
            author: author.to_string(),
            subject: subject.to_string(),
        }))
}

impl Flow {
    // `GitMergeProvider.calcColumnName`.
    fn yours_column(&self) -> String {
        match &self.current {
            Some(name) => format!("Yours ({name})"),
            None => "Yours".to_string(),
        }
    }

    fn theirs_column(&self) -> String {
        let name = match self.operation {
            Some(Operation::CherryPicking) => self.other.as_ref().map(|_| CHERRY_PICK.to_string()),
            _ => self.other.as_ref().map(Other::presentable),
        };
        match name {
            Some(name) => format!("Theirs ({name})"),
            None => "Theirs".to_string(),
        }
    }

    // `getMultipleFileMergeDescription` and `getDescriptionForRebase`.
    fn description(&self) -> String {
        let current = self.current.clone().unwrap_or_default();
        match (self.operation, &self.other) {
            (Some(Operation::Merging), Some(other)) => {
                format!(
                    "Merging branch {} into branch {current}",
                    other.presentable()
                )
            }
            (Some(Operation::Rebasing), other) => {
                let rebasing = self
                    .current
                    .as_ref()
                    .map(|name| format!(" branch {name}"))
                    .unwrap_or_default();
                match other {
                    Some(Other {
                        branch: Some(branch),
                        ..
                    }) => {
                        format!("Rebasing{rebasing} onto branch {branch}")
                    }
                    Some(other) => format!("Rebasing{rebasing} onto {}", short(&other.hash)),
                    None => format!("Rebasing{rebasing} onto diverging branches"),
                }
            }
            (Some(Operation::CherryPicking), Some(other)) => {
                let head = format!(
                    "Conflicts during cherry-picking commit {}",
                    short(&other.hash)
                );
                match &self.picked {
                    Some(picked) => {
                        format!("{head} made by {}\n{}", picked.author, picked.subject)
                    }
                    None => head,
                }
            }
            // `apply.conflict.dialog.description.label.text`, which the revert takes from
            // `GitApplyChangesProcess`.
            (Some(Operation::Reverting), _) => match &self.reverted {
                Some((hash, picked)) => format!(
                    "Conflicts during revert of commit {} made by {}\n{}",
                    short(hash),
                    picked.author,
                    picked.subject
                ),
                None => String::new(),
            },
            _ => String::new(),
        }
    }

    // `getTitleCustomizerList`: without what it needs, the titles of `MergeDialogCustomizer`.
    fn titles(&self, repo: &Path) -> Result<[String; 3], GitError> {
        let defaults = DEFAULT_TITLES.map(str::to_string);
        let (Some(other), Some(current)) = (&self.other, &self.current) else {
            return Ok(defaults);
        };
        let titles = |left: String, right: String| [left, RESULT.to_string(), right];
        match self.operation {
            Some(Operation::Merging) if has_merge_base(repo, &self.head, &other.hash)? => {
                Ok(titles(
                    format!("Changes from {current}"),
                    format!("Changes from {}", other.presentable()),
                ))
            }
            Some(Operation::Rebasing) => {
                let Some(applied) = &self.rebase_head else {
                    return Ok(defaults);
                };
                if !has_merge_base(repo, applied, &other.hash)? {
                    return Ok(defaults);
                }
                let right = match &other.branch {
                    Some(branch) => format!("Already rebased commits and commits from {branch}"),
                    None => "Already rebased commits".to_string(),
                };
                Ok(titles(
                    format!("Rebasing {} from {current}", short(applied)),
                    right,
                ))
            }
            Some(Operation::CherryPicking) if has_merge_base(repo, &other.hash, &self.head)? => {
                Ok(titles(
                    "Local Changes".to_string(),
                    format!("Changes from cherry-pick {}", short(&other.hash)),
                ))
            }
            _ => Ok(defaults),
        }
    }
}

pub fn session(ctx: &TaskContext) -> Result<Session, GitError> {
    let repo = ctx.repo();
    let flow = flow(repo)?;
    let reversed = flow.operation == Some(Operation::Rebasing);
    let conflicts = stages(repo)?
        .into_iter()
        .map(|(path, mask)| conflict_of(path, mask, reversed))
        .collect();
    Ok(Session {
        conflicts,
        reversed,
        yours_column: flow.yours_column(),
        theirs_column: flow.theirs_column(),
        description: flow.description(),
    })
}

// A file that is not text that can be edited and saved again without changing its bytes.
fn version_of(bytes: Vec<u8>) -> Version {
    if bytes.contains(&0) {
        return Version::Binary;
    }
    match String::from_utf8(bytes) {
        Ok(text) => Version::Text(text),
        Err(_) => Version::Binary,
    }
}

// What the working file has now, or nothing if it does not exist.
fn read_disk(repo: &Path, path: &str) -> Result<Option<Vec<u8>>, GitError> {
    worktree_diff::check_inside(repo, path).map_err(|error| invalid(error.to_string()))?;
    match fs::read(repo.join(path)) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(git::io_error(error)),
    }
}

fn hash_of(disk: &Option<Vec<u8>>) -> String {
    disk.as_deref()
        .map(worktree_diff::content_hash)
        .unwrap_or_default()
}

// The errors of `loadRevisionCatchingErrors`: the stage is not there.
fn is_missing(error: &GitError) -> bool {
    let GitError::Failed { stderr, .. } = error else {
        return false;
    };
    let message = stderr.trim();
    message.starts_with("fatal: ambiguous argument ")
        || (message.starts_with("fatal: Path '")
            && message.contains("' exists on disk, but not in '"))
        || message.contains("is in the index, but not at stage ")
        || message.contains("bad revision")
        || message.starts_with("fatal: Not a valid object name")
}

fn read_stage(repo: &Path, mask: u8, stage: u8, path: &str) -> Result<Option<Vec<u8>>, GitError> {
    let (bit, number) = match stage {
        BASE => (BASE, 1),
        OURS => (OURS, 2),
        _ => (THEIRS, 3),
    };
    if mask & bit == 0 {
        return Ok(None);
    }
    let object = format!(":{number}:{path}");
    match git::run(repo, &git::exact(&["cat-file", "-p", &object])) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if is_missing(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

// The text the blocks are computed with. `GitMergeUtil.loadRevisionCatchingErrors` reads a
// missing stage as empty; a binary has no blocks.
fn text_of(version: &Version) -> Option<&str> {
    match version {
        Version::Text(text) => Some(text),
        Version::Missing => Some(""),
        Version::Binary => None,
    }
}

pub fn sides(ctx: &TaskContext, path: &str) -> Result<Sides, GitError> {
    let repo = ctx.repo();
    snapshots::check_path(repo, path).map_err(bad_path)?;
    let flow = flow(repo)?;
    let reversed = flow.operation == Some(Operation::Rebasing);
    let Some(mask) = stages(repo)?.get(path).copied() else {
        return Err(invalid(format!("\"{path}\" {NOT_IN_CONFLICT}")));
    };
    // The hash is that of the file seen here: what is read afterwards does not count for it.
    let disk = read_disk(repo, path)?;
    let hash = hash_of(&disk);
    let read = |stage: u8| -> Result<Version, GitError> {
        ctx.check_cancelled()?;
        Ok(match read_stage(repo, mask, stage, path)? {
            Some(bytes) => version_of(bytes),
            None => Version::Missing,
        })
    };
    // `loadOriginalContent`: without a base the working file is used as it is.
    let base = match read(BASE)? {
        Version::Missing => version_of(disk.unwrap_or_default()),
        found => found,
    };
    let (yours_stage, theirs_stage) = if reversed {
        (THEIRS, OURS)
    } else {
        (OURS, THEIRS)
    };
    let yours = read(yours_stage)?;
    let theirs = read(theirs_stage)?;
    ctx.check_cancelled()?;
    let blocks = match (text_of(&yours), text_of(&base), text_of(&theirs)) {
        (Some(left), Some(middle), Some(right)) => {
            merge3::blocks(left, middle, right, Whitespace::Default)
        }
        _ => Vec::new(),
    };
    Ok(Sides {
        path: path.to_string(),
        base,
        yours,
        theirs,
        titles: flow.titles(repo)?,
        hash,
        reversed,
        blocks,
    })
}

// Git commands over many paths, in batches and with the paths as literals.
fn each_chunk(ctx: &TaskContext, head: &[&str], paths: &[String]) -> Result<(), GitError> {
    for chunk in paths.chunks(git::PATHS_PER_COMMAND) {
        ctx.check_cancelled()?;
        let mut args = vec!["--literal-pathspecs", "-c", "core.precomposeunicode=false"];
        args.extend_from_slice(head);
        args.push("--");
        args.extend(chunk.iter().map(String::as_str));
        ctx.git_to_the_end(&args)?;
    }
    Ok(())
}

// Of what was requested, what is still unresolved and exists on disk: the only thing a copy can lose.
fn on_disk(root: &Path, paths: &[String]) -> Vec<String> {
    paths
        .iter()
        .filter(|path| root.join(path).symlink_metadata().is_ok())
        .cloned()
        .collect()
}

// `side` is the user's, as seen in the window: in a rebase `yours` is `--theirs` for git.
// This function is the only place where it is inverted. What is no longer in conflict is left alone:
// `checkout --ours` on it would fail without touching anything, and someone resolved it in the meantime.
pub fn accept(repo: &Repo, paths: &[String], side: Side) -> Result<(), GitError> {
    for path in paths {
        snapshots::check_path(repo.path, path).map_err(bad_path)?;
    }
    let found = stages(repo.path)?;
    let mut targets: Vec<String> = Vec::new();
    for path in paths {
        if found.contains_key(path) && !targets.contains(path) {
            targets.push(path.clone());
        }
    }
    if targets.is_empty() {
        return Ok(());
    }
    let reversed = operation::current(repo.path)? == Some(Operation::Rebasing);
    let ours = (side == Side::Yours) != reversed;
    let (flag, stage) = if ours {
        ("--ours", OURS)
    } else {
        ("--theirs", THEIRS)
    };
    // `acceptOneVersion` leaves the file to whoever deleted it with `git rm`, and copies the rest.
    let (keep, remove): (Vec<String>, Vec<String>) = targets
        .iter()
        .cloned()
        .partition(|path| found[path] & stage != 0);
    let reason = if side == Side::Yours {
        ACCEPT_YOURS
    } else {
        ACCEPT_THEIRS
    };
    branches::copy_before(repo, reason, |root| Ok(on_disk(root, &targets)))?;
    each_chunk(repo.ctx, &["checkout", flag], &keep)?;
    each_chunk(repo.ctx, &["add", "--ignore-errors", "-A", "-f"], &keep)?;
    each_chunk(repo.ctx, &["rm"], &remove)
}

// Saves `content` as the result of `path` and marks it resolved. `expected_hash` is that of `sides`:
// if the file changed since then, nothing is written.
pub fn resolve(
    repo: &Repo,
    path: &str,
    content: &[u8],
    expected_hash: &str,
) -> Result<(), ConflictError> {
    snapshots::check_path(repo.path, path).map_err(bad_path)?;
    if !stages(repo.path)?.contains_key(path) {
        return Err(invalid(format!("\"{path}\" {NOT_IN_CONFLICT}")).into());
    }
    let unchanged = || -> Result<Option<Option<Vec<u8>>>, GitError> {
        let disk = read_disk(repo.path, path)?;
        Ok((hash_of(&disk) == expected_hash).then_some(disk))
    };
    if unchanged()?.is_none() {
        return Err(ConflictError::Outdated);
    }
    let wanted = [path.to_string()];
    branches::copy_before(repo, RESOLVE, |root| Ok(on_disk(root, &wanted)))?;
    // The copy takes a while: Devin may have written in the meantime.
    if unchanged()?.is_none() {
        return Err(ConflictError::Outdated);
    }
    let full = repo.path.join(path);
    let permissions = fs::metadata(&full).ok().map(|meta| meta.permissions());
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent).map_err(git::io_error)?;
    }
    storage::write_atomic_with(&full, content, permissions).map_err(git::io_error)?;
    each_chunk(repo.ctx, &["add", "-f"], &wanted)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshots;
    use crate::test_support::RepoHarness;
    use crate::worktree_diff::content_hash;
    use std::fs;

    fn session_of(harness: &RepoHarness) -> Session {
        harness.run(|repo| session(repo.ctx)).unwrap()
    }

    fn sides_of(harness: &RepoHarness, path: &str) -> Sides {
        let path = path.to_string();
        harness.run(move |repo| sides(repo.ctx, &path)).unwrap()
    }

    fn accept_in(harness: &RepoHarness, paths: &[&str], side: Side) -> Result<(), GitError> {
        let paths: Vec<String> = paths.iter().map(|path| path.to_string()).collect();
        harness.run(move |repo| accept(repo, &paths, side))
    }

    fn resolve_in(
        harness: &RepoHarness,
        path: &str,
        content: &[u8],
        hash: &str,
    ) -> Result<(), ConflictError> {
        let (path, content, hash) = (path.to_string(), content.to_vec(), hash.to_string());
        harness.run(move |repo| resolve(repo, &path, &content, &hash))
    }

    fn text(content: &str) -> Version {
        Version::Text(content.to_string())
    }

    fn unmerged(harness: &RepoHarness) -> String {
        harness.git(&["ls-files", "--unmerged"])
    }

    // `a.txt` changed by both sides since `base`: `main` (where it stays) and `side`.
    fn merging() -> RepoHarness {
        let harness = RepoHarness::new();
        harness.diverged();
        harness.git_fails(&["merge", "side"]);
        harness
    }

    // `side` rebased onto `main`, stopped at the conflict.
    fn rebasing() -> RepoHarness {
        let harness = RepoHarness::new();
        harness.diverged();
        harness.git(&["checkout", "-q", "side"]);
        harness.git_fails(&["rebase", "main"]);
        harness
    }

    fn by_path(session: &Session, path: &str) -> Conflict {
        session
            .conflicts
            .iter()
            .find(|conflict| conflict.path == path)
            .unwrap_or_else(|| panic!("{path} is not in {session:?}"))
            .clone()
    }

    fn commit_all(harness: &RepoHarness, message: &str) {
        harness.git(&["add", "-A"]);
        harness.git(&["commit", "-q", "-m", message]);
    }

    // One conflict of each kind in a single merge. `f` is renamed on both sides: git leaves
    // `f` with neither (`DD`), and the two new names as added by one side only.
    fn seven_kinds() -> RepoHarness {
        let harness = RepoHarness::new();
        let lines: String = (1..=20).map(|n| format!("line {n}\n")).collect();
        harness.write("f", &lines);
        for name in ["u", "du", "ud"] {
            harness.write(name, "base\n");
        }
        commit_all(&harness, "base");
        harness.git(&["checkout", "-q", "-b", "side"]);
        harness.git(&["mv", "f", "f2"]);
        harness.write("u", "side\n");
        harness.write("du", "side\n");
        harness.write("aa", "side\n");
        harness.git(&["rm", "-q", "ud"]);
        commit_all(&harness, "side");
        harness.git(&["checkout", "-q", "main"]);
        harness.git(&["mv", "f", "f1"]);
        harness.write("u", "main\n");
        harness.write("ud", "main\n");
        harness.write("aa", "main\n");
        harness.git(&["rm", "-q", "du"]);
        commit_all(&harness, "main");
        harness.git_fails(&["merge", "side"]);
        harness
    }

    #[test]
    fn lists_the_seven_kinds() {
        let harness = seven_kinds();

        // What git says: the stages of each file and `status`'s XY.
        let stages: Vec<String> = unmerged(&harness)
            .lines()
            .map(|line| {
                let (head, path) = line.split_once('\t').unwrap();
                format!("{}{path}", head.rsplit(' ').next().unwrap())
            })
            .collect();
        assert_eq!(
            stages,
            ["2aa", "3aa", "1du", "3du", "1f", "2f1", "3f2", "1u", "2u", "3u", "1ud", "2ud"]
        );
        assert_eq!(
            harness.git(&["status", "--porcelain"]),
            "AA aa\nDU du\nDD f\nAU f1\nUA f2\nUU u\nUD ud\n"
        );

        let found = session_of(&harness);
        let seen: Vec<(String, SideStatus, SideStatus, Kind)> = found
            .conflicts
            .iter()
            .map(|c| (c.path.clone(), c.yours, c.theirs, c.kind))
            .collect();
        use Kind::*;
        use SideStatus::{Deleted as D, Modified as M};
        let expected = [
            ("aa", M, M, BothAdded),
            ("du", D, M, DeletedByYou),
            ("f", D, D, BothDeleted),
            ("f1", M, D, AddedByYou),
            ("f2", D, M, AddedByThem),
            ("u", M, M, BothModified),
            ("ud", M, D, DeletedByThem),
        ];
        let expected: Vec<_> = expected
            .into_iter()
            .map(|(path, y, t, kind)| (path.to_string(), y, t, kind))
            .collect();
        assert_eq!(seen, expected);
        assert!(!found.reversed);
    }

    #[test]
    fn the_kind_uses_intellijs_texts() {
        let texts: Vec<String> = [
            Kind::BothDeleted,
            Kind::BothAdded,
            Kind::BothModified,
            Kind::DeletedByYou,
            Kind::DeletedByThem,
            Kind::AddedByYou,
            Kind::AddedByThem,
        ]
        .iter()
        .map(|kind| {
            serde_json::to_value(kind)
                .unwrap()
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
        assert_eq!(
            texts,
            [
                "both deleted",
                "both added",
                "both modified",
                "deleted by you",
                "deleted by them",
                "added by you",
                "added by them"
            ]
        );
    }

    #[test]
    fn stays_in_conflict_until_marked_resolved() {
        let harness = merging();

        assert!(is_unmerged(harness.path(), "a.txt").unwrap());
        assert!(!is_unmerged(harness.path(), "another.txt").unwrap());
        harness.git(&["add", "a.txt"]);
        assert!(!is_unmerged(harness.path(), "a.txt").unwrap());
    }

    #[test]
    fn a_merge_session_names_the_branches() {
        let found = session_of(&merging());

        assert_eq!(found.yours_column, "Yours (main)");
        assert_eq!(found.theirs_column, "Theirs (side)");
        assert_eq!(found.description, "Merging branch side into branch main");
    }

    #[test]
    fn sides_reads_the_three_stages() {
        let harness = merging();

        let found = sides_of(&harness, "a.txt");

        assert_eq!(found.base, text("base\n"));
        assert_eq!(found.yours, text("main\n"));
        assert_eq!(found.theirs, text("side\n"));
        assert_eq!(
            found.titles,
            ["Changes from main", "Result", "Changes from side"]
        );
        assert_eq!(found.hash, content_hash(harness.read("a.txt").as_bytes()));
        assert!(found.hash.len() == 64);
        assert!(!found.reversed);
        assert_eq!(found.path, "a.txt");
        assert_eq!(
            found.blocks,
            [Block {
                left: (0, 1),
                base: (0, 1),
                right: (0, 1),
                kind: merge3::BlockKind::Conflict,
                left_changed: true,
                right_changed: true,
                auto: None,
            }]
        );
    }

    #[test]
    fn deleted_by_them_gives_missing_in_theirs() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"base\n", "base");
        harness.git(&["checkout", "-q", "-b", "side"]);
        harness.git(&["rm", "-q", "a.txt"]);
        harness.git(&["commit", "-q", "-m", "deletes"]);
        harness.git(&["checkout", "-q", "main"]);
        harness.repo.commit("a.txt", b"main\n", "changes");
        harness.git_fails(&["merge", "side"]);

        let found = sides_of(&harness, "a.txt");

        assert_eq!(found.theirs, Version::Missing);
        assert_eq!(found.yours, text("main\n"));
        assert_eq!(found.base, text("base\n"));
    }

    // Like `loadRevisionCatchingErrors`: the missing stage is an empty text, and the merge
    // window has its blocks.
    #[test]
    fn deleted_by_them_has_blocks_against_empty_text() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"base\n", "base");
        harness.git(&["checkout", "-q", "-b", "side"]);
        harness.git(&["rm", "-q", "a.txt"]);
        harness.git(&["commit", "-q", "-m", "deletes"]);
        harness.git(&["checkout", "-q", "main"]);
        harness.repo.commit("a.txt", b"main\n", "changes");
        harness.git_fails(&["merge", "side"]);

        let found = sides_of(&harness, "a.txt");

        assert_eq!(found.theirs, Version::Missing);
        // The empty line after the last newline of `main\n` and of `base\n` is the only one on the right.
        assert_eq!(
            found.blocks,
            [Block {
                left: (0, 1),
                base: (0, 1),
                right: (0, 0),
                kind: merge3::BlockKind::Conflict,
                left_changed: true,
                right_changed: true,
                auto: None,
            }]
        );
    }

    #[test]
    fn binary_gives_binary() {
        let harness = RepoHarness::new();
        let png = |tail: u8| [&b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"[..], &[tail, 0, 1]].concat();
        harness.repo.commit("i.png", &png(0), "base");
        harness.git(&["checkout", "-q", "-b", "side"]);
        harness.repo.commit("i.png", &png(1), "side");
        harness.git(&["checkout", "-q", "main"]);
        harness.repo.commit("i.png", &png(2), "main");
        harness.git_fails(&["merge", "side"]);

        let found = sides_of(&harness, "i.png");

        assert_eq!(found.base, Version::Binary);
        assert_eq!(found.yours, Version::Binary);
        assert_eq!(found.theirs, Version::Binary);
        assert!(found.blocks.is_empty());
        assert_eq!(
            found.hash,
            content_hash(&fs::read(harness.path().join("i.png")).unwrap())
        );
    }

    #[test]
    fn non_utf8_text_is_not_offered_as_text() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"base\n", "base");
        harness.git(&["checkout", "-q", "-b", "side"]);
        harness.repo.commit("a.txt", b"caf\xe9\n", "side");
        harness.git(&["checkout", "-q", "main"]);
        harness.repo.commit("a.txt", b"main\n", "main");
        harness.git_fails(&["merge", "side"]);

        let found = sides_of(&harness, "a.txt");

        assert_eq!(found.theirs, Version::Binary);
        assert_eq!(found.yours, text("main\n"));
    }

    // `GitMergeUtil.loadOriginalContent`: without stage 1 the base is the working file as it
    // is, with its markers, and empty if it does not even exist.
    #[test]
    fn missing_base_like_intellij() {
        let harness = RepoHarness::new();
        harness.repo.commit("another.txt", b"x\n", "base");
        harness.git(&["checkout", "-q", "-b", "side"]);
        harness.repo.commit("a.txt", b"side\n", "side");
        harness.git(&["checkout", "-q", "main"]);
        harness.repo.commit("a.txt", b"main\n", "main");
        harness.git_fails(&["merge", "side"]);
        assert_eq!(harness.git(&["status", "--porcelain"]), "AA a.txt\n");
        let marked = harness.read("a.txt");
        assert!(marked.contains("<<<<<<<"), "{marked}");

        let found = sides_of(&harness, "a.txt");

        assert_eq!(found.base, Version::Text(marked));
        assert_eq!(found.yours, text("main\n"));
        assert_eq!(found.theirs, text("side\n"));

        fs::remove_file(harness.path().join("a.txt")).unwrap();
        let gone = sides_of(&harness, "a.txt");
        assert_eq!(gone.base, text(""));
        assert_eq!(gone.hash, "");
    }

    #[test]
    fn sides_of_something_not_in_conflict_is_rejected() {
        let harness = merging();
        harness.write("clean.txt", "x\n");

        let error = harness
            .run(|repo| sides(repo.ctx, "clean.txt"))
            .unwrap_err();

        assert!(matches!(error, GitError::Io(_)), "{error:?}");
    }

    #[test]
    fn in_rebase_sides_and_columns_are_swapped() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"base\n", "base");
        harness.repo.commit("d.txt", b"base\n", "base d");
        harness.git(&["checkout", "-q", "-b", "side"]);
        harness.write("a.txt", "side\n");
        harness.write("d.txt", "side\n");
        commit_all(&harness, "side");
        harness.git(&["checkout", "-q", "main"]);
        harness.repo.commit("a.txt", b"main\n", "main a");
        harness.git(&["rm", "-q", "d.txt"]);
        harness.git(&["commit", "-q", "-m", "main d"]);
        harness.git(&["checkout", "-q", "side"]);
        harness.git_fails(&["rebase", "main"]);
        assert_eq!(
            harness.git(&["status", "--porcelain"]),
            "UU a.txt\nDU d.txt\n"
        );

        let found = session_of(&harness);

        assert!(found.reversed);
        assert_eq!(found.yours_column, "Yours (side)");
        assert_eq!(found.theirs_column, "Theirs (main)");
        assert_eq!(found.description, "Rebasing branch side onto branch main");
        // In git, `d.txt` is `DU`: it is deleted by the `--ours` side, which here is `main`, the other one.
        let deleted = by_path(&found, "d.txt");
        assert_eq!(deleted.yours, SideStatus::Modified);
        assert_eq!(deleted.theirs, SideStatus::Deleted);
        assert_eq!(deleted.kind, Kind::DeletedByThem);

        let both = sides_of(&harness, "a.txt");
        assert!(both.reversed);
        assert_eq!(both.yours, text("side\n"));
        assert_eq!(both.theirs, text("main\n"));
        assert_eq!(both.base, text("base\n"));
        let rebased = harness.git(&["rev-parse", "--short=8", "REBASE_HEAD"]);
        assert_eq!(
            both.titles,
            [
                format!("Rebasing {} from side", rebased.trim()),
                "Result".to_string(),
                "Already rebased commits and commits from main".to_string()
            ]
        );

        // Accept Yours keeps what is on the branch being rebased, which for git is `--theirs`.
        accept_in(&harness, &["a.txt", "d.txt"], Side::Yours).unwrap();
        assert_eq!(harness.read("a.txt"), "side\n");
        assert_eq!(harness.read("d.txt"), "side\n");
        assert_eq!(unmerged(&harness), "");
    }

    #[test]
    fn accept_theirs_in_rebase_keeps_the_base_branch_content() {
        let harness = rebasing();

        accept_in(&harness, &["a.txt"], Side::Theirs).unwrap();

        assert_eq!(harness.read("a.txt"), "main\n");
        assert_eq!(unmerged(&harness), "");
    }

    #[test]
    fn a_cherry_pick_uses_its_own_texts() {
        let harness = RepoHarness::new();
        harness.diverged();
        harness.git_fails(&["cherry-pick", "side"]);
        let picked = harness.git(&["rev-parse", "--short=8", "CHERRY_PICK_HEAD"]);
        let picked = picked.trim();

        let found = session_of(&harness);
        let sides = sides_of(&harness, "a.txt");

        assert_eq!(found.theirs_column, "Theirs (cherry-pick)");
        assert_eq!(found.yours_column, "Yours (main)");
        assert_eq!(
            found.description,
            format!("Conflicts during cherry-picking commit {picked} made by Test\nside")
        );
        assert_eq!(
            sides.titles,
            [
                "Local Changes".to_string(),
                "Result".to_string(),
                format!("Changes from cherry-pick {picked}")
            ]
        );
        assert!(!found.reversed);
    }

    #[test]
    fn a_revert_describes_the_commit_it_undoes() {
        let harness = RepoHarness::new();
        harness.diverged();
        harness.repo.commit("a.txt", b"other\n", "again");
        harness.git_fails(&["revert", "--no-edit", "HEAD~1"]);
        let reverted = harness.git(&["rev-parse", "--short=8", "REVERT_HEAD"]);

        let found = session_of(&harness);

        assert_eq!(found.theirs_column, "Theirs");
        assert_eq!(
            found.description,
            format!(
                "Conflicts during revert of commit {} made by Test\nmain",
                reverted.trim()
            )
        );
    }

    #[test]
    fn accept_yours_keeps_main_content_and_marks_it_resolved() {
        let harness = merging();

        accept_in(&harness, &["a.txt"], Side::Yours).unwrap();

        assert_eq!(harness.read("a.txt"), "main\n");
        assert_eq!(unmerged(&harness), "");
        assert_eq!(harness.git(&["status", "--porcelain"]), "");
    }

    #[test]
    fn accept_theirs_of_a_deletion_does_rm() {
        let harness = RepoHarness::new();
        harness.repo.commit("ud.txt", b"base\n", "base ud");
        harness.repo.commit("du.txt", b"base\n", "base du");
        harness.git(&["checkout", "-q", "-b", "side"]);
        harness.git(&["rm", "-q", "ud.txt"]);
        harness.write("du.txt", "side\n");
        commit_all(&harness, "side");
        harness.git(&["checkout", "-q", "main"]);
        harness.write("ud.txt", "main\n");
        harness.git(&["rm", "-q", "du.txt"]);
        commit_all(&harness, "main");
        harness.git_fails(&["merge", "side"]);
        assert_eq!(
            harness.git(&["status", "--porcelain"]),
            "DU du.txt\nUD ud.txt\n"
        );

        // `ud.txt` is deleted by them and `du.txt` by us.
        accept_in(&harness, &["ud.txt"], Side::Theirs).unwrap();
        accept_in(&harness, &["du.txt"], Side::Yours).unwrap();

        assert_eq!(unmerged(&harness), "");
        assert!(!harness.path().join("ud.txt").exists());
        assert!(!harness.path().join("du.txt").exists());
        assert_eq!(harness.git(&["ls-files"]), "");
    }

    #[test]
    fn accept_saves_a_copy_of_the_file_with_markers() {
        let harness = merging();
        let marked = harness.read("a.txt");
        assert!(marked.contains("<<<<<<<"));

        accept_in(&harness, &["a.txt"], Side::Theirs).unwrap();

        assert_eq!(harness.read("a.txt"), "side\n");
        assert_eq!(harness.snapshots(), 1);
        assert_eq!(harness.saved_paths(), ["a.txt"]);
        let saved = snapshots::list(harness.data.path());
        let copy = snapshots::stored(harness.data.path(), &saved[0].id, "a.txt", false).unwrap();
        assert_eq!(fs::read_to_string(copy).unwrap(), marked);
    }

    #[test]
    fn accept_does_not_touch_what_is_no_longer_in_conflict() {
        let harness = merging();
        harness.write("another.txt", "mine\n");
        harness.git(&["add", "another.txt"]);

        accept_in(&harness, &["another.txt", "a.txt"], Side::Theirs).unwrap();

        assert_eq!(harness.read("another.txt"), "mine\n");
        assert_eq!(harness.read("a.txt"), "side\n");
        assert_eq!(harness.snapshots(), 1);
        assert_eq!(harness.saved_paths(), ["a.txt"]);
    }

    #[test]
    fn resolve_writes_and_marks_resolved() {
        let harness = merging();
        let hash = sides_of(&harness, "a.txt").hash;

        resolve_in(&harness, "a.txt", b"merged\n", &hash).unwrap();

        assert_eq!(harness.read("a.txt"), "merged\n");
        assert_eq!(unmerged(&harness), "");
        assert_eq!(harness.git(&["show", ":a.txt"]), "merged\n");
        // The merge is still halfway: the commit finishes it.
        assert!(harness.in_git_dir("MERGE_HEAD"));
        assert_eq!(harness.saved_paths(), ["a.txt"]);
    }

    #[test]
    fn resolve_with_old_hash_does_not_write() {
        let harness = merging();
        let hash = sides_of(&harness, "a.txt").hash;
        harness.write("a.txt", "what Devin wrote\n");

        let result = resolve_in(&harness, "a.txt", b"merged\n", &hash);

        assert_eq!(result, Err(ConflictError::Outdated));
        assert_eq!(harness.read("a.txt"), "what Devin wrote\n");
        assert!(unmerged(&harness).contains("a.txt"));
        assert_eq!(harness.snapshots(), 0);
    }

    // The merge window reacts to this kind, not to a text: it has to get all the way to `ApiError`,
    // which is what a failed task hands to the interface.
    #[test]
    fn resolve_with_old_hash_gives_kind_outdated() {
        let harness = merging();
        let hash = sides_of(&harness, "a.txt").hash;
        harness.write("a.txt", "what Devin wrote\n");

        let error = resolve_in(&harness, "a.txt", b"merged\n", &hash).unwrap_err();
        let api = crate::commands::ApiError::from(GitError::from(error));

        assert_eq!(api.kind, "outdated");
        // Any other failure is still `failed`.
        let other = resolve_in(&harness, "clean.txt", b"y\n", "").unwrap_err();
        assert_eq!(
            crate::commands::ApiError::from(GitError::from(other)).kind,
            "failed"
        );
    }

    #[test]
    fn resolve_of_something_not_in_conflict_does_not_write() {
        let harness = merging();
        harness.write("clean.txt", "x\n");
        let hash = content_hash(b"x\n");

        let result = resolve_in(&harness, "clean.txt", b"y\n", &hash);

        assert!(
            matches!(result, Err(ConflictError::Git(GitError::Io(_)))),
            "{result:?}"
        );
        assert_eq!(harness.read("clean.txt"), "x\n");
    }

    #[test]
    fn resolve_does_not_leave_the_repository() {
        let harness = merging();

        for path in ["../outside.txt", ".git/config", "/etc/hosts"] {
            let result = resolve_in(&harness, path, b"x", "");
            assert!(result.is_err(), "{path}");
        }
        assert!(!harness.path().join("../outside.txt").exists());
    }

    #[test]
    fn path_with_leading_dash_is_not_an_option() {
        let harness = RepoHarness::new();
        harness.repo.conflict("-x");

        assert_eq!(
            by_path(&session_of(&harness), "-x").kind,
            Kind::BothModified
        );
        assert_eq!(sides_of(&harness, "-x").theirs, text("other\n"));
        accept_in(&harness, &["-x"], Side::Theirs).unwrap();

        assert_eq!(harness.read("-x"), "other\n");
        assert_eq!(unmerged(&harness), "");
    }
}
