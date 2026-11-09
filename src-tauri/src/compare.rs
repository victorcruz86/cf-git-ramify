// What the comparisons of two revisions need (H§4): the revision a file has in HEAD, the
// commits that touched it, the changes between a revision and the working tree or another
// revision, and the two sides of one file. Every `<rev>:<path>` and every path-taking call goes
// through `git::exact` with `--literal-pathspecs`, so a glob-named or NFD file is that file.

use std::fs;
use std::io::Read as _;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::commands::{blocking_api, current_repo, ApiError, AppState};
use crate::diff::{self, ChangedFile, Side};
use crate::git::{self, GitError};
use crate::history::{self, Starts, Walk};
use crate::hunks::{self, Hunk, HunkError};
use crate::status::{self, FileState};
use crate::worktree_diff;

// The `revision` of the side that is the file on disk.
pub const LOCAL: &str = "local";
// `LARGE_FILE_PREVIEW_SIZE`: what is shown of a file above the diff limit.
pub const PREVIEW_BYTES: usize = 2500 * 1024;

// One side of a comparison: a full hash, the file on disk (`local`) or nothing (`None`: the file
// does not exist on that side). `title` is what the diff window shows above it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SideRef {
    pub revision: Option<String>,
    pub path: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevisionPair {
    pub left: SideRef,
    pub right: SideRef,
    // A merge of the file history, shown in three sides `left | center | right` with the merge in
    // the center (`MergedChangeDiffRequestProvider`): read-only, and never the local file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub center: Option<SideRef>,
}

// What the diff window shows: the working tree of one file (phase 1), or two revisions of one or more
// files, with their own titles; `selected` is the pair it opens on (`ListSelection`'s index), the
// first without it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum DiffRequest {
    Worktree {
        path: String,
    },
    Revisions {
        files: Vec<RevisionPair>,
        title: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selected: Option<usize>,
    },
}

// A commit as the `File Revisions` popup and the side titles need it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileRevision {
    pub hash: String,
    // Seconds; `GitRevisionNumber` compares by the commit time.
    pub commit_time: i64,
    pub author_time: i64,
    pub author: String,
    pub message: String,
    // In the `File Revisions` popup, the path the file had in that revision (`null` for a folder).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

// What the titles of a comparison with the local file depend on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalState {
    // The repository folder, for the `name (folder)` window titles.
    pub root: String,
    // `ChangeListManager.getStatus` is neither `NOT_CHANGED`, `UNKNOWN` nor `IGNORED`.
    pub modified: bool,
    // `GitDiffProvider.getCurrentRevision`: the last commit of HEAD that touched the file.
    pub current: Option<FileRevision>,
    // `getPresentableCurrentBranchName`: the branch, or the short hash when detached.
    pub head_label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RefKind {
    // A local branch, else a remote one (`findBranchByName`).
    Branch,
    Tag,
}

// The two sides of one file, in the shape of `WorktreeSides` so that `FilePane` shows them, plus
// what only a comparison of two revisions has.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevisionSides {
    pub old: Side,
    pub new: Side,
    pub hunks: Vec<Hunk>,
    // Only the local side on the right is edited, and only if saving the displayed text leaves on
    // disk the same thing that was there.
    pub editable: bool,
    pub note: Option<String>,
    // Of the bytes on disk, for `file_write`.
    pub hash: Option<String>,
    pub conflicted: bool,
    // The side is the first part of a file above the diff limit (`DB:273`).
    pub old_preview: bool,
    pub new_preview: bool,
    // With a binary side: whether both sides have the same bytes (`DB:46-47`).
    pub identical: Option<bool>,
}

// `%B` is the whole message; `-z` ends each commit with a NUL, so no field needs escaping.
const LOG_FORMAT: &str = "--format=%H%x00%ct%x00%at%x00%an%x00%B";
const LOG_FIELDS: usize = 5;
// `DvcsUtil.getShortHash`.
const SHORT_HASH: usize = 8;
const LOCAL_TITLE: &str = "Local";

fn parse_log(output: &[u8]) -> Vec<FileRevision> {
    let fields: Vec<String> = output
        .split(|byte| *byte == 0)
        .map(|field| String::from_utf8_lossy(field).into_owned())
        .collect();
    fields
        .chunks(LOG_FIELDS)
        .filter(|record| record.len() == LOG_FIELDS)
        .map(|record| FileRevision {
            // After the first record, git starts each one with the newline that ends `%B`.
            hash: record[0].trim_start_matches('\n').to_string(),
            commit_time: record[1].parse().unwrap_or_default(),
            author_time: record[2].parse().unwrap_or_default(),
            author: record[3].clone(),
            message: record[4].clone(),
            path: None,
        })
        .filter(|revision| !revision.hash.is_empty())
        .collect()
}

// A revision reaches git as an argument before `--`: one starting with `-` would be an option, and
// a `:` would change what `<rev>:<path>` reads.
fn check_revision(revision: &str) -> Result<(), GitError> {
    let valid = !revision.is_empty()
        && !revision.starts_with('-')
        && !revision.contains(':')
        && !revision
            .chars()
            .any(|c| c.is_whitespace() || c.is_control());
    if valid {
        Ok(())
    } else {
        Err(GitError::Io(format!("invalid revision: {revision}")))
    }
}

fn log(repo: &Path, head: &[&str], paths: &[&str]) -> Result<Vec<FileRevision>, GitError> {
    let mut args = git::exact(&[
        history::NO_SIGNATURES[0],
        history::NO_SIGNATURES[1],
        "--literal-pathspecs",
        "log",
        "-z",
        LOG_FORMAT,
    ]);
    args.extend_from_slice(head);
    args.push("--");
    args.extend_from_slice(paths);
    Ok(parse_log(&git::run(repo, &args)?))
}

// `GitHistoryUtils.getCurrentRevision`: `git log -n1 HEAD -- <path>`.
pub fn head_revision_of(repo: &Path, path: &str) -> Result<Option<FileRevision>, GitError> {
    if !git::head_exists(repo)? {
        return Ok(None);
    }
    last_revision(repo, "HEAD", Some(path))
}

// The last commit of `revision` that touched `path`, or the commit itself without a path.
pub fn last_revision(
    repo: &Path,
    revision: &str,
    path: Option<&str>,
) -> Result<Option<FileRevision>, GitError> {
    check_revision(revision)?;
    let paths: Vec<&str> = path.into_iter().collect();
    Ok(log(repo, &["-n1", revision], &paths)?.into_iter().next())
}

// What `Show Current Revision` says of the file (`VcsRevisionDescription`): `author` already has
// the committer after it when that is someone else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentRevision {
    pub hash: String,
    // Seconds: `GitLogRecord.getDate` is the commit time.
    pub commit_time: i64,
    pub author: String,
    pub message: String,
}

const CURRENT_FORMAT: &str = "--format=%H%x00%ct%x00%an%x00%cn%x00%B";
const CURRENT_FIELDS: usize = 5;

// `VcsUtil.getLastCommitPath`: a file renamed and not committed yet has its history at the old path.
fn last_commit_path(repo: &Path, path: &str) -> Result<String, GitError> {
    let renamed = status::read(repo, false)?.into_iter().find(|change| {
        change.path == path && change.state == FileState::Renamed && change.old_path.is_some()
    });
    Ok(renamed
        .and_then(|change| change.old_path)
        .unwrap_or_else(|| path.to_string()))
}

// `GitHistoryUtils.getCurrentRevisionDescription`: `git log -n1 --encoding=UTF-8 --all -- <path>`,
// the last commit of any branch that touched the file; none if no commit did.
pub fn current_revision(repo: &Path, path: &str) -> Result<Option<CurrentRevision>, GitError> {
    let path = last_commit_path(repo, path)?;
    let args = git::exact(&[
        history::NO_SIGNATURES[0],
        history::NO_SIGNATURES[1],
        "--literal-pathspecs",
        "log",
        "-z",
        "-n1",
        CURRENT_FORMAT,
        "--encoding=UTF-8",
        "--all",
        "--",
        &path,
    ]);
    parse_current(&git::run(repo, &args)?)
}

// The record of `CURRENT_FORMAT`; a time that is not a number is an error, not 1970.
fn parse_current(output: &[u8]) -> Result<Option<CurrentRevision>, GitError> {
    let fields: Vec<String> = output
        .split(|byte| *byte == 0)
        .map(|field| String::from_utf8_lossy(field).into_owned())
        .collect();
    if fields.len() < CURRENT_FIELDS || fields[0].is_empty() {
        return Ok(None);
    }
    let commit_time = fields[1]
        .parse()
        .map_err(|_| GitError::Io(format!("unreadable commit time: {}", fields[1])))?;
    let (author, committer) = (&fields[2], &fields[3]);
    Ok(Some(CurrentRevision {
        hash: fields[0].clone(),
        commit_time,
        author: if author == committer {
            author.clone()
        } else {
            format!("{author} ({committer})")
        },
        // `GitLogRecord.getFullMessage`, trimmed.
        message: fields[4].trim().to_string(),
    }))
}

// The `File Revisions` popup: `GitHistoryProvider`'s history from HEAD, newest first, which is
// `GitFileHistory` without `fullHistory` (each walk ends at its first addition, renames followed),
// with the path of each revision; a folder is one walk. The details come from one `git log
// --no-walk` in the order of the history.
pub fn file_revisions(repo: &Path, path: &str) -> Result<Vec<FileRevision>, GitError> {
    if !git::head_exists(repo)? {
        return Ok(Vec::new());
    }
    let folder = repo.join(path).is_dir();
    let mode = if folder {
        Walk::Folder
    } else {
        Walk::FirstAddition
    };
    let mut run = |args: &[&str]| {
        git::run(repo, args).map(|output| String::from_utf8_lossy(&output).into_owned())
    };
    let starts = Starts::Revisions(vec!["HEAD".to_string()]);
    let rows = history::file_history(&mut run, path, &starts, mode)?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let input: String = rows.iter().map(|row| format!("{}\n", row.hash)).collect();
    let args = [
        history::NO_SIGNATURES[0],
        history::NO_SIGNATURES[1],
        "log",
        "-z",
        "--no-walk=unsorted",
        "--stdin",
        LOG_FORMAT,
    ];
    let mut revisions = parse_log(&git::run_with_input(repo, &args, input.as_bytes())?);
    for (revision, row) in revisions.iter_mut().zip(&rows) {
        if revision.hash == row.hash && !folder {
            revision.path = Some(row.path.clone());
        }
    }
    Ok(revisions)
}

// `GitChangeUtils.getDiff*`: name and status, renames found, in batches of paths. Without paths,
// the whole repository.
fn diff_names(repo: &Path, range: &[&str], paths: &[String]) -> Result<Vec<ChangedFile>, GitError> {
    let mut head = git::exact(&[
        "--literal-pathspecs",
        "diff",
        "--name-status",
        "-z",
        "--diff-filter=ADCMRUXT",
        "-M",
    ]);
    head.extend_from_slice(range);
    head.push("--");
    if paths.is_empty() {
        return Ok(diff::parse_name_status(&git::run(repo, &head)?));
    }
    let mut changes = Vec::new();
    for chunk in paths.chunks(git::PATHS_PER_COMMAND) {
        let mut args = head.clone();
        args.extend(chunk.iter().map(String::as_str));
        changes.extend(diff::parse_name_status(&git::run(repo, &args)?));
    }
    Ok(changes)
}

// `git diff --name-status --diff-filter=ADCMRUXT -M <rev> -- <paths>`: the revision against the
// working tree.
pub fn changes_with(
    repo: &Path,
    revision: &str,
    paths: &[String],
) -> Result<Vec<ChangedFile>, GitError> {
    check_revision(revision)?;
    diff_names(repo, &[revision], paths)
}

// `git diff --name-status --diff-filter=ADCMRUXT -M <a>..<b> -- <paths>`.
pub fn changes_between(
    repo: &Path,
    left: &str,
    right: &str,
    paths: &[String],
) -> Result<Vec<ChangedFile>, GitError> {
    check_revision(left)?;
    check_revision(right)?;
    let range = format!("{left}..{right}");
    diff_names(repo, &[&range], paths)
}

// `GitBranchWorker.loadTotalDiff` (`GitChangeUtils.getDiffWithWorkingDir` with `reverse`):
// `git diff -R --name-status --diff-filter=ADCMRUXT -M <rev>`, the working tree as the base on the
// left and the revision's tree on the right, for the whole repository.
pub fn changes_from_working_tree(
    repo: &Path,
    revision: &str,
) -> Result<Vec<ChangedFile>, GitError> {
    check_revision(revision)?;
    diff_names(repo, &["-R", revision], &[])
}

// `findBranchByName` looks among the local branches and then the remote ones; a tag is
// `refs/tags/<name>`. `None` if there is no such reference.
pub fn resolve(repo: &Path, kind: RefKind, name: &str) -> Result<Option<FileRevision>, GitError> {
    if check_revision(name).is_err() {
        return Ok(None);
    }
    let candidates: Vec<String> = match kind {
        RefKind::Branch => vec![format!("refs/heads/{name}"), format!("refs/remotes/{name}")],
        RefKind::Tag => vec![format!("refs/tags/{name}")],
    };
    for full in candidates {
        let commit = format!("{full}^{{commit}}");
        match git::run(repo, &["rev-parse", "-q", "--verify", &commit]) {
            Ok(_) => return last_revision(repo, &full, None),
            Err(GitError::Failed { .. }) => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(None)
}

// `GitBranchUtil.getAllTags`: `git tag -l`, in git's order.
pub fn tags(repo: &Path) -> Result<Vec<String>, GitError> {
    let output = git::run(repo, &["tag", "-l"])?;
    Ok(String::from_utf8_lossy(&output)
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

// The branch, else the short hash, else `Local`.
fn head_label(repo: &Path) -> Result<String, GitError> {
    match git::run(repo, &["symbolic-ref", "-q", "HEAD"]) {
        Ok(output) => {
            let name = String::from_utf8_lossy(&output).trim().to_string();
            if let Some(branch) = name.strip_prefix("refs/heads/") {
                return Ok(branch.to_string());
            }
        }
        Err(GitError::Failed { .. }) => {}
        Err(error) => return Err(error),
    }
    match git::run(repo, &["rev-parse", "-q", "--verify", "HEAD"]) {
        Ok(output) => Ok(String::from_utf8_lossy(&output)
            .trim()
            .chars()
            .take(SHORT_HASH)
            .collect()),
        Err(GitError::Failed { .. }) => Ok(LOCAL_TITLE.to_string()),
        Err(error) => Err(error),
    }
}

// Changed against HEAD, in the stage or on disk. An unversioned or ignored file is not.
fn has_local_changes(repo: &Path, path: &str) -> Result<bool, GitError> {
    let args = git::exact(&[
        "--literal-pathspecs",
        "status",
        "--porcelain=v1",
        "-z",
        "--untracked-files=all",
        "--",
        path,
    ]);
    let output = git::run(repo, &args)?;
    Ok(output
        .split(|byte| *byte == 0)
        .filter(|record| record.len() > 3)
        .any(|record| !record.starts_with(b"??") && !record.starts_with(b"!!")))
}

pub fn local_state(repo: &Path, path: &str) -> Result<LocalState, HunkError> {
    hunks::check_path(repo, path)?;
    Ok(LocalState {
        root: repo.display().to_string(),
        modified: has_local_changes(repo, path)?,
        current: head_revision_of(repo, path)?,
        head_label: head_label(repo)?,
    })
}

// What one side gives to compare: the side as shown and the bytes it is compared by (the stored
// form for a local text, the raw bytes for a binary).
struct Read {
    side: Side,
    bytes: Vec<u8>,
    preview: bool,
    // Of a binary side: the blob it is, or would be, so two binaries compare without their bytes.
    oid: Option<String>,
}

impl Read {
    fn whole(side: Side) -> Self {
        Self {
            side,
            bytes: Vec::new(),
            preview: false,
            oid: None,
        }
    }

    fn binary(oid: String) -> Self {
        Self {
            oid: Some(oid),
            ..Self::whole(Side::Binary)
        }
    }
}

fn trimmed(output: &[u8]) -> String {
    String::from_utf8_lossy(output).trim().to_string()
}

// The blob `git add` would make of the bytes on disk, without filters: a binary is stored as it is.
fn disk_oid(repo: &Path, path: &str) -> Result<String, GitError> {
    let output = git::run(repo, &["hash-object", "--no-filters", "--", path])?;
    Ok(trimmed(&output))
}

// The first `PREVIEW_BYTES` of a text, cut where a character ends.
fn preview_of(bytes: &[u8]) -> Read {
    let mut cut = &bytes[..bytes.len().min(PREVIEW_BYTES)];
    if let Err(error) = std::str::from_utf8(cut) {
        if error.error_len().is_none() {
            cut = &cut[..error.valid_up_to()];
        }
    }
    Read {
        side: lossy(cut),
        bytes: cut.to_vec(),
        preview: true,
        oid: None,
    }
}

fn lossy(bytes: &[u8]) -> Side {
    Side::Text {
        content: String::from_utf8_lossy(bytes).into_owned(),
    }
}

// `<hash>:<path>` read with the exact bytes of the path.
fn read_revision(repo: &Path, hash: &str, path: &str) -> Result<Read, GitError> {
    diff::check_hash(hash)?;
    let object = format!("{hash}:{path}");
    match git::run(repo, &git::exact(&["cat-file", "-e", &object])) {
        Ok(_) => {}
        Err(GitError::Failed { stderr, .. }) if stderr.is_empty() || diff::is_absent(&stderr) => {
            return Ok(match diff::gitlink_at(repo, hash, path)? {
                Some(commit) => Read::whole(diff::gitlink_side(&commit)),
                None => Read::whole(Side::Absent),
            })
        }
        Err(failure @ GitError::Failed { .. }) => {
            return match diff::gitlink_at(repo, hash, path)? {
                Some(commit) => Ok(Read::whole(diff::gitlink_side(&commit))),
                None => Err(failure),
            }
        }
        Err(error) => return Err(error),
    }
    let content = git::run(repo, &git::exact(&["cat-file", "blob", &object]))?;
    if diff::looks_binary(&content) {
        let oid = git::run(repo, &git::exact(&["rev-parse", "--verify", "-q", &object]))?;
        return Ok(Read::binary(trimmed(&oid)));
    }
    if content.len() as u64 > diff::MAX_DIFF_BYTES {
        return Ok(preview_of(&content));
    }
    Ok(Read {
        side: lossy(&content),
        bytes: content,
        preview: false,
        oid: None,
    })
}

struct LocalRead {
    read: Read,
    editable: bool,
    note: Option<String>,
    hash: Option<String>,
}

// The file on disk, as `worktree_diff::sides` shows it: in the form git stores, editable only if
// saving that text leaves on disk the same bytes.
fn read_local(repo: &Path, path: &str) -> Result<LocalRead, HunkError> {
    worktree_diff::check_inside(repo, path)?;
    let full = repo.join(path);
    let regular = fs::symlink_metadata(&full).is_ok_and(|meta| meta.is_file());
    if let Some(disk) = hunks::read_current(repo, path)? {
        let hash = Some(worktree_diff::content_hash(&disk.bytes));
        if diff::looks_binary(&disk.bytes) {
            return Ok(LocalRead {
                read: Read::binary(disk_oid(repo, path)?),
                editable: false,
                note: None,
                hash,
            });
        }
        let stored = hunks::stored_form(repo, path)?.unwrap_or_else(|| disk.bytes.clone());
        if diff::looks_binary(&stored) {
            return Ok(LocalRead {
                read: Read::binary(disk_oid(repo, path)?),
                editable: false,
                note: None,
                hash,
            });
        }
        let utf8 = std::str::from_utf8(&stored).is_ok();
        let note = if !utf8 {
            Some(worktree_diff::NOTE_NOT_UTF8)
        } else if stored != disk.bytes {
            Some(worktree_diff::NOTE_CONVERTED)
        } else {
            None
        };
        return Ok(LocalRead {
            editable: utf8 && stored == disk.bytes,
            note: note.map(str::to_string),
            hash,
            read: Read {
                side: lossy(&stored),
                bytes: stored,
                preview: false,
                oid: None,
            },
        });
    }
    if regular {
        // Above the diff limit: only the first part is read, and shown read-only.
        let mut bytes = Vec::new();
        fs::File::open(&full)?
            .take(PREVIEW_BYTES as u64)
            .read_to_end(&mut bytes)?;
        let read = if diff::looks_binary(&bytes) {
            Read::binary(disk_oid(repo, path)?)
        } else {
            preview_of(&bytes)
        };
        return Ok(LocalRead {
            read,
            editable: false,
            note: None,
            hash: None,
        });
    }
    let (side, note) = worktree_diff::unsplit_side(repo, path)?;
    Ok(LocalRead {
        read: Read::whole(side),
        editable: false,
        note: note.map(str::to_string),
        hash: None,
    })
}

fn is_local(side: &SideRef) -> bool {
    side.revision.as_deref() == Some(LOCAL)
}

fn check_side(repo: &Path, side: &SideRef) -> Result<(), HunkError> {
    match side.revision.as_deref() {
        Some(LOCAL) => worktree_diff::check_inside(repo, &side.path),
        Some(hash) => {
            diff::check_hash(hash)?;
            hunks::check_path(repo, &side.path)
        }
        None => hunks::check_path(repo, &side.path),
    }
}

// Both sides inside the repository, each one a full hash, the disk or nothing, and at most one
// of them the disk.
pub fn check_pair(repo: &Path, pair: &RevisionPair) -> Result<(), HunkError> {
    check_side(repo, &pair.left)?;
    check_side(repo, &pair.right)?;
    if let Some(center) = &pair.center {
        check_side(repo, center)?;
        if is_local(center) || is_local(&pair.left) || is_local(&pair.right) {
            return Err(GitError::Io("a merge has no local side".to_string()).into());
        }
    }
    if is_local(&pair.left) && is_local(&pair.right) {
        return Err(GitError::Io("both sides are the local file".to_string()).into());
    }
    Ok(())
}

// What the diff window may be asked to show: paths inside the repository and at least one file.
pub fn check_request(repo: &Path, request: &DiffRequest) -> Result<(), HunkError> {
    match request {
        DiffRequest::Worktree { path } => worktree_diff::check_inside(repo, path),
        DiffRequest::Revisions {
            files, selected, ..
        } => {
            if files.is_empty() {
                return Err(GitError::Io("there are no files to compare".to_string()).into());
            }
            if selected.is_some_and(|index| index >= files.len()) {
                return Err(
                    GitError::Io("the pair to open on is not in the request".to_string()).into(),
                );
            }
            files.iter().try_for_each(|pair| check_pair(repo, pair))
        }
    }
}

fn read_side(repo: &Path, side: &SideRef) -> Result<LocalRead, HunkError> {
    match side.revision.as_deref() {
        Some(LOCAL) => read_local(repo, &side.path),
        Some(hash) => Ok(LocalRead {
            read: read_revision(repo, hash, &side.path)?,
            editable: false,
            note: None,
            hash: None,
        }),
        None => Ok(LocalRead {
            read: Read::whole(Side::Absent),
            editable: false,
            note: None,
            hash: None,
        }),
    }
}

fn is_text(read: &Read) -> bool {
    matches!(read.side, Side::Text { .. })
}

pub fn sides(repo: &Path, left: &SideRef, right: &SideRef) -> Result<RevisionSides, HunkError> {
    check_pair(
        repo,
        &RevisionPair {
            left: left.clone(),
            right: right.clone(),
            center: None,
        },
    )?;
    let old = read_side(repo, left)?;
    let new = read_side(repo, right)?;
    let comparable = |read: &Read| is_text(read) || read.side == Side::Absent;
    // Two previews would only compare where each one was cut: `DB:42` instead.
    let both_previews = old.read.preview && new.read.preview;
    let hunks = if comparable(&old.read) && comparable(&new.read) && !both_previews {
        hunks::compute(&old.read.bytes, &new.read.bytes)
    } else {
        Vec::new()
    };
    let binary = old.read.side == Side::Binary || new.read.side == Side::Binary;
    let identical = binary.then(|| old.read.oid.is_some() && old.read.oid == new.read.oid);
    // The local side is edited on whichever side it is (`MASTER_SIDE`), never as a preview.
    let local = if is_local(left) { &old } else { &new };
    let has_local = is_local(left) || is_local(right);
    let editable = has_local && local.editable && !local.read.preview;
    let note = local.note.clone();
    let hash = if has_local { local.hash.clone() } else { None };
    Ok(RevisionSides {
        old: old.read.side,
        new: new.read.side,
        hunks,
        editable,
        note,
        hash,
        conflicted: false,
        old_preview: old.read.preview,
        new_preview: new.read.preview,
        identical,
    })
}

// The hunks the local side would have with `content`, while it is being edited.
pub fn preview(
    repo: &Path,
    left: &SideRef,
    right: &SideRef,
    content: &str,
) -> Result<Vec<Hunk>, HunkError> {
    check_pair(
        repo,
        &RevisionPair {
            left: left.clone(),
            right: right.clone(),
            center: None,
        },
    )?;
    let (other, local_left) = match (is_local(left), is_local(right)) {
        (false, true) => (left, false),
        (true, false) => (right, true),
        _ => return Err(HunkError::NotEditable(right.path.clone())),
    };
    let other = read_side(repo, other)?.read;
    if !is_text(&other) && other.side != Side::Absent {
        return Ok(Vec::new());
    }
    Ok(if local_left {
        hunks::compute(content.as_bytes(), &other.bytes)
    } else {
        hunks::compute(&other.bytes, content.as_bytes())
    })
}

#[tauri::command]
pub async fn compare_sides(
    left: SideRef,
    right: SideRef,
    state: State<'_, AppState>,
) -> Result<RevisionSides, ApiError> {
    let repo = current_repo(&state)?;
    blocking_api(move || Ok(sides(&repo, &left, &right)?)).await
}

#[tauri::command]
pub async fn compare_preview(
    left: SideRef,
    right: SideRef,
    content: String,
    state: State<'_, AppState>,
) -> Result<Vec<Hunk>, ApiError> {
    let repo = current_repo(&state)?;
    blocking_api(move || Ok(preview(&repo, &left, &right, &content)?)).await
}

#[tauri::command]
pub async fn compare_local_state(
    path: String,
    state: State<'_, AppState>,
) -> Result<LocalState, ApiError> {
    let repo = current_repo(&state)?;
    blocking_api(move || Ok(local_state(&repo, &path)?)).await
}

// Without `revision`, HEAD.
#[tauri::command]
pub async fn compare_last_revision(
    revision: Option<String>,
    path: Option<String>,
    state: State<'_, AppState>,
) -> Result<Option<FileRevision>, ApiError> {
    let repo = current_repo(&state)?;
    blocking_api(move || {
        Ok(match revision {
            Some(revision) => last_revision(&repo, &revision, path.as_deref())?,
            None => match path {
                Some(path) => head_revision_of(&repo, &path)?,
                None => last_revision(&repo, "HEAD", None)?,
            },
        })
    })
    .await
}

#[tauri::command]
pub async fn compare_resolve(
    kind: RefKind,
    name: String,
    state: State<'_, AppState>,
) -> Result<Option<FileRevision>, ApiError> {
    let repo = current_repo(&state)?;
    blocking_api(move || Ok(resolve(&repo, kind, &name)?)).await
}

#[tauri::command]
pub async fn compare_current_revision(
    path: String,
    state: State<'_, AppState>,
) -> Result<Option<CurrentRevision>, ApiError> {
    let repo = current_repo(&state)?;
    blocking_api(move || Ok(current_revision(&repo, &path)?)).await
}

#[tauri::command]
pub async fn compare_file_revisions(
    path: String,
    state: State<'_, AppState>,
) -> Result<Vec<FileRevision>, ApiError> {
    let repo = current_repo(&state)?;
    blocking_api(move || Ok(file_revisions(&repo, &path)?)).await
}

#[tauri::command]
pub async fn compare_changes_with(
    revision: String,
    paths: Vec<String>,
    state: State<'_, AppState>,
) -> Result<Vec<ChangedFile>, ApiError> {
    let repo = current_repo(&state)?;
    blocking_api(move || Ok(changes_with(&repo, &revision, &paths)?)).await
}

#[tauri::command]
pub async fn compare_changes_between(
    left: String,
    right: String,
    paths: Vec<String>,
    state: State<'_, AppState>,
) -> Result<Vec<ChangedFile>, ApiError> {
    let repo = current_repo(&state)?;
    blocking_api(move || Ok(changes_between(&repo, &left, &right, &paths)?)).await
}

#[tauri::command]
pub async fn compare_changes_from_working_tree(
    revision: String,
    state: State<'_, AppState>,
) -> Result<Vec<ChangedFile>, ApiError> {
    let repo = current_repo(&state)?;
    blocking_api(move || Ok(changes_from_working_tree(&repo, &revision)?)).await
}

#[tauri::command]
pub async fn compare_tags(state: State<'_, AppState>) -> Result<Vec<String>, ApiError> {
    let repo = current_repo(&state)?;
    blocking_api(move || Ok(tags(&repo)?)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::MAX_DIFF_BYTES;
    use crate::test_support::TempRepo;

    const NFD_NAME: &str = "cafe\u{301}.txt";
    const NFC_NAME: &str = "caf\u{e9}.txt";

    fn local(path: &str) -> SideRef {
        SideRef {
            revision: Some(LOCAL.to_string()),
            path: path.to_string(),
            title: "Local".to_string(),
        }
    }

    fn at(hash: &str, path: &str) -> SideRef {
        SideRef {
            revision: Some(hash.to_string()),
            path: path.to_string(),
            title: hash.to_string(),
        }
    }

    fn text(side: &Side) -> &str {
        match side {
            Side::Text { content } => content,
            other => panic!("expected text: {other:?}"),
        }
    }

    fn paths(changes: &[ChangedFile]) -> Vec<(String, Option<String>, String)> {
        changes
            .iter()
            .map(|change| {
                (
                    change.path.clone(),
                    change.old_path.clone(),
                    change.status.clone(),
                )
            })
            .collect()
    }

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    // Puts an entry in the index with exactly these bytes as its name, whatever
    // `core.precomposeunicode` says.
    fn add_exact(repo: &TempRepo, name: &str, contents: &[u8]) {
        let scratch = repo.path().join(".scratch");
        fs::write(&scratch, contents).unwrap();
        let blob = repo.git(&["hash-object", "-w", ".scratch"]);
        fs::remove_file(scratch).unwrap();
        let info = format!("100644,{},{name}", blob.trim());
        repo.git(&[
            "-c",
            "core.precomposeunicode=false",
            "update-index",
            "--add",
            "--cacheinfo",
            &info,
        ]);
    }

    #[test]
    fn the_head_revision_is_the_last_commit_of_head_that_touched_the_file() {
        let repo = TempRepo::new();
        let first = repo.commit("a.txt", b"1\n", "one");
        repo.commit("b.txt", b"1\n", "other file");
        repo.git(&["checkout", "-q", "-b", "side"]);
        repo.commit("a.txt", b"side\n", "on side");
        repo.git(&["checkout", "-q", "main"]);

        let current = head_revision_of(repo.path(), "a.txt").unwrap().unwrap();

        assert_eq!(current.hash, first);
        assert_eq!(current.message, "one\n");
        assert_eq!(current.author, "Test");
        assert!(current.commit_time > 0);
        assert_eq!(head_revision_of(repo.path(), "missing.txt").unwrap(), None);
    }

    #[test]
    fn without_commits_there_is_no_head_revision_and_no_file_revisions() {
        let repo = TempRepo::new();
        fs::write(repo.path().join("a.txt"), "1\n").unwrap();

        assert_eq!(head_revision_of(repo.path(), "a.txt").unwrap(), None);
        assert!(file_revisions(repo.path(), "a.txt").unwrap().is_empty());
    }

    // `GitHistoryUtils.getCurrentRevisionDescription`: `log -n1 --all`, so a newer commit of another
    // branch is the current one; the committer goes after the author when it is someone else, and a
    // renamed file is looked up at the path of its last commit.
    #[test]
    fn current_revision_is_the_last_commit_on_any_branch() {
        let repo = TempRepo::new();
        // Older than the commit of `other`: `--all` walks by commit date from every branch.
        repo.commit_dated("a.txt", "one", "2020-01-01T00:00:00Z");
        repo.git(&["checkout", "-q", "-b", "other"]);
        fs::write(repo.path().join("a.txt"), "2\n").unwrap();
        repo.git(&["add", "a.txt"]);
        repo.git(&[
            "-c",
            "user.name=Committer",
            "commit",
            "-q",
            "--author=Author <author@example.com>",
            "-m",
            "two\n\nbody",
        ]);
        let two = repo.git(&["rev-parse", "HEAD"]).trim().to_string();
        repo.git(&["checkout", "-q", "main"]);

        let current = current_revision(repo.path(), "a.txt").unwrap().unwrap();

        assert_eq!(current.hash, two);
        assert_eq!(current.author, "Author (Committer)");
        assert_eq!(current.message, "two\n\nbody");
        assert!(current.commit_time > 0);

        repo.git(&["mv", "a.txt", "b.txt"]);
        let renamed = current_revision(repo.path(), "b.txt").unwrap().unwrap();
        assert_eq!(renamed.hash, two);
        assert_eq!(current_revision(repo.path(), "none.txt").unwrap(), None);
    }

    // A time git did not give as a number is not taken as 1970.
    #[test]
    fn a_current_revision_without_a_readable_time_is_an_error() {
        let hash = "a".repeat(40);
        let good = format!("{hash}\x001700000000\x00Ann\x00Ann\x00one\n\x00");
        assert_eq!(
            parse_current(good.as_bytes()).unwrap().unwrap().commit_time,
            1_700_000_000
        );
        let bad = format!("{hash}\x00soon\x00Ann\x00Ann\x00one\n\x00");
        assert!(matches!(
            parse_current(bad.as_bytes()),
            Err(GitError::Io(_))
        ));
        assert_eq!(parse_current(b"").unwrap(), None);
    }

    #[test]
    fn current_revision_names_the_author_once_when_they_committed() {
        let repo = TempRepo::new();
        let one = repo.commit("a.txt", b"1\n", "one");

        let current = current_revision(repo.path(), "a.txt").unwrap().unwrap();

        assert_eq!(
            (current.hash.as_str(), current.author.as_str()),
            (one.as_str(), "Test")
        );
        assert_eq!(current.message, "one");
    }

    #[test]
    fn file_revisions_are_the_commits_of_head_that_touched_the_file_newest_first() {
        let repo = TempRepo::new();
        let one = repo.commit("a.txt", b"1\n", "one");
        repo.commit("b.txt", b"1\n", "other file");
        let two = repo.commit("a.txt", b"2\n", "two\n\nbody");

        let revisions = file_revisions(repo.path(), "a.txt").unwrap();

        let hashes: Vec<&str> = revisions.iter().map(|rev| rev.hash.as_str()).collect();
        assert_eq!(hashes, [two.as_str(), one.as_str()]);
        assert_eq!(revisions[0].message, "two\n\nbody\n");
    }

    // `CompareWithSelectedRevisionAction` reads `GitHistoryProvider`'s history: `GitFileHistory`
    // without `fullHistory`, which follows a rename and gives each revision the path it had.
    #[test]
    fn file_revisions_follow_a_rename_with_the_path_of_each_revision() {
        let repo = TempRepo::new();
        let mut text: String = (1..=20).map(|line| format!("line {line}\n")).collect();
        let added = repo.commit("old.txt", text.as_bytes(), "add");
        repo.git(&["mv", "old.txt", "new.txt"]);
        repo.git(&["commit", "-q", "-m", "rename"]);
        let renamed = repo.git(&["rev-parse", "HEAD"]).trim().to_string();
        text.push_str("more\n");
        let edited = repo.commit("new.txt", text.as_bytes(), "edit");
        repo.commit("dir/x.txt", b"x\n", "folder");

        let revisions = file_revisions(repo.path(), "new.txt").unwrap();

        let found: Vec<(&str, Option<&str>)> = revisions
            .iter()
            .map(|rev| (rev.hash.as_str(), rev.path.as_deref()))
            .collect();
        assert_eq!(
            found,
            [
                (edited.as_str(), Some("new.txt")),
                (renamed.as_str(), Some("new.txt")),
                (added.as_str(), Some("old.txt")),
            ]
        );
        assert_eq!(revisions[2].message, "add\n");
        let folder = file_revisions(repo.path(), "dir").unwrap();
        assert_eq!(folder.len(), 1);
        assert_eq!(folder[0].path, None);
    }

    // `log.showSignature` on and signed commits: git prints the signature check on stdout before
    // each record, and none of it may get into a hash, a message or a path.
    #[test]
    fn signatures_stay_out_of_the_revisions() {
        let repo = TempRepo::new();
        repo.sign_commits();
        let one = repo.commit("old.txt", b"1\n2\n3\n4\n5\n6\n", "one");
        repo.git(&["mv", "old.txt", "a.txt"]);
        repo.git(&["commit", "-q", "-m", "rename"]);
        let renamed = repo.git(&["rev-parse", "HEAD"]).trim().to_string();
        let two = repo.commit("a.txt", b"1\n2\n3\n4\n5\n6\n7\n", "two");
        assert!(repo
            .git(&["log", "-1", "--format=%H"])
            .contains("No signature"));

        let head = head_revision_of(repo.path(), "a.txt").unwrap().unwrap();
        assert_eq!(
            (head.hash.as_str(), head.message.as_str()),
            (two.as_str(), "two\n")
        );
        let commit = last_revision(repo.path(), &one, None).unwrap().unwrap();
        assert_eq!(commit.hash, one);

        let current = current_revision(repo.path(), "a.txt").unwrap().unwrap();
        assert_eq!(
            (current.hash.as_str(), current.message.as_str()),
            (two.as_str(), "two")
        );

        let revisions = file_revisions(repo.path(), "a.txt").unwrap();
        let found: Vec<(&str, Option<&str>, &str)> = revisions
            .iter()
            .map(|rev| (rev.hash.as_str(), rev.path.as_deref(), rev.message.as_str()))
            .collect();
        assert_eq!(
            found,
            [
                (two.as_str(), Some("a.txt"), "two\n"),
                (renamed.as_str(), Some("a.txt"), "rename\n"),
                (one.as_str(), Some("old.txt"), "one\n"),
            ]
        );
    }

    #[test]
    fn changes_with_a_revision_compare_it_with_the_working_tree() {
        let repo = TempRepo::new();
        let base = repo.commit("a.txt", b"1\n", "one");
        repo.commit("dir/b.txt", b"1\n", "two");
        fs::write(repo.path().join("a.txt"), "changed\n").unwrap();
        fs::write(repo.path().join("untracked.txt"), "new\n").unwrap();

        let all = changes_with(repo.path(), &base, &[]).unwrap();
        let only_dir = changes_with(repo.path(), &base, &strings(&["dir"])).unwrap();

        assert_eq!(
            paths(&all),
            [
                ("a.txt".to_string(), None, "M".to_string()),
                ("dir/b.txt".to_string(), None, "A".to_string())
            ]
        );
        assert_eq!(
            paths(&only_dir),
            [("dir/b.txt".to_string(), None, "A".to_string())]
        );
    }

    // `GitBranchWorker.loadTotalDiff`: the working tree is the base, so what only the branch has is
    // added, what only the working tree has is deleted, and a rename goes from the local path.
    #[test]
    fn changes_from_the_working_tree_put_it_on_the_left() {
        let repo = TempRepo::new();
        repo.commit("kept.txt", b"1\n", "one");
        repo.commit("dir/old.txt", b"1\n2\n3\n4\n", "two");
        repo.git(&["checkout", "-q", "-b", "side"]);
        let side = repo.commit("only-side.txt", b"1\n", "three");
        repo.git(&["checkout", "-q", "-"]);
        repo.git(&["mv", "dir/old.txt", "dir/new.txt"]);
        repo.git(&["commit", "-q", "-m", "rename"]);
        fs::write(repo.path().join("kept.txt"), "changed\n").unwrap();

        let changes = changes_from_working_tree(repo.path(), &side).unwrap();

        let mut found = paths(&changes);
        found.sort();
        assert_eq!(found.len(), 3, "{found:?}");
        assert_eq!(found[0].0, "dir/old.txt");
        assert_eq!(found[0].1.as_deref(), Some("dir/new.txt"));
        assert!(found[0].2.starts_with('R'));
        assert_eq!(found[1], ("kept.txt".to_string(), None, "M".to_string()));
        assert_eq!(
            found[2],
            ("only-side.txt".to_string(), None, "A".to_string())
        );
        assert!(changes_from_working_tree(repo.path(), "--output=x").is_err());
        assert!(!repo.path().join("x").exists());
    }

    #[test]
    fn a_renamed_file_between_two_revisions_keeps_its_old_path() {
        let repo = TempRepo::new();
        let before = repo.commit("dir/old.txt", b"1\n2\n3\n4\n", "one");
        repo.git(&["mv", "dir/old.txt", "dir/new.txt"]);
        repo.git(&["commit", "-q", "-m", "rename"]);
        let after = repo.git(&["rev-parse", "HEAD"]).trim().to_string();

        let changes = changes_between(repo.path(), &before, &after, &[]).unwrap();

        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "dir/new.txt");
        assert_eq!(changes[0].old_path.as_deref(), Some("dir/old.txt"));
        assert!(changes[0].status.starts_with('R'));
    }

    #[test]
    fn a_revision_that_looks_like_an_option_is_refused() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");

        assert!(changes_with(repo.path(), "--output=x", &[]).is_err());
        assert!(changes_between(repo.path(), "-p", "HEAD", &[]).is_err());
        assert!(last_revision(repo.path(), "--all", None).is_err());
        assert!(resolve(repo.path(), RefKind::Branch, "-x")
            .unwrap()
            .is_none());
        assert!(!repo.path().join("x").exists());
    }

    #[test]
    fn branches_resolve_local_first_then_remote_and_tags_by_their_own_name() {
        let repo = TempRepo::new();
        let one = repo.commit("a.txt", b"1\n", "one");
        let two = repo.commit("a.txt", b"2\n", "two");
        repo.git(&["update-ref", "refs/remotes/origin/feature", &one]);
        repo.git(&["branch", "feature", &two]);
        repo.git(&["update-ref", "refs/remotes/origin/only", &one]);
        repo.git(&["tag", "v1", &one]);

        let hash = |kind, name| {
            resolve(repo.path(), kind, name)
                .unwrap()
                .map(|rev| rev.hash)
        };

        assert_eq!(hash(RefKind::Branch, "feature"), Some(two.clone()));
        assert_eq!(hash(RefKind::Branch, "origin/only"), Some(one.clone()));
        assert_eq!(hash(RefKind::Tag, "v1"), Some(one));
        assert_eq!(hash(RefKind::Branch, "missing"), None);
        assert_eq!(tags(repo.path()).unwrap(), ["v1"]);
    }

    #[test]
    fn the_local_state_says_whether_the_file_changed_and_names_the_head() {
        let repo = TempRepo::new();
        let first = repo.commit("a.txt", b"1\n", "one");
        repo.commit("b.txt", b"1\n", "two");

        let clean = local_state(repo.path(), "a.txt").unwrap();
        fs::write(repo.path().join("a.txt"), "2\n").unwrap();
        let changed = local_state(repo.path(), "a.txt").unwrap();
        fs::write(repo.path().join("new.txt"), "1\n").unwrap();
        let unversioned = local_state(repo.path(), "new.txt").unwrap();
        repo.git(&["checkout", "-q", "--detach", "HEAD"]);
        let detached = local_state(repo.path(), "a.txt").unwrap();

        assert!(!clean.modified);
        assert_eq!(
            clean.current.as_ref().map(|rev| rev.hash.as_str()),
            Some(first.as_str())
        );
        assert_eq!(clean.head_label, "main");
        assert_eq!(clean.root, repo.path().display().to_string());
        assert!(changed.modified);
        assert!(!unversioned.modified);
        assert_eq!(unversioned.current, None);
        let head = repo.git(&["rev-parse", "HEAD"]);
        assert_eq!(detached.head_label, &head[..8]);
    }

    #[test]
    fn a_revision_is_compared_with_the_local_file_and_the_local_side_is_edited_on_either_side() {
        let repo = TempRepo::new();
        let one = repo.commit("a.txt", b"1\n2\n3\n", "one");
        repo.commit("a.txt", b"1\n2\n", "two");
        fs::write(repo.path().join("a.txt"), "1\nlocal\n").unwrap();

        let shown = sides(repo.path(), &at(&one, "a.txt"), &local("a.txt")).unwrap();
        let swapped = sides(repo.path(), &local("a.txt"), &at(&one, "a.txt")).unwrap();

        assert_eq!(text(&shown.old), "1\n2\n3\n");
        assert_eq!(text(&shown.new), "1\nlocal\n");
        assert_eq!(shown.hunks.len(), 1);
        assert!(shown.editable);
        assert_eq!(
            shown.hash.as_deref(),
            Some(worktree_diff::content_hash(b"1\nlocal\n").as_str())
        );
        assert_eq!(text(&swapped.old), "1\nlocal\n");
        // G§0.9: an unmodified file against a newer revision puts the local side on the left, and
        // 241 still edits it (`MASTER_SIDE = LEFT`).
        assert!(swapped.editable);
        assert_eq!(swapped.hash, shown.hash);
        assert_eq!(
            preview(
                repo.path(),
                &local("a.txt"),
                &at(&one, "a.txt"),
                "1\n2\n3\n"
            )
            .unwrap(),
            []
        );
        assert_eq!(
            preview(repo.path(), &local("a.txt"), &at(&one, "a.txt"), "1\n")
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            preview(
                repo.path(),
                &at(&one, "a.txt"),
                &local("a.txt"),
                "1\n2\n3\n"
            )
            .unwrap(),
            []
        );
    }

    #[test]
    fn a_large_binary_is_compared_by_its_object_without_reading_the_disk_whole() {
        let repo = TempRepo::new();
        let mut large = vec![0u8; MAX_DIFF_BYTES as usize + 1024];
        large[100] = 7;
        let one = repo.commit("big.bin", &large, "big");
        let same = sides(repo.path(), &at(&one, "big.bin"), &local("big.bin")).unwrap();
        large[200] = 9;
        fs::write(repo.path().join("big.bin"), &large).unwrap();
        let changed = sides(repo.path(), &at(&one, "big.bin"), &local("big.bin")).unwrap();
        let text_against_binary = {
            fs::write(repo.path().join("big.bin"), "text\n").unwrap();
            sides(repo.path(), &at(&one, "big.bin"), &local("big.bin")).unwrap()
        };

        assert_eq!(same.new, Side::Binary);
        assert_eq!(same.identical, Some(true));
        assert_eq!(changed.identical, Some(false));
        assert_eq!(text_against_binary.identical, Some(false));
    }

    #[test]
    fn two_revisions_are_compared_without_editing() {
        let repo = TempRepo::new();
        let one = repo.commit("a.txt", b"1\n", "one");
        let two = repo.commit("a.txt", b"2\n", "two");

        let shown = sides(repo.path(), &at(&one, "a.txt"), &at(&two, "a.txt")).unwrap();
        let added = sides(
            repo.path(),
            &SideRef {
                revision: None,
                path: "a.txt".to_string(),
                title: String::new(),
            },
            &at(&two, "a.txt"),
        )
        .unwrap();

        assert_eq!(text(&shown.old), "1\n");
        assert_eq!(text(&shown.new), "2\n");
        assert!(!shown.editable);
        assert_eq!(shown.hash, None);
        assert_eq!(added.old, Side::Absent);
        assert_eq!(added.hunks.len(), 1);
    }

    #[test]
    fn the_local_side_saves_with_the_hash_check_and_refuses_a_changed_disk() {
        let repo = TempRepo::new();
        let one = repo.commit("a.txt", b"1\n", "one");
        fs::write(repo.path().join("a.txt"), "local\n").unwrap();
        let shown = sides(repo.path(), &at(&one, "a.txt"), &local("a.txt")).unwrap();
        let hash = shown.hash.unwrap();
        // Devin writes the file after the window read it.
        fs::write(repo.path().join("a.txt"), "devin\n").unwrap();

        let refused = worktree_diff::write(repo.path(), "a.txt", "typed\n", &hash);

        assert!(matches!(refused, Err(HunkError::Changed(_))), "{refused:?}");
        assert_eq!(
            fs::read_to_string(repo.path().join("a.txt")).unwrap(),
            "devin\n"
        );
        let fresh = sides(repo.path(), &at(&one, "a.txt"), &local("a.txt")).unwrap();
        let saved = worktree_diff::write(repo.path(), "a.txt", "typed\n", &fresh.hash.unwrap());
        assert!(saved.is_ok(), "{saved:?}");
        assert_eq!(
            fs::read_to_string(repo.path().join("a.txt")).unwrap(),
            "typed\n"
        );
    }

    #[test]
    fn a_large_or_binary_revision_shows_the_241_texts() {
        let repo = TempRepo::new();
        let line = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcde\n";
        let large = line.repeat((MAX_DIFF_BYTES as usize) / line.len() + 2);
        let big = repo.commit("big.txt", large.as_bytes(), "big");
        let png = [0x89, b'P', b'N', b'G', 0, 0, 0, 13, 1, 2, 3];
        let image = repo.commit("image.png", &png, "image");
        let mut other = png.to_vec();
        other.push(4);
        let changed = repo.commit("image.png", &other, "image again");

        let preview = sides(repo.path(), &at(&big, "big.txt"), &local("big.txt")).unwrap();
        let same = sides(repo.path(), &at(&changed, "image.png"), &local("image.png")).unwrap();
        let different = sides(
            repo.path(),
            &at(&image, "image.png"),
            &at(&changed, "image.png"),
        )
        .unwrap();

        assert!(preview.old_preview && preview.new_preview);
        assert!(text(&preview.old).len() <= PREVIEW_BYTES);
        assert!(large.starts_with(text(&preview.old)));
        assert!(!preview.editable);
        assert!(preview.hunks.is_empty());
        assert_eq!(same.old, Side::Binary);
        assert_eq!(same.new, Side::Binary);
        assert_eq!(same.identical, Some(true));
        assert_eq!(different.identical, Some(false));
    }

    #[test]
    fn compare_reaches_a_glob_named_and_nfd_file() {
        let repo = TempRepo::new();
        repo.git(&["config", "core.precomposeunicode", "true"]);
        for (name, contents) in [
            ("a.txt", "plain\n"),
            ("[a].txt", "brackets\n"),
            ("*.txt", "star\n"),
            ("-x.txt", "dash\n"),
            (":(glob)b.txt", "magic\n"),
            ("with space.txt", "space\n"),
        ] {
            fs::write(repo.path().join(name), contents).unwrap();
        }
        repo.git(&["add", "-A"]);
        add_exact(&repo, NFD_NAME, b"nfd\n");
        add_exact(&repo, NFC_NAME, b"nfc\n");
        repo.git(&["commit", "-q", "-m", "names"]);
        let head = repo.git(&["rev-parse", "HEAD"]).trim().to_string();
        fs::write(repo.path().join("a.txt"), "plain changed\n").unwrap();

        for (name, contents) in [
            ("[a].txt", "brackets\n"),
            ("*.txt", "star\n"),
            ("-x.txt", "dash\n"),
            (":(glob)b.txt", "magic\n"),
            ("with space.txt", "space\n"),
            (NFD_NAME, "nfd\n"),
            (NFC_NAME, "nfc\n"),
        ] {
            let shown = sides(repo.path(), &at(&head, name), &at(&head, name)).unwrap();
            assert_eq!(text(&shown.old), contents, "{name}");
            assert_eq!(
                head_revision_of(repo.path(), name)
                    .unwrap()
                    .map(|rev| rev.hash),
                Some(head.clone()),
                "{name}"
            );
            assert_eq!(
                file_revisions(repo.path(), name).unwrap().len(),
                1,
                "{name}"
            );
        }
        for name in ["[a].txt", "*.txt", "-x.txt", "with space.txt"] {
            assert!(
                changes_with(repo.path(), &head, &strings(&[name]))
                    .unwrap()
                    .is_empty(),
                "{name}"
            );
        }
        // Both forms are only in the index (APFS keeps one name for both on disk): each one is
        // deleted on disk, and the exact name finds only itself.
        for name in [NFD_NAME, NFC_NAME] {
            assert_eq!(
                paths(&changes_with(repo.path(), &head, &strings(&[name])).unwrap()),
                [(name.to_string(), None, "D".to_string())]
            );
        }
        // `a.txt` changed: only a literal `a.txt` finds it, never `[a].txt` or `*.txt`.
        for name in ["[a].txt", "*.txt", ":(glob)b.txt"] {
            assert!(
                changes_with(repo.path(), &head, &strings(&[name]))
                    .unwrap()
                    .is_empty(),
                "{name}"
            );
        }
        assert_eq!(
            paths(&changes_with(repo.path(), &head, &strings(&["a.txt"])).unwrap()),
            [("a.txt".to_string(), None, "M".to_string())]
        );
    }

    // The pair to open on travels with the request, and a request without it has none.
    #[test]
    fn a_revisions_request_keeps_the_pair_it_opens_on() {
        let pair = r#"{"left":{"revision":null,"path":"a","title":"A"},"right":{"revision":"local","path":"a","title":"B"}}"#;
        let with: DiffRequest = serde_json::from_str(&format!(
            r#"{{"kind":"revisions","title":"t","files":[{pair},{pair}],"selected":1}}"#
        ))
        .unwrap();
        let without: DiffRequest = serde_json::from_str(&format!(
            r#"{{"kind":"revisions","title":"t","files":[{pair}]}}"#
        ))
        .unwrap();

        assert!(matches!(
            with,
            DiffRequest::Revisions {
                selected: Some(1),
                ..
            }
        ));
        assert!(matches!(
            without,
            DiffRequest::Revisions { selected: None, .. }
        ));
        assert!(!serde_json::to_string(&without)
            .unwrap()
            .contains("selected"));
    }

    #[test]
    fn a_request_without_files_or_with_a_path_outside_is_refused() {
        let repo = TempRepo::new();
        let one = repo.commit("a.txt", b"1\n", "one");
        let revisions = |files: Vec<RevisionPair>| DiffRequest::Revisions {
            files,
            title: "a.txt".to_string(),
            selected: None,
        };
        let pair = RevisionPair {
            left: at(&one, "a.txt"),
            right: local("a.txt"),
            center: None,
        };
        let outside = RevisionPair {
            left: at(&one, "a.txt"),
            right: local("../a.txt"),
            center: None,
        };
        let merged = RevisionPair {
            left: at(&one, "a.txt"),
            right: at(&one, "a.txt"),
            center: Some(at(&one, "a.txt")),
        };
        let merged_with_local = RevisionPair {
            center: Some(local("a.txt")),
            ..merged.clone()
        };

        assert!(check_request(repo.path(), &revisions(vec![pair.clone()])).is_ok());
        assert!(check_request(repo.path(), &revisions(vec![])).is_err());
        // The pair to open on must be one of the request's.
        let opened_on = |selected: usize| DiffRequest::Revisions {
            files: vec![pair.clone(), pair.clone()],
            title: "a.txt".to_string(),
            selected: Some(selected),
        };
        assert!(check_request(repo.path(), &opened_on(1)).is_ok());
        assert!(check_request(repo.path(), &opened_on(2)).is_err());
        assert!(check_request(repo.path(), &revisions(vec![pair, outside])).is_err());
        assert!(check_request(repo.path(), &revisions(vec![merged])).is_ok());
        assert!(check_request(repo.path(), &revisions(vec![merged_with_local])).is_err());
        let worktree = |path: &str| DiffRequest::Worktree {
            path: path.to_string(),
        };
        assert!(check_request(repo.path(), &worktree("a.txt")).is_ok());
        assert!(check_request(repo.path(), &worktree("../a.txt")).is_err());
        let json = serde_json::to_value(revisions(Vec::new())).unwrap();
        assert_eq!(json["kind"], "revisions");
    }

    #[test]
    fn a_pair_is_refused_when_a_side_leaves_the_repository_or_is_not_a_hash() {
        let repo = TempRepo::new();
        let one = repo.commit("a.txt", b"1\n", "one");
        let pair = |left: SideRef, right: SideRef| RevisionPair {
            left,
            right,
            center: None,
        };

        assert!(check_pair(repo.path(), &pair(at(&one, "a.txt"), local("a.txt"))).is_ok());
        assert!(check_pair(repo.path(), &pair(at(&one, "../x"), local("a.txt"))).is_err());
        assert!(check_pair(repo.path(), &pair(at(&one, "a.txt"), local("/etc/hosts"))).is_err());
        assert!(check_pair(repo.path(), &pair(at("main", "a.txt"), local("a.txt"))).is_err());
        assert!(check_pair(repo.path(), &pair(local("a.txt"), local("a.txt"))).is_err());
        assert!(sides(repo.path(), &at("--all", "a.txt"), &local("a.txt")).is_err());
    }
}
